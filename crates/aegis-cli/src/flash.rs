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

use crate::attest;
use crate::detect::{self, Drive};

/// Entry point for `aegis-boot flash [device] [--yes]`.
pub fn run(args: &[String]) -> ExitCode {
    match try_run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => ExitCode::from(code),
    }
}

/// Inner runner that returns a Result so callers (`aegis-boot init`)
/// can branch on success/failure without comparing opaque `ExitCode`s.
/// Shape matches the public `run` surface — same args, same semantics —
/// just with a typed error channel.
pub(crate) fn try_run(args: &[String]) -> Result<(), u8> {
    let mut explicit_dev: Option<&str> = None;
    let mut assume_yes = false;
    let mut prebuilt_image: Option<PathBuf> = None;
    let mut dry_run = false;
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "--help" | "-h" => {
                print_help();
                return Ok(());
            }
            "--yes" | "-y" => assume_yes = true,
            "--dry-run" => dry_run = true,
            "--image" => {
                i += 1;
                let Some(p) = args.get(i) else {
                    eprintln!("aegis-boot flash: --image requires a path argument");
                    return Err(2);
                };
                prebuilt_image = Some(PathBuf::from(p));
            }
            arg if arg.starts_with("--image=") => {
                prebuilt_image = Some(PathBuf::from(&arg["--image=".len()..]));
            }
            arg if arg.starts_with("--") => {
                eprintln!("aegis-boot flash: unknown option '{arg}'");
                return Err(2);
            }
            other => {
                if explicit_dev.is_some() {
                    eprintln!("aegis-boot flash: only one device allowed");
                    return Err(2);
                }
                explicit_dev = Some(other);
            }
        }
        i += 1;
    }

    // Validate --image path up front so we fail before asking for confirmation.
    if let Some(path) = prebuilt_image.as_ref() {
        if !path.is_file() {
            eprintln!(
                "aegis-boot flash: --image path is not a file: {}",
                path.display()
            );
            return Err(1);
        }
    }

    // Step 1: select drive.
    let Some(drive) = select_drive(explicit_dev) else {
        return Err(1);
    };

    // PR2 of #247: --dry-run prints the typed Plan and exits before the
    // destructive steps. Operators get "show me what you'd do before you
    // do it" without burning a USB write cycle. The same Plan shape is
    // intentionally narrated by the non-dry-run path too (next block) so
    // the operator's mental model matches what runs.
    let plan = build_flash_plan(&drive, prebuilt_image.as_deref());
    if dry_run {
        print!("{plan}");
        println!();
        println!("--dry-run: no changes were made. Re-run without --dry-run to execute.");
        return Ok(());
    }

    // Step 2: typed confirmation (skipped under --yes).
    if !assume_yes && !confirm_destructive(&drive) {
        eprintln!("Cancelled.");
        return Ok(());
    }

    // Step 3: build + write + verify.
    match flash(&drive, prebuilt_image.as_deref()) {
        Ok(()) => {
            println!();
            println!("Done. Next steps:");
            println!("  1. Add ISOs to the stick (handles mount, verify, attestation):");
            println!("       aegis-boot add /path/to/distro.iso");
            println!("     (or — for a curated bundle in one command —)");
            println!(
                "       aegis-boot init {} --profile panic-room",
                drive.dev.display()
            );
            println!("  2. Safely power-off the stick before removal:");
            println!("       aegis-boot eject {}", drive.dev.display());
            println!("  3. Boot the target machine from the stick (UEFI boot menu),");
            println!("     pick an ISO in rescue-tui, and press Enter.");
            println!();
            #[cfg(target_os = "linux")]
            println!(
                "Manual fallback: sudo mount {}2 /mnt && cp *.iso /mnt/ && sudo umount /mnt",
                drive.dev.display()
            );
            #[cfg(target_os = "macos")]
            println!(
                "Manual fallback: open Finder (the AEGIS_ISOS volume will mount automatically) \
                 and drag ISOs into it; then `diskutil eject {}`.",
                drive.dev.display()
            );
            Ok(())
        }
        Err(e) => {
            eprintln!("flash failed: {e}");
            Err(1)
        }
    }
}

fn print_help() {
    println!("aegis-boot flash — write aegis-boot to a USB stick");
    println!();
    println!("USAGE: aegis-boot flash [DEVICE] [--dry-run] [--yes] [--image PATH]");
    println!("  No DEVICE        = auto-detect removable drives.");
    println!("  /dev/sdX (Linux) or /dev/diskN (macOS) = flash to that drive.");
    println!("  --dry-run        = print the typed Plan of operations and exit");
    println!("                     before any destructive action. Useful for");
    println!("                     pre-flight review or CI dry-runs. (#247)");
    println!("  --yes / -y       = skip the 'type flash to confirm' prompt (DESTRUCTIVE).");
    println!("  --image PATH     = write a pre-built image instead of invoking mkusb.sh.");
    println!("                     Required on macOS (mkusb.sh is Linux-only).");
}

/// Build the typed `Plan` describing what `aegis-boot flash` would do
/// against this drive. The same plan shape feeds both `--dry-run`
/// output (where it's the only thing that runs) and — eventually,
/// once the per-stage progress UI from #244 PR2 lands — the non-dry-run
/// narration.
///
/// The plan is intentionally a high-level *description* of operations,
/// not a perfect mirror of the imperative steps `flash()` performs. The
/// operator-visible value is "what side effects will this run cause";
/// reading the plan and reading the implementation must agree on the
/// answer to that question.
fn build_flash_plan(drive: &Drive, prebuilt_image: Option<&Path>) -> crate::plan::Plan {
    use crate::plan::{Operation, Plan};

    let mut plan = Plan::new(format!(
        "flash {} ({}, {})",
        drive.dev.display(),
        drive.model,
        drive.size_human()
    ));

    // 1. Source: either a pre-built image we'll dd, or build one inline.
    match prebuilt_image {
        Some(img) => {
            plan.add(Operation::PrecheckRefuseUnless {
                predicate: "image is a regular file".to_string(),
                details: format!("{}", img.display()),
            });
        }
        None => {
            plan.add(Operation::PrecheckRefuseUnless {
                predicate: "scripts/mkusb.sh succeeds".to_string(),
                details: "build a fresh aegis-boot.img inline (signed shim+grub+kernel chain + AEGIS_ISOS data partition)".to_string(),
            });
        }
    }

    // 2. Refuse if the device isn't removable + USB. Already enforced
    //    at drive-selection time, but listed in the plan so dry-run
    //    output documents the safety gate.
    plan.add(Operation::PrecheckRefuseUnless {
        predicate: "device is removable + USB".to_string(),
        details: format!("model={}, partitions={}", drive.model, drive.partitions),
    });

    // 3. The destructive write.
    let source = prebuilt_image.map_or_else(
        || std::path::PathBuf::from("(mkusb.sh output)"),
        std::path::Path::to_path_buf,
    );
    plan.add(Operation::WriteToBlockDevice {
        device: drive.dev.clone(),
        source,
        bytes: drive.size_bytes,
        // No accurate ETA without knowing USB version. Leave None;
        // PR for #244 (progress UI) will compute this from real-time
        // bytes/sec measurements once the flash is actually running.
        estimated_duration_secs: None,
    });

    // 4. Readback verification of the signed-chain payload prefix.
    //    The actual hash isn't known until the image is built; the
    //    plan documents the *intent* to verify, not a literal expected
    //    hash. Once #244 PR2 wires this for real, the executed step
    //    can populate the hash post-write.
    plan.add(Operation::ReadbackVerify {
        device: drive.dev.clone(),
        bytes: crate::readback::DEFAULT_READBACK_BYTES,
        expected_sha256: None,
    });

    // 5. Persist an attestation receipt so `aegis-boot attest list`
    //    has a record of this flash. The receipt path is
    //    sudo-aware and lands under the operator's
    //    $XDG_DATA_HOME/aegis-boot/attestations/.
    plan.add(Operation::WriteAttestation {
        destination: std::path::PathBuf::from(
            "$XDG_DATA_HOME/aegis-boot/attestations/<guid>-<ts>.json",
        ),
    });

    plan
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
    match io::stdin().lock().read_line(&mut line) {
        Ok(0) => {
            // EOF before a newline — operator closed stdin (Ctrl-D) or a
            // pipe writer dropped. Silently cancel is fine; no destructive
            // action has happened yet. Keep the preceding prompt visible
            // so the operator sees where the interaction stopped.
            eprintln!("(no input; cancelled)");
            return None;
        }
        Ok(_) => {}
        Err(e) => {
            // Surface I/O errors explicitly. Previously `unwrap_or(0)`
            // rendered EBADF / EIO indistinguishable from EOF, leaving
            // the operator with no diagnostic. (#138)
            eprintln!("stdin read error: {e}; cannot select drive.");
            return None;
        }
    }
    let input = line.trim();

    if drives.len() == 1 && (input.is_empty() || input.eq_ignore_ascii_case("y")) {
        // drives.len() == 1 was just checked; next() is guaranteed Some.
        // Propagate as a structured error rather than `unreachable!()` so
        // that a future refactor that breaks the invariant (e.g. an
        // early-removed race in the drive list) fails loudly instead of
        // panicking. (#138)
        return drives.into_iter().next().or_else(|| {
            eprintln!(
                "internal: drive list became empty between len-check and consume; \
                 rescan with 'aegis-boot flash' and report if reproducible."
            );
            None
        });
    }

    let idx: usize = match input.parse::<usize>() {
        Ok(n) if n >= 1 && n <= drives.len() => n - 1,
        _ => {
            eprintln!("Invalid selection.");
            return None;
        }
    };
    drives.into_iter().nth(idx).or_else(|| {
        // idx is bounds-checked above; propagate a structured error on
        // the impossible path rather than `unreachable!()`. (#138)
        eprintln!(
            "internal: drive {idx} disappeared between bounds-check and consume; \
             rescan with 'aegis-boot flash' and report if reproducible."
        );
        None
    })
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
    match io::stdin().lock().read_line(&mut line) {
        Ok(0) => {
            // EOF before input — treat as "not confirmed". Destructive
            // action declined silently by convention.
            return false;
        }
        Ok(_) => {}
        Err(e) => {
            // I/O error on stdin: fail safe (no flash). Previously
            // swallowed as "no input" — operator saw nothing. (#138)
            eprintln!("stdin read error during confirmation: {e}; cancelled.");
            return false;
        }
    }
    line.trim() == "flash"
}

fn flash(drive: &Drive, prebuilt_image: Option<&Path>) -> Result<(), String> {
    // Step 3a: get the image. --image skips the build; otherwise we
    // shell out to mkusb.sh (Linux only) to generate a fresh image.
    let (img_path, img_size) = if let Some(path) = prebuilt_image {
        let size = std::fs::metadata(path)
            .map(|m| m.len())
            .map_err(|e| format!("stat: {e}"))?;
        (path.to_path_buf(), size)
    } else {
        build_image_via_mkusb(drive)?
    };

    // Step 3b: macOS requires an explicit unmount of the disk's volumes
    // before dd'ing to the raw device. Linux doesn't; skip it there.
    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("diskutil")
            .args(["unmountDisk", &drive.dev.display().to_string()])
            .status();
    }

    // Step 3c: dd with progress.
    println!();
    #[allow(clippy::cast_precision_loss)]
    let img_gb = img_size as f64 / 1_073_741_824.0;
    println!(
        "Writing {} ({img_gb:.1} GB) to {} ...",
        img_path.display(),
        drive.dev.display()
    );

    // PR2 of #244: precompute the sha256 of the source image's first
    // DEFAULT_READBACK_BYTES bytes so we can verify the device matches
    // post-dd. Done BEFORE dd while we still have local-only file I/O —
    // means readback failures land as a clean error after the write
    // instead of "couldn't even read the source for comparison".
    let expected_prefix_hash = match precompute_image_prefix_hash(&img_path) {
        Ok(h) => Some(h),
        Err(e) => {
            // Soft-fail: dd still runs, we just can't readback-verify.
            // Operator sees the warning so a silent skip doesn't look
            // like a successful verification.
            eprintln!(
                "warning: could not precompute source-image prefix hash for readback verify: {e}"
            );
            eprintln!("(dd will proceed; post-write readback verification SKIPPED)");
            None
        }
    };

    // On macOS, /dev/diskN is buffered; /dev/rdiskN is raw and 10x
    // faster. We rewrite the target here so the operator doesn't need
    // to know the trick.
    #[cfg(target_os = "macos")]
    let dd_target = raw_disk_path(&drive.dev);
    #[cfg(not(target_os = "macos"))]
    let dd_target = drive.dev.clone();

    let dd_status = Command::new("sudo")
        .args(dd_args(&img_path, &dd_target))
        .status()
        .map_err(|e| format!("dd exec failed: {e}"))?;

    if !dd_status.success() {
        return Err(format!("dd exited with {dd_status}"));
    }

    // Step 3d: sync + partition rescan. partprobe is Linux-only.
    println!("Syncing...");
    let _ = Command::new("sudo").arg("sync").status();
    #[cfg(target_os = "linux")]
    let _ = Command::new("sudo")
        .args(["partprobe", &drive.dev.display().to_string()])
        .status();

    // PR2 of #244: post-write readback verification. Reads back the
    // first DEFAULT_READBACK_BYTES bytes of the device and compares
    // against the precomputed source hash. Catches silent USB write
    // failures: cheap sticks sometimes accept a dd happily, return
    // success, and hold zeros in the boot sector — the next boot then
    // fails with a Secure Boot violation that's impossible to diagnose
    // from the rescue UI. Reading back ~64 MB and re-checking sha256
    // catches that BEFORE the operator pulls the stick.
    if let Some(expected) = expected_prefix_hash.as_deref() {
        match readback_verify_device(&dd_target, expected) {
            Ok(()) => {
                println!("✓ readback verified — first 64 MB on stick matches the source image");
            }
            Err(e) => {
                return Err(format!(
                    "readback verification FAILED — the stick's first 64 MB does not \
                     match the source image. This usually means a silent USB write \
                     failure on the stick (often a counterfeit/failing flash chip). \
                     Try a different stick or USB port. ({e})"
                ));
            }
        }
    }

    println!();
    println!(
        "aegis-boot installed on {} ({}, {}).",
        drive.dev.display(),
        drive.model,
        drive.size_human()
    );
    println!("  Partition 1: ESP (signed boot chain)");
    println!("  Partition 2: AEGIS_ISOS (ready for ISOs)");

    // Attestation: record what was flashed. Failure here must NOT abort
    // the flash — the data is on the stick regardless. We just print
    // the failure and proceed.
    println!();
    match attest::record_flash(drive, &img_path, img_size) {
        Ok(att_path) => {
            println!("Attestation receipt: {}", att_path.display());
            println!(
                "  Inspect with: aegis-boot attest show {}",
                att_path.display()
            );
        }
        Err(e) => {
            eprintln!("warning: attestation receipt could not be recorded: {e}");
            eprintln!("(the stick is still valid; this is a host-side audit-trail miss)");
        }
    }

    Ok(())
}

/// Shell out to `scripts/mkusb.sh` (Linux only) to build a fresh image.
/// Returns the image path + size. On non-Linux, returns a typed error
/// pointing the operator at `--image` or at Docker.
fn build_image_via_mkusb(drive: &Drive) -> Result<(PathBuf, u64), String> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = drive;
        Err(format!(
            "building aegis-boot.img requires Linux (uses losetup/sbsign/sgdisk); \
             pass --image /path/to/aegis-boot.img with a pre-built image. \
             Running on {}.",
            crate::detect::platform_display_name()
        ))
    }

    #[cfg(target_os = "linux")]
    {
        let repo_root =
            find_repo_root().ok_or("cannot find aegis-boot repo root (no Cargo.toml)")?;
        let mkusb = repo_root.join("scripts/mkusb.sh");
        let out_dir = repo_root.join("out");

        println!();
        println!("Building aegis-boot image...");

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

        Ok((img_path, img_size))
    }
}

/// Platform-appropriate `dd` argv.
///
/// On Linux: `oflag=direct` + `conv=fsync` + `status=progress`.
/// On macOS: `dd` accepts `bs=4m` (lowercase) and doesn't support
/// `oflag=direct` or `status=progress`; use `bs` + `conv=sync`.
fn dd_args(img_path: &Path, target: &Path) -> Vec<String> {
    let ifv = format!("if={}", img_path.display());
    let ofv = format!("of={}", target.display());
    #[cfg(target_os = "macos")]
    {
        vec![
            "dd".to_string(),
            ifv,
            ofv,
            "bs=4m".to_string(),
            "conv=sync".to_string(),
        ]
    }
    #[cfg(not(target_os = "macos"))]
    {
        vec![
            "dd".to_string(),
            ifv,
            ofv,
            "bs=4M".to_string(),
            "oflag=direct".to_string(),
            "conv=fsync".to_string(),
            "status=progress".to_string(),
        ]
    }
}

/// Convert `/dev/diskN` → `/dev/rdiskN`. macOS buffers writes to the
/// non-raw node; the raw variant is ~10x faster for dd. No-op if the
/// input already starts with `/dev/rdisk` or isn't recognizable.
#[cfg(target_os = "macos")]
fn raw_disk_path(dev: &Path) -> PathBuf {
    let s = dev.to_string_lossy();
    if let Some(rest) = s.strip_prefix("/dev/disk") {
        PathBuf::from(format!("/dev/rdisk{rest}"))
    } else {
        dev.to_path_buf()
    }
}

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
fn dirs_from_exe() -> Option<PathBuf> {
    // Used to locate the developer's repo workspace (walks up to find
    // `Cargo.toml + crates/`), not for any security decision. A
    // tampered current_exe just makes us fail to find the repo.
    // nosemgrep: rust.lang.security.current-exe.current-exe
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
}

// ---- post-write readback verification (#244 PR2) -----------------------------

/// Open the source image and compute the sha256 of its first
/// `DEFAULT_READBACK_BYTES`. Used to seed the post-dd readback
/// comparison. No sudo needed — the image is a regular file the
/// operator owns.
fn precompute_image_prefix_hash(img_path: &Path) -> Result<String, String> {
    use crate::readback::{sha256_of_first_bytes, DEFAULT_READBACK_BYTES};
    let mut f =
        std::fs::File::open(img_path).map_err(|e| format!("open {}: {e}", img_path.display()))?;
    let (hex, consumed) = sha256_of_first_bytes(&mut f, DEFAULT_READBACK_BYTES)
        .map_err(|e| format!("hash source-image prefix: {e}"))?;
    if consumed < DEFAULT_READBACK_BYTES {
        return Err(format!(
            "source image is shorter than the {DEFAULT_READBACK_BYTES}-byte readback window \
             (got {consumed}); image likely truncated"
        ));
    }
    Ok(hex)
}

/// Read back the first `DEFAULT_READBACK_BYTES` bytes of `device` and
/// verify the sha256 matches `expected_hex`. Shells out to `sudo dd`
/// to read since the device is root-owned; the dd output is held in
/// memory (~64 MB) and hashed in-process via `sha256_of_first_bytes`.
fn readback_verify_device(device: &Path, expected_hex: &str) -> Result<(), String> {
    use crate::readback::{sha256_of_first_bytes, DEFAULT_READBACK_BYTES};
    println!(
        "Reading back first {} MB of {} for verification...",
        DEFAULT_READBACK_BYTES / 1024 / 1024,
        device.display()
    );
    let count_mb = DEFAULT_READBACK_BYTES / (1024 * 1024);
    let dd = Command::new("sudo")
        .args([
            "dd",
            &format!("if={}", device.display()),
            "bs=1M",
            &format!("count={count_mb}"),
            "status=none",
        ])
        .output()
        .map_err(|e| format!("readback dd exec: {e}"))?;
    if !dd.status.success() {
        return Err(format!(
            "readback dd exited with {} ({})",
            dd.status,
            String::from_utf8_lossy(&dd.stderr).trim()
        ));
    }
    let mut cursor = std::io::Cursor::new(dd.stdout);
    let (actual_hex, consumed) = sha256_of_first_bytes(&mut cursor, DEFAULT_READBACK_BYTES)
        .map_err(|e| format!("hash device prefix: {e}"))?;
    if consumed < DEFAULT_READBACK_BYTES {
        return Err(format!(
            "readback short: device returned {consumed} bytes, expected {DEFAULT_READBACK_BYTES} \
             (likely a truncated write — the stick may have failed)"
        ));
    }
    if actual_hex != expected_hex {
        return Err(format!(
            "sha256 mismatch: expected {expected_hex}, got {actual_hex}"
        ));
    }
    Ok(())
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
    use crate::plan::Operation;

    fn fake_drive() -> Drive {
        Drive {
            dev: PathBuf::from("/dev/sda"),
            model: "SanDisk Cruzer".to_string(),
            size_bytes: 31_914_983_424, // 29.7 GB
            partitions: 0,
        }
    }

    #[test]
    fn build_flash_plan_with_mkusb_describes_full_pipeline() {
        let drive = fake_drive();
        let plan = build_flash_plan(&drive, None);
        let ops = plan.operations();

        // 5 operations: precheck (mkusb), precheck (removable+usb),
        // write, readback verify, attestation.
        assert_eq!(ops.len(), 5, "got {ops:#?}");
        assert!(matches!(ops[0], Operation::PrecheckRefuseUnless { .. }));
        assert!(matches!(ops[1], Operation::PrecheckRefuseUnless { .. }));
        assert!(matches!(ops[2], Operation::WriteToBlockDevice { .. }));
        assert!(matches!(ops[3], Operation::ReadbackVerify { .. }));
        assert!(matches!(ops[4], Operation::WriteAttestation { .. }));
    }

    #[test]
    fn build_flash_plan_with_image_uses_image_path() {
        let drive = fake_drive();
        let img = PathBuf::from("/tmp/aegis-boot.img");
        let plan = build_flash_plan(&drive, Some(&img));
        let rendered = plan.to_string();
        assert!(
            rendered.contains("aegis-boot.img"),
            "image path missing from plan: {rendered}"
        );
        assert!(
            !rendered.contains("mkusb.sh"),
            "should not mention mkusb when --image is set: {rendered}"
        );
    }

    #[test]
    fn build_flash_plan_writes_full_device_size() {
        let drive = fake_drive();
        let plan = build_flash_plan(&drive, None);
        // The WriteToBlockDevice operation should report the drive's
        // size — guards against a future refactor that drops the
        // size into the wrong place.
        let write_op = plan
            .operations()
            .iter()
            .find_map(|op| match op {
                Operation::WriteToBlockDevice { bytes, .. } => Some(*bytes),
                _ => None,
            })
            .expect("plan should contain WriteToBlockDevice");
        assert_eq!(write_op, drive.size_bytes);
    }

    #[test]
    fn build_flash_plan_readback_uses_default_byte_count() {
        let drive = fake_drive();
        let plan = build_flash_plan(&drive, None);
        let readback_bytes = plan
            .operations()
            .iter()
            .find_map(|op| match op {
                Operation::ReadbackVerify { bytes, .. } => Some(*bytes),
                _ => None,
            })
            .expect("plan should contain ReadbackVerify");
        assert_eq!(readback_bytes, crate::readback::DEFAULT_READBACK_BYTES);
    }

    #[test]
    fn build_flash_plan_intent_mentions_device() {
        let drive = fake_drive();
        let plan = build_flash_plan(&drive, None);
        assert!(
            plan.intent().contains("/dev/sda"),
            "intent should mention device: {}",
            plan.intent()
        );
    }

    #[test]
    fn precompute_hash_succeeds_on_image_at_least_64mib() {
        use std::io::Write;
        // Write a 64 MiB + 1 byte file: the hash should be of the
        // first 64 MiB exactly, not the whole file. Guards against a
        // future refactor where someone "helpfully" hashes the whole
        // image and breaks the readback contract.
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let mut f = std::fs::File::create(tmp.path()).unwrap();
        let chunk = vec![0xAAu8; 1024 * 1024];
        for _ in 0..64 {
            f.write_all(&chunk).unwrap();
        }
        f.write_all(&[0xBBu8]).unwrap();
        f.sync_all().unwrap();

        let hash = precompute_image_prefix_hash(tmp.path()).expect("hash should succeed");
        // sha256 hex is always 64 lowercase chars.
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')));
    }

    #[test]
    fn precompute_hash_rejects_image_shorter_than_window() {
        // Only 1 MB of bytes — less than the 64 MB readback window —
        // should error out (truncated image).
        use std::io::Write;
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let mut f = std::fs::File::create(tmp.path()).unwrap();
        f.write_all(&vec![0xCCu8; 1024 * 1024]).unwrap();
        f.sync_all().unwrap();

        match precompute_image_prefix_hash(tmp.path()) {
            Err(e) => assert!(
                e.contains("truncated") || e.contains("shorter"),
                "expected truncated/shorter error, got: {e}"
            ),
            Ok(_) => panic!("expected error on short image"),
        }
    }
}
