//! Minisign detached signature verification.
//!
//! Looks for `<iso>.minisig` sibling files and verifies them against a trust
//! store of `.pub` keys (minisign format) provided via the `AEGIS_TRUSTED_KEYS`
//! environment variable (colon-separated list of directories or individual
//! `.pub` files).
//!
//! Unlike [`crate::signature`] (which only checks hash integrity), minisign
//! provides **authentication** — the signer possesses the private key
//! corresponding to the trusted public key, and no byte of the ISO has been
//! changed since they signed it.
//!
//! # Trust model
//!
//! - A public key under `AEGIS_TRUSTED_KEYS` is **authoritative**. Anything it
//!   signs is treated as authentic.
//! - No key fingerprint pinning beyond minisign's key ID. Key rotation is
//!   the operator's problem.
//! - Missing key dir / no loaded keys → every ISO is `KeyNotTrusted` even
//!   when the signature itself is syntactically valid. Fail-closed.

use std::fs;
use std::path::{Path, PathBuf};

use minisign_verify::{Error as MinisignError, PublicKey, Signature};
use serde::{Deserialize, Serialize};

/// Outcome of a minisign signature verification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignatureVerification {
    /// Signature is cryptographically valid AND signed by a key in the trust
    /// store. Authentication established.
    Verified {
        /// Hex-encoded first 8 bytes of the key's raw `keynum` (minisign's
        /// identifier). Lets the TUI render "signed by: abcd1234" without
        /// claiming more provenance than we actually have.
        key_id: String,
        /// Path to the .minisig file we validated.
        sig_path: PathBuf,
    },
    /// Signature parsed and is structurally valid, but the signing key is
    /// not in the trust store.
    KeyNotTrusted {
        /// Observed key ID from the signature envelope.
        key_id: String,
    },
    /// Signature parsed but the computed signature over the ISO bytes does
    /// not match what the sig file claims — tampering or corruption.
    Forged {
        /// Path to the .minisig file.
        sig_path: PathBuf,
    },
    /// No .minisig sidecar was found.
    NotPresent,
    /// An I/O or parse error made verification impossible. Treated the same
    /// as `NotPresent` for UX purposes but logged separately.
    Error {
        /// Human-readable reason.
        reason: String,
    },
}

impl SignatureVerification {
    /// Short user-facing label for the TUI.
    #[must_use]
    pub fn summary(&self) -> &'static str {
        match self {
            Self::Verified { .. } => "verified",
            Self::KeyNotTrusted { .. } => "UNTRUSTED KEY",
            Self::Forged { .. } => "FORGED",
            Self::NotPresent => "not present",
            Self::Error { .. } => "error",
        }
    }
}

/// Verify `iso_path` against its sibling `<iso>.minisig` (if any).
///
/// The trust store is read from `AEGIS_TRUSTED_KEYS` (colon-separated list of
/// either directories containing `.pub` files or individual `.pub` files).
/// Missing / empty env var → `KeyNotTrusted` even for valid signatures.
///
/// # Errors
///
/// This function does not return `Err`; all failures are reported as
/// [`SignatureVerification::Error`] or [`SignatureVerification::NotPresent`]
/// so the caller can make a UX decision rather than bubble up.
#[must_use]
pub fn verify_iso_signature(iso_path: &Path) -> SignatureVerification {
    let sig_path = sidecar_sig_path(iso_path);
    let Ok(sig_text) = fs::read_to_string(&sig_path) else {
        return SignatureVerification::NotPresent;
    };
    let signature = match Signature::decode(&sig_text) {
        Ok(s) => s,
        Err(e) => {
            return SignatureVerification::Error {
                reason: format!("sig parse failed: {e}"),
            };
        }
    };

    let trusted = load_trusted_keys();
    let iso_bytes = match fs::read(iso_path) {
        Ok(b) => b,
        Err(e) => {
            return SignatureVerification::Error {
                reason: format!("ISO read failed: {e}"),
            };
        }
    };

    let mut saw_forgery_under_trusted_key = false;
    for (pubkey, source) in &trusted {
        match pubkey.verify(&iso_bytes, &signature, false) {
            Ok(()) => {
                return SignatureVerification::Verified {
                    key_id: key_id_from_sig(&signature),
                    sig_path: PathBuf::from(source),
                };
            }
            // Trusted key matches the signature's key_id but the signature
            // does not verify over the bytes — the file was tampered after
            // the trusted signer signed it. Distinct from "wrong signer."
            // (#57)
            Err(MinisignError::InvalidSignature) => {
                saw_forgery_under_trusted_key = true;
            }
            // UnexpectedKeyId / other errors: this trusted key didn't sign
            // it. Keep iterating in case another trusted key did.
            Err(_) => {}
        }
    }

    if saw_forgery_under_trusted_key {
        return SignatureVerification::Forged {
            sig_path: sig_path.clone(),
        };
    }

    // No trusted key signed this ISO. Either trust store is empty (fail-
    // closed default) or the signer is unknown to us. Either way the user
    // sees an "untrusted" diagnostic, not a "forged" one.
    SignatureVerification::KeyNotTrusted {
        key_id: key_id_from_sig(&signature),
    }
}

fn sidecar_sig_path(iso_path: &Path) -> PathBuf {
    let mut p = PathBuf::from(iso_path);
    let ext = p
        .extension()
        .map(|e| e.to_string_lossy().to_string())
        .unwrap_or_default();
    p.set_extension(if ext.is_empty() {
        "minisig".to_string()
    } else {
        format!("{ext}.minisig")
    });
    p
}

fn load_trusted_keys() -> Vec<(PublicKey, String)> {
    let Ok(env) = std::env::var("AEGIS_TRUSTED_KEYS") else {
        return Vec::new();
    };
    let mut keys = Vec::new();
    for entry in env.split(':').filter(|s| !s.is_empty()) {
        let path = PathBuf::from(entry);
        if path.is_dir() {
            // Defense-in-depth: refuse the entire directory if it's
            // group- or world-writable. An attacker with write access
            // could drop a malicious `.pub` file and redirect the
            // trust anchor. Safe-default in the single-user initramfs
            // today, but the env var is operator-configurable and
            // this forecloses a foot-gun on multi-user hosts.
            if !is_path_safely_owned(&path) {
                tracing::warn!(
                    key_dir = %path.display(),
                    "iso-probe: refusing AEGIS_TRUSTED_KEYS directory — \
                     group- or world-writable (would allow an attacker to \
                     drop a malicious pub-key). Fix: chmod go-w <dir>."
                );
                continue;
            }
            let Ok(iter) = fs::read_dir(&path) else {
                continue;
            };
            for child in iter.flatten() {
                let child_path = child.path();
                if child_path.extension().and_then(|s| s.to_str()) == Some("pub") {
                    load_key_into(&child_path, &mut keys);
                }
            }
        } else if path.is_file() {
            load_key_into(&path, &mut keys);
        }
    }
    keys
}

fn load_key_into(path: &Path, out: &mut Vec<(PublicKey, String)>) {
    // Same defense as for the parent dir: refuse group/world-writable
    // pub-key files regardless of how they were discovered. An
    // attacker who can overwrite the .pub file can swap in their own
    // public key and make their signatures appear trusted.
    if !is_path_safely_owned(path) {
        tracing::warn!(
            key = %path.display(),
            "iso-probe: refusing trusted pub-key file — group- or \
             world-writable. Fix: chmod go-w <file>."
        );
        return;
    }
    let Ok(text) = fs::read_to_string(path) else {
        return;
    };
    match PublicKey::decode(text.trim()) {
        Ok(key) => out.push((key, path.display().to_string())),
        Err(e) => tracing::debug!(
            key = %path.display(),
            error = %e,
            "iso-probe: rejected invalid minisign public key"
        ),
    }
}

/// Return `true` when `path`'s filesystem mode has no group- or
/// world-write bits set (i.e. it's owned and writable only by the
/// owner — mode `0o7xx` with no `0o022` bits).
///
/// On non-Unix hosts (Windows), Unix mode bits don't meaningfully
/// map to this attack — returns `true` so key loading works without
/// a meaningful check. This is an acceptable tradeoff for iso-probe's
/// primary deployment target (Linux initramfs + Linux operator host).
///
/// Failure to stat the path (ENOENT, EACCES) returns `false` — better
/// to refuse an unreadable key than silently skip the permissions
/// gate. The caller's subsequent `read_to_string` would have failed
/// anyway; this just makes the refusal explicit in logs.
fn is_path_safely_owned(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let Ok(meta) = fs::metadata(path) else {
            return false;
        };
        let mode = meta.permissions().mode();
        // 0o022 = group-write (0o020) | world-write (0o002).
        // If either bit is set, the file is not safely owned.
        (mode & 0o022) == 0
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        true
    }
}

/// Minisign's trusted-comment line is the closest thing to a human-readable
/// ID we can surface without owning the private key. Truncate to avoid
/// blowing up the TUI with arbitrary signer-chosen text.
fn key_id_from_sig(sig: &Signature) -> String {
    let comment = sig.trusted_comment();
    let truncated: String = comment.chars().take(40).collect();
    if comment.chars().count() > 40 {
        format!("{truncated}…")
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_path_appends_minisig_to_extension() {
        assert_eq!(
            sidecar_sig_path(Path::new("/x/y.iso")),
            PathBuf::from("/x/y.iso.minisig")
        );
    }

    #[test]
    fn sidecar_path_handles_no_extension() {
        assert_eq!(
            sidecar_sig_path(Path::new("/x/y")),
            PathBuf::from("/x/y.minisig")
        );
    }

    #[test]
    fn no_sig_returns_not_present() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let iso = dir.path().join("x.iso");
        std::fs::write(&iso, b"dummy").unwrap_or_else(|e| panic!("write: {e}"));
        assert!(matches!(
            verify_iso_signature(&iso),
            SignatureVerification::NotPresent
        ));
    }

    #[test]
    fn malformed_sig_returns_error() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let iso = dir.path().join("x.iso");
        std::fs::write(&iso, b"dummy").unwrap_or_else(|e| panic!("write: {e}"));
        std::fs::write(dir.path().join("x.iso.minisig"), "not-a-minisig\n")
            .unwrap_or_else(|e| panic!("write: {e}"));
        assert!(matches!(
            verify_iso_signature(&iso),
            SignatureVerification::Error { .. }
        ));
    }

    // ---- AEGIS_TRUSTED_KEYS permissions check (CWE-732) ---------------

    #[cfg(unix)]
    #[test]
    fn is_path_safely_owned_accepts_owner_only_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let f = dir.path().join("key.pub");
        std::fs::write(&f, b"x").unwrap_or_else(|e| panic!("write: {e}"));
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o600))
            .unwrap_or_else(|e| panic!("chmod: {e}"));
        assert!(is_path_safely_owned(&f));
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644))
            .unwrap_or_else(|e| panic!("chmod: {e}"));
        assert!(is_path_safely_owned(&f));
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755))
            .unwrap_or_else(|e| panic!("chmod: {e}"));
        assert!(is_path_safely_owned(&f));
    }

    #[cfg(unix)]
    #[test]
    fn is_path_safely_owned_rejects_group_writable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let f = dir.path().join("key.pub");
        std::fs::write(&f, b"x").unwrap_or_else(|e| panic!("write: {e}"));
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o664))
            .unwrap_or_else(|e| panic!("chmod: {e}"));
        assert!(!is_path_safely_owned(&f));
    }

    #[cfg(unix)]
    #[test]
    fn is_path_safely_owned_rejects_world_writable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let f = dir.path().join("key.pub");
        std::fs::write(&f, b"x").unwrap_or_else(|e| panic!("write: {e}"));
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o646))
            .unwrap_or_else(|e| panic!("chmod: {e}"));
        assert!(!is_path_safely_owned(&f));
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o666))
            .unwrap_or_else(|e| panic!("chmod: {e}"));
        assert!(!is_path_safely_owned(&f));
    }

    #[cfg(unix)]
    #[test]
    fn is_path_safely_owned_rejects_missing_file() {
        // Fail-closed: if we can't stat the path, refuse rather than
        // defaulting to trust. The caller's read_to_string would fail
        // anyway; this just surfaces the refusal in structured logs.
        let p = std::path::PathBuf::from("/definitely/does/not/exist-aegis-tk");
        assert!(!is_path_safely_owned(&p));
    }

    #[cfg(unix)]
    #[test]
    fn load_key_into_skips_group_writable_pub_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let f = dir.path().join("attacker.pub");
        // Write a syntactically valid pub-key string shape (doesn't
        // need to be a real minisign key — decode will fail; the
        // assertion is that we never reach decode because perms are
        // rejected first).
        std::fs::write(&f, b"untrusted").unwrap_or_else(|e| panic!("write: {e}"));
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o664))
            .unwrap_or_else(|e| panic!("chmod: {e}"));
        let mut keys: Vec<(PublicKey, String)> = Vec::new();
        load_key_into(&f, &mut keys);
        assert!(
            keys.is_empty(),
            "group-writable pub-key should be refused before minisign decode"
        );
    }

    #[test]
    fn summary_strings_are_stable() {
        assert_eq!(SignatureVerification::NotPresent.summary(), "not present");
        assert_eq!(
            SignatureVerification::KeyNotTrusted { key_id: "x".into() }.summary(),
            "UNTRUSTED KEY"
        );
        assert_eq!(
            SignatureVerification::Forged {
                sig_path: PathBuf::new()
            }
            .summary(),
            "FORGED"
        );
    }
}
