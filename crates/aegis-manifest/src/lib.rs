// Phase 4a of #286. README becomes the rustdoc landing page
// (same pattern as the library trio).
#![allow(clippy::doc_markdown)]
#![doc = include_str!("../README.md")]
//!
//! ---
//!
//! # Rust API
//!
//! The core type is [`Manifest`] — the on-disk JSON body that gets
//! written to `::/aegis-boot-manifest.json` + signed into
//! `::/aegis-boot-manifest.json.minisig`. Every field is part of a
//! pinned wire contract; see the comments on each struct for the
//! invariants a verifier must enforce.
//!
//! # Schema versioning
//!
//! [`SCHEMA_VERSION`] is the canonical value for `schema_version`.
//! Bump it only alongside a shape-breaking change (removing a field,
//! changing a field's type). Adding a new optional field is
//! backwards-compatible and does not require a version bump — the
//! verifier ignores fields it doesn't know about.
//!
//! # JSON Schema
//!
//! With the `schema` feature enabled, every public type derives
//! [`schemars::JsonSchema`]. The `aegis-manifest-schema-docgen`
//! binary emits a validated JSON Schema document to
//! `docs/reference/schemas/aegis-boot-manifest.schema.json` in the
//! parent workspace; third-party verifiers can pin to this schema.

use serde::{Deserialize, Serialize};

/// Locked schema version. Bump alongside a breaking shape change
/// (removing a field, changing a field's type). Adding a new
/// optional field is backwards-compatible and does not require a
/// version bump — the verifier ignores fields it doesn't know about.
pub const SCHEMA_VERSION: u32 = 1;

/// Top-level manifest body. Serialized field order matches the
/// declaration order below — relied on for canonical JSON stability
/// (the signature is computed over `serde_json::to_vec(&Manifest)`).
///
/// The Rust field is `sequence` (clippy prefers not to prefix the
/// struct name); the JSON wire field stays `manifest_sequence` per
/// the [#277] schema lock via `#[serde(rename)]`.
///
/// [#277]: https://github.com/williamzujkowski/aegis-boot/issues/277
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Manifest {
    /// Wire-format version. See [`SCHEMA_VERSION`].
    pub schema_version: u32,
    /// `aegis-boot` binary version that produced the manifest
    /// (e.g. `"aegis-boot 0.14.1"`). Informational — not a trust
    /// anchor.
    pub tool_version: String,
    /// Monotonic-per-flash sequence number. Defends against
    /// rollback to an older validly-signed manifest without
    /// relying on a secure RTC. Verifiers track the highest
    /// sequence they've ever seen for a given device fingerprint
    /// and reject manifests whose sequence is strictly less.
    #[serde(rename = "manifest_sequence")]
    pub sequence: u64,
    /// Device identity captured at flash time.
    pub device: Device,
    /// Closed set of files on the ESP. Verifier rejects the stick
    /// if any ESP file is not listed here or is missing / has a
    /// different sha256 than recorded. Six entries today, one per
    /// line in the signed-chain layout established by Phase 2b of
    /// [#274](https://github.com/williamzujkowski/aegis-boot/issues/274).
    pub esp_files: Vec<EspFileEntry>,
    /// When `true`, the verifier treats [`Self::esp_files`] as
    /// exhaustive — the presence of any additional file on the ESP
    /// is itself a violation. Always `true` in PR3; left as a
    /// field so future phases can ship an evolutionary "extended"
    /// manifest without breaking consumers.
    pub allowed_files_closed_set: bool,
    /// Reserved for E6 / Phase 3 TPM attestation. Left empty by
    /// PR3; once E6 locks the PCR selection this vector grows
    /// populated rows.
    pub expected_pcrs: Vec<PcrEntry>,
}

/// Device identity captured at flash time. All values come from the
/// freshly-written GPT (`blkid` + `sgdisk -p`); the verifier
/// re-reads them at runtime and asserts equality.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Device {
    /// GPT disk GUID.
    pub disk_guid: String,
    /// Total partition count observed at flash time. aegis-boot
    /// lays down exactly two partitions (ESP + AEGIS_ISOS); a
    /// verifier reading three or more partitions on the same disk
    /// rejects the stick.
    pub partition_count: u32,
    /// ESP partition identity — first partition by design.
    pub esp: EspPartition,
    /// Data partition identity — `AEGIS_ISOS` label, exfat by
    /// default, fat32 or ext4 opt-in.
    pub data: DataPartition,
}

/// Identity of the ESP partition (partition 1).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct EspPartition {
    /// GPT PARTUUID (per-partition unique identifier).
    pub partuuid: String,
    /// GPT partition-type GUID. For the ESP this is
    /// `C12A7328-F81F-11D2-BA4B-00A0C93EC93B`.
    pub type_guid: String,
    /// FAT32 volume serial number (`blkid -o value -s UUID`).
    pub fs_uuid: String,
    /// First absolute LBA of the partition's extent on disk.
    pub first_lba: u64,
    /// Last absolute LBA of the partition's extent on disk.
    pub last_lba: u64,
}

/// Identity of the AEGIS_ISOS data partition (partition 2).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DataPartition {
    /// GPT PARTUUID.
    pub partuuid: String,
    /// GPT partition-type GUID — Microsoft Basic Data for
    /// exfat/fat32, Linux Filesystem for ext4.
    pub type_guid: String,
    /// Filesystem UUID.
    pub fs_uuid: String,
    /// Filesystem label — always `AEGIS_ISOS` for aegis-boot
    /// sticks.
    pub label: String,
}

/// A single file on the ESP with its content hash and size.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct EspFileEntry {
    /// Mtools-style `::/`-rooted path on the ESP (e.g.
    /// `::/EFI/BOOT/BOOTX64.EFI`). The verifier lowercases both
    /// sides before comparison because FAT32 is case-insensitive.
    pub path: String,
    /// Lowercase hex sha256 of the file body.
    pub sha256: String,
    /// File size in bytes. Redundant with sha256 but cheap to
    /// check first — a size mismatch lets the verifier reject
    /// without reading the full body.
    pub size_bytes: u64,
}

/// Expected TPM PCR value at an aegis-boot stick's first post-boot
/// measurement. Empty in PR3; populated once E6 locks the PCR
/// selection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct PcrEntry {
    /// PCR index (0..23 for most banks).
    pub pcr_index: u32,
    /// Hash bank — `sha256`, `sha384`, etc.
    pub bank: String,
    /// Lowercase hex expected digest.
    pub digest_hex: String,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn sample_manifest() -> Manifest {
        Manifest {
            schema_version: SCHEMA_VERSION,
            tool_version: "aegis-boot 0.14.1".to_string(),
            sequence: 42,
            device: Device {
                disk_guid: "00000000-0000-0000-0000-000000000001".to_string(),
                partition_count: 2,
                esp: EspPartition {
                    partuuid: "aaa".to_string(),
                    type_guid: "C12A7328-F81F-11D2-BA4B-00A0C93EC93B".to_string(),
                    fs_uuid: "1234-ABCD".to_string(),
                    first_lba: 2048,
                    last_lba: 821_247,
                },
                data: DataPartition {
                    partuuid: "bbb".to_string(),
                    type_guid: "EBD0A0A2-B9E5-4433-87C0-68B6B72699C7".to_string(),
                    fs_uuid: "ABCD-1234".to_string(),
                    label: "AEGIS_ISOS".to_string(),
                },
            },
            esp_files: vec![EspFileEntry {
                path: "::/EFI/BOOT/BOOTX64.EFI".to_string(),
                sha256: "0".repeat(64),
                size_bytes: 947_200,
            }],
            allowed_files_closed_set: true,
            expected_pcrs: vec![],
        }
    }

    #[test]
    fn schema_version_is_one() {
        // Bumping this is intentional and downstream-visible.
        assert_eq!(SCHEMA_VERSION, 1);
    }

    #[test]
    fn round_trip_preserves_all_fields() {
        let m = sample_manifest();
        let body = serde_json::to_string(&m).expect("serialize");
        let parsed: Manifest = serde_json::from_str(&body).expect("parse");
        assert_eq!(m, parsed);
    }

    #[test]
    fn serialized_uses_manifest_sequence_wire_name() {
        let m = sample_manifest();
        let body = serde_json::to_string(&m).expect("serialize");
        assert!(
            body.contains("\"manifest_sequence\":42"),
            "wire field should be manifest_sequence, got: {body}"
        );
        assert!(
            !body.contains("\"sequence\":"),
            "bare `sequence` leak would break verifiers: {body}"
        );
    }
}
