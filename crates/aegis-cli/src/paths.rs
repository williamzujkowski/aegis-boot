// SPDX-License-Identifier: MIT OR Apache-2.0

//! Operator-visible state paths for aegis-boot (#375 Phase 1).
//!
//! Single source of truth for "where does aegis-boot keep its state?"
//! Previously every caller resolved `XDG_DATA_HOME` / `HOME` / sudo
//! rewrites independently (see pre-refactor `attest.rs::data_dir` +
//! `update.rs::attestation_dir`). This module centralizes that
//! resolution and adds the `AEGIS_STATE_DIR` override foundation that
//! ADR 0002 (epoch counter seen-epoch state) and ADR 0003 (cross-
//! reboot last-booted persistence) both build on.
//!
//! Resolution order (first match wins):
//!
//! 1. `AEGIS_STATE_DIR` — explicit override. Used verbatim; test +
//!    deployment knob for operators who want state elsewhere. The
//!    value replaces the entire `<base>/aegis-boot` prefix, so
//!    `AEGIS_STATE_DIR=/var/lib/aegis-boot` yields
//!    `/var/lib/aegis-boot/attestations/` (not
//!    `/var/lib/aegis-boot/aegis-boot/attestations/`).
//! 2. Sudo-aware `HOME` — when running under `sudo`, prefer the
//!    original user's `~/.local/share/aegis-boot` over root's,
//!    so `sudo aegis-boot flash` writes attestations where
//!    `aegis-boot attest list` (run unprivileged) will look for
//!    them. See `crate::attest::sudo_aware_data_dir` for the
//!    mechanism.
//! 3. `XDG_DATA_HOME/aegis-boot` — standard spec-compliant path.
//! 4. `$HOME/.local/share/aegis-boot` — XDG default when unset.
//! 5. `/tmp/aegis-boot` — fall-through when even `HOME` is unset;
//!    signals a degraded environment (likely a CI sandbox or init
//!    script context) and keeps things runnable rather than
//!    panicking.

use std::path::PathBuf;

/// Environment variable that overrides the default state-dir base.
/// Shared with `crates/rescue-tui/src/persistence.rs` so the same
/// knob drives both the tmpfs last-choice path and the on-disk
/// attestation path.
pub(crate) const AEGIS_STATE_DIR_ENV: &str = "AEGIS_STATE_DIR";

/// Root of the aegis-boot state tree. Everything the CLI persists
/// (attestations, future trust-anchor seen-epoch, future last-booted
/// state) lives under this dir.
///
/// This is a pure function of the environment; no filesystem i/o and
/// no subprocess calls.
#[must_use]
pub(crate) fn aegis_state_root() -> PathBuf {
    if let Some(explicit) = std::env::var_os(AEGIS_STATE_DIR_ENV) {
        return PathBuf::from(explicit);
    }
    if let Some(sudo_data) = crate::attest::sudo_aware_data_dir() {
        return sudo_data.join("aegis-boot");
    }
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("aegis-boot")
}

/// Where flash + add write signed attestation manifests. Always
/// resolvable (ultimately falls back to `/tmp/aegis-boot/attestations`),
/// so callers don't need to handle an absent base.
#[must_use]
pub(crate) fn attestations_dir() -> PathBuf {
    aegis_state_root().join("attestations")
}

/// Where the ADR-0002 Key Epoch counter's `seen-epoch` file lives.
/// Not yet written or read by any code; defined here so the trust-
/// anchor module's future plumbing has a canonical path to target.
/// File contents: a single integer in ASCII, newline-terminated.
#[must_use]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn trust_seen_epoch_path() -> PathBuf {
    aegis_state_root().join("trust").join("seen-epoch")
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::missing_panics_doc
)]
mod tests {
    use super::*;

    /// Tests MUST NOT touch process env (crate has `forbid(unsafe_code)`
    /// which blocks Rust-2024's unsafe `std::env::set_var`). Instead,
    /// each test inspects the function's behavior under a specific
    /// current env state and asserts what the state-root *would* be
    /// given the visible env. This isolates from ambient config and
    /// doesn't break under parallel tests.

    #[test]
    fn aegis_state_root_honors_explicit_override_when_set() {
        // If AEGIS_STATE_DIR is set in the ambient env (e.g. in CI),
        // the function MUST return exactly that path. Otherwise, skip
        // the override-specific assertion and only verify the
        // fallback path.
        if let Some(explicit) = std::env::var_os(AEGIS_STATE_DIR_ENV) {
            assert_eq!(aegis_state_root(), PathBuf::from(explicit));
        } else {
            // Without the override, we should land in one of the
            // sudo-aware / XDG / HOME / /tmp paths — they all have
            // `aegis-boot` as their final segment.
            let root = aegis_state_root();
            let last = root.file_name().and_then(|s| s.to_str()).unwrap();
            assert_eq!(
                last, "aegis-boot",
                "non-override resolution should end in 'aegis-boot', got {root:?}"
            );
        }
    }

    #[test]
    fn attestations_dir_is_state_root_plus_attestations() {
        let root = aegis_state_root();
        let att = attestations_dir();
        assert_eq!(att, root.join("attestations"));
    }

    #[test]
    fn trust_seen_epoch_path_is_state_root_plus_trust_seen_epoch() {
        let root = aegis_state_root();
        let p = trust_seen_epoch_path();
        assert_eq!(p, root.join("trust").join("seen-epoch"));
    }
}
