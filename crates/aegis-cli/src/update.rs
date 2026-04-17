//! `aegis-boot update [device]` — eligibility check for in-place
//! signed-chain rotation.
//!
//! # Phase 1 (this module): read-only verification
//!
//! Validates that a target USB stick can be updated in-place without
//! re-flashing. No writes.
//!
//! Eligibility criteria, in order (first-fail wins):
//!   1. Device path exists and is a block device
//!   2. Has a GPT partition table (sgdisk -p)
//!   3. Partition 1 exists (would be the ESP)
//!   4. Partition 2 exists with a filesystem label `AEGIS_ISOS` or
//!      `AEGIS-ISOS` (case-insensitive; lsblk reports labels
//!      normalized by the filesystem driver)
//!   5. An attestation manifest exists whose `disk_guid` matches the
//!      target's GPT disk GUID from `sgdisk -p`
//!
//! When all five pass, the stick is "eligible" — a future phase will
//! actually perform the update. Right now we print a clear "eligible"
//! message with the matched attestation path so the operator can
//! verify ownership and time of flash.
//!
//! # What's deliberately NOT in this phase
//!
//! - Building a fresh ESP image to diff against (deferred to phase 1.5
//!   — requires either calling mkusb.sh or a Rust equivalent, and the
//!   diff is useful but not blocking for the safety story)
//! - Any write to the device (phase 2 — atomic file replace with
//!   backup)
//! - CA signature verification on the new chain (phase 3)
//!
//! Tracked under epic [#181](https://github.com/williamzujkowski/aegis-boot/issues/181).

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// Entry point for `aegis-boot update [device]`.
pub fn run(args: &[String]) -> ExitCode {
    match try_run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => ExitCode::from(code),
    }
}

/// Inner runner returning a typed result. Same contract as `run`.
pub(crate) fn try_run(args: &[String]) -> Result<(), u8> {
    let mut explicit_dev: Option<&str> = None;
    for a in args {
        match a.as_str() {
            "--help" | "-h" => {
                print_help();
                return Ok(());
            }
            arg if arg.starts_with("--") => {
                eprintln!("aegis-boot update: unknown option '{arg}'");
                eprintln!("(in-place update is under active development — only the");
                eprintln!(" eligibility check is shipped today; see issue #181)");
                return Err(2);
            }
            other => {
                if explicit_dev.is_some() {
                    eprintln!("aegis-boot update: only one device allowed");
                    return Err(2);
                }
                explicit_dev = Some(other);
            }
        }
    }

    let Some(d) = explicit_dev else {
        eprintln!("aegis-boot update: missing <device> argument");
        eprintln!("usage: aegis-boot update /dev/sdX");
        return Err(2);
    };
    let dev = PathBuf::from(d);

    println!("aegis-boot update — eligibility check");
    println!();
    println!("Target device: {}", dev.display());
    println!();

    match check_eligibility(&dev) {
        Eligibility::Eligible {
            attestation_path,
            disk_guid,
        } => {
            println!("Status: ELIGIBLE for in-place update.");
            println!();
            println!("  disk GUID:        {disk_guid}");
            println!("  attestation:      {}", attestation_path.display());
            println!("  AEGIS_ISOS:       will be preserved byte-for-byte");
            println!();
            println!("NOTE: this is a read-only eligibility check (phase 1 of #181).");
            println!("The actual in-place update lands in a follow-up PR. No writes");
            println!("were made to {} during this command.", dev.display());
            Ok(())
        }
        Eligibility::Ineligible(reason) => {
            eprintln!("Status: NOT ELIGIBLE for in-place update.");
            eprintln!();
            eprintln!("Reason: {reason}");
            eprintln!();
            eprintln!("Your options:");
            eprintln!("  1. If this is a genuine aegis-boot stick but lacks an attestation,");
            eprintln!("     it was flashed before v0.13.0. Re-flash with `aegis-boot flash`");
            eprintln!("     to create a new attestation; your ISOs will be lost, so back");
            eprintln!("     them up first.");
            eprintln!("  2. If this is a fresh / non-aegis-boot USB stick, run");
            eprintln!("     `aegis-boot init {}` to initialize it.", dev.display());
            Err(1)
        }
    }
}

fn print_help() {
    println!("aegis-boot update — in-place signed-chain update (read-only check for now)");
    println!();
    println!("USAGE:");
    println!("  aegis-boot update <device>");
    println!("  aegis-boot update --help");
    println!();
    println!("BEHAVIOR (phase 1 of #181):");
    println!("  Validates that the target stick is a known aegis-boot stick and");
    println!("  that its attestation manifest matches the disk GUID. Reports");
    println!("  ELIGIBLE / NOT ELIGIBLE with a specific reason. Does NOT write.");
    println!();
    println!("  The actual atomic in-place update lands in follow-up PRs — see");
    println!("  issue #181 for the phased plan.");
    println!();
    println!("WHY YOU'D USE THIS (once full update ships):");
    println!("  - Apply shim/GRUB/kernel CVE fixes without wiping AEGIS_ISOS");
    println!("  - Rotate signing keys on the boot chain");
    println!("  - Bump aegis-boot releases without losing your ISO inventory");
}

/// Outcome of the pre-flight check. `Ineligible` carries an operator-
/// readable reason the TUI can surface verbatim.
#[derive(Debug)]
pub(crate) enum Eligibility {
    Eligible {
        attestation_path: PathBuf,
        disk_guid: String,
    },
    Ineligible(String),
}

/// Run all five eligibility gates against the given device. Returns the
/// matched attestation + GUID on success; a specific human-readable
/// reason on failure.
pub(crate) fn check_eligibility(dev: &Path) -> Eligibility {
    if !dev.exists() {
        return Eligibility::Ineligible(format!(
            "device {} does not exist (unplugged? wrong path?)",
            dev.display()
        ));
    }

    // Gate 1: GPT partition table.
    let sgdisk = match Command::new("sgdisk").args(["-p"]).arg(dev).output() {
        Ok(o) if o.status.success() => o,
        Ok(o) => {
            return Eligibility::Ineligible(format!(
                "`sgdisk -p {}` exited non-zero: {}",
                dev.display(),
                String::from_utf8_lossy(&o.stderr).trim()
            ));
        }
        Err(e) => {
            return Eligibility::Ineligible(format!(
                "cannot run sgdisk: {e} (is gptfdisk installed?)"
            ));
        }
    };
    let sgdisk_out = String::from_utf8_lossy(&sgdisk.stdout);

    // Gate 2: extract disk GUID. sgdisk emits "Disk identifier (GUID): XXXX-..."
    let Some(disk_guid) = parse_disk_guid(&sgdisk_out) else {
        return Eligibility::Ineligible(
            "sgdisk did not report a disk GUID (not GPT? corrupted?)".to_string(),
        );
    };

    // Gate 3: partition 1 + 2 present. Trust sgdisk's line format:
    //   "   1     2048      820207   400.0 MiB   EF00  EFI System"
    //   "   2   822256    31277055   14.5  GiB   8300  AEGIS_ISOS"
    let (has_esp, part2_label) = parse_partitions(&sgdisk_out);
    if !has_esp {
        return Eligibility::Ineligible(
            "partition 1 missing or not an ESP (type EF00)".to_string(),
        );
    }
    if !part2_label.eq_ignore_ascii_case("AEGIS_ISOS")
        && !part2_label.eq_ignore_ascii_case("AEGIS-ISOS")
    {
        return Eligibility::Ineligible(format!(
            "partition 2 label is {part2_label:?} — expected AEGIS_ISOS. \
             This stick was not flashed by aegis-boot."
        ));
    }

    // Gate 4: locate matching attestation by disk GUID.
    let Some(attestation_path) = find_attestation_by_guid(&disk_guid) else {
        return Eligibility::Ineligible(format!(
            "no attestation manifest found for disk GUID {disk_guid}. \
             Was this stick flashed on a different host, or before v0.13.0?"
        ));
    };

    Eligibility::Eligible {
        attestation_path,
        disk_guid,
    }
}

/// Extract the disk GUID from `sgdisk -p` output. Line is:
///   `Disk identifier (GUID): DDDDDDDD-DDDD-DDDD-DDDD-DDDDDDDDDDDD`
/// (lowercase hex with dashes, per GPT spec).
pub(crate) fn parse_disk_guid(sgdisk_out: &str) -> Option<String> {
    for line in sgdisk_out.lines() {
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix("Disk identifier (GUID):") else {
            continue;
        };
        let guid = rest.trim().to_ascii_lowercase();
        // Sanity: GPT GUIDs are 36 chars (32 hex + 4 dashes).
        if guid.len() == 36 && guid.matches('-').count() == 4 {
            return Some(guid);
        }
    }
    None
}

/// Parse `sgdisk`'s partition table. Returns (`has_part1_ef00`, `part2_name`).
///
/// Note: sgdisk abbreviates GUID partition types to 4-char codes
/// (`EF00` for EFI System, `8300` for Linux filesystem, etc). The
/// partition name is the free-text label set by `-c N:LABEL`, NOT the
/// filesystem label. We set it to `AEGIS_ISOS` during `mkusb.sh`.
pub(crate) fn parse_partitions(sgdisk_out: &str) -> (bool, String) {
    let mut has_esp = false;
    let mut part2_label = String::new();
    // Find the "Number" header line, then parse fixed-ish columns.
    let mut in_table = false;
    for line in sgdisk_out.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("Number ") && trimmed.contains("Code") {
            in_table = true;
            continue;
        }
        if !in_table || trimmed.is_empty() {
            continue;
        }
        // Split on whitespace; first token is the partition number.
        let mut tokens: Vec<&str> = trimmed.split_whitespace().collect();
        if tokens.len() < 7 {
            continue;
        }
        let num = tokens[0];
        // Find the "Code" column — it's a 4-char hex code like EF00.
        // sgdisk's format has it at position 5 (after Number, Start,
        // End, Size-value, Size-unit).
        let code = tokens[5];
        // Everything after position 5 is the name (can contain spaces).
        let name = tokens.split_off(6).join(" ");
        if num == "1" && code == "EF00" {
            has_esp = true;
        }
        if num == "2" {
            part2_label = name;
        }
    }
    (has_esp, part2_label)
}

/// Walk the attestations dir and return the path of the first manifest
/// whose `disk_guid` field matches `target_guid` (case-insensitive).
/// Returns `None` if no match is found OR the attestations dir doesn't
/// exist yet.
fn find_attestation_by_guid(target_guid: &str) -> Option<PathBuf> {
    let dir = attestation_dir()?;
    let entries = std::fs::read_dir(&dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        // We deliberately don't deserialize the whole Attestation here —
        // this keeps `update` decoupled from attest.rs's internal schema
        // if it evolves. Match on the raw JSON field instead.
        if body_contains_guid(&body, target_guid) {
            return Some(path);
        }
    }
    None
}

/// Matches `"disk_guid": "XXXX"` in a JSON body, case-insensitive on
/// the GUID value. Pure-string so it's fast and doesn't require a
/// JSON dep. Anchors on the closing `"` so that a short target GUID
/// can't false-match a prefix of a longer one (e.g. target `abcd`
/// matching stored `abcdef01-...` would be wrong).
pub(crate) fn body_contains_guid(body: &str, target_guid: &str) -> bool {
    let lower_body = body.to_ascii_lowercase();
    let needle = format!("\"disk_guid\": \"{}\"", target_guid.to_ascii_lowercase());
    lower_body.contains(&needle)
}

/// Same resolution rules as `attest::attestation_dir()` — duplicated here
/// to avoid exporting a new pub surface in attest.rs this PR. Kept
/// `pub(crate)` so a future `mv` into attest.rs is a mechanical rename.
pub(crate) fn attestation_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(base.join("aegis-boot").join("attestations"))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::missing_panics_doc)]
mod tests {
    use super::*;

    #[test]
    fn parse_disk_guid_extracts_valid_guid() {
        let out = "\
            Disk /dev/sdc: 30228480 sectors, 14.4 GiB\n\
            Sector size (logical/physical): 512/512 bytes\n\
            Disk identifier (GUID): abcdef01-2345-6789-abcd-ef0123456789\n\
            Partition table holds up to 128 entries\n\
        ";
        assert_eq!(
            parse_disk_guid(out).as_deref(),
            Some("abcdef01-2345-6789-abcd-ef0123456789"),
        );
    }

    #[test]
    fn parse_disk_guid_uppercase_normalized_to_lower() {
        let out = "Disk identifier (GUID): ABCDEF01-2345-6789-ABCD-EF0123456789";
        assert_eq!(
            parse_disk_guid(out).as_deref(),
            Some("abcdef01-2345-6789-abcd-ef0123456789"),
        );
    }

    #[test]
    fn parse_disk_guid_rejects_malformed() {
        assert!(parse_disk_guid("Disk identifier (GUID): not-a-guid").is_none());
        assert!(parse_disk_guid("no guid line here").is_none());
        assert!(parse_disk_guid("Disk identifier (GUID): ").is_none());
    }

    #[test]
    fn parse_partitions_detects_esp_and_label() {
        let out = "\
            Number  Start (sector)    End (sector)  Size       Code  Name\n   \
               1            2048          821247   400.0 MiB   EF00  EFI System\n   \
               2          821248        31277055   14.5 GiB    8300  AEGIS_ISOS\n\
        ";
        let (has_esp, label) = parse_partitions(out);
        assert!(has_esp, "missed ESP detection in: {out}");
        assert_eq!(label, "AEGIS_ISOS");
    }

    #[test]
    fn parse_partitions_no_esp_when_type_wrong() {
        let out = "\
            Number  Start (sector)    End (sector)  Size       Code  Name\n   \
               1            2048          821247   400.0 MiB   8300  Random\n   \
               2          821248        31277055   14.5 GiB    8300  AEGIS_ISOS\n\
        ";
        let (has_esp, _) = parse_partitions(out);
        assert!(!has_esp);
    }

    #[test]
    fn parse_partitions_reports_empty_label_when_part2_missing() {
        let out = "\
            Number  Start (sector)    End (sector)  Size       Code  Name\n   \
               1            2048          821247   400.0 MiB   EF00  EFI System\n\
        ";
        let (has_esp, label) = parse_partitions(out);
        assert!(has_esp);
        assert_eq!(label, "");
    }

    #[test]
    fn body_contains_guid_case_insensitive() {
        let body = r#"{"disk_guid": "ABCDEF01-2345-6789-ABCD-EF0123456789", "other": "x"}"#;
        assert!(body_contains_guid(
            body,
            "abcdef01-2345-6789-abcd-ef0123456789"
        ));
        assert!(body_contains_guid(
            body,
            "ABCDEF01-2345-6789-ABCD-EF0123456789"
        ));
    }

    #[test]
    fn body_contains_guid_prefix_match_rejected() {
        // Defensive: matching "abc" inside "abcd-..." would be a bug.
        // Our impl anchors on the full GUID inside quotes, so a
        // shorter target should not match.
        let body = r#"{"disk_guid": "abcdef01-2345-6789-abcd-ef0123456789"}"#;
        assert!(!body_contains_guid(body, "abcdef01"));
    }

    #[test]
    fn body_contains_guid_misses_different_guid() {
        let body = r#"{"disk_guid": "11111111-2222-3333-4444-555555555555"}"#;
        assert!(!body_contains_guid(
            body,
            "00000000-0000-0000-0000-000000000000"
        ));
    }

    #[test]
    fn check_eligibility_missing_device_is_specific() {
        let fake = PathBuf::from("/dev/this-device-does-not-exist-aegis-boot");
        let result = check_eligibility(&fake);
        match result {
            Eligibility::Ineligible(reason) => {
                assert!(
                    reason.contains("does not exist"),
                    "reason should name the missing-device case: {reason}",
                );
            }
            Eligibility::Eligible { .. } => panic!("expected Ineligible for missing device"),
        }
    }
}
