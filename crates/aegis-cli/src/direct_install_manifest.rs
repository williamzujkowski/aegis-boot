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

/// Hard cap on manifest body size. The verifier refuses to parse a
/// manifest larger than this — bounds JSON-parser attack surface in
/// the early-boot rescue-tui code path. 64 KiB is ~100× the expected
/// body size so legitimate future growth has room.
pub(crate) const MAX_MANIFEST_BYTES: usize = 64 * 1024;

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
}
