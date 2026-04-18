//! `aegis-boot completions <shell>` — emit a shell completion script.
//!
//! Operators install with:
//!
//! ```bash
//! # bash
//! aegis-boot completions bash | sudo tee /etc/bash_completion.d/aegis-boot
//! # or transient:
//! source <(aegis-boot completions bash)
//!
//! # zsh (bash-compatible via bashcompinit)
//! aegis-boot completions zsh > ~/.zsh/completions/_aegis-boot
//! ```
//!
//! Scope: subcommand completion + a few slug-aware completions for
//! `aegis-boot recommend <TAB>` and `aegis-boot fetch <TAB>` (both use
//! the existing `recommend --slugs-only` path).
//!
//! Why hand-rolled and not `clap_complete`: the CLI parses argv by
//! hand (no clap/derive dep). Generating clap-flavored completions
//! would require adopting clap's struct-derive flow for all 13
//! subcommands. The subcommand surface is small and stable enough
//! that a maintained bash template is simpler.

use std::process::ExitCode;

/// Canonical list of top-level subcommands. Kept in sync with the
/// dispatch table in `main.rs`.
const SUBCOMMANDS: &[&str] = &[
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
    "man",
    "tour",
    "version",
    "help",
];

pub fn run(args: &[String]) -> ExitCode {
    match try_run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => ExitCode::from(code),
    }
}

pub(crate) fn try_run(args: &[String]) -> Result<(), u8> {
    let shell = args.first().map(String::as_str);
    match shell {
        Some("--help" | "-h") | None => {
            print_help();
            // Argumentless form is a usage error, not success — keeps
            // scripting honest (emitting the help to stdout then
            // returning 0 would poison a `>(completions bash)`
            // redirect the operator actually wanted).
            if shell.is_none() {
                return Err(2);
            }
            Ok(())
        }
        Some("bash") => {
            print!("{}", bash_completion());
            Ok(())
        }
        Some("zsh") => {
            // zsh's bashcompinit consumes bash completion scripts
            // unchanged; we prepend the two magic lines that tell
            // zsh to load the bash-compat shim. Purely-zsh native
            // completions (with _describe / _arguments) are a larger
            // project; deferred until there's a concrete ask.
            println!("#compdef aegis-boot");
            println!("autoload -U +X bashcompinit && bashcompinit");
            print!("{}", bash_completion());
            Ok(())
        }
        Some(other) => {
            eprintln!("aegis-boot completions: unknown shell '{other}'");
            eprintln!("supported: bash, zsh");
            Err(2)
        }
    }
}

fn print_help() {
    println!("aegis-boot completions — emit shell completion scripts");
    println!();
    println!("USAGE:");
    println!("  aegis-boot completions bash       # print bash completion to stdout");
    println!("  aegis-boot completions zsh        # print zsh completion to stdout");
    println!();
    println!("INSTALL:");
    println!("  # bash (persistent):");
    println!("  aegis-boot completions bash | sudo tee /etc/bash_completion.d/aegis-boot");
    println!();
    println!("  # bash (transient, current shell only):");
    println!("  source <(aegis-boot completions bash)");
    println!();
    println!("  # zsh:");
    println!("  aegis-boot completions zsh > ~/.zsh/completions/_aegis-boot");
}

/// Build the bash completion script. Static-string with string
/// interpolation for the subcommand list so changes to `SUBCOMMANDS`
/// propagate without hand-editing template.
fn bash_completion() -> String {
    let subcmds = SUBCOMMANDS.join(" ");
    format!(
        r#"# aegis-boot bash completion (hand-rolled)
#
# Completes:
#   * top-level subcommands
#   * `aegis-boot recommend <TAB>` / `aegis-boot fetch <TAB>` → catalog slugs
#     (uses `aegis-boot recommend --slugs-only`)
#   * `aegis-boot compat <TAB>` → vendor + model tokens from compat DB
#     (uses `aegis-boot compat --json` + jq, falls back silently if jq missing)
#   * `aegis-boot completions <TAB>` → bash / zsh

_aegis_boot_completions() {{
    local cur prev
    COMPREPLY=()
    cur="${{COMP_WORDS[COMP_CWORD]}}"
    prev="${{COMP_WORDS[COMP_CWORD-1]}}"

    # First arg: subcommand.
    if [[ $COMP_CWORD -eq 1 ]]; then
        COMPREPLY=( $(compgen -W "{subcmds}" -- "$cur") )
        return 0
    fi

    case "${{COMP_WORDS[1]}}" in
        recommend|fetch)
            # Second arg: catalog slug. Fall through to --help / --json
            # if the user is typing flags. `recommend --slugs-only` is
            # the contract — one slug per line.
            if [[ "$cur" != -* ]]; then
                local slugs
                slugs=$(aegis-boot recommend --slugs-only 2>/dev/null) || return 0
                COMPREPLY=( $(compgen -W "$slugs" -- "$cur") )
            else
                COMPREPLY=( $(compgen -W "--help --json --slugs-only" -- "$cur") )
            fi
            return 0
            ;;
        compat)
            # Offer --my-machine + vendor tokens. The vendor list comes
            # from the compat DB via --json; parsed via jq when
            # available, otherwise skipped silently.
            if [[ "$cur" == -* ]]; then
                COMPREPLY=( $(compgen -W "--help --json --my-machine" -- "$cur") )
                return 0
            fi
            if command -v jq >/dev/null 2>&1; then
                local vendors
                vendors=$(aegis-boot compat --json 2>/dev/null | \
                          jq -r '.entries[]?.vendor' 2>/dev/null | sort -u)
                COMPREPLY=( $(compgen -W "$vendors" -- "$cur") )
            fi
            return 0
            ;;
        doctor|list|attest|verify|update)
            COMPREPLY=( $(compgen -W "--help --json --stick" -- "$cur") )
            return 0
            ;;
        init|flash)
            # These take a device path. Let bash's default path
            # completion handle it.
            COMPREPLY=( $(compgen -f -- "$cur") )
            return 0
            ;;
        completions)
            COMPREPLY=( $(compgen -W "bash zsh" -- "$cur") )
            return 0
            ;;
    esac
}}

complete -F _aegis_boot_completions aegis-boot
"#
    )
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
    fn try_run_with_no_args_is_usage_error() {
        // Printing the help + returning 0 would poison a redirect like
        // `aegis-boot completions > /etc/bash_completion.d/aegis-boot`
        // where the operator forgot the shell arg. Exit 2 instead.
        assert_eq!(try_run(&[]), Err(2));
    }

    #[test]
    fn try_run_help_flag_returns_ok() {
        assert_eq!(try_run(&["--help".to_string()]), Ok(()));
        assert_eq!(try_run(&["-h".to_string()]), Ok(()));
    }

    #[test]
    fn try_run_bash_returns_ok() {
        assert_eq!(try_run(&["bash".to_string()]), Ok(()));
    }

    #[test]
    fn try_run_zsh_returns_ok() {
        assert_eq!(try_run(&["zsh".to_string()]), Ok(()));
    }

    #[test]
    fn try_run_unknown_shell_returns_two() {
        assert_eq!(try_run(&["fish".to_string()]), Err(2));
        assert_eq!(try_run(&["pwsh".to_string()]), Err(2));
    }

    #[test]
    fn bash_completion_contains_every_subcommand() {
        let script = bash_completion();
        for sub in SUBCOMMANDS {
            assert!(
                script.contains(sub),
                "generated completion script missing subcommand '{sub}'"
            );
        }
    }

    #[test]
    fn bash_completion_has_complete_binding() {
        let script = bash_completion();
        assert!(script.contains("complete -F _aegis_boot_completions aegis-boot"));
    }

    #[test]
    fn subcommands_list_has_core_entries() {
        // Guardrail against accidental over-delete when refactoring.
        // (`is_empty()` is a const-eval expression on SUBCOMMANDS
        // literal, so clippy flags it — the .contains checks below
        // suffice as a non-emptiness proof.)
        assert!(SUBCOMMANDS.contains(&"compat"));
        assert!(SUBCOMMANDS.contains(&"doctor"));
        assert!(SUBCOMMANDS.contains(&"completions"));
    }
}
