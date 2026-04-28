// SPDX-License-Identifier: MIT OR Apache-2.0

//! Parse SHA-256 / SHA-512 sums files of the shape vendors publish.

// Used by the verify-dispatch path inside `fetch_catalog_entry`,
// wired in after the HTTPS downloader commit.
#![allow(dead_code)]
//!
//! Two layouts are common:
//!
//! 1. The `sha256sum -b` format — `<hex>  <filename>` per line. One
//!    line per ISO. Used by Debian, Ubuntu, Kali, openSUSE,
//!    Pop!\_OS, Linux Mint, `GParted`.
//! 2. Per-ISO sidecar — single line, often `<hex>  <filename>`.
//!    Used by Alpine, Manjaro, MX, `SystemRescue`.
//! 3. Fedora/AlmaLinux/Rocky `CHECKSUM` — multi-line format with
//!    `SHA256 (<filename>) = <hex>` syntax (the BSD-style format
//!    `coreutils` accepts via `--tag`). Embedded inside the
//!    clearsigned envelope.
//!
//! The parser tolerates leading `*` (binary) / single-space /
//! double-space separators and the BSD-style. Returns the first
//! sha256 line that matches `iso_filename`. SHA-512 lines are
//! ignored — Debian's SHA512SUMS is currently the only catalog
//! entry that publishes 512-bit digests, and our SHA-256 path
//! doesn't accept 512-bit hashes regardless. (When that
//! cross-distro case bites, add a parallel sha512 module.)

use crate::FetchError;

/// Parsed `(filename, lowercase-hex digest)` pairs.
type Pair<'a> = (&'a str, String);

/// Find the SHA-256 hex digest for `iso_filename` inside a sums
/// file (already authenticated by the caller). Returns the
/// lowercase-hex digest as a `String`. Errors with
/// [`FetchError::IsoNotInSums`] when no matching line is present
/// or [`FetchError::MalformedSums`] when no parseable sha256 lines
/// exist at all.
///
/// # Errors
///
/// See [`FetchError::IsoNotInSums`] / [`FetchError::MalformedSums`].
pub(crate) fn find_iso_sha256(sums_text: &str, iso_filename: &str) -> Result<String, FetchError> {
    let mut any_sha256 = false;
    for (name, hex) in iter_sha256_lines(sums_text) {
        any_sha256 = true;
        if name == iso_filename {
            return Ok(hex);
        }
    }
    if any_sha256 {
        Err(FetchError::IsoNotInSums {
            iso: iso_filename.to_string(),
        })
    } else {
        Err(FetchError::MalformedSums)
    }
}

fn iter_sha256_lines(sums_text: &str) -> impl Iterator<Item = Pair<'_>> {
    sums_text.lines().filter_map(parse_sha256_line)
}

fn parse_sha256_line(line: &str) -> Option<Pair<'_>> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }

    // BSD-style: "SHA256 (<filename>) = <hex>"
    if let Some(rest) = line.strip_prefix("SHA256 (") {
        let close = rest.find(')')?;
        let filename = &rest[..close];
        let after = rest[close..].strip_prefix(") = ")?;
        if is_sha256_hex(after) {
            return Some((filename, after.to_lowercase()));
        }
        return None;
    }

    // GNU coreutils format: "<hex>  <filename>" (two spaces) or
    // "<hex> *<filename>" (binary mode marker). Accept either.
    let mut parts = line.splitn(2, char::is_whitespace);
    let hex = parts.next()?;
    if !is_sha256_hex(hex) {
        return None;
    }
    let rest = parts.next()?.trim_start();
    let filename = rest.strip_prefix('*').unwrap_or(rest);
    Some((filename, hex.to_lowercase()))
}

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]
    use super::*;

    const FAKE_HEX: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[test]
    fn parses_gnu_two_space_format() {
        let body = format!("{FAKE_HEX}  ubuntu.iso\n");
        let r = find_iso_sha256(&body, "ubuntu.iso").expect("parse");
        assert_eq!(r, FAKE_HEX);
    }

    #[test]
    fn parses_gnu_binary_marker_format() {
        let body = format!("{FAKE_HEX} *alpine.iso\n");
        let r = find_iso_sha256(&body, "alpine.iso").expect("parse");
        assert_eq!(r, FAKE_HEX);
    }

    #[test]
    fn parses_bsd_tag_format() {
        let body = format!("SHA256 (Fedora-Server-dvd-x86_64-43-1.6.iso) = {FAKE_HEX}\n");
        let r = find_iso_sha256(&body, "Fedora-Server-dvd-x86_64-43-1.6.iso").expect("parse");
        assert_eq!(r, FAKE_HEX);
    }

    #[test]
    fn returns_specific_iso_among_many() {
        let other = "1234567890abcdef".repeat(4); // 64-char fake hex
        let body = format!("{other}  desktop.iso\n{FAKE_HEX}  server.iso\n{other}  netinst.iso\n");
        let r = find_iso_sha256(&body, "server.iso").expect("parse");
        assert_eq!(r, FAKE_HEX);
    }

    #[test]
    fn iso_not_in_sums_when_filename_absent() {
        let body = format!("{FAKE_HEX}  some-other.iso\n");
        let err = find_iso_sha256(&body, "missing.iso").expect_err("missing");
        assert!(matches!(err, FetchError::IsoNotInSums { .. }));
    }

    #[test]
    fn malformed_sums_when_no_sha256_lines_at_all() {
        let body = "this is a regular text file, not a sums file\n";
        let err = find_iso_sha256(body, "x.iso").expect_err("malformed");
        assert!(matches!(err, FetchError::MalformedSums));
    }

    #[test]
    fn ignores_comments_and_blank_lines() {
        let body = format!("# comment\n\n{FAKE_HEX}  alpine.iso\n# another comment\n");
        let r = find_iso_sha256(&body, "alpine.iso").expect("parse");
        assert_eq!(r, FAKE_HEX);
    }

    #[test]
    fn ignores_sha512_lines_so_a_pure_sha512_file_is_malformed() {
        // Debian's SHA512SUMS — 128-char hex, not 64. We're a
        // sha256 finder; a pure sha512 file should report
        // MalformedSums so the caller can decide what to do.
        let sha512 = "0".repeat(128);
        let body = format!("{sha512}  debian.iso\n");
        let err = find_iso_sha256(&body, "debian.iso").expect_err("malformed");
        assert!(matches!(err, FetchError::MalformedSums));
    }

    #[test]
    fn rejects_garbage_in_hex_field() {
        let bad = "Z".repeat(64);
        let body = format!("{bad}  alpine.iso\n");
        let err = find_iso_sha256(&body, "alpine.iso").expect_err("malformed");
        assert!(matches!(err, FetchError::MalformedSums));
    }
}
