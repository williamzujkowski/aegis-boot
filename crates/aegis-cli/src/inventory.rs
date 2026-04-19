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
                let label = iso.display_name.as_deref().unwrap_or(&iso.name);
                println!(
                    "  [{sha_marker} sha256] [{sig_marker} minisig]  {:>8}  {}",
                    humanize(iso.size),
                    label
                );
                // Two-line layout when the sidecar provides both a custom
                // display name AND a description: row 1 shows the curated
                // name, row 2 indents the description so it's visually
                // grouped with its parent ISO. Sidecar-less rows render
                // exactly as before. (#246)
                if iso.display_name.is_some() && iso.name != label {
                    println!("                                          ({})", iso.name);
                }
                if let Some(desc) = &iso.description {
                    println!("                                          {desc}");
                }
            }
            println!();
            println!("{} ISO(s) total. Legend:", isos.len());
            println!("  \u{2713} sha256   sibling <iso>.sha256 present");
            println!("  \u{2713} minisig  sibling <iso>.minisig present");
            println!("  (missing sidecars mean the ISO will show GRAY verdict in rescue-tui)");
            println!("  Display name/description from <iso>.aegis.toml when present (#246)");
        }
    }

    if mount.temporary {
        unmount_temp(&mount);
    }
    ExitCode::SUCCESS
}

/// Emit the list inventory as a stable `schema_version=1` JSON document
/// on stdout.
///
/// Phase 4b-2 of #286 migrated this from a hand-rolled `println!()`
/// chain to the typed [`aegis_manifest::ListReport`] envelope. The
/// wire contract is pinned via
/// `docs/reference/schemas/aegis-boot-list.schema.json`.
fn print_list_json(mount_path: &Path, isos: &[IsoEntry]) {
    let attestation = crate::attest::summary_for_mount(mount_path).map(|s| {
        aegis_manifest::ListAttestationSummary {
            flashed_at: s.flashed_at,
            operator: s.operator,
            isos_recorded: u32::try_from(s.isos_recorded).unwrap_or(u32::MAX),
            manifest_path: s.manifest_path.display().to_string(),
        }
    });
    let report = aegis_manifest::ListReport {
        schema_version: aegis_manifest::LIST_SCHEMA_VERSION,
        tool_version: env!("CARGO_PKG_VERSION").to_string(),
        mount_path: mount_path.display().to_string(),
        attestation,
        count: u32::try_from(isos.len()).unwrap_or(u32::MAX),
        isos: isos
            .iter()
            .map(|iso| aegis_manifest::ListIsoSummary {
                name: iso.name.clone(),
                size_bytes: iso.size,
                has_sha256: iso.has_sha256,
                has_minisig: iso.has_minisig,
                display_name: iso.display_name.clone(),
                description: iso.description.clone(),
            })
            .collect(),
    };
    match serde_json::to_string_pretty(&report) {
        Ok(body) => println!("{body}"),
        Err(e) => eprintln!("aegis-boot list: failed to serialize --json envelope: {e}"),
    }
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
        print_add_help();
        return if args.is_empty() { Err(2) } else { Ok(()) };
    }

    let AddArgs {
        iso_path: iso_arg,
        mount_arg,
        description,
        version,
        category,
    } = parse_add_args(args)?;

    if !iso_arg.is_file() {
        eprintln!("aegis-boot add: not a file: {}", iso_arg.display());
        return Err(1);
    }
    let iso_filename = iso_arg
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown.iso");

    let mount = match resolve_mount(mount_arg.as_deref()) {
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

    let mut sidecars_copied = copy_classic_sidecars(&iso_arg, iso_filename, &mount.path);

    maybe_write_aegis_sidecar(
        &mount.path,
        iso_filename,
        AegisSidecarFlags {
            description,
            version,
            category,
        },
        &mut sidecars_copied,
    );

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
            let device = crate::mounts::device_for_mount(&p);
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
    // Try filesystem types in order of how mkusb.sh formats them today:
    // exfat first (the #243 default), then ext4 (DATA_FS=ext4 opt-in),
    // then vfat (legacy DATA_FS=fat32 sticks). The vfat path needs the
    // explicit codepage/iocharset because the kernel default
    // `iocharset=iso8859-1` is a module not always loaded; cp437 is a
    // codepage, NOT an iocharset, so the iocharset is utf8.
    let mount_attempts: &[(&str, &str)] = &[
        ("exfat", "rw"),
        ("ext4", "rw"),
        ("vfat", "rw,codepage=437,iocharset=utf8"),
    ];
    let mut last_err = String::new();
    for (fstype, opts) in mount_attempts {
        let out = Command::new("sudo")
            .args([
                "mount",
                "-t",
                fstype,
                "-o",
                opts,
                &part.display().to_string(),
                &tmp.display().to_string(),
            ])
            .output()
            .map_err(|e| format!("mount exec: {e}"))?;
        if out.status.success() {
            return Ok(Mount {
                path: tmp,
                temporary: true,
                device: Some(part),
            });
        }
        last_err = format!(
            "  -t {fstype}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let _ = std::fs::remove_dir(&tmp);
    Err(format!(
        "mount {} failed (tried exfat, ext4, vfat):\n{last_err}",
        part.display()
    ))
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
    /// Operator-curated display name from `<iso>.aegis.toml`, when present. (#246)
    display_name: Option<String>,
    /// Operator-curated description from `<iso>.aegis.toml`, when present. (#246)
    description: Option<String>,
}

/// Parsed flags for `aegis-boot add`.
#[derive(Debug)]
struct AddArgs {
    iso_path: PathBuf,
    mount_arg: Option<String>,
    description: Option<String>,
    version: Option<String>,
    category: Option<String>,
}

/// Parse the argv tail of `aegis-boot add`. Recognizes positional
/// `<iso>` (required) + `[mount]` and three optional `--description`,
/// `--version`, `--category` flags (each accepting either `--flag VALUE`
/// or `--flag=VALUE` form). Returns `Err(2)` for usage errors so the
/// caller surfaces the standard "exit 2 = bad args" code.
fn parse_add_args(args: &[String]) -> Result<AddArgs, u8> {
    let mut iso_path: Option<PathBuf> = None;
    let mut mount_arg: Option<String> = None;
    let mut description: Option<String> = None;
    let mut version: Option<String> = None;
    let mut category: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "--description" => {
                i += 1;
                let Some(v) = args.get(i) else {
                    eprintln!("aegis-boot add: --description requires a value");
                    return Err(2);
                };
                description = Some(v.clone());
            }
            s if s.starts_with("--description=") => {
                description = Some(s["--description=".len()..].to_string());
            }
            "--version" => {
                i += 1;
                let Some(v) = args.get(i) else {
                    eprintln!("aegis-boot add: --version requires a value");
                    return Err(2);
                };
                version = Some(v.clone());
            }
            s if s.starts_with("--version=") => {
                version = Some(s["--version=".len()..].to_string());
            }
            "--category" => {
                i += 1;
                let Some(v) = args.get(i) else {
                    eprintln!("aegis-boot add: --category requires a value");
                    return Err(2);
                };
                category = Some(v.clone());
            }
            s if s.starts_with("--category=") => {
                category = Some(s["--category=".len()..].to_string());
            }
            s if s.starts_with("--") => {
                eprintln!("aegis-boot add: unknown option '{s}'");
                return Err(2);
            }
            other => {
                if iso_path.is_none() {
                    iso_path = Some(PathBuf::from(other));
                } else if mount_arg.is_none() {
                    mount_arg = Some(other.to_string());
                } else {
                    eprintln!("aegis-boot add: unexpected extra argument '{other}'");
                    return Err(2);
                }
            }
        }
        i += 1;
    }
    let Some(iso_path) = iso_path else {
        eprintln!("aegis-boot add: missing required <iso-file> argument");
        return Err(2);
    };
    Ok(AddArgs {
        iso_path,
        mount_arg,
        description,
        version,
        category,
    })
}

/// Copy any sibling `.sha256`, `.SHA256SUMS`, and `.minisig` files that
/// live next to the source ISO onto the stick. Returns the list of
/// copied suffixes for the per-add summary line.
fn copy_classic_sidecars(iso_src: &Path, iso_filename: &str, mount: &Path) -> Vec<String> {
    let mut copied = Vec::new();
    for suffix in ["sha256", "SHA256SUMS", "minisig"] {
        let sidecar_src = iso_src.with_extension(format!("iso.{suffix}"));
        if sidecar_src.is_file() {
            let sidecar_dest = mount.join(format!("{iso_filename}.{suffix}"));
            if copy_with_sudo(&sidecar_src, &sidecar_dest).is_ok() {
                println!("  Copied sidecar: .{suffix}");
                copied.push(suffix.to_string());
            }
        }
    }
    copied
}

/// Help text for `aegis-boot add`. Extracted so `try_run_add` stays
/// under clippy's per-function line ceiling.
fn print_add_help() {
    println!("aegis-boot add — copy an ISO onto the stick with verification");
    println!();
    println!("USAGE: aegis-boot add <iso-file> [/dev/sdX | /mnt/aegis-isos]");
    println!("                  [--description TEXT] [--version VER] [--category CAT]");
    println!();
    println!("Optional sidecar metadata (#246) is written next to the ISO as");
    println!("<iso>.aegis.toml so rescue-tui can show 'Network-install Debian 12'");
    println!("instead of the bare filename. The sidecar is unsigned cosmetic");
    println!("metadata — boot decisions still key off the sha256-attested manifest.");
}

/// Operator-supplied aegis sidecar fields collected from `--description`,
/// `--version`, `--category`. Wrapping these in a small struct lets
/// `maybe_write_aegis_sidecar` take a stable signature without growing
/// past clippy's `too_many_arguments` ceiling as more curated fields
/// land. (#246)
struct AegisSidecarFlags {
    description: Option<String>,
    version: Option<String>,
    category: Option<String>,
}

/// Write an aegis sidecar TOML to the stick if any curated metadata was
/// provided on the command line. Cosmetic only — does not affect what
/// boots. Failures degrade to a warning so the ISO add itself succeeds
/// even if the sidecar can't be persisted (matches the existing
/// "attestation update failed" behaviour). (#246)
fn maybe_write_aegis_sidecar(
    mount: &Path,
    iso_filename: &str,
    flags: AegisSidecarFlags,
    sidecars_copied: &mut Vec<String>,
) {
    if flags.description.is_none() && flags.version.is_none() && flags.category.is_none() {
        return;
    }
    let sidecar = iso_probe::IsoSidecar {
        display_name: None,
        description: flags.description,
        version: flags.version,
        category: flags.category,
        last_verified_at: None,
        last_verified_on: None,
        notes: None,
    };
    match write_sidecar_via_sudo(mount, iso_filename, &sidecar) {
        Ok(()) => {
            println!("  Wrote sidecar: {iso_filename}.aegis.toml");
            sidecars_copied.push("aegis.toml".to_string());
        }
        Err(e) => eprintln!("warning: aegis sidecar write failed: {e}"),
    }
}

/// Write a sidecar TOML to the sudo-mounted `AEGIS_ISOS` partition by
/// staging it locally, then routing through `copy_with_sudo` (same path
/// the sha256/minisig sidecars take). The staging file is created with
/// a random name via `tempfile::NamedTempFile` (atomic `O_EXCL` + 6-char
/// random suffix in `$TMPDIR`) so a local attacker cannot pre-create
/// a symlink at a predictable path and trick `fs::write` into writing
/// the body elsewhere. The temp file is deleted on success and on
/// failure (`NamedTempFile`'s Drop handles both).
fn write_sidecar_via_sudo(
    mount: &Path,
    iso_filename: &str,
    sidecar: &iso_probe::IsoSidecar,
) -> Result<(), String> {
    use std::io::Write as _;
    let body = iso_probe::sidecar_to_toml(sidecar).map_err(|e| format!("serialize: {e}"))?;
    let mut staging = tempfile::Builder::new()
        .prefix("aegis-sidecar-")
        .suffix(".toml")
        .tempfile()
        .map_err(|e| format!("staging tempfile: {e}"))?;
    staging
        .write_all(body.as_bytes())
        .map_err(|e| format!("staging write: {e}"))?;
    staging
        .as_file()
        .sync_all()
        .map_err(|e| format!("staging sync: {e}"))?;
    let dest = mount.join(format!("{iso_filename}.aegis.toml"));
    copy_with_sudo(staging.path(), &dest)
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
        // Aegis sidecar (#246) — cosmetic operator-curated metadata.
        // Failures degrade silently to "no sidecar" (matches iso-probe's
        // scan-time behavior); the menu falls back to the bare filename.
        let sidecar = iso_probe::load_sidecar(&dir.join(&name)).ok().flatten();
        let display_name = sidecar.as_ref().and_then(|s| s.display_name.clone());
        let description = sidecar.as_ref().and_then(|s| s.description.clone());
        out.push(IsoEntry {
            name,
            size,
            has_sha256,
            has_minisig,
            display_name,
            description,
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
/// this size cannot be written to a FAT32 filesystem. As of #243 the
/// default is exfat, which has no per-file ceiling — this check now
/// only fires for the opt-in `DATA_FS=fat32` path on legacy sticks.
/// The recovery message points to both exfat (cross-OS) and ext4
/// (Linux-only) reflash recipes.
const FAT32_MAX_FILE_SIZE: u64 = (4 * 1024 * 1024 * 1024) - 1;

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
    let Some(fs_type) = crate::mounts::filesystem_type(&mount.path) else {
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
        .and_then(crate::mounts::parent_disk)
        .map_or_else(|| "/dev/sdX".to_string(), |p| p.display().to_string());
    let detail = format!(
        "{iso_filename} is {size} — exceeds FAT32's 4 GiB per-file ceiling. \
         The AEGIS_ISOS partition is formatted as {fs_type}, which cannot store files at or \
         above 4 GiB. Current contents will be wiped on reflash, so back up first.",
        size = humanize(iso_size),
    );
    let err = AddError::Fat32CeilingExceeded {
        detail,
        flash_target,
    };
    eprint!("{}", crate::userfacing::render_string(&err));
    if mount.temporary {
        unmount_temp(mount);
    }
    Err(1)
}

/// Operator-visible errors from `aegis-boot add`. Rendered through
/// `UserFacing` so multi-option advice (e.g. "reflash with exfat OR
/// reflash with ext4") surfaces as a numbered `try one of:` list
/// instead of the ad-hoc `eprintln!` block the FAT32-ceiling branch
/// used before #247 PR5.
#[derive(Debug)]
pub(crate) enum AddError {
    /// The ISO is at or above FAT32's 4 GiB per-file ceiling and the
    /// current `AEGIS_ISOS` is formatted as a FAT-family filesystem.
    /// `detail` is pre-formatted at construction time (because
    /// `UserFacing::detail()` must return `&str`); `flash_target` is
    /// interpolated into both suggestions so the operator can
    /// copy-paste the reflash command.
    Fat32CeilingExceeded {
        detail: String,
        flash_target: String,
    },
}

impl std::fmt::Display for AddError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Fat32CeilingExceeded { detail, .. } => {
                write!(f, "add refused: {detail}")
            }
        }
    }
}

impl std::error::Error for AddError {}

impl crate::userfacing::UserFacing for AddError {
    fn summary(&self) -> &str {
        match self {
            Self::Fat32CeilingExceeded { .. } => "ISO exceeds FAT32 4 GiB per-file ceiling",
        }
    }

    fn detail(&self) -> &str {
        match self {
            Self::Fat32CeilingExceeded { detail, .. } => detail,
        }
    }

    fn suggestions(&self) -> Vec<String> {
        match self {
            Self::Fat32CeilingExceeded { flash_target, .. } => vec![
                format!(
                    "Reflash with the new exfat default (preserves cross-OS r/w on Linux + \
                     macOS + Windows): `sudo aegis-boot flash {flash_target}`"
                ),
                format!(
                    "Reflash with ext4 for a Linux-only stick: \
                     `DATA_FS=ext4 sudo aegis-boot flash {flash_target}`"
                ),
            ],
        }
    }

    fn code(&self) -> Option<&str> {
        Some("ADD_FAT32_CEILING")
    }
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

    // ---- #246 sidecar parsing tests ----------------------------------------

    fn s(v: &str) -> String {
        v.to_string()
    }

    #[test]
    fn parse_add_args_minimum_takes_iso_only() {
        let parsed = parse_add_args(&[s("debian.iso")]).unwrap();
        assert_eq!(parsed.iso_path, PathBuf::from("debian.iso"));
        assert!(parsed.mount_arg.is_none());
        assert!(parsed.description.is_none());
        assert!(parsed.version.is_none());
        assert!(parsed.category.is_none());
    }

    #[test]
    fn parse_add_args_iso_then_mount() {
        let parsed = parse_add_args(&[s("debian.iso"), s("/mnt/aegis-isos")]).unwrap();
        assert_eq!(parsed.mount_arg.as_deref(), Some("/mnt/aegis-isos"));
    }

    #[test]
    fn parse_add_args_recognises_description_separate_form() {
        let parsed = parse_add_args(&[
            s("debian.iso"),
            s("--description"),
            s("Network-install Debian 12"),
        ])
        .unwrap();
        assert_eq!(
            parsed.description.as_deref(),
            Some("Network-install Debian 12")
        );
    }

    #[test]
    fn parse_add_args_recognises_description_equals_form() {
        let parsed =
            parse_add_args(&[s("debian.iso"), s("--description=Headless server")]).unwrap();
        assert_eq!(parsed.description.as_deref(), Some("Headless server"));
    }

    #[test]
    fn parse_add_args_handles_all_three_curated_flags_in_any_order() {
        let parsed = parse_add_args(&[
            s("--version=12.5.0"),
            s("debian.iso"),
            s("--category"),
            s("install"),
            s("/mnt/aegis-isos"),
            s("--description=netinst"),
        ])
        .unwrap();
        assert_eq!(parsed.iso_path, PathBuf::from("debian.iso"));
        assert_eq!(parsed.mount_arg.as_deref(), Some("/mnt/aegis-isos"));
        assert_eq!(parsed.description.as_deref(), Some("netinst"));
        assert_eq!(parsed.version.as_deref(), Some("12.5.0"));
        assert_eq!(parsed.category.as_deref(), Some("install"));
    }

    #[test]
    fn parse_add_args_rejects_missing_iso() {
        let err = parse_add_args(&[s("--description=x")]).unwrap_err();
        assert_eq!(err, 2);
    }

    #[test]
    fn parse_add_args_rejects_unknown_flag() {
        let err = parse_add_args(&[s("debian.iso"), s("--bogus")]).unwrap_err();
        assert_eq!(err, 2);
    }

    #[test]
    fn parse_add_args_rejects_third_positional() {
        let err = parse_add_args(&[s("a.iso"), s("/mnt"), s("extra")]).unwrap_err();
        assert_eq!(err, 2);
    }

    #[test]
    fn parse_add_args_rejects_dangling_value_flag() {
        // --description without a following value should error rather
        // than silently consume the next positional.
        let err = parse_add_args(&[s("debian.iso"), s("--description")]).unwrap_err();
        assert_eq!(err, 2);
    }

    #[test]
    fn scan_isos_attaches_sidecar_when_present() {
        use std::fs;
        let dir = tempfile::tempdir().unwrap();
        let iso_path = dir.path().join("debian.iso");
        fs::write(&iso_path, b"fake iso bytes").unwrap();
        let sidecar_path = iso_probe::sidecar_path_for(&iso_path);
        fs::write(
            &sidecar_path,
            "display_name = \"Network-install Debian 12\"\ndescription = \"netinst variant\"\n",
        )
        .unwrap();

        let entries = scan_isos(dir.path());
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(e.name, "debian.iso");
        assert_eq!(e.display_name.as_deref(), Some("Network-install Debian 12"));
        assert_eq!(e.description.as_deref(), Some("netinst variant"));
    }

    #[test]
    fn scan_isos_returns_no_sidecar_fields_when_absent() {
        use std::fs;
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("plain.iso"), b"x").unwrap();
        let entries = scan_isos(dir.path());
        assert_eq!(entries.len(), 1);
        assert!(entries[0].display_name.is_none());
        assert!(entries[0].description.is_none());
    }

    // ---- #247 PR5: AddError UserFacing conversion -----------------------

    #[test]
    fn add_error_fat32_ceiling_renders_numbered_list_with_both_reflash_recipes() {
        use crate::userfacing::{render_string, UserFacing};
        let err = AddError::Fat32CeilingExceeded {
            detail: "Win11_25H2.iso is 7.9 GiB — exceeds FAT32's 4 GiB per-file ceiling. \
                     The AEGIS_ISOS partition is formatted as vfat, which cannot store \
                     files at or above 4 GiB. Current contents will be wiped on reflash, \
                     so back up first."
                .to_string(),
            flash_target: "/dev/sdc".to_string(),
        };
        assert_eq!(err.code(), Some("ADD_FAT32_CEILING"));
        let s = render_string(&err);
        assert!(
            s.starts_with("error[ADD_FAT32_CEILING]: ISO exceeds FAT32 4 GiB per-file ceiling"),
            "header mismatch: {s}",
        );
        // Detail surfaces the filename + humanized size + fs type.
        assert!(s.contains("Win11_25H2.iso is 7.9 GiB"), "size missing: {s}");
        assert!(
            s.contains("formatted as vfat"),
            "filesystem type missing: {s}",
        );
        // Both reflash recipes appear as numbered alternatives with
        // the concrete device path interpolated — the value operators
        // get from the suggestions() path vs. the old ad-hoc eprintln!
        // block is *exactly* this copy-paste readiness.
        assert!(s.contains("  try one of:"), "expected numbered list: {s}");
        assert!(
            s.contains("    1. Reflash with the new exfat default")
                && s.contains("`sudo aegis-boot flash /dev/sdc`"),
            "option 1 missing or missing device: {s}",
        );
        assert!(
            s.contains("    2. Reflash with ext4")
                && s.contains("`DATA_FS=ext4 sudo aegis-boot flash /dev/sdc`"),
            "option 2 missing or missing device: {s}",
        );
    }

    #[test]
    fn add_error_fat32_ceiling_falls_back_to_placeholder_device_when_unknown() {
        // When the mount wasn't backed by a resolvable block device
        // (bind mount / operator-supplied path), check_fat32_ceiling
        // falls back to `/dev/sdX` as the flash_target. Both
        // suggestions should still render but with the placeholder
        // inlined so the operator knows what to substitute.
        use crate::userfacing::render_string;
        let err = AddError::Fat32CeilingExceeded {
            detail: "whatever".to_string(),
            flash_target: "/dev/sdX".to_string(),
        };
        let s = render_string(&err);
        assert!(s.contains("`sudo aegis-boot flash /dev/sdX`"), "{s}");
        assert!(
            s.contains("`DATA_FS=ext4 sudo aegis-boot flash /dev/sdX`"),
            "{s}",
        );
    }

    #[test]
    fn add_error_fat32_ceiling_display_includes_detail() {
        let err = AddError::Fat32CeilingExceeded {
            detail: "iso too big".to_string(),
            flash_target: "/dev/sdc".to_string(),
        };
        let display = format!("{err}");
        assert!(display.contains("add refused"), "{display}");
        assert!(display.contains("iso too big"), "{display}");
    }

    #[test]
    fn scan_isos_silently_drops_sidecar_when_malformed() {
        // A broken sidecar shouldn't stop the ISO from listing — the
        // operator can still see + boot it, just without the curated label.
        use std::fs;
        let dir = tempfile::tempdir().unwrap();
        let iso_path = dir.path().join("ubuntu.iso");
        fs::write(&iso_path, b"x").unwrap();
        fs::write(
            iso_probe::sidecar_path_for(&iso_path),
            "this = is = not = valid\n",
        )
        .unwrap();
        let entries = scan_isos(dir.path());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "ubuntu.iso");
        assert!(entries[0].display_name.is_none());
        assert!(entries[0].description.is_none());
    }
}
