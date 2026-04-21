// SPDX-License-Identifier: MIT OR Apache-2.0

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
//!   * [`render_grub_cfg`] — emit the 3-entry rescue-tui menu.
//!   * [`combine_initrd`] — concat distro initrd + aegis-boot initramfs.
//!   * [`stage_esp`] — mmd + mcopy the signed chain onto partition 1.
//!
//! The signed-manifest attestation lives in
//! [`crate::direct_install_manifest`] — see the epic for the phased
//! rollout.
//!
//! Wired into `aegis-boot flash --direct-install` (#274 Phase 3) via
//! [`crate::flash::flash_direct_install`]. The legacy `mkusb.sh + dd`
//! path is still the default; direct-install is opt-in until the
//! 10-run green-streak gate from #274 Phase 4 flips it.

#![cfg(target_os = "linux")]

use std::fs;
use std::path::Path;
use std::process::Command;

/// Re-export the ESP partition size from the shared constants
/// registry. See [`crate::constants::ESP_SIZE_MB`] for the rationale
/// and [#286] for the single-source-of-truth pattern this implements.
///
/// [#286]: https://github.com/williamzujkowski/aegis-boot/issues/286
pub(crate) use crate::constants::ESP_SIZE_MB;

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

/// Re-export the grub menu timeout from the shared constants
/// registry. See [`crate::constants::GRUB_TIMEOUT_SECS`].
pub(crate) use crate::constants::GRUB_TIMEOUT_SECS;

/// Default menuentry selected on boot. 0 = tty0-primary rescue (the
/// right choice for a local-monitor operator). Matches mkusb.sh's
/// `MKUSB_GRUB_DEFAULT:-0` fallback. Operators needing the
/// serial-primary variant (no local monitor) set the same
/// `MKUSB_GRUB_DEFAULT` env var that mkusb.sh honors — see
/// [`resolve_grub_default_entry`]. Same knob → same value →
/// byte-identical grub.cfg across both flash paths, which is the
/// byte-parity invariant the direct-install E2E asserts.
pub(crate) const GRUB_DEFAULT_ENTRY: u32 = 0;

/// Resolve the grub default-entry index honoring the same
/// `MKUSB_GRUB_DEFAULT` env var that `scripts/mkusb.sh` consumes.
/// Values outside the known menuentry range (0, 1, 2) fall back to
/// [`GRUB_DEFAULT_ENTRY`] silently — operators passing a bogus value
/// should get the safe default, not a flash-time failure.
pub(crate) fn resolve_grub_default_entry() -> u32 {
    resolve_grub_default_entry_from(std::env::var("MKUSB_GRUB_DEFAULT").ok().as_deref())
}

/// Pure form for unit testing. The crate is `forbid(unsafe_code)`, so
/// tests can't use `env::set_var` (unsafe in edition 2024) — passing
/// the env value as an argument keeps the predicate testable without
/// touching process-global state.
pub(crate) fn resolve_grub_default_entry_from(env_value: Option<&str>) -> u32 {
    env_value
        .and_then(|s| s.parse::<u32>().ok())
        .filter(|&n| n <= 2)
        .unwrap_or(GRUB_DEFAULT_ENTRY)
}

/// Render the 3-entry rescue-tui grub menu to `out`. Matches the
/// literal text produced by `scripts/mkusb.sh:145-178`, including
/// the `MKUSB_GRUB_DEFAULT` override hook — the byte-parity CI gate
/// in `.github/workflows/direct-install-e2e.yml` confirms this.
///
/// Kept as a standalone text render rather than a template file so
/// direct-install stays buildable without an asset directory and the
/// output is unit-testable on content invariants without file fs.
pub(crate) fn render_grub_cfg(out: &Path) -> Result<(), String> {
    let body = build_grub_cfg_body(GRUB_TIMEOUT_SECS, resolve_grub_default_entry());
    fs::write(out, body).map_err(|e| format!("grub.cfg write {}: {e}", out.display()))
}

/// Pure builder: returns the grub.cfg body string for the given
/// timeout + default entry. Split out from [`render_grub_cfg`] so the
/// content can be asserted in unit tests without touching the fs.
///
/// BYTE-PARITY INVARIANT: every character of this output — including
/// the explanatory comments before each menuentry — must match
/// `scripts/mkusb.sh:145-178` verbatim. The direct-install E2E
/// (`.github/workflows/direct-install-e2e.yml`) asserts sha256 parity
/// between the two paths' ESPs; a one-character drift in a comment
/// fails the gate. If you need to change a comment, change mkusb.sh
/// and this function in the same commit.
pub(crate) fn build_grub_cfg_body(timeout_secs: u32, default_entry: u32) -> String {
    format!(
        "set timeout={timeout_secs}
set default={default_entry}

# Normal boot — concise kernel logs.
# console= order MATTERS: last one wins as /dev/console for userspace.
# We want tty0 (local monitor) as the default rescue-tui target on
# real-hardware boots; kernel still echoes to all console= targets
# so a serial operator gets dmesg + can edit grub to flip the order.
# (#112)
menuentry \"aegis-boot rescue\" {{
    linux /vmlinuz console=ttyS0,115200 console=tty0 panic=5 loglevel=4
    initrd /initrd.img
}}

# Serial-primary variant — for operators using a serial console or a
# KVM IP console with no local monitor. rescue-tui's alt-screen
# renders on ttyS0.
menuentry \"aegis-boot rescue (serial-primary)\" {{
    linux /vmlinuz console=tty0 console=ttyS0,115200 panic=5 loglevel=4
    initrd /initrd.img
}}

# Verbose boot (#109 shakedown) — loglevel=7, earlyprintk, and
# AEGIS_BOOT_VERBOSE=1 causes /init to pause 30s after diagnostics so
# the operator can read the pre-rescue-tui state on screen. Also tees
# the /init log to /run/media/aegis-isos/aegis-boot-<ts>.log.
menuentry \"aegis-boot rescue (verbose — first-boot debug)\" {{
    linux /vmlinuz console=ttyS0,115200 console=tty0 panic=30 loglevel=7 earlyprintk=efi ignore_loglevel aegis.verbose=1
    initrd /initrd.img
}}
"
    )
}

/// Concatenate `distro_initrd || aegis_initrd` into `out`. The kernel
/// unpacks concatenated cpio archives in order, so the distro's
/// driver payload is live before the aegis-boot `/init` runs — matches
/// `scripts/mkusb.sh:114-115`.
pub(crate) fn combine_initrd(
    distro_initrd: &Path,
    aegis_initrd: &Path,
    out: &Path,
) -> Result<(), String> {
    let distro =
        fs::read(distro_initrd).map_err(|e| format!("read {}: {e}", distro_initrd.display()))?;
    let aegis =
        fs::read(aegis_initrd).map_err(|e| format!("read {}: {e}", aegis_initrd.display()))?;
    let mut combined = Vec::with_capacity(distro.len() + aegis.len());
    combined.extend_from_slice(&distro);
    combined.extend_from_slice(&aegis);
    fs::write(out, &combined).map_err(|e| format!("write {}: {e}", out.display()))?;
    Ok(())
}

/// Sources for the signed chain staged onto an ESP partition. Bundled
/// into a struct rather than passed as 6 positional args so call sites
/// read as `sources.shim` / `sources.kernel` (and to stay inside the
/// clippy `too_many_arguments` budget).
#[derive(Debug, Clone)]
pub(crate) struct EspStagingSources<'a> {
    pub shim: &'a Path,
    pub grub: &'a Path,
    pub kernel: &'a Path,
    pub combined_initrd: &'a Path,
    pub grub_cfg: &'a Path,
}

/// Canonical ESP destination paths (as mcopy `::/` targets).
/// Exported as constants so both `stage_esp` and the attestation
/// manifest in Phase 2c can key off the same closed-list ordering.
pub(crate) const ESP_DEST_SHIM: &str = "::/EFI/BOOT/BOOTX64.EFI";
pub(crate) const ESP_DEST_GRUB: &str = "::/EFI/BOOT/grubx64.efi";
pub(crate) const ESP_DEST_GRUB_CFG_BOOT: &str = "::/EFI/BOOT/grub.cfg";
pub(crate) const ESP_DEST_GRUB_CFG_UBUNTU: &str = "::/EFI/ubuntu/grub.cfg";
pub(crate) const ESP_DEST_KERNEL: &str = "::/vmlinuz";
pub(crate) const ESP_DEST_INITRD: &str = "::/initrd.img";

/// Build the argv for a single mcopy invocation that writes `src` to
/// `dest` on the FAT32 image or block device at `image_or_dev`.
///
/// `--` is inserted before the positional args so that a path
/// beginning with `-` can't be misread by mcopy as an option flag.
/// `-D o` means "overwrite destination if it exists" (mkusb.sh relies
/// on freshly-formatted ESP so there's nothing to overwrite, but
/// direct-install's idempotent replay path needs this).
pub(crate) fn build_mcopy_argv(image_or_dev: &str, src: &Path, dest: &str) -> Vec<String> {
    vec![
        "mcopy".to_string(),
        "-i".to_string(),
        image_or_dev.to_string(),
        "-D".to_string(),
        "o".to_string(),
        "--".to_string(),
        src.display().to_string(),
        dest.to_string(),
    ]
}

/// Build the argv for `mmd` — creates directories on a FAT image
/// without failing if they already exist (via `-D s` = skip existing).
pub(crate) fn build_mmd_argv(image_or_dev: &str, dirs: &[&str]) -> Vec<String> {
    let mut argv = vec![
        "mmd".to_string(),
        "-i".to_string(),
        image_or_dev.to_string(),
        "-D".to_string(),
        "s".to_string(),
        "--".to_string(),
    ];
    for d in dirs {
        argv.push((*d).to_string());
    }
    argv
}

/// Stage the signed chain onto partition 1 of the target stick.
///
/// Calls `mmd` once to create `::/EFI ::/EFI/BOOT ::/EFI/ubuntu`, then
/// six `mcopy` invocations laying the signed chain down in the layout
/// required by the firmware + shim + grub:
///
/// | destination                | source                         |
/// | -------------------------- | ------------------------------ |
/// | `::/EFI/BOOT/BOOTX64.EFI`  | `sources.shim`                 |
/// | `::/EFI/BOOT/grubx64.efi`  | `sources.grub`                 |
/// | `::/EFI/BOOT/grub.cfg`     | `sources.grub_cfg`             |
/// | `::/EFI/ubuntu/grub.cfg`   | `sources.grub_cfg`             |
/// | `::/vmlinuz`               | `sources.kernel`               |
/// | `::/initrd.img`            | `sources.combined_initrd`      |
///
/// Matches `scripts/mkusb.sh:185-191`.
///
/// `part1_dev` is the ESP partition path (e.g. `/dev/sda1`) or, for
/// test fixtures, a path to a FAT32 image file. mcopy acts on either
/// transparently. Caller is responsible for having released any
/// desktop auto-mounts on the device — see the #273 pattern.
pub(crate) fn stage_esp(part1_dev: &Path, sources: &EspStagingSources<'_>) -> Result<(), String> {
    let dev = part1_dev.display().to_string();

    // Create ESP directory skeleton (idempotent).
    let mmd = build_mmd_argv(&dev, &["::/EFI", "::/EFI/BOOT", "::/EFI/ubuntu"]);
    let mmd_refs: Vec<&str> = mmd.iter().map(String::as_str).collect();
    run_sudo(&mmd_refs).map_err(|e| format!("mmd: {e}"))?;

    // Six mcopy writes in a fixed order matching mkusb.sh.
    for (src, dest) in [
        (sources.shim, ESP_DEST_SHIM),
        (sources.grub, ESP_DEST_GRUB),
        (sources.grub_cfg, ESP_DEST_GRUB_CFG_BOOT),
        (sources.grub_cfg, ESP_DEST_GRUB_CFG_UBUNTU),
        (sources.kernel, ESP_DEST_KERNEL),
        (sources.combined_initrd, ESP_DEST_INITRD),
    ] {
        let argv = build_mcopy_argv(&dev, src, dest);
        let argv_refs: Vec<&str> = argv.iter().map(String::as_str).collect();
        run_sudo(&argv_refs).map_err(|e| format!("mcopy {dest}: {e}"))?;
    }

    Ok(())
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

    // ---- Phase 2b: stage_esp helpers ---------------------------------------

    #[test]
    fn build_mcopy_argv_starts_with_mcopy_and_minus_i() {
        let argv = build_mcopy_argv("/dev/sda1", Path::new("/tmp/shim.efi"), ESP_DEST_SHIM);
        assert_eq!(argv.first().map(String::as_str), Some("mcopy"));
        assert_eq!(argv.get(1).map(String::as_str), Some("-i"));
        assert_eq!(argv.get(2).map(String::as_str), Some("/dev/sda1"));
    }

    #[test]
    fn build_mcopy_argv_inserts_double_dash_before_positional_args() {
        // `--` stops mcopy from interpreting a path that starts with
        // `-` as a flag. Defense against argv-injection on paths we
        // don't fully control in downstream phases (e.g. a user-named
        // ISO subfolder in a future direct-install layout).
        let argv = build_mcopy_argv("/dev/sda1", Path::new("-rogue"), ESP_DEST_SHIM);
        let dd_idx = argv.iter().position(|a| a == "--").expect("-- delimiter");
        let src_idx = argv.iter().position(|a| a == "-rogue").expect("src");
        assert!(dd_idx < src_idx, "`--` must precede the src path: {argv:?}");
    }

    #[test]
    fn build_mcopy_argv_uses_overwrite_mode() {
        // `-D o` = overwrite destination. Idempotent replay requires
        // this; mkusb.sh doesn't need it because it formats first,
        // but direct-install's re-stage-on-retry path would otherwise
        // fail on an existing file.
        let argv = build_mcopy_argv("/dev/sda1", Path::new("/tmp/x"), "::/x");
        let d_idx = argv.iter().position(|a| a == "-D").expect("-D flag");
        assert_eq!(argv.get(d_idx + 1).map(String::as_str), Some("o"));
    }

    #[test]
    fn build_mmd_argv_creates_all_requested_dirs() {
        let argv = build_mmd_argv("/dev/sda1", &["::/EFI", "::/EFI/BOOT", "::/EFI/ubuntu"]);
        assert_eq!(argv.first().map(String::as_str), Some("mmd"));
        // `-D s` = skip existing directories so a replay on a
        // partial-stage doesn't fail on the first `mmd`.
        let d_idx = argv.iter().position(|a| a == "-D").expect("-D flag");
        assert_eq!(argv.get(d_idx + 1).map(String::as_str), Some("s"));
        assert!(argv.iter().any(|a| a == "::/EFI"));
        assert!(argv.iter().any(|a| a == "::/EFI/BOOT"));
        assert!(argv.iter().any(|a| a == "::/EFI/ubuntu"));
    }

    #[test]
    fn esp_destination_paths_match_mkusb_layout() {
        // Drift guard — these six paths are the contract between the
        // ESP staging layer and the firmware + shim + grub chain.
        // mkusb.sh:186-191 writes to these exact paths. Any change
        // here without a corresponding change to mkusb.sh (or vice
        // versa) would produce a stick that doesn't boot.
        assert_eq!(ESP_DEST_SHIM, "::/EFI/BOOT/BOOTX64.EFI");
        assert_eq!(ESP_DEST_GRUB, "::/EFI/BOOT/grubx64.efi");
        assert_eq!(ESP_DEST_GRUB_CFG_BOOT, "::/EFI/BOOT/grub.cfg");
        assert_eq!(ESP_DEST_GRUB_CFG_UBUNTU, "::/EFI/ubuntu/grub.cfg");
        assert_eq!(ESP_DEST_KERNEL, "::/vmlinuz");
        assert_eq!(ESP_DEST_INITRD, "::/initrd.img");
    }

    #[test]
    fn build_grub_cfg_body_contains_three_menuentries() {
        let body = build_grub_cfg_body(GRUB_TIMEOUT_SECS, GRUB_DEFAULT_ENTRY);
        let count = body.matches("menuentry ").count();
        assert_eq!(count, 3, "expected exactly 3 menuentries in grub.cfg");
    }

    #[test]
    fn build_grub_cfg_body_sets_timeout_and_default() {
        let body = build_grub_cfg_body(7, 2);
        assert!(body.starts_with("set timeout=7\n"), "body: {body}");
        assert!(body.contains("\nset default=2\n"), "body: {body}");
    }

    #[test]
    fn build_grub_cfg_body_references_kernel_and_initrd_by_mkusb_paths() {
        // The kernel and initrd are written to `::/vmlinuz` and
        // `::/initrd.img` — grub sees them as `/vmlinuz` + `/initrd.img`.
        // Drift between these paths and the mcopy destinations
        // (ESP_DEST_KERNEL / ESP_DEST_INITRD) would silently break boot.
        let body = build_grub_cfg_body(GRUB_TIMEOUT_SECS, GRUB_DEFAULT_ENTRY);
        assert!(body.contains("linux /vmlinuz"), "body: {body}");
        assert!(body.contains("initrd /initrd.img"), "body: {body}");
    }

    #[test]
    fn build_grub_cfg_body_includes_serial_and_verbose_variants() {
        // The rescue-tui's screen layout assumes both console orders
        // are bootable so operators on a serial-only KVM can flip
        // into the serial-primary entry. The verbose entry is #109's
        // first-boot debug path. Missing either is a UX regression.
        let body = build_grub_cfg_body(GRUB_TIMEOUT_SECS, GRUB_DEFAULT_ENTRY);
        assert!(
            body.contains("serial-primary"),
            "missing serial-primary menuentry: {body}"
        );
        assert!(
            body.contains("verbose"),
            "missing verbose menuentry: {body}"
        );
        assert!(
            body.contains("aegis.verbose=1"),
            "missing aegis.verbose flag on verbose entry: {body}"
        );
    }

    #[test]
    fn render_grub_cfg_writes_expected_body_to_disk() {
        // tempfile::TempDir rather than std::env::temp_dir so the
        // test fs ops happen inside a private, auto-cleaned 0700 dir
        // (semgrep rust.lang.security.temp-dir flags the latter).
        //
        // Note on env-dependence: render_grub_cfg now reads
        // MKUSB_GRUB_DEFAULT via resolve_grub_default_entry(). We
        // compare against resolve_grub_default_entry() rather than
        // the bare GRUB_DEFAULT_ENTRY constant so a dev running with
        // MKUSB_GRUB_DEFAULT=1 in their shell doesn't fail this test
        // falsely — the rendering contract is "render whatever
        // resolver returns," not "always render 0."
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("grub.cfg");
        render_grub_cfg(&path).expect("render_grub_cfg");
        let written = std::fs::read_to_string(&path).expect("read back grub.cfg");
        let expected = build_grub_cfg_body(GRUB_TIMEOUT_SECS, resolve_grub_default_entry());
        assert_eq!(written, expected);
    }

    #[test]
    fn build_grub_cfg_body_matches_mkusb_sh_byte_for_byte() {
        // Committed reference of the exact byte sequence scripts/mkusb.sh
        // emits for the grub.cfg at MKUSB_GRUB_DEFAULT=1 / timeout=3.
        // The direct-install E2E asserts sha256 parity between the two
        // paths' ESPs (`.github/workflows/direct-install-e2e.yml`); this
        // test catches drift at `cargo test` time instead of after a
        // 2-minute E2E round trip. If mkusb.sh changes, update both
        // files in the same commit.
        let expected = "\
set timeout=3
set default=1

# Normal boot — concise kernel logs.
# console= order MATTERS: last one wins as /dev/console for userspace.
# We want tty0 (local monitor) as the default rescue-tui target on
# real-hardware boots; kernel still echoes to all console= targets
# so a serial operator gets dmesg + can edit grub to flip the order.
# (#112)
menuentry \"aegis-boot rescue\" {
    linux /vmlinuz console=ttyS0,115200 console=tty0 panic=5 loglevel=4
    initrd /initrd.img
}

# Serial-primary variant — for operators using a serial console or a
# KVM IP console with no local monitor. rescue-tui's alt-screen
# renders on ttyS0.
menuentry \"aegis-boot rescue (serial-primary)\" {
    linux /vmlinuz console=tty0 console=ttyS0,115200 panic=5 loglevel=4
    initrd /initrd.img
}

# Verbose boot (#109 shakedown) — loglevel=7, earlyprintk, and
# AEGIS_BOOT_VERBOSE=1 causes /init to pause 30s after diagnostics so
# the operator can read the pre-rescue-tui state on screen. Also tees
# the /init log to /run/media/aegis-isos/aegis-boot-<ts>.log.
menuentry \"aegis-boot rescue (verbose — first-boot debug)\" {
    linux /vmlinuz console=ttyS0,115200 console=tty0 panic=30 loglevel=7 earlyprintk=efi ignore_loglevel aegis.verbose=1
    initrd /initrd.img
}
";
        let got = build_grub_cfg_body(3, 1);
        assert_eq!(
            got, expected,
            "grub.cfg drift from mkusb.sh — byte-parity E2E will fail. \
             Update scripts/mkusb.sh:145-178 and this test together."
        );
    }

    #[test]
    fn resolve_grub_default_entry_honors_env_values() {
        // Byte-parity invariant: same knob mkusb.sh honors must select
        // the same grub.cfg content in direct-install. Unset env =
        // fallback to GRUB_DEFAULT_ENTRY (0); "1" → serial-primary;
        // "2" → verbose; out-of-range / malformed → safe fallback.
        assert_eq!(resolve_grub_default_entry_from(None), GRUB_DEFAULT_ENTRY);
        assert_eq!(resolve_grub_default_entry_from(Some("0")), 0);
        assert_eq!(resolve_grub_default_entry_from(Some("1")), 1);
        assert_eq!(resolve_grub_default_entry_from(Some("2")), 2);
        assert_eq!(
            resolve_grub_default_entry_from(Some("3")),
            GRUB_DEFAULT_ENTRY,
            "out-of-range value falls back — no flash-time failure"
        );
        assert_eq!(
            resolve_grub_default_entry_from(Some("not-a-number")),
            GRUB_DEFAULT_ENTRY,
            "malformed value falls back silently"
        );
        assert_eq!(
            resolve_grub_default_entry_from(Some("")),
            GRUB_DEFAULT_ENTRY,
            "empty string falls back silently"
        );
    }

    #[test]
    fn combine_initrd_concats_distro_then_aegis() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let distro = tmp.path().join("distro.img");
        let aegis = tmp.path().join("aegis.img");
        let out = tmp.path().join("combined.img");

        std::fs::write(&distro, b"DISTRO_PAYLOAD").expect("write distro");
        std::fs::write(&aegis, b"AEGIS_PAYLOAD").expect("write aegis");
        combine_initrd(&distro, &aegis, &out).expect("combine_initrd");

        let got = std::fs::read(&out).expect("read combined");
        assert_eq!(got, b"DISTRO_PAYLOADAEGIS_PAYLOAD");
    }

    #[test]
    fn combine_initrd_rejects_missing_input() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let distro = tmp.path().join("does-not-exist.img");
        let aegis = tmp.path().join("aegis.img");
        let out = tmp.path().join("combined.img");

        std::fs::write(&aegis, b"aegis").expect("write aegis");
        let err = combine_initrd(&distro, &aegis, &out).expect_err("should fail");
        assert!(err.contains("does-not-exist.img"), "err: {err}");
    }

    #[test]
    fn esp_staging_sources_is_debug_and_clone() {
        // The struct is Debug+Clone because Phase 2c's attestation
        // helpers will capture the staging sources by value; easier
        // to pass a clone than force lifetime gymnastics through the
        // signing layer.
        let p = Path::new("/tmp/x");
        let s = EspStagingSources {
            shim: p,
            grub: p,
            kernel: p,
            combined_initrd: p,
            grub_cfg: p,
        };
        let cloned = s.clone();
        let debug = format!("{cloned:?}");
        assert!(debug.contains("EspStagingSources"));
    }
}
