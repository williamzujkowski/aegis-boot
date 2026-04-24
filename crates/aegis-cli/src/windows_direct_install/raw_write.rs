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
//!    real-time scan or `BitLocker` has the disk open, fail with the
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
    /// Lookup a source path by [`EspFile`]. Used by [`stage_esp`]'s
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
/// debug assertion — callers get it from `IOCTL_DISK_GET_DRIVE_GEOMETRY`).
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
    let chunks = aligned_total.div_ceil(chunk_bytes);
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
// #484 wires the raw-disk I/O path: `CreateFileW` with direct-I/O flags,
// `IOCTL_DISK_GET_DRIVE_GEOMETRY_EX` for the runtime sector size,
// `FSCTL_LOCK_VOLUME` before write, sector-aligned `WriteFile` loop,
// `FSCTL_DISMOUNT_VOLUME` after write, `CloseHandle` cleanup via RAII.
//
// Unsafe is narrow: each syscall site carries its own
// `#[allow(unsafe_code)]` annotation with a documented invariant
// comment. The workspace-level `unsafe_code = "deny"` catches any
// unannotated slip.

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

    let mut src_file = std::fs::File::open(src)
        .map_err(|e| format!("raw_write: open source {}: {e}", src.display()))?;
    let total_bytes = src_file
        .metadata()
        .map_err(|e| format!("raw_write: stat source {}: {e}", src.display()))?
        .len();

    let handle = sys::open_physical_drive(physical_drive)?;
    let sector_bytes = sys::query_sector_size(&handle)?;
    let plan = plan_write(offset, total_bytes, sector_bytes)
        .map_err(|e| format!("raw_write: plan rejected: {e}"))?;

    // Lock before write: refuses on BitLocker + surfaces sharing
    // conflicts (Defender real-time scan) as ACCESS_DENIED.
    sys::lock_volume(&handle)?;

    // Ensure the volume is dismounted even if the write loop errors —
    // otherwise Windows caches pre-format partition info and the next
    // operator sees a stale layout.
    let write_result = sys::write_all_sector_aligned(&handle, &mut src_file, &plan);

    // Always attempt dismount — errors here are logged but don't
    // mask the primary write error.
    let dismount_result = sys::dismount_volume(&handle);

    write_result?;
    dismount_result?;
    Ok(())
}

/// Stage all 6 ESP files from `sources` onto the freshly-formatted
/// ESP of `physical_drive`. Windows-only. Resolves the volume GUID for
/// partition 1 of the target drive via `FindFirstVolumeW` +
/// `IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS`, then uses the FAT32 FS
/// driver for file creation + write — no direct-I/O semantics needed
/// since the payloads are small (a few MiB total).
#[cfg(target_os = "windows")]
pub(crate) fn stage_esp(physical_drive: u32, sources: &EspStagingSources) -> Result<(), String> {
    check_not_boot_drive(physical_drive).map_err(|e| format!("stage_esp: {e}"))?;

    // Source-existence precondition — fails fast before any
    // destructive action touches the ESP.
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

    let volume_guid_path = sys::find_esp_volume_guid(physical_drive)?;

    for esp_file in EspFile::ALL {
        let src = sources.path_for(esp_file);
        // Volume GUID path ends with a trailing `\`; EspFile::esp_path
        // begins with `/`. Strip the leading `/` to build
        // `\\?\Volume{GUID}\EFI\BOOT\BOOTX64.EFI`.
        let rel = esp_file
            .esp_path()
            .trim_start_matches('/')
            .replace('/', "\\");
        let dst_path = format!("{volume_guid_path}{rel}");
        sys::copy_file_to_volume(src, &dst_path)
            .map_err(|e| format!("stage_esp: {} → {dst_path}: {e}", src.display()))?;
    }
    Ok(())
}

#[cfg(target_os = "windows")]
mod sys {
    //! Narrow Win32 FFI for [`super::write_bytes_to_physical_drive`] and
    //! [`super::stage_esp`]. Each `unsafe` call carries its own
    //! `#[allow(unsafe_code)]` with a documented safety invariant; the
    //! workspace lint would otherwise refuse to compile.
    //!
    //! RAII wrappers close handles + free aligned buffers on drop so
    //! early returns on write-loop errors don't leak a locked volume
    //! or a 4 MiB VirtualAlloc region.

    use std::alloc::{Layout, alloc_zeroed, dealloc};
    use std::ffi::c_void;
    use std::fs::File;
    use std::io::{Read, Write};
    use std::mem::MaybeUninit;
    use std::os::windows::ffi::OsStrExt as _;
    use std::path::Path;
    use std::ptr;

    use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_NO_BUFFERING, FILE_FLAG_WRITE_THROUGH, FILE_FLAGS_AND_ATTRIBUTES,
        FILE_SHARE_NONE, FindFirstVolumeW, FindNextVolumeW, FindVolumeClose,
        IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, OPEN_EXISTING, WriteFile,
    };
    use windows::Win32::System::IO::DeviceIoControl;
    use windows::Win32::System::Ioctl::{
        DISK_GEOMETRY_EX, FSCTL_DISMOUNT_VOLUME, FSCTL_LOCK_VOLUME,
        IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, VOLUME_DISK_EXTENTS,
    };
    use windows::core::PCWSTR;

    use super::{WRITE_CHUNK_BYTES, WritePlan, translate_win32_error};

    /// RAII wrapper for a `HANDLE`. `CloseHandle` runs on `Drop` so
    /// early returns (write-loop failure, lock failure, etc.) don't
    /// leak an exclusive-access handle that would otherwise leave the
    /// volume locked until the process exits.
    pub(super) struct OwnedHandle(HANDLE);

    impl OwnedHandle {
        pub(super) fn raw(&self) -> HANDLE {
            self.0
        }
    }

    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            if !self.0.is_invalid() {
                // Safety: self.0 was produced by CreateFileW and is
                // exclusively owned — no aliasing HANDLE copies exist.
                #[allow(unsafe_code)]
                // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
                let _ = unsafe { CloseHandle(self.0) };
            }
        }
    }

    /// Sector-aligned heap buffer for direct-I/O writes. Layout
    /// `align = 4 KiB` covers any plausible sector size (512 B –
    /// 4 KiB typical; 64 KiB is the upper bound the pure-fn layer
    /// accepts). `Drop` frees on every exit path.
    pub(super) struct AlignedBuffer {
        ptr: *mut u8,
        layout: Layout,
    }

    impl AlignedBuffer {
        fn new(size: usize) -> Result<Self, String> {
            let layout = Layout::from_size_align(size, 4096)
                .map_err(|e| format!("AlignedBuffer::new layout: {e}"))?;
            // Safety: layout has size > 0 (WRITE_CHUNK_BYTES = 4 MiB)
            // and a power-of-two alignment. alloc_zeroed returns a
            // valid pointer or null; we check null below.
            #[allow(unsafe_code)]
            // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
            let ptr = unsafe { alloc_zeroed(layout) };
            if ptr.is_null() {
                return Err(format!(
                    "AlignedBuffer::new: allocator returned null for {size} bytes @ 4 KiB"
                ));
            }
            Ok(Self { ptr, layout })
        }

        fn as_mut_slice(&mut self) -> &mut [u8] {
            // Safety: self.ptr is non-null (enforced in ::new), points
            // to self.layout.size() bytes we allocated, and self owns
            // that region exclusively (no aliasing — we yield a
            // mutable slice through a &mut self).
            #[allow(unsafe_code)]
            // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
            unsafe {
                std::slice::from_raw_parts_mut(self.ptr, self.layout.size())
            }
        }

        fn as_ptr(&self) -> *const c_void {
            self.ptr.cast()
        }
    }

    impl Drop for AlignedBuffer {
        fn drop(&mut self) {
            if !self.ptr.is_null() {
                // Safety: self.ptr was returned by alloc_zeroed with
                // self.layout, and we own it exclusively.
                #[allow(unsafe_code)]
                // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
                unsafe {
                    dealloc(self.ptr, self.layout);
                }
                self.ptr = ptr::null_mut();
            }
        }
    }

    fn to_wide(s: &str) -> Vec<u16> {
        std::ffi::OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0u16))
            .collect()
    }

    fn last_error_message(op: &str) -> String {
        // windows::core::Error::from_win32() reads GetLastError.
        let err = windows::core::Error::from_win32();
        let code = u32::try_from(err.code().0).unwrap_or(u32::MAX);
        format!("{op}: {}", translate_win32_error(code))
    }

    /// Open `\\.\PhysicalDriveN` with the flag set documented in the
    /// module safety invariants: `GENERIC_READ | GENERIC_WRITE`,
    /// `FILE_SHARE_NONE`, `OPEN_EXISTING`,
    /// `FILE_FLAG_NO_BUFFERING | FILE_FLAG_WRITE_THROUGH`.
    pub(super) fn open_physical_drive(physical_drive: u32) -> Result<OwnedHandle, String> {
        let path = format!(r"\\.\PhysicalDrive{physical_drive}");
        let wide = to_wide(&path);
        // Safety: `wide` is a NUL-terminated UTF-16 buffer owned by
        // this function for the entire CreateFileW call. All other
        // arguments are Win32 constants. The return value is a
        // Result that reflects GetLastError on failure.
        #[allow(unsafe_code)]
        // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
        let handle = unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                (GENERIC_READ | GENERIC_WRITE).0,
                FILE_SHARE_NONE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_NO_BUFFERING | FILE_FLAG_WRITE_THROUGH,
                None,
            )
        }
        .map_err(|e| {
            let code = u32::try_from(e.code().0).unwrap_or(u32::MAX);
            format!("CreateFileW({path}): {}", translate_win32_error(code))
        })?;

        if handle.is_invalid() {
            return Err(format!(
                "CreateFileW({path}): returned INVALID_HANDLE_VALUE without error"
            ));
        }
        Ok(OwnedHandle(handle))
    }

    /// `IOCTL_DISK_GET_DRIVE_GEOMETRY_EX` reads the on-disk sector
    /// size. Returns the sector size in bytes. The pure-fn layer
    /// refuses implausible values (<512 B, >64 KiB, non-power-of-two);
    /// that gate runs in [`super::plan_write`] after this returns.
    pub(super) fn query_sector_size(handle: &OwnedHandle) -> Result<u32, String> {
        let mut out = MaybeUninit::<DISK_GEOMETRY_EX>::zeroed();
        let mut bytes_returned: u32 = 0;
        let out_size = u32::try_from(std::mem::size_of::<DISK_GEOMETRY_EX>()).unwrap_or(u32::MAX);

        // Safety: out points to a zeroed DISK_GEOMETRY_EX for Windows
        // to fill. out_size matches its byte length. bytes_returned is
        // a valid u32 slot. handle is a valid raw-disk HANDLE.
        #[allow(unsafe_code)]
        // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
        let rc = unsafe {
            DeviceIoControl(
                handle.raw(),
                IOCTL_DISK_GET_DRIVE_GEOMETRY_EX,
                None,
                0,
                Some(out.as_mut_ptr().cast()),
                out_size,
                Some(&mut bytes_returned),
                None,
            )
        };
        rc.map_err(|_| last_error_message("IOCTL_DISK_GET_DRIVE_GEOMETRY_EX"))?;

        // Safety: DeviceIoControl returned success, so Windows wrote
        // a DISK_GEOMETRY_EX into `out`. Reading it back as initialized
        // is therefore sound.
        #[allow(unsafe_code)]
        // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
        let geom = unsafe { out.assume_init() };
        Ok(geom.Geometry.BytesPerSector)
    }

    /// `FSCTL_LOCK_VOLUME` — exclusive access before write. Fails
    /// cleanly on BitLocker-protected volumes (ACCESS_DENIED) or on
    /// Defender-scan contention (SHARING_VIOLATION).
    pub(super) fn lock_volume(handle: &OwnedHandle) -> Result<(), String> {
        let mut bytes_returned: u32 = 0;
        // Safety: FSCTL_LOCK_VOLUME takes no input/output buffers.
        #[allow(unsafe_code)]
        // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
        let rc = unsafe {
            DeviceIoControl(
                handle.raw(),
                FSCTL_LOCK_VOLUME,
                None,
                0,
                None,
                0,
                Some(&mut bytes_returned),
                None,
            )
        };
        rc.map_err(|_| last_error_message("FSCTL_LOCK_VOLUME"))
    }

    /// `FSCTL_DISMOUNT_VOLUME` — forces Windows to drop cached
    /// partition-table state so the next open sees the fresh layout.
    pub(super) fn dismount_volume(handle: &OwnedHandle) -> Result<(), String> {
        let mut bytes_returned: u32 = 0;
        // Safety: FSCTL_DISMOUNT_VOLUME takes no input/output buffers.
        #[allow(unsafe_code)]
        // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
        let rc = unsafe {
            DeviceIoControl(
                handle.raw(),
                FSCTL_DISMOUNT_VOLUME,
                None,
                0,
                None,
                0,
                Some(&mut bytes_returned),
                None,
            )
        };
        rc.map_err(|_| last_error_message("FSCTL_DISMOUNT_VOLUME"))
    }

    /// Sector-aligned write loop. Seeks to `plan.offset`, then reads
    /// `WRITE_CHUNK_BYTES` from `src` into an aligned buffer and
    /// `WriteFile`s each chunk. The final chunk is zero-padded to a
    /// sector boundary (required by direct-I/O flags).
    pub(super) fn write_all_sector_aligned(
        handle: &OwnedHandle,
        src: &mut File,
        plan: &WritePlan,
    ) -> Result<(), String> {
        use windows::Win32::Storage::FileSystem::{FILE_BEGIN, SetFilePointerEx};

        // Seek the raw-disk handle to the write offset. offset is
        // already validated sector-aligned by plan_write.
        let offset_i64 = i64::try_from(plan.offset)
            .map_err(|_| format!("write_all: offset {} exceeds i64", plan.offset))?;
        // Safety: handle is a valid raw-disk HANDLE open for write;
        // FILE_BEGIN is the documented constant. new-position out ptr
        // is None because we don't need it.
        #[allow(unsafe_code)]
        // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
        let rc = unsafe { SetFilePointerEx(handle.raw(), offset_i64, None, FILE_BEGIN) };
        rc.map_err(|_| last_error_message("SetFilePointerEx"))?;

        let mut buf = AlignedBuffer::new(WRITE_CHUNK_BYTES)?;
        let sector_bytes = plan.sector_bytes as usize;

        let mut remaining = plan.aligned_total;
        loop {
            if remaining == 0 {
                break;
            }

            // Fill the buffer from src. read_to_end would allocate;
            // instead, loop read() until the chunk is full or EOF.
            let chunk_target = std::cmp::min(remaining, WRITE_CHUNK_BYTES as u64);
            let chunk_target_usize = usize::try_from(chunk_target).unwrap_or(usize::MAX);
            let slice = &mut buf.as_mut_slice()[..chunk_target_usize];
            let mut filled: usize = 0;
            while filled < chunk_target_usize {
                match src.read(&mut slice[filled..]) {
                    Ok(0) => break,
                    Ok(n) => filled += n,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(format!("write_all: source read: {e}")),
                }
            }
            // Pad the read shortfall to a sector boundary with zeros.
            // The buffer was zero-initialized and we only overwrite
            // bytes we wrote, so zero-pad is implicit up to the slice
            // end; but `filled` may sit between sectors, so round UP.
            let filled_rounded = if filled == 0 {
                0
            } else {
                ((filled - 1) / sector_bytes + 1) * sector_bytes
            };
            // Tail-chunk semantics: read 0 bytes means we're done.
            let write_len = if filled == 0 {
                0
            } else {
                std::cmp::min(filled_rounded, chunk_target_usize)
            };
            if write_len == 0 {
                // No more bytes to read AND remaining==0 handled at loop top.
                break;
            }

            // Bounds check: WRITE_CHUNK_BYTES = 4 MiB fits in u32 but
            // keep the explicit conversion so a future WRITE_CHUNK_BYTES
            // change can't silently overflow.
            u32::try_from(write_len)
                .map_err(|_| format!("write_all: chunk {write_len} exceeds u32"))?;
            let mut bytes_written: u32 = 0;
            // Safety: buf.as_ptr() points to write_len bytes of our
            // AlignedBuffer. handle is a valid write handle. Output
            // ptr is a local u32.
            #[allow(unsafe_code)]
            // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
            let rc = unsafe {
                WriteFile(
                    handle.raw(),
                    Some(std::slice::from_raw_parts(
                        buf.as_ptr().cast::<u8>(),
                        write_len,
                    )),
                    Some(&mut bytes_written),
                    None,
                )
            };
            rc.map_err(|_| last_error_message("WriteFile"))?;
            if bytes_written as usize != write_len {
                return Err(format!(
                    "WriteFile: short write {bytes_written}/{write_len} — \
                     direct I/O should never partial-write"
                ));
            }
            remaining = remaining.saturating_sub(write_len as u64);

            // When we hit source EOF (filled < chunk_target_usize),
            // we've zero-padded to the final sector — stop.
            if filled < chunk_target_usize {
                break;
            }
        }
        Ok(())
    }

    /// Find the volume GUID path (`\\?\Volume{GUID}\`) for partition 1
    /// of `physical_drive`. Used by [`super::stage_esp`] to drop files
    /// through the FS driver without needing a drive letter.
    pub(super) fn find_esp_volume_guid(physical_drive: u32) -> Result<String, String> {
        let mut name_buf = [0u16; 256];
        // Safety: name_buf is a valid u16 slice for FindFirstVolumeW
        // to fill. Length passed matches slice size.
        #[allow(unsafe_code)]
        // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
        let find = unsafe { FindFirstVolumeW(&mut name_buf) }
            .map_err(|_| last_error_message("FindFirstVolumeW"))?;

        // RAII cleanup: always close the find handle. windows-rs 0.58
        // exposes Find*VolumeW as HANDLE-based APIs; the distinct
        // FindVolumeHandle newtype doesn't exist in this version.
        struct FindGuard(HANDLE);
        impl Drop for FindGuard {
            fn drop(&mut self) {
                #[allow(unsafe_code)]
                // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
                let _ = unsafe { FindVolumeClose(self.0) };
            }
        }
        let _guard = FindGuard(find);

        loop {
            // Convert UTF-16 to Rust string, trimming the NUL.
            let nul = name_buf
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(name_buf.len());
            let vol_name = String::from_utf16_lossy(&name_buf[..nul]);

            if volume_backs_physical_drive(&vol_name, physical_drive).unwrap_or(false) {
                return Ok(vol_name);
            }

            name_buf.fill(0);
            // Safety: find is a valid FindVolume handle; name_buf is
            // a valid mutable u16 buffer.
            #[allow(unsafe_code)]
            // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
            let next = unsafe { FindNextVolumeW(find, &mut name_buf) };
            if next.is_err() {
                return Err(format!(
                    "find_esp_volume_guid: no volume backed by \
                     PhysicalDrive{physical_drive}"
                ));
            }
        }
    }

    /// Check whether the volume named `vol_name` (a `\\?\Volume{GUID}\`
    /// path) is backed by disk `physical_drive`. Opens the volume
    /// read-only, queries `IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS`, and
    /// returns true if any extent lives on the target disk.
    fn volume_backs_physical_drive(vol_name: &str, physical_drive: u32) -> Result<bool, String> {
        // Trim the trailing `\` — CreateFileW accepts the volume name
        // with or without, but the raw-volume handle needs it without.
        let trimmed = vol_name.trim_end_matches('\\');
        let wide = to_wide(trimmed);

        // Safety: wide is a NUL-terminated UTF-16 buffer; all other
        // args are Win32 constants. Handle is checked below.
        #[allow(unsafe_code)]
        // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
        let handle = unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                0, // query-only; no access needed for IOCTL
                windows::Win32::Storage::FileSystem::FILE_SHARE_READ
                    | windows::Win32::Storage::FileSystem::FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(0),
                None,
            )
        }
        .map_err(|_| last_error_message("CreateFileW(volume)"))?;

        let owned = OwnedHandle(handle);

        // VOLUME_DISK_EXTENTS is variable-length (DISK_EXTENT[]); a
        // buffer large enough for 8 extents suffices for USB sticks
        // (usually 1, occasionally 2-4 for striped layouts).
        const EXTENT_BUF_SIZE: usize = 256;
        let mut ext_buf = [0u8; EXTENT_BUF_SIZE];
        let mut bytes_returned: u32 = 0;

        // Safety: ext_buf points to EXTENT_BUF_SIZE valid bytes;
        // handle is a valid volume handle.
        #[allow(unsafe_code)]
        // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
        let rc = unsafe {
            DeviceIoControl(
                owned.raw(),
                IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS,
                None,
                0,
                Some(ext_buf.as_mut_ptr().cast()),
                u32::try_from(EXTENT_BUF_SIZE).unwrap_or(u32::MAX),
                Some(&mut bytes_returned),
                None,
            )
        };
        rc.map_err(|_| last_error_message("IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS"))?;

        // Safety: DeviceIoControl filled ext_buf with a
        // VOLUME_DISK_EXTENTS header + DISK_EXTENT[] tail. Reading the
        // header is sound once bytes_returned covers its size.
        if (bytes_returned as usize) < std::mem::size_of::<VOLUME_DISK_EXTENTS>() {
            return Ok(false);
        }
        #[allow(unsafe_code)]
        // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
        let header = unsafe { &*(ext_buf.as_ptr() as *const VOLUME_DISK_EXTENTS) };
        let n = header.NumberOfDiskExtents as usize;
        // Extents live after the first u32 + 4-byte pad; windows-rs
        // DISK_EXTENT[1] is a tail array.
        for i in 0..n {
            // Safety: `header.Extents` is a flexible array member; the
            // DeviceIoControl filled bytes_returned >= header_size +
            // n * extent_size. Indexing up to n-1 is sound.
            #[allow(unsafe_code)]
            // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
            let extent = unsafe { &*header.Extents.as_ptr().add(i) };
            if extent.DiskNumber == physical_drive {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Copy `src` to `dst_path` using the FAT32 FS driver — standard
    /// buffered I/O, no direct-I/O flags, no volume lock. Writes are
    /// bounded to a few MiB per file, so buffered semantics are fine.
    pub(super) fn copy_file_to_volume(src: &Path, dst_path: &str) -> Result<(), String> {
        // Ensure the ESP sub-directories exist. EFI\BOOT is the only
        // one needed (per EspFile::esp_path); vmlinuz + initramfs.cpio.gz
        // live at volume root.
        if let Some(parent) = std::path::Path::new(dst_path).parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("create_dir_all({}): {e}", parent.display()))?;
        }

        let bytes = std::fs::read(src).map_err(|e| format!("read source: {e}"))?;
        let mut out = std::fs::File::create(dst_path).map_err(|e| format!("create dest: {e}"))?;
        out.write_all(&bytes)
            .map_err(|e| format!("write dest: {e}"))?;
        out.sync_all().map_err(|e| format!("sync dest: {e}"))?;
        Ok(())
    }
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

    // ---- Windows integration test (#484) -------------------------------
    //
    // Destructive write + readback round-trip against a real physical
    // disk. Opt-in via `AEGIS_BOOT_RAW_WRITE_TEST_DRIVE=N` to avoid
    // accidental execution — CI + local dev runs without that env var
    // skip these entirely. The only safe drive number to pass is the
    // operator's pre-designated scratch disk; the refuse-disk-0 gate
    // catches the most common typo.
    //
    // Running this locally:
    //     $env:AEGIS_BOOT_RAW_WRITE_TEST_DRIVE = "1"  # or whatever
    //     cargo test -p aegis-bootctl --bins windows_direct_install::raw_write -- --ignored --test-threads=1
    //
    // The test writes a 3-sector pattern at offset 64 KiB (past any
    // partition-table header), reads it back through a read-only
    // handle, and asserts byte equality.
    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "destructive — requires AEGIS_BOOT_RAW_WRITE_TEST_DRIVE + Administrator"]
    fn raw_write_roundtrip_on_scratch_disk() {
        use std::io::Write as _;

        let drive: u32 = match std::env::var("AEGIS_BOOT_RAW_WRITE_TEST_DRIVE") {
            Ok(s) => s
                .parse()
                .expect("AEGIS_BOOT_RAW_WRITE_TEST_DRIVE must be u32"),
            Err(_) => {
                eprintln!("AEGIS_BOOT_RAW_WRITE_TEST_DRIVE unset; skipping");
                return;
            }
        };
        assert_ne!(drive, 0, "disk 0 is the OS boot drive — refuse");

        // Build a deterministic payload the readback can verify.
        // 3 × 4 KiB = 12 KiB (covers the "final chunk padding" path
        // without taking seconds of I/O).
        let mut payload = Vec::with_capacity(3 * 4096);
        for i in 0..(3 * 4096) {
            payload.push(((i * 131) & 0xff) as u8);
        }
        let mut src = tempfile::NamedTempFile::new().expect("temp");
        src.write_all(&payload).expect("write temp");
        src.flush().expect("flush temp");

        // Write offset chosen well past any real partition-table
        // header (MBR lives in the first 512 B; GPT header + table in
        // the first ~34 sectors ≈ 17 KiB). 64 KiB gives 46+ KiB of
        // safety margin.
        let offset: u64 = 64 * 1024;

        write_bytes_to_physical_drive(drive, src.path(), offset)
            .expect("raw write should succeed on scratch disk");

        // Readback: open the same physical drive read-only (no direct
        // I/O flags needed — this is a validation read) and compare.
        let readback =
            read_bytes_for_verify(drive, offset, payload.len()).expect("readback should succeed");
        assert_eq!(&readback[..], &payload[..], "roundtrip mismatch");
    }

    #[cfg(target_os = "windows")]
    fn read_bytes_for_verify(
        physical_drive: u32,
        offset: u64,
        len: usize,
    ) -> Result<Vec<u8>, String> {
        use std::os::windows::ffi::OsStrExt as _;
        use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, HANDLE};
        use windows::Win32::Storage::FileSystem::{
            CreateFileW, FILE_BEGIN, FILE_FLAG_NO_BUFFERING, FILE_SHARE_READ, FILE_SHARE_WRITE,
            OPEN_EXISTING, ReadFile, SetFilePointerEx,
        };
        use windows::core::PCWSTR;

        let path = format!(r"\\.\PhysicalDrive{physical_drive}");
        let wide: Vec<u16> = std::ffi::OsStr::new(&path)
            .encode_wide()
            .chain(std::iter::once(0u16))
            .collect();
        // Safety: wide is a NUL-terminated UTF-16 buffer owned for the
        // duration of the call; all other args are Win32 constants.
        #[allow(unsafe_code)]
        // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
        let h: HANDLE = unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                GENERIC_READ.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_NO_BUFFERING,
                None,
            )
        }
        .map_err(|e| format!("readback open: {e}"))?;

        // Read a sector-aligned region covering `len`.
        // Scratch disks are typically 512 B sectors; round up to 4 KiB
        // to cover both 512e and 4Kn. The test payload is already 12 KiB
        // (3 × 4 KiB).
        let sector_bytes: usize = 4096;
        let read_size = ((len + sector_bytes - 1) / sector_bytes) * sector_bytes;

        // Need an aligned buffer for direct I/O.
        let layout = std::alloc::Layout::from_size_align(read_size, 4096)
            .map_err(|e| format!("readback layout: {e}"))?;
        // Safety: layout is valid with power-of-two alignment and
        // non-zero size; null is checked below.
        #[allow(unsafe_code)]
        // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
        let ptr = unsafe { std::alloc::alloc_zeroed(layout) };
        if ptr.is_null() {
            // Safety: h was returned by CreateFileW above and is owned.
            #[allow(unsafe_code)]
            // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
            unsafe {
                let _ = CloseHandle(h);
            }
            return Err("readback: alloc failed".into());
        }
        // Safety: ptr was returned by alloc_zeroed with layout above,
        // non-null (checked), and exclusively owned.
        let buf: &mut [u8] = {
            #[allow(unsafe_code)]
            // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
            unsafe {
                std::slice::from_raw_parts_mut(ptr, read_size)
            }
        };

        // Seek, read, close.
        let offset_i64 = i64::try_from(offset).map_err(|_| "offset > i64")?;
        // Safety: h is a valid read-only raw-disk HANDLE.
        #[allow(unsafe_code)]
        // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
        let rc = unsafe { SetFilePointerEx(h, offset_i64, None, FILE_BEGIN) };
        if let Err(e) = rc {
            // Safety: ptr + h are both still owned at this point.
            #[allow(unsafe_code)]
            // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
            unsafe {
                std::alloc::dealloc(ptr, layout);
                let _ = CloseHandle(h);
            }
            return Err(format!("readback seek: {e}"));
        }

        let mut bytes_read: u32 = 0;
        // Safety: buf points to read_size bytes; h is a valid read handle.
        #[allow(unsafe_code)]
        // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
        let rc = unsafe { ReadFile(h, Some(buf), Some(&mut bytes_read), None) };

        let out = buf[..len].to_vec();

        // Safety: ptr + h ownership unchanged since creation; last use.
        #[allow(unsafe_code)]
        // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
        unsafe {
            std::alloc::dealloc(ptr, layout);
            let _ = CloseHandle(h);
        }
        rc.map_err(|e| format!("readback ReadFile: {e}"))?;
        if (bytes_read as usize) < len {
            return Err(format!("readback: short read {bytes_read}/{read_size}"));
        }
        Ok(out)
    }
}
