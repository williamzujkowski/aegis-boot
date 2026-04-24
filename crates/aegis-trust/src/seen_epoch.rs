// SPDX-License-Identifier: MIT OR Apache-2.0

//! Monotonic `seen-epoch` state file per ADR 0002 §3.5.
//!
//! The file lives at `$XDG_STATE_HOME/aegis-boot/trust/seen-epoch`
//! (or `~/.local/state/aegis-boot/trust/seen-epoch` if XDG is unset)
//! and stores a single decimal u32. Every successful verify CAN
//! advance it via [`store_seen_epoch`]; loads via [`load_seen_epoch`]
//! return `0` when the file doesn't exist yet (first-run semantics).
//!
//! The critical invariant: **the value on disk never regresses.**
//! [`store_seen_epoch`] reads the current value first and refuses
//! to write a lower one — a rollback attempt gets rejected even if
//! the caller passes a bad input.

use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use crate::errors::TrustAnchorError;

/// Pure data type — what's in the seen-epoch state file. Separate
/// from the file I/O so callers can construct synthetic values for
/// tests or for `aegis-boot doctor`'s "what's the current floor"
/// read without touching the filesystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeenEpochState {
    /// Highest trust-epoch this install has ever observed.
    /// Monotonic — never decreases once set.
    pub epoch: u32,
}

impl Default for SeenEpochState {
    /// First-run default — zero means "never observed any epoch."
    /// [`crate::effective_floor`] still bounds below by
    /// [`crate::MIN_REQUIRED_EPOCH`], so this default doesn't
    /// bypass the binary trust floor.
    fn default() -> Self {
        Self { epoch: 0 }
    }
}

/// Resolve the on-disk path per XDG conventions. Injected for tests
/// via [`seen_epoch_path_in`].
#[must_use]
pub fn seen_epoch_path() -> PathBuf {
    seen_epoch_path_in(
        std::env::var("XDG_STATE_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
    )
}

/// Inner form — takes the two env vars explicitly so tests can
/// pass synthetic values without mutating the process env.
#[must_use]
pub fn seen_epoch_path_in(xdg_state: Option<&str>, home: Option<&str>) -> PathBuf {
    let base: PathBuf = match (xdg_state, home) {
        (Some(x), _) if !x.is_empty() => PathBuf::from(x),
        (_, Some(h)) if !h.is_empty() => {
            let mut p = PathBuf::from(h);
            p.push(".local");
            p.push("state");
            p
        }
        _ => {
            // Unusual environment — fall back to a relative `.state`
            // dir. Not ideal, but better than silently misfiling.
            PathBuf::from(".state")
        }
    };
    base.join("aegis-boot").join("trust").join("seen-epoch")
}

/// Read the seen-epoch counter from disk. Returns
/// [`SeenEpochState::default`] if the file doesn't exist yet.
///
/// Treats any parse failure (corrupted file, unexpected contents)
/// as a hard error rather than silently falling back to 0 — a
/// corrupted file is a signal worth flagging, not smoothing over.
///
/// # Errors
///
/// [`TrustAnchorError::SeenEpochIo`] on read failure or parse
/// failure (file exists but isn't a bare u32).
pub fn load_seen_epoch() -> Result<SeenEpochState, TrustAnchorError> {
    load_seen_epoch_from(&seen_epoch_path())
}

/// Explicit-path variant for tests.
///
/// # Errors
///
/// Same as [`load_seen_epoch`].
pub fn load_seen_epoch_from(path: &Path) -> Result<SeenEpochState, TrustAnchorError> {
    match fs::read_to_string(path) {
        Ok(body) => {
            let trimmed = body.trim();
            let epoch: u32 = trimmed.parse().map_err(|e| {
                TrustAnchorError::SeenEpochIo(format!(
                    "{}: parse {trimmed:?} as u32: {e}",
                    path.display()
                ))
            })?;
            Ok(SeenEpochState { epoch })
        }
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(SeenEpochState::default()),
        Err(e) => Err(TrustAnchorError::SeenEpochIo(format!(
            "{}: {e}",
            path.display()
        ))),
    }
}

/// Advance the seen-epoch counter on disk. Refuses to write a value
/// lower than what's already there — monotonicity is ADR 0002's
/// defense against rollback attacks.
///
/// Creates the parent directory if needed. Uses a stage-then-rename
/// pattern so a mid-write crash can't leave a truncated file that
/// would then parse-fail on the next load.
///
/// # Errors
///
/// [`TrustAnchorError::SeenEpochIo`] if reading the current value,
/// creating the parent dir, writing the staging file, fsyncing, or
/// renaming fails.
pub fn store_seen_epoch(new: u32) -> Result<SeenEpochState, TrustAnchorError> {
    store_seen_epoch_at(&seen_epoch_path(), new)
}

/// Explicit-path variant for tests.
///
/// # Errors
///
/// Same as [`store_seen_epoch`].
pub fn store_seen_epoch_at(path: &Path, new: u32) -> Result<SeenEpochState, TrustAnchorError> {
    let current = load_seen_epoch_from(path)?;
    let next = current.epoch.max(new);

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            TrustAnchorError::SeenEpochIo(format!("mkdir {}: {e}", parent.display()))
        })?;
    }

    // Stage-then-rename: write to `path.tmp`, rename over `path`.
    // Rename is atomic on the same filesystem so either the old
    // value or the new value is visible, never a truncated file.
    let tmp = path.with_extension("tmp");
    {
        let mut f = fs::File::create(&tmp)
            .map_err(|e| TrustAnchorError::SeenEpochIo(format!("create {}: {e}", tmp.display())))?;
        writeln!(f, "{next}")
            .map_err(|e| TrustAnchorError::SeenEpochIo(format!("write {}: {e}", tmp.display())))?;
        f.sync_all()
            .map_err(|e| TrustAnchorError::SeenEpochIo(format!("fsync {}: {e}", tmp.display())))?;
    }
    fs::rename(&tmp, path).map_err(|e| {
        TrustAnchorError::SeenEpochIo(format!(
            "rename {} -> {}: {e}",
            tmp.display(),
            path.display()
        ))
    })?;

    Ok(SeenEpochState { epoch: next })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn path_prefers_xdg_state_home() {
        let p = seen_epoch_path_in(Some("/x"), Some("/h"));
        assert_eq!(p, PathBuf::from("/x/aegis-boot/trust/seen-epoch"));
    }

    #[test]
    fn path_falls_back_to_home_plus_dot_local_state() {
        let p = seen_epoch_path_in(None, Some("/home/op"));
        assert_eq!(
            p,
            PathBuf::from("/home/op/.local/state/aegis-boot/trust/seen-epoch")
        );
    }

    #[test]
    fn path_empty_xdg_treated_as_unset() {
        let p = seen_epoch_path_in(Some(""), Some("/home/op"));
        assert_eq!(
            p,
            PathBuf::from("/home/op/.local/state/aegis-boot/trust/seen-epoch")
        );
    }

    #[test]
    fn load_returns_default_when_file_absent() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("nonexistent");
        let state = load_seen_epoch_from(&p).unwrap();
        assert_eq!(state, SeenEpochState::default());
        assert_eq!(state.epoch, 0);
    }

    #[test]
    fn load_parses_valid_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("seen-epoch");
        std::fs::write(&p, "42\n").unwrap();
        assert_eq!(load_seen_epoch_from(&p).unwrap().epoch, 42);
    }

    #[test]
    fn load_refuses_corrupted_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("seen-epoch");
        std::fs::write(&p, "not a number").unwrap();
        match load_seen_epoch_from(&p) {
            Err(TrustAnchorError::SeenEpochIo(_)) => {}
            other => panic!("expected SeenEpochIo, got {other:?}"),
        }
    }

    #[test]
    fn store_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("aegis-boot/trust/seen-epoch");
        assert!(!p.exists());
        let s = store_seen_epoch_at(&p, 3).unwrap();
        assert_eq!(s.epoch, 3);
        assert_eq!(load_seen_epoch_from(&p).unwrap().epoch, 3);
    }

    #[test]
    fn store_monotonic_refuses_rollback() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("seen-epoch");
        store_seen_epoch_at(&p, 5).unwrap();
        // Try to regress to 3 — stored value should stay at 5.
        let s = store_seen_epoch_at(&p, 3).unwrap();
        assert_eq!(s.epoch, 5, "rollback must be refused");
        assert_eq!(load_seen_epoch_from(&p).unwrap().epoch, 5);
    }

    #[test]
    fn store_advances_on_higher_value() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("seen-epoch");
        store_seen_epoch_at(&p, 1).unwrap();
        store_seen_epoch_at(&p, 2).unwrap();
        store_seen_epoch_at(&p, 7).unwrap();
        store_seen_epoch_at(&p, 7).unwrap(); // idempotent
        store_seen_epoch_at(&p, 5).unwrap(); // ignored
        assert_eq!(load_seen_epoch_from(&p).unwrap().epoch, 7);
    }

    #[test]
    fn store_is_atomic_no_tmp_leftover_on_success() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("seen-epoch");
        store_seen_epoch_at(&p, 42).unwrap();
        let tmp = p.with_extension("tmp");
        assert!(
            !tmp.exists(),
            "stage file should be renamed, not left behind"
        );
    }
}
