// SPDX-License-Identifier: MIT OR Apache-2.0

//! `tiers-docgen` — regenerate the trust-tier table and keybinding
//! reference in user-facing docs from the canonical in-code sources
//! (`rescue_tui::verdict::TrustVerdict` and
//! `rescue_tui::keybindings::KEYBINDINGS`).
//!
//! Phase 7 of [#455] / closes #462. Mirrors the `constants-docgen`
//! pattern (Phase 2 of #286): marker-pair rewriting in a fixed list
//! of target docs.
//!
//! ## Modes
//!
//! - `--write` (default): rewrite each target file in place.
//! - `--check`: compute what `--write` would produce, diff against
//!   the committed file, exit non-zero if any file would change.
//!   Used by CI to enforce drift-freedom.
//!
//! [#455]: https://github.com/aegis-boot/aegis-boot/issues/455

#![forbid(unsafe_code)]

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use rescue_tui::docgen;

/// Fixed list of doc files this tool may rewrite. Hard-coded (rather
/// than a `walkdir` scan) so scope is explicit and a stray marker in
/// an unrelated file can't accidentally trigger a rewrite.
fn target_files(repo_root: &Path) -> Vec<PathBuf> {
    [
        "docs/HOW_IT_WORKS.md",
        "docs/TOUR.md",
        "crates/rescue-tui/README.md",
    ]
    .iter()
    .map(|p| repo_root.join(p))
    .collect()
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
            "usage: tiers-docgen [--write|--check]  (got: {args:?})"
        )),
    }
}

/// Walk up from `start` looking for the workspace root (top-level
/// `Cargo.toml` containing `[workspace]`). Mirrors
/// `constants-docgen::find_repo_root`.
fn find_repo_root(start: &Path) -> Result<PathBuf, String> {
    let mut cur = start;
    loop {
        let candidate = cur.join("Cargo.toml");
        if candidate.is_file()
            && let Ok(body) = fs::read_to_string(&candidate)
            && body.contains("[workspace]")
        {
            return Ok(cur.to_path_buf());
        }
        match cur.parent() {
            Some(parent) => cur = parent,
            None => {
                return Err(format!(
                    "tiers-docgen: could not find workspace root Cargo.toml walking up from {}",
                    start.display()
                ));
            }
        }
    }
}

fn main() -> ExitCode {
    // Devtool — args are flag names only, not security keys.
    // nosemgrep: rust.lang.security.args.args
    let args: Vec<String> = env::args().skip(1).collect();
    let mode = match parse_mode(&args) {
        Ok(m) => m,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::from(2);
        }
    };

    let cwd = match env::current_dir() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("tiers-docgen: cannot read CWD: {e}");
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
                eprintln!("tiers-docgen: cannot read {}: {e}", path.display());
                return ExitCode::from(2);
            }
        };
        let (rendered, n) = docgen::apply_markers(&body);
        total_replacements += n;

        match mode {
            Mode::Write => {
                if rendered == body {
                    println!("unchanged {} ({n} markers)", path.display());
                } else {
                    if let Err(e) = fs::write(&path, &rendered) {
                        eprintln!("tiers-docgen: cannot write {}: {e}", path.display());
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
                "tiers-docgen: wrote {} target files, {total_replacements} total markers rendered",
                target_files(&repo_root).len()
            );
            ExitCode::SUCCESS
        }
        Mode::Check => {
            if drift_files.is_empty() {
                println!(
                    "tiers-docgen: OK — {total_replacements} markers across {} files all match source",
                    target_files(&repo_root).len()
                );
                ExitCode::SUCCESS
            } else {
                eprintln!();
                eprintln!(
                    "tiers-docgen: FAIL — {} file(s) diverge from the canonical source.",
                    drift_files.len()
                );
                eprintln!(
                    "Fix: run `cargo run -p rescue-tui --bin tiers-docgen` locally and commit the result."
                );
                ExitCode::from(1)
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

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
        assert!(parse_mode(&["--bogus".to_string()]).is_err());
        assert!(parse_mode(&["--write".to_string(), "extra".to_string()]).is_err());
    }

    #[test]
    fn target_files_lists_three_known_docs() {
        let repo = PathBuf::from("/tmp/fake-repo");
        let files = target_files(&repo);
        assert_eq!(files.len(), 3);
        assert!(files.iter().any(|p| p.ends_with("docs/HOW_IT_WORKS.md")));
        assert!(files.iter().any(|p| p.ends_with("docs/TOUR.md")));
        assert!(
            files
                .iter()
                .any(|p| p.ends_with("crates/rescue-tui/README.md"))
        );
    }
}
