// SPDX-License-Identifier: MIT OR Apache-2.0

//! #449 — Phase 3 of the Windows direct-install adapter.
//!
//! Raw-disk I/O via `windows-rs` — the production Rust path for
//! staging the signed chain onto a freshly-formatted ESP. Uses direct
//! I/O (`FILE_FLAG_NO_BUFFERING | FILE_FLAG_WRITE_THROUGH`) so flash
//! duration tracks size cleanly without surprise page-cache flushing
//! at the end — same semantics as Linux's `O_DIRECT`.
//!
//! ## Why `windows-rs`, not `.NET FileStream`
//!
//! Win11 prototyping on 2026-04-23 validated `[System.IO.FileStream]`
//! works — but the managed path hides the flush-at-end stall and
//! adds a PowerShell-spawn per write call. The `windows-rs` crate
//! maps each Win32 function directly to a Rust `unsafe fn`, giving
//! us a bounded dependency set (`Win32_Foundation` +
//! `Win32_Storage_FileSystem` + `Win32_System_Ioctl`) and identical
//! direct-I/O behavior to the Linux flasher.
//!
//! ## Safety invariants (per #449)
//!
//! 1. **Refuse disk 0** — OS boot drive, checked at the pure-fn
//!    layer before any handle is opened.
//! 2. **Elevation required** — raw-disk writes refuse without
//!    Administrator. Caller must gate via
//!    [`crate::windows_direct_install::preflight`] first.
//! 3. **`FILE_SHARE_NONE`** — exclusive access. If Windows Defender
//!    real-time scan or BitLocker has the disk open, fail with the
//!    specific Win32 error translated by [`translate_win32_error`].
//! 4. **`FSCTL_LOCK_VOLUME` before write** — returns
//!    `ERROR_ACCESS_DENIED` on BitLocker-protected; surface that
//!    cleanly so the operator sees the real reason.
//! 5. **`FSCTL_DISMOUNT_VOLUME` after write** — forces Windows to
//!    re-read the fresh partition table. Without this, cached
//!    partition info can still reflect pre-format state.
//! 6. **Sector-aligned I/O** — `FILE_FLAG_NO_BUFFERING` requires
//!    offset + size + buffer alignment to the sector size (typically
//!    512 B, but 4 KiB Advanced Format is increasingly common).
//!    Queried at runtime from `IOCTL_DISK_GET_DRIVE_GEOMETRY_EX`.
//!
//! ## Dead-code allow
//!
//! Pure-fn helpers + Windows-only syscalls are scaffolding awaiting
//! CLI wiring in the `flash --direct-install` integration phase. The
//! unit tests keep the pure-fn side exercised, but no runtime caller
//! exists yet. Module-scoped `allow(dead_code)` for the same reason
//! `partition` and `format` use it.

#![allow(dead_code)]

use std::path::PathBuf;

use crate::windows_direct_install::partition::PartitionBuildError;

/// Common sector sizes. Windows drives are typically 512 B or
/// 4096 B (Advanced Format). We query at runtime on Windows; the
/// pure-fn layer accepts any power-of-two ≤ 65536.
pub(crate) const DEFAULT_SECTOR_BYTES: u32 = 512;
pub(crate) const ADVANCED_FORMAT_SECTOR_BYTES: u32 = 4096;

/// The minimum direct-I/O chunk we write at a time. Matches the
/// Linux flasher's 4 MiB chunk size — gives stable throughput on
/// typical USB 2.0/3.0 sticks without blowing out the kernel's
/// page-reclaim pressure. Must be sector-aligned (trivially true
/// for a 4 MiB multiple of any plausible sector size).
pub(crate) const WRITE_CHUNK_BYTES: usize = 4 * 1024 * 1024;

/// Canonical ESP file list — the 6 files the signed chain writes.
/// Order is the ESP directory layout the rescue-env bootloader
/// reads; callers supply the host-side source paths via
/// [`EspStagingSources`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EspFile {
    /// `/EFI/BOOT/BOOTX64.EFI` — signed shim (Microsoft UEFI CA root).
    ShimX64,
    /// `/EFI/BOOT/grubx64.efi` — signed grub (Debian CA).
    GrubX64,
    /// `/EFI/BOOT/mmx64.efi` — MOK manager (Debian CA).
    MmX64,
    /// `/EFI/BOOT/grub.cfg` — grub config entry point.
    GrubCfg,
    /// `/vmlinuz` — signed kernel (Debian CA).
    Vmlinuz,
    /// `/initramfs.cpio.gz` — the rescue-env initramfs.
    Initramfs,
}

impl EspFile {
    /// Canonical ESP-relative path for this file, using `/` as the
    /// separator. The Win32 FS driver accepts `/` in FAT32 paths.
    pub(crate) fn esp_path(self) -> &'static str {
        match self {
            Self::ShimX64 => "/EFI/BOOT/BOOTX64.EFI",
            Self::GrubX64 => "/EFI/BOOT/grubx64.efi",
            Self::MmX64 => "/EFI/BOOT/mmx64.efi",
            Self::GrubCfg => "/EFI/BOOT/grub.cfg",
            Self::Vmlinuz => "/vmlinuz",
            Self::Initramfs => "/initramfs.cpio.gz",
        }
    }

    /// All 6 files in staging order.
    pub(crate) const ALL: [Self; 6] = [
        Self::ShimX64,
        Self::GrubX64,
        Self::MmX64,
        Self::GrubCfg,
        Self::Vmlinuz,
        Self::Initramfs,
    ];
}

/// Host-side source paths for each ESP file the Windows flasher
/// will stage. Constructed by the CLI-integration phase; consumed
/// by [`stage_esp`].
#[derive(Debug, Clone)]
pub(crate) struct EspStagingSources {
    pub(crate) shim_x64: PathBuf,
    pub(crate) grub_x64: PathBuf,
    pub(crate) mm_x64: PathBuf,
    pub(crate) grub_cfg: PathBuf,
    pub(crate) vmlinuz: PathBuf,
    pub(crate) initramfs: PathBuf,
}

impl EspStagingSources {
    /// Lookup a source path by [`EspFile`]. Used by stage_esp's
    /// iteration over [`EspFile::ALL`].
    pub(crate) fn path_for(&self, f: EspFile) -> &std::path::Path {
        match f {
            EspFile::ShimX64 => &self.shim_x64,
            EspFile::GrubX64 => &self.grub_x64,
            EspFile::MmX64 => &self.mm_x64,
            EspFile::GrubCfg => &self.grub_cfg,
            EspFile::Vmlinuz => &self.vmlinuz,
            EspFile::Initramfs => &self.initramfs,
        }
    }
}

/// Round `bytes` up to the next multiple of `sector_bytes`.
/// `sector_bytes` must be a non-zero power of two (asserted by a
/// debug assertion — callers get it from IOCTL_DISK_GET_DRIVE_GEOMETRY).
/// Returns the rounded value; overflow on the final sector saturates
/// to `u64::MAX`.
pub(crate) fn round_up_to_sector(bytes: u64, sector_bytes: u32) -> u64 {
    debug_assert!(
        sector_bytes > 0 && sector_bytes.is_power_of_two(),
        "sector size must be a non-zero power of two"
    );
    let s = u64::from(sector_bytes);
    let mask = s - 1;
    bytes.saturating_add(mask) & !mask
}

/// Whether a given sector size is one we'll accept from Windows'
/// `DISK_GEOMETRY_EX` query. The Win32 API has returned values as
/// odd as 1 B on buggy USB enclosures; refuse anything that's not a
/// power of two between 512 B and 64 KiB inclusive.
pub(crate) fn is_plausible_sector_size(sector_bytes: u32) -> bool {
    sector_bytes > 0 && sector_bytes.is_power_of_two() && (512..=65_536).contains(&sector_bytes)
}

/// Classify a Win32 error code into an English sentence the CLI can
/// surface directly. Only covers the codes that commonly fire on the
/// raw-disk path — everything else falls through to a generic
/// `Win32 error NNN` string.
///
/// The numeric codes are stable Win32 constants; listing them in
/// source keeps the pure-fn layer unit-testable on Linux without
/// pulling the `windows` crate.
pub(crate) fn translate_win32_error(code: u32) -> String {
    match code {
        1 => "ERROR_INVALID_FUNCTION (1): Windows rejected the IOCTL — \
              the target is not a raw-disk handle, or its driver lacks \
              the requested operation."
            .to_string(),
        5 => "ERROR_ACCESS_DENIED (5): the volume is locked by another \
              process or BitLocker. Close Windows Defender real-time \
              scan of the target, or decrypt the volume first."
            .to_string(),
        6 => "ERROR_INVALID_HANDLE (6): the disk handle is closed or \
              invalid — usually means a previous operation left the \
              handle in a stale state."
            .to_string(),
        19 => "ERROR_WRITE_PROTECT (19): the media is write-protected. \
               Check for a physical write-protect switch on the stick."
            .to_string(),
        21 => "ERROR_NOT_READY (21): the device is not ready — the \
               operator may have pulled the stick, or Windows hasn't \
               finished enumerating it after insertion."
            .to_string(),
        32 => "ERROR_SHARING_VIOLATION (32): another process has the \
               disk open in a conflicting mode. Windows Defender \
               real-time scan is the usual culprit."
            .to_string(),
        87 => "ERROR_INVALID_PARAMETER (87): an argument to the Win32 \
               call was rejected — usually a sector-alignment miss on \
               a buffered-I/O handle."
            .to_string(),
        1224 => "ERROR_USER_MAPPED_FILE (1224): a memory-mapped file \
                 blocks the operation. Close any Explorer preview or \
                 antivirus handle on the target."
            .to_string(),
        other => format!("Win32 error {other}"),
    }
}

/// Pure-fn pre-write validation: the caller asked to write
/// `total_bytes` at `offset`, both relative to the raw physical
/// device. Returns `Err` if the combination would violate direct-I/O
/// alignment rules; otherwise returns the number of sector-aligned
/// chunks the write will produce.
pub(crate) fn plan_write(
    offset: u64,
    total_bytes: u64,
    sector_bytes: u32,
) -> Result<WritePlan, WritePlanError> {
    if !is_plausible_sector_size(sector_bytes) {
        return Err(WritePlanError::ImplausibleSectorSize(sector_bytes));
    }
    let s = u64::from(sector_bytes);
    if offset % s != 0 {
        return Err(WritePlanError::OffsetNotSectorAligned {
            offset,
            sector_bytes,
        });
    }
    let aligned_total = round_up_to_sector(total_bytes, sector_bytes);
    let chunk_bytes = WRITE_CHUNK_BYTES as u64;
    // We want chunks that are sector-aligned; WRITE_CHUNK_BYTES = 4 MiB
    // is divisible by any plausible sector size (512/1024/2048/4096/…
    // up to 65 KiB), so this division is exact. Guard it anyway — a
    // future tweak that makes the chunk size non-power-of-two would
    // otherwise silently round down.
    if chunk_bytes % s != 0 {
        return Err(WritePlanError::ChunkNotSectorAligned {
            chunk_bytes: WRITE_CHUNK_BYTES,
            sector_bytes,
        });
    }
    let chunks = u64::try_from(aligned_total.div_ceil(chunk_bytes)).unwrap_or(u64::MAX);
    Ok(WritePlan {
        offset,
        aligned_total,
        sector_bytes,
        chunks,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WritePlan {
    pub(crate) offset: u64,
    pub(crate) aligned_total: u64,
    pub(crate) sector_bytes: u32,
    pub(crate) chunks: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WritePlanError {
    ImplausibleSectorSize(u32),
    OffsetNotSectorAligned {
        offset: u64,
        sector_bytes: u32,
    },
    ChunkNotSectorAligned {
        chunk_bytes: usize,
        sector_bytes: u32,
    },
}

impl std::fmt::Display for WritePlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ImplausibleSectorSize(s) => write!(
                f,
                "sector size {s} is not a power-of-two in [512, 65536] — \
                 refuse to dispatch raw I/O"
            ),
            Self::OffsetNotSectorAligned {
                offset,
                sector_bytes,
            } => write!(
                f,
                "offset {offset} is not a multiple of sector size {sector_bytes} — \
                 FILE_FLAG_NO_BUFFERING requires alignment"
            ),
            Self::ChunkNotSectorAligned {
                chunk_bytes,
                sector_bytes,
            } => write!(
                f,
                "write chunk size {chunk_bytes} is not a multiple of sector size \
                 {sector_bytes} — future-proof guard against rebasing WRITE_CHUNK_BYTES"
            ),
        }
    }
}

/// Refuse disk 0 — same defense-in-depth gate the `partition` module
/// enforces. Raw-disk writes against `\\.\PhysicalDrive0` almost
/// always mean "operator typo" on Windows since the OS boot drive
/// is always Disk 0.
pub(crate) fn check_not_boot_drive(physical_drive: u32) -> Result<(), PartitionBuildError> {
    if physical_drive == 0 {
        return Err(PartitionBuildError::BootDriveRefused);
    }
    Ok(())
}

// ---- Windows-only destructive side ----------------------------------
//
// Each fn below compiles only on target_os = "windows". They're
// pre-stubbed to return an "unimplemented" error against the public
// contract — the follow-up PR turns on the actual `unsafe`
// `windows::Win32::*` syscalls behind a narrow `#[allow(unsafe_code)]`
// annotation and flips the return paths from Err(...) to the real
// implementations. This split keeps #449 reviewable without a Windows
// dev environment: pure-fn math + contract tests land first, raw I/O
// lands in the wiring PR where a reviewer with WinDbg can iterate on
// real hardware.
//
// The `windows` crate dep is still pulled in on target_os = "windows"
// (see Cargo.toml) so the CI cross-compile validates the pin + feature
// list up-front — no surprise feature flip in the wiring PR.

/// Write `src` bytes to `\\.\PhysicalDriveN` starting at `offset`.
/// Direct I/O (`FILE_FLAG_NO_BUFFERING | FILE_FLAG_WRITE_THROUGH`),
/// `FILE_SHARE_NONE` exclusive access, volume-locked for the write
/// duration, volume dismounted afterward to force Windows to re-read
/// the on-disk partition table.
///
/// Refuses `physical_drive == 0` unconditionally via the pure-fn
/// [`check_not_boot_drive`] gate — that refusal is already tested on
/// Linux.
#[cfg(target_os = "windows")]
pub(crate) fn write_bytes_to_physical_drive(
    physical_drive: u32,
    src: &std::path::Path,
    offset: u64,
) -> Result<(), String> {
    check_not_boot_drive(physical_drive).map_err(|e| format!("raw_write: {e}"))?;
    let _ = (src, offset); // silence unused-warnings until the wiring PR
    Err(format!(
        "raw_write::write_bytes_to_physical_drive: not yet wired \
         (#449 pure-fn scaffold only; the CreateFileW + FSCTL_LOCK_VOLUME \
          + WriteFile path lands in the wiring PR)"
    ))
}

/// Stage all 6 ESP files from `sources` onto the freshly-formatted
/// ESP of `physical_drive`. Windows-only. Each file goes through the
/// volume GUID path (`\\.\Volume{...}`) rather than mcopy — we rely
/// on Windows' native FAT32 FS driver for file creation + write.
#[cfg(target_os = "windows")]
pub(crate) fn stage_esp(physical_drive: u32, sources: &EspStagingSources) -> Result<(), String> {
    check_not_boot_drive(physical_drive).map_err(|e| format!("stage_esp: {e}"))?;
    // Source-existence check stays in the scaffold — it's a host-fs
    // operation with no unsafe involved, and lets the wiring PR's
    // first "did I get the plumbing right?" smoke test start from a
    // known-good precondition.
    for esp_file in EspFile::ALL {
        let src = sources.path_for(esp_file);
        if !src.is_file() {
            return Err(format!(
                "stage_esp: source missing for {}: {}",
                esp_file.esp_path(),
                src.display()
            ));
        }
    }
    Err("raw_write::stage_esp: not yet wired (#449 pure-fn scaffold only)".to_string())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn round_up_is_identity_on_aligned_values() {
        assert_eq!(round_up_to_sector(0, 512), 0);
        assert_eq!(round_up_to_sector(512, 512), 512);
        assert_eq!(round_up_to_sector(4096, 4096), 4096);
        assert_eq!(round_up_to_sector(4 * 1024 * 1024, 4096), 4 * 1024 * 1024);
    }

    #[test]
    fn round_up_rounds_up_non_aligned_values() {
        assert_eq!(round_up_to_sector(1, 512), 512);
        assert_eq!(round_up_to_sector(513, 512), 1024);
        assert_eq!(round_up_to_sector(4097, 4096), 8192);
    }

    #[test]
    fn round_up_saturates_on_overflow() {
        // Values near u64::MAX saturate rather than wrap — a wrap-
        // around would silently round DOWN, which is unsafe for
        // write-sized arithmetic.
        let near_max = u64::MAX - 10;
        let rounded = round_up_to_sector(near_max, 512);
        assert_eq!(rounded, u64::MAX - 511);
    }

    #[test]
    fn is_plausible_accepts_common_sector_sizes() {
        for s in [512u32, 1024, 2048, 4096, 8192, 16_384, 32_768, 65_536] {
            assert!(is_plausible_sector_size(s), "expected {s} to be plausible");
        }
    }

    #[test]
    fn is_plausible_rejects_bogus_sector_sizes() {
        for s in [0u32, 1, 2, 256, 500, 1000, 4097, 131_072, u32::MAX] {
            assert!(!is_plausible_sector_size(s), "expected {s} to be rejected");
        }
    }

    #[test]
    fn plan_write_rejects_bogus_sector_size() {
        let err = plan_write(0, 1024, 1).unwrap_err();
        assert!(matches!(err, WritePlanError::ImplausibleSectorSize(1)));
    }

    #[test]
    fn plan_write_rejects_offset_not_sector_aligned() {
        let err = plan_write(513, 1024, 512).unwrap_err();
        assert!(matches!(
            err,
            WritePlanError::OffsetNotSectorAligned { offset: 513, .. }
        ));
    }

    #[test]
    fn plan_write_computes_chunks_for_whole_image() {
        // A 2 GiB image at offset 0 with 512-byte sectors produces
        // 512 × 4 MiB chunks.
        let plan = plan_write(0, 2 * 1024 * 1024 * 1024, 512).unwrap();
        assert_eq!(plan.offset, 0);
        assert_eq!(plan.aligned_total, 2 * 1024 * 1024 * 1024);
        assert_eq!(plan.chunks, 512);
    }

    #[test]
    fn plan_write_rounds_up_partial_final_chunk() {
        // 4 MiB + 1 byte rounds up to 4 MiB + 1 sector, which still
        // fits in 1 chunk for 512-byte sectors, but 2 chunks for
        // accounting purposes since the 1 extra byte forces a
        // second buffered read.
        let plan = plan_write(0, 4 * 1024 * 1024 + 1, 512).unwrap();
        // Aligned total = 4 MiB + 512 B (one more sector).
        assert_eq!(plan.aligned_total, 4 * 1024 * 1024 + 512);
        // 4 MiB chunk + 1 chunk containing just the trailing sector.
        assert_eq!(plan.chunks, 2);
    }

    #[test]
    fn esp_file_paths_match_boot_layout() {
        // The canonical 6 files under the ESP. Ordering matches
        // EspFile::ALL which stages in this order.
        assert_eq!(EspFile::ShimX64.esp_path(), "/EFI/BOOT/BOOTX64.EFI");
        assert_eq!(EspFile::GrubX64.esp_path(), "/EFI/BOOT/grubx64.efi");
        assert_eq!(EspFile::MmX64.esp_path(), "/EFI/BOOT/mmx64.efi");
        assert_eq!(EspFile::GrubCfg.esp_path(), "/EFI/BOOT/grub.cfg");
        assert_eq!(EspFile::Vmlinuz.esp_path(), "/vmlinuz");
        assert_eq!(EspFile::Initramfs.esp_path(), "/initramfs.cpio.gz");
    }

    #[test]
    fn esp_file_all_has_six_entries_in_staging_order() {
        assert_eq!(EspFile::ALL.len(), 6);
        // ShimX64 must come first — bootloader entry point.
        assert_eq!(EspFile::ALL[0], EspFile::ShimX64);
        // Initramfs last — typically the largest payload, writing it
        // last means any mid-flash interrupt loses only the initramfs
        // and the operator can retry without reflashing the small
        // boot chain.
        assert_eq!(EspFile::ALL[5], EspFile::Initramfs);
    }

    #[test]
    fn staging_sources_lookup_returns_each_path() {
        let sources = EspStagingSources {
            shim_x64: PathBuf::from("/a/shim"),
            grub_x64: PathBuf::from("/a/grub"),
            mm_x64: PathBuf::from("/a/mm"),
            grub_cfg: PathBuf::from("/a/cfg"),
            vmlinuz: PathBuf::from("/a/vmlinuz"),
            initramfs: PathBuf::from("/a/initramfs"),
        };
        assert_eq!(
            sources.path_for(EspFile::ShimX64),
            std::path::Path::new("/a/shim")
        );
        assert_eq!(
            sources.path_for(EspFile::Vmlinuz),
            std::path::Path::new("/a/vmlinuz")
        );
        assert_eq!(
            sources.path_for(EspFile::Initramfs),
            std::path::Path::new("/a/initramfs")
        );
    }

    #[test]
    fn check_not_boot_drive_refuses_disk_zero() {
        assert_eq!(
            check_not_boot_drive(0),
            Err(PartitionBuildError::BootDriveRefused)
        );
    }

    #[test]
    fn check_not_boot_drive_accepts_non_zero_drives() {
        for n in [1u32, 2, 3, 5, 7, 15, 99] {
            assert!(check_not_boot_drive(n).is_ok());
        }
    }

    #[test]
    fn translate_win32_error_maps_common_codes_with_prose() {
        for code in [1u32, 5, 6, 19, 21, 32, 87, 1224] {
            let msg = translate_win32_error(code);
            assert!(
                msg.contains(&format!("({code})")),
                "message for {code} must include the numeric code: {msg}"
            );
        }
    }

    #[test]
    fn translate_win32_error_falls_through_to_generic_on_unknown() {
        let msg = translate_win32_error(9999);
        assert_eq!(msg, "Win32 error 9999");
    }

    #[test]
    fn translate_win32_error_access_denied_hints_at_bitlocker() {
        // ERROR_ACCESS_DENIED is the most common raw-disk failure on
        // Windows — operator hit-rate goes up dramatically when the
        // message names BitLocker as a likely cause.
        let msg = translate_win32_error(5);
        assert!(msg.contains("BitLocker"));
        assert!(msg.contains("Defender"));
    }

    #[test]
    fn write_plan_error_display_names_the_offender() {
        let e = WritePlanError::OffsetNotSectorAligned {
            offset: 513,
            sector_bytes: 512,
        };
        let msg = format!("{e}");
        assert!(msg.contains("513"));
        assert!(msg.contains("512"));
    }
}
