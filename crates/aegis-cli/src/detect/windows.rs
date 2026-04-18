//! Windows removable-drive detection via `PowerShell` `Get-Disk`.
//!
//! `Get-Disk | ConvertTo-Json -Depth 2` emits a JSON array of physical
//! disks with `Number`, `FriendlyName`, `Size`, `BusType`,
//! `NumberOfPartitions`, and (critically) `IsBoot` / `IsSystem` flags.
//! We filter to disks with `BusType == "USB"` and reject `IsBoot` /
//! `IsSystem` drives so we can't accidentally overwrite the operator's
//! OS.
//!
//! Writing to the raw disk on Windows is a separate problem (no native
//! `dd`) and is out of scope for this PR — [`list_removable_drives`]
//! exists today; `flash` on Windows still errors with a Rufus /
//! `dd-for-Windows` hint.
//!
//! Architecture matches the macOS module: a pure
//! [`parse_get_disk_json`] tested via committed fixtures on Linux CI,
//! plus a Windows-only [`list_removable_drives`] that shells out.

use super::Drive;
use std::path::PathBuf;

#[cfg(target_os = "windows")]
use std::process::Command;

/// Parse the JSON that `Get-Disk | ConvertTo-Json -Depth 2` emits.
///
/// Filters to `BusType == "USB"` disks that are neither `IsBoot` nor
/// `IsSystem`. Returns `/dev/diskN` style paths (matching the macOS
/// convention) where N is the Windows `Number`; the caller's flash path
/// converts these to `\\.\PhysicalDriveN` for the actual write call.
///
/// Always compiled so it's testable from non-Windows CI via committed
/// fixtures.
#[allow(dead_code)]
#[must_use]
pub fn parse_get_disk_json(json: &str) -> Vec<Drive> {
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    // Get-Disk produces an array; a single-disk system may emit a bare
    // object instead, so normalize.
    let disks: Vec<&serde_json::Value> = match &parsed {
        serde_json::Value::Array(a) => a.iter().collect(),
        serde_json::Value::Object(_) => vec![&parsed],
        _ => return Vec::new(),
    };

    let mut drives: Vec<Drive> = disks
        .iter()
        .filter_map(|entry| {
            let bus = entry.get("BusType")?.as_str()?;
            if bus != "USB" {
                return None;
            }
            if entry
                .get("IsBoot")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
            {
                return None;
            }
            if entry
                .get("IsSystem")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
            {
                return None;
            }
            let number = entry.get("Number")?.as_u64()?;
            let size = entry.get("Size")?.as_u64()?;
            let model = entry
                .get("FriendlyName")
                .or_else(|| entry.get("Model"))
                .and_then(|v| v.as_str())
                .unwrap_or("(unknown USB disk)")
                .trim()
                .to_string();
            let partitions = entry
                .get("NumberOfPartitions")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0)
                .try_into()
                .unwrap_or(0_usize);
            Some(Drive {
                // Windows drive path. Flash path will rewrite to
                // \\.\PhysicalDriveN before passing to the writer —
                // kept as a friendly pseudo-path here for display.
                dev: PathBuf::from(format!("\\\\.\\PhysicalDrive{number}")),
                model,
                size_bytes: size,
                partitions,
            })
        })
        .collect();
    drives.sort_by(|a, b| a.dev.cmp(&b.dev));
    drives
}

/// Enumerate removable USB disks on Windows.
///
/// Invokes `powershell -NoProfile -NonInteractive -Command "Get-Disk | ConvertTo-Json -Depth 2"`
/// and routes the stdout through [`parse_get_disk_json`]. Returns an
/// empty list on any subprocess failure so callers surface the generic
/// "no drives detected" hint.
#[cfg(target_os = "windows")]
#[must_use]
pub fn list_removable_drives() -> Vec<Drive> {
    let Some(json) = run_get_disk() else {
        return Vec::new();
    };
    parse_get_disk_json(&json)
}

#[cfg(target_os = "windows")]
fn run_get_disk() -> Option<String> {
    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Get-Disk | ConvertTo-Json -Depth 2",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const BUSTYPE_FIXTURE: &str =
        include_str!("../../tests/fixtures/windows/get-disk-bustype.json");

    #[test]
    fn parse_get_disk_filters_non_usb_disks() {
        let drives = parse_get_disk_json(BUSTYPE_FIXTURE);
        // Fixture has 3 disks: 1 NVMe (system), 2 USB. Only 2 USB
        // should survive; NVMe is BusType!=USB AND IsBoot+IsSystem.
        assert_eq!(drives.len(), 2);
        let sandisk = drives
            .iter()
            .find(|d| d.model.contains("SanDisk"))
            .unwrap();
        assert_eq!(sandisk.dev, PathBuf::from("\\\\.\\PhysicalDrive2"));
        assert_eq!(sandisk.size_bytes, 16_102_883_328);
        assert_eq!(sandisk.partitions, 2);

        let t7 = drives
            .iter()
            .find(|d| d.model.contains("T7 Shield"))
            .unwrap();
        assert_eq!(t7.dev, PathBuf::from("\\\\.\\PhysicalDrive3"));
    }

    #[test]
    fn parse_get_disk_refuses_boot_disk_even_if_usb() {
        // A USB disk that's also `IsBoot` or `IsSystem` (rare but
        // possible on a machine that boots from a USB key) must not
        // be offered as a flash target. We'd destroy the operator's
        // OS.
        let boot_usb = r#"[{
            "Number": 5,
            "FriendlyName": "Boot USB",
            "BusType": "USB",
            "Size": 32000000000,
            "NumberOfPartitions": 1,
            "IsBoot": true,
            "IsSystem": false
        }]"#;
        let drives = parse_get_disk_json(boot_usb);
        assert!(drives.is_empty(), "IsBoot=true USB disk must not be listed");

        let sys_usb = r#"[{
            "Number": 5,
            "FriendlyName": "System USB",
            "BusType": "USB",
            "Size": 32000000000,
            "NumberOfPartitions": 1,
            "IsBoot": false,
            "IsSystem": true
        }]"#;
        assert!(parse_get_disk_json(sys_usb).is_empty());
    }

    #[test]
    fn parse_get_disk_handles_single_disk_bare_object() {
        // PowerShell emits a bare object (not an array) when there's
        // exactly one disk. The parser must normalize.
        let single = r#"{
            "Number": 4,
            "FriendlyName": "Lone USB",
            "BusType": "USB",
            "Size": 8000000000,
            "NumberOfPartitions": 0,
            "IsBoot": false,
            "IsSystem": false
        }"#;
        let drives = parse_get_disk_json(single);
        assert_eq!(drives.len(), 1);
        assert_eq!(drives[0].model, "Lone USB");
    }

    #[test]
    fn parse_get_disk_gracefully_returns_empty_on_bad_input() {
        assert!(parse_get_disk_json("not json").is_empty());
        assert!(parse_get_disk_json("null").is_empty());
        assert!(parse_get_disk_json("[]").is_empty());
    }

    #[test]
    fn parse_get_disk_uses_friendly_name_fallback_to_model() {
        // FriendlyName may be empty on some drivers; `Model` is a
        // reasonable fallback.
        let no_friendly = r#"[{
            "Number": 1,
            "Model": "Kingston DataTraveler 3.0",
            "BusType": "USB",
            "Size": 64000000000,
            "NumberOfPartitions": 1,
            "IsBoot": false,
            "IsSystem": false
        }]"#;
        let drives = parse_get_disk_json(no_friendly);
        assert_eq!(drives.len(), 1);
        assert_eq!(drives[0].model, "Kingston DataTraveler 3.0");
    }
}
