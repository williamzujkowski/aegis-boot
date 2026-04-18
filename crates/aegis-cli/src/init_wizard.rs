//! `aegis-boot init` wizard helpers — pure logic for the
//! serial-confirmation safety gate (#245).
//!
//! No callers wired up in this PR. The interactive prompt that
//! consumes these helpers lands in a follow-up so the logic gets full
//! unit-test coverage before any operator-visible behaviour change.
//!
//! # Why this scope
//!
//! Wrong-device `dd` is the catastrophic failure mode for every USB
//! tool. The single highest-leverage safety improvement we can ship is
//! a typed serial-confirmation prompt: the operator picks `/dev/sda`
//! from a numbered list, sees the device's serial number, and types
//! the last 4 characters back to confirm. Catches the "I meant to
//! type sdb but typed sda" finger-fumble that brings down every USB
//! tool.
//!
//! # Why no `dialoguer` dep yet
//!
//! The interactive surface (read from stdin, redraw on miss, support
//! Ctrl-C) needs careful UX work. Shipping the pure logic first lets
//! us land tests against `serial_token` / `serial_matches` /
//! `parse_menu_selection` / `is_target_mounted` *now*, then choose
//! the prompt library (or hand-roll on `std::io::stdin`) in PR2 with
//! a clean unit-tested foundation underneath.

#![allow(dead_code)] // Foundation PR — callers wired in follow-up. See module docs.

use serde::Deserialize;
use std::path::PathBuf;

/// One USB drive presented to the operator in the wizard menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsbDrive {
    /// Block-device path (e.g. `/dev/sda`).
    pub dev: PathBuf,
    /// Vendor + model string (e.g. `"SanDisk Cruzer"`).
    pub model: String,
    /// Capacity in bytes.
    pub size_bytes: u64,
    /// Hardware serial number, if the kernel exposed it.
    /// Used as the input source for the confirmation token —
    /// `None` falls back to a refusal-with-clear-message rather than
    /// allowing an unconfirmed flash.
    pub serial: Option<String>,
}

impl UsbDrive {
    /// Human-readable capacity. Same shape as `detect::Drive::size_human`.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn size_human(&self) -> String {
        let gb = self.size_bytes as f64 / 1_073_741_824.0;
        if gb >= 1.0 {
            format!("{gb:.1} GB")
        } else {
            let mb = self.size_bytes as f64 / 1_048_576.0;
            format!("{mb:.0} MB")
        }
    }
}

// ---- lsblk JSON parsing ----------------------------------------------------

#[derive(Deserialize)]
struct LsblkRoot {
    blockdevices: Vec<LsblkDev>,
}

#[derive(Deserialize)]
struct LsblkDev {
    name: String,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    serial: Option<String>,
    #[serde(default)]
    rm: Option<bool>,
    #[serde(default)]
    tran: Option<String>,
}

/// Parse `lsblk -J -b -o NAME,SIZE,MODEL,SERIAL,RM,TRAN` output and
/// return only removable USB devices.
///
/// Filters by `rm == true` AND `tran == "usb"` — matches the same
/// filter the existing sysfs-based detect path applies, but recovers
/// the serial number which sysfs scanning didn't surface.
///
/// Returns drives sorted by `dev` so the menu order is stable across
/// invocations.
///
/// # Errors
///
/// Returns the wrapped `serde_json::Error` on malformed input. Empty
/// `blockdevices` arrays return `Ok(vec![])` (no devices, not an error).
pub fn parse_lsblk_removable_usb(json: &str) -> Result<Vec<UsbDrive>, serde_json::Error> {
    let root: LsblkRoot = serde_json::from_str(json)?;
    let mut drives: Vec<UsbDrive> = root
        .blockdevices
        .into_iter()
        .filter(|d| d.rm.unwrap_or(false))
        .filter(|d| d.tran.as_deref() == Some("usb"))
        .map(|d| UsbDrive {
            dev: PathBuf::from(format!("/dev/{}", d.name)),
            model: d.model.map(|m| m.trim().to_string()).unwrap_or_default(),
            size_bytes: d.size.unwrap_or(0),
            serial: d
                .serial
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
        })
        .collect();
    drives.sort_by(|a, b| a.dev.cmp(&b.dev));
    Ok(drives)
}

// ---- serial-confirmation logic ---------------------------------------------

/// Number of trailing alphanumeric characters the operator must type
/// to confirm the device. 4 is a Goldilocks number: short enough to
/// type in one second on a tired-at-2-AM keyboard, long enough that
/// the chance of two attached USB sticks colliding on the last 4 is
/// vanishingly small.
pub const SERIAL_CONFIRMATION_LEN: usize = 4;

/// Compute the confirmation token for a serial number — the last
/// `SERIAL_CONFIRMATION_LEN` alphanumeric characters, lowercased.
///
/// Strips non-alphanumerics first so a serial like `4C53-9173` and
/// `4C539173` produce the same token. Returns `None` when the serial
/// is too short (under 4 alphanumeric chars after stripping) — the
/// caller should refuse to proceed in that case.
///
/// Examples:
///   - `"4C5301AABBCC9173"` → `Some("9173")`
///   - `"AKE74Z1Y00098765"` → `Some("8765")`
///   - `"abc"` (3 chars)    → `None`
#[must_use]
pub fn serial_token(serial: &str) -> Option<String> {
    let alnum: String = serial
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect();
    if alnum.len() < SERIAL_CONFIRMATION_LEN {
        return None;
    }
    Some(alnum[alnum.len() - SERIAL_CONFIRMATION_LEN..].to_string())
}

/// Check whether the operator-typed input matches the expected
/// confirmation token. Trims whitespace, lowercases, and ignores
/// non-alphanumerics on BOTH sides so `9173`, ` 9173 `, and `9-1-7-3`
/// all match `9173`. Pure exact-match-after-normalization — no
/// Levenshtein fuzziness, by design (the gate exists to catch typos,
/// not be permissive about them). Normalising the expected token is
/// purely defensive — callers should always source it from
/// `serial_token` which already produces lowercase alphanumerics.
#[must_use]
pub fn serial_matches(input: &str, expected_token: &str) -> bool {
    let normalize = |s: &str| -> String {
        s.chars()
            .filter(char::is_ascii_alphanumeric)
            .map(|c| c.to_ascii_lowercase())
            .collect()
    };
    let n_in = normalize(input);
    let n_exp = normalize(expected_token);
    !n_in.is_empty() && n_in == n_exp
}

// ---- menu rendering + selection parsing ------------------------------------

/// Render the numbered drive-selection menu. Two-column layout:
/// `[N] /dev/sdX  <model>  <size>   serial: <serial>`. Drives without
/// a serial show `serial: (unknown)` — the caller refuses to proceed
/// on those rather than letting an unconfirmable flash slip through.
///
/// One drive per line; trailing newline included.
#[must_use]
pub fn format_drive_menu(drives: &[UsbDrive]) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    if drives.is_empty() {
        let _ = writeln!(s, "No removable USB drives detected.");
        return s;
    }
    let _ = writeln!(s, "Removable USB drives detected:");
    let _ = writeln!(s);
    for (i, d) in drives.iter().enumerate() {
        let serial_str = d.serial.as_deref().unwrap_or("(unknown)");
        let _ = writeln!(
            s,
            "  [{}] {:<10} {:<24} {:>8}   serial: {}",
            i + 1,
            d.dev.display(),
            d.model,
            d.size_human(),
            serial_str
        );
    }
    s
}

/// Parse a 1-indexed menu selection (e.g. `"1"`, `" 2 "`, `"3\n"`)
/// into a 0-indexed array offset, or `None` if the input is empty,
/// non-numeric, zero, or > `max_n`. `max_n` is exclusive of zero —
/// pass `drives.len()` directly.
#[must_use]
pub fn parse_menu_selection(input: &str, max_n: usize) -> Option<usize> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    let n: usize = trimmed.parse().ok()?;
    if n == 0 || n > max_n {
        return None;
    }
    Some(n - 1)
}

// ---- mounted-target detection ---------------------------------------------

#[derive(Deserialize)]
struct FindmntRoot {
    #[serde(default)]
    filesystems: Vec<FindmntEntry>,
}

#[derive(Deserialize)]
struct FindmntEntry {
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    source: Option<String>,
}

/// Returns true when the parsed `findmnt -J <device>` output reports
/// at least one mounted filesystem on the given device or any of its
/// partitions. Empty `filesystems` array → `false` (not mounted).
///
/// The wizard refuses to proceed when this returns `true`, with an
/// `--force` override path. Catches the case where the operator forgot
/// to eject the previous stick.
///
/// # Errors
///
/// Returns the wrapped `serde_json::Error` on malformed input.
pub fn is_target_mounted(findmnt_json: &str) -> Result<bool, serde_json::Error> {
    let root: FindmntRoot = serde_json::from_str(findmnt_json)?;
    Ok(!root.filesystems.is_empty())
}

// ---- trust-narrative paragraph --------------------------------------------

/// Trust-narrative shown once during `init` (then never again — operators
/// don't want a wall of text every flash). Mirrors the language in
/// `docs/HOW_IT_WORKS.md` so the operator hears the same words at flash
/// time and at first boot. Single source of truth (#245 + #248).
#[must_use]
pub fn trust_narrative_paragraph() -> &'static str {
    "About the trust story:\n\
     \n\
     aegis-boot installs a signed boot chain (shim → grub → kernel) onto\n\
     partition 1 of this stick. The signed chain refuses to boot if anyone\n\
     tampers with it. ISOs you add later get hashed and listed in a signed\n\
     manifest you can verify with `aegis-boot attest show`.\n\
     \n\
     For the full trust model, run `aegis-boot tour` or read\n\
     docs/HOW_IT_WORKS.md."
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    // ---- parse_lsblk_removable_usb ----

    #[test]
    fn parse_lsblk_filters_to_removable_usb() {
        let json = r#"{
          "blockdevices": [
            {"name":"sda","size":31914983424,"model":"Cruzer","serial":"4C530001234567","rm":true,"tran":"usb"},
            {"name":"sdb","size":1000204886016,"model":"Internal","serial":"WD-XYZ","rm":false,"tran":"sata"},
            {"name":"sdc","size":15728640000,"model":"DataTrav","serial":"408D5A41E","rm":true,"tran":"usb"},
            {"name":"nvme0n1","size":256000000000,"model":"Samsung","serial":"S4-NVME","rm":false,"tran":"nvme"}
          ]
        }"#;
        let drives = parse_lsblk_removable_usb(json).unwrap();
        assert_eq!(drives.len(), 2);
        assert_eq!(drives[0].dev, PathBuf::from("/dev/sda"));
        assert_eq!(drives[0].model, "Cruzer");
        assert_eq!(drives[0].serial.as_deref(), Some("4C530001234567"));
        assert_eq!(drives[1].dev, PathBuf::from("/dev/sdc"));
    }

    #[test]
    fn parse_lsblk_orders_by_dev_name() {
        let json = r#"{
          "blockdevices": [
            {"name":"sdc","size":1,"model":"X","serial":"X","rm":true,"tran":"usb"},
            {"name":"sda","size":1,"model":"Y","serial":"Y","rm":true,"tran":"usb"},
            {"name":"sdb","size":1,"model":"Z","serial":"Z","rm":true,"tran":"usb"}
          ]
        }"#;
        let drives = parse_lsblk_removable_usb(json).unwrap();
        let names: Vec<_> = drives.iter().map(|d| d.dev.display().to_string()).collect();
        assert_eq!(names, vec!["/dev/sda", "/dev/sdb", "/dev/sdc"]);
    }

    #[test]
    fn parse_lsblk_handles_missing_optional_fields() {
        // Older lsblk or USB device without a serial — should produce
        // a UsbDrive with serial: None, not error out.
        let json = r#"{
          "blockdevices": [
            {"name":"sda","size":1024,"rm":true,"tran":"usb"}
          ]
        }"#;
        let drives = parse_lsblk_removable_usb(json).unwrap();
        assert_eq!(drives.len(), 1);
        assert!(drives[0].serial.is_none());
        assert!(drives[0].model.is_empty());
    }

    #[test]
    fn parse_lsblk_blank_serial_normalizes_to_none() {
        let json = r#"{
          "blockdevices": [
            {"name":"sda","size":1024,"model":"X","serial":"   ","rm":true,"tran":"usb"}
          ]
        }"#;
        let drives = parse_lsblk_removable_usb(json).unwrap();
        assert!(drives[0].serial.is_none());
    }

    #[test]
    fn parse_lsblk_rejects_malformed_json() {
        assert!(parse_lsblk_removable_usb("{not json}").is_err());
    }

    #[test]
    fn parse_lsblk_empty_blockdevices_returns_empty_vec() {
        let drives = parse_lsblk_removable_usb(r#"{"blockdevices": []}"#).unwrap();
        assert!(drives.is_empty());
    }

    // ---- serial_token ----

    #[test]
    fn serial_token_extracts_last_four_alnum() {
        assert_eq!(serial_token("4C5301AABBCC9173"), Some("9173".to_string()));
    }

    #[test]
    fn serial_token_strips_dashes_and_spaces() {
        assert_eq!(serial_token("4C53-01AA-9173"), Some("9173".to_string()));
        assert_eq!(serial_token(" 4 C 5 3 9 1 7 3"), Some("9173".to_string()));
    }

    #[test]
    fn serial_token_lowercases_alphabetic_chars() {
        assert_eq!(serial_token("DEADBEEF"), Some("beef".to_string()));
    }

    #[test]
    fn serial_token_returns_none_when_too_short() {
        assert_eq!(serial_token("abc"), None);
        assert_eq!(serial_token(""), None);
        assert_eq!(serial_token("---"), None);
    }

    #[test]
    fn serial_token_accepts_exactly_four_chars() {
        assert_eq!(serial_token("9173"), Some("9173".to_string()));
    }

    // ---- serial_matches ----

    #[test]
    fn serial_matches_exact() {
        assert!(serial_matches("9173", "9173"));
    }

    #[test]
    fn serial_matches_case_insensitive() {
        assert!(serial_matches("BEEF", "beef"));
        assert!(serial_matches("beef", "BEEF"));
    }

    #[test]
    fn serial_matches_strips_whitespace_and_punctuation() {
        assert!(serial_matches(" 9173 ", "9173"));
        assert!(serial_matches("9-1-7-3", "9173"));
        assert!(serial_matches("9173\n", "9173"));
    }

    #[test]
    fn serial_matches_rejects_wrong_token() {
        assert!(!serial_matches("9174", "9173"));
        assert!(!serial_matches("917", "9173"));
        assert!(!serial_matches("91733", "9173"));
    }

    #[test]
    fn serial_matches_rejects_empty_input() {
        assert!(!serial_matches("", "9173"));
        assert!(!serial_matches("   ", "9173"));
    }

    // ---- parse_menu_selection ----

    #[test]
    fn parse_menu_selection_one_indexed_to_zero_indexed() {
        assert_eq!(parse_menu_selection("1", 3), Some(0));
        assert_eq!(parse_menu_selection("3", 3), Some(2));
    }

    #[test]
    fn parse_menu_selection_strips_whitespace() {
        assert_eq!(parse_menu_selection(" 2 \n", 3), Some(1));
    }

    #[test]
    fn parse_menu_selection_rejects_zero_and_oob() {
        assert_eq!(parse_menu_selection("0", 3), None);
        assert_eq!(parse_menu_selection("4", 3), None);
        assert_eq!(parse_menu_selection("99", 3), None);
    }

    #[test]
    fn parse_menu_selection_rejects_non_numeric() {
        assert_eq!(parse_menu_selection("a", 3), None);
        assert_eq!(parse_menu_selection("1a", 3), None);
        assert_eq!(parse_menu_selection("", 3), None);
    }

    // ---- format_drive_menu ----

    #[test]
    fn format_drive_menu_empty_says_so() {
        let s = format_drive_menu(&[]);
        assert!(s.contains("No removable USB drives detected"), "got: {s}");
    }

    #[test]
    fn format_drive_menu_renders_each_drive_on_its_own_line() {
        let drives = vec![
            UsbDrive {
                dev: PathBuf::from("/dev/sda"),
                model: "Cruzer".into(),
                size_bytes: 31_914_983_424,
                serial: Some("4C530001234567".into()),
            },
            UsbDrive {
                dev: PathBuf::from("/dev/sdc"),
                model: "DataTrav".into(),
                size_bytes: 15_728_640_000,
                serial: Some("408D5A41E".into()),
            },
        ];
        let s = format_drive_menu(&drives);
        assert!(s.contains("[1]"), "got: {s}");
        assert!(s.contains("[2]"), "got: {s}");
        assert!(s.contains("/dev/sda"), "got: {s}");
        assert!(s.contains("/dev/sdc"), "got: {s}");
        assert!(s.contains("Cruzer"), "got: {s}");
        assert!(s.contains("4C530001234567"), "got: {s}");
    }

    #[test]
    fn format_drive_menu_renders_unknown_serial_explicitly() {
        let drives = vec![UsbDrive {
            dev: PathBuf::from("/dev/sda"),
            model: "X".into(),
            size_bytes: 1024,
            serial: None,
        }];
        let s = format_drive_menu(&drives);
        assert!(s.contains("(unknown)"), "got: {s}");
    }

    // ---- is_target_mounted ----

    #[test]
    fn is_target_mounted_true_when_filesystems_nonempty() {
        let json = r#"{"filesystems": [{"target":"/mnt/aegis-isos","source":"/dev/sda2"}]}"#;
        assert!(is_target_mounted(json).unwrap());
    }

    #[test]
    fn is_target_mounted_false_when_filesystems_empty() {
        assert!(!is_target_mounted(r#"{"filesystems": []}"#).unwrap());
    }

    #[test]
    fn is_target_mounted_false_when_filesystems_missing() {
        // findmnt with no match returns {} (no filesystems key at all).
        assert!(!is_target_mounted("{}").unwrap());
    }

    #[test]
    fn is_target_mounted_rejects_malformed_json() {
        assert!(is_target_mounted("not json").is_err());
    }

    // ---- trust_narrative_paragraph ----

    #[test]
    fn trust_narrative_mentions_signed_chain_and_attestation() {
        let p = trust_narrative_paragraph();
        assert!(p.contains("signed boot chain"), "got: {p}");
        assert!(p.contains("attest"), "got: {p}");
        assert!(p.contains("HOW_IT_WORKS.md"), "got: {p}");
    }

    // ---- size_human ----

    #[test]
    fn size_human_formats_gb_for_large_drives() {
        let d = UsbDrive {
            dev: PathBuf::from("/dev/sda"),
            model: "X".into(),
            size_bytes: 31_914_983_424,
            serial: None,
        };
        assert!(d.size_human().ends_with(" GB"), "got: {}", d.size_human());
    }

    #[test]
    fn size_human_formats_mb_for_small_drives() {
        let d = UsbDrive {
            dev: PathBuf::from("/dev/sda"),
            model: "X".into(),
            size_bytes: 256 * 1_048_576,
            serial: None,
        };
        assert!(d.size_human().ends_with(" MB"), "got: {}", d.size_human());
    }
}
