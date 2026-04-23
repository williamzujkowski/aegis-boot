// SPDX-License-Identifier: MIT OR Apache-2.0

//! #448 — Phase 2 of the Windows direct-install adapter.
//!
//! Formats the two partitions created by the #447 partition step:
//! ESP as FAT32 labeled `AEGIS_ESP`, `AEGIS_ISOS` as exFAT labeled
//! `AEGIS_ISOS`. Both labels and filesystem choices match Linux
//! direct-install defaults so a stick flashed on Windows is
//! interchangeable with one flashed on Linux.
//!
//! ## Why `Format-Volume`, not `format` inside diskpart
//!
//! diskpart's built-in `format` verb works for FAT32 but has
//! well-known quirks (silent fallback to FAT16 on small volumes,
//! no exFAT support in older builds, opaque error messages). The
//! `PowerShell` `Format-Volume` cmdlet is the modern-supported path
//! and has stable error semantics we can pattern-match on.
//!
//! Win11 prototyping on 2026-04-23 confirmed `Format-Volume` works
//! end-to-end with native exFAT driver — no third-party FS driver
//! needed.
//!
//! ## Safety invariants (per #448)
//!
//! 1. **Refuse disk 0** (defense-in-depth; #447 already refuses it).
//! 2. **Verify partition GPT type** before format — if partition 1's
//!    type GUID isn't the EFI System Partition GUID
//!    (`c12a7328-f81f-11d2-ba4b-00a0c93ec93b`), abort. Protects
//!    against formatting whatever happens to be at partition 1 on a
//!    stick we didn't just partition.
//! 3. **`-Confirm:$false`** required — Format-Volume must not prompt.
//!    If the prompt fires anyway (operator policy, GPO override),
//!    surface it as a specific error rather than hang.

// Scaffolding for the #419 Windows adapter — everything here is
// pure-fn builders + a subprocess wrapper awaiting CLI wiring in a
// later phase. Dead-code detection on the Windows compile target
// would otherwise gate our CI. See #447's matching note.
#![allow(dead_code)]

use crate::windows_direct_install::partition::PartitionBuildError;

/// EFI System Partition type GUID — the only valid type for
/// partition 1 on an aegis-boot-Windows-flashed stick. If we see
/// something else there, refuse to format.
pub(crate) const ESP_GPT_TYPE_GUID: &str = "{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}";

/// Microsoft Basic Data Partition GUID — expected at partition 2
/// (`AEGIS_ISOS` data partition). Matches the type Linux's direct-
/// install writes via `sgdisk -t 2:0700`.
pub(crate) const MSDATA_GPT_TYPE_GUID: &str = "{ebd0a0a2-b9e5-4433-87c0-68b6b72699c7}";

/// Which partition is being formatted. Used both for the `PowerShell`
/// command selection and for the GPT-type verification guard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FormatTarget {
    /// Partition 1: FAT32, labeled `AEGIS_ESP`, GUID `ESP_GPT_TYPE_GUID`.
    Esp,
    /// Partition 2: exFAT, labeled `AEGIS_ISOS`, GUID `MSDATA_GPT_TYPE_GUID`.
    AegisIsos,
}

impl FormatTarget {
    pub(crate) fn partition_number(self) -> u32 {
        match self {
            Self::Esp => 1,
            Self::AegisIsos => 2,
        }
    }

    pub(crate) fn filesystem(self) -> &'static str {
        match self {
            Self::Esp => "FAT32",
            Self::AegisIsos => "exFAT",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Esp => "AEGIS_ESP",
            Self::AegisIsos => "AEGIS_ISOS",
        }
    }

    pub(crate) fn expected_gpt_type(self) -> &'static str {
        match self {
            Self::Esp => ESP_GPT_TYPE_GUID,
            Self::AegisIsos => MSDATA_GPT_TYPE_GUID,
        }
    }
}

/// Build the `PowerShell` command that formats `target` on
/// `physical_drive`. Pure-fn so it can be unit-tested without
/// spawning `powershell.exe`. Returns the full command string ready
/// to be passed as `-Command <this>` to `PowerShell`.
///
/// The command first verifies the partition's GPT type matches
/// [`FormatTarget::expected_gpt_type`] and throws if it doesn't —
/// protects against formatting partitions we didn't create.
///
/// Refuses disk 0 at the pure-fn layer (same defense-in-depth as
/// #447; drives shipped to this layer should already have been
/// filtered by an earlier gate).
pub(crate) fn build_format_ps_command(
    physical_drive: u32,
    target: FormatTarget,
) -> Result<String, PartitionBuildError> {
    if physical_drive == 0 {
        return Err(PartitionBuildError::BootDriveRefused);
    }
    let part = target.partition_number();
    let fs = target.filesystem();
    let label = target.label();
    let expected = target.expected_gpt_type();
    Ok(format!(
        "$p = Get-Partition -DiskNumber {physical_drive} -PartitionNumber {part}; \
         if ($p.GptType -ne '{expected}') {{ \
             throw \"partition {part} has unexpected GptType $($p.GptType), expected {expected}\" \
         }}; \
         $p | Format-Volume -FileSystem {fs} -NewFileSystemLabel '{label}' -Confirm:$false -Force | Out-Null"
    ))
}

/// Run `powershell.exe -NoProfile -Command <build_format_ps_command>`
/// against the target. Windows-only. Returns on successful exit;
/// propagates PowerShell's stderr on non-zero exit.
#[cfg(target_os = "windows")]
pub(crate) fn format_partition(physical_drive: u32, target: FormatTarget) -> Result<(), String> {
    use std::process::Command;

    let cmd =
        build_format_ps_command(physical_drive, target).map_err(|e| format!("format ps: {e}"))?;

    let out = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command"])
        .arg(&cmd)
        .output()
        .map_err(|e| format!("spawn powershell: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "Format-Volume exited {}: {}",
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
    fn format_target_partition_numbers_match_partition_phase() {
        assert_eq!(FormatTarget::Esp.partition_number(), 1);
        assert_eq!(FormatTarget::AegisIsos.partition_number(), 2);
    }

    #[test]
    fn format_target_labels_match_linux_direct_install() {
        // Must match scripts/mkusb.sh's labels + direct_install.rs's
        // AEGIS_ISOS_LABEL constant. Drift here silently breaks
        // cross-platform stick portability.
        assert_eq!(FormatTarget::Esp.label(), "AEGIS_ESP");
        assert_eq!(FormatTarget::AegisIsos.label(), "AEGIS_ISOS");
    }

    #[test]
    fn format_target_filesystems_match_linux_direct_install() {
        assert_eq!(FormatTarget::Esp.filesystem(), "FAT32");
        assert_eq!(FormatTarget::AegisIsos.filesystem(), "exFAT");
    }

    #[test]
    fn esp_gpt_type_guid_matches_efi_system_partition() {
        // {c12a7328-f81f-11d2-ba4b-00a0c93ec93b} is the universal
        // EFI System Partition GUID; firmware keys off this to
        // decide what to boot. Drift here = stick stops booting.
        assert_eq!(
            ESP_GPT_TYPE_GUID.to_ascii_lowercase(),
            "{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}"
        );
    }

    #[test]
    fn msdata_gpt_type_guid_matches_basic_data() {
        // {ebd0a0a2-b9e5-4433-87c0-68b6b72699c7} is the Microsoft
        // Basic Data Partition GUID; Linux direct-install writes
        // it via `sgdisk -t 2:0700`.
        assert_eq!(
            MSDATA_GPT_TYPE_GUID.to_ascii_lowercase(),
            "{ebd0a0a2-b9e5-4433-87c0-68b6b72699c7}"
        );
    }

    #[test]
    fn build_command_refuses_disk_zero() {
        for target in [FormatTarget::Esp, FormatTarget::AegisIsos] {
            let result = build_format_ps_command(0, target);
            assert_eq!(result, Err(PartitionBuildError::BootDriveRefused));
        }
    }

    #[test]
    fn build_command_pins_disk_and_partition_numbers() {
        let esp = build_format_ps_command(3, FormatTarget::Esp).unwrap();
        assert!(
            esp.contains("Get-Partition -DiskNumber 3 -PartitionNumber 1"),
            "ESP must pin disk 3 partition 1, got:\n{esp}"
        );
        let data = build_format_ps_command(3, FormatTarget::AegisIsos).unwrap();
        assert!(
            data.contains("Get-Partition -DiskNumber 3 -PartitionNumber 2"),
            "AEGIS_ISOS must pin disk 3 partition 2, got:\n{data}"
        );
    }

    #[test]
    fn build_command_verifies_gpt_type_before_format() {
        // The `if ($p.GptType -ne '<expected>')` guard is the
        // safety invariant #2 from #448: don't format a partition
        // whose type GUID doesn't match what we'd have written.
        let esp = build_format_ps_command(1, FormatTarget::Esp).unwrap();
        assert!(
            esp.contains(&format!("$p.GptType -ne '{ESP_GPT_TYPE_GUID}'")),
            "ESP format must guard on GptType, got:\n{esp}"
        );
        let data = build_format_ps_command(1, FormatTarget::AegisIsos).unwrap();
        assert!(
            data.contains(&format!("$p.GptType -ne '{MSDATA_GPT_TYPE_GUID}'")),
            "AEGIS_ISOS format must guard on GptType, got:\n{data}"
        );
    }

    #[test]
    fn build_command_sets_confirm_false_for_non_interactive_use() {
        // Format-Volume prompts by default. In a scripted flasher
        // context we MUST pass -Confirm:$false or we deadlock on
        // operator input that isn't coming.
        let cmd = build_format_ps_command(1, FormatTarget::Esp).unwrap();
        assert!(
            cmd.contains("-Confirm:$false"),
            "Format-Volume must be non-interactive, got:\n{cmd}"
        );
    }

    #[test]
    fn build_command_uses_expected_filesystem_and_label() {
        let esp = build_format_ps_command(1, FormatTarget::Esp).unwrap();
        assert!(esp.contains("-FileSystem FAT32"));
        assert!(esp.contains("-NewFileSystemLabel 'AEGIS_ESP'"));

        let data = build_format_ps_command(1, FormatTarget::AegisIsos).unwrap();
        assert!(data.contains("-FileSystem exFAT"));
        assert!(data.contains("-NewFileSystemLabel 'AEGIS_ISOS'"));
    }

    #[test]
    fn build_command_pipes_output_to_null() {
        // Format-Volume's default output is a verbose Volume object
        // we don't need on success. Piping through Out-Null keeps
        // stdout clean for operators. On failure PowerShell writes
        // the error to stderr anyway.
        let cmd = build_format_ps_command(1, FormatTarget::Esp).unwrap();
        assert!(cmd.contains("| Out-Null"));
    }
}
