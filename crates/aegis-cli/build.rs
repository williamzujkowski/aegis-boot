// Build scripts fail the build via panic on error — the workspace-
// wide `unwrap_used`/`expect_used = deny` is inappropriate here
// (a build-time panic IS the correct failure mode).
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Build-time man-page templating — Phase 1b of #286 / #287.
//!
//! `man/aegis-boot.1.in` is the authored source. This build script
//! renders it to `$OUT_DIR/aegis-boot.1` by substituting:
//!
//!   * `@VERSION@` — from `env!("CARGO_PKG_VERSION")`, which (thanks
//!     to `version.workspace = true`) always matches the single
//!     source of truth in `[workspace.package].version`.
//!   * `@DATE@` — parsed from the top-most released version entry in
//!     `CHANGELOG.md`. Authoritative, maintainer-controlled, stable
//!     across builds of the same source tree (= reproducible).
//!
//! Writes ONLY to `OUT_DIR` per the Security Engineer review of
//! #286 — build scripts must not mutate the source tree.
//!
//! Called by `cargo build` automatically. The rendered path is read
//! at compile time by `src/man.rs` via `include_str!(concat!(
//! env!("OUT_DIR"), "/aegis-boot.1"))`, so the binary ships with the
//! rendered man page embedded.

use std::fs;
use std::path::Path;

fn main() {
    let template_path = Path::new("../../man/aegis-boot.1.in");
    let changelog_path = Path::new("../../CHANGELOG.md");
    let out_dir = std::env::var_os("OUT_DIR").expect("OUT_DIR must be set by cargo");
    let out_path = Path::new(&out_dir).join("aegis-boot.1");

    // Re-run the build script when either input changes. Without these
    // directives, cargo would not re-render the man page after a
    // CHANGELOG edit or a template tweak.
    println!("cargo:rerun-if-changed={}", template_path.display());
    println!("cargo:rerun-if-changed={}", changelog_path.display());

    let template = fs::read_to_string(template_path)
        .unwrap_or_else(|e| panic!("read man template {}: {e}", template_path.display()));

    let version = std::env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION must be set");
    let date = latest_release_date(changelog_path);

    let rendered = template
        .replace("@VERSION@", &version)
        .replace("@DATE@", &date);

    fs::write(&out_path, rendered).unwrap_or_else(|e| panic!("write {}: {e}", out_path.display()));
}

/// Parse the top-most released-version date from `CHANGELOG.md`.
/// Looks for the first line matching `## [X.Y.Z...] — YYYY-MM-DD`.
/// Skips `## [Unreleased]` since that's an in-progress section with
/// no date.
///
/// Falls back to `0000-00-00` if no matching heading is found — a
/// visibly-wrong placeholder that surfaces the parse failure without
/// breaking the build (dev environments with a stripped CHANGELOG
/// still produce a usable man page).
fn latest_release_date(changelog_path: &Path) -> String {
    let Ok(text) = fs::read_to_string(changelog_path) else {
        return "0000-00-00".to_string();
    };
    for line in text.lines() {
        // Heading shape: `## [X.Y.Z-suffix?] — YYYY-MM-DD`
        let Some(rest) = line.strip_prefix("## [") else {
            continue;
        };
        if rest.starts_with("Unreleased]") {
            continue;
        }
        // Skip past the closing bracket + `] — ` (em dash) to the date.
        let Some(close_bracket) = rest.find(']') else {
            continue;
        };
        let after_bracket = &rest[close_bracket + 1..];
        // Accept `]` followed by either ` — ` or ` - ` before the date.
        let trimmed = after_bracket
            .trim_start_matches(' ')
            .trim_start_matches('—')
            .trim_start_matches('-')
            .trim_start();
        // Date is the first 10 chars — YYYY-MM-DD. Validate shape
        // loosely (length + digit/dash pattern) so a malformed
        // heading falls back rather than producing garbage.
        if trimmed.len() >= 10 {
            let date = &trimmed[..10];
            if date.len() == 10
                && date.as_bytes()[4] == b'-'
                && date.as_bytes()[7] == b'-'
                && date[..4].bytes().all(|b| b.is_ascii_digit())
                && date[5..7].bytes().all(|b| b.is_ascii_digit())
                && date[8..].bytes().all(|b| b.is_ascii_digit())
            {
                return date.to_string();
            }
        }
    }
    "0000-00-00".to_string()
}
