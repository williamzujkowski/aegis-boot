// SPDX-License-Identifier: MIT OR Apache-2.0

//! `cli-docgen` — drift-check between the `aegis-boot` subcommand
//! dispatch table and its user-facing documentation, and
//! auto-generator for the CLI synopsis doc.
//!
//! Phase 3a of [#289] shipped the list drift-check (subcommand
//! names in dispatch vs. names in docs/CLI.md + man page). Phase 3b
//! (this iteration) extends the tool with synopsis emission: it
//! spawns `aegis-boot <sub> --help` for each registered subcommand,
//! captures the live stdout, and renders
//! `docs/reference/CLI_SYNOPSIS.md` — the authoritative usage
//! reference. Prose + examples stay hand-written in `docs/CLI.md`;
//! the synopsis file is the single source of truth for flag lists.
//!
//! Subprocess capture (rather than `const HELP: &str` extraction
//! from each module) was chosen because the 16 subcommand modules
//! today use scattered `println!()` chains — a blanket const
//! refactor would touch ~15 files and is a separable piece of
//! work. Capturing the live `--help` output matches what users see
//! verbatim, which is the stronger contract.
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
//! [#286]: https://github.com/aegis-boot/aegis-boot/issues/286
//! [#289]: https://github.com/aegis-boot/aegis-boot/issues/289

#![forbid(unsafe_code)]
// Doc comments describe markdown + troff syntax literally; pedantic
// doc-markdown complains about unbalanced backticks even when they
// are intentional literal examples.
#![allow(clippy::doc_markdown)]

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

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
    "bug-report",
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
    "quickstart",
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

/// CLI mode. `Check` is the read-only drift verifier (CI). `Write`
/// regenerates the synopsis file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Check,
    Write,
}

fn parse_mode(args: &[String]) -> Result<(Mode, Option<PathBuf>), String> {
    let mut mode = Mode::Check;
    let mut bin_override: Option<PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--check" => mode = Mode::Check,
            "--write" => mode = Mode::Write,
            "--aegis-boot-bin" => {
                i += 1;
                let Some(path) = args.get(i) else {
                    return Err("--aegis-boot-bin requires a path argument".to_string());
                };
                bin_override = Some(PathBuf::from(path));
            }
            other => {
                return Err(format!(
                    "cli-docgen: unknown argument {other:?} (expected --check|--write [--aegis-boot-bin PATH])"
                ));
            }
        }
        i += 1;
    }
    Ok((mode, bin_override))
}

/// Locate the built `aegis-boot` binary relative to the workspace
/// root. Prefers the release build; falls back to debug.
fn find_aegis_boot_bin(
    repo_root: &Path,
    override_path: Option<PathBuf>,
) -> Result<PathBuf, String> {
    if let Some(p) = override_path {
        return if p.is_file() {
            Ok(p)
        } else {
            Err(format!(
                "cli-docgen: --aegis-boot-bin {} is not a file",
                p.display()
            ))
        };
    }
    let candidates = [
        repo_root.join("target/release/aegis-boot"),
        repo_root.join("target/debug/aegis-boot"),
    ];
    for p in &candidates {
        if p.is_file() {
            return Ok(p.clone());
        }
    }
    Err(format!(
        "cli-docgen: cannot find built aegis-boot binary; looked at {} and {}. \
         Run `cargo build -p aegis-cli` first, or pass --aegis-boot-bin PATH.",
        candidates[0].display(),
        candidates[1].display()
    ))
}

/// Invoke `aegis_boot_bin <sub> --help` and capture stdout. Returns
/// the captured text (stdout only; stderr and non-zero exit codes
/// are treated as errors to surface parser regressions early).
fn capture_help(aegis_boot_bin: &Path, subcommand: &str) -> Result<String, String> {
    let output = Command::new(aegis_boot_bin)
        .arg(subcommand)
        .arg("--help")
        .output()
        .map_err(|e| {
            format!(
                "cli-docgen: failed to spawn {} {} --help: {e}",
                aegis_boot_bin.display(),
                subcommand
            )
        })?;
    if !output.status.success() {
        return Err(format!(
            "cli-docgen: `{} {} --help` exited {} (stderr: {})",
            aegis_boot_bin.display(),
            subcommand,
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout).map_err(|e| {
        format!(
            "cli-docgen: `{} {} --help` stdout is not valid UTF-8: {e}",
            aegis_boot_bin.display(),
            subcommand
        )
    })
}

/// Render the synopsis markdown from a map of subcommand → captured
/// help text. Deterministic: map is already sorted (BTreeMap) so
/// diffs are stable.
fn render_synopsis(helps: &BTreeMap<String, String>) -> String {
    let mut out = String::new();
    out.push_str("# `aegis-boot` CLI synopsis\n\n");
    out.push_str("<!-- AUTO-GENERATED by `cli-docgen --write`. Do not edit directly. -->\n\n");
    out.push_str(
        "This file is auto-generated from the live output of `aegis-boot <SUBCOMMAND> --help` — it is the authoritative usage + flags reference. For prose guides, examples, and exit-code narratives, see [`docs/CLI.md`](../CLI.md).\n\n",
    );
    out.push_str("Phase 3b of [#286](https://github.com/aegis-boot/aegis-boot/issues/286). Regenerate with:\n\n");
    out.push_str("```bash\ncargo build -p aegis-cli --release\ncargo run -p aegis-cli --bin cli-docgen --features docgen -- --write\n```\n\n");
    out.push_str("---\n\n");
    for (sub, help) in helps {
        writeln!(out, "## `aegis-boot {sub}`\n").expect("writing to String never fails");
        out.push_str("```text\n");
        // Normalize trailing whitespace: ensure exactly one \n at end
        // of captured block so the code fence sits tight to content.
        let trimmed = help.trim_end_matches('\n');
        out.push_str(trimmed);
        out.push('\n');
        out.push_str("```\n\n");
    }
    out
}

/// Shared entry point for the list drift-check. Returns the human
/// report lines (empty = no drift).
fn check_subcommand_list(repo_root: &Path) -> Result<Vec<String>, String> {
    let expected: BTreeSet<String> = SUBCOMMANDS.iter().map(|s| (*s).to_string()).collect();

    let cli_md_path = repo_root.join("docs/CLI.md");
    let man_path = repo_root.join("man/aegis-boot.1.in");

    let cli_md = fs::read_to_string(&cli_md_path)
        .map_err(|e| format!("cli-docgen: cannot read {}: {e}", cli_md_path.display()))?;
    let man = fs::read_to_string(&man_path)
        .map_err(|e| format!("cli-docgen: cannot read {}: {e}", man_path.display()))?;

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
    Ok(reports)
}

/// Build the synopsis content by invoking `aegis-boot X --help` for
/// every subcommand. Returns the rendered markdown, ready to diff or
/// write.
fn build_synopsis(aegis_boot_bin: &Path) -> Result<String, String> {
    let mut helps: BTreeMap<String, String> = BTreeMap::new();
    for sub in SUBCOMMANDS {
        let help = capture_help(aegis_boot_bin, sub)?;
        helps.insert((*sub).to_string(), help);
    }
    Ok(render_synopsis(&helps))
}

fn main() -> ExitCode {
    // Devtool — argv consists of `--check` / `--write` /
    // `--aegis-boot-bin PATH`. No security decisions key off argv.
    // Same safety story as `main.rs` and `constants-docgen`.
    // nosemgrep: rust.lang.security.args.args
    let args: Vec<String> = env::args().skip(1).collect();

    let (mode, bin_override) = match parse_mode(&args) {
        Ok(v) => v,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::from(2);
        }
    };

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

    // --- Step 1: subcommand list drift (always runs). ---
    let list_reports = match check_subcommand_list(&repo_root) {
        Ok(r) => r,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::from(2);
        }
    };

    // --- Step 2: synopsis emission / drift check. ---
    let aegis_boot_bin = match find_aegis_boot_bin(&repo_root, bin_override) {
        Ok(p) => p,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::from(2);
        }
    };
    let synopsis = match build_synopsis(&aegis_boot_bin) {
        Ok(s) => s,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::from(2);
        }
    };
    let synopsis_path = repo_root.join("docs/reference/CLI_SYNOPSIS.md");

    match mode {
        Mode::Write => {
            if let Some(parent) = synopsis_path.parent() {
                if let Err(e) = fs::create_dir_all(parent) {
                    eprintln!("cli-docgen: cannot create {}: {e}", parent.display());
                    return ExitCode::from(2);
                }
            }
            if let Err(e) = fs::write(&synopsis_path, &synopsis) {
                eprintln!("cli-docgen: cannot write {}: {e}", synopsis_path.display());
                return ExitCode::from(2);
            }
            println!("cli-docgen: wrote {}", synopsis_path.display());
            if !list_reports.is_empty() {
                for r in &list_reports {
                    eprint!("{r}");
                }
                eprintln!("cli-docgen: WARN — subcommand list drift still present (see above).");
                return ExitCode::from(1);
            }
            ExitCode::SUCCESS
        }
        Mode::Check => {
            let committed = fs::read_to_string(&synopsis_path).unwrap_or_default();
            let synopsis_drift = committed != synopsis;

            if list_reports.is_empty() && !synopsis_drift {
                println!(
                    "cli-docgen: OK — {} subcommands covered in docs/CLI.md, man/aegis-boot.1.in, and docs/reference/CLI_SYNOPSIS.md",
                    SUBCOMMANDS.len()
                );
                return ExitCode::SUCCESS;
            }
            for r in &list_reports {
                eprint!("{r}");
            }
            if synopsis_drift {
                eprintln!(
                    "drift in docs/reference/CLI_SYNOPSIS.md: regenerated content differs from committed copy"
                );
            }
            eprintln!();
            eprintln!("cli-docgen: FAIL — CLI documentation drift detected.");
            eprintln!(
                "Fix list drift: add a `## `aegis-boot X`` heading to docs/CLI.md and a `.TP` entry to man/aegis-boot.1.in for each missing subcommand (or remove from SUBCOMMANDS)."
            );
            eprintln!(
                "Fix synopsis drift: run `cargo run -p aegis-cli --bin cli-docgen --features docgen -- --write` locally and commit."
            );
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
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

    #[test]
    fn parse_mode_defaults_to_check() {
        let (m, bin) = parse_mode(&[]).unwrap();
        assert_eq!(m, Mode::Check);
        assert!(bin.is_none());
    }

    #[test]
    fn parse_mode_accepts_write() {
        let (m, _) = parse_mode(&["--write".to_string()]).unwrap();
        assert_eq!(m, Mode::Write);
    }

    #[test]
    fn parse_mode_accepts_bin_override() {
        let (m, bin) = parse_mode(&[
            "--check".to_string(),
            "--aegis-boot-bin".to_string(),
            "/usr/local/bin/aegis-boot".to_string(),
        ])
        .unwrap();
        assert_eq!(m, Mode::Check);
        assert_eq!(bin, Some(PathBuf::from("/usr/local/bin/aegis-boot")));
    }

    #[test]
    fn parse_mode_rejects_bin_override_without_path() {
        let err =
            parse_mode(&["--aegis-boot-bin".to_string()]).expect_err("must fail without path");
        assert!(err.contains("requires a path"));
    }

    #[test]
    fn parse_mode_rejects_unknown() {
        assert!(parse_mode(&["--garbage".to_string()]).is_err());
    }

    #[test]
    fn render_synopsis_includes_all_subcommands() {
        let mut helps = BTreeMap::new();
        helps.insert("init".to_string(), "USAGE: aegis-boot init\n".to_string());
        helps.insert("flash".to_string(), "USAGE: aegis-boot flash\n".to_string());
        let rendered = render_synopsis(&helps);
        assert!(rendered.contains("## `aegis-boot init`"));
        assert!(rendered.contains("## `aegis-boot flash`"));
        assert!(rendered.contains("USAGE: aegis-boot init"));
        assert!(rendered.contains("AUTO-GENERATED"));
    }

    #[test]
    fn render_synopsis_is_deterministic() {
        // BTreeMap iteration order is sorted; render should be stable
        // across two invocations of the same input.
        let mut helps = BTreeMap::new();
        helps.insert("z".to_string(), "Z\n".to_string());
        helps.insert("a".to_string(), "A\n".to_string());
        let r1 = render_synopsis(&helps);
        let r2 = render_synopsis(&helps);
        assert_eq!(r1, r2);
        // `a` appears before `z` in the output.
        let pos_a = r1.find("## `aegis-boot a`").unwrap();
        let pos_z = r1.find("## `aegis-boot z`").unwrap();
        assert!(pos_a < pos_z);
    }

    #[test]
    fn render_synopsis_trims_trailing_newlines_in_captured_help() {
        // Captured help text often ends with several `\n`; the render
        // should normalize to exactly one before the code fence.
        let mut helps = BTreeMap::new();
        helps.insert("x".to_string(), "A\nB\n\n\n".to_string());
        let rendered = render_synopsis(&helps);
        assert!(rendered.contains("A\nB\n```\n"));
        assert!(!rendered.contains("A\nB\n\n```"));
    }
}
