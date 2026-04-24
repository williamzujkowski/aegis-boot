// SPDX-License-Identifier: MIT OR Apache-2.0

//! Build-time-embedded trust anchor + runtime verify.
//!
//! The anchor is NOT read from disk at runtime — both the epoch
//! history (`historical-anchors.json`) and each epoch's pubkey
//! (`maintainer-epoch-<N>.pub`) are baked in via `include_str!` at
//! build time. That means:
//!
//!   * The binary ships a self-contained trust chain that cannot be
//!     silently swapped by a filesystem attacker.
//!   * Rotating keys (new epoch) requires a new binary release —
//!     exactly the property ADR 0002 §3.4 demands.
//!   * The `keys/` directory on disk is the maintainer-facing source
//!     of truth, consumed by `build.rs`; operators don't need it.

use minisign::{PublicKeyBox, SignatureBox};
use serde::Deserialize;

use crate::errors::TrustAnchorError;

/// One epoch's entry from `keys/historical-anchors.json`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct EpochEntry {
    /// Monotonic u32. Epoch 1 is the initial maintainer-held key.
    pub epoch: u32,
    /// Path the pubkey lives at in the repo. Informational — the
    /// crate doesn't resolve it at runtime (pubkey bytes are
    /// `include_str!`-embedded instead).
    pub pubkey_file: String,
    /// The pubkey's base64 line (second line of a minisign `.pub` file).
    /// This is the canonical on-wire identifier.
    pub pubkey_fingerprint: String,
    /// RFC3339 timestamp of when this epoch was declared valid.
    pub valid_from: String,
    /// RFC3339 timestamp of expiry, or `null` for "valid until
    /// superseded by a later epoch."
    pub expires_at: Option<String>,
    /// Free-text maintainer note.
    #[serde(default)]
    pub note: String,
}

/// The built-in historical-anchors list, embedded at build time.
/// `AEGIS_KEYS_DIR` points at the workspace's `keys/` directory.
const EMBEDDED_ANCHORS_JSON: &str =
    include_str!(concat!(env!("AEGIS_KEYS_DIR"), "/historical-anchors.json"));

/// Epoch-1 pubkey, the only one for the initial release. When a
/// new epoch is minted, add a corresponding `include_str!` line
/// below and a matching branch in [`TrustAnchor::pubkey_for_epoch`].
/// This is deliberately NOT loaded dynamically — every epoch must
/// be a reviewed, committed change to this file.
const EMBEDDED_EPOCH_1_PUB: &str =
    include_str!(concat!(env!("AEGIS_KEYS_DIR"), "/maintainer-epoch-1.pub"));

/// Runtime trust anchor. Holds the parsed epoch history + a view of
/// the binary's `MIN_REQUIRED_EPOCH`.
///
/// Cheap to construct: no I/O, no network. [`TrustAnchor::load`]
/// exists as a single-call setup for callers that want to handle
/// the (rare) embedded-JSON-parse-failure case.
#[derive(Debug, Clone)]
pub struct TrustAnchor {
    epochs: Vec<EpochEntry>,
    min_required: u32,
}

impl TrustAnchor {
    /// Parse the build-time-embedded anchor list.
    ///
    /// # Errors
    ///
    /// - [`TrustAnchorError::UnsafeDefaultEpoch`] if the binary was
    ///   compiled without a discoverable `keys/canonical-epoch.json`
    ///   (sentinel value `MIN_REQUIRED_EPOCH == 0`).
    /// - [`TrustAnchorError::AnchorsParseFailure`] if the embedded
    ///   `historical-anchors.json` couldn't deserialize.
    pub fn load() -> Result<Self, TrustAnchorError> {
        Self::load_with_floor(crate::MIN_REQUIRED_EPOCH)
    }

    /// Testing/injection hook: build a `TrustAnchor` with an explicit
    /// `min_required` floor instead of the binary-embedded constant.
    /// Production callers should use [`TrustAnchor::load`].
    ///
    /// # Errors
    ///
    /// Same as [`TrustAnchor::load`].
    pub fn load_with_floor(min_required: u32) -> Result<Self, TrustAnchorError> {
        if min_required == 0 {
            return Err(TrustAnchorError::UnsafeDefaultEpoch);
        }
        let epochs: Vec<EpochEntry> = serde_json::from_str(EMBEDDED_ANCHORS_JSON)
            .map_err(|e| TrustAnchorError::AnchorsParseFailure(e.to_string()))?;
        Ok(Self {
            epochs,
            min_required,
        })
    }

    /// Returns the binary-embedded `MIN_REQUIRED_EPOCH` this anchor
    /// was loaded with. Exposed so doctor-style surfaces can show it
    /// without reaching through to the crate-level constant.
    #[must_use]
    pub fn min_required(&self) -> u32 {
        self.min_required
    }

    /// Read-only view of the embedded epoch history.
    #[must_use]
    pub fn epochs(&self) -> &[EpochEntry] {
        &self.epochs
    }

    /// Lookup a specific epoch's metadata.
    #[must_use]
    pub fn epoch(&self, epoch: u32) -> Option<&EpochEntry> {
        self.epochs.iter().find(|e| e.epoch == epoch)
    }

    /// Resolve the minisign pubkey bytes for the given epoch.
    ///
    /// Currently only epoch 1 is hard-wired (the initial release);
    /// adding epoch 2 means adding a second `include_str!` branch
    /// below plus the corresponding `keys/maintainer-epoch-2.pub`
    /// file. That edit is the reviewed moment ADR 0002 gates
    /// rotation on — no dynamic lookup by design.
    fn pubkey_for_epoch(epoch: u32) -> Option<&'static str> {
        match epoch {
            1 => Some(EMBEDDED_EPOCH_1_PUB),
            _ => None,
        }
    }

    /// Core verify. Runs the full rotation-aware check:
    ///
    ///   1. `payload_epoch >= self.min_required` (binary floor).
    ///   2. `payload_epoch >= seen_epoch` (local install floor).
    ///   3. `payload_epoch` is registered in the embedded anchor list.
    ///   4. The embedded pubkey for that epoch validates `sig` over `body`.
    ///
    /// Returns the matched [`EpochEntry`] on success so callers can
    /// log the epoch they trusted (useful in audit trails).
    ///
    /// # Errors
    ///
    /// One of [`TrustAnchorError::EpochBelowBinaryFloor`],
    /// [`TrustAnchorError::EpochBelowSeenFloor`],
    /// [`TrustAnchorError::UnknownEpoch`],
    /// [`TrustAnchorError::PubkeyParseFailure`], or
    /// [`TrustAnchorError::SignatureInvalid`] — distinct variants
    /// let callers render distinct operator messages.
    pub fn verify_with_epoch(
        &self,
        body: &[u8],
        sig: &[u8],
        payload_epoch: u32,
        seen_epoch: u32,
    ) -> Result<&EpochEntry, TrustAnchorError> {
        if payload_epoch < self.min_required {
            return Err(TrustAnchorError::EpochBelowBinaryFloor {
                payload_epoch,
                required: self.min_required,
            });
        }
        if payload_epoch < seen_epoch {
            return Err(TrustAnchorError::EpochBelowSeenFloor {
                payload_epoch,
                seen_epoch,
            });
        }

        let entry = self
            .epoch(payload_epoch)
            .ok_or(TrustAnchorError::UnknownEpoch { payload_epoch })?;

        let pubkey_text = Self::pubkey_for_epoch(payload_epoch)
            .ok_or(TrustAnchorError::UnknownEpoch { payload_epoch })?;
        let pk_box = PublicKeyBox::from_string(pubkey_text).map_err(|e| {
            TrustAnchorError::PubkeyParseFailure {
                epoch: payload_epoch,
                detail: e.to_string(),
            }
        })?;
        let pk = pk_box
            .into_public_key()
            .map_err(|e| TrustAnchorError::PubkeyParseFailure {
                epoch: payload_epoch,
                detail: e.to_string(),
            })?;

        let sig_box = SignatureBox::from_string(std::str::from_utf8(sig).map_err(|e| {
            TrustAnchorError::SignatureInvalid {
                payload_epoch,
                detail: format!("signature bytes are not UTF-8: {e}"),
            }
        })?)
        .map_err(|e| TrustAnchorError::SignatureInvalid {
            payload_epoch,
            detail: e.to_string(),
        })?;

        let mut cursor = std::io::Cursor::new(body);
        minisign::verify(&pk, &sig_box, &mut cursor, true, false, false).map_err(|e| {
            TrustAnchorError::SignatureInvalid {
                payload_epoch,
                detail: e.to_string(),
            }
        })?;

        Ok(entry)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn embedded_anchors_parse() {
        let anchor = TrustAnchor::load_with_floor(1).unwrap();
        assert!(
            !anchor.epochs().is_empty(),
            "need at least epoch 1 registered"
        );
        let e1 = anchor.epoch(1).expect("epoch 1 must exist");
        assert_eq!(e1.epoch, 1);
        assert!(
            e1.pubkey_fingerprint.starts_with("RWS"),
            "minisign fingerprints start with RWS, got {:?}",
            e1.pubkey_fingerprint
        );
    }

    #[test]
    fn unsafe_default_epoch_rejected_at_load() {
        let err = TrustAnchor::load_with_floor(0).unwrap_err();
        assert_eq!(err, TrustAnchorError::UnsafeDefaultEpoch);
    }

    #[test]
    fn verify_refuses_payload_below_binary_floor() {
        // Pin floor at 5; payload claims epoch 1 → below floor.
        let anchor = TrustAnchor::load_with_floor(5).unwrap();
        let err = anchor
            .verify_with_epoch(b"body", b"sig-does-not-matter", 1, 0)
            .unwrap_err();
        match err {
            TrustAnchorError::EpochBelowBinaryFloor {
                payload_epoch: 1,
                required: 5,
            } => {}
            other => panic!("wrong error: {other:?}"),
        }
    }

    #[test]
    fn verify_refuses_payload_below_seen_floor() {
        // Binary floor 1, but local seen_epoch has advanced to 7.
        // Payload claims epoch 3 → below seen.
        let anchor = TrustAnchor::load_with_floor(1).unwrap();
        let err = anchor.verify_with_epoch(b"body", b"sig", 3, 7).unwrap_err();
        match err {
            TrustAnchorError::EpochBelowSeenFloor {
                payload_epoch: 3,
                seen_epoch: 7,
            } => {}
            other => panic!("wrong error: {other:?}"),
        }
    }

    #[test]
    fn verify_refuses_unknown_epoch() {
        // Only epoch 1 is registered. Payload claims epoch 9.
        let anchor = TrustAnchor::load_with_floor(1).unwrap();
        let err = anchor.verify_with_epoch(b"body", b"sig", 9, 0).unwrap_err();
        match err {
            TrustAnchorError::UnknownEpoch { payload_epoch: 9 } => {}
            other => panic!("wrong error: {other:?}"),
        }
    }

    #[test]
    fn verify_refuses_malformed_signature() {
        let anchor = TrustAnchor::load_with_floor(1).unwrap();
        let err = anchor
            .verify_with_epoch(b"body", b"not a valid minisig", 1, 0)
            .unwrap_err();
        match err {
            TrustAnchorError::SignatureInvalid {
                payload_epoch: 1, ..
            } => {}
            other => panic!("wrong error: {other:?}"),
        }
    }

    /// End-to-end: sign a body with the committed epoch-1 secret
    /// key, verify it through the `TrustAnchor`. Uses the actual
    /// in-repo key material, so this test is the closest thing to
    /// the production flow we can do without `pass show`.
    ///
    /// Gated on `AEGIS_TRUST_E2E_SECRET_KEY` (a path to the decrypted
    /// secret-key file) + `AEGIS_TRUST_E2E_PASSPHRASE` (the minisign
    /// passphrase). CI doesn't set these, so the test auto-skips;
    /// the maintainer runs it via `pass show ... | tee` pipeline.
    #[test]
    #[ignore = "requires maintainer key material via AEGIS_TRUST_E2E_SECRET_KEY + AEGIS_TRUST_E2E_PASSPHRASE"]
    fn verify_roundtrip_against_committed_key() {
        use std::env;
        use std::fs;
        let (Ok(key_path), Ok(passphrase)) = (
            env::var("AEGIS_TRUST_E2E_SECRET_KEY"),
            env::var("AEGIS_TRUST_E2E_PASSPHRASE"),
        ) else {
            eprintln!("AEGIS_TRUST_E2E_* unset — skipping");
            return;
        };
        let raw = fs::read_to_string(&key_path).unwrap();
        let sk_box = minisign::SecretKeyBox::from_string(&raw).unwrap();
        let sk = sk_box.into_secret_key(Some(passphrase)).unwrap();

        let body = b"aegis-trust verify-roundtrip canary";
        let sig_box =
            minisign::sign(None, &sk, &mut std::io::Cursor::new(body), None, None).expect("sign");
        let sig_str = sig_box.into_string();

        let anchor = TrustAnchor::load_with_floor(1).unwrap();
        let entry = anchor
            .verify_with_epoch(body, sig_str.as_bytes(), 1, 0)
            .expect("verify");
        assert_eq!(entry.epoch, 1);
    }
}
