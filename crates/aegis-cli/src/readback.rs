//! Post-write readback verification — read back the first N bytes of
//! a freshly-flashed device and verify the sha256 matches the source
//! image's prefix. PR1 of #244 (the `flash` command's "step 4 of 4
//! readback verify" surface).
//!
//! Catches silent USB write failures: cheap sticks sometimes accept a
//! `dd` happily, return success, and then hold zeros in the boot
//! sector. The next boot fails with a Secure Boot violation that's
//! impossible to diagnose from the rescue UI. Reading back the first
//! ~64 MB and re-checking the sha256 closes that window before the
//! operator pulls the stick.
//!
//! Wired up by `flash`: `sha256_of_first_bytes` + `DEFAULT_READBACK_BYTES`
//! are called inline from `precompute_image_prefix_hash` (pre-dd) and
//! `readback_verify_device` (post-dd). The high-level `verify_readback`
//! wrapper is kept as the library-shaped API but has no production
//! caller. See #244 for the rollout history.
//!
//! # Why bound to a prefix
//!
//! Reading back the entire device on a 30 GB stick at USB 2.0 speeds
//! is a 5+ minute wait — operators won't tolerate it. The signed-chain
//! payload (shim + grub + kernel + initramfs) lives in the first ~50 MB
//! of partition 1; rounding up to 64 MB covers it with margin. The
//! data partition isn't readback-verified because it's mutable
//! operator content (per `USB_LAYOUT.md`).

// `flash` uses `sha256_of_first_bytes` + `DEFAULT_READBACK_BYTES` inline
// and surfaces its own error strings; the high-level `verify_readback`
// helper + typed `ReadbackError` remain unused in production but are
// kept (and unit-tested) as the library-shaped API. Dead-code allow
// covers that deliberate gap until a caller wants structured errors.
#![allow(dead_code)]

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use sha2::{Digest, Sha256};

/// Re-export the default readback window from the shared constants
/// registry. See [`crate::constants::DEFAULT_READBACK_BYTES`] for
/// the rationale.
pub(crate) use crate::constants::DEFAULT_READBACK_BYTES;

/// Stream up to `n_bytes` from `reader` and return the lowercased hex
/// sha256. Stops at EOF if the reader has fewer than `n_bytes` bytes
/// (the caller decides whether that's an error — for readback against
/// a freshly-written device, EOF before `n_bytes` is itself a sign of
/// silent write failure).
///
/// Returns the actual number of bytes consumed alongside the hash so
/// the caller can detect short-reads explicitly.
///
/// # Errors
///
/// Propagates any `io::Error` raised by `reader`.
pub fn sha256_of_first_bytes<R: Read>(reader: &mut R, n_bytes: u64) -> io::Result<(String, u64)> {
    // 64 KiB chunk: large enough to keep syscall overhead negligible,
    // small enough that the working-set stays in L2 cache.
    const CHUNK: usize = 64 * 1024;
    let mut buf = vec![0u8; CHUNK];
    let mut hasher = Sha256::new();
    let mut consumed: u64 = 0;
    while consumed < n_bytes {
        // Cast through usize::try_from rather than `as` so 32-bit
        // platforms surface the impossible >4 GiB chunk size as an
        // io::Error rather than silently truncating.
        let remaining = n_bytes - consumed;
        let want_u64 = std::cmp::min(remaining, CHUNK as u64);
        let want = usize::try_from(want_u64)
            .map_err(|e| io::Error::other(format!("chunk size overflow: {e}")))?;
        let n = reader.read(&mut buf[..want])?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        consumed += n as u64;
    }
    Ok((hex::encode(hasher.finalize()), consumed))
}

/// Errors raised by `verify_readback`.
#[derive(Debug, thiserror::Error)]
pub enum ReadbackError {
    /// I/O error opening or reading the device / file.
    #[error("io: {0}")]
    Io(#[from] io::Error),
    /// The expected sha256 string was not 64 lowercase hex characters.
    #[error("expected sha256 must be 64 lowercase hex chars; got {len} chars")]
    InvalidExpectedFormat {
        /// Length of the offending input.
        len: usize,
    },
    /// The reader produced fewer than `n_bytes` bytes before EOF.
    /// Indicates a silent short-write — the freshly-flashed device
    /// has fewer bytes available than the source image.
    #[error("short read: expected {expected} bytes, got {actual}")]
    ShortRead {
        /// Bytes the caller asked for.
        expected: u64,
        /// Bytes the reader produced.
        actual: u64,
    },
    /// The readback sha256 disagreed with the expected value. The
    /// bytes on the device do not match what `dd` wrote.
    #[error("sha256 mismatch: expected {expected}, got {actual}")]
    Mismatch {
        /// Sha256 the caller passed in.
        expected: String,
        /// Sha256 computed from the readback.
        actual: String,
    },
}

/// Open `path` for reading, stream the first `n_bytes`, and verify the
/// resulting sha256 matches `expected_sha256_hex`.
///
/// # Errors
///
/// - `Io` for any underlying read failure.
/// - `InvalidExpectedFormat` if `expected_sha256_hex` isn't 64 lowercase hex chars.
/// - `ShortRead` if EOF arrives before `n_bytes` bytes have been read.
/// - `Mismatch` if the bytes on disk don't match the expected hash.
pub fn verify_readback(
    path: &Path,
    n_bytes: u64,
    expected_sha256_hex: &str,
) -> Result<(), ReadbackError> {
    if !is_valid_sha256_hex(expected_sha256_hex) {
        return Err(ReadbackError::InvalidExpectedFormat {
            len: expected_sha256_hex.len(),
        });
    }
    let mut f = File::open(path)?;
    let (actual, consumed) = sha256_of_first_bytes(&mut f, n_bytes)?;
    if consumed < n_bytes {
        return Err(ReadbackError::ShortRead {
            expected: n_bytes,
            actual: consumed,
        });
    }
    if actual != expected_sha256_hex {
        return Err(ReadbackError::Mismatch {
            expected: expected_sha256_hex.to_string(),
            actual,
        });
    }
    Ok(())
}

/// Lightweight format guard for sha256 hex strings: 64 chars, all in
/// `[0-9a-f]`. Avoids surfacing a useless "mismatch" error when the
/// caller passes the wrong shape entirely (e.g. uppercase, prefixed
/// `0x`, or a binary blob).
#[must_use]
pub fn is_valid_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use tempfile::NamedTempFile;

    /// sha256 of an empty input — used by multiple tests.
    const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    #[test]
    fn sha256_of_first_bytes_empty_reader_yields_empty_hash() {
        let mut empty = Cursor::new(Vec::<u8>::new());
        let (h, n) = sha256_of_first_bytes(&mut empty, 1024).unwrap();
        assert_eq!(h, EMPTY_SHA256);
        assert_eq!(n, 0);
    }

    #[test]
    fn sha256_of_first_bytes_reads_at_most_n_bytes() {
        // Reader has 100 bytes; ask for 50. Hash should match sha256 of
        // the first 50 bytes, NOT the full 100.
        let payload: Vec<u8> = (0..100u8).collect();
        let mut full_hash = Sha256::new();
        full_hash.update(&payload[..50]);
        let expected = hex::encode(full_hash.finalize());

        let mut cur = Cursor::new(payload);
        let (h, n) = sha256_of_first_bytes(&mut cur, 50).unwrap();
        assert_eq!(h, expected);
        assert_eq!(n, 50);
    }

    #[test]
    fn sha256_of_first_bytes_short_reader_returns_actual_consumed() {
        // Reader has 10 bytes; ask for 1024. Returns hash of all 10 +
        // consumed=10. Caller decides if 10 < 1024 is a failure.
        let payload: Vec<u8> = (0..10u8).collect();
        let mut full_hash = Sha256::new();
        full_hash.update(&payload);
        let expected = hex::encode(full_hash.finalize());

        let mut cur = Cursor::new(payload);
        let (h, n) = sha256_of_first_bytes(&mut cur, 1024).unwrap();
        assert_eq!(h, expected);
        assert_eq!(n, 10);
    }

    #[test]
    fn sha256_of_first_bytes_handles_chunk_boundary() {
        // 64 KiB + 1 byte to confirm we don't lose the trailing chunk
        // or miscount on a partial-chunk boundary.
        let mut payload = vec![0xAAu8; 64 * 1024];
        payload.push(0xBBu8);
        let mut full_hash = Sha256::new();
        full_hash.update(&payload);
        let expected = hex::encode(full_hash.finalize());

        let mut cur = Cursor::new(payload.clone());
        let (h, n) = sha256_of_first_bytes(&mut cur, payload.len() as u64).unwrap();
        assert_eq!(h, expected);
        assert_eq!(n, payload.len() as u64);
    }

    #[test]
    fn verify_readback_passes_when_hashes_match() {
        let payload: Vec<u8> = (0..=255u8).collect();
        let mut hasher = Sha256::new();
        hasher.update(&payload[..100]);
        let expected_hash = hex::encode(hasher.finalize());

        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(&payload).unwrap();
        tmp.flush().unwrap();

        verify_readback(tmp.path(), 100, &expected_hash).expect("readback should succeed");
    }

    #[test]
    fn verify_readback_returns_mismatch_on_wrong_hash() {
        let payload = vec![0xAAu8; 1024];
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(&payload).unwrap();
        tmp.flush().unwrap();

        // Anything that's a valid 64-char lowercase hex string but
        // doesn't match the actual file contents.
        let wrong_hash = "1".repeat(64);
        match verify_readback(tmp.path(), 1024, &wrong_hash) {
            Err(ReadbackError::Mismatch { expected, actual }) => {
                assert_eq!(expected, wrong_hash);
                assert!(!actual.is_empty());
                assert_eq!(actual.len(), 64);
            }
            other => panic!("expected Mismatch, got {other:?}"),
        }
    }

    #[test]
    fn verify_readback_returns_short_read_when_file_smaller_than_requested() {
        let payload = vec![0xAAu8; 100];
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(&payload).unwrap();
        tmp.flush().unwrap();

        // Expected hash is well-formed but irrelevant because we'll
        // short-read first.
        let dummy_hash = "0".repeat(64);
        match verify_readback(tmp.path(), 1024, &dummy_hash) {
            Err(ReadbackError::ShortRead { expected, actual }) => {
                assert_eq!(expected, 1024);
                assert_eq!(actual, 100);
            }
            other => panic!("expected ShortRead, got {other:?}"),
        }
    }

    #[test]
    fn verify_readback_rejects_malformed_expected_format() {
        let tmp = NamedTempFile::new().unwrap();
        // Uppercase hex — wrong format per is_valid_sha256_hex.
        let bad = "ABCDEFABCDEFABCDEFABCDEFABCDEFABCDEFABCDEFABCDEFABCDEFABCDEFABCD";
        match verify_readback(tmp.path(), 64, bad) {
            Err(ReadbackError::InvalidExpectedFormat { len }) => assert_eq!(len, 64),
            other => panic!("expected InvalidExpectedFormat, got {other:?}"),
        }
    }

    #[test]
    fn verify_readback_returns_io_error_on_missing_file() {
        let dummy_hash = "0".repeat(64);
        let nonexistent = Path::new("/nonexistent/path/aegis-boot-readback-test");
        match verify_readback(nonexistent, 64, &dummy_hash) {
            Err(ReadbackError::Io(_)) => {}
            other => panic!("expected Io error, got {other:?}"),
        }
    }

    #[test]
    fn is_valid_sha256_hex_accepts_canonical_form() {
        assert!(is_valid_sha256_hex(&"0".repeat(64)));
        assert!(is_valid_sha256_hex(&"f".repeat(64)));
        assert!(is_valid_sha256_hex(EMPTY_SHA256));
    }

    #[test]
    fn is_valid_sha256_hex_rejects_wrong_length() {
        assert!(!is_valid_sha256_hex(""));
        assert!(!is_valid_sha256_hex(&"0".repeat(63)));
        assert!(!is_valid_sha256_hex(&"0".repeat(65)));
    }

    #[test]
    fn is_valid_sha256_hex_rejects_uppercase() {
        assert!(!is_valid_sha256_hex(&"A".repeat(64)));
    }

    #[test]
    fn is_valid_sha256_hex_rejects_non_hex_chars() {
        // Length 64 but contains 'g' (not in [0-9a-f]).
        let bad = "g".repeat(64);
        assert!(!is_valid_sha256_hex(&bad));
        // Length 64 but contains the 0x prefix marker.
        let prefixed = format!("0x{}", "0".repeat(62));
        assert!(!is_valid_sha256_hex(&prefixed));
    }

    #[test]
    fn default_readback_bytes_constant_is_sized_for_signed_chain() {
        // Lock the default. The signed-chain payload (shim + grub +
        // kernel + initramfs) is ~50 MB; 64 MB gives margin without
        // pushing the readback over 10s on slow USB.
        assert_eq!(DEFAULT_READBACK_BYTES, 64 * 1024 * 1024);
    }
}
