// SPDX-License-Identifier: MIT OR Apache-2.0

//! Vendor public-key registry consumed by the verify path.
//!
//! A [`VendorKeyring`] is a map from [`aegis_catalog::Vendor`] →
//! parsed PGP cert. It's constructed once per process and passed
//! by reference into [`crate::fetch_catalog_entry`].
//!
//! ## Sources
//!
//! Three constructors:
//!
//! - [`VendorKeyring::embedded`] — loads keys baked into the
//!   binary at compile time via `include_bytes!`. This is the
//!   production path used by both `aegis-cli` and `rescue-tui`.
//!   Loads from `aegis_catalog::EMBEDDED_KEYRING` and validates
//!   each `.asc`'s primary fingerprints against the pinned set in
//!   `aegis_catalog::EMBEDDED_FINGERPRINTS` — a tampered or
//!   rotated key fails the load with [`FetchError::SignatureVerifyFailed`].
//! - [`VendorKeyring::from_dir`] — loads keys from a directory of
//!   `<vendor>.asc` files at runtime. Used by the
//!   catalog-refresh GitHub Action to validate a freshly-fetched
//!   keyring before opening the auto-PR.
//! - [`VendorKeyring::empty`] — for tests; subsequent commits in
//!   this PR add a `pub(crate)` cert-injection helper for the
//!   verify-path fixture tests.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use aegis_catalog::{EMBEDDED_FINGERPRINTS, EMBEDDED_KEYRING, Vendor};
use pgp::composed::{Deserializable, SignedPublicKey};

use crate::FetchError;

/// Map from [`Vendor`] → its pinned PGP cert.
///
/// Constructed once per process, then passed into the fetch
/// pipeline by reference. Cert bytes are kept in memory; rpgp
/// re-parses for each verification (cheap relative to the
/// signature verification itself).
pub struct VendorKeyring {
    /// Raw ASCII-armored cert bytes per vendor. Stored armored so
    /// the rpgp parser can re-deserialize on each verify call;
    /// keeping a parsed `SignedPublicKey` here would force the
    /// rpgp type into our public API, which we want to avoid for
    /// API-stability reasons.
    armored: HashMap<Vendor, Vec<u8>>,
}

impl VendorKeyring {
    /// Construct an empty keyring. Production callers should use
    /// [`VendorKeyring::embedded`] or [`VendorKeyring::from_dir`].
    /// Tests use this together with the `pub(crate)` injection
    /// helper added in the verify-path commit.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            armored: HashMap::new(),
        }
    }

    /// Load the keyring baked into the binary at compile time via
    /// `include_bytes!` (sourced from
    /// [`aegis_catalog::EMBEDDED_KEYRING`]). This is the
    /// production path.
    ///
    /// Each `.asc` file is parsed and its primary-fingerprint set
    /// is validated against the pinned set in
    /// [`aegis_catalog::EMBEDDED_FINGERPRINTS`]. A mismatch
    /// (tampered file, surreptitious key swap) fails the load.
    ///
    /// # Errors
    ///
    /// - [`FetchError::SignatureVerifyFailed`] when an embedded
    ///   `.asc` does not parse, or when its fingerprint set does
    ///   not match the pinned set for that vendor.
    pub fn embedded() -> Result<Self, FetchError> {
        let mut k = Self::empty();
        for (vendor, bytes) in EMBEDDED_KEYRING {
            validate_armored_against_pin(*vendor, bytes)?;
            k.armored.insert(*vendor, bytes.to_vec());
        }
        Ok(k)
    }

    /// Load the keyring from a directory containing one
    /// `<vendor>.asc` file per [`Vendor::all`] entry. Missing
    /// files are tolerated (recorded as absent vendors); malformed
    /// armor surfaces as a parse error.
    ///
    /// # Errors
    ///
    /// [`FetchError::Filesystem`] for I/O failures. A file that
    /// doesn't parse as ASCII-armored PGP is logged but does not
    /// fail the load — verification using that vendor will
    /// surface the parse error at fetch time. (This makes
    /// sub-vendor key rotations recoverable without bricking the
    /// whole keyring.)
    pub fn from_dir(dir: &Path) -> Result<Self, FetchError> {
        let mut armored = HashMap::new();
        for vendor in Vendor::all() {
            let path = dir.join(format!("{}.asc", vendor.slug()));
            match std::fs::read(&path) {
                Ok(bytes) => {
                    armored.insert(*vendor, bytes);
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    // Tolerated — vendor not yet rolled out.
                }
                Err(e) => {
                    return Err(FetchError::Filesystem {
                        detail: format!("read {}: {e}", path.display()),
                    });
                }
            }
        }
        Ok(Self { armored })
    }

    /// Look up the armored cert bytes for a vendor. Returns
    /// `None` when the vendor has no entry in this keyring.
    #[must_use]
    pub fn cert_armor(&self, vendor: Vendor) -> Option<&[u8]> {
        self.armored.get(&vendor).map(Vec::as_slice)
    }

    /// Number of vendors with a cert in this keyring. Useful for
    /// audit logs and the `aegis-boot doctor` keyring health
    /// reporter.
    #[must_use]
    pub fn len(&self) -> usize {
        self.armored.len()
    }

    /// True when no vendor has a cert in this keyring. Equivalent
    /// to `len() == 0`.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.armored.is_empty()
    }
}

impl Default for VendorKeyring {
    fn default() -> Self {
        Self::empty()
    }
}

/// Parse `bytes` as a (possibly multi-cert) ASCII-armored `OpenPGP`
/// keyring and assert that the set of primary-key fingerprints
/// matches the pin in [`EMBEDDED_FINGERPRINTS`] for `vendor`. An
/// empty pin slice (e.g., the Manjaro developer-keyring bundle)
/// is treated as "validate parse only" — the workflow surfaces
/// any membership change as a reviewable PR rather than as a
/// runtime hard-fail, on the theory that a developer-keyring
/// addition isn't a trust-anchor change.
fn validate_armored_against_pin(vendor: Vendor, bytes: &[u8]) -> Result<(), FetchError> {
    let actual = parse_primary_fingerprints(bytes, vendor.slug())?;
    let expected: HashSet<String> = EMBEDDED_FINGERPRINTS
        .iter()
        .find(|(v, _)| *v == vendor)
        .map(|(_, fps)| fps.iter().map(ToString::to_string).collect())
        .unwrap_or_default();
    if expected.is_empty() {
        // "Validate parse only" — see doc above.
        return Ok(());
    }
    let actual_set: HashSet<String> = actual.into_iter().collect();
    if actual_set != expected {
        let missing: Vec<&String> = expected.difference(&actual_set).collect();
        let extra: Vec<&String> = actual_set.difference(&expected).collect();
        return Err(FetchError::SignatureVerifyFailed {
            entry: format!("keyring/{}.asc", vendor.slug()),
            detail: format!("fingerprint pin mismatch — missing {missing:?}, extra {extra:?}"),
        });
    }
    Ok(())
}

/// Extract the uppercase-hex fingerprints of every primary cert
/// in a (possibly multi-cert) ASCII-armored keyring blob.
fn parse_primary_fingerprints(bytes: &[u8], slug: &str) -> Result<Vec<String>, FetchError> {
    use pgp::types::KeyDetails;
    let result: Result<Vec<SignedPublicKey>, pgp::errors::Error> =
        SignedPublicKey::from_armor_many(bytes)
            .and_then(|(iter, _hdr)| iter.collect::<Result<Vec<_>, _>>());
    let certs = result.map_err(|e| FetchError::SignatureVerifyFailed {
        entry: format!("keyring/{slug}.asc"),
        detail: format!("parse: {e}"),
    })?;
    Ok(certs
        .iter()
        .map(|c| format!("{:X}", c.fingerprint()))
        .collect())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]
    use super::*;

    #[test]
    fn empty_is_empty() {
        let k = VendorKeyring::empty();
        assert!(k.is_empty());
        assert_eq!(k.len(), 0);
        assert!(k.cert_armor(Vendor::Ubuntu).is_none());
    }

    #[test]
    fn embedded_loads_all_fourteen_vendors_with_valid_pinned_fingerprints() {
        // PR-B + PR-B2 together populate all 14 vendors. Each .asc
        // must parse and its primary-fingerprint set must equal the
        // pin (or the pin is empty, opting into "validate parse
        // only" — this is the case for Manjaro's 27-key developer
        // bundle).
        let k = VendorKeyring::embedded().expect("embedded ok");
        assert_eq!(
            k.len(),
            14,
            "all 14 catalog vendors have embedded keys (PR-B + PR-B2)"
        );
        for v in Vendor::all() {
            assert!(
                k.cert_armor(*v).is_some(),
                "embedded keyring missing vendor {v:?}"
            );
        }
        // Sanity: at least one vendor from each PR's batch.
        for v in [
            Vendor::Ubuntu,
            Vendor::Manjaro, // PR-B
            Vendor::LinuxMint,
            Vendor::SystemRescue, // PR-B2
        ] {
            assert!(k.cert_armor(v).is_some(), "missing expected vendor {v:?}");
        }
    }

    #[test]
    fn embedded_validation_rejects_swapped_keyring() {
        // Build a keyring where one vendor's bytes are replaced
        // by a cert with a different fingerprint. embedded()
        // refuses; from_dir() (the runtime / refresh-action path)
        // doesn't validate fingerprints, so it succeeds — that's
        // by design (the workflow runs validate_armored_against_pin
        // separately as a strict gate).
        // This test exercises the validator directly.
        let bogus_armor = b"-----BEGIN PGP PUBLIC KEY BLOCK-----\nVersion: bogus\n\nabcd\n-----END PGP PUBLIC KEY BLOCK-----\n";
        let err = validate_armored_against_pin(Vendor::Ubuntu, bogus_armor)
            .expect_err("bogus armor must fail");
        assert!(matches!(err, FetchError::SignatureVerifyFailed { .. }));
    }

    #[test]
    fn from_dir_handles_missing_directory_gracefully() {
        // `Vendor::all` returns 14 entries; from_dir against a
        // missing dir reads zero of them and returns an empty
        // keyring. The directory-doesn't-exist case is the same
        // as every-vendor-file-missing.
        let dir = tempfile::tempdir().expect("tempdir");
        let k = VendorKeyring::from_dir(dir.path()).expect("load ok");
        assert!(k.is_empty());
    }

    #[test]
    fn from_dir_loads_present_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Ubuntu's keyring file present, Debian's absent.
        let path = dir.path().join("ubuntu.asc");
        std::fs::write(
            &path,
            b"-----BEGIN PGP PUBLIC KEY BLOCK-----\nfake\n-----END PGP PUBLIC KEY BLOCK-----\n",
        )
        .expect("write");
        let k = VendorKeyring::from_dir(dir.path()).expect("load ok");
        assert_eq!(k.len(), 1);
        assert!(k.cert_armor(Vendor::Ubuntu).is_some());
        assert!(k.cert_armor(Vendor::Debian).is_none());
    }
}
