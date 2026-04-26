// SPDX-License-Identifier: MIT OR Apache-2.0

//! #450 — Phase 4 of the Windows direct-install adapter.
//!
//! Pre-flight checks that gate destructive operations behind explicit
//! operator safety invariants. The earlier phases (#447/#448/#449) each
//! repeat a narrow subset (refuse disk 0); this module is the central
//! place to enforce the broader set before any partition / format /
//! raw-write work starts.
//!
//! Checks:
//!
//! 1. **Elevation** — `IsUserAnAdmin`-equivalent via `PowerShell`'s
//!    `WindowsPrincipal.IsInRole(Administrators)`. The three code
//!    paths we ship (`diskpart`, `Format-Volume`, raw-disk `WriteFile`)
//!    all fail without admin; surfacing the condition up-front avoids
//!    noisy mid-flash errors.
//! 2. **`BitLocker` detection** — if the target drive is `BitLocker`-
//!    protected, `FSCTL_LOCK_VOLUME` will fail with `ERROR_ACCESS_DENIED`
//!    and we'd leak a confusing error. Detect + surface a clear
//!    "decrypt first" remedy.
//! 3. **Target identity display** — before anything destructive,
//!    present the target's model + size + interface type so the
//!    operator can bail out if they picked the wrong disk.
//! 4. **Windows Defender exclusion hint** — operator-facing tip
//!    that surfaces when real-time scanning delays raw writes.
//!
//! Pure-fn builders + `cfg(target_os = "windows")` subprocess wrappers.
//! Same scaffolding shape as the other submodules.

// Scaffolding for the #419 Windows adapter — everything here is
// pure-fn builders + subprocess wrappers awaiting CLI wiring in
// the integration phase. Same allow(dead_code) rationale as the
// partition/format submodules.
#![allow(dead_code)]

/// Build the `PowerShell` one-liner that prints `"True"` if the
/// current process is running with Administrators-group
/// membership active in the token, `"False"` otherwise.
///
/// Uses `WindowsPrincipal.IsInRole(BuiltInRole::Administrator)`
/// which respects the UAC split-token correctly: a user who is a
/// member of Administrators but running a non-elevated shell
/// gets `False`, matching what `IsUserAnAdmin()` would return.
pub(crate) fn build_is_admin_ps_command() -> String {
    // `$p.IsInRole(...)` returns a .NET bool; PowerShell's string
    // coercion emits `True` / `False` which we parse on the Rust
    // side. Using Write-Output explicitly so no trailing whitespace
    // decoration sneaks in from the default pipeline formatter.
    "Write-Output ([Security.Principal.WindowsPrincipal]::new(\
     [Security.Principal.WindowsIdentity]::GetCurrent()\
     ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator))"
        .to_string()
}

/// Parse the output of [`build_is_admin_ps_command`] into a bool.
/// `PowerShell` emits `True\r\n` or `False\r\n`; anything else
/// (caller mismatched the command, shell error, locale oddity) is
/// treated as not-admin because fail-closed is the right default
/// for a destructive-capability check.
pub(crate) fn parse_is_admin_output(stdout: &str) -> bool {
    stdout.trim().eq_ignore_ascii_case("True")
}

/// Human-readable message to print when the elevation check says
/// "not admin." Included as a pure-fn so unit tests can assert its
/// shape (operators depend on the exact command being copy-
/// pasteable, so the test pins the `Start` + `cmd` + `Ctrl+Shift+
/// Enter` path).
pub(crate) fn elevation_required_message() -> &'static str {
    "aegis-boot flash: requires Administrator privileges on Windows.\n\
     Open an elevated PowerShell (Start → cmd → Ctrl+Shift+Enter) \
     and re-run the same command."
}

/// Build the `PowerShell` command that queries `BitLocker` status for a
/// given `\\.\PhysicalDriveN` via `manage-bde -status`. Returns the
/// stdout of manage-bde for caller-side parsing.
///
/// Why `manage-bde`: always on PATH on Windows 10+, consistent text
/// output across versions, no WMI dependency. The only drawback is
/// localized output — Windows in languages other than English may
/// emit "Conversion Status" in the local tongue. We pattern-match
/// on English-specific markers; non-English users see "unknown" and
/// get a safer "try to decrypt and retry" remedy, never an
/// incorrect "drive is safe" answer.
pub(crate) fn build_bitlocker_status_ps_command(physical_drive: u32) -> String {
    // `manage-bde -status \\.\PhysicalDriveN` returns "Fully
    // Decrypted" for unprotected drives. `2>&1` folds stderr so
    // failure messages are visible to the parser.
    format!("& manage-bde.exe -status \\\\.\\PhysicalDrive{physical_drive} 2>&1 | Out-String")
}

/// Classified `BitLocker` state for a target drive. Only the
/// unambiguous cases are accepted; anything else falls back to
/// `Unknown`, which the caller treats as "assume protected, refuse
/// to write."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BitLockerStatus {
    /// `manage-bde -status` explicitly reports "Fully Decrypted"
    /// for all volumes on the disk. Safe to proceed.
    FullyDecrypted,
    /// Any conversion state other than Fully Decrypted. Caller
    /// must refuse writes and surface the decrypt-first remedy.
    Protected,
    /// Output couldn't be classified (localized output, manage-bde
    /// missing, drive offline, etc.). Treat as protected (fail
    /// closed) but include the raw text in the error for
    /// diagnostics.
    Unknown,
}

/// Classify `manage-bde -status` output into a [`BitLockerStatus`].
///
/// Strategy: look for the English "Conversion Status" line and
/// match its value. Anything else → Unknown.
pub(crate) fn classify_bitlocker_status(stdout: &str) -> BitLockerStatus {
    let mut saw_conversion_line = false;
    let mut fully_decrypted_count = 0u32;
    let mut volume_count = 0u32;

    for line in stdout.lines() {
        let trimmed = line.trim();
        // Volume marker — manage-bde emits "Volume X: [Label]" per
        // protected/unprotected volume on the drive. Count them to
        // tell the difference between "empty output / parse error"
        // and "multi-volume drive all-decrypted." Skip the preamble
        // "Disk volumes that can be protected with..." which is a
        // section header, not a per-volume entry.
        if trimmed.starts_with("Volume ") && trimmed.contains(':') {
            volume_count += 1;
        }
        // Conversion Status line looks like:
        //   Conversion Status:    Fully Decrypted
        if let Some((key, val)) = trimmed.split_once(':')
            && key.trim().eq_ignore_ascii_case("Conversion Status")
        {
            saw_conversion_line = true;
            if val.trim().eq_ignore_ascii_case("Fully Decrypted") {
                fully_decrypted_count += 1;
            }
        }
    }

    if !saw_conversion_line {
        return BitLockerStatus::Unknown;
    }
    // Every conversion line we saw must say Fully Decrypted for the
    // disk to count as safe. Any other state (Decrypting, Encrypting,
    // Fully Encrypted, Encryption Paused) means we can't safely
    // FSCTL_LOCK_VOLUME + write.
    if fully_decrypted_count > 0 && fully_decrypted_count >= volume_count.max(1) {
        BitLockerStatus::FullyDecrypted
    } else {
        BitLockerStatus::Protected
    }
}

/// Remedy text printed when `BitLocker` detection says protected.
pub(crate) fn bitlocker_protected_message(physical_drive: u32) -> String {
    format!(
        "aegis-boot flash: target drive \\\\.\\PhysicalDrive{physical_drive} is \
         BitLocker-protected. Decrypt it first:\n  \
         manage-bde.exe -off \\\\.\\PhysicalDrive{physical_drive}\n\
         Wait for decryption to complete (may take hours on large drives), \
         then re-run. If you're unsure whether decryption finished, run \
         `manage-bde.exe -status \\\\.\\PhysicalDrive{physical_drive}` and \
         look for `Conversion Status: Fully Decrypted`."
    )
}

/// Operator-facing hint that surfaces if a raw-disk write runs
/// slower than expected. Windows Defender's real-time scan
/// intercepts raw sector writes and can add multi-minute overhead
/// on cheap USB sticks. The exclusion is reversible.
pub(crate) fn defender_exclusion_hint(physical_drive: u32) -> String {
    format!(
        "Tip: Windows Defender real-time scanning can delay raw-disk writes. \
         To temporarily exclude the target:\n  \
         Add-MpPreference -ExclusionPath \"\\\\.\\PhysicalDrive{physical_drive}\"\n\
         Remove with:\n  \
         Remove-MpPreference -ExclusionPath \"\\\\.\\PhysicalDrive{physical_drive}\""
    )
}

/// Run the elevation check via `PowerShell`. Returns `Ok(true)` if
/// elevated, `Ok(false)` if not, `Err` on subprocess failure.
#[cfg(target_os = "windows")]
pub(crate) fn is_running_as_admin() -> Result<bool, String> {
    use std::process::Command;
    let cmd = build_is_admin_ps_command();
    let out = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command"])
        .arg(&cmd)
        .output()
        .map_err(|e| format!("spawn powershell: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "elevation check failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    Ok(parse_is_admin_output(&stdout))
}

/// Run the `BitLocker` status check via `PowerShell` → `manage-bde`.
#[cfg(target_os = "windows")]
pub(crate) fn check_bitlocker_status(physical_drive: u32) -> Result<BitLockerStatus, String> {
    use std::process::Command;
    let cmd = build_bitlocker_status_ps_command(physical_drive);
    let out = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command"])
        .arg(&cmd)
        .output()
        .map_err(|e| format!("spawn powershell: {e}"))?;
    // manage-bde returns non-zero for missing drives etc. — we
    // still want to classify whatever output made it through,
    // rather than treat non-zero as a hard fail.
    let stdout = String::from_utf8_lossy(&out.stdout);
    Ok(classify_bitlocker_status(&stdout))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn is_admin_command_uses_windowsprincipal_in_role() {
        // The exact shape is load-bearing: `IsInRole` with
        // `WindowsBuiltInRole::Administrator` is what respects the
        // UAC split-token semantics. If a future contributor
        // swaps to `Get-WMIObject Win32_UserAccount` or similar,
        // we could silently accept a non-elevated user who happens
        // to be in the Administrators group. This test locks the
        // intended API in place.
        let cmd = build_is_admin_ps_command();
        assert!(cmd.contains("WindowsPrincipal"), "got: {cmd}");
        assert!(cmd.contains("IsInRole"), "got: {cmd}");
        assert!(cmd.contains("WindowsBuiltInRole"), "got: {cmd}");
        assert!(cmd.contains("Administrator"), "got: {cmd}");
    }

    #[test]
    fn is_admin_output_parser_accepts_true() {
        assert!(parse_is_admin_output("True"));
        assert!(parse_is_admin_output("True\n"));
        assert!(parse_is_admin_output("True\r\n"));
        assert!(parse_is_admin_output("  True  "));
        assert!(parse_is_admin_output("true"));
        assert!(parse_is_admin_output("TRUE"));
    }

    #[test]
    fn is_admin_output_parser_rejects_false_and_garbage() {
        assert!(!parse_is_admin_output("False"));
        assert!(!parse_is_admin_output("False\r\n"));
        assert!(!parse_is_admin_output(""));
        assert!(!parse_is_admin_output("unexpected error"));
        // Fail-closed: anything that isn't the exact string "True"
        // must return false. Don't give attackers a partial-match
        // to worm through (e.g. a truncated error message that
        // happens to contain "True" somewhere).
        assert!(!parse_is_admin_output(
            "The operation completed. True: false"
        ));
    }

    #[test]
    fn elevation_message_includes_concrete_next_step() {
        let msg = elevation_required_message();
        assert!(msg.contains("Administrator"), "got: {msg}");
        // The exact `Start → cmd → Ctrl+Shift+Enter` recipe is what
        // operators copy-paste. Assert specific characters so we
        // notice if the breadcrumb rot starts.
        assert!(msg.contains("Start"), "got: {msg}");
        assert!(msg.contains("Ctrl+Shift+Enter"), "got: {msg}");
    }

    #[test]
    fn bitlocker_status_command_targets_correct_drive() {
        let cmd = build_bitlocker_status_ps_command(3);
        assert!(
            cmd.contains("\\\\.\\PhysicalDrive3"),
            "must target \\\\.\\PhysicalDrive3, got: {cmd}"
        );
        assert!(cmd.contains("manage-bde"), "must invoke manage-bde: {cmd}");
    }

    #[test]
    fn classify_bitlocker_decrypted_single_volume() {
        let out = "\
            Computer Name: WIN11\n\
            Disk volumes that can be protected with\n\
            BitLocker Drive Encryption:\n\
            \n\
            Volume C: [OS]\n\
            [OS Volume]\n\
            \n\
                Size:                 128.00 GB\n\
                BitLocker Version:    2.0\n\
                Conversion Status:    Fully Decrypted\n\
                Percentage Encrypted: 0.0%\n";
        assert_eq!(
            classify_bitlocker_status(out),
            BitLockerStatus::FullyDecrypted
        );
    }

    #[test]
    fn classify_bitlocker_encrypting_is_protected() {
        let out = "\
            Volume D: [Data]\n\
                Conversion Status:    Encrypting\n\
                Percentage Encrypted: 42.0%\n";
        assert_eq!(classify_bitlocker_status(out), BitLockerStatus::Protected);
    }

    #[test]
    fn classify_bitlocker_fully_encrypted_is_protected() {
        let out = "\
            Volume E: [Secret]\n\
                Conversion Status:    Fully Encrypted\n\
                Percentage Encrypted: 100.0%\n";
        assert_eq!(classify_bitlocker_status(out), BitLockerStatus::Protected);
    }

    #[test]
    fn classify_bitlocker_no_conversion_line_is_unknown() {
        // Localized output, manage-bde missing, drive offline, etc.
        // Fail-closed behavior.
        let out = "some unrelated output\nwith no conversion status line";
        assert_eq!(classify_bitlocker_status(out), BitLockerStatus::Unknown);
        assert_eq!(classify_bitlocker_status(""), BitLockerStatus::Unknown);
    }

    #[test]
    fn classify_bitlocker_mixed_volumes_protected_wins() {
        // If the drive has two volumes, one decrypted + one
        // encrypting, the whole drive must be treated as protected.
        // `FSCTL_LOCK_VOLUME` only succeeds on the volume level;
        // the drive's overall safety is the weakest link.
        let out = "\
            Volume D: [Data1]\n\
                Conversion Status:    Fully Decrypted\n\
            Volume E: [Data2]\n\
                Conversion Status:    Encrypting\n";
        assert_eq!(classify_bitlocker_status(out), BitLockerStatus::Protected);
    }

    #[test]
    fn bitlocker_protected_message_includes_decrypt_recipe() {
        let msg = bitlocker_protected_message(5);
        assert!(msg.contains("PhysicalDrive5"), "got: {msg}");
        assert!(msg.contains("manage-bde.exe -off"), "got: {msg}");
        assert!(
            msg.contains("Fully Decrypted"),
            "remedy must mention the exact status to look for: {msg}"
        );
    }

    #[test]
    fn defender_exclusion_hint_has_add_and_remove() {
        let hint = defender_exclusion_hint(2);
        assert!(hint.contains("PhysicalDrive2"), "got: {hint}");
        assert!(hint.contains("Add-MpPreference"), "got: {hint}");
        assert!(hint.contains("Remove-MpPreference"), "got: {hint}");
    }
}
