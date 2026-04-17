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

use std::path::PathBuf;
use std::process::ExitCode;

use crate::catalog::find_entry;

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

/// Registry of known profiles. Keep in sync with `--help` output below.
pub const PROFILES: &[&Profile] = &[&PANIC_ROOM];

/// Entry point for `aegis-boot init [/dev/sdX] [--profile NAME] [--yes]`.
pub fn run(args: &[String]) -> ExitCode {
    // Help-first so we don't prompt on an accidentally-empty invocation.
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_help();
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

    if !parsed.skip_doctor {
        if let Err(code) = doctor_preflight(parsed.device.as_deref(), parsed.yes) {
            return ExitCode::from(code);
        }
    }

    if let Err(code) = flash_step(parsed.device.as_deref(), parsed.yes) {
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

fn flash_step(device: Option<&str>, assume_yes: bool) -> Result<(), u8> {
    println!();
    println!("--- flash stick ---");
    let mut flash_args: Vec<String> = Vec::new();
    if let Some(d) = device {
        flash_args.push(d.to_string());
    }
    if assume_yes {
        flash_args.push("--yes".to_string());
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
struct Parsed {
    profile_name: String,
    device: Option<String>,
    yes: bool,
    skip_doctor: bool,
    skip_gpg: bool,
}

fn parse_flags(args: &[String]) -> Result<Parsed, String> {
    let mut profile_name: Option<String> = None;
    let mut device: Option<String> = None;
    let mut yes = false;
    let mut skip_doctor = false;
    let mut skip_gpg = false;
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
    })
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
    println!("  1. Eject: sudo sync && sudo eject /dev/sdX");
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
    println!("  --profile NAME    Profile to install (default: panic-room)");
    println!("  --yes, -y         Skip interactive confirmations (destructive)");
    println!("  --no-doctor       Skip doctor preflight (not recommended)");
    println!("  --no-gpg          Skip GPG verification on fetched ISOs");
    println!("  --help, -h        This message");
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
    println!("  aegis-boot init --profile panic-room  # explicit profile");
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
    fn panic_room_slugs_all_in_catalog() {
        for slug in PANIC_ROOM.slugs {
            assert!(
                find_entry(slug).is_some(),
                "PANIC_ROOM profile references slug '{slug}' which is not in catalog",
            );
        }
    }

    #[test]
    fn profiles_registry_contains_panic_room() {
        assert!(PROFILES.iter().any(|p| p.name == "panic-room"));
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
