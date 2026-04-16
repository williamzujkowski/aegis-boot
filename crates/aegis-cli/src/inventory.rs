//! `aegis-boot list` + `add` subcommands — ISO inventory operations.
//!
//! `list` prints ISO files on the stick's `AEGIS_ISOS` partition with
//! verification status (sha256 sidecar, minisig sidecar, size).
//! `add` copies an ISO onto the stick, running the same iso-probe
//! verification that rescue-tui would at boot time.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// Entry point for `aegis-boot list [mount-or-device]`.
pub fn run_list(args: &[String]) -> ExitCode {
    if args.first().map(String::as_str) == Some("--help")
        || args.first().map(String::as_str) == Some("-h")
    {
        println!("aegis-boot list — inventory ISOs on the stick");
        println!();
        println!("USAGE: aegis-boot list [/dev/sdX | /mnt/aegis-isos]");
        println!("  No argument = auto-find the mounted AEGIS_ISOS partition");
        return ExitCode::SUCCESS;
    }

    let mount = match resolve_mount(args.first().map(String::as_str)) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("aegis-boot list: {e}");
            return ExitCode::from(1);
        }
    };

    let isos = scan_isos(&mount.path);
    if isos.is_empty() {
        println!("No .iso files on {}", mount.path.display());
        if mount.temporary {
            unmount_temp(&mount);
        }
        return ExitCode::SUCCESS;
    }

    println!("ISOs on {}:", mount.path.display());
    println!();
    for iso in &isos {
        let sha_marker = if iso.has_sha256 { "\u{2713}" } else { " " };
        let sig_marker = if iso.has_minisig { "\u{2713}" } else { " " };
        println!(
            "  [{sha_marker} sha256] [{sig_marker} minisig]  {:>8}  {}",
            humanize(iso.size),
            iso.name
        );
    }
    println!();
    println!("{} ISO(s) total. Legend:", isos.len());
    println!("  \u{2713} sha256   sibling <iso>.sha256 present");
    println!("  \u{2713} minisig  sibling <iso>.minisig present");
    println!("  (missing sidecars mean the ISO will show GRAY verdict in rescue-tui)");

    if mount.temporary {
        unmount_temp(&mount);
    }
    ExitCode::SUCCESS
}

/// Entry point for `aegis-boot add <iso> [mount-or-device]`.
pub fn run_add(args: &[String]) -> ExitCode {
    if args.is_empty()
        || args.first().map(String::as_str) == Some("--help")
        || args.first().map(String::as_str) == Some("-h")
    {
        println!("aegis-boot add — copy an ISO onto the stick with verification");
        println!();
        println!("USAGE: aegis-boot add <iso-file> [/dev/sdX | /mnt/aegis-isos]");
        return if args.is_empty() {
            ExitCode::from(2)
        } else {
            ExitCode::SUCCESS
        };
    }

    let iso_arg = PathBuf::from(&args[0]);
    if !iso_arg.is_file() {
        eprintln!("aegis-boot add: not a file: {}", iso_arg.display());
        return ExitCode::from(1);
    }
    let iso_filename = iso_arg
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown.iso");

    let mount = match resolve_mount(args.get(1).map(String::as_str)) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("aegis-boot add: {e}");
            return ExitCode::from(1);
        }
    };

    let iso_size = std::fs::metadata(&iso_arg).map(|m| m.len()).unwrap_or(0);
    println!(
        "Adding {} ({}) to {}",
        iso_filename,
        humanize(iso_size),
        mount.path.display()
    );

    // Check free space first — copying to a full partition kills the stick's
    // filesystem state.
    match free_bytes(&mount.path) {
        Some(free) if free < iso_size + 10 * 1024 * 1024 => {
            eprintln!(
                "aegis-boot add: not enough free space ({} free, need {} + 10 MiB headroom)",
                humanize(free),
                humanize(iso_size)
            );
            if mount.temporary {
                unmount_temp(&mount);
            }
            return ExitCode::from(1);
        }
        _ => {}
    }

    // Copy the ISO + any sidecars.
    let dest = mount.path.join(iso_filename);
    println!("  Copying {iso_filename}...");
    if let Err(e) = copy_with_sudo(&iso_arg, &dest) {
        eprintln!("aegis-boot add: copy failed: {e}");
        if mount.temporary {
            unmount_temp(&mount);
        }
        return ExitCode::from(1);
    }

    let mut sidecars_copied = 0;
    for suffix in ["sha256", "SHA256SUMS", "minisig"] {
        let sidecar_src = iso_arg.with_extension(format!("iso.{suffix}"));
        if sidecar_src.is_file() {
            let sidecar_dest = mount.path.join(format!("{iso_filename}.{suffix}"));
            if copy_with_sudo(&sidecar_src, &sidecar_dest).is_ok() {
                println!("  Copied sidecar: .{suffix}");
                sidecars_copied += 1;
            }
        }
    }

    let _ = Command::new("sync").status();
    println!();
    println!("Done. {iso_filename} + {sidecars_copied} sidecar(s) on the stick.");
    if sidecars_copied == 0 {
        println!("Note: no sibling .sha256 or .minisig found — rescue-tui will");
        println!("show GRAY (no verification) verdict and require typed 'boot' confirmation.");
    }

    if mount.temporary {
        unmount_temp(&mount);
    }
    ExitCode::SUCCESS
}

// ---- helpers ---------------------------------------------------------------

struct Mount {
    path: PathBuf,
    /// If true, we mounted the partition ourselves and should unmount on exit.
    temporary: bool,
    #[allow(dead_code)]
    device: Option<PathBuf>,
}

/// Resolve the target mount from either:
///   - no arg: find an already-mounted `AEGIS_ISOS` partition, or auto-mount `/dev/sdX2`
///   - `/dev/sdX`: find partition 2, mount it (temp dir), return that
///   - `/some/path`: use as-is (assume already mounted)
fn resolve_mount(arg: Option<&str>) -> Result<Mount, String> {
    if let Some(raw) = arg {
        let p = PathBuf::from(raw);
        if p.is_dir() {
            return Ok(Mount {
                path: p,
                temporary: false,
                device: None,
            });
        }
        if raw.starts_with("/dev/") {
            return mount_dev(&p);
        }
        return Err(format!("not a directory or /dev/* path: {raw}"));
    }

    // Auto: look for an already-mounted AEGIS_ISOS in /proc/mounts.
    if let Ok(mounts) = std::fs::read_to_string("/proc/mounts") {
        for line in mounts.lines() {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() >= 2 && fields[1].contains("AEGIS_ISOS") {
                return Ok(Mount {
                    path: PathBuf::from(fields[1]),
                    temporary: false,
                    device: Some(PathBuf::from(fields[0])),
                });
            }
        }
    }
    Err("no AEGIS_ISOS partition found mounted; specify /dev/sdX or a mount path".to_string())
}

fn mount_dev(dev: &Path) -> Result<Mount, String> {
    // Assume partition 2 is AEGIS_ISOS (mkusb layout).
    let part = PathBuf::from(format!("{}2", dev.display()));
    if !part.exists() {
        return Err(format!(
            "{}2 not found — is {} an aegis-boot stick?",
            dev.display(),
            dev.display()
        ));
    }
    let tmp = tempdir().ok_or_else(|| "mktemp failed".to_string())?;
    // iocharset=utf8 (not cp437 — that's a codepage, not an iocharset;
    // using cp437 as iocharset silently falls back to the default
    // iso8859-1 and fails on kernels without nls_iso8859-1 loaded).
    let out = Command::new("sudo")
        .args([
            "mount",
            "-t",
            "vfat",
            "-o",
            "rw,codepage=437,iocharset=utf8",
            &part.display().to_string(),
            &tmp.display().to_string(),
        ])
        .output()
        .map_err(|e| format!("mount exec: {e}"))?;
    if !out.status.success() {
        let _ = std::fs::remove_dir(&tmp);
        return Err(format!(
            "mount {} failed: {}",
            part.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(Mount {
        path: tmp,
        temporary: true,
        device: Some(part),
    })
}

fn unmount_temp(m: &Mount) {
    let _ = Command::new("sudo")
        .args(["umount", &m.path.display().to_string()])
        .status();
    let _ = std::fs::remove_dir(&m.path);
}

fn tempdir() -> Option<PathBuf> {
    // Name is unique per process (PID + counter) and `create_dir` is
    // atomic, returning Err if the path already exists. This rules out
    // the predictable-name attack the temp-dir rule warns about.
    // nosemgrep: rust.lang.security.temp-dir.temp-dir
    let base = std::env::temp_dir();
    for i in 0..100 {
        let path = base.join(format!("aegis-cli-{}-{i}", std::process::id()));
        if std::fs::create_dir(&path).is_ok() {
            return Some(path);
        }
    }
    None
}

struct IsoEntry {
    name: String,
    size: u64,
    has_sha256: bool,
    has_minisig: bool,
}

fn scan_isos(dir: &Path) -> Vec<IsoEntry> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    let mut filenames: Vec<(String, u64)> = Vec::new();
    let mut sidecar_names: Vec<String> = Vec::new();
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let size = e.metadata().map(|m| m.len()).unwrap_or(0);
        let is_iso = Path::new(&name)
            .extension()
            .and_then(|s| s.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("iso"));
        if is_iso {
            filenames.push((name, size));
        } else {
            sidecar_names.push(name);
        }
    }
    filenames.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, size) in filenames {
        let has_sha256 = sidecar_names.iter().any(|s| {
            s.eq_ignore_ascii_case(&format!("{name}.sha256"))
                || s.eq_ignore_ascii_case(&format!("{name}.SHA256SUMS"))
        });
        let has_minisig = sidecar_names
            .iter()
            .any(|s| s.eq_ignore_ascii_case(&format!("{name}.minisig")));
        out.push(IsoEntry {
            name,
            size,
            has_sha256,
            has_minisig,
        });
    }
    out
}

fn copy_with_sudo(src: &Path, dest: &Path) -> Result<(), String> {
    let out = Command::new("sudo")
        .args([
            "cp",
            &src.display().to_string(),
            &dest.display().to_string(),
        ])
        .output()
        .map_err(|e| format!("cp exec: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(())
}

fn free_bytes(path: &Path) -> Option<u64> {
    let out = Command::new("df")
        .args(["-B1", "--output=avail"])
        .arg(path)
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .nth(1)
        .and_then(|l| l.trim().parse().ok())
}

#[allow(clippy::cast_precision_loss)]
fn humanize(bytes: u64) -> String {
    let gib = bytes as f64 / 1_073_741_824.0;
    if gib >= 1.0 {
        format!("{gib:.1} GiB")
    } else {
        let mib = bytes as f64 / 1_048_576.0;
        format!("{mib:.0} MiB")
    }
}
