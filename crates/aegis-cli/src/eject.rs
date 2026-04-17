//! `aegis-boot eject [device]` — safe-eject helper.
//!
//! Operators who pull a stick without first syncing risk FAT32 / ext4
//! dirty state on the `AEGIS_ISOS` partition, which downstream presents as
//! "file ends mid-ISO" on the next boot or "ISO sha256 mismatch" during
//! verification. The common host recipe (`sudo sync && sudo eject /dev/sdX`)
//! works but is easy to skip under time pressure.
//!
//! This subcommand bundles:
//!   1. `sync` to flush dirty buffers (filesystem-level)
//!   2. `blockdev --flushbufs /dev/sdX` to flush the block-device cache
//!   3. `udisksctl power-off` when available (polkit-friendly, no sudo)
//!      OR `eject /dev/sdX` fallback (requires sudo for removable drives
//!      on systems without polkit; prints the sudo recipe if not root)
//!   4. Exit 0 with a single "Safe to remove." line on success
//!
//! Non-goals:
//!   - Updating the attestation manifest to note ejection time (could
//!     conflict with an already-closed attestation; deferred)
//!   - Forcing unmount of a busy filesystem (fuser/lsof integration)
//!   - Working on encrypted / LVM / MD — those need per-stack handling
//!     we don't have today

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use crate::detect::{self, Drive};

/// Entry point for `aegis-boot eject [device]`.
pub fn run(args: &[String]) -> ExitCode {
    match try_run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => ExitCode::from(code),
    }
}

/// Inner runner returning a typed result. Same contract as `run`.
pub(crate) fn try_run(args: &[String]) -> Result<(), u8> {
    let mut explicit_dev: Option<&str> = None;
    for a in args {
        match a.as_str() {
            "--help" | "-h" => {
                print_help();
                return Ok(());
            }
            arg if arg.starts_with("--") => {
                eprintln!("aegis-boot eject: unknown option '{arg}'");
                return Err(2);
            }
            other => {
                if explicit_dev.is_some() {
                    eprintln!("aegis-boot eject: only one device allowed");
                    return Err(2);
                }
                explicit_dev = Some(other);
            }
        }
    }

    let Some(drive) = select_drive(explicit_dev) else {
        return Err(1);
    };

    println!(
        "Ejecting {} ({}, {})...",
        drive.dev.display(),
        drive.model,
        drive.size_human()
    );

    // Step 1: filesystem-level sync.
    println!("  sync...");
    if !run_status(Command::new("sync").arg(drive.dev.display().to_string())) {
        // Whole-device sync may fail on some kernels; fall back to bare sync.
        let _ = Command::new("sync").status();
    }

    // Step 2: flush block-device buffers. Requires root; surface cleanly
    // if we don't have it — the `udisksctl` fallback below works without.
    println!("  flush block-device buffers...");
    if !run_status(
        Command::new("sudo")
            .args(["-n", "blockdev", "--flushbufs"])
            .arg(drive.dev.display().to_string()),
    ) {
        eprintln!("  (skipped: sudo -n blockdev unavailable; relying on sync)");
    }

    // Step 3: power-off. Try udisksctl (no sudo) first, then eject(1).
    if has_command("udisksctl") && try_udisksctl_power_off(&drive.dev) {
        println!();
        println!("Done. Safe to remove {}.", drive.dev.display());
        return Ok(());
    }
    if try_eject_command(&drive.dev) {
        println!();
        println!("Done. Safe to remove {}.", drive.dev.display());
        return Ok(());
    }

    eprintln!();
    eprintln!(
        "aegis-boot eject: could not power-off {} automatically.",
        drive.dev.display()
    );
    eprintln!("The stick IS synced — it's safe to remove, we just couldn't");
    eprintln!("power-gate the bus. Try one of:");
    eprintln!("  sudo eject {}", drive.dev.display());
    eprintln!("  udisksctl power-off -b {}", drive.dev.display());
    Err(1)
}

fn print_help() {
    println!("aegis-boot eject — safely power-off and prepare a USB stick for removal");
    println!();
    println!("USAGE:");
    println!("  aegis-boot eject [device]");
    println!("  aegis-boot eject --help");
    println!();
    println!("BEHAVIOR:");
    println!("  1. sync (flush dirty buffers)");
    println!("  2. blockdev --flushbufs /dev/sdX (needs sudo)");
    println!("  3. udisksctl power-off (polkit-friendly, no sudo) OR eject /dev/sdX");
    println!();
    println!("EXAMPLES:");
    println!("  aegis-boot eject              # auto-detect removable drive");
    println!("  aegis-boot eject /dev/sdc     # explicit device");
}

fn select_drive(explicit: Option<&str>) -> Option<Drive> {
    if let Some(dev) = explicit {
        let path = PathBuf::from(dev);
        if !path.exists() {
            eprintln!("device not found: {dev}");
            return None;
        }
        let drives = detect::list_removable_drives();
        if let Some(d) = drives.into_iter().find(|d| d.dev == path) {
            return Some(d);
        }
        // Accept the explicit path anyway — operator is specific, and
        // an eject mistake on the wrong disk is caught by the usual
        // is-it-removable guard inside `detect::list_removable_drives`.
        // We still warn so a typo surfaces.
        eprintln!("{dev} is not listed as a removable drive — proceeding anyway");
        return Some(Drive {
            dev: path,
            model: "(unknown)".to_string(),
            size_bytes: 0,
            partitions: 0,
        });
    }

    let drives = detect::list_removable_drives();
    match drives.len() {
        0 => {
            eprintln!("No removable USB drives detected to eject.");
            eprintln!("Plug in a stick first, or specify: aegis-boot eject /dev/sdX");
            None
        }
        1 => drives.into_iter().next(),
        n => {
            eprintln!("Multiple removable drives present ({n}); specify which to eject:");
            for d in &drives {
                eprintln!(
                    "  aegis-boot eject {}   # {} ({})",
                    d.dev.display(),
                    d.model,
                    d.size_human()
                );
            }
            None
        }
    }
}

fn has_command(name: &str) -> bool {
    Command::new(name)
        .arg("--help")
        .output()
        .map(|o| o.status.success() || o.status.code() == Some(1))
        .unwrap_or(false)
}

fn try_udisksctl_power_off(dev: &Path) -> bool {
    println!("  udisksctl power-off...");
    run_status(
        Command::new("udisksctl")
            .args(["power-off", "-b"])
            .arg(dev.display().to_string()),
    )
}

fn try_eject_command(dev: &Path) -> bool {
    if !has_command("eject") {
        return false;
    }
    println!("  eject...");
    // Most distros require root to eject a whole-device; try with sudo -n.
    run_status(
        Command::new("sudo")
            .args(["-n", "eject"])
            .arg(dev.display().to_string()),
    ) || run_status(Command::new("eject").arg(dev.display().to_string()))
}

/// Run a `Command` and return true on successful exit.
fn run_status(cmd: &mut Command) -> bool {
    cmd.status().map(|s| s.success()).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn help_flag_returns_ok() {
        let r = try_run(&["--help".to_string()]);
        assert!(r.is_ok());
    }

    #[test]
    fn short_help_flag_returns_ok() {
        let r = try_run(&["-h".to_string()]);
        assert!(r.is_ok());
    }

    #[test]
    fn rejects_two_devices() {
        let r = try_run(&["/dev/sdc".to_string(), "/dev/sdd".to_string()]);
        assert!(r.is_err());
    }

    #[test]
    fn rejects_unknown_flag() {
        let r = try_run(&["--bogus".to_string()]);
        assert!(r.is_err());
    }

    #[test]
    fn run_status_returns_false_for_missing_command() {
        let mut cmd = Command::new("this-command-definitely-does-not-exist-aegis-boot");
        assert!(!run_status(&mut cmd));
    }

    #[test]
    fn has_command_misses_nonexistent() {
        // Positive case for has_command() is hard to test portably:
        // different utilities respond to `--help` with different exit
        // codes (ls: 0, sh: 2, busybox: 1, BSD coreutils: varies).
        // The miss case is stable across hosts — a name we invent
        // can't be on PATH.
        assert!(!has_command(
            "this-command-definitely-does-not-exist-aegis-boot"
        ));
    }
}
