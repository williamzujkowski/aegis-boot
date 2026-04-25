// SPDX-License-Identifier: MIT OR Apache-2.0

//! Shared utility helpers for the aegis-boot workspace (#556 proof-of-concept).
//!
//! See `README.md` for the scope policy. Tiny pure helpers only: no
//! I/O, no domain types, no third-party deps. The crate is intended as
//! the cheap-to-depend-on home for cross-cutting primitives that
//! were previously duplicated across `aegis-cli` and `rescue-tui`.
//!
//! ## What lives here today
//!
//! - [`short_hex`] — UTF-8-safe hex-digest truncation for human-readable
//!   display strings (replaces 3 near-identical local impls in
//!   `aegis-cli/src/inventory.rs`, `rescue-tui/src/render.rs`,
//!   `rescue-tui/src/verdict.rs`).
//! - [`humanize_bytes`] — `u64` bytes → `B`/`KiB`/`MiB`/`GiB` ladder
//!   (replaces 2 simpler 2-level impls + harmonizes with the 4-level
//!   impl in rescue-tui's render module).
//!
//! ## What's deliberately NOT here
//!
//! - Tracing subscriber setup — only `rescue-tui` uses tracing; not
//!   duplicated.
//! - Catalog's MiB-input `humanize` — different input concept (MiB,
//!   not bytes); kept local at the call site.
//! - rescue-tui's `Option<u64>`-input `humanize_size` — wrapper around
//!   `humanize_bytes` at the call site; one line of glue.

#![forbid(unsafe_code)]

/// Truncate a hex digest to ≤14 visible chars: keeps strings that are
/// already short verbatim, otherwise renders the first 12 chars
/// followed by an ellipsis (`…`).
///
/// UTF-8-safe: if the prefix would split a multi-byte codepoint we walk
/// back to the nearest character boundary. In practice all callers pass
/// ASCII hex digests so the boundary walk is a defensive no-op, but the
/// safety preserves the invariant promised by `&str` slicing.
///
/// # Examples
///
/// ```
/// assert_eq!(aegis_core::short_hex("deadbeef"), "deadbeef");
/// assert_eq!(
///     aegis_core::short_hex("abcdef0123456789abcdef0123456789"),
///     "abcdef012345…"
/// );
/// ```
#[must_use]
pub fn short_hex(s: &str) -> String {
    if s.len() <= 14 {
        return s.to_string();
    }
    let mut end = 12;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

const KIB: u64 = 1024;
const MIB: u64 = KIB * 1024;
const GIB: u64 = MIB * 1024;

/// Format a byte count as a human-readable string with binary
/// (1024-based) units. Returns `"42 B"` / `"3.5 KiB"` / `"12.7 MiB"` /
/// `"4.50 GiB"` depending on magnitude.
///
/// Precision intentionally varies by unit: GiB gets 2 decimals (matters
/// at human-perceived granularity), MiB gets 1 decimal (rounds cleanly),
/// KiB rounds to integer (sub-KiB precision is noise), and bytes are
/// printed raw.
///
/// # Examples
///
/// ```
/// assert_eq!(aegis_core::humanize_bytes(0), "0 B");
/// assert_eq!(aegis_core::humanize_bytes(1023), "1023 B");
/// assert_eq!(aegis_core::humanize_bytes(2048), "2 KiB");
/// assert_eq!(aegis_core::humanize_bytes(3 * 1024 * 1024 + 100 * 1024), "3.1 MiB");
/// ```
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn humanize_bytes(bytes: u64) -> String {
    if bytes >= GIB {
        format!("{:.2} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.0} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn short_hex_passes_short_digests_verbatim() {
        assert_eq!(short_hex("deadbeef"), "deadbeef");
        // Boundary: exactly 14 chars stays as-is.
        assert_eq!(short_hex("0123456789abcd"), "0123456789abcd");
    }

    #[test]
    fn short_hex_truncates_long_digests_with_ellipsis() {
        let full = "a".repeat(64);
        let out = short_hex(&full);
        assert!(out.ends_with('…'));
        // Twelve 'a' chars before the ellipsis.
        assert_eq!(out.chars().filter(|c| *c == 'a').count(), 12);
    }

    /// Defensive: ensure we don't panic on multi-byte chars. Hex
    /// digests are ASCII in practice, but the safety is contract.
    #[test]
    fn short_hex_handles_multibyte_at_boundary() {
        // 4-byte emoji — putting one near the 12-byte cut forces the
        // boundary walk-back path.
        let s = "abc🎉defghijkl-extra-padding-to-exceed-14-chars";
        let out = short_hex(s);
        // Just assert no panic + ends with ellipsis.
        assert!(out.ends_with('…'), "got: {out}");
    }

    #[test]
    fn humanize_bytes_b_unit() {
        assert_eq!(humanize_bytes(0), "0 B");
        assert_eq!(humanize_bytes(42), "42 B");
        assert_eq!(humanize_bytes(1023), "1023 B");
    }

    #[test]
    fn humanize_bytes_kib_unit() {
        assert_eq!(humanize_bytes(1024), "1 KiB");
        assert_eq!(humanize_bytes(2048), "2 KiB");
        // 1.5 KiB rounds to 2 (KiB uses {:.0}).
        assert_eq!(humanize_bytes(1024 + 512), "2 KiB");
    }

    #[test]
    fn humanize_bytes_mib_unit() {
        assert_eq!(humanize_bytes(1024 * 1024), "1.0 MiB");
        assert_eq!(humanize_bytes(3 * 1024 * 1024 + 100 * 1024), "3.1 MiB");
    }

    #[test]
    fn humanize_bytes_gib_unit() {
        assert_eq!(humanize_bytes(1024 * 1024 * 1024), "1.00 GiB");
        // ~30 GB stick.
        let n = 32_010_928_128_u64;
        let s = humanize_bytes(n);
        assert!(s.starts_with("29."), "got: {s}");
        assert!(s.ends_with(" GiB"), "got: {s}");
    }

    /// Boundary: exactly at the unit threshold, the larger unit wins.
    #[test]
    fn humanize_bytes_unit_boundaries() {
        assert_eq!(humanize_bytes(KIB), "1 KiB");
        assert_eq!(humanize_bytes(MIB), "1.0 MiB");
        assert_eq!(humanize_bytes(GIB), "1.00 GiB");
    }
}
