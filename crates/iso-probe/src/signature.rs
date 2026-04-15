//! ISO hash verification against sibling checksum files.
//!
//! Most distros publish their ISOs alongside a `SHA256SUMS` file (or per-ISO
//! `<iso>.sha256`). This module looks for either form, parses the expected
//! hash, computes the actual hash of the ISO bytes, and reports the result.
//!
//! **This is not crypto-grade signing.** A checksum file sitting next to the
//! ISO proves *nothing* about authenticity — only that whoever handed you the
//! ISO also handed you a matching checksum. Real provenance requires a
//! signed checksum file (e.g. `SHA256SUMS.gpg`) verified against a trusted
//! key. That's a separate follow-up; tracked in #24 under the "sigstore /
//! minisign" line item.
//!
//! What checksum verification *does* buy us:
//! - Detects ISO corruption in transit (flipped bits, truncated downloads).
//! - Detects accidental tampering on a shared USB stick.
//! - Gives the TUI a preflight warning before kexec when the ISO doesn't
//!   match its published checksum.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Outcome of a hash verification attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HashVerification {
    /// Computed hash matched the expected value.
    Verified {
        /// The hex-encoded SHA-256 that both sides agreed on.
        digest: String,
        /// Which sibling file supplied the expected value.
        source: String,
    },
    /// Computed hash did NOT match the expected value.
    Mismatch {
        /// What was computed over the ISO bytes.
        actual: String,
        /// What the sibling file claimed.
        expected: String,
        /// Which sibling file supplied the expected value.
        source: String,
    },
    /// No sibling checksum file was found.
    NotPresent,
}

impl HashVerification {
    /// Short user-facing string suitable for the TUI confirm screen.
    #[must_use]
    pub fn summary(&self) -> &'static str {
        match self {
            Self::Verified { .. } => "verified",
            Self::Mismatch { .. } => "MISMATCH",
            Self::NotPresent => "not present",
        }
    }
}

/// Verify `iso_path` against any sibling `.sha256` / `SHA256SUMS` file.
///
/// Search order:
/// 1. `<iso>.sha256` (single-line: `<hex>  <filename>` or just `<hex>`)
/// 2. `SHA256SUMS` in the same directory (find the line matching the ISO's
///    basename)
///
/// First match wins. If neither exists, returns [`HashVerification::NotPresent`].
///
/// # Errors
///
/// Returns [`std::io::Error`] on failure to read the ISO itself. Missing or
/// unreadable sibling files are handled as `NotPresent`, not errors.
pub fn verify_iso_hash(iso_path: &Path) -> std::io::Result<HashVerification> {
    verify_iso_hash_with_progress(iso_path, |_, _| {})
}

/// Progress-reporting variant of [`verify_iso_hash`] for interactive
/// operator-initiated re-verification (#89). Calls `on_progress(bytes_read,
/// total_bytes)` periodically during the hash computation so the caller can
/// render a progress bar. No guarantees on tick frequency — fast enough for
/// a human-perceivable bar (~10 Hz on modern `NVMe`).
///
/// # Errors
///
/// Same as [`verify_iso_hash`].
pub fn verify_iso_hash_with_progress<F>(
    iso_path: &Path,
    mut on_progress: F,
) -> std::io::Result<HashVerification>
where
    F: FnMut(u64, u64),
{
    let Some(expected) = find_expected_hash(iso_path) else {
        return Ok(HashVerification::NotPresent);
    };
    let total = std::fs::metadata(iso_path).map(|m| m.len()).unwrap_or(0);
    let actual = sha256_of_file_with_progress(iso_path, total, &mut on_progress)?;
    if actual == expected.hash.to_ascii_lowercase() {
        Ok(HashVerification::Verified {
            digest: actual,
            source: expected.source,
        })
    } else {
        Ok(HashVerification::Mismatch {
            actual,
            expected: expected.hash,
            source: expected.source,
        })
    }
}

struct ExpectedHash {
    hash: String,
    source: String,
}

fn find_expected_hash(iso_path: &Path) -> Option<ExpectedHash> {
    // 1. <iso>.sha256 sibling.
    let mut per_iso = PathBuf::from(iso_path);
    let ext = per_iso
        .extension()
        .map(|e| e.to_string_lossy().to_string())
        .unwrap_or_default();
    per_iso.set_extension(if ext.is_empty() {
        "sha256".to_string()
    } else {
        format!("{ext}.sha256")
    });
    if let Ok(body) = std::fs::read_to_string(&per_iso) {
        if let Some(hash) = parse_sha256sum_line(body.trim()) {
            return Some(ExpectedHash {
                hash,
                source: per_iso.display().to_string(),
            });
        }
    }

    // 2. SHA256SUMS in the same dir.
    let dir = iso_path.parent()?;
    let sums_path = dir.join("SHA256SUMS");
    let sums = std::fs::read_to_string(&sums_path).ok()?;
    let basename = iso_path.file_name()?.to_string_lossy().to_string();
    for line in sums.lines() {
        if let Some((hash, fname)) = parse_sha256sums_line(line) {
            if fname == basename {
                return Some(ExpectedHash {
                    hash,
                    source: sums_path.display().to_string(),
                });
            }
        }
    }
    None
}

/// Parse a single sha256 line of either form:
///   - just the hex digest
///   - `<hex>  <filename>` (double-space per GNU coreutils)
fn parse_sha256sum_line(line: &str) -> Option<String> {
    let token = line.split_whitespace().next()?;
    if is_sha256_hex(token) {
        Some(token.to_ascii_lowercase())
    } else {
        None
    }
}

/// Parse one line of a GNU-style SHA256SUMS file: `<hex>  <filename>` or
/// `<hex> *<filename>` (binary mode).
fn parse_sha256sums_line(line: &str) -> Option<(String, String)> {
    let mut parts = line.splitn(2, char::is_whitespace);
    let hash = parts.next()?;
    if !is_sha256_hex(hash) {
        return None;
    }
    let rest = parts.next()?.trim_start().trim_start_matches('*');
    Some((hash.to_ascii_lowercase(), rest.to_string()))
}

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Streaming SHA-256 with periodic callback into `on_progress(bytes_read,
/// total)`. Tick rate capped at ~10 Hz so the callback doesn't dominate
/// CPU on fast storage. (#89)
fn sha256_of_file_with_progress(
    path: &Path,
    total: u64,
    on_progress: &mut dyn FnMut(u64, u64),
) -> std::io::Result<String> {
    use std::time::{Duration, Instant};
    let file = File::open(path)?;
    let mut reader = BufReader::with_capacity(1 << 20, file);
    let mut hasher = Sha256::new();
    // Heap-allocated 64 KiB buffer — too large to sit on the stack per
    // clippy::large_stack_arrays.
    let mut buf = vec![0u8; 65_536];
    let mut bytes = 0u64;
    let mut last_tick = Instant::now();
    let tick_interval = Duration::from_millis(100);
    // Emit an initial "starting" tick so the progress bar renders
    // immediately on slow storage.
    on_progress(0, total);
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        bytes += n as u64;
        if last_tick.elapsed() >= tick_interval {
            on_progress(bytes, total);
            last_tick = Instant::now();
        }
    }
    on_progress(bytes, total);
    Ok(hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_hex_tokens() {
        assert!(parse_sha256sum_line("not-a-hash").is_none());
        assert!(parse_sha256sum_line("").is_none());
    }

    #[test]
    fn accepts_bare_hex_digest() {
        let hex = "a".repeat(64);
        assert_eq!(parse_sha256sum_line(&hex), Some(hex));
    }

    #[test]
    fn accepts_hex_with_filename() {
        let hex = "1".repeat(64);
        let line = format!("{hex}  some.iso");
        assert_eq!(parse_sha256sum_line(&line), Some(hex));
    }

    #[test]
    fn sums_line_parses_name() {
        let hex = "b".repeat(64);
        let line = format!("{hex}  test.iso");
        let (h, name) = parse_sha256sums_line(&line).unwrap_or_else(|| panic!("must parse"));
        assert_eq!(h, hex);
        assert_eq!(name, "test.iso");
    }

    #[test]
    fn sums_line_accepts_binary_star() {
        let hex = "c".repeat(64);
        let line = format!("{hex} *test.iso");
        let (_, name) = parse_sha256sums_line(&line).unwrap_or_else(|| panic!("must parse"));
        assert_eq!(name, "test.iso");
    }

    #[test]
    fn sums_line_rejects_bad_hash() {
        assert!(parse_sha256sums_line("short  test.iso").is_none());
    }

    #[test]
    fn summary_strings_are_stable() {
        let v = HashVerification::Verified {
            digest: "x".into(),
            source: "y".into(),
        };
        assert_eq!(v.summary(), "verified");
        assert_eq!(HashVerification::NotPresent.summary(), "not present");
    }

    #[test]
    fn verify_returns_not_present_when_no_sibling() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let iso = dir.path().join("x.iso");
        std::fs::write(&iso, b"dummy").unwrap_or_else(|e| panic!("write: {e}"));
        let result = verify_iso_hash(&iso).unwrap_or_else(|e| panic!("io: {e}"));
        assert!(matches!(result, HashVerification::NotPresent));
    }

    #[test]
    fn verify_detects_correct_hash_from_per_iso_sidecar() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let iso = dir.path().join("x.iso");
        let payload = b"hello world";
        std::fs::write(&iso, payload).unwrap_or_else(|e| panic!("write iso: {e}"));
        // Precomputed SHA-256 of "hello world".
        let hex = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
        std::fs::write(dir.path().join("x.iso.sha256"), hex)
            .unwrap_or_else(|e| panic!("write sidecar: {e}"));
        let result = verify_iso_hash(&iso).unwrap_or_else(|e| panic!("io: {e}"));
        match result {
            HashVerification::Verified { digest, .. } => assert_eq!(digest, hex),
            other => panic!("expected Verified, got {other:?}"),
        }
    }

    #[test]
    fn verify_detects_mismatch_from_sums_file() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let iso = dir.path().join("x.iso");
        std::fs::write(&iso, b"hello world").unwrap_or_else(|e| panic!("write iso: {e}"));
        let wrong = "0".repeat(64);
        let sums = format!("{wrong}  x.iso\n");
        std::fs::write(dir.path().join("SHA256SUMS"), sums)
            .unwrap_or_else(|e| panic!("write sums: {e}"));
        let result = verify_iso_hash(&iso).unwrap_or_else(|e| panic!("io: {e}"));
        match result {
            HashVerification::Mismatch {
                actual, expected, ..
            } => {
                assert_eq!(expected, wrong);
                assert_ne!(actual, wrong);
            }
            other => panic!("expected Mismatch, got {other:?}"),
        }
    }

    #[test]
    fn verify_prefers_per_iso_over_sums() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let iso = dir.path().join("x.iso");
        std::fs::write(&iso, b"hello world").unwrap_or_else(|e| panic!("write iso: {e}"));
        // Correct hash in per-iso sidecar.
        let correct = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
        std::fs::write(dir.path().join("x.iso.sha256"), correct)
            .unwrap_or_else(|e| panic!("write sidecar: {e}"));
        // Wrong hash in SHA256SUMS — must be ignored because sidecar wins.
        let wrong = "0".repeat(64);
        std::fs::write(dir.path().join("SHA256SUMS"), format!("{wrong}  x.iso\n"))
            .unwrap_or_else(|e| panic!("write sums: {e}"));
        let result = verify_iso_hash(&iso).unwrap_or_else(|e| panic!("io: {e}"));
        assert!(
            matches!(result, HashVerification::Verified { .. }),
            "per-iso sidecar must win over SHA256SUMS"
        );
    }
}
