//! `aegis-boot quickstart /dev/sdX` — shortest-path net-new-user flow.
//!
//! #352 UX-1: the single-command capstone of the sub-10-minute
//! flash-to-boot epic. Forwards to [`crate::init::run`] with
//! `--profile minimal --yes --direct-install` preset. Lands the
//! operator at a booted rescue-tui with Alpine 3.20 Standard ready in
//! the menu, without requiring them to remember the 4-step
//! `fetch-image → flash → fetch → add` recipe.
//!
//! Scope per #352 consensus vote (`higher_order`, 80% approve,
//! contrarian's data-loss flag incorporated):
//! - Device argument is **required** — no auto-detect. The contrarian
//!   correctly flagged that single-candidate-auto-detect risks nuking
//!   the wrong drive if the heuristic misclassifies a mounted USB.
//! - Uses `--direct-install` (from #274 Phase 3, merged earlier today)
//!   for the ~8× faster flash vs. the legacy dd path.
//! - Uses the `minimal` init profile (Alpine 3.20 Standard, ~200 MB)
//!   as the canonical fastest-to-useful ISO for a cold-start user. If
//!   the operator wants a different distro, they use
//!   `aegis-boot init --profile <name>` or `aegis-boot add <slug>`
//!   instead.
//!
//! This command intentionally stays a thin wrapper. Everything below
//! it is existing tested code paths (doctor → flash → fetch → add).
//! The UX win is discovery + sensible defaults, not new functionality.

use std::process::ExitCode;

/// Entry point for `aegis-boot quickstart /dev/sdX`.
pub fn run(args: &[String]) -> ExitCode {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_help();
        return ExitCode::SUCCESS;
    }

    // Rewrite the invocation as `init --profile minimal --yes
    // --direct-install <device-arg>` and forward. Keeps the composed
    // behavior in a single canonical place (init::run) rather than
    // re-implementing the flash+fetch+add dance here.
    let forwarded = build_forwarded_init_args(args);
    crate::init::run(&forwarded)
}

/// Translate `quickstart [/dev/sdX] [...forwarded]` into the init argv
/// form. Pure function so the composition is unit-testable without
/// running the actual pipeline.
pub(crate) fn build_forwarded_init_args(args: &[String]) -> Vec<String> {
    // Pre-seed the args init expects. Any additional args from the
    // operator (e.g. a device path) append after.
    let mut out: Vec<String> = vec![
        "--profile".to_string(),
        "minimal".to_string(),
        "--yes".to_string(),
        "--direct-install".to_string(),
    ];
    // Don't double-pass any of the flags we auto-set; forward the rest
    // verbatim (most commonly: the device path positional arg).
    let drop_set = ["--profile", "--yes", "--direct-install"];
    let mut skip_next = false;
    for a in args {
        if skip_next {
            skip_next = false;
            continue;
        }
        if a == "--profile" {
            // `--profile NAME` form: skip both tokens.
            skip_next = true;
            continue;
        }
        if drop_set.contains(&a.as_str()) || a.starts_with("--profile=") {
            continue;
        }
        out.push(a.clone());
    }
    out
}

fn print_help() {
    println!("aegis-boot quickstart — shortest path from stick to booted rescue");
    println!();
    println!("USAGE: aegis-boot quickstart /dev/sdX");
    println!();
    println!("Flashes the stick via --direct-install, fetches Alpine 3.20 Standard");
    println!("(~200 MiB) from the signed catalog, and stages it — one command from");
    println!("stick-in-hand to booted rescue-tui.");
    println!();
    println!("Behavior:");
    println!("  * The device arg is REQUIRED. Explicit path — no auto-detect.");
    println!("  * Equivalent to:  aegis-boot init --profile minimal --yes \\");
    println!("                                   --direct-install /dev/sdX");
    println!("  * --direct-install is ~8x faster than the legacy dd path on USB 2.0.");
    println!();
    println!("For a different ISO, use:");
    println!("  * aegis-boot init --profile panic-room /dev/sdX   # 3 ISOs, 5 GiB");
    println!("  * aegis-boot init --profile server     /dev/sdX   # 3 server distros");
    println!("  * aegis-boot flash /dev/sdX && aegis-boot add <slug>  # fine-grained");
    println!();
    println!("Related: #352 (sub-10-minute flash-to-boot epic).");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_forwarded_seeds_profile_minimal_yes_direct_install() {
        let forwarded = build_forwarded_init_args(&["/dev/sda".to_string()]);
        assert!(forwarded.contains(&"--profile".to_string()));
        assert!(forwarded.contains(&"minimal".to_string()));
        assert!(forwarded.contains(&"--yes".to_string()));
        assert!(forwarded.contains(&"--direct-install".to_string()));
        assert!(
            forwarded.contains(&"/dev/sda".to_string()),
            "device arg must be forwarded"
        );
    }

    #[test]
    fn build_forwarded_drops_duplicate_profile_arg_from_operator() {
        // Operator typed --profile something; we override with minimal.
        // No double --profile in the forwarded argv.
        let forwarded = build_forwarded_init_args(&[
            "--profile".to_string(),
            "panic-room".to_string(),
            "/dev/sda".to_string(),
        ]);
        let profile_count = forwarded
            .iter()
            .filter(|a| a.as_str() == "--profile")
            .count();
        assert_eq!(
            profile_count, 1,
            "only one --profile after override: {forwarded:?}"
        );
        assert!(forwarded.contains(&"minimal".to_string()));
        assert!(
            !forwarded.contains(&"panic-room".to_string()),
            "operator's --profile panic-room must be dropped: {forwarded:?}"
        );
    }

    #[test]
    fn build_forwarded_drops_profile_equals_form() {
        let forwarded =
            build_forwarded_init_args(&["--profile=server".to_string(), "/dev/sda".to_string()]);
        assert!(!forwarded.iter().any(|a| a == "--profile=server"));
        assert!(forwarded.contains(&"minimal".to_string()));
    }

    #[test]
    fn build_forwarded_drops_duplicate_yes_and_direct_install() {
        let forwarded = build_forwarded_init_args(&[
            "--yes".to_string(),
            "--direct-install".to_string(),
            "/dev/sda".to_string(),
        ]);
        assert_eq!(
            forwarded.iter().filter(|a| a.as_str() == "--yes").count(),
            1,
            "only one --yes: {forwarded:?}"
        );
        assert_eq!(
            forwarded
                .iter()
                .filter(|a| a.as_str() == "--direct-install")
                .count(),
            1,
            "only one --direct-install: {forwarded:?}"
        );
    }

    #[test]
    fn build_forwarded_preserves_pass_through_flags() {
        // Operator-supplied flags like --dry-run / --out-dir pass to init
        // untouched — we only swallow the 3 we auto-set.
        let forwarded = build_forwarded_init_args(&[
            "--dry-run".to_string(),
            "--out-dir".to_string(),
            "./out".to_string(),
            "/dev/sda".to_string(),
        ]);
        assert!(forwarded.contains(&"--dry-run".to_string()));
        assert!(forwarded.contains(&"--out-dir".to_string()));
        assert!(forwarded.contains(&"./out".to_string()));
    }
}
