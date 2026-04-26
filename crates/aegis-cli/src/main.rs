// SPDX-License-Identifier: MIT OR Apache-2.0

//! `aegis-boot` — operator CLI for aegis-boot.
//!
//! Subcommands:
//!   * `quickstart` — shortest-path: flash + Alpine (#352 UX-1)
//!   * `init`      — one-command rescue stick (flash + fetch + add profile)
//!   * `flash`     — write aegis-boot to a USB stick (3-step guided)
//!   * `add`       — copy + validate an ISO onto the stick
//!   * `list`      — show ISOs on the stick with verification status
//!   * `doctor`    — diagnose host environment + a stick's health
//!   * `recommend` — curated catalog of known-good ISOs
//!   * `fetch`     — download + verify a catalog ISO
//!   * `attest`    — list / show attestation receipts for past flashes
//!   * `eject`     — safely power-off and prepare a USB stick for removal
//!   * `update`    — in-place signed-chain rotation (phase 1: eligibility check)
//!   * `verify`    — re-run sha256 verification on every ISO on the stick
//!   * `compat`    — hardware compatibility lookup (verified reports only)
//!   * `tour`      — 30-second in-terminal walkthrough (#248)
//!
//! This replaces the developer workflow of running shell scripts
//! manually. The binary is named `aegis-boot` so operators type
//! `aegis-boot flash /dev/sdX` etc.

// Workspace-level `unsafe_code = "deny"` (Cargo.toml [workspace.lints])
// already denies `unsafe` everywhere. Binary-level `#![forbid]` was
// previously redundant AND hostile to narrow test-only allowlists —
// edition 2024 made `std::env::set_var` + `remove_var` unsafe, and
// the existing env-mutating tests in `attest.rs` + `fetch.rs`
// legitimately need to poke process env. Downgrading `forbid` → `deny`
// lets those specific test-site `#[allow(unsafe_code)]` tags compile
// while keeping production-path `unsafe` denied. No new unsafe blocks
// are introduced in non-test code.
#![deny(unsafe_code)]

mod attest;
mod bug_report;
mod bundle_cache;
mod bundle_fetch;
mod bundle_verify;
mod catalog;
mod catalog_resolvers;
mod cmd_path;
mod compat;
mod completions;
mod constants;
mod detect;
mod direct_install;
mod direct_install_manifest;
mod doctor;
mod eject;
mod fetch;
mod fetch_image;
mod flash;
mod init;
mod init_wizard;
mod inventory;
mod macos_direct_install;
mod man;
mod mounts;
mod paths;
mod plan;
mod quickstart;
mod readback;
mod redact;
mod tour;
mod update;
mod update_apply;
mod userfacing;
mod verify;
mod verify_stick;
mod windows_direct_install;

use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    // Standard CLI dispatch. We drop argv[0] (the rule's specific
    // concern) and use remaining args only as command names + paths
    // the user already controls. No security decision keys off argv.
    // nosemgrep: rust.lang.security.args.args
    let args: Vec<String> = env::args().skip(1).collect();
    let subcmd = args.first().map(std::string::String::as_str);

    match subcmd {
        Some("quickstart") => quickstart::run(&args[1..]),
        Some("init") => init::run(&args[1..]),
        Some("flash") => flash::run(&args[1..]),
        Some("list") => inventory::run_list(&args[1..]),
        Some("add") => inventory::run_add(&args[1..]),
        Some("doctor") => doctor::run(&args[1..]),
        Some("recommend") => catalog::run(&args[1..]),
        Some("fetch") => fetch::run(&args[1..]),
        Some("fetch-image") => fetch_image::run(&args[1..]),
        Some("fetch-trust-chain") => bundle_fetch::run(&args[1..]),
        Some("attest") => attest::run(&args[1..]),
        Some("bug-report") => bug_report::run(&args[1..]),
        Some("eject") => eject::run(&args[1..]),
        Some("update") => update::run(&args[1..]),
        Some("verify") => verify::run(&args[1..]),
        Some("compat") => compat::run(&args[1..]),
        Some("completions") => completions::run(&args[1..]),
        Some("man") => man::run(&args[1..]),
        Some("tour") => tour::run(&args[1..]),
        Some("-h" | "--help" | "help") | None => {
            print_help();
            ExitCode::SUCCESS
        }
        Some("--version" | "version") => {
            // `--json` can appear anywhere after the `--version` /
            // `version` subcommand. The envelope shape lives in
            // `aegis_wire_formats::Version` — a typed, drift-checked
            // wire contract that scripted consumers pin against
            // via `docs/reference/schemas/aegis-boot-version.schema.json`
            // (Phase 4b-1 of #286).
            if args.iter().skip(1).any(|a| a == "--json") {
                let envelope = aegis_wire_formats::Version {
                    schema_version: aegis_wire_formats::VERSION_SCHEMA_VERSION,
                    tool: "aegis-boot".to_string(),
                    version: env!("CARGO_PKG_VERSION").to_string(),
                };
                // `to_string_pretty` matches the multi-line style
                // used by the other `--json` surfaces (doctor,
                // list, verify, …). The full `--json` suite's
                // serde-refactor is tracked as Phase 4b of #290;
                // byte-level output for `--version --json`
                // changes from single-line to pretty across this
                // refactor — scripted consumers that parse via a
                // JSON library see no change (field shape is
                // identical), consumers that byte-grep the
                // whole string see a whitespace diff only.
                match serde_json::to_string_pretty(&envelope) {
                    Ok(body) => println!("{body}"),
                    Err(e) => {
                        eprintln!("aegis-boot: failed to serialize version envelope: {e}");
                        return ExitCode::from(2);
                    }
                }
            } else {
                println!("aegis-boot v{}", env!("CARGO_PKG_VERSION"));
            }
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("aegis-boot: unknown command '{other}'");
            eprintln!("run 'aegis-boot --help' for usage");
            ExitCode::from(2)
        }
    }
}

fn print_help() {
    println!("aegis-boot — Signed boot. Any ISO. Your keys.");
    println!();
    println!("USAGE:");
    println!("  aegis-boot quickstart <device> Shortest path: flash + Alpine 3.20 (#352)");
    println!("  aegis-boot init [device]       One-command rescue stick (flash + fetch + add)");
    println!("  aegis-boot flash [device]      Write aegis-boot to a USB stick");
    println!("  aegis-boot list [device]       Show ISOs on the stick");
    println!("  aegis-boot add <iso-or-slug>   Copy + validate an ISO (file path OR catalog slug)");
    println!("  aegis-boot doctor [--stick D]  Health check (host + stick)");
    println!("  aegis-boot recommend [slug]    Curated catalog of known-good ISOs");
    println!("  aegis-boot fetch <slug>        Download + verify a catalog ISO");
    println!("  aegis-boot fetch-image         Download + cosign-verify pre-built aegis-boot.img");
    println!("                                 (see `aegis-boot fetch-image --help` for flags)");
    println!("  aegis-boot fetch-trust-chain   Download + verify the signed-chain bundle (#417)");
    println!("                                 (ADR 0002 minisign; caches under XDG_CACHE_HOME)");
    println!("  aegis-boot attest [list|show]  Attestation receipts for past flashes");
    println!("  aegis-boot bug-report          Generate a redacted bug-report bundle (#342)");
    println!("  aegis-boot eject [device]      Safely power-off a stick before removal");
    println!("  aegis-boot update <device>     Check eligibility for in-place update");
    println!("  aegis-boot verify [device]     Re-verify every ISO's sha256 against its sidecar");
    println!("  aegis-boot compat [query]      Hardware compatibility lookup");
    println!("  aegis-boot completions <shell> Emit bash/zsh completion script");
    println!("  aegis-boot man                 Emit the aegis-boot(1) man page to stdout");
    println!("  aegis-boot tour                30-second in-terminal walkthrough");
    println!("  aegis-boot --version [--json]  Print version (--json emits schema_version=1)");
    println!("  aegis-boot --help              This message");
    println!();
    println!("NEW HERE? Run `aegis-boot quickstart /dev/sdX` for the fastest flash-to-boot");
    println!("path, or `aegis-boot tour` for the 30-second walkthrough.");
    println!();
    println!("Per-subcommand help: `aegis-boot <sub> --help`.");
    println!();
    println!("EXAMPLES:");
    println!("  aegis-boot quickstart /dev/sdc             # flash + Alpine in one command");
    println!("  aegis-boot init /dev/sdc                   # panic-room profile: 3 rescue ISOs");
    println!("  aegis-boot doctor                          # quick environment + stick health");
    println!("  aegis-boot recommend                       # browse the curated ISO catalog");
    println!("  aegis-boot add ubuntu-24.04-live-server    # fetch + add by catalog slug (#352)");
    println!("  aegis-boot add ./ubuntu.iso                # add a local ISO file");
    println!("  aegis-boot flash /dev/sdc                  # explicit device");
    println!("  aegis-boot attest list                     # show recorded flashes");
    println!("  aegis-boot eject /dev/sdc                  # sync + power-off before removal");
}
