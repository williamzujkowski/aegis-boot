//! Rust-native flash provisioning — foundation for #274.
//!
//! This module replaces the `scripts/mkusb.sh` + `dd` pipeline with
//! **in-place partitioning + formatting** on the target block device.
//! The provisioning surface is split into narrow helpers so each step
//! has a testable arg-builder + a thin subprocess wrapper:
//!
//!   * [`partition_stick`] — sgdisk zaps the old GPT and writes a
//!     fresh one with ESP (EF00, 400 MB) + `AEGIS_ISOS` (0700, rest).
//!   * [`format_esp`] — mkfs.fat on partition 1.
//!   * [`format_data_partition`] — mkfs.exfat on partition 2.
//!
//! ESP staging (mcopy of shim / grub / kernel / initrd) and the
//! signed-manifest attestation land in follow-up PRs under #274 —
//! see the epic for the phased rollout.
//!
//! **No caller wired up yet.** This is the foundation slice — the
//! flash command still goes through the mkusb.sh + dd path; a future
//! PR adds a `--direct-install` flag that dispatches to these helpers
//! instead. `#[allow(dead_code)]` at the module level rides until
//! that next PR drops the flag and wires the call-site.

#![cfg(target_os = "linux")]
#![allow(dead_code)]

use std::path::Path;
use std::process::Command;

/// ESP partition size in megabytes. Matches `scripts/mkusb.sh`'s
/// `ESP_SIZE_MB` default. 400 MB is enough for the signed chain
/// (shim ~1 MB, grub ~2 MB, kernel ~15 MB, initrd ~60 MB) plus
/// comfortable headroom for future binary growth.
pub(crate) const ESP_SIZE_MB: u64 = 400;

/// Label for the data partition. Keyed on by rescue-tui's ISO
/// discovery + by `aegis-boot list` / `add` mount recovery. Must
/// match `scripts/mkusb.sh`'s `DATA_LABEL` default.
pub(crate) const AEGIS_ISOS_LABEL: &str = "AEGIS_ISOS";

/// GPT partition type code for an EFI System Partition.
pub(crate) const ESP_TYPE_CODE: &str = "ef00";

/// GPT partition type code for Microsoft Basic Data. exfat and
/// fat32 both advertise under this code; aegis-boot's rescue-tui
/// fs-probe loop (`exfat → ext4 → vfat`) handles both forms so the
/// runtime doesn't key off the type code.
pub(crate) const DATA_TYPE_CODE: &str = "0700";

/// Build the argv for the partitioning sgdisk invocation. Returns an
/// owned `Vec<String>` rather than `&[&str]` so callers can embed a
/// dynamic device path (e.g. `/dev/sda`, `/dev/nvme0n1`) without
/// lifetime juggling.
///
/// The argv is split into three logical sgdisk calls in the real
/// runner, but the **table-creation** call is what this helper
/// builds — it's the one with the interesting flag layout. The
/// zap-existing + start-fresh calls are trivial (`-Z` then `-o`).
///
/// Matches `scripts/mkusb.sh:249-253`:
/// ```text
/// sgdisk -n 1:2048:+400M -t 1:ef00 -c 1:"EFI System" \
///        -n 2:0:0        -t 2:0700 -c 2:"AEGIS_ISOS" \
///        /dev/sdX
/// ```
pub(crate) fn build_partition_argv(dev_path: &str) -> Vec<String> {
    let esp_size = format!("+{ESP_SIZE_MB}M");
    vec![
        "sgdisk".to_string(),
        "-n".to_string(),
        format!("1:2048:{esp_size}"),
        "-t".to_string(),
        format!("1:{ESP_TYPE_CODE}"),
        "-c".to_string(),
        "1:EFI System".to_string(),
        "-n".to_string(),
        "2:0:0".to_string(),
        "-t".to_string(),
        format!("2:{DATA_TYPE_CODE}"),
        "-c".to_string(),
        format!("2:{AEGIS_ISOS_LABEL}"),
        dev_path.to_string(),
    ]
}

/// Zap + recreate the GPT on `dev`. Destructive — caller must have
/// operator confirmation already. Three sgdisk invocations:
///
///   1. `sgdisk -Z <dev>` — zero both primary and backup GPT headers
///      (closes the case where an old aegis-boot flash left a backup
///      GPT near the 2 GB mark that would confuse the rescan).
///   2. `sgdisk -o <dev>` — clear any remaining partition table and
///      write a fresh empty GPT.
///   3. `sgdisk -n 1:... -n 2:... <dev>` — write ESP + `AEGIS_ISOS`
///      entries (see [`build_partition_argv`]).
///
/// Linux-only. Shells out to `sudo sgdisk` because raw block-device
/// writes require root. Fails closed on any non-zero exit.
pub(crate) fn partition_stick(dev: &Path) -> Result<(), String> {
    let dev_str = dev.display().to_string();

    // 1. Zap existing GPT headers (both copies).
    run_sudo(&["sgdisk", "-Z", &dev_str]).map_err(|e| format!("sgdisk -Z: {e}"))?;

    // 2. Write a fresh empty GPT.
    run_sudo(&["sgdisk", "-o", &dev_str]).map_err(|e| format!("sgdisk -o: {e}"))?;

    // 3. Write the ESP + AEGIS_ISOS entries.
    let argv = build_partition_argv(&dev_str);
    let argv_refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    run_sudo(&argv_refs).map_err(|e| format!("sgdisk partition: {e}"))?;

    Ok(())
}

/// Format partition 1 of `dev` as FAT32 with label `AEGIS_ESP`.
/// Matches `scripts/mkusb.sh:183` (`mkfs.vfat -F 32 -n AEGIS_ESP`).
///
/// Caller is responsible for having already run [`partition_stick`]
/// and for having released any desktop auto-mounts (see the #273
/// `unmount_if_mounted` pattern) before calling this.
pub(crate) fn format_esp(part1_path: &Path) -> Result<(), String> {
    let p = part1_path.display().to_string();
    run_sudo(&["mkfs.fat", "-F", "32", "-n", "AEGIS_ESP", &p]).map_err(|e| format!("mkfs.fat: {e}"))
}

/// Format partition 2 of `dev` as exFAT with label `AEGIS_ISOS`.
/// Matches `scripts/mkusb.sh:211` (`mkfs.exfat -L "$DATA_LABEL"`).
///
/// Same caller contract as [`format_esp`] — partitioning done,
/// automounts released.
pub(crate) fn format_data_partition(part2_path: &Path) -> Result<(), String> {
    let p = part2_path.display().to_string();
    run_sudo(&["mkfs.exfat", "-L", AEGIS_ISOS_LABEL, &p]).map_err(|e| format!("mkfs.exfat: {e}"))
}

/// Run `sudo <argv>` and return Ok if the exit status is success.
/// Thin wrapper — shared between the sgdisk / mkfs subprocess calls
/// above. Kept local to this module rather than reused from flash.rs
/// to avoid a cross-module sudo-helper coupling that would outlive
/// its usefulness.
fn run_sudo(argv: &[&str]) -> Result<(), String> {
    let out = Command::new("sudo")
        .args(argv)
        .output()
        .map_err(|e| format!("{} exec failed: {e}", argv.first().unwrap_or(&"?")))?;
    if !out.status.success() {
        return Err(format!(
            "{} exited {}: {}",
            argv.first().unwrap_or(&"?"),
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
    fn esp_size_matches_mkusb_default() {
        // mkusb.sh:34 hardcodes ESP_SIZE_MB=400. Drift-test so a
        // surprise change in either source is caught here.
        assert_eq!(ESP_SIZE_MB, 400);
    }

    #[test]
    fn aegis_isos_label_matches_mkusb_default() {
        // mkusb.sh's DATA_LABEL default is AEGIS_ISOS; rescue-tui's
        // iso discovery + aegis-boot list's mount recovery both key
        // off this exact string. Drift would silently break boot.
        assert_eq!(AEGIS_ISOS_LABEL, "AEGIS_ISOS");
    }

    #[test]
    fn esp_and_data_type_codes_match_mkusb() {
        // mkusb.sh:251 uses ef00 for ESP; mkusb.sh:244-247 uses 0700
        // for both fat32 and exfat data partitions. exfat is the
        // post-#243 default; sticks carry 0700 regardless of which
        // fs we put on top (the fs driver recognizes the magic).
        assert_eq!(ESP_TYPE_CODE, "ef00");
        assert_eq!(DATA_TYPE_CODE, "0700");
    }

    #[test]
    fn build_partition_argv_starts_with_sgdisk() {
        let argv = build_partition_argv("/dev/sda");
        assert_eq!(argv.first().map(String::as_str), Some("sgdisk"));
    }

    #[test]
    fn build_partition_argv_embeds_esp_size_as_plus_mb() {
        // sgdisk's `-n 1:start:+400M` form grows partition 1 from
        // start to start+400M. Must match mkusb.sh's invocation
        // shape or the produced partition tables diverge.
        let argv = build_partition_argv("/dev/sda");
        let expected = format!("1:2048:+{ESP_SIZE_MB}M");
        assert!(
            argv.iter().any(|a| a == &expected),
            "expected {expected:?} in argv: {argv:?}"
        );
    }

    #[test]
    fn build_partition_argv_uses_defaults_for_data_partition() {
        // sgdisk's `-n 2:0:0` form means partition 2, default start
        // (= end of partition 1 + alignment), default end (= last
        // usable sector = rest of disk). Auto-fills to exactly the
        // available space, which is what makes direct-install on
        // any stick size work without custom math.
        let argv = build_partition_argv("/dev/sda");
        assert!(
            argv.iter().any(|a| a == "2:0:0"),
            "expected 2:0:0 in argv: {argv:?}"
        );
    }

    #[test]
    fn build_partition_argv_sets_both_type_codes() {
        let argv = build_partition_argv("/dev/sda");
        assert!(
            argv.iter().any(|a| a == "1:ef00"),
            "expected 1:ef00 (ESP type) in argv: {argv:?}"
        );
        assert!(
            argv.iter().any(|a| a == "2:0700"),
            "expected 2:0700 (Microsoft Basic Data) in argv: {argv:?}"
        );
    }

    #[test]
    fn build_partition_argv_labels_match_mkusb() {
        // `c 1:"EFI System"` is the partition *name* (GPT partition
        // entry name field) — not the filesystem label. mkusb.sh
        // emits "EFI System" verbatim and update.rs's check_eligibility
        // parses sgdisk's partition-name column to match it.
        let argv = build_partition_argv("/dev/sda");
        assert!(
            argv.iter().any(|a| a == "1:EFI System"),
            "expected 1:EFI System in argv: {argv:?}"
        );
        assert!(
            argv.iter().any(|a| a == "2:AEGIS_ISOS"),
            "expected 2:AEGIS_ISOS in argv: {argv:?}"
        );
    }

    #[test]
    fn build_partition_argv_puts_device_path_last() {
        // sgdisk's convention: flags first, target device path
        // last. If the device path slips into the middle, sgdisk
        // may silently pick a different target (or error out
        // depending on the flag order); either is worse than
        // failing the argv-build test.
        let argv = build_partition_argv("/dev/sda");
        assert_eq!(argv.last().map(String::as_str), Some("/dev/sda"));

        // Works for NVMe-style paths too — the function is path-
        // agnostic; the caller resolves the disk device. (The
        // partition helpers use partition paths, not the disk.)
        let argv = build_partition_argv("/dev/nvme0n1");
        assert_eq!(argv.last().map(String::as_str), Some("/dev/nvme0n1"));
    }

    #[test]
    fn build_partition_argv_contains_both_partition_specs() {
        // Regression guard: the argv must include both -n flags
        // (one for ESP, one for data). A future refactor that drops
        // either would still pass most of the per-flag tests above
        // but would produce a single-partition stick that can't boot.
        let argv = build_partition_argv("/dev/sda");
        let n_flags = argv.iter().filter(|a| a.as_str() == "-n").count();
        assert_eq!(n_flags, 2, "expected exactly two -n flags: {argv:?}");
    }
}
