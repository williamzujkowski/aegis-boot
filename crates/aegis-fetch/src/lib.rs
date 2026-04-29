// SPDX-License-Identifier: MIT OR Apache-2.0

//! HTTPS download + signed-chain verification for aegis-boot
//! catalog ISOs. See [`fetch_catalog_entry`] for the entry point.
//!
//! ## Why this crate exists (#655 Phase 2B)
//!
//! Both `aegis-cli` (host) and `rescue-tui` (in-rescue) need to
//! download + verify catalog ISOs via the same trust path. Before
//! this crate landed, the host CLI shelled out to `curl` + `gpg` +
//! `sha256sum`; the rescue env couldn't reuse that because the
//! initramfs doesn't ship those binaries. Pulling the verify path
//! into a Rust library that links into both binaries gives us:
//!
//! - One trust implementation, audited once.
//! - Static-musl friendly (rustls, no native TLS, pure-Rust PGP).
//! - Progress callbacks the TUI can render without a child-process
//!   stdout-scraping shim.
//!
//! ## Trust posture
//!
//! The PGP verifier is [rpgp][rpgp] (MIT OR Apache-2.0), pinned via
//! workspace [`Cargo.toml`]. The HTTPS stack is [ureq][ureq] +
//! [rustls][rustls] + [ring][ring] + [webpki-roots][webpki-roots]
//! (Mozilla's CA bundle). Vendor certs live in
//! `crates/aegis-catalog/keyring/<vendor>.asc` with fingerprints
//! pinned in `fingerprints.toml`; this crate refuses to load a cert
//! whose fingerprint disagrees with the pin.
//!
//! No signing, no key generation, no encryption — verify-only.
//!
//! [rpgp]: https://crates.io/crates/pgp
//! [ureq]: https://crates.io/crates/ureq
//! [rustls]: https://crates.io/crates/rustls
//! [ring]: https://crates.io/crates/ring
//! [webpki-roots]: https://crates.io/crates/webpki-roots

#![warn(missing_docs)]

use std::path::{Path, PathBuf};

use aegis_catalog::{Entry, SigPattern, Vendor};

mod download;
mod keyring;
mod sha256;
mod sums;
mod verify;

pub use keyring::VendorKeyring;

/// Streaming progress for the ISO download. Emitted from
/// [`FetchEvent::Downloading`].
#[derive(Debug, Clone, Copy)]
pub struct FetchProgress {
    /// Bytes downloaded so far.
    pub bytes: u64,
    /// Content-Length when the server provided it. `None` for
    /// chunked / streaming responses; UIs should fall back to a
    /// spinner in that case.
    pub total: Option<u64>,
}

/// Lifecycle event emitted by [`fetch_catalog_entry`] via the
/// caller-supplied callback. Downstream UIs (the host CLI's
/// `indicatif` bar, the rescue-tui's progress overlay) translate
/// these into rendering decisions.
#[derive(Debug, Clone)]
pub enum FetchEvent {
    /// TLS handshake is in progress; no bytes received yet.
    Connecting,
    /// ISO bytes are streaming. `bytes` is the running total since
    /// the request started; reset on retry.
    Downloading(FetchProgress),
    /// SHA-256 of the downloaded ISO is being computed and matched
    /// against the (already authenticated) sums file.
    VerifyingHash,
    /// PGP signature is being verified against the pinned vendor
    /// cert. The exact target depends on the entry's
    /// [`aegis_catalog::SigPattern`].
    VerifyingSig,
    /// Terminal event. Carries the verified ISO path and the
    /// fingerprint of the cert that authenticated it.
    Done(FetchOutcome),
}

/// Successful fetch result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchOutcome {
    /// Absolute path to the verified ISO on disk.
    pub iso_path: PathBuf,
    /// ISO size in bytes.
    pub bytes: u64,
    /// Lowercase hex SHA-256 digest of the ISO.
    pub sha256_hex: String,
    /// Vendor whose cert authenticated this fetch.
    pub vendor: Vendor,
    /// Hex fingerprint of the cert that signed the verified
    /// artifact. Useful for audit logs / `aegis-boot doctor`.
    pub key_fingerprint: String,
}

/// All errors [`fetch_catalog_entry`] can raise.
///
/// On any error, callers should treat any partial file at the
/// destination as untrusted: this crate's contract is "the file at
/// `iso_path` is verified iff the call returns `Ok`". A future
/// Phase 3 may change this contract by writing to `<iso>.partial`
/// and renaming on success.
#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    /// HTTPS transport failed (DNS, TLS, non-2xx, timeout, etc.).
    #[error("network: failed to fetch {url}: {detail}")]
    Network {
        /// Upstream URL that the fetch was directed at.
        url: String,
        /// Detail string forwarded from the underlying transport.
        detail: String,
    },
    /// Filesystem operation failed (mkdir, create, write, sync).
    #[error("filesystem: {detail}")]
    Filesystem {
        /// Operator-readable cause description.
        detail: String,
    },
    /// SHA-256 of the downloaded ISO did not match the vendor's
    /// authenticated checksum. Most likely cause: corrupted /
    /// truncated download. Less likely but worth flagging:
    /// MITM substitution that made it past TLS (compromised CA?).
    #[error("sha256: expected {expected}, got {actual} for {iso}")]
    Sha256Mismatch {
        /// Hex digest the vendor's authenticated sums file reports.
        expected: String,
        /// Hex digest of the bytes we downloaded.
        actual: String,
        /// ISO filename we were verifying.
        iso: String,
    },
    /// PGP signature verification failed. Either the artifact was
    /// tampered with or the pinned vendor cert is wrong (key
    /// rotation we haven't picked up yet). Operator-facing
    /// remediation: re-fetch from a different network, or update
    /// to a newer aegis-boot release.
    #[error("signature: verify failed for {entry}: {detail}")]
    SignatureVerifyFailed {
        /// Catalog entry slug being fetched.
        entry: String,
        /// Detail from the PGP verifier.
        detail: String,
    },
    /// Vendor cert for the entry is not in the keyring. Indicates
    /// a missing keyring file in `crates/aegis-catalog/keyring/`,
    /// caught at runtime rather than compile time when the keyring
    /// is loaded via [`VendorKeyring::from_dir`].
    #[error("keyring: no cert for vendor {vendor:?}")]
    UnknownVendor {
        /// The vendor whose cert was missing.
        vendor: Vendor,
    },
    /// Sums file was authenticated successfully but doesn't list
    /// the ISO filename. Vendor mirror layout drift —
    /// `aegis-catalog::Entry::iso_url` is no longer correct.
    #[error("sums: no entry for {iso} in authenticated sums file")]
    IsoNotInSums {
        /// ISO filename we were looking for.
        iso: String,
    },
    /// Sums file parsed but did not appear to be a sha256-format
    /// digest list. Defensive: detects a vendor switching to a
    /// different digest algorithm under our feet.
    #[error("sums: malformed (no sha256 lines found)")]
    MalformedSums,
    /// Cleartext-signed sums file did not contain a recognizable
    /// signature envelope. Returned when an entry tagged
    /// [`aegis_catalog::SigPattern::ClearsignedSums`] fetched a
    /// file that wasn't actually a clearsigned envelope.
    #[error("sums: not a clearsigned envelope")]
    NotClearsigned,
}

/// Download + verify a catalog [`Entry`] end-to-end, writing the
/// authenticated ISO to `dest_dir`. Emits lifecycle events via
/// `on_event` so the caller can render progress.
///
/// On `Ok`, the ISO at `iso_path` is byte-for-byte what the vendor
/// signed. On `Err`, the destination directory may contain partial
/// or unverified files — caller is responsible for cleanup or
/// retry semantics.
///
/// This is a synchronous, blocking call. The caller chooses the
/// thread (rescue-tui spawns a worker; the host CLI calls inline).
///
/// # Errors
///
/// See [`FetchError`] for the failure taxonomy.
pub fn fetch_catalog_entry(
    entry: &Entry,
    dest_dir: &Path,
    keyring: &VendorKeyring,
    on_event: &mut dyn FnMut(FetchEvent),
) -> Result<FetchOutcome, FetchError> {
    let cert_bytes = keyring
        .cert_armor(entry.vendor)
        .ok_or(FetchError::UnknownVendor {
            vendor: entry.vendor,
        })?;

    std::fs::create_dir_all(dest_dir).map_err(|e| FetchError::Filesystem {
        detail: format!("create {}: {e}", dest_dir.display()),
    })?;

    let iso_filename = filename_from_url(entry.iso_url);
    let iso_path = dest_dir.join(iso_filename);
    let bytes = download::download_to_file(entry.iso_url, &iso_path, on_event)?;

    let fingerprint = match entry.verify {
        SigPattern::ClearsignedSums => {
            verify_clearsigned_path(entry, cert_bytes, &iso_path, on_event)?
        }
        SigPattern::DetachedSigOnSums => {
            verify_detached_on_sums_path(entry, cert_bytes, &iso_path, on_event)?
        }
        SigPattern::DetachedSigOnIso => {
            verify_detached_on_iso_path(entry, cert_bytes, &iso_path, on_event)?
        }
    };

    let outcome = FetchOutcome {
        iso_path,
        bytes,
        sha256_hex: fingerprint.iso_sha256_hex,
        vendor: entry.vendor,
        key_fingerprint: fingerprint.signer_fingerprint_hex,
    };
    on_event(FetchEvent::Done(outcome.clone()));
    Ok(outcome)
}

/// Result of a verify dispatch — both the cert fingerprint that
/// authenticated the artifact and the ISO's sha256, since both
/// flow into [`FetchOutcome`].
struct VerifyResult {
    iso_sha256_hex: String,
    signer_fingerprint_hex: String,
}

fn verify_clearsigned_path(
    entry: &Entry,
    cert_bytes: &[u8],
    iso_path: &Path,
    on_event: &mut dyn FnMut(FetchEvent),
) -> Result<VerifyResult, FetchError> {
    // sha256_url == sig_url for ClearsignedSums; one fetch.
    let sums_bytes = download::download_to_vec(entry.sha256_url)?;
    let sums_text =
        std::str::from_utf8(&sums_bytes).map_err(|e| FetchError::SignatureVerifyFailed {
            entry: entry.slug.to_string(),
            detail: format!("clearsigned sums not utf-8: {e}"),
        })?;

    on_event(FetchEvent::VerifyingSig);
    let verified = verify::verify_clearsigned_sums(cert_bytes, sums_text, entry.slug)?;

    on_event(FetchEvent::VerifyingHash);
    let mut last_hash_bytes: u64 = 0;
    let (iso_sha256_hex, _bytes) = sha256::hash_file(iso_path, &mut |b| last_hash_bytes = b)?;
    let _ = last_hash_bytes;

    let iso_filename = filename_from_url(entry.iso_url);
    let expected = sums::find_iso_sha256(&verified.signed_text, iso_filename)?;
    if !iso_sha256_hex.eq_ignore_ascii_case(&expected) {
        return Err(FetchError::Sha256Mismatch {
            expected,
            actual: iso_sha256_hex,
            iso: iso_filename.to_string(),
        });
    }
    Ok(VerifyResult {
        iso_sha256_hex,
        signer_fingerprint_hex: verified.fingerprint_hex,
    })
}

fn verify_detached_on_sums_path(
    entry: &Entry,
    cert_bytes: &[u8],
    iso_path: &Path,
    on_event: &mut dyn FnMut(FetchEvent),
) -> Result<VerifyResult, FetchError> {
    let sums_bytes = download::download_to_vec(entry.sha256_url)?;
    let sig_bytes = download::download_to_vec(entry.sig_url)?;

    on_event(FetchEvent::VerifyingSig);
    let signer_fingerprint_hex =
        verify::verify_detached_sig_on_sums(cert_bytes, &sig_bytes, &sums_bytes, entry.slug)?;

    on_event(FetchEvent::VerifyingHash);
    let (iso_sha256_hex, _bytes) = sha256::hash_file(iso_path, &mut |_| {})?;

    let sums_text = std::str::from_utf8(&sums_bytes).map_err(|_| FetchError::MalformedSums)?;
    let iso_filename = filename_from_url(entry.iso_url);
    let expected = sums::find_iso_sha256(sums_text, iso_filename)?;
    if !iso_sha256_hex.eq_ignore_ascii_case(&expected) {
        return Err(FetchError::Sha256Mismatch {
            expected,
            actual: iso_sha256_hex,
            iso: iso_filename.to_string(),
        });
    }
    Ok(VerifyResult {
        iso_sha256_hex,
        signer_fingerprint_hex,
    })
}

fn verify_detached_on_iso_path(
    entry: &Entry,
    cert_bytes: &[u8],
    iso_path: &Path,
    on_event: &mut dyn FnMut(FetchEvent),
) -> Result<VerifyResult, FetchError> {
    let sig_bytes = download::download_to_vec(entry.sig_url)?;

    on_event(FetchEvent::VerifyingSig);
    let signer_fingerprint_hex =
        verify::verify_detached_sig_on_iso(cert_bytes, &sig_bytes, iso_path, entry.slug)?;

    // The signature already authenticates the ISO bytes; the
    // sha256 here is informational, surfaced in FetchOutcome for
    // audit logs and `aegis-boot doctor` output.
    on_event(FetchEvent::VerifyingHash);
    let (iso_sha256_hex, _bytes) = sha256::hash_file(iso_path, &mut |_| {})?;

    Ok(VerifyResult {
        iso_sha256_hex,
        signer_fingerprint_hex,
    })
}

fn filename_from_url(url: &str) -> &str {
    url.rsplit('/').next().unwrap_or("download")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]
    use super::*;
    use aegis_catalog::CATALOG;

    #[test]
    fn empty_keyring_short_circuits_with_unknown_vendor() {
        // An empty keyring should make fetch_catalog_entry return
        // UnknownVendor before any network or filesystem
        // interaction. This is the production posture until #655
        // PR-B populates the keyring — every fetch must fail
        // cleanly instead of silently bypassing verification.
        let entry = CATALOG
            .iter()
            .find(|e| e.slug == "ubuntu-24.04-live-server")
            .expect("seeded entry");
        let dir = tempfile::tempdir().expect("tempdir");
        let keyring = VendorKeyring::empty();
        let mut events: Vec<FetchEvent> = Vec::new();
        let err = fetch_catalog_entry(entry, dir.path(), &keyring, &mut |e| events.push(e))
            .expect_err("must fail without keyring");
        assert!(matches!(
            err,
            FetchError::UnknownVendor {
                vendor: Vendor::Ubuntu
            }
        ));
        // No events emitted; we short-circuit before Connecting.
        assert!(events.is_empty(), "no events on UnknownVendor path");
    }

    #[test]
    fn filename_from_url_handles_basic_paths() {
        assert_eq!(
            filename_from_url("https://x.example/a/b/file.iso"),
            "file.iso"
        );
        assert_eq!(filename_from_url("https://x.example/file"), "file");
        assert_eq!(filename_from_url("https://x.example/"), "");
    }
}
