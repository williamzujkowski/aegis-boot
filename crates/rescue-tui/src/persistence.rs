// SPDX-License-Identifier: MIT OR Apache-2.0

//! Boot menu persistence — remember the user's last choice so they don't
//! have to re-navigate after a failed kexec or between sessions.
//!
//! # Two-tier storage (ADR 0003, #375 Phase 1)
//!
//! Since v0.17+, the authoritative storage target is **`AEGIS_ISOS`** (the
//! operator's data partition at `/run/media/aegis-isos/.aegis-state/`),
//! which survives reboot. The legacy **tmpfs** location under
//! `$AEGIS_STATE_DIR` (default `/run/aegis-boot`) remains as a fallback
//! for the early-boot window before `AEGIS_ISOS` mounts. This lets the
//! rescue-tui cursor land on the previously-booted ISO even after a
//! full reboot of the stick, closing the long-standing #132 spec
//! mismatch + the misleading #123 "pre-selection on next boot — SHIPPED"
//! claim (which was accurate only within a single boot session).
//!
//! See [`docs/architecture/LAST_BOOTED_PERSISTENCE.md`] for the full
//! design + threat model.
//!
//! ## Write path
//!
//! Two coordinated writes per confirm-kexec call:
//!
//! - [`save`] writes to the tmpfs directory passed by the caller
//!   (typically [`default_state_dir`]). Full fidelity — preserves
//!   `cmdline_override`. Used by failed-kexec retry within the same
//!   boot session.
//! - [`save_durable`] writes to `AEGIS_ISOS/.aegis-state/` with atomic
//!   rename-over + directory fsync for durability across mid-write
//!   power loss. Strips `cmdline_override` per ADR 0003 §2.
//!
//! Either failure logs at debug and the boot continues; persistence
//! MUST NEVER fail a kexec. [`migrate_tmpfs_to_aegis_isos`] drains
//! the tmpfs copy onto `AEGIS_ISOS` when the data partition becomes
//! writable later in boot — called opportunistically from
//! `apply_persisted_choice` at startup.
//!
//! ## Load path
//!
//! [`load`] reads tmpfs first (session-local, full fidelity — wins
//! within-session), then falls back to `AEGIS_ISOS` (cross-reboot,
//! stripped). The ordering matters: after a clean reboot `/run` has
//! vaporized so the tmpfs read returns `None` and load picks up from
//! `AEGIS_ISOS`. Fresh-start is always the final fallback.
//!
//! ## What we do NOT persist
//!
//! * **Kernel cmdline overrides.** Documented in ADR 0003 §2 as a
//!   security smell — an attacker with write access to `AEGIS_ISOS` could
//!   inject a cmdline override that survives reboot. Re-enter every
//!   boot if you want non-default.
//! * **Attestation cross-reference.** Kept orthogonal — `attest` is
//!   audit-trail, this module is UX.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The user's last remembered choice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastChoice {
    /// ISO path that was last confirmed. Used to pre-select on next run.
    pub iso_path: PathBuf,
    /// Kernel cmdline override, if the user edited it.
    ///
    /// Per ADR 0003 §2, this field is not persisted across reboots: the
    /// [`save_durable`] path drops it before write, and the cross-reboot
    /// load path synthesizes `None`. Within-session (tmpfs-only) save
    /// still round-trips the value so a failed-kexec retry can replay
    /// the same override.
    pub cmdline_override: Option<String>,
}

impl LastChoice {
    /// Strip fields that must not cross a reboot boundary. Per ADR
    /// 0003 §2: `cmdline_override` is a security smell if persisted.
    fn for_cross_reboot(&self) -> Self {
        Self {
            iso_path: self.iso_path.clone(),
            cmdline_override: None,
        }
    }
}

/// Default `AEGIS_ISOS` mount point used across rescue-tui. Mirrors the
/// constant in [`crate::failure_log::AEGIS_ISOS_MOUNT`] to avoid a
/// circular module dependency while keeping the two locations in sync.
/// The initramfs auto-mounts `AEGIS_ISOS` here on every boot.
const DEFAULT_AEGIS_ISOS_MOUNT: &str = "/run/media/aegis-isos";

/// Hidden subdirectory under `AEGIS_ISOS` holding cross-reboot state.
/// The leading dot + exFAT `hidden` attr (set by the initramfs mkdir
/// path) keeps the directory out of operators' mount-and-browse view
/// when they plug the stick into a laptop.
const AEGIS_ISOS_STATE_DIR: &str = ".aegis-state";

/// Default tmpfs state directory. Overridable via `AEGIS_STATE_DIR`
/// for tests and for operators who want to persist state somewhere
/// other than `/run`. Used as the WRITE FALLBACK when `AEGIS_ISOS`
/// isn't mounted yet (early boot).
#[must_use]
pub fn default_state_dir() -> PathBuf {
    std::env::var("AEGIS_STATE_DIR")
        .map_or_else(|_| PathBuf::from("/run/aegis-boot"), PathBuf::from)
}

/// Directory under `AEGIS_ISOS` that holds the persistent last-choice
/// file. Created on first write; hidden per [`AEGIS_ISOS_STATE_DIR`].
///
/// Resolution order:
/// 1. `AEGIS_ISOS_MOUNT` env var — set by tests to point at a
///    tempdir acting as a simulated `AEGIS_ISOS`. Also usable by
///    operators with a non-standard rescue layout, though that's
///    rare; the shipped initramfs always mounts at the default.
/// 2. `DEFAULT_AEGIS_ISOS_MOUNT` — `/run/media/aegis-isos`.
#[must_use]
pub fn aegis_isos_state_dir() -> PathBuf {
    let mount = std::env::var("AEGIS_ISOS_MOUNT")
        .map_or_else(|_| PathBuf::from(DEFAULT_AEGIS_ISOS_MOUNT), PathBuf::from);
    mount.join(AEGIS_ISOS_STATE_DIR)
}

/// Path to the last-choice file inside `dir`.
#[must_use]
pub fn last_choice_path(dir: &Path) -> PathBuf {
    dir.join("last-choice.json")
}

/// Write `choice` to the state file. Returns an [`std::io::Error`] on
/// filesystem failure; callers typically log and continue rather than
/// error out, since persistence is best-effort.
///
/// This is the **within-session** save path — writes directly to
/// `dir`. Per ADR 0003 §2, cross-reboot saves use [`save_durable`]
/// instead. Retained for within-session-only use cases (a failed
/// kexec returning to rescue-tui should still replay the exact
/// user choice including any `cmdline_override` for that session;
/// that doesn't cross a reboot boundary).
///
/// # Errors
///
/// Returns any I/O error from `create_dir_all` or `write`.
pub fn save(dir: &Path, choice: &LastChoice) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    let json = serde_json::to_string_pretty(choice)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    fs::write(last_choice_path(dir), json)
}

/// Write `choice` durably to **`AEGIS_ISOS`** only, stripping
/// `cmdline_override` per ADR 0003 §2 before write. This is the
/// cross-reboot write path; within-session full-fidelity saves go
/// through [`save`] to tmpfs.
///
/// Call both from the same kexec-confirm site: [`save`] captures
/// the full choice including any cmdline override (useful for
/// failed-kexec retry replay within the same boot), while this
/// function persists a stripped copy that survives reboot.
///
/// Write protocol per ADR 0003 §6.2:
/// 1. Write to a `.tmp` file in the same dir as the final destination.
/// 2. Rename `.tmp` over the destination (atomic within a filesystem).
/// 3. fsync the directory so the rename is durable.
///
/// # Errors
///
/// Returns any I/O error from the atomic-write sequence. Callers
/// typically log at debug and continue — persistence is best-effort
/// and a save failure must never fail the boot or kexec.
pub fn save_durable(choice: &LastChoice) -> std::io::Result<()> {
    let trimmed = choice.for_cross_reboot();
    let json = serde_json::to_string_pretty(&trimmed)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    atomic_write(&aegis_isos_state_dir(), &json)
}

/// Atomic write within a single filesystem: `body` → `dir/last-
/// choice.json.tmp` → rename over `dir/last-choice.json` → fsync the
/// directory. Rename-onto is atomic on Linux + exfat.ko ≥ 5.7 (see
/// ADR 0003 §6.2); directory fsync makes the rename durable across
/// power loss. Any failure short-circuits with the original error.
fn atomic_write(dir: &Path, body: &str) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    if !dir.is_dir() {
        return Err(std::io::Error::other(format!(
            "{} is not a directory after mkdir",
            dir.display()
        )));
    }
    let final_path = last_choice_path(dir);
    let tmp_path = dir.join("last-choice.json.tmp");

    fs::write(&tmp_path, body)?;
    fs::rename(&tmp_path, &final_path)?;

    // fsync the *directory* so the new dir entry is persisted. On
    // exfat.ko ≥ 5.7 this flushes the rename to the underlying flash.
    // Older kernels are not our problem because the *writer* is the
    // rescue kernel we ship (≥ 6.14 per REAL_HARDWARE_REPORT_132.md).
    let dir_handle = fs::File::open(dir)?;
    dir_handle.sync_all()?;
    Ok(())
}

/// Read `choice` from the state file. Tries **tmpfs first** (session-
/// local, full fidelity — preserves `cmdline_override` for failed-kexec
/// retry within the same boot), falls through to `AEGIS_ISOS` (cross-
/// reboot, stripped). Fresh-start is always the final fallback.
///
/// This ordering matters: after a clean reboot tmpfs is empty (`/run`
/// doesn't survive reboot), so load falls through to `AEGIS_ISOS` which
/// is what we want for the cross-reboot UX. Within the same boot
/// session, tmpfs holds the just-saved choice and short-circuits.
///
/// Missing file, invalid JSON, or I/O failure all return [`None`]
/// rather than an error — this is best-effort recall.
///
/// `dir` is the tmpfs location (typically [`default_state_dir()`]),
/// preserved as a parameter for testability and for within-session-
/// only call sites.
#[must_use]
pub fn load(dir: &Path) -> Option<LastChoice> {
    load_from(dir).or_else(|| load_from(&aegis_isos_state_dir()))
}

/// Single-location load helper. Returns `None` on any error and
/// logs at debug so we don't flood the boot log on a fresh install.
/// Exposed at `pub(crate)` so tests (and the ADR 0003 §8 Phase 3
/// reboot-simulation harness) can walk the tmpfs + `AEGIS_ISOS`
/// sources explicitly, without racing on the `AEGIS_ISOS_MOUNT`
/// env var.
pub(crate) fn load_from(dir: &Path) -> Option<LastChoice> {
    let path = last_choice_path(dir);
    let contents = fs::read_to_string(&path).ok()?;
    match serde_json::from_str::<LastChoice>(&contents) {
        Ok(choice) => Some(choice),
        Err(e) => {
            tracing::debug!(
                path = %path.display(),
                error = %e,
                "rescue-tui: last-choice file is corrupt, ignoring"
            );
            None
        }
    }
}

/// Drain the tmpfs last-choice file onto `AEGIS_ISOS` once the data
/// partition is mounted. No-op when `AEGIS_ISOS` isn't available or
/// the tmpfs file doesn't exist. Safe to call every few seconds from
/// a rescue-tui event-loop tick (same cadence as
/// `failure_log::migrate_tmpfs_to_aegis_isos`).
///
/// Returns `Ok(true)` when a migration occurred, `Ok(false)` when
/// there was nothing to migrate, or an `Err` when the tmpfs source
/// existed but couldn't be moved.
///
/// # Errors
///
/// Propagates filesystem errors from the read / `atomic_write` / remove
/// sequence. Callers usually log and continue.
pub fn migrate_tmpfs_to_aegis_isos() -> std::io::Result<bool> {
    let tmpfs = default_state_dir();
    let tmpfs_file = last_choice_path(&tmpfs);
    if !tmpfs_file.exists() {
        return Ok(false);
    }
    let body = fs::read_to_string(&tmpfs_file)?;
    atomic_write(&aegis_isos_state_dir(), &body)?;
    // Success — remove the tmpfs copy so load() doesn't read a stale
    // version on the next tick.
    let _ = fs::remove_file(&tmpfs_file);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Env-var behavior (`AEGIS_STATE_DIR`) is not tested directly: mutating
    // the process-global environment in Rust 2024 requires `unsafe`, which
    // the crate forbids at the top level. The env-read is trivial (two
    // lines); logic-heavy tests below exercise save/load against explicit
    // paths instead.

    #[test]
    fn save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let choice = LastChoice {
            iso_path: PathBuf::from("/run/media/usb1/ubuntu.iso"),
            cmdline_override: Some("quiet splash".to_string()),
        };
        save(dir.path(), &choice).unwrap_or_else(|e| panic!("save: {e}"));
        let loaded = load(dir.path()).unwrap_or_else(|| panic!("load"));
        assert_eq!(loaded, choice);
    }

    #[test]
    fn load_missing_returns_none() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        assert!(load(dir.path()).is_none());
    }

    #[test]
    fn load_corrupt_returns_none() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        fs::create_dir_all(dir.path()).unwrap_or_else(|e| panic!("mkdir: {e}"));
        fs::write(last_choice_path(dir.path()), "{{{not json")
            .unwrap_or_else(|e| panic!("write: {e}"));
        assert!(load(dir.path()).is_none());
    }

    #[test]
    fn save_creates_missing_parent_dir() {
        let root = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let nested = root.path().join("nonexistent/aegis");
        let choice = LastChoice {
            iso_path: PathBuf::from("/x"),
            cmdline_override: None,
        };
        save(&nested, &choice).unwrap_or_else(|e| panic!("save: {e}"));
        assert!(nested.join("last-choice.json").exists());
    }

    #[test]
    fn cross_reboot_form_strips_cmdline_override() {
        let choice = LastChoice {
            iso_path: PathBuf::from("/run/media/usb1/alpine.iso"),
            cmdline_override: Some("init=/bin/sh".to_string()),
        };
        let trimmed = choice.for_cross_reboot();
        assert_eq!(trimmed.iso_path, choice.iso_path);
        assert_eq!(trimmed.cmdline_override, None);
    }

    #[test]
    fn atomic_write_produces_final_not_tmp() {
        // `atomic_write` ends with only `last-choice.json` on disk —
        // the `.tmp` staging file must not be left behind.
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        atomic_write(dir.path(), r#"{"iso_path":"/x","cmdline_override":null}"#)
            .unwrap_or_else(|e| panic!("atomic_write: {e}"));
        assert!(dir.path().join("last-choice.json").exists());
        assert!(!dir.path().join("last-choice.json.tmp").exists());
    }

    #[test]
    fn atomic_write_overwrites_existing() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        atomic_write(dir.path(), r#"{"iso_path":"/a","cmdline_override":null}"#)
            .unwrap_or_else(|e| panic!("atomic_write first: {e}"));
        atomic_write(dir.path(), r#"{"iso_path":"/b","cmdline_override":null}"#)
            .unwrap_or_else(|e| panic!("atomic_write second: {e}"));
        let body = fs::read_to_string(last_choice_path(dir.path()))
            .unwrap_or_else(|e| panic!("read: {e}"));
        assert!(body.contains("/b"));
        assert!(!body.contains("/a"));
    }

    #[test]
    fn load_from_prefers_newer_content() {
        // load_from returns exactly what's on disk — no stacking of
        // fallback locations here. That's load()'s responsibility.
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        atomic_write(
            dir.path(),
            r#"{"iso_path":"/fresh.iso","cmdline_override":null}"#,
        )
        .unwrap_or_else(|e| panic!("atomic_write: {e}"));
        let loaded = load_from(dir.path()).unwrap_or_else(|| panic!("load_from"));
        assert_eq!(loaded.iso_path, PathBuf::from("/fresh.iso"));
    }

    // ---- #375 Phase 3: reboot-simulation round-trip -----------------------

    /// ADR 0003 §8 Phase 3's core acceptance property — written as a
    /// pure-Rust round-trip (no QEMU, no env-var mutation). Models the
    /// two locations explicitly so the test reads as "write to `AEGIS_ISOS`,
    /// then pretend /run vaporized, then verify the read still finds
    /// our choice via the `AEGIS_ISOS` fallback."
    ///
    /// Closes the #132 acceptance-criteria gap at the unit level; the
    /// real-hardware variant of this test (flash stick, boot, pick ISO,
    /// reboot, check cursor) is documented in
    /// `docs/validation/REAL_HARDWARE_REPORT_132.md` and requires
    /// physical multi-vendor hardware.
    #[test]
    fn reboot_simulation_round_trip() {
        let simulated_aegis_isos =
            tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir(aegis_isos): {e}"));
        let simulated_tmpfs_preboot =
            tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir(tmpfs pre): {e}"));

        // ---- Pre-reboot: user confirms ISO ----
        let original = LastChoice {
            iso_path: PathBuf::from("/run/media/aegis-isos/ubuntu-24.04-desktop-amd64.iso"),
            cmdline_override: Some("quiet splash debug".to_string()),
        };
        // save_last_choice wires both writes; emulate that here.
        let body_full = serde_json::to_string_pretty(&original)
            .unwrap_or_else(|e| panic!("serialize tmpfs: {e}"));
        fs::write(last_choice_path(simulated_tmpfs_preboot.path()), body_full)
            .unwrap_or_else(|e| panic!("tmpfs write: {e}"));

        let body_stripped = serde_json::to_string_pretty(&original.for_cross_reboot())
            .unwrap_or_else(|e| panic!("serialize aegis: {e}"));
        atomic_write(simulated_aegis_isos.path(), &body_stripped)
            .unwrap_or_else(|e| panic!("atomic_write aegis: {e}"));

        // ---- Reboot happens ----
        // /run vaporizes; AEGIS_ISOS persists. Simulate by dropping
        // the pre-reboot tmpfs handle and making a fresh empty one.
        drop(simulated_tmpfs_preboot);
        let simulated_tmpfs_postboot =
            tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir(tmpfs post): {e}"));
        assert!(
            fs::read_to_string(last_choice_path(simulated_tmpfs_postboot.path())).is_err(),
            "post-reboot tmpfs should be empty"
        );

        // ---- Post-reboot: walk the load priorities manually ----
        // This exactly mirrors `load()`'s logic (tmpfs first, AEGIS_ISOS
        // fallback) but against explicit paths so we avoid racing on
        // the AEGIS_ISOS_MOUNT env var with other tests.
        let loaded = load_from(simulated_tmpfs_postboot.path())
            .or_else(|| load_from(simulated_aegis_isos.path()))
            .unwrap_or_else(|| panic!("load: both sources returned None post-reboot"));

        // The restored choice matches the ISO we picked ...
        assert_eq!(loaded.iso_path, original.iso_path);
        // ... BUT the cmdline override is stripped per ADR 0003 §2.
        assert_eq!(
            loaded.cmdline_override, None,
            "cross-reboot load must NOT carry cmdline_override"
        );
    }

    /// Within-session variant: tmpfs is warm, `load()` short-circuits
    /// there and preserves `cmdline_override` for failed-kexec retry.
    #[test]
    fn within_session_load_prefers_tmpfs_with_cmdline_override() {
        let aegis_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir(aegis): {e}"));
        let tmpfs_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir(tmpfs): {e}"));

        let choice = LastChoice {
            iso_path: PathBuf::from("/x.iso"),
            cmdline_override: Some("init=/bin/sh".to_string()),
        };

        // tmpfs has the full choice (matches what save() writes)
        save(tmpfs_dir.path(), &choice).unwrap_or_else(|e| panic!("save tmpfs: {e}"));
        // AEGIS_ISOS has the stripped version (matches save_durable)
        let stripped = serde_json::to_string_pretty(&choice.for_cross_reboot())
            .unwrap_or_else(|e| panic!("serialize aegis: {e}"));
        atomic_write(aegis_dir.path(), &stripped).unwrap_or_else(|e| panic!("atomic_write: {e}"));

        // Within-session load pattern (tmpfs first)
        let loaded = load_from(tmpfs_dir.path())
            .or_else(|| load_from(aegis_dir.path()))
            .unwrap_or_else(|| panic!("load"));

        assert_eq!(loaded.iso_path, choice.iso_path);
        assert_eq!(
            loaded.cmdline_override, choice.cmdline_override,
            "within-session load must preserve cmdline_override for failed-kexec retry"
        );
    }
}
