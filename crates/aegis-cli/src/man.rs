//! `aegis-boot man` — emit the man page to stdout.
//!
//! The canonical `aegis-boot(1)` source lives at `man/aegis-boot.1` in
//! the repo and ships via `install.sh`. This subcommand embeds it into
//! the binary via `include_str!` so operators can install the man page
//! without GitHub round-trips:
//!
//! ```bash
//! aegis-boot man | sudo tee /usr/local/share/man/man1/aegis-boot.1 > /dev/null
//! sudo mandb -q        # refresh the man index (if mandb is installed)
//! man aegis-boot       # now works
//! ```
//!
//! Pairs with `aegis-boot completions bash|zsh` for self-contained
//! discoverability — Homebrew formulas and other single-binary
//! distribution channels can produce both completions and man page
//! from the installed binary without needing the repo.

use std::process::ExitCode;

/// Raw contents of the rendered `aegis-boot.1` man page.
///
/// Sourced from the build-time template `man/aegis-boot.1.in` —
/// `build.rs` substitutes `@VERSION@` (from `CARGO_PKG_VERSION`,
/// which flows from `[workspace.package].version`) and `@DATE@`
/// (parsed from the top released entry in `CHANGELOG.md`) and writes
/// the result to `$OUT_DIR/aegis-boot.1`. Phase 1b of #286 / #287 —
/// removes the last manually-synced version reference.
///
/// Keeping the man page as a templated `.in` file (rather than inline
/// Rust string) means roff editors / preview tools / `man -l` work
/// directly on the source file during development; the rendered
/// `OUT_DIR/aegis-boot.1` can also be inspected after `cargo build`.
const MAN_PAGE: &str = include_str!(concat!(env!("OUT_DIR"), "/aegis-boot.1"));

pub fn run(args: &[String]) -> ExitCode {
    match try_run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => ExitCode::from(code),
    }
}

pub(crate) fn try_run(args: &[String]) -> Result<(), u8> {
    if matches!(args.first().map(String::as_str), Some("--help" | "-h")) {
        print_help();
        return Ok(());
    }
    // Any positional arg is a usage error — the subcommand takes none.
    if let Some(unexpected) = args.iter().find(|a| !a.starts_with("--")) {
        eprintln!("aegis-boot man: unexpected argument '{unexpected}'");
        eprintln!("run 'aegis-boot man --help' for usage");
        return Err(2);
    }
    // Emit via print!, not println!, so the trailing newline comes from
    // the man source file itself — preserves the exact bytes for any
    // downstream consumer doing a sha256 on the output.
    print!("{MAN_PAGE}");
    Ok(())
}

fn print_help() {
    println!("aegis-boot man — emit the aegis-boot(1) man page to stdout");
    println!();
    println!("USAGE:");
    println!("  aegis-boot man                              # print to stdout");
    println!("  aegis-boot man | sudo tee \\");
    println!("    /usr/local/share/man/man1/aegis-boot.1 > /dev/null");
    println!();
    println!("After installing:");
    println!("  sudo mandb -q                               # refresh man index");
    println!("  man aegis-boot                              # test it");
    println!();
    println!("See also: `aegis-boot completions bash` / `zsh` for shell completions.");
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

    #[test]
    fn try_run_help_returns_ok() {
        assert_eq!(try_run(&["--help".to_string()]), Ok(()));
        assert_eq!(try_run(&["-h".to_string()]), Ok(()));
    }

    #[test]
    fn try_run_with_no_args_emits_page() {
        assert_eq!(try_run(&[]), Ok(()));
    }

    #[test]
    fn try_run_with_unexpected_arg_is_usage_error() {
        assert_eq!(try_run(&["flash".to_string()]), Err(2));
        assert_eq!(
            try_run(&["/usr/local/share/man/man1/aegis-boot.1".to_string()]),
            Err(2)
        );
    }

    #[test]
    fn embedded_page_starts_with_th_header() {
        // `.TH` is the mandatory title macro that every man page
        // begins with. Catches the page getting corrupted or the
        // include_str! path breaking.
        assert!(
            MAN_PAGE.starts_with(".TH AEGIS-BOOT 1"),
            "man page must begin with `.TH AEGIS-BOOT 1 ...` title macro"
        );
    }

    #[test]
    fn embedded_page_contains_core_sections() {
        // Spot-check that every required section is present. Guardrails
        // against someone editing the .1 file in a way that drops a
        // section header.
        for section in [
            ".SH NAME",
            ".SH SYNOPSIS",
            ".SH DESCRIPTION",
            ".SH SUBCOMMANDS",
            ".SH EXIT STATUS",
        ] {
            assert!(
                MAN_PAGE.contains(section),
                "man page missing required section: {section}"
            );
        }
    }

    #[test]
    fn rendered_page_substitutes_version_from_cargo_pkg_version() {
        // Phase 1b contract: build.rs substitutes @VERSION@ into the
        // embedded page. The rendered page must carry the exact
        // CARGO_PKG_VERSION string (which via version.workspace = true
        // flows from [workspace.package].version). If the template or
        // build.rs ever regresses to emitting a hardcoded version or
        // dropping the substitution, this test fails.
        let expected = format!("\"aegis-boot {}\"", env!("CARGO_PKG_VERSION"));
        assert!(
            MAN_PAGE.contains(&expected),
            "rendered man page must contain the .TH header string {expected:?}; \
             got header line: {}",
            MAN_PAGE.lines().next().unwrap_or("<empty>")
        );
    }

    #[test]
    fn rendered_page_has_no_unresolved_template_markers() {
        // Catches a build.rs regression that forgot to substitute a
        // placeholder (or someone adding a new @FOO@ marker to the
        // template without wiring the substitution).
        assert!(
            !MAN_PAGE.contains("@VERSION@"),
            "rendered man page contains unresolved @VERSION@ marker — build.rs substitution regressed"
        );
        assert!(
            !MAN_PAGE.contains("@DATE@"),
            "rendered man page contains unresolved @DATE@ marker — build.rs substitution regressed"
        );
    }

    #[test]
    fn template_source_still_carries_placeholder_not_a_hardcoded_version() {
        // Enforces the Phase 1b single-source contract: the authored
        // template at man/aegis-boot.1.in MUST carry `@VERSION@`, not a
        // literal version string. If a maintainer accidentally
        // hand-edits a real version into the template, build.rs still
        // renders it (no-op substitution) but we've silently lost the
        // single-source property. This test catches that regression
        // at `cargo test` time.
        let template_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../man/aegis-boot.1.in");
        let template = std::fs::read_to_string(template_path)
            .expect("template must exist at man/aegis-boot.1.in");
        assert!(
            template.contains("@VERSION@"),
            "template must carry the literal `@VERSION@` placeholder — Phase 1b contract"
        );
        assert!(
            template.contains("@DATE@"),
            "template must carry the literal `@DATE@` placeholder"
        );
    }

    #[test]
    fn embedded_page_mentions_every_subcommand() {
        // The NAME of every subcommand must appear in the man page so
        // future readers find it. Mirrors the completions-coverage
        // test in completions.rs.
        for sub in [
            "init",
            "flash",
            "list",
            "add",
            "doctor",
            "recommend",
            "fetch",
            "attest",
            "eject",
            "update",
            "verify",
            "compat",
            "completions",
            "tour",
        ] {
            let marker = format!(".B {sub}");
            assert!(
                MAN_PAGE.contains(&marker),
                "man page missing subcommand bold-reference: {marker}"
            );
        }
    }
}
