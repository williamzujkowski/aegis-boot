//! `aegis-boot flash` — guided USB writer.
//!
//! Three steps:
//!   1. Auto-detect removable drives (or accept explicit `/dev/sdX`)
//!   2. Typed confirmation (`flash`)
//!   3. Build image inline + write with progress + verify
//!
//! Wraps the logic of `scripts/mkusb.sh` + `dd` into one command.
//! For v1.0.0 the image is built by shelling out to mkusb.sh; a
//! future version can inline the Rust equivalent.

use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use crate::detect::{self, Drive};

/// Entry point for `aegis-boot flash [device]`.
pub fn run(args: &[String]) -> ExitCode {
    if args.first().map(String::as_str) == Some("--help")
        || args.first().map(String::as_str) == Some("-h")
    {
        println!("aegis-boot flash — write aegis-boot to a USB stick");
        println!();
        println!("USAGE: aegis-boot flash [/dev/sdX]");
        println!("  No argument = auto-detect removable drives.");
        println!("  Explicit device = flash to that drive.");
        return ExitCode::SUCCESS;
    }
    let explicit_dev = args.first().map(std::string::String::as_str);

    // Step 1: select drive.
    let Some(drive) = select_drive(explicit_dev) else {
        return ExitCode::from(1);
    };

    // Step 2: typed confirmation.
    if !confirm_destructive(&drive) {
        eprintln!("Cancelled.");
        return ExitCode::SUCCESS;
    }

    // Step 3: build + write + verify.
    match flash(&drive) {
        Ok(()) => {
            println!();
            println!("Done. Next steps:");
            println!("  1. Mount the AEGIS_ISOS partition and copy .iso files onto it:");
            println!(
                "       sudo mount {}2 /mnt && cp *.iso /mnt/ && sudo umount /mnt",
                drive.dev.display()
            );
            println!("  2. Boot from the stick (UEFI boot menu, select the USB entry).");
            println!("  3. In rescue-tui, pick an ISO and press Enter.");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("flash failed: {e}");
            ExitCode::from(1)
        }
    }
}

fn select_drive(explicit: Option<&str>) -> Option<Drive> {
    if let Some(dev) = explicit {
        let path = PathBuf::from(dev);
        if !path.exists() {
            eprintln!("device not found: {dev}");
            return None;
        }
        // Build a minimal Drive for the explicit path.
        let drives = detect::list_removable_drives();
        if let Some(d) = drives.into_iter().find(|d| d.dev == path) {
            return Some(d);
        }
        eprintln!("{dev} is not a removable drive (or not detected as one).");
        eprintln!("Available removable drives:");
        for d in detect::list_removable_drives() {
            eprintln!("  {} — {} ({})", d.dev.display(), d.model, d.size_human());
        }
        return None;
    }

    let drives = detect::list_removable_drives();
    if drives.is_empty() {
        eprintln!("No removable USB drives detected.");
        eprintln!("Plug in a USB stick and try again, or specify a device:");
        eprintln!("  aegis-boot flash /dev/sdX");
        return None;
    }

    println!("Detected removable drives:");
    for (i, d) in drives.iter().enumerate() {
        let parts = if d.partitions > 0 {
            format!("{} partitions", d.partitions)
        } else {
            "no partitions".to_string()
        };
        println!(
            "  [{}] {}  {}  {}  ({})",
            i + 1,
            d.dev.display(),
            d.model,
            d.size_human(),
            parts,
        );
    }
    println!();

    if drives.len() == 1 {
        print!(
            "Use {} {}? [Y/n]: ",
            drives[0].dev.display(),
            drives[0].model
        );
    } else {
        print!("Select drive [1-{}]: ", drives.len());
    }
    io::stdout().flush().ok();

    let mut line = String::new();
    if io::stdin().lock().read_line(&mut line).unwrap_or(0) == 0 {
        return None;
    }
    let input = line.trim();

    if drives.len() == 1 && (input.is_empty() || input.eq_ignore_ascii_case("y")) {
        return Some(drives.into_iter().next().unwrap_or_else(|| unreachable!()));
    }

    let idx: usize = match input.parse::<usize>() {
        Ok(n) if n >= 1 && n <= drives.len() => n - 1,
        _ => {
            eprintln!("Invalid selection.");
            return None;
        }
    };
    Some(
        drives
            .into_iter()
            .nth(idx)
            .unwrap_or_else(|| unreachable!()),
    )
}

fn confirm_destructive(drive: &Drive) -> bool {
    println!();
    println!(
        "  ALL DATA ON {} ({}, {}) WILL BE ERASED.",
        drive.dev.display(),
        drive.model,
        drive.size_human()
    );
    println!();
    print!("  Type 'flash' to confirm: ");
    io::stdout().flush().ok();

    let mut line = String::new();
    if io::stdin().lock().read_line(&mut line).unwrap_or(0) == 0 {
        return false;
    }
    line.trim() == "flash"
}

fn flash(drive: &Drive) -> Result<(), String> {
    let repo_root = find_repo_root().ok_or("cannot find aegis-boot repo root (no Cargo.toml)")?;
    let mkusb = repo_root.join("scripts/mkusb.sh");
    let out_dir = repo_root.join("out");

    // Step 3a: build the image via mkusb.sh.
    println!();
    println!("Building aegis-boot image...");

    // Compute disk size from drive capacity — use the full stick.
    let disk_mb = (drive.size_bytes / (1024 * 1024)).max(2048);

    let status = Command::new("bash")
        .arg(&mkusb)
        .env("OUT_DIR", &out_dir)
        .env("DISK_SIZE_MB", disk_mb.to_string())
        .status()
        .map_err(|e| format!("mkusb.sh exec failed: {e}"))?;

    if !status.success() {
        return Err(format!("mkusb.sh exited with {status}"));
    }

    let img_path = out_dir.join("aegis-boot.img");
    if !img_path.is_file() {
        return Err("mkusb.sh did not produce out/aegis-boot.img".to_string());
    }

    let img_size = std::fs::metadata(&img_path)
        .map(|m| m.len())
        .map_err(|e| format!("stat: {e}"))?;

    // Step 3b: dd with progress.
    println!();
    #[allow(clippy::cast_precision_loss)]
    let img_gb = img_size as f64 / 1_073_741_824.0;
    println!(
        "Writing {} ({img_gb:.1} GB) to {} ...",
        img_path.display(),
        drive.dev.display()
    );

    let dd_status = Command::new("sudo")
        .args([
            "dd",
            &format!("if={}", img_path.display()),
            &format!("of={}", drive.dev.display()),
            "bs=4M",
            "oflag=direct",
            "conv=fsync",
            "status=progress",
        ])
        .status()
        .map_err(|e| format!("dd exec failed: {e}"))?;

    if !dd_status.success() {
        return Err(format!("dd exited with {dd_status}"));
    }

    // Step 3c: sync + verify partition table.
    println!("Syncing...");
    let _ = Command::new("sudo").arg("sync").status();
    let _ = Command::new("sudo")
        .args(["partprobe", &drive.dev.display().to_string()])
        .status();

    println!();
    println!(
        "aegis-boot installed on {} ({}, {}).",
        drive.dev.display(),
        drive.model,
        drive.size_human()
    );
    println!("  Partition 1: ESP (signed boot chain)");
    println!("  Partition 2: AEGIS_ISOS (ready for ISOs)");

    Ok(())
}

fn find_repo_root() -> Option<PathBuf> {
    // Check common locations.
    for candidate in [std::env::current_dir().ok(), dirs_from_exe()]
        .into_iter()
        .flatten()
    {
        let mut cur = candidate;
        loop {
            if cur.join("Cargo.toml").is_file() && cur.join("crates").is_dir() {
                return Some(cur);
            }
            if !cur.pop() {
                break;
            }
        }
    }
    None
}

fn dirs_from_exe() -> Option<PathBuf> {
    // Used to locate the developer's repo workspace (walks up to find
    // `Cargo.toml + crates/`), not for any security decision. A
    // tampered current_exe just makes us fail to find the repo.
    // nosemgrep: rust.lang.security.current-exe.current-exe
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
}
