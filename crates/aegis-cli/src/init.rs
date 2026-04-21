// SPDX-License-Identifier: MIT OR Apache-2.0

//! `aegis-boot init` — one-command rescue stick: flash + fetch + add.
//!
//! Composes the existing primitives (`doctor`, `flash`, `fetch`,
//! `inventory::run_add`) behind a single verb so an operator goes from
//! stick-in-hand to rescue-ready without remembering the 4-step recipe.
//!
//! A **profile** is a named, constant bundle of catalog slugs. The
//! default profile `panic-room` ships three ISOs chosen to cover the
//! most common rescue cases on a 16 GB stick: fast minimal (Alpine),
//! familiar server (Ubuntu), enterprise (Rocky).
//!
//! Flow:
//!   1. `doctor --stick <dev>` preflight (fail closed on BROKEN unless
//!      `--yes` is given).
//!   2. `flash <dev> --yes` (same device, skipping the typed 'flash'
//!      confirmation — the operator already consented at the `init`
//!      layer by passing `--yes`, or declined to do so at step 1).
//!   3. For each slug in the profile: `fetch <slug>` (idempotent via
//!      `$XDG_CACHE_HOME/aegis-boot/<slug>/`) then `aegis-boot add
//!      <iso-path>` (auto-finds the freshly-flashed mount).
//!
//! The single attestation manifest written by `flash` is appended to by
//! every `add` call (existing plumbing in `attest.rs`), so the whole
//! `init` run produces one audit record.

use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::process::{Command, ExitCode};

use crate::catalog::find_entry;
use crate::init_wizard;

/// A named bundle of catalog slugs for `aegis-boot init --profile ...`.
pub struct Profile {
    pub name: &'static str,
    pub description: &'static str,
    pub slugs: &'static [&'static str],
}

/// Default profile — emergency recovery kit.
///
/// Three ISOs from the verified catalog (post-#159) covering the common
/// rescue scenarios: minimal/fast boot (alpine), familiar-to-every-sysadmin
/// server (ubuntu), RHEL-family enterprise rescue (rocky). Total on-disk
/// footprint ~5 GiB — fits on a 16 GB stick with room for operator-added
/// ISOs.
pub const PANIC_ROOM: Profile = Profile {
    name: "panic-room",
    description: "Emergency recovery kit — Alpine 3.20 + Ubuntu 24.04 Server + Rocky 9",
    slugs: &[
        "alpine-3.20-standard",
        "ubuntu-24.04-live-server",
        "rocky-9-minimal",
    ],
};

/// Smallest possible kit — just Alpine for basic rescue work. Fastest
/// `init` run (single ~200 MiB download), fits on any stick. Useful when
/// the target is "I just need a known-good Linux userspace to poke at
/// this disk" and the operator doesn't want to wait for 5 GiB of GPG
/// verification.
pub const MINIMAL: Profile = Profile {
    name: "minimal",
    description: "Fastest rescue stick — Alpine 3.20 only (~200 MiB)",
    slugs: &["alpine-3.20-standard"],
};

/// Enterprise server triple — RHEL-family + Ubuntu-family, all three
/// "known signed by a vendor our operators trust" minimal installers.
/// No desktop; no live session. For operators whose targets are servers,
/// not laptops. Total ~6 GiB.
pub const SERVER: Profile = Profile {
    name: "server",
    description: "Enterprise server rescue — Ubuntu 24.04 Server + Rocky 9 + AlmaLinux 9",
    slugs: &[
        "ubuntu-24.04-live-server",
        "rocky-9-minimal",
        "almalinux-9-minimal",
    ],
};

/// Registry of known profiles. Keep in sync with `--help` output below.
///
/// Ordering matters for the help output: operators see the list in this
/// order, so put the default (panic-room) first, then the other choices
/// roughly by expected frequency of use.
pub const PROFILES: &[&Profile] = &[&PANIC_ROOM, &MINIMAL, &SERVER];

/// Entry point for `aegis-boot init [/dev/sdX] [--profile NAME] [--yes]`.
pub fn run(args: &[String]) -> ExitCode {
    // Help-first so we don't prompt on an accidentally-empty invocation.
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_help();
        return ExitCode::SUCCESS;
    }

    // Machine-readable profile enumeration for shell completion scripts
    // (complements `aegis-boot recommend --slugs-only`). One profile name
    // per line, nothing else on stdout. Stable contract — completion
    // scripts parse this line-for-line.
    if args.iter().any(|a| a == "--list-profiles") {
        for profile in PROFILES {
            println!("{}", profile.name);
        }
        return ExitCode::SUCCESS;
    }

    let parsed = match parse_flags(args) {
        Ok(p) => p,
        Err(msg) => {
            eprintln!("aegis-boot init: {msg}");
            return ExitCode::from(2);
        }
    };

    let Some(profile) = resolve_profile(&parsed.profile_name) else {
        eprintln!("aegis-boot init: unknown profile '{}'", parsed.profile_name);
        print_profile_list();
        return ExitCode::from(1);
    };

    // Fail fast on a profile that references a slug not in the catalog.
    // This catches authorship errors at the boundary instead of after
    // we've already flashed the stick.
    for slug in profile.slugs {
        if find_entry(slug).is_none() {
            eprintln!(
                "aegis-boot init: profile '{}' references unknown catalog slug '{}'",
                profile.name, slug
            );
            eprintln!("(this is a bug — please report)");
            return ExitCode::from(1);
        }
    }

    print_header(profile, parsed.device.as_deref());

    // PR2 of #245: interactive wizard with serial-confirmation safety
    // gate. Triggers when no explicit device was passed AND not running
    // unattended (--yes). Lets the operator pick a removable USB drive
    // by number, see its serial, type the last 4 chars to confirm, then
    // hands the resolved device path back to flash with --yes (the
    // wizard already did the human-confirmation work).
    //
    // Skipped when --device is explicit (operator already chose) OR
    // --yes is set (unattended; no human to type the serial token).
    let mut device = parsed.device.clone();
    let wizard_confirmed_device = device.is_none() && !parsed.yes;
    if wizard_confirmed_device {
        match run_init_wizard(parsed.force) {
            Ok(dev) => device = Some(dev),
            Err(msg) => {
                eprintln!("aegis-boot init: {msg}");
                return ExitCode::from(1);
            }
        }
    }

    if !parsed.skip_doctor {
        if let Err(code) = doctor_preflight(device.as_deref(), parsed.yes) {
            return ExitCode::from(code);
        }
    }

    // The wizard's serial-confirmation IS the destructive consent gate.
    // Once it passes, downstream flash should not re-prompt — pass --yes
    // to flash so the operator isn't asked to type 'flash' a second time
    // for the device they already typed the serial of.
    let flash_yes = parsed.yes || wizard_confirmed_device;
    if let Err(code) = flash_step(device.as_deref(), flash_yes, parsed.direct_install) {
        return ExitCode::from(code);
    }

    for slug in profile.slugs {
        if let Err(code) = fetch_and_add_step(slug, parsed.skip_gpg) {
            return ExitCode::from(code);
        }
    }

    print_success(profile);
    ExitCode::SUCCESS
}

// ---- flow steps -------------------------------------------------------------

fn doctor_preflight(device: Option<&str>, assume_yes: bool) -> Result<(), u8> {
    println!("--- doctor preflight ---");
    let mut doctor_args: Vec<String> = Vec::new();
    if let Some(dev) = device {
        doctor_args.push("--stick".to_string());
        doctor_args.push(dev.to_string());
    }
    match crate::doctor::try_run(&doctor_args) {
        Ok(()) => Ok(()),
        Err(_) if assume_yes => {
            eprintln!();
            eprintln!("aegis-boot init: doctor reported failures but --yes given; continuing.");
            Ok(())
        }
        Err(code) => {
            eprintln!();
            eprintln!("aegis-boot init: doctor preflight FAILED.");
            eprintln!("Fix the issues above and re-run, or pass --yes to override.");
            Err(code)
        }
    }
}

fn flash_step(device: Option<&str>, assume_yes: bool, direct_install: bool) -> Result<(), u8> {
    println!();
    println!("--- flash stick ---");
    let mut flash_args: Vec<String> = Vec::new();
    if let Some(d) = device {
        flash_args.push(d.to_string());
    }
    if assume_yes {
        flash_args.push("--yes".to_string());
    }
    // #352 UX-1 / #374: `quickstart` forwards `--direct-install` through
    // `init` so the nested `flash` invocation uses the Rust-native
    // partition+stage pipeline (#274) instead of the legacy dd path.
    // Pre-#374, init silently dropped the flag and flash ran dd,
    // negating the quickstart speedup.
    if direct_install {
        flash_args.push("--direct-install".to_string());
    }
    crate::flash::try_run(&flash_args).inspect_err(|_| {
        eprintln!();
        eprintln!("aegis-boot init: flash step failed; stopping.");
    })
}

fn fetch_and_add_step(slug: &str, skip_gpg: bool) -> Result<(), u8> {
    println!();
    println!("--- {slug} ---");

    let mut fetch_args: Vec<String> = Vec::new();
    if skip_gpg {
        fetch_args.push("--no-gpg".to_string());
    }
    fetch_args.push(slug.to_string());
    crate::fetch::try_run(&fetch_args).inspect_err(|_| {
        eprintln!();
        eprintln!("aegis-boot init: fetch of '{slug}' failed; stopping.");
        eprintln!("(re-run this command to resume — already-downloaded files are cached)");
    })?;

    let iso_path = cached_iso_path(slug).ok_or_else(|| {
        eprintln!();
        eprintln!("aegis-boot init: could not locate fetched ISO for '{slug}' in the cache dir.");
        eprintln!("(this indicates a mismatch between the catalog URL and the cached filename.)");
        1u8
    })?;

    let add_args = vec![iso_path.display().to_string()];
    crate::inventory::try_run_add(&add_args).inspect_err(|_| {
        eprintln!();
        eprintln!("aegis-boot init: add of '{slug}' failed; stopping.");
    })
}

// ---- helpers ----------------------------------------------------------------

/// Derive the path to a fetched ISO from its slug, matching the naming
/// convention used by `fetch::download` (filename is the last `/`-segment
/// of the catalog URL) and the default cache-dir convention
/// (`$XDG_CACHE_HOME/aegis-boot/<slug>/`). Returns `None` if the file
/// isn't present on disk.
fn cached_iso_path(slug: &str) -> Option<PathBuf> {
    let entry = find_entry(slug)?;
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    let filename = entry.iso_url.rsplit('/').next().unwrap_or("");
    let p = base.join("aegis-boot").join(slug).join(filename);
    p.is_file().then_some(p)
}

fn resolve_profile(name: &str) -> Option<&'static Profile> {
    PROFILES.iter().copied().find(|p| p.name == name)
}

// ---- arg parsing ------------------------------------------------------------

#[derive(Debug)]
// All five flags are independent booleans on the command-line surface;
// packing into a bitflag struct (clippy::struct_excessive_bools warns
// at >3) would obscure the 1:1 mapping to argv flags without buying
// anything. Allow at the type level.
#[allow(clippy::struct_excessive_bools)]
struct Parsed {
    profile_name: String,
    device: Option<String>,
    yes: bool,
    skip_doctor: bool,
    skip_gpg: bool,
    /// `--force` skips the wizard's "device is currently mounted" gate
    /// (#245). Useful when the operator deliberately wants to flash a
    /// stick that's currently mounted (e.g. `AEGIS_ISOS` already in use
    /// by `aegis-boot list`); rare in practice.
    force: bool,
    /// `--direct-install` passes through to the nested `flash` step so
    /// operators using `init` (and transitively `quickstart`) get the
    /// Rust-native partition+stage pipeline (#274) instead of legacy
    /// dd. Default off — opt-in for now, mirrors flash's own default.
    direct_install: bool,
}

fn parse_flags(args: &[String]) -> Result<Parsed, String> {
    let mut profile_name: Option<String> = None;
    let mut device: Option<String> = None;
    let mut yes = false;
    let mut skip_doctor = false;
    let mut skip_gpg = false;
    let mut force = false;
    let mut direct_install = false;
    let mut iter = args.iter();
    while let Some(a) = iter.next() {
        match a.as_str() {
            "--help" | "-h" => {
                // `run` pre-filters --help; parse_flags is only called
                // from there *after* the check. Unreachable in practice,
                // but keep the match exhaustive for unit tests that
                // might call parse_flags directly.
                return Err("--help handled by caller".to_string());
            }
            "--yes" | "-y" => yes = true,
            "--no-doctor" => skip_doctor = true,
            "--no-gpg" => skip_gpg = true,
            "--force" => force = true,
            // #374: `quickstart` forwards --direct-install through
            // `init`; without explicit recognition here, init rejected
            // the flag and quickstart failed before any work started.
            "--direct-install" => direct_install = true,
            "--profile" => {
                let Some(v) = iter.next() else {
                    return Err("--profile requires a name argument".to_string());
                };
                profile_name = Some(v.clone());
            }
            arg if arg.starts_with("--profile=") => {
                profile_name = Some(arg.trim_start_matches("--profile=").to_string());
            }
            arg if arg.starts_with("--") => {
                return Err(format!("unknown option '{arg}'"));
            }
            other => {
                if device.is_some() {
                    return Err(format!(
                        "only one device allowed (got '{other}' after '{}')",
                        device.unwrap_or_else(|| "?".into()),
                    ));
                }
                device = Some(other.to_string());
            }
        }
    }
    Ok(Parsed {
        profile_name: profile_name.unwrap_or_else(|| PANIC_ROOM.name.to_string()),
        device,
        yes,
        skip_doctor,
        skip_gpg,
        force,
        direct_install,
    })
}

// ---- interactive wizard (#245 PR2) -----------------------------------------

/// Drive the operator through the serial-confirmation safety gate from
/// [`init_wizard`]. Returns the resolved `/dev/sdX` path on confirmation,
/// or an `Err` describing what went wrong / what the operator typed
/// instead of accepting.
///
/// Called when `init` is invoked without an explicit device AND without
/// `--yes`. The wizard performs all the destructive consent in one go,
/// so the downstream `flash` step gets `--yes` and won't re-prompt.
fn run_init_wizard(force: bool) -> Result<String, String> {
    println!("--- pick a target stick ---");
    let lsblk_out = run_lsblk_json()?;
    let drives = init_wizard::parse_lsblk_removable_usb(&lsblk_out)
        .map_err(|e| format!("lsblk JSON parse: {e}"))?;
    if drives.is_empty() {
        return Err(concat!(
            "No removable USB drives detected.\n",
            "  - plug a USB stick in and try again, or\n",
            "  - pass an explicit device:  aegis-boot init /dev/sdX --yes",
        )
        .to_string());
    }

    print!("{}", init_wizard::format_drive_menu(&drives));
    println!();

    let idx = if drives.len() == 1 {
        // One device — confirm "Y" to use, anything else cancels. Saves
        // the operator typing a numeral when there's nothing to choose
        // from.
        print!("Use {} [Y/n]: ", drives[0].dev.display());
        io::stdout().flush().ok();
        let line = read_stdin_line()?;
        let trimmed = line.trim();
        if !(trimmed.is_empty() || trimmed.eq_ignore_ascii_case("y")) {
            return Err("Cancelled (single drive declined).".to_string());
        }
        0
    } else {
        print!("Select target [1-{}]: ", drives.len());
        io::stdout().flush().ok();
        let line = read_stdin_line()?;
        init_wizard::parse_menu_selection(&line, drives.len()).ok_or_else(|| {
            format!(
                "Invalid selection (expected 1-{}, got '{}').",
                drives.len(),
                line.trim()
            )
        })?
    };
    let chosen = &drives[idx];

    // Refuse-on-mounted gate. Catches "operator forgot to eject the
    // previous AEGIS_ISOS run". --force overrides for the rare cases
    // (deliberate reformat, partition busy by another aegis-boot
    // subcommand we tolerate).
    if !force {
        let findmnt_out = run_findmnt_json(&chosen.dev.display().to_string()).unwrap_or_default();
        if let Ok(true) = init_wizard::is_target_mounted(&findmnt_out) {
            return Err(format!(
                "{} is currently mounted. Unmount it first or pass --force.",
                chosen.dev.display()
            ));
        }
    }

    let Some(serial) = chosen.serial.as_deref() else {
        return Err(format!(
            "{} has no kernel-reported serial number; can't drive the serial-confirmation gate. \
             Pass an explicit device + --yes to bypass: aegis-boot init {} --yes",
            chosen.dev.display(),
            chosen.dev.display()
        ));
    };
    let token = init_wizard::serial_token(serial).ok_or_else(|| {
        format!(
            "Serial '{serial}' has fewer than {} alphanumeric chars; cannot \
             produce a confirmation token. Pass --yes to bypass.",
            init_wizard::SERIAL_CONFIRMATION_LEN,
        )
    })?;

    println!();
    println!(
        "You selected {} ({}, {}).",
        chosen.dev.display(),
        chosen.model,
        chosen.size_human()
    );
    println!("ALL DATA on this device WILL BE ERASED.");
    println!();
    print!(
        "Confirm by typing the last {} chars of the serial '{}': ",
        init_wizard::SERIAL_CONFIRMATION_LEN,
        serial
    );
    io::stdout().flush().ok();
    let confirm = read_stdin_line()?;
    if !init_wizard::serial_matches(&confirm, &token) {
        return Err(format!(
            "Serial confirmation did not match (expected last {} chars '{}'). Cancelled.",
            init_wizard::SERIAL_CONFIRMATION_LEN,
            token
        ));
    }
    println!("  ✓ Match. Proceeding.");
    println!();
    println!("{}", init_wizard::trust_narrative_paragraph());
    println!();
    print!("Press Enter to continue, or Ctrl-C to abort: ");
    io::stdout().flush().ok();
    let _ack = read_stdin_line()?;
    Ok(chosen.dev.display().to_string())
}

/// Run `lsblk -J -b -o NAME,SIZE,MODEL,SERIAL,RM,TRAN` and return its
/// stdout. Output is JSON suitable for [`init_wizard::parse_lsblk_removable_usb`].
fn run_lsblk_json() -> Result<String, String> {
    let out = Command::new("lsblk")
        .args(["-J", "-b", "-o", "NAME,SIZE,MODEL,SERIAL,RM,TRAN"])
        .output()
        .map_err(|e| format!("lsblk exec: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "lsblk failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    String::from_utf8(out.stdout).map_err(|e| format!("lsblk stdout not UTF-8: {e}"))
}

/// Run `findmnt -J <device>` and return its stdout. Empty stdout (no
/// match) returns `Ok("")`. Output is JSON suitable for
/// [`init_wizard::is_target_mounted`].
fn run_findmnt_json(dev: &str) -> Result<String, String> {
    let out = Command::new("findmnt")
        .args(["-J", dev])
        .output()
        .map_err(|e| format!("findmnt exec: {e}"))?;
    // findmnt returns exit 1 when nothing matches — that's "not mounted",
    // not an error. Treat any non-zero exit as "not mounted, no JSON".
    if !out.status.success() {
        return Ok(String::new());
    }
    String::from_utf8(out.stdout).map_err(|e| format!("findmnt stdout not UTF-8: {e}"))
}

/// Read one line from stdin. Errors propagate as descriptive strings
/// so the caller can show them to the operator.
fn read_stdin_line() -> Result<String, String> {
    let mut line = String::new();
    io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| format!("stdin read: {e}"))?;
    Ok(line)
}

// ---- user-facing output -----------------------------------------------------

fn print_header(profile: &Profile, device: Option<&str>) {
    println!("aegis-boot init — {}", profile.description);
    println!();
    println!("Plan:");
    println!("  1. doctor preflight (host + stick health)");
    match device {
        Some(d) => println!("  2. flash {d}"),
        None => println!("  2. flash (auto-detect removable drive)"),
    }
    println!("  3. fetch + add each ISO in the profile:");
    for slug in profile.slugs {
        println!("       - {slug}");
    }
    println!();
}

fn print_success(profile: &Profile) {
    println!();
    println!("=== aegis-boot init: DONE ===");
    println!(
        "Profile '{}' is ready on the stick ({} ISO(s) added).",
        profile.name,
        profile.slugs.len()
    );
    println!();
    println!("Next steps:");
    println!("  1. Eject the stick safely: aegis-boot eject /dev/sdX");
    println!("     (manual fallback: sudo sync && sudo eject /dev/sdX)");
    println!("  2. Boot the target machine (UEFI boot menu → USB entry).");
    println!("  3. In rescue-tui, pick an ISO and press Enter.");
    println!();
    println!("Inspect attestation: aegis-boot list");
}

fn print_profile_list() {
    eprintln!("Available profiles:");
    for p in PROFILES {
        eprintln!("  {:<14} {}", p.name, p.description);
    }
}

fn print_help() {
    println!("aegis-boot init — one-command rescue stick (flash + fetch + add)");
    println!();
    println!("USAGE:");
    println!("  aegis-boot init [/dev/sdX] [OPTIONS]");
    println!();
    println!("OPTIONS:");
    println!("  --profile NAME     Profile to install (default: panic-room)");
    println!("  --yes, -y          Skip interactive confirmations (destructive); also");
    println!("                     skips the wizard's serial-confirmation gate (#245)");
    println!("  --force            Skip the wizard's 'device is currently mounted' gate");
    println!("  --no-doctor        Skip doctor preflight (not recommended)");
    println!("  --no-gpg           Skip GPG verification on fetched ISOs");
    println!("  --list-profiles    Print profile names, one per line (for completion)");
    println!("  --help, -h         This message");
    println!();
    println!("INTERACTIVE MODE (no /dev/sdX, no --yes):");
    println!("  The serial-confirmation wizard guards against wrong-device dd: pick a");
    println!("  USB stick from a numbered list, see its hardware serial, type the");
    println!("  last 4 chars to confirm. (#245) See docs/HOW_IT_WORKS.md for the");
    println!("  full trust narrative.");
    println!();
    println!("PROFILES:");
    for p in PROFILES {
        println!("  {:<14}    {}", p.name, p.description);
    }
    println!();
    println!("EXAMPLES:");
    println!("  aegis-boot init                       # auto-detect drive, panic-room");
    println!("  aegis-boot init /dev/sdc              # explicit device");
    println!("  aegis-boot init /dev/sdc --yes        # unattended");
    println!("  aegis-boot init --profile minimal     # fastest — Alpine only");
    println!("  aegis-boot init --profile server      # Ubuntu Server + Rocky + Alma");
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]
    use super::*;

    #[test]
    fn panic_room_has_three_slugs() {
        assert_eq!(PANIC_ROOM.slugs.len(), 3);
    }

    #[test]
    fn minimal_has_one_slug() {
        assert_eq!(MINIMAL.slugs.len(), 1);
    }

    #[test]
    fn server_has_three_slugs() {
        assert_eq!(SERVER.slugs.len(), 3);
    }

    #[test]
    fn every_profile_slug_is_in_catalog() {
        for profile in PROFILES {
            for slug in profile.slugs {
                assert!(
                    find_entry(slug).is_some(),
                    "profile '{}' references slug '{}' which is not in catalog",
                    profile.name,
                    slug,
                );
            }
        }
    }

    #[test]
    fn panic_room_is_the_default() {
        // Default comes from parse_flags when --profile isn't given;
        // verify the string literal matches the const name.
        assert_eq!(parse_flags(&[]).unwrap().profile_name, PANIC_ROOM.name,);
    }

    #[test]
    fn profiles_registry_contains_all_three() {
        let names: Vec<&str> = PROFILES.iter().map(|p| p.name).collect();
        assert!(names.contains(&"panic-room"));
        assert!(names.contains(&"minimal"));
        assert!(names.contains(&"server"));
    }

    #[test]
    fn profile_names_match_slash_free_kebab() {
        // Names go into argv, help text, and CLI examples. Enforce a
        // simple shape so future profile authors don't introduce
        // shell-escape hazards (spaces, slashes, quotes).
        for p in PROFILES {
            for ch in p.name.chars() {
                assert!(
                    ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-',
                    "profile name '{}' contains non-kebab char '{}'",
                    p.name,
                    ch,
                );
            }
            assert!(!p.name.starts_with('-'));
            assert!(!p.name.ends_with('-'));
        }
    }

    #[test]
    fn profile_names_unique() {
        let mut names: Vec<&str> = PROFILES.iter().map(|p| p.name).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "duplicate profile name(s)");
    }

    #[test]
    fn resolve_profile_hits_and_misses() {
        assert!(resolve_profile("panic-room").is_some());
        assert!(resolve_profile("bogus-profile").is_none());
        assert!(resolve_profile("").is_none());
    }

    #[test]
    fn parse_defaults_to_panic_room() {
        let p = parse_flags(&[]).unwrap();
        assert_eq!(p.profile_name, "panic-room");
        assert_eq!(p.device, None);
        assert!(!p.yes);
        assert!(!p.skip_doctor);
        assert!(!p.skip_gpg);
    }

    #[test]
    fn parse_device_positional() {
        let args = vec!["/dev/sdc".to_string()];
        let p = parse_flags(&args).unwrap();
        assert_eq!(p.device.as_deref(), Some("/dev/sdc"));
    }

    #[test]
    fn parse_yes_flag_both_forms() {
        assert!(parse_flags(&["--yes".to_string()]).unwrap().yes);
        assert!(parse_flags(&["-y".to_string()]).unwrap().yes);
    }

    #[test]
    fn parse_direct_install_flag_from_quickstart() {
        // #374 regression: `quickstart` forwards --direct-install
        // through to init; prior to this fix, init rejected it with
        // 'unknown option'. Assert the flag parses cleanly + defaults
        // to false when absent.
        let p = parse_flags(&["--direct-install".to_string()]).unwrap();
        assert!(p.direct_install);
        let p = parse_flags(&[]).unwrap();
        assert!(!p.direct_install, "default is false (legacy dd)");
        // And mixed with the rest of quickstart's argv doesn't regress.
        let args = [
            "--profile".to_string(),
            "minimal".to_string(),
            "--yes".to_string(),
            "--direct-install".to_string(),
            "/dev/sda".to_string(),
        ];
        let p = parse_flags(&args).unwrap();
        assert!(p.direct_install);
        assert!(p.yes);
        assert_eq!(p.profile_name, "minimal");
        assert_eq!(p.device.as_deref(), Some("/dev/sda"));
    }

    #[test]
    fn parse_profile_flag_both_forms() {
        let a = parse_flags(&["--profile".to_string(), "panic-room".to_string()]).unwrap();
        assert_eq!(a.profile_name, "panic-room");
        let b = parse_flags(&["--profile=panic-room".to_string()]).unwrap();
        assert_eq!(b.profile_name, "panic-room");
    }

    #[test]
    fn parse_skip_flags() {
        let p = parse_flags(&["--no-doctor".to_string(), "--no-gpg".to_string()]).unwrap();
        assert!(p.skip_doctor);
        assert!(p.skip_gpg);
    }

    #[test]
    fn parse_rejects_two_devices() {
        let args = vec!["/dev/sdc".to_string(), "/dev/sdd".to_string()];
        let r = parse_flags(&args);
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("only one device"));
    }

    #[test]
    fn parse_rejects_unknown_flag() {
        let r = parse_flags(&["--bogus".to_string()]);
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("unknown option"));
    }

    #[test]
    fn parse_profile_requires_value() {
        let r = parse_flags(&["--profile".to_string()]);
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("requires"));
    }

    #[test]
    fn parse_full_combo() {
        let args = vec![
            "/dev/sdc".to_string(),
            "--profile".to_string(),
            "panic-room".to_string(),
            "--yes".to_string(),
            "--no-gpg".to_string(),
        ];
        let p = parse_flags(&args).unwrap();
        assert_eq!(p.device.as_deref(), Some("/dev/sdc"));
        assert_eq!(p.profile_name, "panic-room");
        assert!(p.yes);
        assert!(p.skip_gpg);
        assert!(!p.skip_doctor);
    }

    #[test]
    fn cached_iso_path_returns_none_for_missing_file() {
        // Unique slug that won't be in the catalog; exercises the
        // "slug not found → None" fast path. We deliberately avoid
        // mutating XDG_CACHE_HOME in tests here because fetch::tests
        // already touches that env var in parallel and the races
        // produce non-deterministic failures (observed: stashed HOME
        // visibly reverted during a parallel test's execution).
        assert!(cached_iso_path("this-slug-does-not-exist-anywhere").is_none());
    }
}
