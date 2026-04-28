// SPDX-License-Identifier: MIT OR Apache-2.0

//! PGP verification primitives for the three [`SigPattern`] variants.
//!
//! Each function takes vendor cert bytes (armored or binary,
//! sniffed) plus the message bytes and returns the signing key's
//! fingerprint on success. The fingerprint is surfaced in
//! [`crate::FetchOutcome`] for audit logs.
//!
//! [`SigPattern`]: aegis_catalog::SigPattern

// Warnings on `pub(crate)` items in this module are silenced until
// the HTTPS downloader commit lands and `fetch_catalog_entry`
// dispatches into them. The verify primitives are tested
// in-isolation in this module's `#[cfg(test)]` block.
#![allow(dead_code)]

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use pgp::composed::{CleartextSignedMessage, Deserializable, DetachedSignature, SignedPublicKey};

use crate::FetchError;

/// Returned from [`verify_clearsigned_sums`].
#[derive(Debug)]
pub(crate) struct VerifiedClearsign {
    /// The signed plaintext (sums lines, dash-unescaped, with
    /// normalized line endings). Caller parses with
    /// [`crate::sums::find_iso_sha256`].
    pub signed_text: String,
    /// Hex fingerprint (uppercase) of the cert that authenticated
    /// the envelope. Surfaced in [`crate::FetchOutcome`].
    pub fingerprint_hex: String,
}

/// Verify a PGP cleartext-signed envelope (RFC 9580 §7) against
/// the vendor's pinned cert. Used by `AlmaLinux`, Fedora, Rocky.
///
/// `clearsigned_text` is the raw text of the downloaded `CHECKSUM`
/// file (`-----BEGIN PGP SIGNED MESSAGE-----` envelope). On
/// success, the verified plaintext is returned for the caller to
/// scan for the ISO's sha256 line.
///
/// # Errors
///
/// - [`FetchError::NotClearsigned`] when the input has no
///   clearsign envelope.
/// - [`FetchError::SignatureVerifyFailed`] when the signature
///   does not authenticate against the vendor's cert.
pub(crate) fn verify_clearsigned_sums(
    cert_bytes: &[u8],
    clearsigned_text: &str,
    entry_slug: &str,
) -> Result<VerifiedClearsign, FetchError> {
    let cert = parse_cert(cert_bytes, entry_slug)?;
    let (msg, _hdr) = CleartextSignedMessage::from_string(clearsigned_text).map_err(|e| {
        // rpgp surfaces an error variant for "no envelope found".
        // Distinguish it from a verification failure so the
        // operator gets the right remediation hint.
        if format!("{e}").contains("clearsigned")
            || format!("{e}").contains("BEGIN PGP SIGNED")
            || !clearsigned_text.contains("-----BEGIN PGP SIGNED MESSAGE-----")
        {
            FetchError::NotClearsigned
        } else {
            FetchError::SignatureVerifyFailed {
                entry: entry_slug.to_string(),
                detail: format!("parse clearsigned envelope: {e}"),
            }
        }
    })?;
    msg.verify(&cert)
        .map_err(|e| FetchError::SignatureVerifyFailed {
            entry: entry_slug.to_string(),
            detail: format!("clearsign verify: {e}"),
        })?;
    Ok(VerifiedClearsign {
        signed_text: msg.signed_text(),
        fingerprint_hex: fingerprint_hex(&cert),
    })
}

/// Verify a detached PGP signature over a sums file. Used by
/// Debian, Ubuntu, Kali, Linux Mint, `GParted`, openSUSE, Pop!\_OS.
///
/// `sig_bytes` is the body of the `.gpg` / `.sign` / `.asc` file;
/// `sums_bytes` is the body of the SHA256SUMS / SHA512SUMS file.
///
/// # Errors
///
/// [`FetchError::SignatureVerifyFailed`] on parse or verify
/// failure.
pub(crate) fn verify_detached_sig_on_sums(
    cert_bytes: &[u8],
    sig_bytes: &[u8],
    sums_bytes: &[u8],
    entry_slug: &str,
) -> Result<String, FetchError> {
    let cert = parse_cert(cert_bytes, entry_slug)?;
    let sig = parse_detached_sig(sig_bytes, entry_slug)?;
    sig.verify(&cert, sums_bytes)
        .map_err(|e| FetchError::SignatureVerifyFailed {
            entry: entry_slug.to_string(),
            detail: format!("detached-on-sums verify: {e}"),
        })?;
    Ok(fingerprint_hex(&cert))
}

/// Verify a detached PGP signature over the ISO bytes
/// themselves. Used by Alpine, Manjaro, MX Linux, `SystemRescue`.
///
/// Streams the ISO from disk via [`File`] + [`BufReader`] so
/// multi-GB ISOs don't have to fit in memory. Calls
/// `on_progress(bytes_read)` on each 1 MiB chunk so the caller
/// can render a verify-progress UI parallel to the download bar.
///
/// # Errors
///
/// - [`FetchError::Filesystem`] for I/O failures opening or
///   reading the ISO.
/// - [`FetchError::SignatureVerifyFailed`] for parse or verify
///   failure.
pub(crate) fn verify_detached_sig_on_iso(
    cert_bytes: &[u8],
    sig_bytes: &[u8],
    iso_path: &Path,
    entry_slug: &str,
) -> Result<String, FetchError> {
    let cert = parse_cert(cert_bytes, entry_slug)?;
    let sig = parse_detached_sig(sig_bytes, entry_slug)?;
    let file = File::open(iso_path).map_err(|e| FetchError::Filesystem {
        detail: format!("open {} for sig verify: {e}", iso_path.display()),
    })?;
    let reader = BufReader::with_capacity(1 << 20, file);
    sig.signature
        .verify(&cert, reader)
        .map_err(|e| FetchError::SignatureVerifyFailed {
            entry: entry_slug.to_string(),
            detail: format!("detached-on-iso verify: {e}"),
        })?;
    Ok(fingerprint_hex(&cert))
}

/// Parse a [`SignedPublicKey`] from armored or binary cert bytes.
fn parse_cert(bytes: &[u8], entry_slug: &str) -> Result<SignedPublicKey, FetchError> {
    let parsed = if is_armored(bytes) {
        SignedPublicKey::from_armor_single(bytes).map(|(k, _)| k)
    } else {
        SignedPublicKey::from_bytes(BufReader::new(bytes))
    };
    parsed.map_err(|e| FetchError::SignatureVerifyFailed {
        entry: entry_slug.to_string(),
        detail: format!("parse vendor cert: {e}"),
    })
}

/// Parse a [`DetachedSignature`] from armored or binary signature
/// bytes. Sniffs the leading bytes to choose the parser.
fn parse_detached_sig(bytes: &[u8], entry_slug: &str) -> Result<DetachedSignature, FetchError> {
    let parsed = if is_armored(bytes) {
        DetachedSignature::from_armor_single(bytes).map(|(s, _)| s)
    } else {
        DetachedSignature::from_bytes(BufReader::new(bytes))
    };
    parsed.map_err(|e| FetchError::SignatureVerifyFailed {
        entry: entry_slug.to_string(),
        detail: format!("parse detached signature: {e}"),
    })
}

fn is_armored(bytes: &[u8]) -> bool {
    bytes.starts_with(b"-----BEGIN PGP")
}

fn fingerprint_hex(cert: &SignedPublicKey) -> String {
    use pgp::types::KeyDetails;
    format!("{:X}", cert.fingerprint())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]
    use super::*;
    use pgp::composed::{
        ArmorOptions, EncryptionCaps, KeyType, SecretKeyParamsBuilder, SignedSecretKey,
    };
    use pgp::crypto::hash::HashAlgorithm;
    use pgp::ser::Serialize;
    use pgp::types::Password;
    use rand::thread_rng;

    /// Hermetic test cert + signing helper. Generates an Ed25519
    /// primary key (microseconds) so test fixtures don't have to
    /// be checked in. Returns (armored pubkey bytes, signing key).
    fn gen_test_cert() -> (Vec<u8>, SignedSecretKey) {
        let mut rng = thread_rng();
        let mut p = SecretKeyParamsBuilder::default();
        p.key_type(KeyType::Ed25519Legacy)
            .can_certify(true)
            .can_sign(true)
            .can_encrypt(EncryptionCaps::None)
            .primary_user_id("aegis-fetch test <test@example.invalid>".into());
        let params = p.build().expect("build params");
        // params.generate(rng) already returns a self-signed
        // SignedSecretKey — see rpgp/examples/generate_key.rs.
        let secret = params.generate(&mut rng).expect("keygen");
        // Derive the equivalent transferable public key (TPK).
        // SignedSecretKey::public_key() returns just the inner
        // primary public key packet; for the armored cert (with
        // user IDs and binding signatures) we need the From impl.
        let public = SignedPublicKey::from(secret.clone());
        let armored = public
            .to_armored_bytes(ArmorOptions::default())
            .expect("armor");
        (armored, secret)
    }

    fn sign_clearsigned(secret: &SignedSecretKey, text: &str) -> String {
        let mut rng = thread_rng();
        let msg = CleartextSignedMessage::sign(&mut rng, text, &**secret, &Password::empty())
            .expect("sign clearsigned");
        msg.to_armored_string(ArmorOptions::default())
            .expect("armor clearsigned")
    }

    fn sign_detached(secret: &SignedSecretKey, data: &[u8]) -> Vec<u8> {
        let mut rng = thread_rng();
        let sig = DetachedSignature::sign_binary_data(
            &mut rng,
            &**secret,
            &Password::empty(),
            HashAlgorithm::Sha256,
            data,
        )
        .expect("sign detached");
        sig.to_armored_bytes(ArmorOptions::default())
            .expect("armor sig")
    }

    // ---- ClearsignedSums ----------------------------------------

    #[test]
    fn clearsigned_roundtrip_verifies() {
        let (cert, secret) = gen_test_cert();
        let sums = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  alpine.iso\n";
        let clearsigned = sign_clearsigned(&secret, sums);
        let r = verify_clearsigned_sums(&cert, &clearsigned, "alpine-3.20-standard")
            .expect("verify ok");
        // signed_text() normalizes line endings; the sha256 line is preserved.
        assert!(r.signed_text.contains("alpine.iso"));
        assert!(!r.fingerprint_hex.is_empty());
    }

    #[test]
    fn clearsigned_tampered_payload_fails_verify() {
        let (cert, secret) = gen_test_cert();
        let original =
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  alpine.iso\n";
        let clearsigned = sign_clearsigned(&secret, original);
        // Flip a hex digit inside the signed plaintext. rpgp's
        // verify recomputes the digest over the signed text and
        // should reject.
        let tampered = clearsigned.replace("ba78", "ba79");
        let err = verify_clearsigned_sums(&cert, &tampered, "alpine-3.20-standard")
            .expect_err("should fail");
        assert!(matches!(err, FetchError::SignatureVerifyFailed { .. }));
    }

    #[test]
    fn clearsigned_wrong_cert_fails_verify() {
        let (_, secret) = gen_test_cert();
        let (other_cert, _) = gen_test_cert();
        let clearsigned = sign_clearsigned(&secret, "abc  alpine.iso\n");
        let err = verify_clearsigned_sums(&other_cert, &clearsigned, "alpine-3.20-standard")
            .expect_err("should fail");
        assert!(matches!(err, FetchError::SignatureVerifyFailed { .. }));
    }

    #[test]
    fn clearsigned_no_envelope_returns_not_clearsigned() {
        let (cert, _) = gen_test_cert();
        let err = verify_clearsigned_sums(&cert, "just plain text, no envelope", "x")
            .expect_err("should fail");
        assert!(matches!(err, FetchError::NotClearsigned));
    }

    // ---- DetachedSigOnSums --------------------------------------

    #[test]
    fn detached_on_sums_roundtrip_verifies() {
        let (cert, secret) = gen_test_cert();
        let sums = b"ba7816bf  ubuntu.iso\n" as &[u8];
        let sig = sign_detached(&secret, sums);
        let fp = verify_detached_sig_on_sums(&cert, &sig, sums, "ubuntu-24.04-live-server")
            .expect("verify ok");
        assert!(!fp.is_empty());
    }

    #[test]
    fn detached_on_sums_tampered_message_fails_verify() {
        let (cert, secret) = gen_test_cert();
        let sums = b"ba7816bf  ubuntu.iso\n" as &[u8];
        let sig = sign_detached(&secret, sums);
        let tampered = b"ba7816bf  ubuntu.iso\n# extra line\n" as &[u8];
        let err =
            verify_detached_sig_on_sums(&cert, &sig, tampered, "ubuntu").expect_err("should fail");
        assert!(matches!(err, FetchError::SignatureVerifyFailed { .. }));
    }

    // ---- DetachedSigOnIso ---------------------------------------

    #[test]
    fn detached_on_iso_roundtrip_verifies() {
        let (cert, secret) = gen_test_cert();
        // 4 KiB of arbitrary bytes; doesn't have to look like a
        // real ISO — rpgp signs whatever bytes you hand it.
        let iso_bytes: Vec<u8> = (0..4096_u32).map(|i| (i & 0xff) as u8).collect();
        let sig = sign_detached(&secret, &iso_bytes);
        let dir = tempfile::tempdir().expect("tempdir");
        let iso_path = dir.path().join("alpine.iso");
        std::fs::write(&iso_path, &iso_bytes).expect("write");
        let fp = verify_detached_sig_on_iso(&cert, &sig, &iso_path, "alpine-3.20-standard")
            .expect("verify ok");
        assert!(!fp.is_empty());
    }

    #[test]
    fn detached_on_iso_tampered_bytes_fails_verify() {
        let (cert, secret) = gen_test_cert();
        let iso_bytes: Vec<u8> = (0..4096_u32).map(|i| (i & 0xff) as u8).collect();
        let sig = sign_detached(&secret, &iso_bytes);
        let dir = tempfile::tempdir().expect("tempdir");
        let iso_path = dir.path().join("alpine.iso");
        // Write a different set of bytes — sig should reject.
        let tampered: Vec<u8> = (0..4096_u32).map(|i| ((i + 1) & 0xff) as u8).collect();
        std::fs::write(&iso_path, &tampered).expect("write");
        let err = verify_detached_sig_on_iso(&cert, &sig, &iso_path, "alpine-3.20-standard")
            .expect_err("should fail");
        assert!(matches!(err, FetchError::SignatureVerifyFailed { .. }));
    }

    #[test]
    fn detached_on_iso_missing_file_is_filesystem_error() {
        let (cert, secret) = gen_test_cert();
        let sig = sign_detached(&secret, b"data");
        let err = verify_detached_sig_on_iso(
            &cert,
            &sig,
            std::path::Path::new("/nonexistent/aegis-fetch-test"),
            "x",
        )
        .expect_err("should fail");
        assert!(matches!(err, FetchError::Filesystem { .. }));
    }

    // ---- Cert parser sniffing -----------------------------------

    #[test]
    fn parses_armored_cert() {
        let (armored, _) = gen_test_cert();
        assert!(armored.starts_with(b"-----BEGIN PGP"));
        let _ = parse_cert(&armored, "x").expect("parse armored");
    }

    #[test]
    fn parses_binary_cert() {
        let (armored, _) = gen_test_cert();
        // Re-serialize as binary (no armor) using rpgp's writer.
        // Roundtrip via from_armor → to_writer.
        let (cert, _) = SignedPublicKey::from_armor_single(&armored[..]).expect("parse");
        let mut binary = Vec::new();
        cert.to_writer(&mut binary).expect("to_writer");
        let _ = parse_cert(&binary, "x").expect("parse binary");
    }
}
