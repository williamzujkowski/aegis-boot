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

/// Raw contents of `man/aegis-boot.1`. The path is relative to this
/// source file (`crates/aegis-cli/src/man.rs` → `../../man/...`).
/// Keeping the man page as a sibling `.1` file (rather than inline
/// Rust string) means roff editors / preview tools / `man -l` work
/// directly on the source file during development.
const MAN_PAGE: &str = include_str!("../../../man/aegis-boot.1");

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
