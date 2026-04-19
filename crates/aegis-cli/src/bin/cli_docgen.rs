//! `cli-docgen` — drift-check between the `aegis-boot` subcommand
//! dispatch table and its user-facing documentation.
//!
//! Phase 3a of [#286]/[#289]. Narrowly scoped: this tool only
//! verifies that every subcommand registered below has a
//! corresponding section in `docs/CLI.md` AND a `.TP` entry in
//! `man/aegis-boot.1.in`. It does **not** (yet) generate help-text
//! bodies — that is Phase 3b and requires consolidating scattered
//! `println!()` help chains into `const HELP: &str` constants first.
//!
//! This phase's payoff: catches the class of drift that hid
//! `fetch-image` from the user-facing docs on the last release
//! cycle (survey finding on #289). A new subcommand added to
//! `main.rs` must now also be registered here and documented in
//! both surfaces, or CI fails.
//!
//! ## Canonical subcommand list
//!
//! [`SUBCOMMANDS`] is the single source of truth for the
//! user-facing subcommand surface. Adding a new subcommand:
//!
//! 1. Wire the dispatch arm in `main.rs` (`Some("new") => ...`).
//! 2. Append the name to [`SUBCOMMANDS`] below (keep sorted for
//!    reviewer sanity).
//! 3. Add a `## \`aegis-boot new\`` section to `docs/CLI.md` (the
//!    literal backticks wrap the subcommand name in the heading).
//! 4. Add a `.TP` paragraph with a `.B new ...` entry to
//!    `man/aegis-boot.1.in`.
//!
//! CI's `cli-docgen --check` fails any PR that lands (1) without
//! also landing (3) + (4).
//!
//! [#286]: https://github.com/williamzujkowski/aegis-boot/issues/286
//! [#289]: https://github.com/williamzujkowski/aegis-boot/issues/289

#![forbid(unsafe_code)]
// Doc comments describe markdown + troff syntax literally; pedantic
// doc-markdown complains about unbalanced backticks even when they
// are intentional literal examples.
#![allow(clippy::doc_markdown)]

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Canonical user-facing subcommand list. Kept sorted for reviewer
/// sanity; the drift-check is set-based so order doesn't matter for
/// correctness.
///
/// Intentionally omitted: `-h` / `--help` / `--version` / `version`
/// are top-level argv flags handled inline in `main.rs`, not
/// subcommands in the docs sense.
const SUBCOMMANDS: &[&str] = &[
    "add",
    "attest",
    "compat",
    "completions",
    "doctor",
    "eject",
    "fetch",
    "fetch-image",
    "flash",
    "init",
    "list",
    "man",
    "recommend",
    "tour",
    "update",
    "verify",
];

/// Parse markdown headings of the form
/// `## ``aegis-boot NAME``` out of [`docs/CLI.md`] and return the
/// set of NAMEs (the doubled backticks in this doc sentence mean
/// "the heading has a single pair of backticks around `aegis-boot
/// NAME`").
///
/// Does not match `###` sub-sections or headings with trailing
/// text after the closing backtick.
fn extract_cli_md_subcommands(body: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in body.lines() {
        if let Some(rest) = line.strip_prefix("## `aegis-boot ") {
            if let Some(name) = rest.strip_suffix('`') {
                // Guard against stray backticks or whitespace in name.
                if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                    out.insert(name.to_string());
                }
            }
        }
    }
    out
}

/// Parse `.TP` → `.B NAME ...` blocks out of the `.SH SUBCOMMANDS`
/// section of the `man` template and return the set of NAMEs.
///
/// The man page uses troff `.TP` paragraph-break markers to delimit
/// each subcommand entry. Every `.TP` line is followed by a `.B
/// NAME ...` line where NAME is the subcommand.
///
/// Scope: we restrict extraction to the `SUBCOMMANDS` section so
/// that `.TP` entries in `EXIT STATUS` (which lists `.B 0`, `.B 1`,
/// `.B 2` exit codes) and `ENVIRONMENT` (flag names) don't leak
/// in. A subcommand name must additionally start with an alphabetic
/// character — belt-and-braces against the numeric exit-code shape.
fn extract_man_subcommands(body: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let lines: Vec<&str> = body.lines().collect();
    let mut in_subcommands_section = false;

    for i in 0..lines.len() {
        let line = lines[i];
        if let Some(section) = line.strip_prefix(".SH ") {
            // `.SH SUBCOMMANDS` → start capture; any other `.SH` →
            // stop. Section names may be quoted (`.SH "..."`); the
            // current man page uses unquoted single-word headers so
            // trimming is enough.
            in_subcommands_section = section.trim().trim_matches('"') == "SUBCOMMANDS";
            continue;
        }
        if !in_subcommands_section {
            continue;
        }
        if line.trim() != ".TP" {
            continue;
        }
        let Some(next) = lines.get(i + 1) else {
            continue;
        };
        let Some(rest) = next.strip_prefix(".B ") else {
            continue;
        };
        // `.B NAME \fR...` — first whitespace-delimited token (before
        // `\fR` or the literal flags) is the name.
        let name = rest
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim_end_matches('\\');
        if !name.is_empty()
            && name.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            out.insert(name.to_string());
        }
    }
    out
}

/// Locate the repo root by walking up from `start`. Same pattern as
/// `constants-docgen`.
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
                    "cli-docgen: could not find workspace root Cargo.toml walking up from {}",
                    start.display()
                ));
            }
        }
    }
}

/// Format a sorted diff between expected and observed sets as a
/// user-readable multi-line message. Empty tuple → no drift.
fn diff_report(surface: &str, expected: &BTreeSet<String>, observed: &BTreeSet<String>) -> String {
    let missing: Vec<&String> = expected.difference(observed).collect();
    let extra: Vec<&String> = observed.difference(expected).collect();
    if missing.is_empty() && extra.is_empty() {
        return String::new();
    }
    let mut msg = format!("drift in {surface}:\n");
    if !missing.is_empty() {
        msg.push_str("  missing (in SUBCOMMANDS but not in surface): ");
        msg.push_str(
            &missing
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        );
        msg.push('\n');
    }
    if !extra.is_empty() {
        msg.push_str("  unexpected (in surface but not in SUBCOMMANDS): ");
        msg.push_str(
            &extra
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        );
        msg.push('\n');
    }
    msg
}

fn main() -> ExitCode {
    // Devtool — this bin has no user-controlled argv behavior.
    // argv[0] is dropped; no arguments supported. Same safety story
    // as constants-docgen.
    // nosemgrep: rust.lang.security.args.args
    let _args: Vec<String> = env::args().skip(1).collect();

    let cwd = match env::current_dir() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("cli-docgen: cannot read CWD: {e}");
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

    let expected: BTreeSet<String> = SUBCOMMANDS.iter().map(|s| (*s).to_string()).collect();

    let cli_md_path = repo_root.join("docs/CLI.md");
    let man_path = repo_root.join("man/aegis-boot.1.in");

    let cli_md = match fs::read_to_string(&cli_md_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cli-docgen: cannot read {}: {e}", cli_md_path.display());
            return ExitCode::from(2);
        }
    };
    let man = match fs::read_to_string(&man_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cli-docgen: cannot read {}: {e}", man_path.display());
            return ExitCode::from(2);
        }
    };

    let cli_md_observed = extract_cli_md_subcommands(&cli_md);
    let man_observed = extract_man_subcommands(&man);

    let mut reports = Vec::new();
    let cli_report = diff_report("docs/CLI.md", &expected, &cli_md_observed);
    if !cli_report.is_empty() {
        reports.push(cli_report);
    }
    let man_report = diff_report("man/aegis-boot.1.in", &expected, &man_observed);
    if !man_report.is_empty() {
        reports.push(man_report);
    }

    if reports.is_empty() {
        println!(
            "cli-docgen: OK — all {} subcommands covered in docs/CLI.md and man/aegis-boot.1.in",
            expected.len()
        );
        ExitCode::SUCCESS
    } else {
        for r in &reports {
            eprint!("{r}");
        }
        eprintln!();
        eprintln!("cli-docgen: FAIL — subcommand documentation drift detected.");
        eprintln!(
            "Fix: for each missing subcommand, add a `## `aegis-boot X`` heading to docs/CLI.md"
        );
        eprintln!("and a `.TP\\n.B X ...` entry to man/aegis-boot.1.in. For each unexpected");
        eprintln!("subcommand, either remove it from the surface or register it in SUBCOMMANDS.");
        ExitCode::from(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_cli_md_matches_standard_heading() {
        let body = "## `aegis-boot init`\n\n## `aegis-boot flash`\n\nunrelated text\n";
        let out = extract_cli_md_subcommands(body);
        assert!(out.contains("init"));
        assert!(out.contains("flash"));
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn extract_cli_md_ignores_sub_section_headings() {
        let body = "## `aegis-boot init`\n### sub-header\n";
        let out = extract_cli_md_subcommands(body);
        assert_eq!(out.len(), 1);
        assert!(out.contains("init"));
    }

    #[test]
    fn extract_cli_md_accepts_hyphenated_names() {
        let body = "## `aegis-boot fetch-image`\n";
        let out = extract_cli_md_subcommands(body);
        assert!(out.contains("fetch-image"));
    }

    #[test]
    fn extract_cli_md_rejects_trailing_text_on_heading() {
        // A heading like `## `aegis-boot init` subtitle` is not
        // treated as a subcommand heading — the matcher requires the
        // backtick to be followed by EOL.
        let body = "## `aegis-boot init` — some subtitle\n";
        let out = extract_cli_md_subcommands(body);
        assert!(out.is_empty(), "expected empty, got {out:?}");
    }

    #[test]
    fn extract_man_matches_tp_b_pair_in_subcommands_section() {
        let body = ".SH SUBCOMMANDS\n.TP\n.B init \\fR[\\fIdevice\\fR]\nDescription line\n";
        let out = extract_man_subcommands(body);
        assert!(out.contains("init"));
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn extract_man_ignores_b_without_preceding_tp() {
        // `.B aegis-boot` in descriptive text should not be picked
        // up as a subcommand — it lacks the immediately preceding
        // `.TP` paragraph marker.
        let body = ".SH SUBCOMMANDS\n.PP\n.B aegis-boot\nis a tool...\n";
        let out = extract_man_subcommands(body);
        assert!(out.is_empty());
    }

    #[test]
    fn extract_man_handles_multiple_entries() {
        let body = ".SH SUBCOMMANDS\n.TP\n.B init\nA\n.TP\n.B flash\nB\n.TP\n.B fetch-image \\fR[\\fIslug\\fR]\nC\n";
        let out = extract_man_subcommands(body);
        assert!(out.contains("init"));
        assert!(out.contains("flash"));
        assert!(out.contains("fetch-image"));
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn extract_man_excludes_exit_status_entries() {
        // `.TP` + `.B 0/1/2` entries in `.SH EXIT STATUS` must not
        // be picked up — real regression from the v1 parser.
        let body = ".SH SUBCOMMANDS\n.TP\n.B init\nA\n.SH EXIT STATUS\n.TP\n.B 0\nSuccess.\n.TP\n.B 1\nFailure.\n";
        let out = extract_man_subcommands(body);
        assert!(out.contains("init"));
        assert!(!out.contains("0"));
        assert!(!out.contains("1"));
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn extract_man_ignores_subcommand_like_entries_before_sh_subcommands() {
        // A `.TP` + `.B foo` appearing before `.SH SUBCOMMANDS` (i.e.
        // in `.SH SYNOPSIS` or above) should not be captured either.
        let body = ".SH SYNOPSIS\n.TP\n.B bogus-early\nno\n.SH SUBCOMMANDS\n.TP\n.B init\n";
        let out = extract_man_subcommands(body);
        assert!(out.contains("init"));
        assert!(!out.contains("bogus-early"));
    }

    #[test]
    fn diff_report_empty_when_sets_match() {
        let a: BTreeSet<String> = ["x", "y"].iter().map(|s| (*s).to_string()).collect();
        let b: BTreeSet<String> = ["y", "x"].iter().map(|s| (*s).to_string()).collect();
        assert_eq!(diff_report("surface", &a, &b), "");
    }

    #[test]
    fn diff_report_names_missing_entries() {
        let expected: BTreeSet<String> = ["x", "y"].iter().map(|s| (*s).to_string()).collect();
        let observed: BTreeSet<String> = ["x"].iter().map(|s| (*s).to_string()).collect();
        let r = diff_report("surface", &expected, &observed);
        assert!(r.contains("missing"));
        assert!(r.contains('y'));
    }

    #[test]
    fn diff_report_names_extra_entries() {
        let expected: BTreeSet<String> = ["x"].iter().map(|s| (*s).to_string()).collect();
        let observed: BTreeSet<String> = ["x", "rogue"].iter().map(|s| (*s).to_string()).collect();
        let r = diff_report("surface", &expected, &observed);
        assert!(r.contains("unexpected"));
        assert!(r.contains("rogue"));
    }

    #[test]
    fn subcommands_registry_is_sorted() {
        let mut sorted = SUBCOMMANDS.to_vec();
        sorted.sort_unstable();
        assert_eq!(SUBCOMMANDS, sorted.as_slice());
    }

    #[test]
    fn subcommands_registry_has_no_duplicates() {
        let set: BTreeSet<&&str> = SUBCOMMANDS.iter().collect();
        assert_eq!(set.len(), SUBCOMMANDS.len());
    }
}
