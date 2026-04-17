//! `aegis-boot` — operator CLI for aegis-boot.
//!
//! Subcommands:
//!   * `init`      — one-command rescue stick (flash + fetch + add profile)
//!   * `flash`     — write aegis-boot to a USB stick (3-step guided)
//!   * `add`       — copy + validate an ISO onto the stick
//!   * `list`      — show ISOs on the stick with verification status
//!   * `doctor`    — diagnose host environment + a stick's health
//!   * `recommend` — curated catalog of known-good ISOs
//!   * `fetch`     — download + verify a catalog ISO
//!   * `attest`    — list / show attestation receipts for past flashes
//!
//! This replaces the developer workflow of running shell scripts
//! manually. The binary is named `aegis-boot` so operators type
//! `aegis-boot flash /dev/sdX` etc.

#![forbid(unsafe_code)]

mod attest;
mod catalog;
mod detect;
mod doctor;
mod fetch;
mod flash;
mod init;
mod inventory;

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
        Some("init") => init::run(&args[1..]),
        Some("flash") => flash::run(&args[1..]),
        Some("list") => inventory::run_list(&args[1..]),
        Some("add") => inventory::run_add(&args[1..]),
        Some("doctor") => doctor::run(&args[1..]),
        Some("recommend") => catalog::run(&args[1..]),
        Some("fetch") => fetch::run(&args[1..]),
        Some("attest") => attest::run(&args[1..]),
        Some("-h" | "--help" | "help") | None => {
            print_help();
            ExitCode::SUCCESS
        }
        Some("--version" | "version") => {
            println!("aegis-boot v{}", env!("CARGO_PKG_VERSION"));
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
    println!("  aegis-boot init [device]      One-command rescue stick (flash + fetch + add)");
    println!("  aegis-boot flash [device]     Write aegis-boot to a USB stick");
    println!("  aegis-boot list [device]      Show ISOs on the stick");
    println!("  aegis-boot add <iso> [device] Copy + validate an ISO");
    println!("  aegis-boot doctor [--stick D] Health check (host + stick)");
    println!("  aegis-boot recommend [slug]   Curated catalog of known-good ISOs");
    println!("  aegis-boot fetch <slug>       Download + verify a catalog ISO");
    println!("  aegis-boot attest [list|show] Attestation receipts for past flashes");
    println!("  aegis-boot --version          Print version");
    println!("  aegis-boot --help             This message");
    println!();
    println!("EXAMPLES:");
    println!("  aegis-boot init /dev/sdc      # panic-room profile: flash + 3 rescue ISOs");
    println!("  aegis-boot doctor             # quick environment + stick health");
    println!("  aegis-boot recommend          # browse the curated ISO catalog");
    println!("  aegis-boot fetch ubuntu-24.04-live-server  # download + verify");
    println!("  aegis-boot flash              # auto-detect removable drive");
    println!("  aegis-boot flash /dev/sdc     # specific drive");
    println!("  aegis-boot add ubuntu.iso     # validate + copy to stick");
    println!("  aegis-boot attest list        # show recorded flashes");
}
