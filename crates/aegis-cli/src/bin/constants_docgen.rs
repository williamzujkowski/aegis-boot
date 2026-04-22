// SPDX-License-Identifier: MIT OR Apache-2.0

//! `constants-docgen` — renders values from [`crate::constants`] into
//! HTML-marker regions in the user-facing docs.
//!
//! Phase 2 of [#286]. Prevents the class of drift where a numeric
//! value in prose (e.g. "ESP is sized at 400 MB") goes stale because
//! the code value changed but the doc didn't.
//!
//! ## Contract
//!
//! Target docs contain marker pairs:
//!
//! ```text
//! <!-- constants:BEGIN:ESP_SIZE_MB -->400 MB<!-- constants:END:ESP_SIZE_MB -->
//! ```
//!
//! This tool walks a fixed list of target doc files, finds each pair,
//! and **replaces the text between the markers** with the current
//! registered value for that name. The markers themselves are
//! preserved verbatim — adding or removing markers is a manual edit.
//!
//! ## Modes
//!
//! - `--write` (default): rewrite each target file in place.
//! - `--check`: compute what `--write` would produce, diff against
//!   the committed file, and exit non-zero if any file would change.
//!   Used by CI to enforce drift-freedom.
//!
//! ## Source-of-truth note
//!
//! Both this binary and the main `aegis-boot` binary compile from
//! `crates/aegis-cli/src/constants.rs` — the constants file is
//! `#[path]`-included below so the two bins share the same const
//! values without a workspace library crate. If the include line
//! is ever broken, the two bins will disagree silently; a small
//! contract test in `constants::tests` pins one known value at
//! compile time to guard against that.
//!
//! [#286]: https://github.com/aegis-boot/aegis-boot/issues/286

#![forbid(unsafe_code)]

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[path = "../constants.rs"]
mod constants;

use constants::{DEFAULT_READBACK_BYTES, ESP_SIZE_MB, GRUB_TIMEOUT_SECS, MAX_MANIFEST_BYTES};

/// A single marker registered with its formatted display value.
struct Marker {
    /// The name that appears inside `<!-- constants:BEGIN:NAME -->`.
    name: &'static str,
    /// The rendered text that appears between BEGIN and END markers.
    value: String,
}

/// Registered marker names + their current rendered values.
///
/// Adding a new marker: append to this list, then wrap the target
/// doc's value with `<!-- constants:BEGIN:NAME -->...<!-- constants:END:NAME -->`.
fn registry() -> Vec<Marker> {
    vec![
        Marker {
            name: "ESP_SIZE_MB",
            value: format!("{ESP_SIZE_MB} MB"),
        },
        Marker {
            name: "READBACK_WINDOW",
            value: format!("{} MB", DEFAULT_READBACK_BYTES / (1024 * 1024)),
        },
        Marker {
            name: "MAX_MANIFEST_SIZE",
            value: format!("{} KiB", MAX_MANIFEST_BYTES / 1024),
        },
        Marker {
            name: "GRUB_TIMEOUT_SECS",
            value: format!("{GRUB_TIMEOUT_SECS}"),
        },
    ]
}

/// Fixed list of doc files this tool may rewrite. Hard-coded rather
/// than a `walkdir` scan so the scope is explicit and an unrelated
/// file containing stray marker syntax can't be accidentally
/// rewritten.
fn target_files(repo_root: &Path) -> Vec<PathBuf> {
    ["docs/ARCHITECTURE.md", "docs/USB_LAYOUT.md", "docs/TOUR.md"]
        .iter()
        .map(|p| repo_root.join(p))
        .collect()
}

/// Replace the body of every registered marker pair in `input`
/// with the current rendered value. Returns the rewritten string
/// plus a count of pair-replacements actually performed (used to
/// detect marker/registry drift).
fn apply_markers(input: &str, markers: &[Marker]) -> (String, usize) {
    let mut out = String::with_capacity(input.len());
    let mut cursor = 0usize;
    let mut replacements = 0usize;
    let bytes = input.as_bytes();

    while let Some(begin_abs) = find_from(bytes, cursor, b"<!-- constants:BEGIN:") {
        // Copy the portion before BEGIN as-is, including the BEGIN
        // marker itself (we rewrite only the region between markers).
        let Some(close_idx) = find_from(bytes, begin_abs, b"-->") else {
            // Malformed BEGIN without closing `-->`; leave rest untouched.
            break;
        };
        let after_begin_tag = close_idx + 3;
        let Some(name) = marker_name(&input[begin_abs..after_begin_tag]) else {
            // Malformed marker name; emit verbatim and move on.
            out.push_str(&input[cursor..after_begin_tag]);
            cursor = after_begin_tag;
            continue;
        };
        let end_tag = format!("<!-- constants:END:{name} -->");
        let Some(end_abs) = find_from(bytes, after_begin_tag, end_tag.as_bytes()) else {
            // Unclosed BEGIN; leave rest untouched.
            break;
        };

        out.push_str(&input[cursor..after_begin_tag]);
        if let Some(m) = markers.iter().find(|m| m.name == name) {
            out.push_str(&m.value);
            replacements += 1;
        } else {
            // Unknown marker name: preserve the existing content
            // rather than deleting it, but don't count it as a
            // successful replacement.
            out.push_str(&input[after_begin_tag..end_abs]);
        }
        out.push_str(&end_tag);
        cursor = end_abs + end_tag.len();
    }

    out.push_str(&input[cursor..]);
    (out, replacements)
}

/// Extract `NAME` from a `<!-- constants:BEGIN:NAME -->` tag.
fn marker_name(begin_tag: &str) -> Option<&str> {
    let prefix = "<!-- constants:BEGIN:";
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

enum Mode {
    Write,
    Check,
}

fn parse_mode(args: &[String]) -> Result<Mode, String> {
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        [] | ["--write"] => Ok(Mode::Write),
        ["--check"] => Ok(Mode::Check),
        _ => Err(format!(
            "usage: constants-docgen [--write|--check]  (got: {args:?})"
        )),
    }
}

/// Locate the repo root by walking up from `start` looking for a
/// top-level `Cargo.toml` that contains `[workspace]`. Mirrors the
/// pattern used by `scripts/check-doc-version.sh`.
fn find_repo_root(start: &Path) -> Result<PathBuf, String> {
    let mut cur = start;
    loop {
        let candidate = cur.join("Cargo.toml");
        if candidate.is_file() {
            if let Ok(body) = fs::read_to_string(&candidate) {
                if body.contains("[workspace]") {
                    return Ok(cur.to_path_buf());
                }
            }
        }
        match cur.parent() {
            Some(parent) => cur = parent,
            None => {
                return Err(format!(
                    "constants-docgen: could not find workspace root Cargo.toml walking up from {}",
                    start.display()
                ));
            }
        }
    }
}

fn main() -> ExitCode {
    // Devtool — args are flag names (`--write` / `--check`) only,
    // not security keys. argv[0] is dropped via `skip(1)`. Same
    // rationale as the main aegis-boot binary.
    // nosemgrep: rust.lang.security.args.args
    let args: Vec<String> = env::args().skip(1).collect();
    let mode = match parse_mode(&args) {
        Ok(m) => m,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::from(2);
        }
    };

    let markers = registry();
    let cwd = match env::current_dir() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("constants-docgen: cannot read CWD: {e}");
            return ExitCode::from(2);
        }
    };
    let repo_root = match find_repo_root(&cwd) {
        Ok(p) => p,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::from(2);
        }
    };

    let mut drift_files: Vec<PathBuf> = Vec::new();
    let mut total_replacements = 0usize;

    for path in target_files(&repo_root) {
        let body = match fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("constants-docgen: cannot read {}: {e}", path.display());
                return ExitCode::from(2);
            }
        };
        let (rendered, n) = apply_markers(&body, &markers);
        total_replacements += n;

        match mode {
            Mode::Write => {
                if rendered == body {
                    println!("unchanged {} ({n} markers)", path.display());
                } else {
                    if let Err(e) = fs::write(&path, &rendered) {
                        eprintln!("constants-docgen: cannot write {}: {e}", path.display());
                        return ExitCode::from(2);
                    }
                    println!("updated {} ({n} markers)", path.display());
                }
            }
            Mode::Check => {
                if rendered != body {
                    drift_files.push(path.clone());
                    eprintln!(
                        "drift: {} would change ({n} markers render differently than committed)",
                        path.display()
                    );
                }
            }
        }
    }

    match mode {
        Mode::Write => {
            println!(
                "constants-docgen: wrote {} target files, {total_replacements} total markers rendered",
                target_files(&repo_root).len()
            );
            ExitCode::SUCCESS
        }
        Mode::Check => {
            if drift_files.is_empty() {
                println!(
                    "constants-docgen: OK — {total_replacements} markers across {} files all match registry",
                    target_files(&repo_root).len()
                );
                ExitCode::SUCCESS
            } else {
                eprintln!();
                eprintln!(
                    "constants-docgen: FAIL — {} file(s) diverge from the constants registry.",
                    drift_files.len()
                );
                eprintln!(
                    "Fix: run `cargo run -p aegis-bootctl --bin constants-docgen --features docgen` locally and commit the result."
                );
                ExitCode::from(1)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_markers() -> Vec<Marker> {
        vec![
            Marker {
                name: "ESP_SIZE_MB",
                value: "400 MB".to_string(),
            },
            Marker {
                name: "READBACK_WINDOW",
                value: "64 MB".to_string(),
            },
        ]
    }

    #[test]
    fn apply_markers_replaces_body_between_tags() {
        let input = "ESP is sized at <!-- constants:BEGIN:ESP_SIZE_MB -->STALE<!-- constants:END:ESP_SIZE_MB --> on disk.";
        let (out, n) = apply_markers(input, &fixture_markers());
        assert_eq!(n, 1);
        assert!(
            out.contains(
                "<!-- constants:BEGIN:ESP_SIZE_MB -->400 MB<!-- constants:END:ESP_SIZE_MB -->"
            ),
            "out: {out}"
        );
    }

    #[test]
    fn apply_markers_preserves_tags_verbatim() {
        let input = "<!-- constants:BEGIN:ESP_SIZE_MB -->X<!-- constants:END:ESP_SIZE_MB -->";
        let (out, _) = apply_markers(input, &fixture_markers());
        assert!(out.starts_with("<!-- constants:BEGIN:ESP_SIZE_MB -->"));
        assert!(out.ends_with("<!-- constants:END:ESP_SIZE_MB -->"));
    }

    #[test]
    fn apply_markers_handles_multiple_pairs() {
        let input = "A <!-- constants:BEGIN:ESP_SIZE_MB -->a<!-- constants:END:ESP_SIZE_MB --> B <!-- constants:BEGIN:READBACK_WINDOW -->b<!-- constants:END:READBACK_WINDOW --> C";
        let (out, n) = apply_markers(input, &fixture_markers());
        assert_eq!(n, 2);
        assert!(out.contains("-->400 MB<!--"));
        assert!(out.contains("-->64 MB<!--"));
    }

    #[test]
    fn apply_markers_leaves_unknown_name_body_untouched() {
        let input = "<!-- constants:BEGIN:UNKNOWN_MARKER -->dont-touch<!-- constants:END:UNKNOWN_MARKER -->";
        let (out, n) = apply_markers(input, &fixture_markers());
        assert_eq!(n, 0);
        assert_eq!(out, input, "unknown marker body should be preserved");
    }

    #[test]
    fn apply_markers_idempotent() {
        let input =
            "val: <!-- constants:BEGIN:ESP_SIZE_MB -->400 MB<!-- constants:END:ESP_SIZE_MB -->";
        let (first, _) = apply_markers(input, &fixture_markers());
        let (second, _) = apply_markers(&first, &fixture_markers());
        assert_eq!(first, second, "second render must equal first");
    }

    #[test]
    fn apply_markers_ignores_unclosed_begin_tag() {
        let input = "trailing <!-- constants:BEGIN:ESP_SIZE_MB --> unclosed";
        let (out, n) = apply_markers(input, &fixture_markers());
        assert_eq!(n, 0);
        assert_eq!(out, input);
    }

    #[test]
    fn marker_name_parses_valid_begin_tag() {
        assert_eq!(
            marker_name("<!-- constants:BEGIN:ESP_SIZE_MB -->"),
            Some("ESP_SIZE_MB")
        );
    }

    #[test]
    fn marker_name_rejects_empty_name() {
        assert_eq!(marker_name("<!-- constants:BEGIN: -->"), None);
    }

    #[test]
    fn marker_name_rejects_non_alnum_chars() {
        assert_eq!(marker_name("<!-- constants:BEGIN:has space -->"), None);
        assert_eq!(marker_name("<!-- constants:BEGIN:has-dash -->"), None);
    }

    #[test]
    fn parse_mode_defaults_to_write() {
        assert!(matches!(parse_mode(&[]), Ok(Mode::Write)));
        assert!(matches!(
            parse_mode(&["--write".to_string()]),
            Ok(Mode::Write)
        ));
    }

    #[test]
    fn parse_mode_accepts_check() {
        assert!(matches!(
            parse_mode(&["--check".to_string()]),
            Ok(Mode::Check)
        ));
    }

    #[test]
    fn parse_mode_rejects_unknown() {
        assert!(parse_mode(&["--whatever".to_string()]).is_err());
        assert!(parse_mode(&["--write".to_string(), "extra".to_string()]).is_err());
    }

    #[test]
    fn registry_contains_all_shared_constants() {
        let r = registry();
        let names: Vec<&str> = r.iter().map(|m| m.name).collect();
        assert!(names.contains(&"ESP_SIZE_MB"));
        assert!(names.contains(&"READBACK_WINDOW"));
        assert!(names.contains(&"MAX_MANIFEST_SIZE"));
        assert!(names.contains(&"GRUB_TIMEOUT_SECS"));
    }

    #[test]
    fn registry_values_match_constants() {
        let r = registry();
        let find = |n: &str| r.iter().find(|m| m.name == n).map(|m| m.value.clone());
        assert_eq!(find("ESP_SIZE_MB"), Some(format!("{ESP_SIZE_MB} MB")));
        assert_eq!(
            find("READBACK_WINDOW"),
            Some(format!("{} MB", DEFAULT_READBACK_BYTES / (1024 * 1024)))
        );
        assert_eq!(
            find("MAX_MANIFEST_SIZE"),
            Some(format!("{} KiB", MAX_MANIFEST_BYTES / 1024))
        );
        assert_eq!(
            find("GRUB_TIMEOUT_SECS"),
            Some(format!("{GRUB_TIMEOUT_SECS}"))
        );
    }

    /// Compile-time sanity: the constants module included via
    /// `#[path]` must be the same file the main binary compiles
    /// against. If this test's expectation ever drifts from the
    /// real value in `src/constants.rs`, it's a sign the include
    /// path is wrong or the constant was changed without
    /// regenerating docs.
    #[test]
    fn included_constants_match_known_values() {
        assert_eq!(ESP_SIZE_MB, 400);
        assert_eq!(DEFAULT_READBACK_BYTES, 64 * 1024 * 1024);
        assert_eq!(MAX_MANIFEST_BYTES, 64 * 1024);
        assert_eq!(GRUB_TIMEOUT_SECS, 3);
    }
}
