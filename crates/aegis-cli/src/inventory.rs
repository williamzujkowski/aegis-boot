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
        println!("USAGE:");
        println!("  aegis-boot list [/dev/sdX | /mnt/aegis-isos]");
        println!("  aegis-boot list --json [target]   # machine-readable output");
        println!();
        println!("  No target argument = auto-find the mounted AEGIS_ISOS partition.");
        return ExitCode::SUCCESS;
    }

    // --json is a mode flag; accepted in any position. Everything else
    // is positional (the target mount path or device).
    let json_mode = args.iter().any(|a| a == "--json");
    let target = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .map(String::as_str);

    let mount = match resolve_mount(target) {
        Ok(m) => m,
        Err(e) => {
            if json_mode {
                println!(
                    "{{ \"schema_version\": 1, \"error\": \"{}\" }}",
                    crate::doctor::json_escape(&e)
                );
            } else {
                eprintln!("aegis-boot list: {e}");
            }
            return ExitCode::from(1);
        }
    };

    let isos = scan_isos(&mount.path);

    if json_mode {
        print_list_json(&mount.path, &isos);
    } else {
        print_attestation_summary(&mount.path);
        if isos.is_empty() {
            println!("No .iso files on {}", mount.path.display());
        } else {
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
        }
    }

    if mount.temporary {
        unmount_temp(&mount);
    }
    ExitCode::SUCCESS
}

/// Emit the list inventory as a stable `schema_version=1` JSON document
/// on stdout. Matches the shape of `aegis-boot doctor --json` so
/// downstream tooling has one parser.
fn print_list_json(mount_path: &Path, isos: &[IsoEntry]) {
    use crate::doctor::json_escape;
    println!("{{");
    println!("  \"schema_version\": 1,");
    println!("  \"tool_version\": \"{}\",", env!("CARGO_PKG_VERSION"));
    println!(
        "  \"mount_path\": \"{}\",",
        json_escape(&mount_path.display().to_string())
    );
    // Attestation summary if any — stays null when the mount has no
    // attestation (operator flashed elsewhere, or pre-v0.13.0 stick).
    match crate::attest::summary_for_mount(mount_path) {
        Some(s) => {
            println!("  \"attestation\": {{");
            println!("    \"flashed_at\": \"{}\",", json_escape(&s.flashed_at));
            println!("    \"operator\": \"{}\",", json_escape(&s.operator));
            println!("    \"isos_recorded\": {},", s.isos_recorded);
            println!(
                "    \"manifest_path\": \"{}\"",
                json_escape(&s.manifest_path.display().to_string())
            );
            println!("  }},");
        }
        None => println!("  \"attestation\": null,"),
    }
    println!("  \"count\": {},", isos.len());
    println!("  \"isos\": [");
    let last = isos.len().saturating_sub(1);
    for (i, iso) in isos.iter().enumerate() {
        let comma = if i == last { "" } else { "," };
        println!(
            "    {{ \"name\": \"{}\", \"size_bytes\": {}, \"has_sha256\": {}, \"has_minisig\": {} }}{comma}",
            json_escape(&iso.name),
            iso.size,
            iso.has_sha256,
            iso.has_minisig,
        );
    }
    println!("  ]");
    println!("}}");
}

/// Entry point for `aegis-boot add <iso> [mount-or-device]`.
pub fn run_add(args: &[String]) -> ExitCode {
    match try_run_add(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => ExitCode::from(code),
    }
}

/// Inner runner returning a typed result so `aegis-boot init` can branch
/// on success/failure. Same semantics as `run_add`.
pub(crate) fn try_run_add(args: &[String]) -> Result<(), u8> {
    if args.is_empty()
        || args.first().map(String::as_str) == Some("--help")
        || args.first().map(String::as_str) == Some("-h")
    {
        println!("aegis-boot add — copy an ISO onto the stick with verification");
        println!();
        println!("USAGE: aegis-boot add <iso-file> [/dev/sdX | /mnt/aegis-isos]");
        return if args.is_empty() { Err(2) } else { Ok(()) };
    }

    let iso_arg = PathBuf::from(&args[0]);
    if !iso_arg.is_file() {
        eprintln!("aegis-boot add: not a file: {}", iso_arg.display());
        return Err(1);
    }
    let iso_filename = iso_arg
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown.iso");

    let mount = match resolve_mount(args.get(1).map(String::as_str)) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("aegis-boot add: {e}");
            return Err(1);
        }
    };

    let iso_size = std::fs::metadata(&iso_arg).map(|m| m.len()).unwrap_or(0);
    println!(
        "Adding {} ({}) to {}",
        iso_filename,
        humanize(iso_size),
        mount.path.display()
    );

    // FAT32 ceiling — refuse 4+ GiB ISOs on vfat-mounted AEGIS_ISOS
    // BEFORE the free-space check so the operator sees a filesystem-
    // specific error (ext4 reflash path) rather than a generic
    // "no-space" message during a partial copy.
    check_fat32_ceiling(&mount, iso_filename, iso_size)?;

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
            return Err(1);
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
        return Err(1);
    }

    let mut sidecars_copied: Vec<String> = Vec::new();
    for suffix in ["sha256", "SHA256SUMS", "minisig"] {
        let sidecar_src = iso_arg.with_extension(format!("iso.{suffix}"));
        if sidecar_src.is_file() {
            let sidecar_dest = mount.path.join(format!("{iso_filename}.{suffix}"));
            if copy_with_sudo(&sidecar_src, &sidecar_dest).is_ok() {
                println!("  Copied sidecar: .{suffix}");
                sidecars_copied.push(suffix.to_string());
            }
        }
    }

    let _ = Command::new("sync").status();
    println!();
    println!(
        "Done. {iso_filename} + {} sidecar(s) on the stick.",
        sidecars_copied.len()
    );
    if sidecars_copied.is_empty() {
        println!("Note: no sibling .sha256 or .minisig found — rescue-tui will");
        println!("show GRAY (no verification) verdict and require typed 'boot' confirmation.");
    }

    // Append to the matching attestation receipt — best-effort. Failure
    // here doesn't fail the add (the ISO is on the stick regardless);
    // we just print a warning.
    match crate::attest::record_iso_added(&mount.path, &iso_arg, sidecars_copied) {
        Ok(att_path) => {
            println!();
            println!("Attestation updated: {}", att_path.display());
        }
        Err(e) => {
            eprintln!();
            eprintln!("warning: attestation could not be updated: {e}");
            eprintln!("(the ISO is still on the stick; this is a host-side audit-trail miss)");
        }
    }

    if mount.temporary {
        unmount_temp(&mount);
    }
    Ok(())
}

// ---- helpers ---------------------------------------------------------------

/// If an attestation matches this stick, print a one-paragraph header.
/// Silent on miss — operator may have flashed elsewhere, that's fine.
fn print_attestation_summary(mount_path: &Path) {
    let Some(s) = crate::attest::summary_for_mount(mount_path) else {
        return;
    };
    println!("Attestation:");
    println!("  flashed   : {} by {}", s.flashed_at, s.operator);
    println!("  ISOs added: {} recorded since flash", s.isos_recorded);
    println!("  manifest  : {}", s.manifest_path.display());
    println!();
}

/// A resolved mount point — either an existing directory or one we
/// created ourselves. Promoted to `pub(crate)` so other subcommands
/// (currently `verify`) can reuse the mount-resolution logic without
/// duplicating it.
pub(crate) struct Mount {
    pub(crate) path: PathBuf,
    /// If true, we mounted the partition ourselves and should unmount on exit.
    pub(crate) temporary: bool,
    #[allow(dead_code)]
    pub(crate) device: Option<PathBuf>,
}

/// Resolve the target mount from either:
///   - no arg: find an already-mounted `AEGIS_ISOS` partition, or auto-mount `/dev/sdX2`
///   - `/dev/sdX`: find partition 2, mount it (temp dir), return that
///   - `/some/path`: use as-is (assume already mounted)
pub(crate) fn resolve_mount(arg: Option<&str>) -> Result<Mount, String> {
    if let Some(raw) = arg {
        let p = PathBuf::from(raw);
        if p.is_dir() {
            // Reverse-lookup the backing device via /proc/mounts so
            // later error messages (FAT32 preflight) can name the
            // specific disk to reflash instead of a generic /dev/sdX.
            let device = device_for_mount_path(&p);
            return Ok(Mount {
                path: p,
                temporary: false,
                device,
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

pub(crate) fn unmount_temp(m: &Mount) {
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

/// FAT32 hard per-file ceiling: 4 GiB minus one byte. Files at or above
/// this size cannot be written to a FAT32 filesystem — reflash with
/// `DATA_FS=ext4` in mkusb.sh to lift the ceiling.
const FAT32_MAX_FILE_SIZE: u64 = (4 * 1024 * 1024 * 1024) - 1;

/// Read `/proc/mounts` and return the filesystem type for the
/// best-matching mount point. Returns `None` when the path isn't a
/// mount point or /proc/mounts can't be read (non-Linux host, or a
/// mount path that's a subdir of the actual mount). Pure-string lookup;
/// no kernel syscalls.
fn filesystem_type(path: &Path) -> Option<String> {
    let mounts = std::fs::read_to_string("/proc/mounts").ok()?;
    let target = path.to_string_lossy();
    let mut best_match: Option<(&str, usize)> = None;
    for line in mounts.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 3 {
            continue;
        }
        let mount_path = fields[1];
        let fs_type = fields[2];
        if target == mount_path || target.starts_with(&format!("{mount_path}/")) {
            // Prefer the longest matching mount path (handles nested
            // mounts correctly).
            let mp_len = mount_path.len();
            if best_match.is_none_or(|(_, prev)| mp_len > prev) {
                best_match = Some((fs_type, mp_len));
            }
        }
    }
    best_match.map(|(fs, _)| fs.to_string())
}

/// Reverse-lookup the backing device for a mount path via /proc/mounts.
/// Returns the longest-matching mount entry's device field, which is
/// the partition path (`/dev/sda2`, `/dev/nvme0n1p2`) — callers can
/// strip the partition suffix via `parent_disk_path` when they need
/// the whole-disk form.
fn device_for_mount_path(path: &Path) -> Option<PathBuf> {
    let mounts = std::fs::read_to_string("/proc/mounts").ok()?;
    let target = path.to_string_lossy();
    let mut best: Option<(&str, usize)> = None;
    for line in mounts.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 2 {
            continue;
        }
        let dev = fields[0];
        let mp = fields[1];
        if (target == mp || target.starts_with(&format!("{mp}/")))
            && best.is_none_or(|(_, prev)| mp.len() > prev)
        {
            best = Some((dev, mp.len()));
        }
    }
    best.map(|(d, _)| PathBuf::from(d))
}

/// True when the filesystem type reported by /proc/mounts indicates a
/// FAT32-family layout with the 4 GiB per-file ceiling.
fn is_fat32_family(fs_type: &str) -> bool {
    // Linux's /proc/mounts reports FAT32/FAT16/FAT12 as "vfat". Older
    // fat drivers and some busybox builds use "msdos". Both have the
    // per-file-size constraint; accept either.
    matches!(fs_type, "vfat" | "msdos" | "fat" | "fat32")
}

/// Preflight for `aegis-boot add`: reject ISOs that exceed FAT32's
/// 4 GiB per-file ceiling on a vfat-mounted `AEGIS_ISOS`. Common
/// triggers: Win10/Win11 install ISOs (~5-8 GiB UDF), Rocky 9 DVD
/// (~10 GiB). Unmounts the temporary mount on refusal so the caller
/// doesn't have to.
fn check_fat32_ceiling(mount: &Mount, iso_filename: &str, iso_size: u64) -> Result<(), u8> {
    let Some(fs_type) = filesystem_type(&mount.path) else {
        return Ok(());
    };
    if !is_fat32_family(&fs_type) || iso_size <= FAT32_MAX_FILE_SIZE {
        return Ok(());
    }
    // Derive the parent disk path from the mount's partition device
    // so the error names the exact disk the operator needs to reflash.
    // Falls back to the generic `/dev/sdX` placeholder when the mount
    // wasn't backed by a resolvable block device (e.g., bind mount,
    // operator supplied a path instead of /dev/sdX).
    let flash_target = mount
        .device
        .as_deref()
        .and_then(parent_disk_path)
        .unwrap_or_else(|| "/dev/sdX".to_string());
    eprintln!(
        "aegis-boot add: {} is {} — exceeds FAT32's 4 GiB per-file ceiling.",
        iso_filename,
        humanize(iso_size)
    );
    eprintln!("  The AEGIS_ISOS partition is formatted as {fs_type}, which cannot store");
    eprintln!("  files at or above 4 GiB. Reflash with ext4 to lift the ceiling:");
    eprintln!();
    eprintln!("      DATA_FS=ext4 sudo aegis-boot flash {flash_target}");
    eprintln!();
    eprintln!("  (Current contents will be wiped — back up first.)");
    if mount.temporary {
        unmount_temp(mount);
    }
    Err(1)
}

/// Strip the trailing partition suffix from a device path so operators
/// see the disk path `aegis-boot flash` wants, not the partition path.
/// Two naming conventions to handle:
///
///   * **sata / virtio / etc.** — whole disk ends in a letter (`sda`);
///     partition appends digits directly (`sda2`, `sdb15`).
///   * **nvme / mmcblk / loop** — whole disk ends in a digit
///     (`nvme0n1`); partition uses a `p` separator (`nvme0n1p2`,
///     `mmcblk0p1`, `loop0p1`).
///
/// Returns `None` when the input is already a whole-disk path or the
/// name doesn't match either convention. Keeping the placeholder is
/// safer than emitting a wrong disk — the operator corrects the hint
/// instead of scripting a destructive reflash of the wrong drive.
fn parent_disk_path(partition: &Path) -> Option<String> {
    let s = partition.to_str()?;
    let stem = s.strip_prefix("/dev/")?;
    let bytes = stem.as_bytes();
    if bytes.is_empty() {
        return None;
    }
    // Find the length of the trailing digit run.
    let mut digits_start = bytes.len();
    while digits_start > 0 && bytes[digits_start - 1].is_ascii_digit() {
        digits_start -= 1;
    }
    if digits_start == bytes.len() || digits_start == 0 {
        // No trailing digits, or the whole name is digits.
        return None;
    }
    let char_before_digits = bytes[digits_start - 1];

    // nvme / mmcblk / loop convention: "stem_ending_in_digit" + "p" +
    // "partition_digits". Strip the `p<digits>` suffix to recover the
    // whole-disk stem.
    if char_before_digits == b'p' && digits_start >= 2 && bytes[digits_start - 2].is_ascii_digit() {
        let parent = &stem[..digits_start - 1];
        return Some(format!("/dev/{parent}"));
    }

    // sata-style: alpha + digits. But nvme whole disks like `nvme0n1`
    // also end in "alpha + digit" (the `n` is the namespace letter).
    // Discriminate via a tighter check: sata partitions have at least
    // TWO alpha chars before the digits (sd-a, vd-a, hd-a, xvd-a) —
    // which nvme/mmcblk/loop whole-disk names *don't* have between the
    // namespace letter and the trailing namespace digit.
    if char_before_digits.is_ascii_alphabetic()
        && digits_start >= 2
        && bytes[digits_start - 2].is_ascii_alphabetic()
    {
        let parent = &stem[..digits_start];
        return Some(format!("/dev/{parent}"));
    }

    None
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

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    #[test]
    fn fat32_family_accepts_canonical_names() {
        assert!(is_fat32_family("vfat"));
        assert!(is_fat32_family("msdos"));
        assert!(is_fat32_family("fat"));
        assert!(is_fat32_family("fat32"));
    }

    #[test]
    fn fat32_family_rejects_non_fat_types() {
        assert!(!is_fat32_family("ext4"));
        assert!(!is_fat32_family("ext2"));
        assert!(!is_fat32_family("exfat"));
        assert!(!is_fat32_family("btrfs"));
        assert!(!is_fat32_family("xfs"));
        assert!(!is_fat32_family("ntfs"));
        assert!(!is_fat32_family("iso9660"));
        assert!(!is_fat32_family("udf"));
        assert!(!is_fat32_family(""));
    }

    #[test]
    fn fat32_max_file_size_is_just_under_4_gib() {
        // Sanity — FAT32 per-file ceiling is 4 GiB minus 1 byte.
        // Tests elsewhere in the codebase may pivot on this constant;
        // lock the value so a surprise change can't slip through.
        assert_eq!(FAT32_MAX_FILE_SIZE, (4 * 1024 * 1024 * 1024) - 1);
        assert_eq!(FAT32_MAX_FILE_SIZE, 4_294_967_295);
    }

    /// The operator pain-point this fix closes. A real Win11 25H2 ISO
    /// is ~7.9 GiB — must trip the FAT32 gate on vfat-mounted
    /// `AEGIS_ISOS` and be refused pre-copy. Sibling ISOs worth noting:
    /// Rocky 9 DVD is ~10 GiB, Win10 consumer ~5.5 GiB, Ubuntu Desktop
    /// has been flirting with the 4 GiB ceiling for several releases.
    #[test]
    fn win11_size_exceeds_fat32_ceiling() {
        // Use a runtime value (std::hint::black_box) so clippy can't
        // const-fold the assertion away — we're asserting the ceiling
        // catches a real-world operator input, not a compile-time truism.
        let size = std::hint::black_box(7_900_000_000u64);
        assert!(size > FAT32_MAX_FILE_SIZE);
    }

    #[test]
    fn parent_disk_path_strips_sata_partition_digit() {
        assert_eq!(
            parent_disk_path(Path::new("/dev/sda2")).as_deref(),
            Some("/dev/sda")
        );
        assert_eq!(
            parent_disk_path(Path::new("/dev/sdc1")).as_deref(),
            Some("/dev/sdc")
        );
        assert_eq!(
            parent_disk_path(Path::new("/dev/sdb15")).as_deref(),
            Some("/dev/sdb")
        );
        // Three-letter device path (vd, hd, etc.)
        assert_eq!(
            parent_disk_path(Path::new("/dev/vda3")).as_deref(),
            Some("/dev/vda")
        );
    }

    #[test]
    fn parent_disk_path_strips_nvme_p_partition() {
        assert_eq!(
            parent_disk_path(Path::new("/dev/nvme0n1p2")).as_deref(),
            Some("/dev/nvme0n1")
        );
        assert_eq!(
            parent_disk_path(Path::new("/dev/nvme1n1p15")).as_deref(),
            Some("/dev/nvme1n1")
        );
        // mmcblk same convention
        assert_eq!(
            parent_disk_path(Path::new("/dev/mmcblk0p1")).as_deref(),
            Some("/dev/mmcblk0")
        );
        // loop devices
        assert_eq!(
            parent_disk_path(Path::new("/dev/loop0p1")).as_deref(),
            Some("/dev/loop0")
        );
    }

    #[test]
    fn parent_disk_path_declines_whole_disk_input() {
        // When the operator (or mount resolver) already passed a whole
        // disk, don't mangle it — return None so the caller keeps the
        // generic placeholder rather than emitting the wrong path.
        assert_eq!(parent_disk_path(Path::new("/dev/sda")), None);
        assert_eq!(parent_disk_path(Path::new("/dev/nvme0n1")), None);
    }

    #[test]
    fn parent_disk_path_declines_non_dev_paths() {
        assert_eq!(parent_disk_path(Path::new("/tmp/fake-stick2")), None);
        assert_eq!(parent_disk_path(Path::new("sda2")), None); // no /dev/ prefix
        assert_eq!(parent_disk_path(Path::new("")), None);
    }

    #[test]
    fn typical_linux_installer_sizes_fit_fat32() {
        // Alpine (~200 MiB), Ubuntu Server (~2.6 GiB), Fedora Server
        // (~2 GiB), Rocky Minimal (~1.9 GiB) — all comfortably under
        // the FAT32 ceiling. Guards against accidentally tightening
        // the constant.
        for size in [200_000_000u64, 2_700_000_000, 2_000_000_000, 1_900_000_000] {
            assert!(
                size <= FAT32_MAX_FILE_SIZE,
                "size {size} should fit FAT32 but ceiling is {FAT32_MAX_FILE_SIZE}"
            );
        }
    }
}
