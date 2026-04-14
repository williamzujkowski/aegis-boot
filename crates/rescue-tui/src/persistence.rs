//! Boot menu persistence — remember the user's last choice so they don't
//! have to re-navigate after a failed kexec or between sessions.
//!
//! Storage: JSON at `$AEGIS_STATE_DIR/last-choice.json` (defaults to
//! `/run/aegis-boot`). `/run` is a tmpfs; state is lost at reboot, which is
//! exactly what we want for a rescue environment.
//!
//! Persistence across reboots would require writing to the boot media,
//! which is out of scope here — that's a TPM/NVRAM story for a later ADR.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The user's last remembered choice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastChoice {
    /// ISO path that was last confirmed. Used to pre-select on next run.
    pub iso_path: PathBuf,
    /// Kernel cmdline override, if the user edited it.
    pub cmdline_override: Option<String>,
}

/// Default state directory. Overridable via `AEGIS_STATE_DIR` for tests and
/// for operators who want to persist state somewhere other than `/run`.
#[must_use]
pub fn default_state_dir() -> PathBuf {
    std::env::var("AEGIS_STATE_DIR").map_or_else(|_| PathBuf::from("/run/aegis-boot"), PathBuf::from)
}

/// Path to the last-choice file inside `dir`.
#[must_use]
pub fn last_choice_path(dir: &Path) -> PathBuf {
    dir.join("last-choice.json")
}

/// Write `choice` to the state file. Returns an [`std::io::Error`] on
/// filesystem failure; callers typically log and continue rather than error
/// out, since persistence is best-effort.
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

/// Read `choice` from the state file.
///
/// Missing file, invalid JSON, or I/O failure all return [`None`] rather than
/// an error — this is best-effort recall, and a fresh state is always the
/// correct fallback.
#[must_use]
pub fn load(dir: &Path) -> Option<LastChoice> {
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
        fs::write(last_choice_path(dir.path()), "{{{not json").unwrap_or_else(|e| panic!("write: {e}"));
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
}
