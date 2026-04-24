// SPDX-License-Identifier: MIT OR Apache-2.0

//! Bundle-manifest verify module — ADR 0002 §3.6 + [#417] Phase 2.
//!
//! Composes the two primitives from sibling crates into a single
//! "verify this runtime-downloaded signed chain" entry point:
//!
//! * [`aegis_wire_formats::BundleManifest`] — the data shape.
//! * [`aegis_trust::TrustAnchor::verify_with_epoch`] — the epoch-aware
//!   minisign check.
//!
//! The caller is typically the Phase 3 HTTP downloader: it fetches
//! `bundle-manifest.json` + `.minisig`, calls [`verify_bundle_manifest`],
//! and on success uses the returned [`VerifiedBundleManifest::files`]
//! list to drive per-file downloads (sha256-verified by the same
//! pass). A single minisig thus gates the whole chain.
//!
//! Non-cryptographic validation happens here too: schema-version
//! gating, path-traversal rejection, sha256 shape, non-empty-file-list
//! — things a verifier MUST refuse before touching a flasher. These
//! aren't duplicates of the wire-format's serde constraints; they
//! tighten what `serde::Deserialize` accepts into what a trust
//! boundary accepts.
//!
//! [#417]: https://github.com/aegis-boot/aegis-boot/issues/417

use aegis_trust::{TrustAnchor, TrustAnchorError};
use aegis_wire_formats::{BUNDLE_MANIFEST_SCHEMA_VERSION, BundleFileEntry, BundleManifest};
use thiserror::Error;

/// A bundle manifest that has passed both JSON decode and trust-anchor
/// verify. Callers receive this on success from [`verify_bundle_manifest`];
/// the presence of the struct is itself the attestation that every
/// invariant below held:
///
///   * JSON decoded into a well-formed [`BundleManifest`].
///   * `schema_version` is at or below the one this binary knows
///     (`BUNDLE_MANIFEST_SCHEMA_VERSION`).
///   * Every [`BundleFileEntry::path`] is archive-relative + free of
///     `..` traversal / leading `/` / backslashes.
///   * Every `sha256` is 64 lowercase hex chars.
///   * Every `size_bytes` is `> 0`.
///   * The minisig over the raw bytes validates under the anchor's
///     key for `manifest.key_epoch`, and that epoch clears both the
///     binary's `MIN_REQUIRED_EPOCH` floor and the supplied `seen_epoch`.
///
/// The struct deliberately does NOT carry a reference to the
/// [`aegis_trust::EpochEntry`] — that would tie the returned value's
/// lifetime to the `TrustAnchor`, which is awkward for downstream
/// callers that want to hand the manifest across thread / function
/// boundaries. The verifying epoch is exposed as a plain `u32` via
/// `manifest.key_epoch` instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedBundleManifest {
    /// The fully-parsed manifest.
    pub manifest: BundleManifest,
}

// VerifiedBundleManifest deliberately exposes only the single
// `manifest` field — callers reach through it for `.key_epoch` or
// `.files`. Accessor methods would duplicate that with no
// additional invariant, so we skip them.

/// Reasons a bundle-manifest verify can fail. Each variant carries
/// enough context for an operator-facing message without requiring a
/// second lookup — the downstream CLI formatter reads these directly.
#[derive(Debug, Error)]
pub enum BundleVerifyError {
    /// The manifest body didn't parse as JSON matching [`BundleManifest`].
    /// Includes the `serde_json` error so the operator can tell
    /// "truncated download" apart from "wrong schema."
    #[error("bundle-manifest JSON parse failed: {0}")]
    Parse(#[from] serde_json::Error),

    /// The manifest's `schema_version` is newer than this binary
    /// understands. Forward-incompatible by policy (ADR 0002): an
    /// older binary that meets a newer manifest is a "please upgrade"
    /// signal, not a best-effort decode.
    #[error(
        "bundle-manifest schema_version={saw} exceeds this binary's known version {known}; upgrade aegis-boot"
    )]
    SchemaTooNew {
        /// The `schema_version` the manifest claims.
        saw: u32,
        /// What this binary was built with (`BUNDLE_MANIFEST_SCHEMA_VERSION`).
        known: u32,
    },

    /// `files` was empty. A signed chain must list at least the five
    /// role files it attests to; an empty list is either a test
    /// fixture leak or a tampered download. Either way, refuse.
    #[error("bundle-manifest `files` list is empty — refusing")]
    EmptyFileList,

    /// A file's `path` would let the flasher write outside the archive
    /// root when joined against `origin_url`. Blocks `..`, leading
    /// `/`, backslashes, and embedded NULs.
    #[error("bundle-manifest file[{index}] path {path:?} is not archive-relative")]
    PathTraversal {
        /// Index into `manifest.files` of the offender.
        index: usize,
        /// The offending path — echoed for log-grep.
        path: String,
    },

    /// A file's `sha256` wasn't 64 lowercase hex characters. The wire
    /// format declares hex + lowercase; anything else is either a
    /// signer bug or a post-sign edit.
    #[error("bundle-manifest file[{index}] sha256 {sha256:?} is not 64 lowercase hex characters")]
    BadSha256 {
        /// Index into `manifest.files` of the offender.
        index: usize,
        /// The offending hex string.
        sha256: String,
    },

    /// A file's `size_bytes` was zero. Zero-byte roles don't happen
    /// in a real signed chain; refuse rather than let a blank payload
    /// through (a flasher that skips zero-byte downloads would render
    /// the stick unbootable anyway).
    #[error("bundle-manifest file[{index}] has zero `size_bytes`")]
    ZeroSize {
        /// Index into `manifest.files` of the offender.
        index: usize,
    },

    /// Trust-anchor verify refused the signature. Wraps the underlying
    /// `TrustAnchorError` so the caller can distinguish
    /// `EpochBelowBinaryFloor` from `SignatureInvalid` etc. without
    /// string-matching.
    #[error("trust-anchor verify refused the bundle manifest")]
    Trust(#[from] TrustAnchorError),
}

/// Full verify entry point. Parses, sanity-checks, then runs the
/// epoch-aware minisign check against `anchor`.
///
/// `body` is the exact bytes of `bundle-manifest.json` as fetched;
/// `sig` is the content of `bundle-manifest.json.minisig`. `seen_epoch`
/// is the local install's highest-observed epoch (from
/// `aegis_trust::load_seen_epoch`). `anchor` is typically from
/// `TrustAnchor::load()`.
///
/// On success the caller SHOULD persist the observed epoch via
/// `aegis_trust::store_seen_epoch` so the monotonic floor advances
/// for subsequent verifies.
///
/// # Errors
///
/// See [`BundleVerifyError`] for the full set. The order of checks is:
/// JSON parse → schema version → file-list shape → trust-anchor verify,
/// which means a test-fixture manifest with bogus hashes fails on the
/// cheap shape check before the expensive signature check — helpful
/// when developers iterate on schema shape.
pub fn verify_bundle_manifest(
    body: &[u8],
    sig: &[u8],
    anchor: &TrustAnchor,
    seen_epoch: u32,
) -> Result<VerifiedBundleManifest, BundleVerifyError> {
    let manifest: BundleManifest = serde_json::from_slice(body)?;

    if manifest.schema_version > BUNDLE_MANIFEST_SCHEMA_VERSION {
        return Err(BundleVerifyError::SchemaTooNew {
            saw: manifest.schema_version,
            known: BUNDLE_MANIFEST_SCHEMA_VERSION,
        });
    }

    if manifest.files.is_empty() {
        return Err(BundleVerifyError::EmptyFileList);
    }

    for (index, entry) in manifest.files.iter().enumerate() {
        validate_file_entry(index, entry)?;
    }

    // Signature check last — the trust-anchor walk hits the filesystem
    // (state file) and does a real curve-verify, so cheap shape checks
    // above avoid that round-trip on obviously-bad manifests.
    anchor.verify_with_epoch(body, sig, manifest.key_epoch, seen_epoch)?;

    Ok(VerifiedBundleManifest { manifest })
}

/// Shape-check a single [`BundleFileEntry`] before the signature
/// verify runs. Path rules are deliberately strict — a flasher that
/// wrote to `/boot/shimx64.efi` from a manifest that said
/// `../boot/shimx64.efi` would wipe the host kernel; refuse the class
/// rather than sanitize.
fn validate_file_entry(index: usize, entry: &BundleFileEntry) -> Result<(), BundleVerifyError> {
    let p = &entry.path;
    let bad_path = p.is_empty()
        || p.starts_with('/')
        || p.contains('\0')
        || p.contains('\\')
        || p.contains("..");
    if bad_path {
        return Err(BundleVerifyError::PathTraversal {
            index,
            path: p.clone(),
        });
    }

    if !is_lowercase_hex_64(&entry.sha256) {
        return Err(BundleVerifyError::BadSha256 {
            index,
            sha256: entry.sha256.clone(),
        });
    }

    if entry.size_bytes == 0 {
        return Err(BundleVerifyError::ZeroSize { index });
    }

    Ok(())
}

/// Exactly 64 ASCII chars, each in `0-9` or `a-f`. Manual rather than
/// a regex so this module stays dep-free and the check is trivially
/// auditable.
fn is_lowercase_hex_64(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use aegis_wire_formats::{BundleFileRole, BundleManifest};

    /// Build a well-formed manifest fixture. Each test starts from this
    /// and mutates the one field it's exercising.
    fn sample_manifest() -> BundleManifest {
        BundleManifest {
            schema_version: BUNDLE_MANIFEST_SCHEMA_VERSION,
            key_epoch: 1,
            bundle_version: "0.17.0+bundle.1".to_string(),
            generated_at: "2026-04-24T18:00:00-04:00".to_string(),
            origin_url: "https://example.invalid/bundle/".to_string(),
            files: vec![BundleFileEntry {
                role: BundleFileRole::Shim,
                path: "shim/shimx64.efi".to_string(),
                sha256: "a".repeat(64),
                size_bytes: 1,
            }],
            note: String::new(),
        }
    }

    fn to_bytes(m: &BundleManifest) -> Vec<u8> {
        serde_json::to_vec(m).expect("serialize")
    }

    /// Helper: load an in-workspace anchor. All shape-check tests use
    /// this; the trust-anchor verify always refuses on our dummy sig,
    /// but the shape-check ordering means we reach the trust call only
    /// when we want to.
    fn anchor() -> TrustAnchor {
        TrustAnchor::load().expect("in-workspace anchor must load")
    }

    #[test]
    fn rejects_malformed_json() {
        let err = verify_bundle_manifest(b"{not json}", b"sig", &anchor(), 0).unwrap_err();
        assert!(matches!(err, BundleVerifyError::Parse(_)), "got {err:?}");
    }

    #[test]
    fn rejects_schema_version_too_new() {
        let mut m = sample_manifest();
        m.schema_version = BUNDLE_MANIFEST_SCHEMA_VERSION + 1;
        let err = verify_bundle_manifest(&to_bytes(&m), b"sig", &anchor(), 0).unwrap_err();
        match err {
            BundleVerifyError::SchemaTooNew { saw, known } => {
                assert_eq!(saw, BUNDLE_MANIFEST_SCHEMA_VERSION + 1);
                assert_eq!(known, BUNDLE_MANIFEST_SCHEMA_VERSION);
            }
            other => panic!("wrong error: {other:?}"),
        }
    }

    #[test]
    fn rejects_empty_file_list() {
        let mut m = sample_manifest();
        m.files.clear();
        let err = verify_bundle_manifest(&to_bytes(&m), b"sig", &anchor(), 0).unwrap_err();
        assert!(
            matches!(err, BundleVerifyError::EmptyFileList),
            "got {err:?}"
        );
    }

    #[test]
    fn rejects_path_with_dot_dot() {
        let mut m = sample_manifest();
        m.files[0].path = "../evil.efi".to_string();
        let err = verify_bundle_manifest(&to_bytes(&m), b"sig", &anchor(), 0).unwrap_err();
        match err {
            BundleVerifyError::PathTraversal { index: 0, path } => {
                assert_eq!(path, "../evil.efi");
            }
            other => panic!("wrong error: {other:?}"),
        }
    }

    #[test]
    fn rejects_path_with_leading_slash() {
        let mut m = sample_manifest();
        m.files[0].path = "/etc/passwd".to_string();
        let err = verify_bundle_manifest(&to_bytes(&m), b"sig", &anchor(), 0).unwrap_err();
        assert!(matches!(err, BundleVerifyError::PathTraversal { .. }));
    }

    #[test]
    fn rejects_path_with_backslash() {
        // Windows path separator is easy to overlook in cross-platform
        // code and still gets interpreted as a path on Linux — refuse.
        let mut m = sample_manifest();
        m.files[0].path = "shim\\shimx64.efi".to_string();
        let err = verify_bundle_manifest(&to_bytes(&m), b"sig", &anchor(), 0).unwrap_err();
        assert!(matches!(err, BundleVerifyError::PathTraversal { .. }));
    }

    #[test]
    fn rejects_path_with_embedded_null() {
        let mut m = sample_manifest();
        m.files[0].path = "shim\0shimx64.efi".to_string();
        let err = verify_bundle_manifest(&to_bytes(&m), b"sig", &anchor(), 0).unwrap_err();
        assert!(matches!(err, BundleVerifyError::PathTraversal { .. }));
    }

    #[test]
    fn rejects_empty_path() {
        let mut m = sample_manifest();
        m.files[0].path = String::new();
        let err = verify_bundle_manifest(&to_bytes(&m), b"sig", &anchor(), 0).unwrap_err();
        assert!(matches!(err, BundleVerifyError::PathTraversal { .. }));
    }

    #[test]
    fn rejects_short_sha256() {
        let mut m = sample_manifest();
        m.files[0].sha256 = "a".repeat(63);
        let err = verify_bundle_manifest(&to_bytes(&m), b"sig", &anchor(), 0).unwrap_err();
        assert!(matches!(err, BundleVerifyError::BadSha256 { .. }));
    }

    #[test]
    fn rejects_uppercase_sha256() {
        // Wire-format contract declares lowercase; a signer emitting
        // uppercase is a signer bug, refuse rather than accept-
        // and-canonicalize (which would break sig validation).
        let mut m = sample_manifest();
        m.files[0].sha256 = "A".repeat(64);
        let err = verify_bundle_manifest(&to_bytes(&m), b"sig", &anchor(), 0).unwrap_err();
        assert!(matches!(err, BundleVerifyError::BadSha256 { .. }));
    }

    #[test]
    fn rejects_non_hex_sha256() {
        // 'z' is not a hex digit — refuse.
        let mut m = sample_manifest();
        m.files[0].sha256 = "z".repeat(64);
        let err = verify_bundle_manifest(&to_bytes(&m), b"sig", &anchor(), 0).unwrap_err();
        assert!(matches!(err, BundleVerifyError::BadSha256 { .. }));
    }

    #[test]
    fn rejects_zero_size_bytes() {
        let mut m = sample_manifest();
        m.files[0].size_bytes = 0;
        let err = verify_bundle_manifest(&to_bytes(&m), b"sig", &anchor(), 0).unwrap_err();
        assert!(matches!(err, BundleVerifyError::ZeroSize { index: 0 }));
    }

    #[test]
    fn shape_ok_then_trust_error_for_bad_sig() {
        // All shape checks pass → we reach the signature verify,
        // which reliably refuses on a bogus sig. This exercises the
        // composition: that the shape gate lets the trust gate fire.
        let m = sample_manifest();
        let err =
            verify_bundle_manifest(&to_bytes(&m), b"not a real minisig", &anchor(), 0).unwrap_err();
        assert!(matches!(err, BundleVerifyError::Trust(_)), "got {err:?}");
    }

    #[test]
    fn rejects_epoch_below_seen_floor() {
        // key_epoch=1 in fixture; seen_epoch=5 → epoch-below-seen.
        // Order-of-check: shape passes, then trust check fires
        // before the signature-parse, so a bogus sig still surfaces
        // the floor rejection.
        let m = sample_manifest();
        let err = verify_bundle_manifest(&to_bytes(&m), b"sig", &anchor(), 5).unwrap_err();
        match err {
            BundleVerifyError::Trust(TrustAnchorError::EpochBelowSeenFloor {
                payload_epoch: 1,
                seen_epoch: 5,
            }) => {}
            other => panic!("wrong error: {other:?}"),
        }
    }

    #[test]
    fn is_lowercase_hex_64_accepts_canonical() {
        assert!(is_lowercase_hex_64(&"0".repeat(64)));
        assert!(is_lowercase_hex_64(&"abcdef0123456789".repeat(4)));
    }

    #[test]
    fn is_lowercase_hex_64_rejects_variants() {
        assert!(!is_lowercase_hex_64(""));
        assert!(!is_lowercase_hex_64(&"a".repeat(63)));
        assert!(!is_lowercase_hex_64(&"a".repeat(65)));
        assert!(!is_lowercase_hex_64(&"A".repeat(64)));
        assert!(!is_lowercase_hex_64(&"g".repeat(64)));
        assert!(!is_lowercase_hex_64(&"\0".repeat(64)));
    }

    #[test]
    fn verified_manifest_exposes_manifest_directly() {
        // Callers reach through `.manifest` rather than via separate
        // accessor methods — keeps the API surface minimal.
        let m = sample_manifest();
        let v = VerifiedBundleManifest { manifest: m };
        assert_eq!(v.manifest.files.len(), 1);
        assert_eq!(v.manifest.key_epoch, 1);
    }
}
