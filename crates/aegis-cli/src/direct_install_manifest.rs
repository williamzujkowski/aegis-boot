//! Signed attestation manifest for direct-install (#274 PR3, #277).
//!
//! Companion module to [`direct_install`]. Produces
//! `::/aegis-boot-manifest.json` + `::/aegis-boot-manifest.json.minisig`
//! on the flashed ESP. The manifest is the contract between
//! flash-time attestation and runtime verification
//! (`aegis-boot doctor --stick`, rescue-tui stick check, aegis-hwsim E6
//! attestation-roundtrip scenario).
//!
//! **Schema version 1** is locked by [#277]. See the issue for the
//! full design rationale; the short version is:
//!
//! * Closed-set file list — verifier rejects the stick if any ESP
//!   file is not in `esp_files` or is missing / mismatched. Six
//!   entries, one per line in the signed-chain layout established by
//!   Phase 2b.
//! * `manifest_sequence` is monotonic-per-flash, defending against
//!   rollback to an older validly-signed manifest without relying on
//!   a secure RTC.
//! * `partition_count` + per-partition `type_guid` / `partuuid` /
//!   `fs_uuid` replaces the brittle `partition_table_sha256` — the
//!   GPT backup header LBA moves with disk size, so hashing it breaks
//!   re-flash onto a different stick.
//! * No `expected_pcrs` in PR3; E6 populates after TPM PCR selection
//!   lock.
//! * Manifest body is hard-capped at [`MAX_MANIFEST_BYTES`] to bound
//!   early-boot JSON parser exposure.
//!
//! PR3 ships **writer + signer + drift tests + a verify helper used
//! by tests**. No caller is wired; `#[allow(dead_code)]` at module
//! scope rides until a Phase 3 PR adds the `--direct-install` flag.
//!
//! [#277]: https://github.com/williamzujkowski/aegis-boot/issues/277

#![cfg(target_os = "linux")]
#![allow(dead_code)]

use std::fs;
use std::io::Cursor;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::direct_install::{
    ESP_DEST_GRUB, ESP_DEST_GRUB_CFG_BOOT, ESP_DEST_GRUB_CFG_UBUNTU, ESP_DEST_INITRD,
    ESP_DEST_KERNEL, ESP_DEST_SHIM,
};

/// Locked schema version for PR3. Bump alongside a breaking shape
/// change (removing a field, changing a field's type). Adding a new
/// optional field is backwards-compatible and does not require a
/// version bump — the verifier ignores fields it doesn't know about.
pub(crate) const SCHEMA_VERSION: u32 = 1;

/// Canonical file name of the manifest on the ESP. Operators and
/// verifiers MUST key off this exact path; moving it is a breaking
/// change that requires a schema version bump.
pub(crate) const MANIFEST_ESP_PATH: &str = "::/aegis-boot-manifest.json";

/// Canonical file name of the detached signature on the ESP.
pub(crate) const MANIFEST_SIG_ESP_PATH: &str = "::/aegis-boot-manifest.json.minisig";

/// Re-export the manifest size cap from the shared constants
/// registry. See [`crate::constants::MAX_MANIFEST_BYTES`] for the
/// rationale.
pub(crate) use crate::constants::MAX_MANIFEST_BYTES;

/// GPT type GUID for EFI System Partition.
pub(crate) const TYPE_GUID_ESP: &str = "C12A7328-F81F-11D2-BA4B-00A0C93EC93B";

/// GPT type GUID for Microsoft Basic Data (covers both FAT32 and
/// exFAT on our sticks — see the [`crate::direct_install`] doc on
/// `DATA_TYPE_CODE` for why the runtime doesn't key off this).
pub(crate) const TYPE_GUID_MSFT_BASIC_DATA: &str = "EBD0A0A2-B9E5-4433-87C0-68B6B72699C7";

/// Top-level manifest body. Serialized field order matches the
/// declaration order below — relied on for canonical JSON stability
/// (the signature is over `serde_json::to_vec(&Manifest)`).
///
/// The Rust field is `sequence` (clippy prefers not to prefix the
/// struct name); the JSON wire field stays `manifest_sequence` per
/// #277 schema lock via `#[serde(rename)]`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Manifest {
    pub schema_version: u32,
    pub tool_version: String,
    #[serde(rename = "manifest_sequence")]
    pub sequence: u64,
    pub device: Device,
    pub esp_files: Vec<EspFileEntry>,
    pub allowed_files_closed_set: bool,
    pub expected_pcrs: Vec<PcrEntry>,
}

/// Device identity captured at flash time. All values come from the
/// freshly-written GPT (`blkid` + `sgdisk -p`); verifier re-reads them
/// and asserts equality.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Device {
    pub disk_guid: String,
    pub partition_count: u32,
    pub esp: EspPartition,
    pub data: DataPartition,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct EspPartition {
    pub partuuid: String,
    pub type_guid: String,
    pub fs_uuid: String,
    pub first_lba: u64,
    pub last_lba: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct DataPartition {
    pub partuuid: String,
    pub type_guid: String,
    pub fs_uuid: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct EspFileEntry {
    /// Mtools-style `::/` path on the ESP. Verifier lowercases both
    /// sides before comparison (FAT32 is case-insensitive).
    pub path: String,
    pub sha256: String,
    pub size_bytes: u64,
}

/// Reserved for E6 / Phase 3 TPM attestation. Left out of the
/// serialized array by PR3 (`expected_pcrs: []`); once E6 locks the
/// PCR selection this struct grows populated rows.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct PcrEntry {
    pub pcr_index: u32,
    pub bank: String,
    pub digest_hex: String,
}

/// Errors from manifest build / parse / verify.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ManifestError {
    #[error("manifest body exceeds MAX_MANIFEST_BYTES ({actual} > {limit})")]
    TooLarge { actual: usize, limit: usize },

    #[error("manifest schema_version {got} is newer than this verifier's max supported ({max})")]
    SchemaTooNew { got: u32, max: u32 },

    #[error("manifest_sequence {got} is less than last-seen {last_seen}")]
    Rollback { got: u64, last_seen: u64 },

    #[error("manifest JSON parse: {0}")]
    Parse(#[from] serde_json::Error),

    #[error("manifest fs i/o {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("minisign: {0}")]
    Minisign(String),
}

/// Build a manifest from its inputs. Pure: no fs i/o, no subprocess.
/// The caller is responsible for having computed the file hashes +
/// gathered the GPT identity fields (a later helper in this module
/// will wrap those calls; for PR3 scope they're left to the eventual
/// Phase 3 caller so the writer layer stays testable without mocking
/// sgdisk or sha256 computation).
pub(crate) fn build_manifest(
    tool_version: &str,
    sequence: u64,
    device: Device,
    esp_files: [EspFileEntry; 6],
) -> Manifest {
    Manifest {
        schema_version: SCHEMA_VERSION,
        tool_version: tool_version.to_string(),
        sequence,
        device,
        esp_files: esp_files.into(),
        allowed_files_closed_set: true,
        expected_pcrs: Vec::new(),
    }
}

/// Pretty-print the manifest as JSON. Serialization field order is
/// struct-declaration order (serde's default), which gives us stable
/// bytes for the signature to cover.
pub(crate) fn serialize_manifest(m: &Manifest) -> Result<Vec<u8>, ManifestError> {
    let body = serde_json::to_vec_pretty(m)?;
    if body.len() > MAX_MANIFEST_BYTES {
        return Err(ManifestError::TooLarge {
            actual: body.len(),
            limit: MAX_MANIFEST_BYTES,
        });
    }
    Ok(body)
}

/// Parse a manifest body and enforce the non-crypto invariants
/// (size cap, schema forward-compat window, sequence rollback).
/// Signature verification is a separate helper; this function is
/// pure JSON + invariants and can be exercised without a keypair.
pub(crate) fn parse_and_validate_manifest(
    body: &[u8],
    last_seen_sequence: u64,
    max_schema: u32,
) -> Result<Manifest, ManifestError> {
    if body.len() > MAX_MANIFEST_BYTES {
        return Err(ManifestError::TooLarge {
            actual: body.len(),
            limit: MAX_MANIFEST_BYTES,
        });
    }
    let m: Manifest = serde_json::from_slice(body)?;
    if m.schema_version > max_schema {
        return Err(ManifestError::SchemaTooNew {
            got: m.schema_version,
            max: max_schema,
        });
    }
    if m.sequence < last_seen_sequence {
        return Err(ManifestError::Rollback {
            got: m.sequence,
            last_seen: last_seen_sequence,
        });
    }
    Ok(m)
}

/// Sign a serialized manifest body with a minisign secret key. The
/// detached signature is what gets written alongside the manifest
/// at `::/aegis-boot-manifest.json.minisig`.
///
/// Takes a live [`minisign::SecretKey`] rather than a serialized
/// `SecretKeyBox` so the caller controls the unlock-from-file step
/// (password prompts, key-from-env, HSM-backed loaders, etc.) — key
/// lifecycle is Phase 3 scope and this layer stays pure-crypto.
pub(crate) fn sign_manifest_body(
    manifest_body: &[u8],
    sk: &minisign::SecretKey,
) -> Result<Vec<u8>, ManifestError> {
    let sig_box = minisign::sign(
        None,
        sk,
        manifest_body,
        Some("aegis-boot attestation manifest"),
        Some("signed by aegis-boot direct-install"),
    )
    .map_err(|e| ManifestError::Minisign(format!("sign: {e}")))?;
    Ok(sig_box.into_string().into_bytes())
}

/// Verify a detached minisign signature against a public key.
/// Returns `Ok(())` on valid signature, `Err` otherwise.
pub(crate) fn verify_manifest_body(
    manifest_body: &[u8],
    sig_bytes: &[u8],
    pk: &minisign::PublicKey,
) -> Result<(), ManifestError> {
    let sig_str = std::str::from_utf8(sig_bytes)
        .map_err(|e| ManifestError::Minisign(format!("sig utf8: {e}")))?;
    let sig_box = minisign::SignatureBox::from_string(sig_str)
        .map_err(|e| ManifestError::Minisign(format!("sig parse: {e}")))?;
    let cursor = Cursor::new(manifest_body);
    minisign::verify(pk, &sig_box, cursor, true, false, false)
        .map_err(|e| ManifestError::Minisign(format!("verify: {e}")))?;
    Ok(())
}

/// Write the manifest body + detached signature to the given paths.
/// Called with local fs paths (the staging layer) or ESP block-device
/// paths; the fs module doesn't distinguish. The ESP write path in
/// Phase 3 will stage through a tempfile + mcopy rather than writing
/// directly to `/dev/sdX1`.
pub(crate) fn write_manifest_and_sig(
    body: &[u8],
    sig: &[u8],
    manifest_path: &Path,
    sig_path: &Path,
) -> Result<(), ManifestError> {
    fs::write(manifest_path, body).map_err(|source| ManifestError::Io {
        path: manifest_path.display().to_string(),
        source,
    })?;
    fs::write(sig_path, sig).map_err(|source| ManifestError::Io {
        path: sig_path.display().to_string(),
        source,
    })?;
    Ok(())
}

/// Return the 6 canonical ESP destination paths in the fixed order
/// used by [`build_manifest`]. Sourced from
/// [`crate::direct_install`] so the manifest's closed set and the
/// ESP staging layer can never drift.
pub(crate) fn canonical_esp_paths() -> [&'static str; 6] {
    [
        ESP_DEST_SHIM,
        ESP_DEST_GRUB,
        ESP_DEST_GRUB_CFG_BOOT,
        ESP_DEST_GRUB_CFG_UBUNTU,
        ESP_DEST_KERNEL,
        ESP_DEST_INITRD,
    ]
}

/// sha256 a file at `path` and return the lowercase hex digest.
/// Streaming read — constant memory regardless of file size (initrd
/// is ~90 MB so the naive `fs::read + hash` path would allocate a
/// full copy we don't need).
pub(crate) fn sha256_file(path: &Path) -> Result<String, ManifestError> {
    use sha2::{Digest, Sha256};
    use std::io::Read;

    // 64 KiB scratch buffer, heap-allocated to keep the stack frame
    // small (clippy's `large_stack_arrays` flags `[u8; 64 * 1024]`).
    let mut buf: Vec<u8> = vec![0u8; 64 * 1024];

    let mut f = fs::File::open(path).map_err(|source| ManifestError::Io {
        path: path.display().to_string(),
        source,
    })?;
    let mut hasher = Sha256::new();
    loop {
        let n = f.read(&mut buf).map_err(|source| ManifestError::Io {
            path: path.display().to_string(),
            source,
        })?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Return `size_bytes` for a file on disk. Paired with [`sha256_file`]
/// when building [`EspFileEntry`] — both values come from the local
/// source we're about to stage, not the post-staging ESP content, so
/// the manifest reflects "what we intended to write" which is what
/// the verifier then asserts against the stick.
pub(crate) fn file_size(path: &Path) -> Result<u64, ManifestError> {
    let meta = fs::metadata(path).map_err(|source| ManifestError::Io {
        path: path.display().to_string(),
        source,
    })?;
    Ok(meta.len())
}

/// Compute the 6 [`EspFileEntry`] rows for a manifest by sha256'ing
/// the local staging sources in the fixed canonical order. Returns
/// entries whose `path` is the ESP destination (from
/// [`canonical_esp_paths`]) and whose `sha256` / `size_bytes` come
/// from the local source file that will be mcopy'd to that path.
///
/// Note that `sources.grub_cfg` maps to two ESP paths
/// (`::/EFI/BOOT/grub.cfg` and `::/EFI/ubuntu/grub.cfg`) — both
/// entries hash the same local file, which is what direct-install's
/// `stage_esp` will copy.
pub(crate) fn compute_esp_file_hashes(
    sources: &crate::direct_install::EspStagingSources<'_>,
) -> Result<[EspFileEntry; 6], ManifestError> {
    // Order must match canonical_esp_paths() exactly.
    let pairs: [(&str, &Path); 6] = [
        (ESP_DEST_SHIM, sources.shim),
        (ESP_DEST_GRUB, sources.grub),
        (ESP_DEST_GRUB_CFG_BOOT, sources.grub_cfg),
        (ESP_DEST_GRUB_CFG_UBUNTU, sources.grub_cfg),
        (ESP_DEST_KERNEL, sources.kernel),
        (ESP_DEST_INITRD, sources.combined_initrd),
    ];

    let mut out: Vec<EspFileEntry> = Vec::with_capacity(6);
    for (dest, src) in pairs {
        out.push(EspFileEntry {
            path: dest.to_string(),
            sha256: sha256_file(src)?,
            size_bytes: file_size(src)?,
        });
    }
    out.try_into().map_err(|_| {
        ManifestError::Minisign("internal: esp_files length != 6 after build".to_string())
    })
}

// ---- Phase 3b: GPT + blkid device-identity readers ------------------------
//
// The helpers below compose the `Device` manifest field from the
// runtime GPT + filesystem metadata. They're split into pure argv
// builders + pure parsers + thin subprocess runners so the parser
// logic is unit-testable without root / without a real block device.

/// Build argv for `sgdisk -p` on a disk path.
pub(crate) fn build_sgdisk_p_argv(disk_dev: &str) -> Vec<String> {
    vec!["sgdisk".to_string(), "-p".to_string(), disk_dev.to_string()]
}

/// Build argv for `sgdisk --info=N` on a disk path.
pub(crate) fn build_sgdisk_info_argv(disk_dev: &str, part_num: u32) -> Vec<String> {
    vec![
        "sgdisk".to_string(),
        format!("--info={part_num}"),
        disk_dev.to_string(),
    ]
}

/// Build argv for `blkid -o value -s <key> <part_dev>`.
pub(crate) fn build_blkid_tag_argv(part_dev: &str, tag: &str) -> Vec<String> {
    vec![
        "blkid".to_string(),
        "-o".to_string(),
        "value".to_string(),
        "-s".to_string(),
        tag.to_string(),
        part_dev.to_string(),
    ]
}

/// Parse the disk GUID line out of `sgdisk -p` stdout.
/// Format: `Disk identifier (GUID): ABCD1234-...`
pub(crate) fn parse_disk_guid_from_sgdisk_p(stdout: &str) -> Option<String> {
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("Disk identifier (GUID): ") {
            return Some(rest.trim().to_string());
        }
    }
    None
}

/// Count the partition rows in `sgdisk -p` stdout.
/// The partition table is preceded by a header line
/// `Number  Start (sector)    End (sector)  Size       Code  Name`
/// and each subsequent non-blank line is a partition.
pub(crate) fn parse_partition_count_from_sgdisk_p(stdout: &str) -> u32 {
    let mut in_table = false;
    let mut count: u32 = 0;
    for raw in stdout.lines() {
        let line = raw.trim_start();
        if line.starts_with("Number") && line.contains("Start") && line.contains("End") {
            in_table = true;
            continue;
        }
        if in_table {
            if line.is_empty() {
                break;
            }
            // Partition rows start with a decimal digit (the number).
            if line.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                count += 1;
            }
        }
    }
    count
}

/// Parse `First sector: N` and `Last sector: N` out of `sgdisk --info=X`
/// stdout. Both fields use "N (at HUMAN)" format; we want the raw
/// sector number.
pub(crate) fn parse_first_last_lba_from_sgdisk_info(stdout: &str) -> Option<(u64, u64)> {
    let mut first: Option<u64> = None;
    let mut last: Option<u64> = None;
    for line in stdout.lines() {
        let l = line.trim();
        if let Some(rest) = l.strip_prefix("First sector: ") {
            first = parse_sector_number(rest);
        } else if let Some(rest) = l.strip_prefix("Last sector: ") {
            last = parse_sector_number(rest);
        }
    }
    match (first, last) {
        (Some(f), Some(lv)) => Some((f, lv)),
        _ => None,
    }
}

/// Pull the leading decimal out of a `sgdisk` "N (at HUMAN)" line
/// fragment. Returns `None` if the first token isn't a u64.
fn parse_sector_number(s: &str) -> Option<u64> {
    s.split_whitespace().next().and_then(|t| t.parse().ok())
}

/// Read device identity off a freshly-partitioned stick and return a
/// populated [`Device`] for the manifest writer. The caller passes
/// the disk device path (e.g. `/dev/sda`) and the ESP + data
/// partition device paths (`/dev/sda1`, `/dev/sda2`) — path
/// construction varies by device kind (SCSI vs `NVMe` vs `mmcblk`)
/// and is already handled in `flash.rs`'s partition helpers.
///
/// Shells out to `sudo sgdisk -p`, `sudo sgdisk --info=1`, and
/// `sudo blkid -o value -s {PARTUUID,UUID}` four times. All are
/// read-only operations. Fails closed on any non-zero exit.
pub(crate) fn read_device_identity(
    disk_dev: &Path,
    esp_dev: &Path,
    data_dev: &Path,
) -> Result<Device, ManifestError> {
    let disk_str = disk_dev.display().to_string();

    let p_out = run_capture(&build_sgdisk_p_argv(&disk_str))?;
    let disk_guid = parse_disk_guid_from_sgdisk_p(&p_out).ok_or_else(|| {
        ManifestError::Minisign("sgdisk -p: no Disk identifier (GUID) line".to_string())
    })?;
    let partition_count = parse_partition_count_from_sgdisk_p(&p_out);

    let info1 = run_capture(&build_sgdisk_info_argv(&disk_str, 1))?;
    let (esp_first, esp_last) = parse_first_last_lba_from_sgdisk_info(&info1)
        .ok_or_else(|| ManifestError::Minisign("sgdisk --info=1: missing LBAs".to_string()))?;

    let esp_partuuid = run_capture(&build_blkid_tag_argv(
        &esp_dev.display().to_string(),
        "PARTUUID",
    ))?
    .trim()
    .to_string();
    let esp_fs_uuid = run_capture(&build_blkid_tag_argv(
        &esp_dev.display().to_string(),
        "UUID",
    ))?
    .trim()
    .to_string();
    let data_partuuid = run_capture(&build_blkid_tag_argv(
        &data_dev.display().to_string(),
        "PARTUUID",
    ))?
    .trim()
    .to_string();
    let data_fs_uuid = run_capture(&build_blkid_tag_argv(
        &data_dev.display().to_string(),
        "UUID",
    ))?
    .trim()
    .to_string();

    Ok(Device {
        disk_guid,
        partition_count,
        esp: EspPartition {
            partuuid: esp_partuuid,
            type_guid: TYPE_GUID_ESP.to_string(),
            fs_uuid: esp_fs_uuid,
            first_lba: esp_first,
            last_lba: esp_last,
        },
        data: DataPartition {
            partuuid: data_partuuid,
            type_guid: TYPE_GUID_MSFT_BASIC_DATA.to_string(),
            fs_uuid: data_fs_uuid,
            label: "AEGIS_ISOS".to_string(),
        },
    })
}

/// Run `sudo <argv>` and capture stdout on success. Used by
/// [`read_device_identity`] for the 6 read-only GPT/blkid reads.
fn run_capture(argv: &[String]) -> Result<String, ManifestError> {
    use std::process::Command;
    let out = Command::new("sudo").args(argv).output().map_err(|e| {
        ManifestError::Minisign(format!(
            "{} exec: {e}",
            argv.first().map_or("?", String::as_str)
        ))
    })?;
    if !out.status.success() {
        return Err(ManifestError::Minisign(format!(
            "{} exited {}: {}",
            argv.first().map_or("?", String::as_str),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Helper for the verifier (Phase 3) — returns `true` if every
/// `esp_files` entry's path is one of the 6 canonical paths and no
/// canonical path is missing. Callers layer the sha256 + size checks
/// on top once the actual stick contents are available.
pub(crate) fn esp_files_cover_canonical_set(m: &Manifest) -> bool {
    if m.esp_files.len() != 6 {
        return false;
    }
    let canonical = canonical_esp_paths();
    for c in canonical {
        let c_lower = c.to_ascii_lowercase();
        if !m
            .esp_files
            .iter()
            .any(|e| e.path.to_ascii_lowercase() == c_lower)
        {
            return false;
        }
    }
    true
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn sample_device() -> Device {
        Device {
            disk_guid: "03EFA1BB-4F1F-4213-BC80-252030C43B24".to_string(),
            partition_count: 2,
            esp: EspPartition {
                partuuid: "39C73252-7FD2-4116-B92D-462CA5B8C5C0".to_string(),
                type_guid: TYPE_GUID_ESP.to_string(),
                fs_uuid: "2007-0EF1".to_string(),
                first_lba: 2048,
                last_lba: 821_247,
            },
            data: DataPartition {
                partuuid: "96695E97-5354-498D-AAF0-494857101427".to_string(),
                type_guid: TYPE_GUID_MSFT_BASIC_DATA.to_string(),
                fs_uuid: "EDF7-E497".to_string(),
                label: "AEGIS_ISOS".to_string(),
            },
        }
    }

    fn sample_esp_files() -> [EspFileEntry; 6] {
        let paths = canonical_esp_paths();
        let mut out: Vec<EspFileEntry> = Vec::new();
        for (i, p) in paths.iter().enumerate() {
            out.push(EspFileEntry {
                path: (*p).to_string(),
                sha256: format!("{:064x}", i + 1),
                size_bytes: 1024 * (i as u64 + 1),
            });
        }
        out.try_into().expect("6 entries")
    }

    #[test]
    fn schema_version_is_pinned_to_one() {
        assert_eq!(SCHEMA_VERSION, 1);
    }

    #[test]
    fn build_manifest_sets_closed_set_flag() {
        let m = build_manifest("aegis-boot 0.13.0", 1, sample_device(), sample_esp_files());
        assert!(m.allowed_files_closed_set);
    }

    #[test]
    fn build_manifest_leaves_expected_pcrs_empty_for_pr3() {
        // E6 populates this after TPM PCR selection; PR3 must not
        // pre-commit to a shape the TPM contract hasn't locked.
        let m = build_manifest("aegis-boot 0.13.0", 1, sample_device(), sample_esp_files());
        assert!(m.expected_pcrs.is_empty());
    }

    #[test]
    fn build_manifest_preserves_canonical_esp_path_order() {
        let m = build_manifest("aegis-boot 0.13.0", 1, sample_device(), sample_esp_files());
        let got: Vec<&str> = m.esp_files.iter().map(|e| e.path.as_str()).collect();
        let expected = canonical_esp_paths();
        assert_eq!(got, expected.to_vec());
    }

    #[test]
    fn canonical_esp_paths_match_phase_2b_constants() {
        // Drift guard against a rename / reorder in direct_install.rs
        // that would silently leave the manifest writing to a set of
        // paths the ESP staging layer never produces. The 6 paths and
        // their order are a contract with mkusb.sh (mkusb.sh:186-191)
        // and every consumer of either layer.
        let got = canonical_esp_paths();
        assert_eq!(got[0], "::/EFI/BOOT/BOOTX64.EFI");
        assert_eq!(got[1], "::/EFI/BOOT/grubx64.efi");
        assert_eq!(got[2], "::/EFI/BOOT/grub.cfg");
        assert_eq!(got[3], "::/EFI/ubuntu/grub.cfg");
        assert_eq!(got[4], "::/vmlinuz");
        assert_eq!(got[5], "::/initrd.img");
    }

    #[test]
    fn serialize_manifest_is_deterministic() {
        let m = build_manifest("aegis-boot 0.13.0", 7, sample_device(), sample_esp_files());
        let a = serialize_manifest(&m).unwrap();
        let b = serialize_manifest(&m).unwrap();
        assert_eq!(a, b, "repeated serialization must produce identical bytes");
    }

    #[test]
    fn serialize_manifest_field_order_is_top_level_schema_first() {
        // Whoever reads the first few bytes of a manifest should see
        // schema_version immediately — lets a dumb consumer dispatch
        // on schema version without loading the whole body. The
        // top-level serialization order is struct declaration order
        // which we assert on here.
        let m = build_manifest("aegis-boot 0.13.0", 1, sample_device(), sample_esp_files());
        let body = serialize_manifest(&m).unwrap();
        let text = std::str::from_utf8(&body).unwrap();
        let schema_idx = text.find("\"schema_version\"").unwrap();
        let tool_idx = text.find("\"tool_version\"").unwrap();
        let seq_idx = text.find("\"manifest_sequence\"").unwrap();
        let device_idx = text.find("\"device\"").unwrap();
        let files_idx = text.find("\"esp_files\"").unwrap();
        assert!(schema_idx < tool_idx);
        assert!(tool_idx < seq_idx);
        assert!(seq_idx < device_idx);
        assert!(device_idx < files_idx);
    }

    #[test]
    fn serialize_manifest_rejects_oversized_body() {
        let mut m = build_manifest("aegis-boot 0.13.0", 1, sample_device(), sample_esp_files());
        // Pad tool_version until the body would exceed the cap.
        m.tool_version = "A".repeat(MAX_MANIFEST_BYTES);
        let err = serialize_manifest(&m).expect_err("should exceed cap");
        assert!(
            matches!(err, ManifestError::TooLarge { .. }),
            "expected TooLarge, got {err:?}"
        );
    }

    #[test]
    fn parse_and_validate_roundtrips_canonical_manifest() {
        let m = build_manifest("aegis-boot 0.13.0", 5, sample_device(), sample_esp_files());
        let body = serialize_manifest(&m).unwrap();
        let parsed = parse_and_validate_manifest(&body, 0, SCHEMA_VERSION).unwrap();
        assert_eq!(parsed, m);
    }

    #[test]
    fn parse_and_validate_rejects_rollback() {
        let m = build_manifest("aegis-boot 0.13.0", 3, sample_device(), sample_esp_files());
        let body = serialize_manifest(&m).unwrap();
        let err =
            parse_and_validate_manifest(&body, 5, SCHEMA_VERSION).expect_err("rollback rejected");
        assert!(
            matches!(
                err,
                ManifestError::Rollback {
                    got: 3,
                    last_seen: 5
                }
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_and_validate_accepts_equal_sequence() {
        // Equal is not rollback — a re-read of the same manifest
        // must still verify. Only strictly-less-than is rejected.
        let m = build_manifest("aegis-boot 0.13.0", 5, sample_device(), sample_esp_files());
        let body = serialize_manifest(&m).unwrap();
        parse_and_validate_manifest(&body, 5, SCHEMA_VERSION).unwrap();
    }

    #[test]
    fn parse_and_validate_rejects_forward_incompat_schema() {
        let mut m = build_manifest("aegis-boot 0.13.0", 1, sample_device(), sample_esp_files());
        m.schema_version = 99;
        let body = serialize_manifest(&m).unwrap();
        let err = parse_and_validate_manifest(&body, 0, SCHEMA_VERSION)
            .expect_err("schema_version > max");
        assert!(
            matches!(err, ManifestError::SchemaTooNew { got: 99, max: 1 }),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_and_validate_rejects_oversized_body() {
        let blob = vec![b' '; MAX_MANIFEST_BYTES + 1];
        let err = parse_and_validate_manifest(&blob, 0, SCHEMA_VERSION).expect_err("too large");
        assert!(matches!(err, ManifestError::TooLarge { .. }), "got {err:?}");
    }

    #[test]
    fn esp_files_cover_canonical_set_passes_on_fresh_manifest() {
        let m = build_manifest("aegis-boot 0.13.0", 1, sample_device(), sample_esp_files());
        assert!(esp_files_cover_canonical_set(&m));
    }

    #[test]
    fn esp_files_cover_canonical_set_is_case_insensitive_fat32() {
        // FAT32 is case-insensitive; the verifier must accept a
        // manifest whose stored paths differ only in case from the
        // canonical declaration.
        let mut m = build_manifest("aegis-boot 0.13.0", 1, sample_device(), sample_esp_files());
        for e in &mut m.esp_files {
            e.path = e.path.to_ascii_uppercase();
        }
        assert!(esp_files_cover_canonical_set(&m));
    }

    #[test]
    fn esp_files_cover_canonical_set_rejects_missing_canonical_entry() {
        let mut m = build_manifest("aegis-boot 0.13.0", 1, sample_device(), sample_esp_files());
        // Corrupt one entry so the set no longer covers the canonical
        // path.
        m.esp_files[0].path = "::/EFI/BOOT/IMPOSTOR.EFI".to_string();
        assert!(!esp_files_cover_canonical_set(&m));
    }

    #[test]
    fn esp_files_cover_canonical_set_rejects_wrong_count() {
        let mut m = build_manifest("aegis-boot 0.13.0", 1, sample_device(), sample_esp_files());
        m.esp_files.pop();
        assert!(!esp_files_cover_canonical_set(&m));
    }

    #[test]
    fn write_manifest_and_sig_round_trips_through_disk() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let m_path = tmp.path().join("manifest.json");
        let s_path = tmp.path().join("manifest.json.minisig");

        let body = b"{\"schema_version\":1}";
        let sig = b"untrusted comment: ...\nDUMMY_SIG\n";
        write_manifest_and_sig(body, sig, &m_path, &s_path).expect("write");

        assert_eq!(std::fs::read(&m_path).unwrap(), body);
        assert_eq!(std::fs::read(&s_path).unwrap(), sig);
    }

    #[test]
    fn sign_then_verify_round_trip_passes() {
        let keypair = minisign::KeyPair::generate_unencrypted_keypair().expect("keygen");
        let m = build_manifest("aegis-boot 0.13.0", 1, sample_device(), sample_esp_files());
        let body = serialize_manifest(&m).unwrap();

        let sig = sign_manifest_body(&body, &keypair.sk).expect("sign");
        verify_manifest_body(&body, &sig, &keypair.pk).expect("verify");
    }

    #[test]
    fn verify_rejects_tampered_body() {
        let keypair = minisign::KeyPair::generate_unencrypted_keypair().expect("keygen");
        let m = build_manifest("aegis-boot 0.13.0", 1, sample_device(), sample_esp_files());
        let body = serialize_manifest(&m).unwrap();
        let sig = sign_manifest_body(&body, &keypair.sk).expect("sign");

        // Tamper with the manifest body — flip one byte.
        let mut tampered = body.clone();
        tampered[10] ^= 0x01;
        let err = verify_manifest_body(&tampered, &sig, &keypair.pk).expect_err("should reject");
        assert!(matches!(err, ManifestError::Minisign(_)), "got {err:?}");
    }

    #[test]
    fn verify_rejects_signature_from_different_key() {
        let kp1 = minisign::KeyPair::generate_unencrypted_keypair().expect("keygen 1");
        let kp2 = minisign::KeyPair::generate_unencrypted_keypair().expect("keygen 2");
        let m = build_manifest("aegis-boot 0.13.0", 1, sample_device(), sample_esp_files());
        let body = serialize_manifest(&m).unwrap();
        let sig = sign_manifest_body(&body, &kp1.sk).expect("sign");
        let err = verify_manifest_body(&body, &sig, &kp2.pk).expect_err("wrong key");
        assert!(matches!(err, ManifestError::Minisign(_)), "got {err:?}");
    }

    #[test]
    fn max_manifest_bytes_leaves_comfortable_headroom() {
        // Sanity regression guard: a realistic manifest fits well
        // under the cap, so future additions can land without
        // immediately bumping MAX_MANIFEST_BYTES.
        let m = build_manifest("aegis-boot 0.13.0", 1, sample_device(), sample_esp_files());
        let body = serialize_manifest(&m).unwrap();
        assert!(
            body.len() < MAX_MANIFEST_BYTES / 10,
            "body {} bytes; cap {} — ≥10× headroom expected",
            body.len(),
            MAX_MANIFEST_BYTES
        );
    }

    // ---- Phase 3a: hash + staging-source helpers -------------------

    #[test]
    fn sha256_file_matches_known_digest_for_empty_file() {
        // sha256("") is the RFC-known empty-input hash. Guards
        // against a streaming-read bug where the final hasher state
        // isn't finalized correctly.
        let tmp = tempfile::tempdir().expect("tempdir");
        let p = tmp.path().join("empty.bin");
        std::fs::write(&p, b"").expect("write");
        let got = sha256_file(&p).expect("hash");
        assert_eq!(
            got,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn sha256_file_matches_known_digest_for_abc() {
        // sha256("abc") = ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
        let tmp = tempfile::tempdir().expect("tempdir");
        let p = tmp.path().join("abc.bin");
        std::fs::write(&p, b"abc").expect("write");
        let got = sha256_file(&p).expect("hash");
        assert_eq!(
            got,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn sha256_file_handles_multi_chunk_input() {
        use sha2::{Digest, Sha256};

        // The streaming reader uses a 64 KiB buffer; feed it >64 KiB
        // to make sure the chunk-boundary path works.
        let tmp = tempfile::tempdir().expect("tempdir");
        let p = tmp.path().join("big.bin");
        let body = vec![0xAAu8; (64 * 1024) + 17];
        std::fs::write(&p, body.as_slice()).expect("write");

        // Independent hasher over the same bytes.
        let mut h = Sha256::new();
        h.update(&body);
        let expected = hex::encode(h.finalize());

        let got = sha256_file(&p).expect("hash");
        assert_eq!(got, expected);
    }

    #[test]
    fn sha256_file_rejects_missing_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let p = tmp.path().join("does-not-exist.bin");
        let err = sha256_file(&p).expect_err("should fail");
        assert!(matches!(err, ManifestError::Io { .. }), "got {err:?}");
    }

    #[test]
    fn file_size_reports_byte_length() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let p = tmp.path().join("payload.bin");
        std::fs::write(&p, vec![0u8; 1024]).expect("write");
        let got = file_size(&p).expect("size");
        assert_eq!(got, 1024);
    }

    #[test]
    fn compute_esp_file_hashes_produces_six_entries_in_canonical_order() {
        // Set up 5 distinct source files (kernel, initrd, shim, grub,
        // grub.cfg); grub.cfg is referenced twice in the fixed pairs
        // array because it gets mcopy'd to two ESP destinations.
        let tmp = tempfile::tempdir().expect("tempdir");
        let shim = tmp.path().join("shim.efi");
        let grub = tmp.path().join("grubx64.efi");
        let cfg = tmp.path().join("grub.cfg");
        let kernel = tmp.path().join("vmlinuz");
        let initrd = tmp.path().join("combined-initrd.img");

        std::fs::write(&shim, b"SHIM_BYTES").unwrap();
        std::fs::write(&grub, b"GRUB_BYTES").unwrap();
        std::fs::write(&cfg, b"GRUB_CFG").unwrap();
        std::fs::write(&kernel, b"KERNEL_BYTES").unwrap();
        std::fs::write(&initrd, b"INITRD_BYTES_CONCATENATED").unwrap();

        let sources = crate::direct_install::EspStagingSources {
            shim: &shim,
            grub: &grub,
            kernel: &kernel,
            combined_initrd: &initrd,
            grub_cfg: &cfg,
        };

        let entries = compute_esp_file_hashes(&sources).expect("hash");
        let got_paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
        let expected = canonical_esp_paths();
        assert_eq!(got_paths, expected.to_vec());

        // Both grub.cfg destinations must share sha256 + size — same
        // local source.
        assert_eq!(entries[2].sha256, entries[3].sha256);
        assert_eq!(entries[2].size_bytes, entries[3].size_bytes);

        // Kernel entry: size + non-empty hex digest, 64 chars.
        assert_eq!(entries[4].size_bytes, b"KERNEL_BYTES".len() as u64);
        assert_eq!(entries[4].sha256.len(), 64);
    }

    #[test]
    fn compute_esp_file_hashes_result_round_trips_through_manifest() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("any.bin");
        std::fs::write(&path, b"content").unwrap();

        let sources = crate::direct_install::EspStagingSources {
            shim: &path,
            grub: &path,
            kernel: &path,
            combined_initrd: &path,
            grub_cfg: &path,
        };

        let entries = compute_esp_file_hashes(&sources).unwrap();
        let m = build_manifest("aegis-boot 0.13.0", 1, sample_device(), entries);
        assert!(esp_files_cover_canonical_set(&m));
        let body = serialize_manifest(&m).unwrap();
        let parsed = parse_and_validate_manifest(&body, 0, SCHEMA_VERSION).unwrap();
        assert_eq!(parsed, m);
    }

    #[test]
    fn compute_esp_file_hashes_propagates_missing_source() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let exists = tmp.path().join("a.bin");
        std::fs::write(&exists, b"ok").unwrap();
        let missing = tmp.path().join("gone.bin");

        let sources = crate::direct_install::EspStagingSources {
            shim: &missing,
            grub: &exists,
            kernel: &exists,
            combined_initrd: &exists,
            grub_cfg: &exists,
        };

        let err = compute_esp_file_hashes(&sources).expect_err("should fail");
        assert!(matches!(err, ManifestError::Io { .. }), "got {err:?}");
    }

    // ---- Phase 3b: device-identity argv + parsers -------------------

    #[test]
    fn build_sgdisk_p_argv_has_three_tokens_in_order() {
        let argv = build_sgdisk_p_argv("/dev/sda");
        assert_eq!(argv, vec!["sgdisk", "-p", "/dev/sda"]);
    }

    #[test]
    fn build_sgdisk_info_argv_formats_info_flag_with_partnum() {
        let argv = build_sgdisk_info_argv("/dev/nvme0n1", 2);
        assert_eq!(argv, vec!["sgdisk", "--info=2", "/dev/nvme0n1"]);
    }

    #[test]
    fn build_blkid_tag_argv_emits_value_only_flags() {
        // `-o value -s PARTUUID` is the narrow form we want: single
        // value, no key=value pair, no device-wide dump. Drift here
        // would either break parsing (extra noise) or widen the
        // surface of what blkid returns.
        let argv = build_blkid_tag_argv("/dev/sda1", "PARTUUID");
        assert_eq!(
            argv,
            vec!["blkid", "-o", "value", "-s", "PARTUUID", "/dev/sda1"]
        );
    }

    const SAMPLE_SGDISK_P_OUTPUT: &str = "Disk /dev/sda: 62521344 sectors, 29.8 GiB
Sector size (logical): 512 bytes
Disk identifier (GUID): 03EFA1BB-4F1F-4213-BC80-252030C43B24
Partition table holds up to 128 entries
Main partition table begins at sector 2 and ends at sector 33
First usable sector is 34, last usable sector is 62521310
Partitions will be aligned on 2048-sector boundaries
Total free space is 0 sectors (0 bytes)

Number  Start (sector)    End (sector)  Size       Code  Name
   1            2048          821247   400.0 MiB   EF00  EFI System
   2          821248        62521343   29.4 GiB    0700  AEGIS_ISOS
";

    #[test]
    fn parse_disk_guid_from_sgdisk_p_extracts_guid() {
        let got =
            parse_disk_guid_from_sgdisk_p(SAMPLE_SGDISK_P_OUTPUT).expect("guid present in sample");
        assert_eq!(got, "03EFA1BB-4F1F-4213-BC80-252030C43B24");
    }

    #[test]
    fn parse_disk_guid_from_sgdisk_p_returns_none_on_missing_line() {
        let got = parse_disk_guid_from_sgdisk_p("no identifier line here\n");
        assert!(got.is_none());
    }

    #[test]
    fn parse_partition_count_from_sgdisk_p_counts_rows() {
        let n = parse_partition_count_from_sgdisk_p(SAMPLE_SGDISK_P_OUTPUT);
        assert_eq!(n, 2);
    }

    #[test]
    fn parse_partition_count_from_sgdisk_p_returns_zero_on_empty_table() {
        let stdout = "Disk /dev/sda: ...\n\nNumber  Start (sector)    End (sector)  Size       Code  Name\n\n";
        let n = parse_partition_count_from_sgdisk_p(stdout);
        assert_eq!(n, 0);
    }

    #[test]
    fn parse_partition_count_from_sgdisk_p_flags_rogue_third_partition() {
        // If some other tooling has written a 3rd partition, the
        // count rises — the verifier uses this to reject the stick
        // (device.partition_count=2 in the manifest, actual=3).
        let stdout = "Disk /dev/sda: ...\n\nNumber  Start (sector)    End (sector)  Size       Code  Name\n   1            2048          821247   400.0 MiB   EF00  EFI System\n   2          821248        62521343   29.4 GiB    0700  AEGIS_ISOS\n   3        62521344        62537727   8.0 MiB     0700  ROGUE\n";
        let n = parse_partition_count_from_sgdisk_p(stdout);
        assert_eq!(n, 3);
    }

    const SAMPLE_SGDISK_INFO_1: &str =
        "Partition GUID code: C12A7328-F81F-11D2-BA4B-00A0C93EC93B (EFI System)
Partition unique GUID: 39C73252-7FD2-4116-B92D-462CA5B8C5C0
First sector: 2048 (at 1024.0 KiB)
Last sector: 821247 (at 400.0 MiB)
Partition size: 819200 sectors (400.0 MiB)
Attribute flags: 0000000000000000
Partition name: 'EFI System'
";

    #[test]
    fn parse_first_last_lba_from_sgdisk_info_extracts_both() {
        let (first, last) =
            parse_first_last_lba_from_sgdisk_info(SAMPLE_SGDISK_INFO_1).expect("lbas present");
        assert_eq!(first, 2048);
        assert_eq!(last, 821_247);
    }

    #[test]
    fn parse_first_last_lba_from_sgdisk_info_returns_none_on_partial_output() {
        let partial = "First sector: 2048 (at 1024.0 KiB)\n";
        let got = parse_first_last_lba_from_sgdisk_info(partial);
        assert!(got.is_none(), "should need both lines, got {got:?}");
    }

    #[test]
    fn parse_first_last_lba_from_sgdisk_info_returns_none_on_non_numeric() {
        let bad = "First sector: NOT_A_NUMBER\nLast sector: ALSO_NOT\n";
        let got = parse_first_last_lba_from_sgdisk_info(bad);
        assert!(got.is_none());
    }
}
