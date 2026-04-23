// SPDX-License-Identifier: MIT OR Apache-2.0

//! #447 — Phase 1 of the Windows direct-install adapter.
//!
//! Produces a GPT partition layout byte-compatible with what Linux
//! `sgdisk` emits: 2-partition layout (ESP 400 MiB EFI + rest as
//! Microsoft Basic Data for `AEGIS_ISOS`). No Microsoft Reserved
//! Partition — `Initialize-Disk -PartitionStyle GPT` would insert
//! one, diskpart scripted via stdin does not.
//!
//! ## Why diskpart, not New-Partition
//!
//! Win11 prototyping on 2026-04-23 confirmed that
//! `Initialize-Disk -PartitionStyle GPT` silently auto-inserts a
//! 16 MiB MSR as partition 1, pushing ESP to partition 2. Linux-
//! flashed sticks have ESP at partition 1. For byte-parity between
//! Linux- and Windows-flashed sticks (which `verify --stick` relies
//! on), Windows must avoid the MSR. diskpart gives explicit layout
//! control.
//!
//! ## Safety invariants (per #447)
//!
//! 1. **Refuse disk 0.** The OS boot drive is virtually always
//!    Disk 0 on Windows. Pure-fn side catches this with a
//!    dedicated error variant so the subprocess wrapper can never
//!    accidentally issue a `clean` against the boot drive.
//! 2. **Pre-flight elevation check.** Subprocess wrapper refuses
//!    to run without admin (handled in #450 when that lands; for
//!    now we assume caller is elevated).
//! 3. **Confirm target identity** — same pattern as Linux flash
//!    UX (handled in #450).
//! 4. **`BitLocker` detection** — also #450.
//!
//! This module ships the pure-fn builders + the `#[cfg(windows)]`
//! subprocess wrapper. The builders are exercised by unit tests
//! on any host; the subprocess wrapper only compiles on Windows.
//!
//! # Dead-code allow
//!
//! Everything in this module is scaffolding awaiting wiring in the
//! future CLI-integration phase (post #450). The unit tests keep the
//! pure-fn side exercised, but no runtime caller exists yet. Module-
//! scoped `allow(dead_code)` because stable Rust's dead-code detection
//! on the Windows compile target would otherwise gate our CI.

#![allow(dead_code)]

use crate::constants::ESP_SIZE_MB;

/// Why a partition request was rejected at the pure-fn layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PartitionBuildError {
    /// Caller passed disk 0 — almost certainly the OS boot drive on
    /// Windows. Refuse without even asking the operator.
    BootDriveRefused,
}

impl std::fmt::Display for PartitionBuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BootDriveRefused => write!(
                f,
                "refusing to partition PhysicalDrive0 — that's virtually always the OS boot drive on Windows; pick a removable USB target (PhysicalDrive1+)"
            ),
        }
    }
}

/// Build the diskpart script that produces aegis-boot's canonical
/// 2-partition GPT layout on the given `physical_drive` index.
///
/// Output shape:
///
/// ```text
/// select disk <N>
/// clean
/// convert gpt
/// create partition efi size=<ESP_SIZE_MB>
/// format fs=fat32 label="AEGIS_ESP" quick
/// create partition primary
/// format fs=exfat label="AEGIS_ISOS" quick
/// exit
/// ```
///
/// The `create partition efi` form is the specific incantation that
/// gives us an EFI System Partition (GUID `c12a7328-f81f-11d2-ba4b-
/// 00a0c93ec93b`) WITHOUT diskpart implicitly reserving a Microsoft
/// Reserved Partition. Using `create partition primary` instead of
/// `create partition msr` for the data side avoids the MSR too.
///
/// Refuses disk 0 unconditionally — even an operator who insists via
/// `--yes` shouldn't be able to clean the OS boot drive from this
/// code path.
pub(crate) fn build_diskpart_script(physical_drive: u32) -> Result<String, PartitionBuildError> {
    if physical_drive == 0 {
        return Err(PartitionBuildError::BootDriveRefused);
    }
    Ok(format!(
        "select disk {physical_drive}\n\
         clean\n\
         convert gpt\n\
         create partition efi size={ESP_SIZE_MB}\n\
         format fs=fat32 label=\"AEGIS_ESP\" quick\n\
         create partition primary\n\
         format fs=exfat label=\"AEGIS_ISOS\" quick\n\
         exit\n"
    ))
}

/// Partition the target disk via `diskpart`, fed the script produced
/// by [`build_diskpart_script`] on stdin. Windows-only. Returns on
/// successful exit; propagates diskpart's stderr on non-zero exit.
///
/// Requires Administrator elevation — diskpart won't even start as a
/// non-admin. Callers should gate on #450's elevation check first;
/// this layer surfaces the resulting `ACCESS_DENIED` cleanly.
#[cfg(target_os = "windows")]
pub(crate) fn partition_via_diskpart(physical_drive: u32) -> Result<(), String> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let script =
        build_diskpart_script(physical_drive).map_err(|e| format!("diskpart script: {e}"))?;

    let mut child = Command::new("diskpart")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn diskpart: {e}"))?;

    child
        .stdin
        .as_mut()
        .ok_or_else(|| "diskpart stdin not open".to_string())?
        .write_all(script.as_bytes())
        .map_err(|e| format!("write diskpart script: {e}"))?;

    let out = child
        .wait_with_output()
        .map_err(|e| format!("wait diskpart: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "diskpart exited {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn build_script_refuses_disk_zero() {
        let result = build_diskpart_script(0);
        assert_eq!(result, Err(PartitionBuildError::BootDriveRefused));
    }

    #[test]
    fn build_script_accepts_non_boot_disks() {
        for n in [1u32, 2, 3, 5, 9, 15, 99] {
            let script = build_diskpart_script(n).expect("non-zero disk should accept");
            assert!(script.contains(&format!("select disk {n}\n")));
        }
    }

    #[test]
    fn script_starts_with_select_disk_and_ends_with_exit() {
        let s = build_diskpart_script(2).unwrap();
        assert!(
            s.starts_with("select disk 2\n"),
            "script must pin the target upfront, got:\n{s}"
        );
        assert!(
            s.trim_end().ends_with("exit"),
            "script must terminate diskpart cleanly, got:\n{s}"
        );
    }

    #[test]
    fn script_converts_gpt_not_mbr() {
        let s = build_diskpart_script(2).unwrap();
        assert!(
            s.contains("\nconvert gpt\n"),
            "GPT conversion must be explicit (MBR is unsigned-boot incompatible)"
        );
    }

    #[test]
    fn script_creates_efi_partition_not_msr() {
        // `create partition efi size=400` gets us the EFI System
        // Partition (GUID c12a7328-...) without the implicit MSR
        // that `Initialize-Disk` would insert. This is the entire
        // raison d'être of using diskpart over the PS cmdlets.
        let s = build_diskpart_script(2).unwrap();
        assert!(
            s.contains(&format!("create partition efi size={ESP_SIZE_MB}\n")),
            "must use `create partition efi size=<ESP_SIZE_MB>`, got:\n{s}"
        );
        assert!(
            !s.contains("msr"),
            "must NOT create an MSR — breaks byte-parity with Linux sgdisk layout"
        );
    }

    #[test]
    fn script_formats_esp_fat32_with_aegis_esp_label() {
        let s = build_diskpart_script(2).unwrap();
        assert!(
            s.contains("format fs=fat32 label=\"AEGIS_ESP\" quick\n"),
            "ESP label + filesystem must match mkusb.sh / direct-install.rs defaults"
        );
    }

    #[test]
    fn script_formats_data_exfat_with_aegis_isos_label() {
        let s = build_diskpart_script(2).unwrap();
        assert!(
            s.contains("format fs=exfat label=\"AEGIS_ISOS\" quick\n"),
            "AEGIS_ISOS label + filesystem must match mkusb.sh / direct-install.rs defaults"
        );
    }

    #[test]
    fn script_emits_only_known_commands() {
        // Guard against accidentally pasting dangerous diskpart
        // verbs into the script. If this test starts failing, think
        // twice before just updating the allowlist — each new verb
        // is a new way to lose data.
        let allowed: &[&str] = &["select", "clean", "convert", "create", "format", "exit"];
        let s = build_diskpart_script(2).unwrap();
        for line in s.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let first = line.split_whitespace().next().unwrap_or("");
            assert!(
                allowed.contains(&first),
                "unexpected diskpart command {first:?} in script — \
                 review + extend the allowlist deliberately. full script:\n{s}"
            );
        }
    }

    #[test]
    fn error_display_names_the_drive_index_and_suggests_alternative() {
        let msg = format!("{}", PartitionBuildError::BootDriveRefused);
        assert!(
            msg.contains("PhysicalDrive0"),
            "error must name the drive: {msg}"
        );
        assert!(
            msg.contains("PhysicalDrive1"),
            "error must suggest a safer alternative: {msg}"
        );
    }
}
