//! Shared command-on-PATH probe. Every aegis-boot surface that asks
//! "is this command available?" goes through [`which`] so the answer
//! is the same whether `doctor`, `fetch-image`, or a future caller
//! is asking. Inconsistent answers were the surface area of #332 —
//! `doctor` said cosign was present while `fetch-image` said it was
//! missing, because each used a different probe.
//!
//! The probe is deliberately just "does the file exist as a regular
//! file at one of these paths?". It does NOT try to run the binary
//! (`--version` probes can return non-zero for reasons unrelated to
//! whether the binary is installed — missing network, locked
//! transparency log, corrupted keyring). Execution errors surface
//! at the actual use-site with the real stderr, which is more
//! actionable than "cosign not on PATH".

use std::path::{Path, PathBuf};

/// Canonical sbin directories probed when `$PATH` lookup misses.
/// Many distros (notably openSUSE, #328) do not include `/usr/sbin`
/// in the `$PATH` inherited by `sudo` or by child processes of the
/// install.sh post-install preflight. Root-utility commands that
/// live only in sbin (e.g. `sgdisk`) would otherwise produce a
/// FAIL row in `doctor` despite being installed.
pub(crate) const SBIN_FALLBACKS: &[&str] = &["/usr/sbin", "/sbin", "/usr/local/sbin"];

/// Look up `cmd` on the current process's PATH, falling back to the
/// canonical sbin directories if it's not found on PATH. Returns the
/// absolute path of the first match, or `None` if neither lookup hits.
pub(crate) fn which(cmd: &str) -> Option<PathBuf> {
    which_in(cmd, std::env::var_os("PATH").as_deref(), SBIN_FALLBACKS)
}

/// Explicit-inputs variant of [`which`] for testing. Takes the PATH
/// env value and the sbin-fallback list directly so a test can pin
/// the exact search space without mutating process-wide state.
pub(crate) fn which_in(
    cmd: &str,
    path_env: Option<&std::ffi::OsStr>,
    sbin_fallbacks: &[&str],
) -> Option<PathBuf> {
    if let Some(path) = path_env {
        for dir in std::env::split_paths(path) {
            let candidate = dir.join(cmd);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    for sbin in sbin_fallbacks {
        let candidate = Path::new(sbin).join(cmd);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn which_in_finds_binary_on_path() {
        let Ok(dir) = tempfile::tempdir() else { return };
        let bin = dir.path().join("fake-cmd");
        if std::fs::write(&bin, b"#!/bin/sh\n").is_err() {
            return;
        }
        let path_env = std::ffi::OsString::from(dir.path());
        let found = which_in("fake-cmd", Some(path_env.as_os_str()), &[]);
        assert_eq!(found.as_deref(), Some(bin.as_path()));
    }

    #[test]
    fn which_in_falls_back_to_sbin_when_path_misses() {
        let Ok(dir) = tempfile::tempdir() else { return };
        let bin = dir.path().join("sbin-only-cmd");
        if std::fs::write(&bin, b"#!/bin/sh\n").is_err() {
            return;
        }
        let fallback = dir.path().to_string_lossy().into_owned();
        let found = which_in(
            "sbin-only-cmd",
            Some(std::ffi::OsStr::new("/nonexistent-path")),
            &[&fallback],
        );
        assert_eq!(found.as_deref(), Some(bin.as_path()));
    }

    #[test]
    fn which_in_returns_none_when_both_miss() {
        let found = which_in(
            "definitely-not-a-real-binary-for-aegis-test",
            Some(std::ffi::OsStr::new("/nonexistent-path")),
            &["/nonexistent-sbin"],
        );
        assert!(found.is_none());
    }
}
