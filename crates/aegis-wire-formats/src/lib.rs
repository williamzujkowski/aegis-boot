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
//! [`schemars::JsonSchema`]. The `aegis-wire-formats-schema-docgen`
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
//! * **CLI envelopes** ([`Version`], [`ListReport`],
//!   [`AttestListReport`], [`VerifyReport`], [`UpdateReport`],
//!   [`RecommendReport`], [`CompatReport`], [`CompatSubmitReport`],
//!   [`DoctorReport`], with their `*_SCHEMA_VERSION` constants) —
//!   the `--json` envelopes emitted by `aegis-boot --version --json`,
//!   `... list --json`, `... attest list --json`, `... verify --json`,
//!   `... update --json`, `... recommend --json`, `... compat --json`,
//!   `... compat --submit --json`, and siblings. Phase 4b of [#286].
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

/// Locked schema version for the [`AttestListReport`] envelope
/// emitted by `aegis-boot attest list --json`. Independent of the
/// other envelope contracts.
pub const ATTEST_LIST_SCHEMA_VERSION: u32 = 1;

/// Locked schema version for the [`VerifyReport`] envelope emitted
/// by `aegis-boot verify --json`. Independent of the other envelope
/// contracts.
pub const VERIFY_SCHEMA_VERSION: u32 = 1;

/// Locked schema version for the [`UpdateReport`] envelope emitted
/// by `aegis-boot update --json`. Independent of the other envelope
/// contracts.
pub const UPDATE_SCHEMA_VERSION: u32 = 1;

/// Locked schema version for the [`RecommendReport`] envelope
/// emitted by `aegis-boot recommend --json`. Independent of the
/// other envelope contracts.
pub const RECOMMEND_SCHEMA_VERSION: u32 = 1;

/// Locked schema version for the [`CompatReport`] envelope emitted
/// by `aegis-boot compat --json`. Shared by the 4 mutually-exclusive
/// shapes (catalog / single / miss / my-machine-miss). Independent
/// of the other envelope contracts.
pub const COMPAT_SCHEMA_VERSION: u32 = 1;

/// Locked schema version for the [`CompatSubmitReport`] envelope
/// emitted by `aegis-boot compat --submit --json` — the
/// draft-a-hardware-report flow. Deliberately a separate schema
/// from [`CompatReport`] because the two surfaces have different
/// consumer contracts (operators draft-submit vs. scripted
/// lookup).
pub const COMPAT_SUBMIT_SCHEMA_VERSION: u32 = 1;

/// Locked schema version for the [`DoctorReport`] envelope emitted
/// by `aegis-boot doctor --json`. Independent of the other envelope
/// contracts.
pub const DOCTOR_SCHEMA_VERSION: u32 = 1;

/// Locked schema version for the [`CliError`] envelope — the
/// generic `{schema_version, error}` shape emitted by any
/// subcommand that fails *before* it can produce its
/// subcommand-specific `--json` envelope. Shared by multiple
/// subcommands because the pre-dispatch error path is identical
/// across them.
pub const CLI_ERROR_SCHEMA_VERSION: u32 = 1;

/// Locked schema version for the [`FailureMicroreport`] envelope —
/// the Tier-A anonymous on-stick failure log written by `rescue-tui`
/// / initramfs when a classifiable boot failure occurs. Per #342
/// Phase 2. Anonymous-by-construction: no hostname, no DMI serial,
/// no free-form error text; just vendor_family + bios_year +
/// classified error code + an opaque hash of the full error text.
pub const FAILURE_MICROREPORT_SCHEMA_VERSION: u32 = 1;

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
    /// ISO basename (no directory component). Consumers that join
    /// `mount_path + name` will get the root-of-stick path, which is
    /// only valid when `folder` is null. For sticks with subfolder
    /// layouts (#274 Phase 6), join `mount_path + folder + name`.
    /// Kept as basename — separate from `folder` — so downstream
    /// automation that shelled out to `basename(1)` on the old flat
    /// layout keeps working.
    pub name: String,
    /// Subfolder path relative to the data-partition mount, or `null`
    /// when the ISO sits at the root. `"ubuntu-24.04"` for a single
    /// level, `"ubuntu/24.04"` for nested (forward-slash separator
    /// regardless of host OS — the stick filesystem is exFAT, which
    /// normalizes to `/`). Added in v0.16.0 (#274 Phase 6a) as an
    /// additive optional field; v0.15.x consumers that ignore
    /// unknown keys see no behavior change, and the `name` field
    /// remains a basename for scripts that joined it directly.
    pub folder: Option<String>,
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

/// Envelope emitted by `aegis-boot attest list --json`. Scans the
/// host's attestation directory, attempts to parse each file, and
/// reports either a parsed summary or a parse-error placeholder per
/// entry. Enables monitoring / fleet tools to audit chain-of-custody
/// across all flashed sticks on a host.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AttestListReport {
    /// Wire-format version. See [`ATTEST_LIST_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// `aegis-boot` binary version that produced this envelope.
    pub tool_version: String,
    /// Host filesystem path of the attestations directory scanned
    /// (typically `$XDG_DATA_HOME/aegis-boot/attestations/`).
    pub attestations_dir: String,
    /// Number of files scanned (including parse-failure entries).
    pub count: u32,
    /// One entry per file found. See [`AttestListEntry`] for the
    /// success/error shape selection.
    pub attestations: Vec<AttestListEntry>,
}

/// One entry in an [`AttestListReport`]. Two mutually-exclusive
/// wire shapes via serde's `untagged` tagging:
///
/// * **Success** — a successful parse, reporting the manifest's
///   headline fields. The `error` field is absent.
/// * **Error** — the file existed but could not be parsed. Only
///   `manifest_path` + `error` are emitted; the summary fields
///   are absent.
///
/// Schemars emits this as a JSON Schema `oneOf` so consumers know
/// to branch on the presence of the `error` field.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum AttestListEntry {
    /// Parsed manifest summary.
    Success(AttestListSuccess),
    /// Placeholder for a file that existed but failed to parse.
    Error(AttestListError),
}

/// Successfully-parsed attestation summary inside an
/// [`AttestListReport`]. Deliberately a strict subset of the full
/// [`Attestation`] — enough to drive a dashboard without requiring
/// consumers to re-parse each file. Full detail is one
/// `aegis-boot attest show <path>` away.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AttestListSuccess {
    /// Host filesystem path of the attestation manifest file.
    pub manifest_path: String,
    /// Schema version declared inside the attestation manifest
    /// (see [`ATTESTATION_SCHEMA_VERSION`]).
    pub schema_version: u32,
    /// `tool_version` field from the attestation manifest.
    pub tool_version: String,
    /// `flashed_at` timestamp from the attestation manifest.
    pub flashed_at: String,
    /// Operator that ran the flash (from the attestation).
    pub operator: String,
    /// `target.device` from the attestation (e.g. `/dev/sda`).
    pub target_device: String,
    /// `target.model` from the attestation.
    pub target_model: String,
    /// GPT disk GUID from the attestation's target info.
    pub disk_guid: String,
    /// Number of [`IsoRecord`] entries inside the attestation.
    pub iso_count: u32,
}

/// Parse-failure entry inside an [`AttestListReport`]. Consumer
/// decision: show the operator which file failed + why, so they
/// can audit / repair / delete.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AttestListError {
    /// Host filesystem path of the unparseable file.
    pub manifest_path: String,
    /// Human-readable error message from the parser.
    pub error: String,
}

/// Envelope emitted by `aegis-boot verify --json`. Re-verifies
/// every ISO on a stick against its sidecar checksum and reports
/// a per-ISO verdict plus a summary tally. Used by CI / monitoring
/// to audit that a stick's ISOs haven't bit-rotted, been replaced,
/// or lost their sha256 sidecars.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct VerifyReport {
    /// Wire-format version. See [`VERIFY_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// `aegis-boot` binary version that produced this envelope.
    pub tool_version: String,
    /// Filesystem mount path of the AEGIS_ISOS partition scanned.
    pub mount_path: String,
    /// Aggregate tally + overall pass/fail.
    pub summary: VerifySummary,
    /// Per-ISO verdict. Always present (even as `[]` for an empty
    /// stick) so consumers see a stable field set.
    pub isos: Vec<VerifyEntry>,
}

/// Tally of per-ISO verdicts in a [`VerifyReport`]. `any_failure`
/// is the summary bit downstream tooling (CI, dashboards)
/// branches on.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct VerifySummary {
    /// Total ISOs scanned. Equals the length of [`VerifyReport::isos`].
    pub total: u32,
    /// Count with `verdict: "Verified"`.
    pub verified: u32,
    /// Count with `verdict: "Mismatch"` — sha256 differed from
    /// sidecar. A serious trust-chain signal; consumer should
    /// surface aggressively.
    pub mismatch: u32,
    /// Count with `verdict: "Unreadable"` — file exists but
    /// couldn't be opened / read (permission, bad media).
    pub unreadable: u32,
    /// Count with `verdict: "NotPresent"` — referenced in an
    /// attestation manifest but missing from the partition.
    pub not_present: u32,
    /// True iff at least one of `mismatch`, `unreadable`, or
    /// `not_present` is non-zero. The overall stick-health bit.
    pub any_failure: bool,
}

/// One ISO's verdict inside a [`VerifyReport`]. The `verdict`
/// field is the discriminator; variant-specific fields follow via
/// `#[serde(flatten)]`. Consumer contract: branch on `verdict`,
/// expect the fields documented for that variant.
///
/// Wire shape examples:
///
/// ```text
/// {"name": "ubuntu.iso", "verdict": "Verified", "digest": "…", "source": "sidecar"}
/// {"name": "debian.iso", "verdict": "Mismatch", "actual": "…", "expected": "…", "source": "sidecar"}
/// {"name": "alpine.iso", "verdict": "Unreadable", "source": "sidecar", "reason": "permission denied"}
/// {"name": "fedora.iso", "verdict": "NotPresent"}
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct VerifyEntry {
    /// ISO filename.
    pub name: String,
    /// Verdict tag + variant-specific fields.
    #[serde(flatten)]
    pub verdict: VerifyVerdict,
}

/// Per-ISO verdict variants. Internally-tagged under `verdict`; a
/// consumer that doesn't recognize a future variant can fall back
/// on the tag string.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "verdict")]
pub enum VerifyVerdict {
    /// sha256 of the ISO matches the sidecar's recorded digest.
    Verified {
        /// Computed sha256 (lowercase hex).
        digest: String,
        /// Where the expected digest came from (e.g. `"sidecar"`
        /// for the on-partition `.sha256` file).
        source: String,
    },
    /// sha256 of the ISO does NOT match the sidecar — either
    /// media corruption or a replaced/tampered file. Trust-chain
    /// breaking.
    Mismatch {
        /// Computed sha256 of the ISO on disk.
        actual: String,
        /// Digest the sidecar asserts.
        expected: String,
        /// Where the expected digest came from.
        source: String,
    },
    /// The ISO file exists but couldn't be opened / hashed.
    Unreadable {
        /// Where the expected digest came from (so the operator
        /// knows which sidecar to reconcile against after
        /// restoring access to the file).
        source: String,
        /// Human-readable explanation (permission, I/O error).
        reason: String,
    },
    /// An ISO referenced elsewhere (e.g. in the attestation
    /// manifest's `isos` list) is not on the partition.
    NotPresent,
}

/// Envelope emitted by `aegis-boot update --json`. Phase 1 of #181
/// is eligibility-check-only: answers "would a non-destructive
/// signed-chain rotation apply cleanly to this stick?" — the actual
/// rotation is Phase 2. The envelope's outer fields
/// (`schema_version`, `tool_version`, `device`) are common; the
/// [`eligibility`] flattened enum carries the variant-specific
/// body.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct UpdateReport {
    /// Wire-format version. See [`UPDATE_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// `aegis-boot` binary version that produced this envelope.
    pub tool_version: String,
    /// Device node path operated on (e.g. `/dev/sda`).
    pub device: String,
    /// Eligibility verdict + variant-specific fields.
    #[serde(flatten)]
    pub eligibility: UpdateEligibility,
}

/// Outcome of the eligibility check. Internally-tagged under
/// `eligibility` with the tag values `"ELIGIBLE"` and
/// `"INELIGIBLE"` (upper-case to match the existing wire format).
/// `flatten`-combined with [`UpdateReport`]'s outer fields so the
/// emitted JSON preserves the `schema_version, tool_version,
/// device, eligibility, …` ordering consumers parse against.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "eligibility")]
pub enum UpdateEligibility {
    /// The stick could accept an in-place signed-chain rotation.
    /// Reports the disk GUID (matches the ESP partition table's
    /// `.disk_guid`), the host-side attestation that would be
    /// updated, and the new signed chain the host would write.
    #[serde(rename = "ELIGIBLE")]
    Eligible {
        /// GPT disk GUID of the stick (matches the attestation
        /// manifest's `device.disk_guid`).
        disk_guid: String,
        /// Host filesystem path of the attestation manifest that
        /// the rotation will update.
        attestation_path: String,
        /// Ordered signed-chain files the host would install if
        /// the operator re-ran flash today (shim / grub /
        /// kernel / initrd). Each carries either `sha256` (success)
        /// or `error` (could not be hashed / located).
        host_chain: Vec<UpdateChainEntry>,
    },
    /// The stick cannot accept a rotation. Carries the operator-
    /// readable reason (device not removable, no attestation on
    /// this host, signed-chain source missing, …).
    #[serde(rename = "INELIGIBLE")]
    Ineligible {
        /// Explanation of why the rotation was refused.
        reason: String,
    },
}

/// One signed-chain entry in an [`UpdateEligibility::Eligible`]
/// response. Two mutually-exclusive wire shapes via untagged-enum
/// dispatch on the presence of `sha256` vs `error`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct UpdateChainEntry {
    /// Role in the signed chain: `shim`, `grub`, `kernel`, or
    /// `initrd`.
    pub role: String,
    /// Host filesystem path of the source file.
    pub path: String,
    /// Success (sha256) or failure (error) — flattened so consumers
    /// see `{role, path, sha256}` or `{role, path, error}`.
    #[serde(flatten)]
    pub result: UpdateChainResult,
}

/// Per-chain-entry result. Untagged to match the current wire
/// format's mutually-exclusive shape (no discriminator tag — the
/// consumer branches on the presence of `sha256` vs `error`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum UpdateChainResult {
    /// File was hashed successfully.
    Ok {
        /// Lowercase hex sha256 of the file.
        sha256: String,
    },
    /// File could not be hashed (missing, permission denied,
    /// read error). `reason` is operator-facing.
    Error {
        /// Human-readable error.
        error: String,
    },
}

/// Envelope emitted by `aegis-boot recommend --json`. Untagged
/// wrapper around three mutually-exclusive shapes: a full catalog
/// listing, a single-entry response, or a miss. Consumers branch
/// on the presence of `entries` / `entry` / `error`.
///
/// The miss shape intentionally omits `tool_version` — matches
/// the existing wire format. Future schema bumps can unify the
/// three shapes; Phase 4b-6 preserves the current contract.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum RecommendReport {
    /// Full catalog listing. Emitted when `aegis-boot recommend
    /// --json` is called with no slug.
    Catalog(RecommendCatalogReport),
    /// Single-entry response. Emitted when the slug matched one
    /// catalog entry exactly.
    Single(RecommendSingleReport),
    /// Miss — the slug didn't match any entry.
    Miss(RecommendMissReport),
}

/// Full-catalog variant of [`RecommendReport`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RecommendCatalogReport {
    /// Wire-format version. See [`RECOMMEND_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// `aegis-boot` binary version that produced this envelope.
    pub tool_version: String,
    /// Total entries in the catalog. Equals `entries.len()`.
    pub count: u32,
    /// All catalog entries in the order `CATALOG` defines them
    /// (typically alphabetical by slug).
    pub entries: Vec<RecommendEntry>,
}

/// Single-entry variant of [`RecommendReport`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RecommendSingleReport {
    /// Wire-format version. See [`RECOMMEND_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// `aegis-boot` binary version that produced this envelope.
    pub tool_version: String,
    /// The matched catalog entry.
    pub entry: RecommendEntry,
}

/// Miss variant of [`RecommendReport`] — no catalog entry matched
/// the given slug. The envelope is deliberately asymmetric from
/// the success variants (no `tool_version`) to match the existing
/// wire format; tightening to always emit `tool_version` would be
/// an additive (non-breaking) future change.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RecommendMissReport {
    /// Wire-format version. See [`RECOMMEND_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Human-readable error ("no catalog entry matching '<slug>'").
    pub error: String,
}

/// Envelope emitted by `aegis-boot compat --json`. Untagged
/// wrapper around 4 mutually-exclusive shapes: full catalog,
/// single match, miss (query matched no DB entry), or
/// my-machine-miss (DMI lookup couldn't resolve an identity).
///
/// Dispatch by field presence:
/// * `entries` → [`CompatReport::Catalog`]
/// * `entry` → [`CompatReport::Single`]
/// * `report_url` + `error` (no entries/entry) → [`CompatReport::Miss`]
/// * `error` without `report_url` → [`CompatReport::MyMachineMiss`]
///
/// The separate `CompatSubmitReport` carries the `--submit` flow's
/// own shape and schema version.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum CompatReport {
    /// Full catalog listing. Emitted with no query argument.
    Catalog(CompatCatalogReport),
    /// Single match. Emitted when the query resolved exactly one
    /// DB entry.
    Single(CompatSingleReport),
    /// Miss. Emitted when the query didn't match any DB entry
    /// (but the query was well-formed).
    Miss(CompatMissReport),
    /// My-machine miss. Emitted when `--my-machine` or
    /// `--submit` couldn't resolve DMI identity (non-Linux host,
    /// placeholder values). Exit code on the CLI side is 2
    /// (host-environment issue) vs the Miss case's 1 (DB coverage
    /// gap).
    MyMachineMiss(CompatMyMachineMissReport),
}

/// Full-catalog variant of [`CompatReport`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CompatCatalogReport {
    /// Wire-format version. See [`COMPAT_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// `aegis-boot` binary version that produced this envelope.
    pub tool_version: String,
    /// URL operators visit to file a new hardware report.
    pub report_url: String,
    /// Number of entries in the DB. Equals `entries.len()`.
    pub count: u32,
    /// All entries in DB declaration order.
    pub entries: Vec<CompatEntry>,
}

/// Single-entry variant of [`CompatReport`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CompatSingleReport {
    /// Wire-format version. See [`COMPAT_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// `aegis-boot` binary version that produced this envelope.
    pub tool_version: String,
    /// URL operators visit to file a new hardware report.
    pub report_url: String,
    /// The matched DB entry.
    pub entry: CompatEntry,
}

/// Miss variant of [`CompatReport`] — the query was well-formed but
/// didn't match any DB entry. Carries `report_url` so the operator
/// can file a new entry; deliberately omits `tool_version` to match
/// the existing wire format.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CompatMissReport {
    /// Wire-format version. See [`COMPAT_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// URL operators visit to file a new hardware report.
    pub report_url: String,
    /// Human-readable error (`"no platform matching '<query>'"`).
    pub error: String,
}

/// My-machine-miss variant of [`CompatReport`] — `--my-machine` or
/// `--submit` couldn't auto-fill the query from DMI. Minimal
/// envelope (just `schema_version` + `error`) to match the existing
/// wire format.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CompatMyMachineMissReport {
    /// Wire-format version. See [`COMPAT_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Human-readable error
    /// (`"--my-machine: DMI fields unavailable (…)"`).
    pub error: String,
}

/// One hardware-compatibility DB row. Mirrors
/// `docs/HARDWARE_COMPAT.md`; every entry corresponds to a real
/// operator report (or the QEMU reference environment).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CompatEntry {
    /// Vendor (e.g. `"Lenovo"`, `"Framework"`, `"QEMU"`).
    pub vendor: String,
    /// Model (e.g. `"ThinkPad X1 Carbon Gen 11"`).
    pub model: String,
    /// Firmware vendor + version, free-form from BIOS.
    pub firmware: String,
    /// Secure Boot state at the time of the report
    /// (typically `"enforcing"` or `"disabled"`).
    pub sb_state: String,
    /// Boot-menu key for this firmware (`"F12"`, `"Esc"`, etc.).
    /// `"n/a"` for reference / virtualized environments.
    pub boot_key: String,
    /// Confidence level: `"verified"`, `"partial"`, or
    /// `"reference"`.
    pub level: String,
    /// GitHub handle or `"aegis-team"` that filed the report.
    pub reported_by: String,
    /// ISO-8601 date string (`"2026-04-18"`).
    pub date: String,
    /// Free-text operator notes (quirks, BIOS tweaks,
    /// fast-boot caveats). May be empty for a clean boot.
    pub notes: Vec<String>,
}

/// Envelope emitted by `aegis-boot compat --submit --json` — the
/// draft-a-hardware-report flow. Collects DMI identity + builds a
/// pre-filled GitHub issue URL the operator can open to file a
/// report. Independent schema from [`CompatReport`] because the
/// consumer contracts diverge: lookup drives scripted decisions,
/// submit drives an operator workflow.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CompatSubmitReport {
    /// Wire-format version. See [`COMPAT_SUBMIT_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Always `"aegis-boot"`. Deliberately named `tool` (not
    /// `tool_version`) to match the existing wire format; this
    /// envelope carries the draft template, not a version pin.
    pub tool: String,
    /// Pre-filled GitHub issue URL with `vendor`, `model`,
    /// `firmware`, and `aegis-version` query-string parameters
    /// set from DMI.
    pub submit_url: String,
    /// DMI `sys_vendor`. Empty string if unavailable.
    pub vendor: String,
    /// DMI product label (name + version). Empty if unavailable.
    pub model: String,
    /// DMI BIOS label (vendor + version + date). Empty if
    /// unavailable.
    pub firmware: String,
}

/// Envelope emitted by `aegis-boot doctor --json`. Reports host +
/// stick health as a rollup score + per-check rows. The monitoring /
/// CI consumer target — a non-zero `has_any_fail` is the signal to
/// surface to an operator.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DoctorReport {
    /// Wire-format version. See [`DOCTOR_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// `aegis-boot` binary version that produced this envelope.
    pub tool_version: String,
    /// Aggregate score (0–100). PASS = 1.0, WARN = 0.7, FAIL =
    /// 0.0; skipped rows are excluded from the denominator.
    pub score: u32,
    /// Human-readable score band: typically `"EXCELLENT"`,
    /// `"GOOD"`, `"FAIR"`, or `"POOR"`. Exact thresholds are an
    /// implementation detail of the CLI; consumers should treat
    /// these as opaque labels paired with the numeric `score`.
    pub band: String,
    /// True iff any row's `verdict` is `"FAIL"`. The minimal
    /// rollup bit for monitoring: operator attention required.
    pub has_any_fail: bool,
    /// Operator-facing remediation hint pulled from the first
    /// `FAIL` row's `next_action` text. `None` when no row failed
    /// or none carried a remedy.
    pub next_action: Option<String>,
    /// One entry per check run. Order is check-declaration order
    /// inside `doctor.rs` — stable across invocations on the same
    /// host.
    pub rows: Vec<DoctorRow>,
}

/// One check result in a [`DoctorReport`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DoctorRow {
    /// Verdict label — `"PASS"`, `"WARN"`, `"FAIL"`, or `"SKIP"`.
    /// String (not enum) because the CLI's verdict vocabulary is
    /// intentionally loose: new verdicts can be added without a
    /// `schema_version` bump so long as consumers treat unknown
    /// values as "don't block on this."
    pub verdict: String,
    /// Short check name (e.g. `"command: mcopy"`).
    pub name: String,
    /// Single-line detail (filepath, value, or error message).
    pub detail: String,
}

/// Generic error envelope emitted when a subcommand fails before
/// it can produce its subcommand-specific `--json` envelope.
/// Examples: `aegis-boot list --json` before mount-resolution
/// succeeds; `aegis-boot verify --json` before a stick is found.
///
/// Kept deliberately minimal (just `schema_version` + `error`) so
/// scripted consumers can parse it without knowing which
/// subcommand was called. Shared across subcommands because the
/// pre-dispatch failure path is semantically identical.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CliError {
    /// Wire-format version. See [`CLI_ERROR_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Human-readable error message.
    pub error: String,
}

/// Tier-A anonymous failure microreport — written by `rescue-tui`
/// / initramfs to `AEGIS_ISOS/aegis-boot-logs/<ts>-<hash>.json`
/// when a classifiable boot failure occurs, so the operator can
/// later include the log in an `aegis-boot bug-report` bundle
/// (#342 Phase 2).
///
/// **Anonymous by construction.** Every field is either an
/// aegis-boot version, a loosely-bucketed machine-family hint, or
/// an opaque content hash. No hostname, no DMI serial, no full
/// error text. Matches [ABRT's uReport]
/// (<https://fedoraproject.org/wiki/Features/SimplifiedCrashReporting>)
/// pattern: safe to ship without operator review, useful for
/// failure-class correlation across a fleet.
///
/// Tier B (full structured log, consent-gated) lands in a later
/// phase with a distinct `schema_version` track.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct FailureMicroreport {
    /// Wire-format version. See [`FAILURE_MICROREPORT_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Tier marker. Always `"A"` in this envelope; reserved for
    /// `"B"` when the consent-gated full-log tier ships.
    pub tier: String,
    /// RFC-3339 UTC timestamp of when the microreport was written.
    pub collected_at: String,
    /// `aegis-boot` version string that produced this log.
    pub aegis_boot_version: String,
    /// Lowercased first token of the DMI `sys_vendor` field (e.g.
    /// `"framework"`, `"lenovo"`, `"dell"`). Vendor-granularity
    /// only — enough to correlate per-vendor bugs without
    /// identifying the operator.
    pub vendor_family: String,
    /// Four-digit year extracted from the DMI `bios_date` (e.g.
    /// `"2024"`). Year-granularity is coarse enough to preserve
    /// anonymity on any laptop model older than a few months.
    pub bios_year: String,
    /// Classified boot stage the failure occurred at. One of:
    /// `"pre_kernel"`, `"kernel_init"`, `"initramfs"`,
    /// `"rescue_tui"`, `"kexec_handoff"`.
    pub boot_step_reached: String,
    /// Classified failure code. String (not enum) so new
    /// classifications can be added without a `schema_version`
    /// bump. Consumer convention: treat unknown codes as
    /// `"unclassified"`.
    pub failure_class: String,
    /// Opaque hash (`sha256:<64-hex>`) of the full raw error text.
    /// Lets a maintainer match two field reports as "same failure"
    /// without either operator sharing the raw text.
    pub failure_hash: String,
}

/// One curated catalog entry. Used in both
/// [`RecommendCatalogReport::entries`] and
/// [`RecommendSingleReport::entry`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RecommendEntry {
    /// Short stable identifier (e.g. `"ubuntu-24.04-live-server"`).
    /// Used by `aegis-boot fetch <slug>` to resolve a URL set.
    pub slug: String,
    /// Human-readable name (e.g. `"Ubuntu 24.04 LTS Live Server"`).
    pub name: String,
    /// CPU architecture (`"amd64"`, `"arm64"`, …).
    pub arch: String,
    /// ISO size in mebibytes (rounded to nearest; informational
    /// for download-time estimates, not a strict guarantee).
    /// `u32` accommodates up to ~4 PiB — plenty of headroom for
    /// any realistic ISO.
    pub size_mib: u32,
    /// HTTPS URL of the ISO body.
    pub iso_url: String,
    /// HTTPS URL of the upstream SHA256SUMS file.
    pub sha256_url: String,
    /// HTTPS URL of the detached signature over the SHA256SUMS file
    /// (typically a GPG `.gpg`).
    pub sig_url: String,
    /// Secure Boot status string — one of `"signed:<vendor>"` (e.g.
    /// `"signed:canonical"`), `"unsigned-needs-mok"`, or
    /// `"unknown"`.
    pub sb: String,
    /// One-line operator-facing purpose (e.g. `"Standard server
    /// install media"`).
    pub purpose: String,
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
                    folder: Some("ubuntu-24.04".to_string()),
                    size_bytes: 5_368_709_120,
                    has_sha256: true,
                    has_minisig: false,
                    display_name: Some("Ubuntu 24.04 Desktop".to_string()),
                    description: None,
                },
                ListIsoSummary {
                    name: "debian-12.iso".to_string(),
                    folder: None,
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

    fn sample_attest_list_success() -> AttestListSuccess {
        AttestListSuccess {
            manifest_path: "/home/alice/.local/share/aegis-boot/attestations/abc.json".to_string(),
            schema_version: ATTESTATION_SCHEMA_VERSION,
            tool_version: "aegis-boot 0.14.1".to_string(),
            flashed_at: "2026-04-19T14:30:00Z".to_string(),
            operator: "alice".to_string(),
            target_device: "/dev/sda".to_string(),
            target_model: "SanDisk Cruzer".to_string(),
            disk_guid: "00000000-0000-0000-0000-000000000001".to_string(),
            iso_count: 3,
        }
    }

    #[test]
    fn attest_list_schema_version_is_one() {
        assert_eq!(ATTEST_LIST_SCHEMA_VERSION, 1);
    }

    #[test]
    fn attest_list_success_serializes_without_error_field() {
        // The untagged enum's Success variant must NOT emit an
        // `error` key — that's how consumers branch between the
        // two shapes.
        let entry = AttestListEntry::Success(sample_attest_list_success());
        let body = serde_json::to_string(&entry).expect("serialize");
        assert!(!body.contains("\"error\""), "must not have error: {body}");
        assert!(body.contains("\"operator\":\"alice\""));
    }

    #[test]
    fn attest_list_error_serializes_without_summary_fields() {
        // The Error variant must NOT emit any of the success
        // fields (schema_version, tool_version, flashed_at,
        // operator, target_device, target_model, disk_guid,
        // iso_count). This is the mutually-exclusive shape
        // contract that Phase 4b-3's untagged enum preserves.
        let entry = AttestListEntry::Error(AttestListError {
            manifest_path: "/tmp/broken.json".to_string(),
            error: "parse failed: missing field".to_string(),
        });
        let body = serde_json::to_string(&entry).expect("serialize");
        assert!(body.contains("\"error\":"));
        for success_field in &[
            "schema_version",
            "tool_version",
            "flashed_at",
            "operator",
            "target_device",
            "target_model",
            "disk_guid",
            "iso_count",
        ] {
            let pattern = format!("\"{success_field}\"");
            assert!(
                !body.contains(&pattern),
                "Error variant must not emit {success_field}: {body}"
            );
        }
    }

    #[test]
    fn attest_list_entry_round_trips() {
        // An untagged-enum round-trip through serde must pick the
        // right variant based on field shape. Success → Success,
        // Error → Error.
        let success = AttestListEntry::Success(sample_attest_list_success());
        let body = serde_json::to_string(&success).expect("serialize");
        let parsed: AttestListEntry = serde_json::from_str(&body).expect("parse");
        assert_eq!(success, parsed);

        let err = AttestListEntry::Error(AttestListError {
            manifest_path: "/tmp/x.json".to_string(),
            error: "nope".to_string(),
        });
        let body = serde_json::to_string(&err).expect("serialize");
        let parsed: AttestListEntry = serde_json::from_str(&body).expect("parse");
        assert_eq!(err, parsed);
    }

    fn sample_verify_report() -> VerifyReport {
        VerifyReport {
            schema_version: VERIFY_SCHEMA_VERSION,
            tool_version: "0.14.1".to_string(),
            mount_path: "/run/media/alice/AEGIS_ISOS".to_string(),
            summary: VerifySummary {
                total: 4,
                verified: 1,
                mismatch: 1,
                unreadable: 1,
                not_present: 1,
                any_failure: true,
            },
            isos: vec![
                VerifyEntry {
                    name: "ubuntu.iso".to_string(),
                    verdict: VerifyVerdict::Verified {
                        digest: "a".repeat(64),
                        source: "sidecar".to_string(),
                    },
                },
                VerifyEntry {
                    name: "debian.iso".to_string(),
                    verdict: VerifyVerdict::Mismatch {
                        actual: "b".repeat(64),
                        expected: "c".repeat(64),
                        source: "sidecar".to_string(),
                    },
                },
                VerifyEntry {
                    name: "alpine.iso".to_string(),
                    verdict: VerifyVerdict::Unreadable {
                        source: "sidecar".to_string(),
                        reason: "permission denied".to_string(),
                    },
                },
                VerifyEntry {
                    name: "fedora.iso".to_string(),
                    verdict: VerifyVerdict::NotPresent,
                },
            ],
        }
    }

    #[test]
    fn verify_schema_version_is_one() {
        assert_eq!(VERIFY_SCHEMA_VERSION, 1);
    }

    #[test]
    fn verify_round_trip_preserves_all_variants() {
        let r = sample_verify_report();
        let body = serde_json::to_string(&r).expect("serialize");
        let parsed: VerifyReport = serde_json::from_str(&body).expect("parse");
        assert_eq!(r, parsed);
    }

    #[test]
    fn verify_entry_emits_name_before_verdict() {
        // Consumer contract: `name` is the first key so a
        // streaming JSON parser can key off it before seeing the
        // variant-specific fields. `#[serde(flatten)]` on the
        // `verdict` field + internally-tagged enum gives us this
        // ordering for free; this test is the guard.
        let entry = VerifyEntry {
            name: "x".to_string(),
            verdict: VerifyVerdict::NotPresent,
        };
        let body = serde_json::to_string(&entry).expect("serialize");
        let name_pos = body.find("\"name\"").expect("has name");
        let verdict_pos = body.find("\"verdict\"").expect("has verdict");
        assert!(
            name_pos < verdict_pos,
            "name must come before verdict: {body}"
        );
    }

    #[test]
    fn verify_notpresent_emits_no_variant_fields() {
        // The unit variant NotPresent must NOT emit `digest`,
        // `actual`, `expected`, `source`, or `reason` — those are
        // variant-specific and would confuse a consumer that
        // dispatched on the `verdict` tag.
        let entry = VerifyEntry {
            name: "x".to_string(),
            verdict: VerifyVerdict::NotPresent,
        };
        let body = serde_json::to_string(&entry).expect("serialize");
        for field in &["digest", "actual", "expected", "source", "reason"] {
            let pattern = format!("\"{field}\"");
            assert!(
                !body.contains(&pattern),
                "NotPresent must not emit {field}: {body}"
            );
        }
    }

    #[test]
    fn verify_verdict_tags_match_strings() {
        // The four tag strings are part of the wire contract.
        // Consumers branch on these literals; this test pins the
        // spelling.
        let v = VerifyEntry {
            name: "x".to_string(),
            verdict: VerifyVerdict::Verified {
                digest: "d".to_string(),
                source: "s".to_string(),
            },
        };
        let body = serde_json::to_string(&v).expect("serialize");
        assert!(body.contains("\"verdict\":\"Verified\""));

        let m = VerifyEntry {
            name: "x".to_string(),
            verdict: VerifyVerdict::Mismatch {
                actual: "a".to_string(),
                expected: "e".to_string(),
                source: "s".to_string(),
            },
        };
        assert!(serde_json::to_string(&m)
            .expect("ok")
            .contains("\"verdict\":\"Mismatch\""));

        let u = VerifyEntry {
            name: "x".to_string(),
            verdict: VerifyVerdict::Unreadable {
                source: "s".to_string(),
                reason: "r".to_string(),
            },
        };
        assert!(serde_json::to_string(&u)
            .expect("ok")
            .contains("\"verdict\":\"Unreadable\""));

        let n = VerifyEntry {
            name: "x".to_string(),
            verdict: VerifyVerdict::NotPresent,
        };
        assert!(serde_json::to_string(&n)
            .expect("ok")
            .contains("\"verdict\":\"NotPresent\""));
    }

    fn sample_update_eligible() -> UpdateReport {
        UpdateReport {
            schema_version: UPDATE_SCHEMA_VERSION,
            tool_version: "0.14.1".to_string(),
            device: "/dev/sda".to_string(),
            eligibility: UpdateEligibility::Eligible {
                disk_guid: "00000000-0000-0000-0000-000000000001".to_string(),
                attestation_path: "/home/alice/.local/share/aegis-boot/attestations/abc.json"
                    .to_string(),
                host_chain: vec![
                    UpdateChainEntry {
                        role: "shim".to_string(),
                        path: "/usr/lib/shim/shimx64.efi.signed".to_string(),
                        result: UpdateChainResult::Ok {
                            sha256: "a".repeat(64),
                        },
                    },
                    UpdateChainEntry {
                        role: "grub".to_string(),
                        path: "/usr/lib/grub/x86_64-efi/grubx64.efi".to_string(),
                        result: UpdateChainResult::Error {
                            error: "file not found".to_string(),
                        },
                    },
                ],
            },
        }
    }

    fn sample_update_ineligible() -> UpdateReport {
        UpdateReport {
            schema_version: UPDATE_SCHEMA_VERSION,
            tool_version: "0.14.1".to_string(),
            device: "/dev/sdb".to_string(),
            eligibility: UpdateEligibility::Ineligible {
                reason: "device is not removable (looks like an internal disk)".to_string(),
            },
        }
    }

    #[test]
    fn update_schema_version_is_one() {
        assert_eq!(UPDATE_SCHEMA_VERSION, 1);
    }

    #[test]
    fn update_round_trip_preserves_all_variants() {
        let eligible = sample_update_eligible();
        let body = serde_json::to_string(&eligible).expect("serialize");
        let parsed: UpdateReport = serde_json::from_str(&body).expect("parse");
        assert_eq!(eligible, parsed);

        let ineligible = sample_update_ineligible();
        let body = serde_json::to_string(&ineligible).expect("serialize");
        let parsed: UpdateReport = serde_json::from_str(&body).expect("parse");
        assert_eq!(ineligible, parsed);
    }

    #[test]
    fn update_emits_header_fields_before_eligibility() {
        // Consumer contract: `schema_version, tool_version, device`
        // appear before the `eligibility` tag. Pre-flatten, the
        // field order is pinned by struct declaration.
        let r = sample_update_ineligible();
        let body = serde_json::to_string(&r).expect("serialize");
        let sv_pos = body.find("\"schema_version\"").expect("sv");
        let tv_pos = body.find("\"tool_version\"").expect("tv");
        let dev_pos = body.find("\"device\"").expect("dev");
        let elig_pos = body.find("\"eligibility\"").expect("eligibility");
        assert!(sv_pos < tv_pos, "{body}");
        assert!(tv_pos < dev_pos, "{body}");
        assert!(dev_pos < elig_pos, "{body}");
    }

    #[test]
    fn update_eligibility_tags_match_upper_case_wire_strings() {
        let e = sample_update_eligible();
        let body = serde_json::to_string(&e).expect("serialize");
        assert!(body.contains("\"eligibility\":\"ELIGIBLE\""), "{body}");
        let i = sample_update_ineligible();
        let body = serde_json::to_string(&i).expect("serialize");
        assert!(body.contains("\"eligibility\":\"INELIGIBLE\""), "{body}");
    }

    #[test]
    fn update_chain_entry_variants_are_mutually_exclusive() {
        // Success variant emits sha256, no error field.
        let ok = UpdateChainEntry {
            role: "shim".to_string(),
            path: "/path/to/shim".to_string(),
            result: UpdateChainResult::Ok {
                sha256: "a".repeat(64),
            },
        };
        let body = serde_json::to_string(&ok).expect("serialize");
        assert!(body.contains("\"sha256\""));
        assert!(!body.contains("\"error\""), "{body}");
        // Error variant emits error, no sha256 field.
        let err = UpdateChainEntry {
            role: "grub".to_string(),
            path: "/path/to/grub".to_string(),
            result: UpdateChainResult::Error {
                error: "missing".to_string(),
            },
        };
        let body = serde_json::to_string(&err).expect("serialize");
        assert!(body.contains("\"error\""));
        assert!(!body.contains("\"sha256\""), "{body}");
    }

    fn sample_recommend_entry() -> RecommendEntry {
        RecommendEntry {
            slug: "ubuntu-24.04-live-server".to_string(),
            name: "Ubuntu 24.04 LTS Live Server".to_string(),
            arch: "amd64".to_string(),
            size_mib: 2_400_u32,
            iso_url: "https://releases.ubuntu.com/24.04/ubuntu-24.04-live-server-amd64.iso"
                .to_string(),
            sha256_url: "https://releases.ubuntu.com/24.04/SHA256SUMS".to_string(),
            sig_url: "https://releases.ubuntu.com/24.04/SHA256SUMS.gpg".to_string(),
            sb: "signed:canonical".to_string(),
            purpose: "Standard server install media".to_string(),
        }
    }

    #[test]
    fn recommend_schema_version_is_one() {
        assert_eq!(RECOMMEND_SCHEMA_VERSION, 1);
    }

    #[test]
    fn recommend_catalog_round_trips() {
        let report = RecommendReport::Catalog(RecommendCatalogReport {
            schema_version: RECOMMEND_SCHEMA_VERSION,
            tool_version: "0.14.1".to_string(),
            count: 1,
            entries: vec![sample_recommend_entry()],
        });
        let body = serde_json::to_string(&report).expect("serialize");
        let parsed: RecommendReport = serde_json::from_str(&body).expect("parse");
        assert_eq!(report, parsed);
        assert!(body.contains("\"entries\""));
        assert!(!body.contains("\"entry\""));
        assert!(!body.contains("\"error\""));
    }

    #[test]
    fn recommend_single_round_trips() {
        let report = RecommendReport::Single(RecommendSingleReport {
            schema_version: RECOMMEND_SCHEMA_VERSION,
            tool_version: "0.14.1".to_string(),
            entry: sample_recommend_entry(),
        });
        let body = serde_json::to_string(&report).expect("serialize");
        let parsed: RecommendReport = serde_json::from_str(&body).expect("parse");
        assert_eq!(report, parsed);
        assert!(body.contains("\"entry\""));
        assert!(!body.contains("\"entries\""));
        assert!(!body.contains("\"error\""));
    }

    #[test]
    fn recommend_miss_round_trips_and_omits_tool_version() {
        // The miss envelope intentionally does NOT carry
        // tool_version — that's the existing wire-format asymmetry
        // we're preserving. Phase 4b-6 keeps this.
        let report = RecommendReport::Miss(RecommendMissReport {
            schema_version: RECOMMEND_SCHEMA_VERSION,
            error: "no catalog entry matching 'x'".to_string(),
        });
        let body = serde_json::to_string(&report).expect("serialize");
        let parsed: RecommendReport = serde_json::from_str(&body).expect("parse");
        assert_eq!(report, parsed);
        assert!(body.contains("\"error\""));
        assert!(
            !body.contains("\"tool_version\""),
            "miss omits tool_version: {body}"
        );
    }

    #[test]
    fn recommend_untagged_dispatch_by_field_presence() {
        // Serde-untagged distinguishes the three variants by the
        // presence of their signature fields (entries / entry /
        // error). This test pins that an out-of-band parser that
        // dispatches on field presence can recover the right
        // variant from bytes alone.
        let catalog_body = r#"{"schema_version":1,"tool_version":"0.1.0","count":0,"entries":[]}"#;
        let parsed: RecommendReport = serde_json::from_str(catalog_body).expect("catalog parse");
        assert!(matches!(parsed, RecommendReport::Catalog(_)));

        let miss_body = r#"{"schema_version":1,"error":"not found"}"#;
        let parsed: RecommendReport = serde_json::from_str(miss_body).expect("miss parse");
        assert!(matches!(parsed, RecommendReport::Miss(_)));
    }

    fn sample_compat_entry() -> CompatEntry {
        CompatEntry {
            vendor: "Framework".to_string(),
            model: "Laptop (12th Gen Intel Core) / A6".to_string(),
            firmware: "INSYDE Corp. 03.19".to_string(),
            sb_state: "enforcing".to_string(),
            boot_key: "F12".to_string(),
            level: "verified".to_string(),
            reported_by: "@williamzujkowski".to_string(),
            date: "2026-04-18".to_string(),
            notes: vec!["Full chain validated".to_string()],
        }
    }

    #[test]
    fn compat_schema_versions_are_one() {
        assert_eq!(COMPAT_SCHEMA_VERSION, 1);
        assert_eq!(COMPAT_SUBMIT_SCHEMA_VERSION, 1);
    }

    #[test]
    fn compat_report_catalog_round_trips() {
        let report = CompatReport::Catalog(CompatCatalogReport {
            schema_version: COMPAT_SCHEMA_VERSION,
            tool_version: "0.14.1".to_string(),
            report_url: "https://example.com/report".to_string(),
            count: 1,
            entries: vec![sample_compat_entry()],
        });
        let body = serde_json::to_string(&report).expect("serialize");
        let parsed: CompatReport = serde_json::from_str(&body).expect("parse");
        assert_eq!(report, parsed);
        assert!(body.contains("\"entries\""));
    }

    #[test]
    fn compat_report_miss_omits_tool_version() {
        let report = CompatReport::Miss(CompatMissReport {
            schema_version: COMPAT_SCHEMA_VERSION,
            report_url: "https://example.com/report".to_string(),
            error: "no platform matching 'foo'".to_string(),
        });
        let body = serde_json::to_string(&report).expect("serialize");
        assert!(body.contains("\"report_url\""));
        assert!(body.contains("\"error\""));
        assert!(
            !body.contains("\"tool_version\""),
            "miss omits tool_version: {body}"
        );
    }

    #[test]
    fn compat_report_my_machine_miss_has_minimal_shape() {
        let report = CompatReport::MyMachineMiss(CompatMyMachineMissReport {
            schema_version: COMPAT_SCHEMA_VERSION,
            error: "--my-machine: DMI fields unavailable".to_string(),
        });
        let body = serde_json::to_string(&report).expect("serialize");
        assert!(body.contains("\"error\""));
        assert!(!body.contains("\"report_url\""));
        assert!(!body.contains("\"tool_version\""));
    }

    #[test]
    fn compat_untagged_dispatch_by_field_presence() {
        let body =
            r#"{"schema_version":1,"tool_version":"0.1","report_url":"x","count":0,"entries":[]}"#;
        assert!(matches!(
            serde_json::from_str::<CompatReport>(body).expect("catalog"),
            CompatReport::Catalog(_)
        ));

        let body = r#"{"schema_version":1,"tool_version":"0.1","report_url":"x","entry":{"vendor":"","model":"","firmware":"","sb_state":"","boot_key":"","level":"","reported_by":"","date":"","notes":[]}}"#;
        assert!(matches!(
            serde_json::from_str::<CompatReport>(body).expect("single"),
            CompatReport::Single(_)
        ));

        let body = r#"{"schema_version":1,"report_url":"x","error":"nope"}"#;
        assert!(matches!(
            serde_json::from_str::<CompatReport>(body).expect("miss"),
            CompatReport::Miss(_)
        ));

        let body = r#"{"schema_version":1,"error":"dmi"}"#;
        assert!(matches!(
            serde_json::from_str::<CompatReport>(body).expect("mymachine"),
            CompatReport::MyMachineMiss(_)
        ));
    }

    #[test]
    fn compat_submit_uses_tool_not_tool_version() {
        let r = CompatSubmitReport {
            schema_version: COMPAT_SUBMIT_SCHEMA_VERSION,
            tool: "aegis-boot".to_string(),
            submit_url: "https://example.com/new?vendor=x".to_string(),
            vendor: "x".to_string(),
            model: "y".to_string(),
            firmware: "z".to_string(),
        };
        let body = serde_json::to_string(&r).expect("serialize");
        let parsed: CompatSubmitReport = serde_json::from_str(&body).expect("parse");
        assert_eq!(r, parsed);
        assert!(body.contains("\"tool\":\"aegis-boot\""));
        assert!(!body.contains("\"tool_version\""), "{body}");
        assert!(body.contains("\"submit_url\""));
    }

    fn sample_doctor_report() -> DoctorReport {
        DoctorReport {
            schema_version: DOCTOR_SCHEMA_VERSION,
            tool_version: "0.14.1".to_string(),
            score: 85,
            band: "GOOD".to_string(),
            has_any_fail: false,
            next_action: None,
            rows: vec![
                DoctorRow {
                    verdict: "PASS".to_string(),
                    name: "OS".to_string(),
                    detail: "Linux 6.17.0".to_string(),
                },
                DoctorRow {
                    verdict: "WARN".to_string(),
                    name: "Secure Boot (host)".to_string(),
                    detail: "disabled".to_string(),
                },
            ],
        }
    }

    #[test]
    fn doctor_schema_version_is_one() {
        assert_eq!(DOCTOR_SCHEMA_VERSION, 1);
    }

    #[test]
    fn doctor_round_trips_and_preserves_all_fields() {
        let r = sample_doctor_report();
        let body = serde_json::to_string(&r).expect("serialize");
        let parsed: DoctorReport = serde_json::from_str(&body).expect("parse");
        assert_eq!(r, parsed);
    }

    #[test]
    fn doctor_next_action_null_when_absent() {
        // next_action = null when no FAIL row carried a remedy.
        // Must serialize as `null` (not omitted) to keep the field
        // set stable for consumers.
        let r = sample_doctor_report();
        let body = serde_json::to_string(&r).expect("serialize");
        assert!(
            body.contains("\"next_action\":null"),
            "next_action must be explicit null: {body}"
        );
    }

    #[test]
    fn doctor_next_action_populated_on_fail() {
        let mut r = sample_doctor_report();
        r.has_any_fail = true;
        r.next_action = Some("install mcopy".to_string());
        r.rows.push(DoctorRow {
            verdict: "FAIL".to_string(),
            name: "command: mcopy".to_string(),
            detail: "not found in PATH".to_string(),
        });
        let body = serde_json::to_string(&r).expect("serialize");
        assert!(body.contains("\"has_any_fail\":true"));
        assert!(body.contains("\"next_action\":\"install mcopy\""));
    }

    #[test]
    fn doctor_row_verdict_is_free_string_not_enum() {
        // The verdict field accepts any string — the CLI's
        // vocabulary can grow with new verdict labels without
        // bumping schema_version. Consumer contract: treat
        // unknown verdicts as "informational / don't block."
        let r = DoctorRow {
            verdict: "FUTURE-VERDICT-LABEL".to_string(),
            name: "some new check".to_string(),
            detail: "novel condition".to_string(),
        };
        let body = serde_json::to_string(&r).expect("serialize");
        let parsed: DoctorRow = serde_json::from_str(&body).expect("parse");
        assert_eq!(r, parsed);
    }

    #[test]
    fn cli_error_schema_version_is_one() {
        assert_eq!(CLI_ERROR_SCHEMA_VERSION, 1);
    }

    #[test]
    fn cli_error_round_trips_and_escapes_properly() {
        // serde_json handles the escaping — no more hand-rolled
        // json_escape needed.
        let e = CliError {
            schema_version: CLI_ERROR_SCHEMA_VERSION,
            error: "mount failed: \"/dev/sdX\" is not removable".to_string(),
        };
        let body = serde_json::to_string(&e).expect("serialize");
        let parsed: CliError = serde_json::from_str(&body).expect("parse");
        assert_eq!(e, parsed);
        // The embedded quotes must be properly escaped on the wire.
        assert!(
            body.contains(r#"\"/dev/sdX\""#),
            "embedded quotes must be escaped: {body}"
        );
    }
}
