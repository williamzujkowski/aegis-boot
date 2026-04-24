// SPDX-License-Identifier: MIT OR Apache-2.0

//! #497 — Drive enumeration for the Windows `--direct-install`
//! pipeline.
//!
//! Operator-facing "what drive should I flash?" prompt needs to
//! enumerate physical disks, filter out the boot drive (disk 0) and
//! internal SATA/NVMe drives, and surface a short list of safe
//! candidates (typically 1 USB stick, occasionally 2 if the operator
//! has two inserted).
//!
//! ## Approach
//!
//! `Get-Disk | ConvertTo-Json` gives us everything we need in one
//! shot:
//! - `Number` (u32) — the `\\.\PhysicalDriveN` suffix
//! - `FriendlyName` (string) — e.g. "QEMU HARDDISK", "`SanDisk` Cruzer"
//! - `Size` (u64 bytes) — used to filter out zero-byte or absurdly
//!   large "drives" (`Get-Disk` occasionally surfaces virtual/LUN
//!   entries that shouldn't be flashable)
//! - `BusType` (u16) — `Get-Disk` exposes this as an integer enum; 7 is
//!   `USB`, 11 is `SATA`, 17 is `NVMe`, etc. We prefer `USB` but don't
//!   require it (advanced users running a Win11 VM against a host
//!   `SATA` scratch disk would otherwise be filtered out incorrectly).
//! - `IsBoot` / `IsSystem` (bool) — defense in depth past "disk 0 is
//!   always boot" since operators may have re-numbered disks.
//!
//! ## Scope
//!
//! Pure-fn JSON parser + filter is unit-testable on Linux against
//! canned `Get-Disk` output. The subprocess wrapper that invokes
//! `PowerShell` is Windows-gated, mirroring the pattern in
//! [`super::preflight`] and [`super::format`].

#![allow(dead_code)]

use serde::Deserialize;

/// `PowerShell` command that emits one flat JSON array of disk
/// records, suitable for piping into [`parse_disks_json`]. Kept as
/// a pure-fn constant so tests can reason about the command shape
/// without spawning `PowerShell`.
pub(crate) const GET_DISKS_PS_COMMAND: &str = concat!(
    // `-CompressOutput` avoids pretty-printed JSON blowing up the
    // subprocess buffer. `-Depth 3` covers the fields we consume
    // without dragging in PartitionStyle nested structs.
    "Get-Disk | ",
    "Select-Object Number, FriendlyName, Size, BusType, IsBoot, IsSystem, IsOffline, IsReadOnly, PartitionStyle | ",
    // `@()` wraps the output so single-disk systems emit an array of
    // length 1, not a bare object. Without this, ConvertTo-Json
    // returns an object for 1-disk outputs and our serde parser
    // chokes. Known PowerShell quirk.
    "ForEach-Object { $_ } | ",
    "ConvertTo-Json -Compress -Depth 3 -AsArray"
);

/// One physical disk the Windows host can see. Populated from
/// `Get-Disk` JSON output. Filtering + selection live in
/// [`filter_flashable_drives`].
// Four bool fields here mirror the Get-Disk JSON schema 1:1; an
// options-bitflags refactor would just obscure what field came from
// which PowerShell property. Narrow allow at the struct level.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PhysicalDisk {
    pub(crate) number: u32,
    pub(crate) friendly_name: String,
    pub(crate) size_bytes: u64,
    pub(crate) bus_type: BusType,
    pub(crate) is_boot: bool,
    pub(crate) is_system: bool,
    pub(crate) is_offline: bool,
    pub(crate) is_read_only: bool,
    /// Partition style string as `Get-Disk` reports it (`"MBR"`, `"GPT"`,
    /// `"RAW"`, or `"Unknown"`). Not used for filtering but surfaced
    /// to the operator so they can spot a disk that's already
    /// partitioned.
    pub(crate) partition_style: String,
}

impl PhysicalDisk {
    /// Human-readable size: `"1.8 GiB"`, `"64 MiB"`, etc. Matches the
    /// Linux `select_drive` UI rounding.
    pub(crate) fn human_size(&self) -> String {
        const KIB: u64 = 1024;
        const MIB: u64 = 1024 * KIB;
        const GIB: u64 = 1024 * MIB;
        const TIB: u64 = 1024 * GIB;
        let b = self.size_bytes;
        if b >= TIB {
            #[allow(clippy::cast_precision_loss)]
            let v = b as f64 / TIB as f64;
            format!("{v:.1} TiB")
        } else if b >= GIB {
            #[allow(clippy::cast_precision_loss)]
            let v = b as f64 / GIB as f64;
            format!("{v:.1} GiB")
        } else if b >= MIB {
            #[allow(clippy::cast_precision_loss)]
            let v = b as f64 / MIB as f64;
            format!("{v:.0} MiB")
        } else {
            format!("{b} B")
        }
    }
}

/// Get-Disk's `BusType` integer enum. Only the common USB/SATA/NVMe
/// values are called out; everything else maps to [`Other`].
///
/// [`Other`]: BusType::Other
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BusType {
    Usb,
    Sata,
    Nvme,
    /// Includes virtual-machine bus types (1/SCSI, 15/Virtual) which
    /// are valid targets in a VM scratch-disk scenario.
    Other(u16),
    Unknown,
}

impl BusType {
    fn from_raw(raw: Option<u16>) -> Self {
        match raw {
            Some(7) => Self::Usb,
            Some(11) => Self::Sata,
            Some(17) => Self::Nvme,
            Some(n) => Self::Other(n),
            None => Self::Unknown,
        }
    }
}

/// Serde mirror of the Get-Disk JSON per-entry shape. Kept private
/// so the public [`PhysicalDisk`] can evolve without breaking the
/// parser contract.
#[derive(Debug, Deserialize)]
struct RawDisk {
    #[serde(rename = "Number")]
    number: u32,
    #[serde(rename = "FriendlyName")]
    friendly_name: Option<String>,
    #[serde(rename = "Size")]
    size: u64,
    #[serde(rename = "BusType")]
    bus_type: Option<u16>,
    #[serde(rename = "IsBoot")]
    is_boot: Option<bool>,
    #[serde(rename = "IsSystem")]
    is_system: Option<bool>,
    #[serde(rename = "IsOffline")]
    is_offline: Option<bool>,
    #[serde(rename = "IsReadOnly")]
    is_read_only: Option<bool>,
    #[serde(rename = "PartitionStyle")]
    partition_style: Option<String>,
}

impl From<RawDisk> for PhysicalDisk {
    fn from(r: RawDisk) -> Self {
        Self {
            number: r.number,
            friendly_name: r.friendly_name.unwrap_or_else(|| "<unknown>".into()),
            size_bytes: r.size,
            bus_type: BusType::from_raw(r.bus_type),
            is_boot: r.is_boot.unwrap_or(false),
            is_system: r.is_system.unwrap_or(false),
            is_offline: r.is_offline.unwrap_or(false),
            is_read_only: r.is_read_only.unwrap_or(false),
            partition_style: r.partition_style.unwrap_or_else(|| "Unknown".into()),
        }
    }
}

/// Parse the JSON array emitted by [`GET_DISKS_PS_COMMAND`] into a
/// vec of [`PhysicalDisk`]. Tolerates leading/trailing whitespace
/// and `PowerShell`'s UTF-8-BOM prefix quirk (some PS hosts prefix a
/// BOM on JSON output even when `-Compress` is set).
pub(crate) fn parse_disks_json(raw: &str) -> Result<Vec<PhysicalDisk>, String> {
    let trimmed = raw.trim_start_matches('\u{FEFF}').trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let raws: Vec<RawDisk> = serde_json::from_str(trimmed)
        .map_err(|e| format!("parse Get-Disk JSON ({} bytes): {e}", trimmed.len()))?;
    Ok(raws.into_iter().map(PhysicalDisk::from).collect())
}

/// Filter the full disk list down to drives the operator can safely
/// flash. Refuses:
/// - Disk 0 (traditionally the OS boot drive on Windows).
/// - Any disk with `IsBoot = true` or `IsSystem = true` (defense in
///   depth — a reimaged host may have renumbered drives).
/// - Read-only drives.
/// - Drives smaller than 1 GiB (reasonable lower bound; aegis-boot
///   needs ~200 MiB for the ESP + variable for `AEGIS_ISOS`).
///
/// Order of output: by disk number ascending (stable, matches how
/// `Get-Disk` emits them).
pub(crate) fn filter_flashable_drives(disks: &[PhysicalDisk]) -> Vec<PhysicalDisk> {
    const MIN_SIZE_BYTES: u64 = 1024 * 1024 * 1024; // 1 GiB
    let mut out: Vec<PhysicalDisk> = disks
        .iter()
        .filter(|d| d.number != 0)
        .filter(|d| !d.is_boot)
        .filter(|d| !d.is_system)
        .filter(|d| !d.is_read_only)
        .filter(|d| d.size_bytes >= MIN_SIZE_BYTES)
        .cloned()
        .collect();
    out.sort_by_key(|d| d.number);
    out
}

/// Top-level PowerShell invocation — Windows-only. Runs
/// [`GET_DISKS_PS_COMMAND`], parses the JSON, filters via
/// [`filter_flashable_drives`].
#[cfg(target_os = "windows")]
pub(crate) fn enumerate_flashable_drives() -> Result<Vec<PhysicalDisk>, String> {
    use std::process::Command;
    let out = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command"])
        .arg(GET_DISKS_PS_COMMAND)
        .output()
        .map_err(|e| format!("spawn powershell: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "Get-Disk failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let all = parse_disks_json(&stdout)?;
    Ok(filter_flashable_drives(&all))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// Canned Get-Disk output matching what the Win11 VM returns
    /// (`Disk 0 QEMU HARDDISK 128 GiB GPT boot`, `Disk 1 QEMU
    /// HARDDISK 2 GiB RAW scratch`). Minus a few fields we don't
    /// need for the parser contract.
    fn canned_win11_vm_json() -> String {
        r#"[
          {"Number":0,"FriendlyName":"QEMU HARDDISK","Size":137438953472,
           "BusType":11,"IsBoot":true,"IsSystem":true,"IsOffline":false,
           "IsReadOnly":false,"PartitionStyle":"GPT"},
          {"Number":1,"FriendlyName":"QEMU HARDDISK","Size":2147483648,
           "BusType":11,"IsBoot":false,"IsSystem":false,"IsOffline":false,
           "IsReadOnly":false,"PartitionStyle":"RAW"}
        ]"#
        .to_string()
    }

    #[test]
    fn parse_disks_json_reads_vm_canned_output() {
        let disks = parse_disks_json(&canned_win11_vm_json()).unwrap();
        assert_eq!(disks.len(), 2);
        assert_eq!(disks[0].number, 0);
        assert!(disks[0].is_boot);
        assert_eq!(disks[1].number, 1);
        assert_eq!(disks[1].size_bytes, 2_147_483_648);
        assert_eq!(disks[1].bus_type, BusType::Sata);
    }

    #[test]
    fn parse_handles_utf8_bom_prefix() {
        // Some PowerShell hosts emit a BOM even with -Compress.
        let json = format!("\u{FEFF}{}", canned_win11_vm_json());
        let disks = parse_disks_json(&json).unwrap();
        assert_eq!(disks.len(), 2);
    }

    #[test]
    fn parse_empty_input_yields_empty_vec() {
        assert!(parse_disks_json("").unwrap().is_empty());
        assert!(parse_disks_json("   \n").unwrap().is_empty());
    }

    #[test]
    fn parse_propagates_malformed_json_as_error() {
        let err = parse_disks_json("{not json}").unwrap_err();
        assert!(err.contains("parse Get-Disk JSON"));
    }

    #[test]
    fn parse_accepts_missing_optional_fields() {
        // Older Windows PowerShell on WinServer 2016 doesn't always
        // emit IsOffline/IsReadOnly. Parser should treat missing as
        // false/defaults rather than refusing the whole payload.
        let json = r#"[{"Number":2,"FriendlyName":"X","Size":1000}]"#;
        let disks = parse_disks_json(json).unwrap();
        assert_eq!(disks.len(), 1);
        assert!(!disks[0].is_boot);
        assert!(!disks[0].is_offline);
        assert_eq!(disks[0].bus_type, BusType::Unknown);
        assert_eq!(disks[0].partition_style, "Unknown");
    }

    #[test]
    fn filter_refuses_disk_zero_even_when_not_boot_flagged() {
        // Defensive: even if IsBoot=false on disk 0 (unlikely but the
        // #447 partition gate already does this as belt+suspenders),
        // filter refuses it.
        let disks = vec![PhysicalDisk {
            number: 0,
            friendly_name: "Mystery Drive".into(),
            size_bytes: 4 * 1024 * 1024 * 1024,
            bus_type: BusType::Usb,
            is_boot: false,
            is_system: false,
            is_offline: false,
            is_read_only: false,
            partition_style: "GPT".into(),
        }];
        assert!(filter_flashable_drives(&disks).is_empty());
    }

    #[test]
    fn filter_refuses_boot_drives() {
        let disks = parse_disks_json(&canned_win11_vm_json()).unwrap();
        let flashable = filter_flashable_drives(&disks);
        // Disk 0 filtered on both number + IsBoot; Disk 1 survives.
        assert_eq!(flashable.len(), 1);
        assert_eq!(flashable[0].number, 1);
    }

    #[test]
    fn filter_refuses_system_drives() {
        let disks = vec![PhysicalDisk {
            number: 3,
            friendly_name: "reimaged".into(),
            size_bytes: 4 * 1024 * 1024 * 1024,
            bus_type: BusType::Nvme,
            is_boot: false,
            is_system: true,
            is_offline: false,
            is_read_only: false,
            partition_style: "GPT".into(),
        }];
        assert!(filter_flashable_drives(&disks).is_empty());
    }

    #[test]
    fn filter_refuses_read_only() {
        let disks = vec![PhysicalDisk {
            number: 2,
            friendly_name: "write-protected stick".into(),
            size_bytes: 8 * 1024 * 1024 * 1024,
            bus_type: BusType::Usb,
            is_boot: false,
            is_system: false,
            is_offline: false,
            is_read_only: true,
            partition_style: "MBR".into(),
        }];
        assert!(filter_flashable_drives(&disks).is_empty());
    }

    #[test]
    fn filter_refuses_too_small_drives() {
        // 500 MiB "drive" — probably a partition surfacing as a disk,
        // or a LUN we can't sensibly flash.
        let disks = vec![PhysicalDisk {
            number: 4,
            friendly_name: "tiny".into(),
            size_bytes: 500 * 1024 * 1024,
            bus_type: BusType::Usb,
            is_boot: false,
            is_system: false,
            is_offline: false,
            is_read_only: false,
            partition_style: "RAW".into(),
        }];
        assert!(filter_flashable_drives(&disks).is_empty());
    }

    #[test]
    fn filter_returns_stable_order_by_number() {
        let mk = |n: u32| PhysicalDisk {
            number: n,
            friendly_name: format!("disk-{n}"),
            size_bytes: 4 * 1024 * 1024 * 1024,
            bus_type: BusType::Usb,
            is_boot: false,
            is_system: false,
            is_offline: false,
            is_read_only: false,
            partition_style: "RAW".into(),
        };
        let disks = vec![mk(5), mk(2), mk(3), mk(1)];
        let flashable = filter_flashable_drives(&disks);
        let numbers: Vec<u32> = flashable.iter().map(|d| d.number).collect();
        assert_eq!(numbers, vec![1, 2, 3, 5]);
    }

    #[test]
    fn human_size_rounds_to_reasonable_units() {
        let mk = |bytes: u64| PhysicalDisk {
            number: 1,
            friendly_name: "x".into(),
            size_bytes: bytes,
            bus_type: BusType::Usb,
            is_boot: false,
            is_system: false,
            is_offline: false,
            is_read_only: false,
            partition_style: "RAW".into(),
        };
        assert_eq!(mk(512).human_size(), "512 B");
        assert_eq!(mk(16 * 1024 * 1024).human_size(), "16 MiB");
        assert_eq!(mk(2_147_483_648).human_size(), "2.0 GiB");
        assert_eq!(mk(2 * 1024u64.pow(4)).human_size(), "2.0 TiB");
    }

    #[test]
    fn bus_type_from_raw_maps_common_values() {
        assert_eq!(BusType::from_raw(Some(7)), BusType::Usb);
        assert_eq!(BusType::from_raw(Some(11)), BusType::Sata);
        assert_eq!(BusType::from_raw(Some(17)), BusType::Nvme);
        assert_eq!(BusType::from_raw(Some(1)), BusType::Other(1)); // SCSI
        assert_eq!(BusType::from_raw(Some(15)), BusType::Other(15)); // Virtual
        assert_eq!(BusType::from_raw(None), BusType::Unknown);
    }

    #[test]
    fn get_disks_ps_command_references_expected_fields() {
        // Cheap contract guard: if someone refactors the PS command
        // and forgets to select the fields the parser expects,
        // serde will fail at runtime. This test at least catches
        // obvious drift at edit time.
        for field in [
            "Number",
            "FriendlyName",
            "Size",
            "BusType",
            "IsBoot",
            "IsSystem",
        ] {
            assert!(
                GET_DISKS_PS_COMMAND.contains(field),
                "PS command should select field {field}"
            );
        }
        assert!(GET_DISKS_PS_COMMAND.contains("ConvertTo-Json"));
    }
}
