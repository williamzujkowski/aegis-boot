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
//!
//! # Two wire formats under one crate
//!
//! This crate hosts two logically distinct JSON wire formats that
//! both carry aegis-boot provenance:
//!
//! * **On-ESP manifest** ([`Manifest`], [`SCHEMA_VERSION`]) — signed
//!   record written into `::/aegis-boot-manifest.json` at flash time,
//!   read by runtime verifiers. Phase 4a of [#286].
//! * **Host attestation receipt** ([`Attestation`],
//!   [`ATTESTATION_SCHEMA_VERSION`]) — per-flash audit record
//!   written to `$XDG_DATA_HOME/aegis-boot/attestations/` for
//!   chain-of-custody + fleet inventory. Phase 4c-1 of [#286].
//! * **CLI envelopes** ([`Version`], [`ListReport`], with
//!   [`VERSION_SCHEMA_VERSION`] / [`LIST_SCHEMA_VERSION`]) — the
//!   `--json` envelopes emitted by `aegis-boot --version --json`,
//!   `aegis-boot list --json`, and siblings. Phase 4b of [#286].
//!
//! Each contract is independently versioned — a change to one
//! schema does not require bumping the others. They are co-located
//! in the same crate because they are all "aegis-boot wire-format
//! structs for third-party consumers" and sharing the optional
//! `schema` feature + docgen infrastructure is cheaper than forking
//! it across N crates. A future crate rename (`aegis-wire-formats`
//! or similar) may follow once the full CLI envelope set lands.
//!
//! [#286]: https://github.com/williamzujkowski/aegis-boot/issues/286

use serde::{Deserialize, Serialize};

/// Locked schema version for the on-ESP signed [`Manifest`]. Bump
/// alongside a breaking shape change (removing a field, changing a
/// field's type). Adding a new optional field is backwards-compatible
/// and does not require a version bump — the verifier ignores fields
/// it doesn't know about.
pub const SCHEMA_VERSION: u32 = 1;

/// Locked schema version for the host-side [`Attestation`] record.
/// Independent of [`SCHEMA_VERSION`] — either contract can advance
/// without the other.
pub const ATTESTATION_SCHEMA_VERSION: u32 = 1;

/// Locked schema version for the [`Version`] envelope emitted by
/// `aegis-boot --version --json`. Independent of the manifest and
/// attestation contract versions.
pub const VERSION_SCHEMA_VERSION: u32 = 1;

/// Locked schema version for the [`ListReport`] envelope emitted
/// by `aegis-boot list --json`. Independent of the other envelope
/// contracts.
pub const LIST_SCHEMA_VERSION: u32 = 1;

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

// -----------------------------------------------------------------
// Host-side attestation record (Phase 4c-1 of #286).
//
// The [`Attestation`] document is written to
// `$XDG_DATA_HOME/aegis-boot/attestations/<guid>-<ts>.json` at flash
// time, and amended with [`IsoRecord`] entries each time
// `aegis-boot add` lands an ISO on the stick. Independent schema
// from the on-ESP [`Manifest`] above — the attestation is audit
// trail + fleet-inventory data, not a boot contract.
// -----------------------------------------------------------------

/// One flash + zero-or-more ISO additions, captured as a single
/// JSON document on the host. Stored under
/// `$XDG_DATA_HOME/aegis-boot/attestations/` (or
/// `~/.local/share/aegis-boot/attestations/` if `XDG_DATA_HOME` is
/// unset). The `aegis-boot attest list` / `attest show` commands
/// read these back for chain-of-custody queries.
///
/// v0 ships unsigned — the trust anchor is "the operator ran this
/// command on this host, the timestamps + hashes are the evidence."
/// TPM PCR attestation + signing lands under epic
/// [#139](https://github.com/williamzujkowski/aegis-boot/issues/139)
/// as additive fields; the current schema is forward-compatible
/// (consumers ignore unknown fields).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Attestation {
    /// Wire-format version. See [`ATTESTATION_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// `aegis-boot` binary version that produced this record
    /// (e.g. `"aegis-boot 0.14.1"`).
    pub tool_version: String,
    /// RFC 3339 / ISO 8601 timestamp of the flash operation.
    /// Generated via the host's `date -u +%FT%TZ` so the crate
    /// does not pull a chrono dep.
    pub flashed_at: String,
    /// The user that ran `aegis-boot flash` — captured from
    /// `$SUDO_USER` if set, else `$USER`.
    pub operator: String,
    /// Host environment captured at flash time.
    pub host: HostInfo,
    /// Target stick captured at flash time.
    pub target: TargetInfo,
    /// ISO records appended on each successful `aegis-boot add`.
    /// Empty immediately after flash; grows over the stick's
    /// lifetime.
    pub isos: Vec<IsoRecord>,
}

/// Host environment fingerprint at flash time.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct HostInfo {
    /// `uname -r` output.
    pub kernel: String,
    /// Secure Boot state: one of `"enforcing"`, `"disabled"`, or
    /// `"unknown"`.
    pub secure_boot: String,
}

/// Target stick fingerprint at flash time.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TargetInfo {
    /// Device node path (e.g. `/dev/sda`).
    pub device: String,
    /// Vendor + model string from `/sys/block/sdX/device/{vendor,model}`.
    pub model: String,
    /// Raw device size in bytes, rounded to the nearest 512B sector.
    pub size_bytes: u64,
    /// Lowercase hex sha256 of the dd'd image body.
    pub image_sha256: String,
    /// Size in bytes of the image body (for sanity-checking the
    /// sha256 over the correct length).
    pub image_size_bytes: u64,
    /// GPT disk GUID, captured from `sgdisk -p` after `partprobe`.
    /// May be empty if sgdisk fails or the drive isn't partitioned.
    pub disk_guid: String,
}

/// One `aegis-boot add` operation appended to the stick's
/// attestation record.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct IsoRecord {
    /// ISO filename as it lives on the AEGIS_ISOS data partition.
    pub filename: String,
    /// Lowercase hex sha256 of the ISO body.
    pub sha256: String,
    /// ISO size in bytes.
    pub size_bytes: u64,
    /// Sidecar filenames recorded alongside the ISO (e.g. a
    /// `.aegis.toml` operator-label file).
    pub sidecars: Vec<String>,
    /// RFC 3339 / ISO 8601 timestamp of when the ISO was added.
    pub added_at: String,
}

// -----------------------------------------------------------------
// CLI envelopes (Phase 4b-1 onward of #286).
//
// These are the `--json` output shapes emitted by `aegis-boot`
// subcommands. The envelopes are tiny, stable, and
// independently-versioned wire contracts scripted consumers
// (monitoring, install-one-liner assertions, Homebrew formula
// tests) depend on.
// -----------------------------------------------------------------

/// Envelope emitted by `aegis-boot --version --json`. Lets scripted
/// consumers (install-one-liner assertions, Homebrew formula tests,
/// ansible-verified installs) parse the version without regex on
/// the human-readable string.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Version {
    /// Wire-format version. See [`VERSION_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Always `"aegis-boot"` for this CLI — field exists to make a
    /// future migration to a multi-tool repo (where the same
    /// envelope shape might be emitted by a sibling binary) a
    /// zero-schema-bump operation.
    pub tool: String,
    /// Semver string matching the workspace version (`Cargo.toml`
    /// `[workspace.package].version`). Does NOT include a `v`
    /// prefix — `"0.14.1"`, not `"v0.14.1"`.
    pub version: String,
}

/// Envelope emitted by `aegis-boot list --json`. Reports the ISOs
/// discovered on a stick's AEGIS_ISOS data partition, plus the
/// attestation summary if one was recorded at flash time.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ListReport {
    /// Wire-format version. See [`LIST_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// `aegis-boot` binary version that produced this envelope.
    pub tool_version: String,
    /// Filesystem mount path the stick was resolved to (e.g.
    /// `/run/media/alice/AEGIS_ISOS`).
    pub mount_path: String,
    /// Chain-of-custody summary from the matching attestation
    /// record, or null if no attestation was found (operator
    /// flashed on a different host, or pre-v0.13.0 stick).
    pub attestation: Option<ListAttestationSummary>,
    /// Number of ISOs observed. Redundant with `isos.len()` but
    /// useful for consumers that only read the header.
    pub count: u32,
    /// Per-ISO details. See [`ListIsoSummary`].
    pub isos: Vec<ListIsoSummary>,
}

/// Compact attestation summary embedded in [`ListReport`]. Derived
/// from the stored [`Attestation`] record; smaller than the full
/// attestation (no device fingerprint, no host kernel string) so
/// the list envelope stays lightweight.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ListAttestationSummary {
    /// RFC 3339 timestamp of the flash operation.
    pub flashed_at: String,
    /// Operator that ran `aegis-boot flash`.
    pub operator: String,
    /// Count of ISOs recorded in the attestation. Note this can
    /// differ from [`ListReport::count`] — the attestation tracks
    /// ISOs that were added via `aegis-boot add`, while the list
    /// count scans the actual partition. A mismatch is a signal
    /// that someone hand-copied an ISO rather than using the CLI.
    pub isos_recorded: u32,
    /// Filesystem path of the host-side attestation manifest.
    pub manifest_path: String,
}

/// Per-ISO detail in [`ListReport`]. The `display_name` +
/// `description` fields come from an optional `<iso>.aegis.toml`
/// operator-label sidecar (#246); they are always present in the
/// wire format (as `null` when absent) so consumers see a stable
/// shape.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ListIsoSummary {
    /// ISO filename on the data partition.
    pub name: String,
    /// ISO size in bytes.
    pub size_bytes: u64,
    /// Whether a matching `<iso>.sha256` sidecar file exists.
    pub has_sha256: bool,
    /// Whether a matching `<iso>.minisig` sidecar file exists.
    pub has_minisig: bool,
    /// Operator-curated display name from `<iso>.aegis.toml`, or
    /// null when the sidecar is absent. Intentionally NOT omitted
    /// when null — consumers depend on a stable field set.
    pub display_name: Option<String>,
    /// Operator-curated description from `<iso>.aegis.toml`, or
    /// null when the sidecar is absent.
    pub description: Option<String>,
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

    fn sample_attestation() -> Attestation {
        Attestation {
            schema_version: ATTESTATION_SCHEMA_VERSION,
            tool_version: "aegis-boot 0.14.1".to_string(),
            flashed_at: "2026-04-19T14:30:00Z".to_string(),
            operator: "alice".to_string(),
            host: HostInfo {
                kernel: "6.17.0-20-generic".to_string(),
                secure_boot: "enforcing".to_string(),
            },
            target: TargetInfo {
                device: "/dev/sda".to_string(),
                model: "SanDisk Cruzer 32 GB".to_string(),
                size_bytes: 32_000_000_000,
                image_sha256: "f".repeat(64),
                image_size_bytes: 536_870_912,
                disk_guid: "00000000-0000-0000-0000-000000000001".to_string(),
            },
            isos: vec![IsoRecord {
                filename: "ubuntu-24.04.iso".to_string(),
                sha256: "a".repeat(64),
                size_bytes: 5_368_709_120,
                sidecars: vec!["ubuntu-24.04.iso.aegis.toml".to_string()],
                added_at: "2026-04-19T14:35:00Z".to_string(),
            }],
        }
    }

    #[test]
    fn attestation_schema_version_is_one() {
        // Independent contract from the on-ESP manifest; bumping is
        // intentional and consumer-visible.
        assert_eq!(ATTESTATION_SCHEMA_VERSION, 1);
    }

    #[test]
    fn attestation_round_trip_preserves_all_fields() {
        let a = sample_attestation();
        let body = serde_json::to_string(&a).expect("serialize");
        let parsed: Attestation = serde_json::from_str(&body).expect("parse");
        assert_eq!(a, parsed);
    }

    #[test]
    fn empty_isos_list_serializes_as_empty_array() {
        // A freshly-flashed stick has `isos: []` — the consumer
        // contract is that the array field is always present,
        // never omitted. Guards against accidentally adding
        // `#[serde(skip_serializing_if = "Vec::is_empty")]`.
        let mut a = sample_attestation();
        a.isos.clear();
        let body = serde_json::to_string(&a).expect("serialize");
        assert!(body.contains("\"isos\":[]"), "isos must be present: {body}");
    }

    fn sample_version() -> Version {
        Version {
            schema_version: VERSION_SCHEMA_VERSION,
            tool: "aegis-boot".to_string(),
            version: "0.14.1".to_string(),
        }
    }

    #[test]
    fn version_schema_version_is_one() {
        assert_eq!(VERSION_SCHEMA_VERSION, 1);
    }

    #[test]
    fn version_round_trip_preserves_all_fields() {
        let v = sample_version();
        let body = serde_json::to_string(&v).expect("serialize");
        let parsed: Version = serde_json::from_str(&body).expect("parse");
        assert_eq!(v, parsed);
    }

    #[test]
    fn version_wire_field_order_matches_documented_shape() {
        // docs/CLI.md pins the shape as `{ schema_version, tool,
        // version }` — serde's default field order is declaration
        // order; this test is the guard against accidental reorder.
        let v = sample_version();
        let body = serde_json::to_string(&v).expect("serialize");
        let sv_pos = body.find("\"schema_version\"").expect("sv");
        let tool_pos = body.find("\"tool\"").expect("tool");
        let ver_pos = body.find("\"version\"").expect("version");
        assert!(sv_pos < tool_pos, "schema_version before tool: {body}");
        assert!(tool_pos < ver_pos, "tool before version: {body}");
    }

    fn sample_list_report() -> ListReport {
        ListReport {
            schema_version: LIST_SCHEMA_VERSION,
            tool_version: "0.14.1".to_string(),
            mount_path: "/run/media/alice/AEGIS_ISOS".to_string(),
            attestation: Some(ListAttestationSummary {
                flashed_at: "2026-04-19T14:30:00Z".to_string(),
                operator: "alice".to_string(),
                isos_recorded: 3,
                manifest_path: "/home/alice/.local/share/aegis-boot/attestations/abc.json"
                    .to_string(),
            }),
            count: 2,
            isos: vec![
                ListIsoSummary {
                    name: "ubuntu-24.04.iso".to_string(),
                    size_bytes: 5_368_709_120,
                    has_sha256: true,
                    has_minisig: false,
                    display_name: Some("Ubuntu 24.04 Desktop".to_string()),
                    description: None,
                },
                ListIsoSummary {
                    name: "debian-12.iso".to_string(),
                    size_bytes: 3_221_225_472,
                    has_sha256: false,
                    has_minisig: false,
                    display_name: None,
                    description: None,
                },
            ],
        }
    }

    #[test]
    fn list_schema_version_is_one() {
        assert_eq!(LIST_SCHEMA_VERSION, 1);
    }

    #[test]
    fn list_round_trip_preserves_all_fields() {
        let r = sample_list_report();
        let body = serde_json::to_string(&r).expect("serialize");
        let parsed: ListReport = serde_json::from_str(&body).expect("parse");
        assert_eq!(r, parsed);
    }

    #[test]
    fn list_attestation_serializes_as_null_when_absent() {
        // A stick flashed on a different host has no matching
        // attestation record — the field is `null`, NOT omitted.
        // Scripted consumers depend on a stable field set.
        let mut r = sample_list_report();
        r.attestation = None;
        let body = serde_json::to_string(&r).expect("serialize");
        assert!(
            body.contains("\"attestation\":null"),
            "attestation must be explicit null: {body}"
        );
    }

    #[test]
    fn list_iso_summary_preserves_null_sidecar_fields() {
        // display_name + description are `null` when the
        // `.aegis.toml` sidecar is absent. This stable-shape
        // property was called out explicitly in inventory.rs's
        // original emitter (see the comment around #246). Guards
        // against an accidental `skip_serializing_if`.
        let mut r = sample_list_report();
        r.isos[1].display_name = None;
        r.isos[1].description = None;
        let body = serde_json::to_string(&r).expect("serialize");
        // The second ISO entry should contain both fields as null.
        assert!(
            body.contains("\"display_name\":null"),
            "display_name missing or omitted: {body}"
        );
        assert!(
            body.contains("\"description\":null"),
            "description missing or omitted: {body}"
        );
    }
}
