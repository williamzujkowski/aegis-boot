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
//!   Empty stub until #655 PR-B populates the keyring.
//! - [`VendorKeyring::from_dir`] — loads keys from a directory of
//!   `<vendor>.asc` files at runtime. Used by the
//!   catalog-refresh GitHub Action to validate a freshly-fetched
//!   keyring before opening the auto-PR.
//! - [`VendorKeyring::empty`] — for tests; subsequent commits in
//!   this PR add a `pub(crate)` cert-injection helper for the
//!   verify-path fixture tests.

use std::collections::HashMap;
use std::path::Path;

use aegis_catalog::Vendor;

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
    /// `include_bytes!`. This is the production path.
    ///
    /// Until #655 PR-B populates `crates/aegis-catalog/keyring/`,
    /// this returns an empty keyring — meaning every fetch in this
    /// build will fail with [`FetchError::UnknownVendor`]. The
    /// scaffold exists so the public API can be reviewed
    /// independently of the keyring rollout.
    ///
    /// # Errors
    ///
    /// Returns [`FetchError::Filesystem`] only when an embedded
    /// keyring exists but is malformed. The empty case is
    /// `Ok(Self::empty())`.
    pub fn embedded() -> Result<Self, FetchError> {
        // PR-B will replace this with a static array of
        // (Vendor, &'static [u8]) pairs from include_bytes!.
        Ok(Self::empty())
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
    fn embedded_returns_empty_until_pr_b() {
        let k = VendorKeyring::embedded().expect("embedded ok");
        assert!(k.is_empty(), "PR-B will populate this");
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
