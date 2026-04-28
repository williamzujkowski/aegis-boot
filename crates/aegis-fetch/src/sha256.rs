// SPDX-License-Identifier: MIT OR Apache-2.0

//! Streaming SHA-256 over a file on disk.
//!
//! [`hash_file`] reads the file in 1 MiB chunks and emits a
//! progress callback per chunk so the caller can render a
//! progress bar without loading the entire ISO into memory.

// Used by `fetch_catalog_entry`'s VerifyingHash phase, wired in
// after the HTTPS downloader commit.
#![allow(dead_code)]

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::FetchError;

/// One-MiB read buffer. Big enough that syscall overhead is
/// dominated by SHA-256 + disk read; small enough that a stalled
/// disk shows up as a missed callback within ~50 ms of CPU time
/// at sustained read speeds.
const CHUNK_BYTES: usize = 1 << 20;

/// Stream-hash a file and return its lowercase-hex SHA-256 digest
/// plus total byte count. Calls `on_progress(bytes_so_far)` once
/// per [`CHUNK_BYTES`] of input so the caller can render a
/// progress bar.
///
/// # Errors
///
/// Wraps `std::io::Error` as [`FetchError::Filesystem`] with the
/// path included for operator-readable diagnostics.
pub(crate) fn hash_file(
    path: &Path,
    on_progress: &mut dyn FnMut(u64),
) -> Result<(String, u64), FetchError> {
    let file = File::open(path).map_err(|e| FetchError::Filesystem {
        detail: format!("open {} for hashing: {e}", path.display()),
    })?;
    let mut reader = BufReader::with_capacity(CHUNK_BYTES, file);
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; CHUNK_BYTES];
    let mut total: u64 = 0;
    loop {
        let n = reader.read(&mut buf).map_err(|e| FetchError::Filesystem {
            detail: format!("read {}: {e}", path.display()),
        })?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        total += n as u64;
        on_progress(total);
    }
    let digest = hasher.finalize();
    Ok((hex_lower(&digest), total))
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        // 8 + 4 = 12 hex digits per byte? No — 2 hex digits per byte.
        // {b:02x} formats as exactly 2 lowercase hex digits.
        use std::fmt::Write;
        let _ = write!(&mut s, "{b:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]
    use super::*;

    #[test]
    fn hash_empty_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("empty");
        std::fs::write(&path, b"").expect("write");
        let (hex, n) = hash_file(&path, &mut |_| {}).expect("hash");
        // SHA-256 of empty input is a well-known fixed value.
        assert_eq!(
            hex,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(n, 0);
    }

    #[test]
    fn hash_known_vector() {
        // SHA-256("abc") = ba7816bf...
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("abc");
        std::fs::write(&path, b"abc").expect("write");
        let (hex, n) = hash_file(&path, &mut |_| {}).expect("hash");
        assert_eq!(
            hex,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(n, 3);
    }

    #[test]
    fn hash_emits_progress_for_multi_chunk_input() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("big");
        // Write 2.5 MiB so we get 3 read iterations at 1 MiB each.
        let big = vec![0u8; (CHUNK_BYTES * 5) / 2];
        std::fs::write(&path, &big).expect("write");
        let mut callbacks: Vec<u64> = Vec::new();
        let (_, n) = hash_file(&path, &mut |b| callbacks.push(b)).expect("hash");
        assert_eq!(n, big.len() as u64);
        assert!(
            callbacks.len() >= 3,
            "expected multiple progress callbacks, got {callbacks:?}"
        );
        assert_eq!(*callbacks.last().expect("at least one"), n);
        // Monotonic non-decreasing.
        for w in callbacks.windows(2) {
            assert!(w[0] <= w[1], "progress regressed: {} -> {}", w[0], w[1]);
        }
    }

    #[test]
    fn hash_missing_file_is_filesystem_error() {
        let err = hash_file(
            std::path::Path::new("/nonexistent/aegis-fetch-test"),
            &mut |_| {},
        )
        .expect_err("should fail");
        assert!(matches!(err, FetchError::Filesystem { .. }));
    }
}
