// SPDX-License-Identifier: MIT OR Apache-2.0

#![doc = include_str!("../README.md")]

mod anchor;
mod errors;
mod seen_epoch;

pub use anchor::{EpochEntry, TrustAnchor};
pub use errors::TrustAnchorError;
pub use seen_epoch::{SeenEpochState, load_seen_epoch, store_seen_epoch};

/// Minimum epoch this binary trusts. Baked in at build time from
/// `keys/canonical-epoch.json` via `build.rs`; falls back to `0`
/// (an unsafe sentinel) when the keys directory isn't discoverable
/// from the build-manifest dir. Callers of [`TrustAnchor::load`]
/// get a `TrustAnchorError::UnsafeDefaultEpoch` if they end up with
/// this fallback at runtime.
pub const MIN_REQUIRED_EPOCH: u32 = {
    // Parse env at compile time. Rust's parse-from-str requires
    // const-fn support that's gated; use a small custom parser.
    const fn parse_u32(s: &str) -> u32 {
        let bytes = s.as_bytes();
        let mut n: u32 = 0;
        let mut i = 0;
        while i < bytes.len() {
            let d = bytes[i];
            assert!(
                d >= b'0' && d <= b'9',
                "non-digit in AEGIS_MIN_REQUIRED_EPOCH"
            );
            n = n * 10 + (d - b'0') as u32;
            i += 1;
        }
        n
    }
    parse_u32(env!("AEGIS_MIN_REQUIRED_EPOCH"))
};

/// Computes the effective floor for verification calls: the greater
/// of [`MIN_REQUIRED_EPOCH`] (binary-embedded) and the monotonically-
/// advancing `seen_epoch` value the local install has observed.
///
/// Separate from [`TrustAnchor::verify_with_epoch`] so callers that
/// want to surface drift (e.g. `aegis-boot doctor`) can query the
/// floor without actually running a verify.
#[must_use]
pub fn effective_floor(seen_epoch: u32) -> u32 {
    MIN_REQUIRED_EPOCH.max(seen_epoch)
}
