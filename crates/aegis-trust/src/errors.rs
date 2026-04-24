// SPDX-License-Identifier: MIT OR Apache-2.0

use thiserror::Error;

/// Every way the trust-anchor layer can refuse to validate a signed
/// body. Each variant corresponds to a specific failure mode ADR 0002
/// calls out; surfacing distinct variants lets callers render distinct
/// operator messages (`aegis-boot doctor` uses the variant type as
/// the row category).
#[derive(Debug, Error, PartialEq, Eq)]
pub enum TrustAnchorError {
    /// Build-time discovery of `keys/canonical-epoch.json` failed and
    /// the binary was compiled against the epoch=0 fallback sentinel.
    /// Verification must refuse — the embedded `MIN_REQUIRED_EPOCH`
    /// doesn't reflect a real trust root.
    #[error(
        "binary was built with AEGIS_MIN_REQUIRED_EPOCH=0 (the unsafe-default sentinel). \
         Rebuild in-workspace, or set AEGIS_MIN_REQUIRED_EPOCH_OVERRIDE=<N> at build time."
    )]
    UnsafeDefaultEpoch,

    /// The epoch carried in a signed payload is below the binary's
    /// embedded `MIN_REQUIRED_EPOCH` floor. Refuse — the signer is
    /// claiming a key we no longer trust.
    #[error("payload epoch {payload_epoch} is below binary floor (MIN_REQUIRED_EPOCH={required})")]
    EpochBelowBinaryFloor {
        /// Epoch the payload claimed.
        payload_epoch: u32,
        /// The binary's built-in `MIN_REQUIRED_EPOCH` floor.
        required: u32,
    },

    /// The epoch is below the local install's monotonic `seen_epoch`
    /// counter. Refuse — someone is trying to roll back to an older
    /// key-epoch than this install has already accepted something from.
    #[error("payload epoch {payload_epoch} is below locally-seen floor (seen_epoch={seen_epoch})")]
    EpochBelowSeenFloor {
        /// Epoch the payload claimed.
        payload_epoch: u32,
        /// Monotonic `seen_epoch` value from the local state file.
        seen_epoch: u32,
    },

    /// No entry in `historical-anchors.json` matches the claimed
    /// epoch. Either the payload is forged or the binary is out of
    /// date with the canonical anchor list.
    #[error("no trust anchor registered for epoch {payload_epoch}")]
    UnknownEpoch {
        /// Epoch the payload claimed.
        payload_epoch: u32,
    },

    /// The signature failed cryptographic verification under the
    /// epoch's public key. Classic "bad signature" refusal.
    #[error("signature verification failed for epoch {payload_epoch}: {detail}")]
    SignatureInvalid {
        /// Epoch the payload claimed.
        payload_epoch: u32,
        /// Underlying minisign error string.
        detail: String,
    },

    /// The pubkey file embedded in the binary couldn't be parsed as
    /// a minisign public key. Indicates build-time corruption or an
    /// out-of-date pubkey format; operator must rebuild.
    #[error("embedded pubkey for epoch {epoch} failed to parse: {detail}")]
    PubkeyParseFailure {
        /// Epoch whose pubkey failed to parse.
        epoch: u32,
        /// Underlying minisign-parse error string.
        detail: String,
    },

    /// `historical-anchors.json` embedded at build time couldn't be
    /// deserialized. Same root cause as `PubkeyParseFailure`.
    #[error("embedded historical-anchors.json failed to parse: {0}")]
    AnchorsParseFailure(String),

    /// Reading or writing the `seen-epoch` state file failed.
    /// Differentiated from the above because it's an install-local
    /// issue (filesystem permissions, `$XDG_STATE_HOME` wrong), not a
    /// trust-chain issue.
    #[error("seen-epoch state I/O error: {0}")]
    SeenEpochIo(String),
}
