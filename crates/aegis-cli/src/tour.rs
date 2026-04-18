//! `aegis-boot tour` — 30-second in-terminal walkthrough.
//!
//! Operators who land on `aegis-boot --help` see 16 subcommands and no
//! sense of which 4 they actually need to make a stick. `tour` answers
//! that question without leaving the terminal. Tracks #248.
//!
//! Non-interactive on purpose — printing a numbered guide to stdout is
//! cheap, scriptable, and works in CI / on dumb terminals. The "press 1
//! to dive deeper" interactive variant from the original #248 sketch
//! adds enough complexity (raw-mode terminal, async key handling) that
//! it's better as a follow-up; `--help` per subcommand already covers
//! the deep-dive surface.

use std::process::ExitCode;

/// Entry point for `aegis-boot tour [--help]`.
pub fn run(args: &[String]) -> ExitCode {
    if args.first().map(String::as_str) == Some("--help")
        || args.first().map(String::as_str) == Some("-h")
    {
        println!("aegis-boot tour — 30-second in-terminal walkthrough");
        println!();
        println!("USAGE: aegis-boot tour");
        println!();
        println!("Prints the 4-command path to a working rescue stick, plus");
        println!("pointers to docs/HOW_IT_WORKS.md and docs/TOUR.md. No flags.");
        return ExitCode::SUCCESS;
    }
    print_tour();
    ExitCode::SUCCESS
}

fn print_tour() {
    println!("aegis-boot — Signed boot. Any ISO. Your keys.");
    println!();
    println!("aegis-boot is a USB rescue-stick builder with Secure Boot built-in.");
    println!("Other multi-ISO tools (Ventoy, YUMI) ask you to disable Secure Boot");
    println!("OR enroll their key into your firmware. aegis-boot reuses the same");
    println!("Microsoft-signed shim → grub → kernel chain real distros use, so the");
    println!("stick boots on every laptop with default firmware out of the box.");
    println!();
    println!("You're 4 commands away from a working rescue stick:");
    println!();
    println!("  1. aegis-boot doctor                check your host has the prereqs");
    println!("  2. aegis-boot init /dev/sdX         flash a fresh USB stick");
    println!("  3. aegis-boot fetch ubuntu-24.04   download a verified rescue ISO");
    println!("  4. aegis-boot add ubuntu.iso       copy the ISO onto the stick");
    println!();
    println!("Each subcommand accepts --help for full usage and examples.");
    println!();
    println!("To learn the trust model:");
    println!("  - docs/HOW_IT_WORKS.md          5-minute conceptual walkthrough");
    println!("  - docs/TOUR.md                  step-by-step first-time guide");
    println!("  - docs/USB_LAYOUT.md            partition layout + signed-chain map");
    println!();
    println!("To see your laptop's compatibility:  aegis-boot compat --my-machine");
    println!("To see what's on a stick:            aegis-boot list /dev/sdX");
    println!("To audit past flashes:               aegis-boot attest list");
    println!();
    println!("For the full subcommand list:        aegis-boot --help");
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn run_returns_success_with_no_args() {
        // Smoke test — just exercises the print path without capturing
        // stdout (println in tests is fine; stdout is captured by the
        // test harness).
        let code = run(&[]);
        assert_eq!(code, ExitCode::SUCCESS);
    }

    #[test]
    fn run_returns_success_for_help() {
        let code = run(&["--help".to_string()]);
        assert_eq!(code, ExitCode::SUCCESS);
    }

    #[test]
    fn run_returns_success_for_short_help() {
        let code = run(&["-h".to_string()]);
        assert_eq!(code, ExitCode::SUCCESS);
    }
}
