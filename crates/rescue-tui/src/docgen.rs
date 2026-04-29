// SPDX-License-Identifier: MIT OR Apache-2.0

//! Programmatic Markdown generation for the trust-tier table and
//! keybinding reference — derived from the same [`TrustVerdict`]
//! enum and [`KEYBINDINGS`] registry the rescue-tui itself uses, so
//! published docs never drift from the code.
//!
//! Consumed by the `tiers-docgen` binary (#462), which writes marker
//! regions in `docs/HOW_IT_WORKS.md`, `docs/TOUR.md`, and
//! `crates/rescue-tui/README.md`.
//!
//! ## Marker format
//!
//! Target docs contain marker pairs identical to the
//! [`constants-docgen`](../../../aegis-cli/src/bin/constants_docgen.rs)
//! pattern (Phase 2 of #286):
//!
//! ```text
//! <!-- tiers:BEGIN:TRUST_TIER_TABLE -->
//! | Tier | Verdict | ... |
//! <!-- tiers:END:TRUST_TIER_TABLE -->
//! ```
//!
//! The marker tags themselves are preserved verbatim — only the body
//! between them is rewritten.
//!
//! [`TrustVerdict`]: crate::verdict::TrustVerdict
//! [`KEYBINDINGS`]: crate::keybindings::KEYBINDINGS

use std::fmt::Write as _;

use crate::keybindings::{KEYBINDINGS, ScreenKind};
use crate::verdict::TrustVerdict;

/// Render the 6-tier trust-tier table as a Markdown table.
/// Covers every [`TrustVerdict`] variant, including the ones carrying
/// payload data (fixtures use placeholder payloads for docs).
#[must_use]
pub fn render_tier_table() -> String {
    let mut out = String::new();
    out.push_str("| Tier | Verdict             | Glyph | Bootable | Meaning                                    |\n");
    out.push_str("| ---- | ------------------- | ----- | -------- | ------------------------------------------ |\n");
    for (i, v) in tier_variants().into_iter().enumerate() {
        let bootable = if v.is_bootable() { "yes" } else { "**no**" };
        let _ = writeln!(
            out,
            "| {tier:<4} | {label:<19} | `{glyph}` | {bootable:<8} | {reason:<42} |",
            tier = i + 1,
            label = v.label(),
            glyph = v.glyph(),
            bootable = bootable,
            reason = tier_short_meaning(&v)
        );
    }
    out
}

/// Canonical ordering of the 6 `TrustVerdict` variants (tier 1 → 6).
/// Constructed with placeholder payloads for variants that carry
/// data — the docgen surface only looks at label/color/glyph/
/// bootable so payload content doesn't affect the rendered table.
fn tier_variants() -> Vec<TrustVerdict> {
    vec![
        TrustVerdict::OperatorAttested,
        TrustVerdict::BareUnverified,
        TrustVerdict::KeyNotTrusted,
        TrustVerdict::ParseFailed {
            reason: String::new(),
        },
        TrustVerdict::SecureBootBlocked {
            reason: String::new(),
        },
        TrustVerdict::HashMismatch {
            expected: String::new(),
            actual: String::new(),
            source: String::new(),
        },
    ]
}

/// Short (≤42 char) English blurb describing each tier. Kept
/// separate from `TrustVerdict::reason` (which renders the specific
/// runtime reason) because the doc table wants a stable static
/// description, not the runtime payload.
fn tier_short_meaning(v: &TrustVerdict) -> &'static str {
    match v {
        TrustVerdict::OperatorAttested => "Hash or sig verified vs trusted source",
        TrustVerdict::BareUnverified => "No sidecar — bootable with typed confirm",
        TrustVerdict::KeyNotTrusted => "Sig parses, signer untrusted",
        TrustVerdict::ParseFailed { .. } => "iso-parser couldn't extract kernel",
        TrustVerdict::SecureBootBlocked { .. } => "Kernel rejected by platform keyring",
        TrustVerdict::HashMismatch { .. } => "ISO bytes don't match declared hash",
    }
}

/// Render the keybinding reference as a Markdown table.
/// Reads from [`KEYBINDINGS`]; one row per registered binding.
#[must_use]
pub fn render_keybinding_table() -> String {
    let mut out = String::new();
    out.push_str("| Key | Screens | Pane | Filter-editing | Description |\n");
    out.push_str("| --- | ------- | ---- | -------------- | ----------- |\n");
    for k in KEYBINDINGS {
        let screens = if k.screens.is_empty() {
            "any".to_string()
        } else {
            k.screens
                .iter()
                .copied()
                .map(screen_short_name)
                .collect::<Vec<_>>()
                .join(", ")
        };
        let pane = match k.pane {
            Some(crate::state::Pane::List) => "List",
            Some(crate::state::Pane::Info) => "Info",
            None => "any",
        };
        let filter = if k.while_filter_editing { "yes" } else { "no" };
        let _ = writeln!(
            out,
            "| `{key}` | {screens} | {pane} | {filter} | {desc} |",
            key = k.key,
            screens = screens,
            pane = pane,
            filter = filter,
            desc = k.description
        );
    }
    out
}

fn screen_short_name(s: ScreenKind) -> String {
    let name = match s {
        ScreenKind::List => "List",
        ScreenKind::Confirm => "Confirm",
        ScreenKind::EditCmdline => "EditCmdline",
        ScreenKind::Error => "Error",
        ScreenKind::Verifying => "Verifying",
        ScreenKind::TrustChallenge => "TrustChallenge",
        ScreenKind::Help => "Help",
        ScreenKind::ConfirmQuit => "ConfirmQuit",
        ScreenKind::Quitting => "Quitting",
        ScreenKind::BlockedToast => "BlockedToast",
        ScreenKind::Consent => "Consent",
        ScreenKind::ConfirmDelete => "ConfirmDelete",
        ScreenKind::Network => "Network",
        ScreenKind::Catalog => "Catalog",
        ScreenKind::CatalogConfirm => "CatalogConfirm",
    };
    name.to_string()
}

/// Marker pair used to delimit regenerable regions in doc files.
/// Mirrors the constants-docgen pattern — add a new marker by
/// wrapping the target region with BEGIN/END tags carrying the same
/// name, then register the name in [`REGISTRY`].
pub struct DocMarker {
    /// Identifier appearing inside `<!-- tiers:BEGIN:NAME -->`.
    pub name: &'static str,
    /// Generator — produces the rendered body between BEGIN and END.
    pub render: fn() -> String,
}

/// Registered doc markers. `tiers-docgen` iterates this list for both
/// `--write` (rewrite target files) and `--check` (fail on drift).
pub const REGISTRY: &[DocMarker] = &[
    DocMarker {
        name: "TRUST_TIER_TABLE",
        render: render_tier_table,
    },
    DocMarker {
        name: "KEYBINDINGS",
        render: render_keybinding_table,
    },
];

/// Rewrite the body of every registered marker pair in `input`.
/// Unknown marker names are preserved verbatim. Returns the rewritten
/// string plus a count of replacements performed.
#[must_use]
pub fn apply_markers(input: &str) -> (String, usize) {
    let mut out = String::with_capacity(input.len());
    let mut cursor = 0usize;
    let mut replacements = 0usize;
    let bytes = input.as_bytes();

    while let Some(begin_abs) = find_from(bytes, cursor, b"<!-- tiers:BEGIN:") {
        let Some(close_idx) = find_from(bytes, begin_abs, b"-->") else {
            break;
        };
        let after_begin_tag = close_idx + 3;
        let Some(name) = marker_name(&input[begin_abs..after_begin_tag]) else {
            out.push_str(&input[cursor..after_begin_tag]);
            cursor = after_begin_tag;
            continue;
        };
        let end_tag = format!("<!-- tiers:END:{name} -->");
        let Some(end_abs) = find_from(bytes, after_begin_tag, end_tag.as_bytes()) else {
            break;
        };

        out.push_str(&input[cursor..after_begin_tag]);
        // A rendered table ends with a trailing \n; we want the body
        // between markers to start on a fresh line and end cleanly
        // before the END tag — so wrap with newlines.
        if let Some(m) = REGISTRY.iter().find(|m| m.name == name) {
            out.push('\n');
            out.push_str(&(m.render)());
            replacements += 1;
        } else {
            out.push_str(&input[after_begin_tag..end_abs]);
        }
        out.push_str(&end_tag);
        cursor = end_abs + end_tag.len();
    }

    out.push_str(&input[cursor..]);
    (out, replacements)
}

fn marker_name(begin_tag: &str) -> Option<&str> {
    let prefix = "<!-- tiers:BEGIN:";
    let suffix = " -->";
    let inner = begin_tag.strip_prefix(prefix)?.strip_suffix(suffix)?;
    if inner.is_empty() || !inner.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    Some(inner)
}

fn find_from(haystack: &[u8], start: usize, needle: &[u8]) -> Option<usize> {
    if start > haystack.len() {
        return None;
    }
    haystack[start..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| start + p)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn tier_table_has_row_for_every_variant() {
        let t = render_tier_table();
        // One data row per variant — 6 total.
        let data_rows = t
            .lines()
            .filter(|l| l.starts_with("| ") && !l.starts_with("| ----"))
            .count();
        assert_eq!(data_rows, 7, "header + 6 data rows expected, got:\n{t}");
    }

    #[test]
    fn tier_table_labels_match_enum() {
        let t = render_tier_table();
        for label in &[
            "VERIFIED",
            "UNVERIFIED",
            "UNTRUSTED KEY",
            "PARSE FAILED",
            "BOOT BLOCKED",
            "HASH MISMATCH",
        ] {
            assert!(t.contains(label), "tier table missing {label}");
        }
    }

    #[test]
    fn keybinding_table_contains_every_registered_binding() {
        let t = render_keybinding_table();
        for k in KEYBINDINGS {
            assert!(
                t.contains(k.description),
                "keybinding description missing: {}",
                k.description
            );
        }
    }

    #[test]
    fn apply_markers_rewrites_tier_region() {
        let input = "before\n<!-- tiers:BEGIN:TRUST_TIER_TABLE -->\nSTALE\n<!-- tiers:END:TRUST_TIER_TABLE -->\nafter";
        let (out, n) = apply_markers(input);
        assert_eq!(n, 1);
        assert!(out.contains("VERIFIED"));
        assert!(!out.contains("STALE"));
    }

    #[test]
    fn apply_markers_idempotent() {
        let input = "<!-- tiers:BEGIN:TRUST_TIER_TABLE -->\n\n<!-- tiers:END:TRUST_TIER_TABLE -->";
        let (first, _) = apply_markers(input);
        let (second, _) = apply_markers(&first);
        assert_eq!(first, second);
    }

    #[test]
    fn apply_markers_preserves_unknown_names() {
        let input = "<!-- tiers:BEGIN:UNKNOWN -->dont-touch<!-- tiers:END:UNKNOWN -->";
        let (out, n) = apply_markers(input);
        assert_eq!(n, 0);
        assert_eq!(out, input);
    }

    #[test]
    fn registry_contains_both_markers() {
        let names: Vec<&str> = REGISTRY.iter().map(|m| m.name).collect();
        assert!(names.contains(&"TRUST_TIER_TABLE"));
        assert!(names.contains(&"KEYBINDINGS"));
    }
}
