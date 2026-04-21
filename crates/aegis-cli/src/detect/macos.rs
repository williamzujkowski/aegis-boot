// SPDX-License-Identifier: MIT OR Apache-2.0

//! macOS removable-drive detection via `diskutil list -plist` piped
//! through `plutil -convert json`.
//!
//! Why plutil: diskutil only emits XML plist. Rather than pull in a
//! plist XML parser, we hop through `plutil -convert json` (built into
//! every Mac since 10.2) and let `serde_json` do the heavy lifting. Two
//! extra subprocess invocations per enumeration, but no new deps.
//!
//! Architecture: this module splits into:
//!
//! - **Pure parsers** (`parse_list_json`, `parse_info_json`) — always
//!   compiled, testable from Linux CI via committed fixtures under
//!   `tests/fixtures/diskutil/`.
//! - **Subprocess wiring** (`list_removable_drives`) — only compiled on
//!   macOS, shells out to diskutil + plutil and routes their stdout
//!   through the pure parsers.

use super::Drive;
use std::path::PathBuf;

#[cfg(target_os = "macos")]
use std::io::Write as _;
#[cfg(target_os = "macos")]
use std::process::{Command, Stdio};

/// Parse the JSON that results from
/// `diskutil list -plist external physical | plutil -convert json -o - -`.
///
/// Returns the list of whole external physical disks with partition counts
/// and sizes. `model` is left empty here; the caller enriches it via
/// `parse_info_json`.
///
/// Always compiled so it's testable from Linux CI via committed fixtures;
/// dead on non-macOS outside tests.
#[allow(dead_code)]
#[must_use]
pub fn parse_list_json(json: &str) -> Vec<Drive> {
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let Some(disks) = parsed
        .get("AllDisksAndPartitions")
        .and_then(|v| v.as_array())
    else {
        return Vec::new();
    };

    let mut drives: Vec<Drive> = disks
        .iter()
        .filter_map(|entry| {
            let id = entry.get("DeviceIdentifier")?.as_str()?;
            let size = entry.get("Size")?.as_u64()?;
            let partitions = entry
                .get("Partitions")
                .and_then(|p| p.as_array())
                .map_or(0, Vec::len);
            Some(Drive {
                dev: PathBuf::from(format!("/dev/{id}")),
                model: String::new(),
                size_bytes: size,
                partitions,
            })
        })
        .collect();
    drives.sort_by(|a, b| a.dev.cmp(&b.dev));
    drives
}

/// Parse the JSON that results from
/// `diskutil info -plist <disk> | plutil -convert json -o - -`.
///
/// Returns a (model, removable) tuple. On parse failure returns
/// ("(unknown model)", false) so the caller can refuse a non-removable
/// disk by default.
///
/// Always compiled so it's testable from Linux CI via committed fixtures;
/// dead on non-macOS outside tests.
#[allow(dead_code)]
#[must_use]
pub fn parse_info_json(json: &str) -> (String, bool) {
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(json) else {
        return ("(unknown model)".to_string(), false);
    };
    let model = parsed
        .get("MediaName")
        .and_then(|v| v.as_str())
        .map_or_else(|| "(unknown model)".to_string(), |s| s.trim().to_string());
    // Removable on macOS: either the media is physically removable OR
    // the device advertises itself as external. Matches the Linux
    // "removable=1" semantics from sysfs.
    let removable = parsed
        .get("Removable")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
        || parsed
            .get("Ejectable")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        || parsed
            .get("RemovableMediaOrExternalDevice")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
    (model, removable)
}

/// Enumerate external physical disks on macOS.
///
/// Calls `diskutil list -plist external physical` once, then one
/// `diskutil info -plist <disk>` per whole disk to fill in the
/// `MediaName` + removable check. Disks that fail the removable check
/// are excluded — matches the Linux sysfs semantics where
/// `removable != 1` disks are filtered.
#[cfg(target_os = "macos")]
#[must_use]
pub fn list_removable_drives() -> Vec<Drive> {
    let Some(list_json) = run_diskutil_list() else {
        return Vec::new();
    };
    let mut drives = parse_list_json(&list_json);
    drives.retain_mut(|drive| {
        let Some(id) = drive.dev.file_name().and_then(|n| n.to_str()) else {
            return false;
        };
        let Some(info_json) = run_diskutil_info(id) else {
            return false;
        };
        let (model, removable) = parse_info_json(&info_json);
        if !removable {
            return false;
        }
        drive.model = model;
        true
    });
    drives
}

/// Run `diskutil list -plist external physical | plutil -convert json -o - -`
/// and return the stdout as a String. `None` on any subprocess failure
/// (caller treats as "no drives detected").
#[cfg(target_os = "macos")]
fn run_diskutil_list() -> Option<String> {
    run_and_convert(&["list", "-plist", "external", "physical"])
}

#[cfg(target_os = "macos")]
fn run_diskutil_info(disk_id: &str) -> Option<String> {
    run_and_convert(&["info", "-plist", disk_id])
}

/// Run `diskutil <args>` and pipe stdout through `plutil -convert json`.
/// Returns the JSON on success, `None` on any failure.
#[cfg(target_os = "macos")]
fn run_and_convert(diskutil_args: &[&str]) -> Option<String> {
    let diskutil = Command::new("diskutil").args(diskutil_args).output().ok()?;
    if !diskutil.status.success() {
        return None;
    }
    let mut plutil = Command::new("plutil")
        .args(["-convert", "json", "-o", "-", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    plutil.stdin.as_mut()?.write_all(&diskutil.stdout).ok()?;
    let out = plutil.wait_with_output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const LIST_FIXTURE: &str =
        include_str!("../../tests/fixtures/diskutil/list-external-physical.json");
    const INFO_FIXTURE: &str = include_str!("../../tests/fixtures/diskutil/info-disk4.json");

    #[test]
    fn parse_list_json_extracts_single_external_disk() {
        let drives = parse_list_json(LIST_FIXTURE);
        assert_eq!(drives.len(), 1);
        assert_eq!(drives[0].dev, PathBuf::from("/dev/disk4"));
        assert_eq!(drives[0].size_bytes, 16_102_883_328);
        assert_eq!(drives[0].partitions, 2);
        // Model is empty here — it's filled in via parse_info_json.
        assert!(drives[0].model.is_empty());
    }

    #[test]
    fn parse_info_json_extracts_model_and_removable_flag() {
        let (model, removable) = parse_info_json(INFO_FIXTURE);
        assert_eq!(model, "SanDisk Ultra USB 3.0 Media");
        assert!(
            removable,
            "SanDisk USB stick should be classified as removable"
        );
    }

    #[test]
    fn parse_info_json_refuses_internal_disk() {
        // Internal=true, Removable=false — classic internal SSD.
        let internal_json = r#"{
            "MediaName": "APPLE SSD AP1024Q Media",
            "Internal": true,
            "Removable": false,
            "Ejectable": false,
            "RemovableMediaOrExternalDevice": false
        }"#;
        let (model, removable) = parse_info_json(internal_json);
        assert_eq!(model, "APPLE SSD AP1024Q Media");
        assert!(!removable, "internal SSD must NOT be flagged removable");
    }

    #[test]
    fn parse_info_json_accepts_external_but_not_removable_media() {
        // External Thunderbolt/USB-C SSDs aren't "removable media" but
        // ARE "external" — we should still treat them as valid flash
        // targets.
        let external_ssd = r#"{
            "MediaName": "Samsung T7 Shield",
            "Internal": false,
            "Removable": false,
            "Ejectable": true,
            "RemovableMediaOrExternalDevice": true
        }"#;
        let (_, removable) = parse_info_json(external_ssd);
        assert!(removable, "external SSD should be accepted");
    }

    #[test]
    fn parse_list_json_empty_when_no_disks() {
        let empty = r#"{
            "AllDisks": [],
            "AllDisksAndPartitions": [],
            "VolumesFromDisks": [],
            "WholeDisks": []
        }"#;
        assert!(parse_list_json(empty).is_empty());
    }

    #[test]
    fn parse_list_json_gracefully_returns_empty_on_bad_input() {
        assert!(parse_list_json("not json").is_empty());
        assert!(parse_list_json("{}").is_empty());
        assert!(parse_list_json(r#"{"AllDisksAndPartitions": "wrong-type"}"#).is_empty());
    }

    #[test]
    fn parse_info_json_gracefully_returns_defaults_on_bad_input() {
        let (model, removable) = parse_info_json("not json");
        assert_eq!(model, "(unknown model)");
        assert!(!removable, "bad input should fail closed (not removable)");
    }
}
