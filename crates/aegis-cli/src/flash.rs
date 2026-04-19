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
    let mut no_progress = false;
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
            "--no-progress" => no_progress = true,
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
    match flash(&drive, prebuilt_image.as_deref(), no_progress) {
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
            // PR3 of #247: classify the flash failure into a typed
            // UserFacing error and render via the structured template.
            // Keeps the internal flash() -> Result<(), String> surface
            // intact while giving operators the cause/detail/try/see
            // format from the epic's spec.
            let classified = FlashError::classify(&e);
            eprint!("{}", crate::userfacing::render_string(&classified));
            Err(1)
        }
    }
}

fn print_help() {
    println!("aegis-boot flash — write aegis-boot to a USB stick");
    println!();
    println!("USAGE: aegis-boot flash [DEVICE] [--dry-run] [--yes] [--image PATH] [--no-progress]");
    println!("  No DEVICE        = auto-detect removable drives.");
    println!("  /dev/sdX (Linux) or /dev/diskN (macOS) = flash to that drive.");
    println!("  --dry-run        = print the typed Plan of operations and exit");
    println!("                     before any destructive action. Useful for");
    println!("                     pre-flight review or CI dry-runs. (#247)");
    println!("  --yes / -y       = skip the 'type flash to confirm' prompt (DESTRUCTIVE).");
    println!("  --image PATH     = write a pre-built image instead of invoking mkusb.sh.");
    println!("                     Required on macOS (mkusb.sh is Linux-only).");
    println!("  --no-progress    = suppress the indicatif progress bar during dd. Use in");
    println!("                     CI, pipes, or dumb terminals where the \\r-updated bar");
    println!("                     would render as a long noisy line. (#244 PR3)");
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

fn flash(drive: &Drive, prebuilt_image: Option<&Path>, no_progress: bool) -> Result<(), String> {
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

    run_dd(&dd_args(&img_path, &dd_target), img_size, no_progress)?;

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

// ---- dd runner dispatch -----------------------------------------------------

/// Platform-dispatching wrapper over `dd`.
///
/// - Linux + progress enabled: uses [`run_dd_with_progress`] for the
///   indicatif bar.
/// - Linux + `--no-progress`: silent `sudo dd`, matches the pre-#244
///   behaviour.
/// - macOS / other: silent `sudo dd` (dd there doesn't emit
///   status=progress on stderr).
fn run_dd(args: &[String], total_bytes: u64, no_progress: bool) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    if !no_progress {
        return run_dd_with_progress(args, total_bytes);
    }
    #[cfg(not(target_os = "linux"))]
    let _ = total_bytes;
    let _ = no_progress;
    let dd_status = Command::new("sudo")
        .args(args)
        .status()
        .map_err(|e| format!("dd exec failed: {e}"))?;
    if !dd_status.success() {
        return Err(format!("dd exited with {dd_status}"));
    }
    Ok(())
}

// ---- dd progress capture (#244 PR3) -----------------------------------------

/// Parse a single line from `dd status=progress` stderr and return the
/// "bytes copied" count, or `None` if the line doesn't match. GNU dd's
/// format is:
///
/// ```text
/// 12345 bytes (12 kB, 12 KiB) copied, 1.234 s, 10.0 MB/s
/// ```
///
/// We only care about the leading integer. Anything before ` bytes`
/// that parses as `u64` wins; everything else returns `None`. Ignoring
/// the trailing rate/time means one parser works across all dd
/// locales (the rate field uses locale-specific decimal separators on
/// some systems, while the leading integer is always C-locale).
// Not cfg-gated so the unit tests run on every CI job (macOS + Linux).
// Only the runner that invokes it (`run_dd_with_progress`) is
// Linux-only; the parser itself is a pure-string helper. Suppress
// dead-code on non-Linux where no caller wires it up.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_dd_progress_line(line: &str) -> Option<u64> {
    let trimmed = line.trim();
    let (num, _rest) = trimmed.split_once(" bytes")?;
    num.trim().parse().ok()
}

/// Run `sudo dd ...` with a reader thread draining its stderr into an
/// indicatif progress bar. Blocks until dd exits. On success returns
/// `Ok(())`; on non-zero exit returns the same error shape the silent
/// path returns so the top-level `FlashError::classify` still matches.
///
/// Uses a preceding `sudo -v` to ensure credentials are cached — dd's
/// stderr is piped (for progress capture), so a password prompt there
/// would silently block. `sudo -v` inherits stdin/stderr from the
/// operator, prompting once before we take over.
#[cfg(target_os = "linux")]
fn run_dd_with_progress(args: &[String], total_bytes: u64) -> Result<(), String> {
    use indicatif::{ProgressBar, ProgressStyle};
    use std::io::BufRead;
    use std::process::Stdio;

    // Validate / refresh sudo credentials up front, using inherited
    // stdin/stderr so the password prompt (if any) is visible. Without
    // this, the later `sudo dd` with piped stderr could hang on a
    // hidden password prompt.
    let sudo_v = Command::new("sudo")
        .arg("-v")
        .status()
        .map_err(|e| format!("sudo -v exec failed: {e}"))?;
    if !sudo_v.success() {
        return Err("sudo credentials not available; cannot run dd".to_string());
    }

    let mut child = Command::new("sudo")
        .args(args)
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("dd exec failed: {e}"))?;

    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "dd: stderr pipe unexpectedly absent".to_string())?;

    let pb = ProgressBar::new(total_bytes);
    pb.set_style(
        ProgressStyle::with_template(
            "{bar:40.cyan/blue} {bytes:>10}/{total_bytes:<10} {bytes_per_sec:>12}  ETA {eta}",
        )
        .unwrap_or_else(|_| ProgressStyle::default_bar()),
    );

    // dd emits progress records separated by `\r` (carriage return,
    // no newline), with a final newline-terminated summary on exit.
    // Split on `\r` to catch each update; the parser is tolerant of
    // trailing `\n` on the last record.
    let pb_thread = pb.clone();
    let reader_thread = std::thread::spawn(move || {
        let mut reader = std::io::BufReader::new(stderr);
        let mut buf: Vec<u8> = Vec::with_capacity(256);
        loop {
            buf.clear();
            match reader.read_until(b'\r', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let line = String::from_utf8_lossy(&buf);
                    if let Some(bytes) = parse_dd_progress_line(&line) {
                        pb_thread.set_position(bytes);
                    }
                }
            }
        }
    });

    let status = child.wait().map_err(|e| format!("dd wait failed: {e}"))?;
    // Let the stderr reader drain whatever final bytes dd emitted
    // before closing the pipe; bounded by EOF on the piped stream.
    let _ = reader_thread.join();
    pb.finish_and_clear();

    if !status.success() {
        return Err(format!("dd exited with {status}"));
    }
    Ok(())
}

// ---- structured errors for operator rendering (#247 PR3) ---------------------

/// Operator-visible failure modes of `aegis-boot flash`. Classified
/// from the internal `Result<(), String>` at the top-level boundary so
/// operators see the cause/detail/try/see shape from #247 instead of a
/// bare `flash failed: <string>` line.
///
/// Keeping `flash()` on `Result<(), String>` is deliberate for PR3:
/// the alternative (propagating a typed enum through every `.map_err`
/// site) is a meaningfully larger refactor. Classification via string
/// pattern-matching at the boundary demonstrates the `UserFacing`
/// surface without touching the error-plumbing of every helper. The
/// narrow surface is unit-tested (see tests at the bottom of this
/// file), so the patterns don't silently drift.
#[derive(Debug, thiserror::Error)]
pub(crate) enum FlashError {
    /// `mkusb.sh` failed to build the source image (Linux-only path).
    #[error("image build failed: {0}")]
    ImageBuild(String),
    /// `dd` exited non-zero while writing to the device.
    #[error("dd write failed: {0}")]
    DdFailed(String),
    /// The sha256 of the first 64 MB read back from the device did
    /// not match the same region of the source image.
    #[error("readback verification failed: {0}")]
    ReadbackMismatch(String),
    /// The readback dd returned fewer bytes than requested — almost
    /// always a silent short-write on a failing / counterfeit stick.
    #[error("short readback: {0}")]
    ShortReadback(String),
    /// Any other internal failure (stat, sync, attestation write, ...).
    /// Preserved verbatim so operators can grep it; the suggestion is
    /// generic ("re-run with `RUST_LOG=debug`" / "file an issue").
    #[error("{0}")]
    Other(String),
}

impl FlashError {
    /// Classify a flash-failure message string into a typed variant
    /// with a specific operator suggestion. Pure function; unit-tested.
    pub(crate) fn classify(msg: &str) -> Self {
        let lower = msg.to_lowercase();
        if lower.contains("mkusb.sh") || lower.starts_with("image build") {
            Self::ImageBuild(msg.to_string())
        } else if lower.contains("dd exited") || lower.contains("dd exec failed") {
            Self::DdFailed(msg.to_string())
        } else if lower.contains("sha256 mismatch") || lower.contains("readback verification") {
            Self::ReadbackMismatch(msg.to_string())
        } else if lower.contains("readback short") || lower.contains("short readback") {
            Self::ShortReadback(msg.to_string())
        } else {
            Self::Other(msg.to_string())
        }
    }
}

impl crate::userfacing::UserFacing for FlashError {
    fn summary(&self) -> &str {
        match self {
            Self::ImageBuild(_) => "image build failed (mkusb.sh)",
            Self::DdFailed(_) => "write to stick failed (dd)",
            Self::ReadbackMismatch(_) => {
                "readback verification failed — bytes on stick don't match source"
            }
            Self::ShortReadback(_) => "readback short — stick returned fewer bytes than requested",
            Self::Other(_) => "flash failed",
        }
    }
    fn detail(&self) -> &str {
        match self {
            Self::ImageBuild(s)
            | Self::DdFailed(s)
            | Self::ReadbackMismatch(s)
            | Self::ShortReadback(s)
            | Self::Other(s) => s,
        }
    }
    fn suggestion(&self) -> Option<&str> {
        Some(match self {
            Self::ImageBuild(_) => {
                "Check the mkusb.sh prerequisites (mtools, dosfstools, exfatprogs, gdisk); \
                 ensure /boot/vmlinuz-* and /boot/initrd.img-* are world-readable \
                 (sudo chmod 0644 /boot/vmlinuz-* /boot/initrd.img-*); re-run flash."
            }
            Self::DdFailed(_) => {
                "The write to the device failed. Unplug, replug, and retry. If it happens \
                 again on the same offset, the stick is failing — try a different stick."
            }
            Self::ReadbackMismatch(_) | Self::ShortReadback(_) => {
                "The stick accepted dd but doesn't hold what was written — typically a \
                 counterfeit or failing flash chip. Try a different stick or a different \
                 USB port (some hubs drop bytes under load). If a new stick also fails, \
                 run `aegis-boot doctor` and file an issue with the report."
            }
            Self::Other(_) => {
                "Re-run with RUST_LOG=debug for more detail. If the error persists, \
                 `aegis-boot doctor --report` captures the host state for a bug report."
            }
        })
    }
    fn docs_url(&self) -> Option<&str> {
        Some(match self {
            Self::ImageBuild(_) => {
                "https://github.com/williamzujkowski/aegis-boot/blob/main/docs/TROUBLESHOOTING.md#mkusbsh-exited-with-n"
            }
            Self::DdFailed(_) | Self::ReadbackMismatch(_) | Self::ShortReadback(_) => {
                "https://github.com/williamzujkowski/aegis-boot/blob/main/docs/TROUBLESHOOTING.md#dd-exited-with-a-non-zero-status-partway-through"
            }
            Self::Other(_) => {
                "https://github.com/williamzujkowski/aegis-boot/blob/main/docs/TROUBLESHOOTING.md"
            }
        })
    }
    fn code(&self) -> Option<&str> {
        Some(match self {
            Self::ImageBuild(_) => "FLASH_IMAGE_BUILD",
            Self::DdFailed(_) => "FLASH_DD_FAILED",
            Self::ReadbackMismatch(_) => "FLASH_READBACK_MISMATCH",
            Self::ShortReadback(_) => "FLASH_READBACK_SHORT",
            Self::Other(_) => "FLASH_OTHER",
        })
    }
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

    // ---- #247 PR3: FlashError classifier -----------------------------------

    #[test]
    fn classify_mkusb_string_into_image_build_variant() {
        let cases = [
            "mkusb.sh exec failed: No such file or directory",
            "mkusb.sh exited with exit status: 1",
            "image build failed: missing dosfstools",
        ];
        for msg in cases {
            assert!(
                matches!(FlashError::classify(msg), FlashError::ImageBuild(_)),
                "classify({msg:?}) did not return ImageBuild"
            );
        }
    }

    #[test]
    fn classify_dd_strings_into_dd_failed_variant() {
        let cases = [
            "dd exited with exit status: 1",
            "dd exec failed: sudo: permission denied",
        ];
        for msg in cases {
            assert!(
                matches!(FlashError::classify(msg), FlashError::DdFailed(_)),
                "classify({msg:?}) did not return DdFailed"
            );
        }
    }

    #[test]
    fn classify_readback_mismatch() {
        assert!(matches!(
            FlashError::classify(
                "readback verification FAILED — sha256 mismatch: expected a, got b"
            ),
            FlashError::ReadbackMismatch(_)
        ));
    }

    #[test]
    fn classify_short_readback() {
        assert!(matches!(
            FlashError::classify(
                "readback short: device returned 1048576 bytes, expected 67108864"
            ),
            FlashError::ShortReadback(_)
        ));
    }

    #[test]
    fn classify_unknown_falls_back_to_other() {
        let msg = "something weird happened during stat()";
        assert!(matches!(FlashError::classify(msg), FlashError::Other(_)));
    }

    #[test]
    fn flash_error_rendered_output_has_all_sections() {
        use crate::userfacing::render_string;
        let err = FlashError::DdFailed("dd exited with exit status: 1".to_string());
        let s = render_string(&err);
        // Check the full structured shape: error code, summary, what,
        // try, see.
        assert!(s.contains("error[FLASH_DD_FAILED]"), "missing code: {s}");
        assert!(s.contains("write to stick failed"), "missing summary: {s}");
        assert!(s.contains("what happened:"), "missing detail line: {s}");
        assert!(s.contains("try:"), "missing try line: {s}");
        assert!(s.contains("see: https://"), "missing docs URL: {s}");
    }

    // ---- #244 PR3: dd progress-line parser --------------------------------

    #[test]
    fn parse_dd_progress_line_extracts_bytes_copied() {
        // Canonical GNU dd status=progress format.
        let line = "12345 bytes (12 kB, 12 KiB) copied, 1.234 s, 10.0 MB/s";
        assert_eq!(parse_dd_progress_line(line), Some(12345));
    }

    #[test]
    fn parse_dd_progress_line_handles_large_values() {
        // ~30 GB stick; bytes count overflows u32 — must be parsed as u64.
        let line = "32010928128 bytes (32 GB, 30 GiB) copied, 98.234 s, 325 MB/s";
        assert_eq!(parse_dd_progress_line(line), Some(32_010_928_128));
    }

    #[test]
    fn parse_dd_progress_line_tolerates_leading_and_trailing_whitespace() {
        let line = "  \r  2147483648 bytes (2.1 GB) copied, 20 s, 107 MB/s\r  ";
        assert_eq!(parse_dd_progress_line(line), Some(2_147_483_648));
    }

    #[test]
    fn parse_dd_progress_line_rejects_non_progress_output() {
        // dd also emits the final `N+M records in/out` summary lines;
        // those don't start with a byte count + " bytes" pattern, so
        // the parser should return None and leave the progress bar at
        // its last good position.
        for line in [
            "123+0 records in",
            "123+0 records out",
            "",
            "dd: error writing: No space left on device",
            "some random noise",
        ] {
            assert_eq!(
                parse_dd_progress_line(line),
                None,
                "line should not parse: {line:?}"
            );
        }
    }

    #[test]
    fn parse_dd_progress_line_rejects_non_numeric_prefix() {
        let line = "abc bytes copied, 1 s, 1 MB/s";
        assert_eq!(parse_dd_progress_line(line), None);
    }

    // ---- #247 PR3 FlashError shared-suggestion invariant -----------------

    #[test]
    fn flash_error_readback_and_short_share_suggestion_and_docs() {
        // Both variants describe a stick that accepted dd but doesn't
        // hold what was written. The suggestion + docs URL should be
        // the same — a future refactor that makes them diverge needs
        // explicit rationale.
        use crate::userfacing::UserFacing;
        let mismatch = FlashError::ReadbackMismatch("x".into());
        let short = FlashError::ShortReadback("y".into());
        assert_eq!(mismatch.suggestion(), short.suggestion());
        assert_eq!(mismatch.docs_url(), short.docs_url());
    }
}
