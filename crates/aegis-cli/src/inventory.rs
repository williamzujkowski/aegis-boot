// SPDX-License-Identifier: MIT OR Apache-2.0

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
                let envelope = aegis_wire_formats::CliError {
                    schema_version: aegis_wire_formats::CLI_ERROR_SCHEMA_VERSION,
                    error: e.clone(),
                };
                match serde_json::to_string_pretty(&envelope) {
                    Ok(body) => println!("{body}"),
                    Err(err) => eprintln!("aegis-boot list: serialize error envelope: {err}"),
                }
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
                // Human text output shows "folder/name" when the ISO
                // sits in a subdirectory so operators can see the
                // layout at a glance (#274 Phase 6a). JSON stays on
                // two fields (`name` + `folder`) for machine consumers.
                let display_path = match &iso.folder {
                    Some(f) => format!("{f}/{label}"),
                    None => label.to_string(),
                };
                println!(
                    "  [{sha_marker} sha256] [{sig_marker} minisig]  {:>8}  {}",
                    humanize(iso.size),
                    display_path
                );
                // Two-line layout when the sidecar provides both a custom
                // display name AND a description: row 1 shows the curated
                // name, row 2 indents the description so it's visually
                // grouped with its parent ISO. Sidecar-less rows render
                // exactly as before. (#246)
                if iso.display_name.is_some() && iso.name != label {
                    let orig_path = match &iso.folder {
                        Some(f) => format!("{f}/{}", iso.name),
                        None => iso.name.clone(),
                    };
                    println!("                                          ({orig_path})");
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
/// chain to the typed [`aegis_wire_formats::ListReport`] envelope. The
/// wire contract is pinned via
/// `docs/reference/schemas/aegis-boot-list.schema.json`.
fn print_list_json(mount_path: &Path, isos: &[IsoEntry]) {
    let attestation = crate::attest::summary_for_mount(mount_path).map(|s| {
        aegis_wire_formats::ListAttestationSummary {
            flashed_at: s.flashed_at,
            operator: s.operator,
            isos_recorded: u32::try_from(s.isos_recorded).unwrap_or(u32::MAX),
            manifest_path: s.manifest_path.display().to_string(),
        }
    });
    let report = aegis_wire_formats::ListReport {
        schema_version: aegis_wire_formats::LIST_SCHEMA_VERSION,
        tool_version: env!("CARGO_PKG_VERSION").to_string(),
        mount_path: mount_path.display().to_string(),
        attestation,
        count: u32::try_from(isos.len()).unwrap_or(u32::MAX),
        isos: isos
            .iter()
            .map(|iso| aegis_wire_formats::ListIsoSummary {
                name: iso.name.clone(),
                folder: iso.folder.clone(),
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
        iso_path: raw_iso_arg,
        mount_arg,
        description,
        version,
        category,
        folder,
        scan,
    } = parse_add_args(args)?;

    if scan {
        return run_scan_mode(mount_arg.as_deref());
    }

    // #352 UX-4: if the positional arg is a catalog slug (not a real
    // file path), fetch it first and substitute the cached ISO path.
    // Collapses today's `fetch <slug> && add <resolved-path>` 2-step
    // into one `add <slug>` action. Only triggers when:
    //   - the arg is NOT a file on disk AND
    //   - the arg IS a known catalog slug
    // so existing flows (local path, typo'd slug) behave unchanged.
    let Some(iso_arg) = resolve_iso_arg(&raw_iso_arg)? else {
        eprintln!("aegis-boot add: not a file: {}", raw_iso_arg.display());
        return Err(1);
    };
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

    let dest_dir = match resolve_add_dest_dir(&mount.path, folder.as_deref()) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("aegis-boot add: {e}");
            if mount.temporary {
                unmount_temp(&mount);
            }
            return Err(1);
        }
    };

    // Copy the ISO + any sidecars.
    let dest = dest_dir.join(iso_filename);
    println!("  Copying {iso_filename}...");
    if let Err(e) = copy_with_sudo(&iso_arg, &dest) {
        eprintln!("aegis-boot add: copy failed: {e}");
        if mount.temporary {
            unmount_temp(&mount);
        }
        return Err(1);
    }

    let mut sidecars_copied = copy_classic_sidecars(&iso_arg, iso_filename, &dest_dir);

    maybe_write_aegis_sidecar(
        &dest_dir,
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

    record_add_attestation(&mount.path, &iso_arg, sidecars_copied);

    if mount.temporary {
        unmount_temp(&mount);
    }
    Ok(())
}

/// `aegis-boot add --scan <mount>` — walk `AEGIS_ISOS`, hash each
/// bare ISO, and write `<iso>.sha256` sidecars so the rescue-tui
/// upgrades them from tier 2 (BareUnverified) to tier 1
/// (OperatorAttested). (#479)
///
/// Does NOT overwrite existing sha256 sidecars — a mismatch between
/// an existing sidecar and the computed hash is surfaced as a
/// tamper signal rather than a write. Does NOT generate minisig
/// sidecars (would require the operator's private key).
fn run_scan_mode(mount_arg: Option<&str>) -> Result<(), u8> {
    let mount = match resolve_mount(mount_arg) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("aegis-boot add --scan: {e}");
            return Err(1);
        }
    };
    println!(
        "Scanning {} for ISOs without sidecars...",
        mount.path.display()
    );

    let summary = scan_for_upgrades(&mount);

    let ScanSummary {
        total,
        upgraded,
        already_verified,
        tamper_flagged,
        minisig_missing,
        io_errors,
    } = &summary;

    for entry in upgraded {
        println!(
            "  [✓] {}  ({}) — sha256 written",
            entry.rel_path,
            humanize(entry.size)
        );
    }
    for entry in already_verified {
        println!("  [-] {} — already verified; skipped", entry.rel_path);
    }
    for entry in tamper_flagged {
        println!(
            "  [!] {} — existing .sha256 MISMATCH (expected {}, actual {}); NOT overwritten",
            entry.rel_path,
            short_hex(&entry.expected),
            short_hex(&entry.actual),
        );
    }
    for entry in minisig_missing {
        println!(
            "  [~] {} — no .minisig (tier-1 requires operator's signing key; stays at tier-2+)",
            entry.rel_path
        );
    }
    for (rel_path, reason) in io_errors {
        eprintln!("  [x] {rel_path} — I/O error: {reason}");
    }

    println!();
    println!(
        "Done: {upgraded_n} upgraded, {already_n} already verified, \
         {tamper_n} tamper-flagged, {minisig_n} missing minisig \
         (of {total} ISOs).",
        upgraded_n = upgraded.len(),
        already_n = already_verified.len(),
        tamper_n = tamper_flagged.len(),
        minisig_n = minisig_missing.len(),
    );
    if !tamper_flagged.is_empty() {
        eprintln!();
        eprintln!(
            "WARNING: {} ISO(s) have .sha256 sidecars that don't match their bytes. \
             This is a tamper signal — inspect them before booting.",
            tamper_flagged.len()
        );
    }

    if mount.temporary {
        unmount_temp(&mount);
    }

    if !io_errors.is_empty() {
        return Err(1);
    }
    Ok(())
}

/// Truncate a hex digest to 12 chars + ellipsis for CLI output.
fn short_hex(s: &str) -> String {
    if s.len() <= 14 {
        return s.to_string();
    }
    let mut end = 12;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

/// Per-run classification of every `.iso` file found during a scan.
#[derive(Debug, Default, PartialEq, Eq)]
struct ScanSummary {
    total: usize,
    upgraded: Vec<ScanEntry>,
    already_verified: Vec<ScanEntry>,
    tamper_flagged: Vec<ScanTamperEntry>,
    minisig_missing: Vec<ScanEntry>,
    io_errors: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ScanEntry {
    /// `AEGIS_ISOS`-relative display path (e.g. `ubuntu-24.04/ubuntu-24.04.iso`).
    rel_path: String,
    size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ScanTamperEntry {
    rel_path: String,
    expected: String,
    actual: String,
}

/// Walk the mounted `AEGIS_ISOS` partition, classify each `.iso` by
/// sidecar state, and generate missing `.sha256` sidecars. Returns a
/// [`ScanSummary`] for the CLI to render.
fn scan_for_upgrades(mount: &Mount) -> ScanSummary {
    let entries = scan_isos(&mount.path);
    let mut summary = ScanSummary {
        total: entries.len(),
        ..Default::default()
    };

    for iso in entries {
        let iso_abs = match iso.folder.as_deref() {
            Some(folder) => mount.path.join(folder).join(&iso.name),
            None => mount.path.join(&iso.name),
        };
        let rel_path = match iso.folder.as_deref() {
            Some(f) => format!("{f}/{}", iso.name),
            None => iso.name.clone(),
        };

        // Track minisig-missing for every ISO — #479 can't generate
        // minisigs, but the summary surfaces them so operators see
        // the upgrade ceiling.
        if !iso.has_minisig {
            summary.minisig_missing.push(ScanEntry {
                rel_path: rel_path.clone(),
                size: iso.size,
            });
        }

        if iso.has_sha256 {
            match check_existing_sha256(&iso_abs) {
                ExistingSha256Check::Match => summary.already_verified.push(ScanEntry {
                    rel_path,
                    size: iso.size,
                }),
                ExistingSha256Check::Mismatch { expected, actual } => {
                    summary.tamper_flagged.push(ScanTamperEntry {
                        rel_path,
                        expected,
                        actual,
                    });
                }
                ExistingSha256Check::Error(reason) => {
                    summary.io_errors.push((rel_path, reason));
                }
            }
            continue;
        }

        // No sidecar yet — compute + write.
        match write_generated_sha256(&iso_abs, &rel_path, mount) {
            Ok(()) => {
                summary.upgraded.push(ScanEntry {
                    rel_path: rel_path.clone(),
                    size: iso.size,
                });
                // Host-side attestation entry.
                let _ = crate::attest::record_iso_added(
                    &mount.path,
                    &iso_abs,
                    vec!["sha256".to_string()],
                );
            }
            Err(reason) => {
                summary.io_errors.push((rel_path, reason));
            }
        }
    }

    summary
}

/// Compute the actual sha256 of an ISO on disk and compare against
/// the declared value in its sibling `.sha256` file. Returns a
/// 3-state verdict so the scan summary can route match vs mismatch
/// vs I/O error to different reporting paths.
enum ExistingSha256Check {
    Match,
    Mismatch { expected: String, actual: String },
    Error(String),
}

fn check_existing_sha256(iso_abs: &Path) -> ExistingSha256Check {
    let sidecar_path = sha256_sidecar_path(iso_abs);
    let body = match std::fs::read_to_string(&sidecar_path) {
        Ok(s) => s,
        Err(e) => return ExistingSha256Check::Error(format!("read sidecar: {e}")),
    };
    let Some(expected) = parse_sha256_sidecar(&body) else {
        return ExistingSha256Check::Error("sidecar has no parseable sha256 line".to_string());
    };
    let actual = match iso_probe::compute_iso_sha256(iso_abs) {
        Ok(h) => h,
        Err(e) => return ExistingSha256Check::Error(format!("hash: {e}")),
    };
    if actual.eq_ignore_ascii_case(&expected) {
        ExistingSha256Check::Match
    } else {
        ExistingSha256Check::Mismatch {
            expected: expected.to_ascii_lowercase(),
            actual,
        }
    }
}

/// Compute + write a `<iso>.sha256` sidecar for an ISO that didn't
/// have one. Writes in coreutils-compatible `<hex>  <basename>\n`
/// format so `sha256sum -c` can verify it independently.
///
/// Atomic: stages to a tempfile, fsyncs, then copies via
/// [`copy_with_sudo`] so the sidecar either exists completely or
/// not at all (no half-written file if we're interrupted).
fn write_generated_sha256(iso_abs: &Path, rel_path: &str, _mount: &Mount) -> Result<(), String> {
    let digest = iso_probe::compute_iso_sha256(iso_abs).map_err(|e| format!("hash: {e}"))?;
    let basename = iso_abs
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("no basename for {rel_path}"))?;
    // Standard sha256sum format: `<hex>  <basename>\n` (double-space).
    let body = format!("{digest}  {basename}\n");
    let dest = sha256_sidecar_path(iso_abs);

    // Try a direct write first. Succeeds when the operator already
    // has write access to the mount (e.g. udisks2 mounted it rw as
    // their user, or they're running the whole command as root, or
    // unit tests writing to a tempdir). Falls through to sudo only
    // when we hit EPERM/EACCES — keeps the test path + the
    // "operator already root" path fast and prompt-free.
    match write_atomic(&dest, body.as_bytes()) {
        Ok(()) => return Ok(()),
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ReadOnlyFilesystem
            ) =>
        {
            // fall through to the sudo path below
        }
        Err(e) => return Err(format!("direct write: {e}")),
    }

    // Sudo fallback: stage to a private tempfile (that we can write
    // without elevation), then `sudo cp` into the mount. Matches the
    // pattern already used by `write_sidecar_via_sudo`.
    use std::io::Write as _;
    let mut staging = tempfile::Builder::new()
        .prefix("aegis-sha256-")
        .suffix(".txt")
        .tempfile()
        .map_err(|e| format!("staging tempfile: {e}"))?;
    staging
        .write_all(body.as_bytes())
        .map_err(|e| format!("staging write: {e}"))?;
    staging
        .as_file()
        .sync_all()
        .map_err(|e| format!("staging sync: {e}"))?;
    copy_with_sudo(staging.path(), &dest)?;
    Ok(())
}

/// Atomically write `bytes` to `dest`: stage in a sibling tempfile
/// (same parent dir so rename is an in-fs move), fsync, rename over
/// the target. Either the sidecar exists completely with the new
/// content or the old one (or none) remains — never a half-written
/// file.
fn write_atomic(dest: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let parent = dest
        .parent()
        .ok_or_else(|| std::io::Error::other("sidecar dest has no parent"))?;
    let mut staged = tempfile::Builder::new()
        .prefix(".aegis-sha256-staged-")
        .tempfile_in(parent)?;
    staged.write_all(bytes)?;
    staged.as_file().sync_all()?;
    staged.persist(dest).map_err(|e| e.error)?;
    Ok(())
}

/// Compute the canonical `<iso>.sha256` sidecar path — matches what
/// `iso-probe` looks for in its `find_expected_hash` + what
/// `copy_classic_sidecars` copies.
fn sha256_sidecar_path(iso_abs: &Path) -> PathBuf {
    let mut p = iso_abs.to_path_buf();
    let ext = p
        .extension()
        .map(|e| e.to_string_lossy().to_string())
        .unwrap_or_default();
    p.set_extension(if ext.is_empty() {
        "sha256".to_string()
    } else {
        format!("{ext}.sha256")
    });
    p
}

/// Extract the hex digest from a sha256 sidecar body. Accepts either
/// the bare-hex form or the coreutils `<hex>  <filename>` form.
fn parse_sha256_sidecar(body: &str) -> Option<String> {
    for line in body.lines() {
        let token = line.split_whitespace().next()?;
        if token.len() == 64 && token.chars().all(|c| c.is_ascii_hexdigit()) {
            return Some(token.to_ascii_lowercase());
        }
    }
    None
}

/// Append the just-added ISO to the matching attestation receipt.
/// Best-effort: failure logs a warning + returns Ok — the ISO is
/// already on the stick regardless. Extracted so `try_run_add` stays
/// under the 100-line budget.
fn record_add_attestation(mount: &Path, iso_src: &Path, sidecars_copied: Vec<String>) {
    match crate::attest::record_iso_added(mount, iso_src, sidecars_copied) {
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

pub(crate) struct IsoEntry {
    /// ISO basename only — never includes a directory component.
    /// Kept as a basename so shell scripts that treat the old flat
    /// layout's `name` field as `basename(1)` output keep working;
    /// the subfolder path lives in [`IsoEntry::folder`] alongside.
    pub(crate) name: String,
    /// Relative path from the mount root to the folder containing
    /// this ISO, or `None` when the ISO sits at the root (flat
    /// layout — the pre-#274-Phase-6a behavior). Always
    /// forward-slash separated regardless of host OS, matching the
    /// exFAT stick filesystem's canonical form.
    pub(crate) folder: Option<String>,
    pub(crate) size: u64,
    pub(crate) has_sha256: bool,
    pub(crate) has_minisig: bool,
    /// Operator-curated display name from `<iso>.aegis.toml`, when present. (#246)
    display_name: Option<String>,
    /// Operator-curated description from `<iso>.aegis.toml`, when present. (#246)
    description: Option<String>,
}

#[cfg(test)]
impl IsoEntry {
    /// Test constructor for call sites that only care about
    /// trust-state fields (doctor's trust-coverage rendering in
    /// particular). Defaults `display_name` + `description` to None.
    pub(crate) fn new_for_test(
        name: impl Into<String>,
        folder: Option<String>,
        has_sha256: bool,
        has_minisig: bool,
    ) -> Self {
        Self {
            name: name.into(),
            folder,
            size: 0,
            has_sha256,
            has_minisig,
            display_name: None,
            description: None,
        }
    }
}

/// Parsed flags for `aegis-boot add`.
#[derive(Debug)]
struct AddArgs {
    iso_path: PathBuf,
    mount_arg: Option<String>,
    description: Option<String>,
    version: Option<String>,
    category: Option<String>,
    /// #274 Phase 6b: place the ISO + sidecars in a subfolder under
    /// `AEGIS_ISOS/<folder>/` instead of at the mount root. Matched
    /// by the recursive `scan_isos` from Phase 6a so `list` renders
    /// it correctly. Validated by [`validate_folder_name`] to reject
    /// path-traversal / reserved-char / exFAT-unsafe input.
    folder: Option<String>,
    /// #479: `--scan` mode. When true, [`try_run_add`] bypasses the
    /// copy-one-ISO path and instead walks `AEGIS_ISOS` to generate
    /// missing `.sha256` sidecars for drag-and-dropped ISOs that
    /// render as tier-2 (BareUnverified) in rescue-tui. The
    /// positional argument (if any) is treated as the mount arg
    /// instead of an ISO path.
    scan: bool,
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
    let mut folder: Option<String> = None;
    let mut scan = false;
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
            "--folder" => {
                i += 1;
                let Some(v) = args.get(i) else {
                    eprintln!("aegis-boot add: --folder requires a value");
                    return Err(2);
                };
                folder = Some(v.clone());
            }
            s if s.starts_with("--folder=") => {
                folder = Some(s["--folder=".len()..].to_string());
            }
            "--scan" => {
                scan = true;
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
    // #479 --scan mode: the positional is (optional) mount arg, not an
    // ISO path. If no positional was given, mount_arg stays None and
    // resolve_mount auto-detects AEGIS_ISOS.
    if scan {
        // In scan mode, iso_path was being collected into the positional
        // slot — treat it as the mount arg instead.
        if let Some(p) = iso_path.as_ref() {
            if mount_arg.is_none() {
                mount_arg = Some(p.to_string_lossy().into_owned());
            } else {
                eprintln!(
                    "aegis-boot add: --scan accepts a single <mount> argument; got \
                     '{}' and '{}'",
                    p.display(),
                    mount_arg.as_deref().unwrap_or("")
                );
                return Err(2);
            }
        }
        if description.is_some() || version.is_some() || category.is_some() || folder.is_some() {
            eprintln!(
                "aegis-boot add: --scan is incompatible with \
                 --description / --version / --category / --folder"
            );
            return Err(2);
        }
        return Ok(AddArgs {
            iso_path: PathBuf::new(), // unused in scan mode
            mount_arg,
            description: None,
            version: None,
            category: None,
            folder: None,
            scan: true,
        });
    }

    let Some(iso_path) = iso_path else {
        eprintln!("aegis-boot add: missing required <iso-file> argument");
        return Err(2);
    };
    if let Some(ref f) = folder {
        if let Err(reason) = validate_folder_name(f) {
            eprintln!("aegis-boot add: --folder {f:?}: {reason}");
            return Err(2);
        }
    }
    Ok(AddArgs {
        iso_path,
        mount_arg,
        description,
        version,
        category,
        folder,
        scan: false,
    })
}

/// #274 Phase 6b: validate a `--folder` argument. Returns `Err` with a
/// specific reason string on any rejection. Accept ONLY names that are
/// safe on exFAT (the `AEGIS_ISOS` filesystem) AND match the slug-ish
/// shape that Phase 6a's recursive `scan_isos` will read back.
///
/// Rules (must ALL pass):
/// - Non-empty
/// - No path separator (`/`, `\\`) — prevents nested paths and traversal
/// - No `..` — belt-and-suspenders against traversal
/// - No leading dot — matches Phase 6a's dot-prefix skip
/// - No whitespace — cross-OS filesystem pain
/// - No exFAT-reserved characters: `< > : " | ? *` (and the null byte,
///   though that's unreachable from a shell argv in practice)
/// - ≤ 64 bytes (exFAT allows 255-char filenames but operator folder
///   names over 64 chars are almost always a mistake; shorter = safer)
fn validate_folder_name(name: &str) -> Result<(), &'static str> {
    if name.is_empty() {
        return Err("folder name is empty");
    }
    if name.len() > 64 {
        return Err("folder name exceeds 64 bytes");
    }
    if name.starts_with('.') {
        return Err("folder name must not start with '.'");
    }
    for c in name.chars() {
        if c == '/' || c == '\\' {
            return Err("folder name must not contain a path separator");
        }
        if c.is_whitespace() {
            return Err("folder name must not contain whitespace");
        }
        if matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*' | '\0') {
            return Err("folder name must not contain any of: < > : \" | ? *");
        }
    }
    if name == ".." || name.contains("..") {
        return Err("folder name must not contain '..'");
    }
    Ok(())
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
    println!("USAGE: aegis-boot add <iso-file-or-catalog-slug> [/dev/sdX | /mnt/aegis-isos]");
    println!("                  [--folder NAME]");
    println!("                  [--description TEXT] [--version VER] [--category CAT]");
    println!();
    println!("       aegis-boot add --scan [/dev/sdX | /mnt/aegis-isos]");
    println!();
    println!("If the first arg is NOT a file on disk but IS a known catalog slug");
    println!("(e.g. 'ubuntu-24.04-live-server'), add fetches + verifies it first,");
    println!("then stages the cached copy — collapses 'fetch X && add <path>' (#352).");
    println!();
    println!("--folder NAME places the ISO + sidecars under AEGIS_ISOS/NAME/ instead");
    println!("of the root. list + rescue-tui handle both layouts transparently (#274");
    println!("Phase 6a). Name must be a single path segment (no '/', no '..', no");
    println!("leading '.', no whitespace, ≤64 bytes, no exFAT-reserved chars).");
    println!();
    println!("Optional sidecar metadata (#246) is written next to the ISO as");
    println!("<iso>.aegis.toml so rescue-tui can show 'Network-install Debian 12'");
    println!("instead of the bare filename. The sidecar is unsigned cosmetic");
    println!("metadata — boot decisions still key off the sha256-attested manifest.");
    println!();
    println!("--scan (#479) walks AEGIS_ISOS looking for .iso files without .sha256");
    println!("sidecars, streams each through sha256, and writes coreutils-compatible");
    println!("sidecars so rescue-tui upgrades them from tier 2 (BareUnverified) to");
    println!("tier 1 (OperatorAttested). Existing sidecars are verified but never");
    println!("overwritten — a mismatch surfaces as a tamper signal instead. Minisig");
    println!("sidecars can't be generated (would need the operator's private key).");
}

/// #352 UX-4: resolve the `<iso-or-slug>` positional arg to a file
/// path.
///
/// If the arg is a real file, returns it unchanged. If it's not a
/// file but IS a known catalog slug, runs `fetch` to download +
/// verify it, then returns the cached ISO path. Otherwise returns
/// `Ok(None)` to signal a usage error (neither file nor slug).
///
/// Intentionally preserves the existing "not a file" error path for
/// unknown / typo'd slugs — we don't want `add --help-typo-lookup`
/// to silently try to interpret every typo as a slug.
fn resolve_iso_arg(raw: &Path) -> Result<Option<PathBuf>, u8> {
    if raw.is_file() {
        return Ok(Some(raw.to_path_buf()));
    }
    // Catalog-slug path: arg must be a bare slug (no /, no spaces),
    // else it looks like a mistyped path and we preserve the old
    // "not a file" error.
    let Some(s) = raw.to_str() else {
        return Ok(None);
    };
    if s.contains('/') || s.contains(char::is_whitespace) || s.starts_with('.') {
        return Ok(None);
    }
    let Some(expected) = crate::fetch::cached_iso_path(s) else {
        return Ok(None); // unknown slug — fall through to "not a file"
    };
    if expected.is_file() {
        println!(
            "aegis-boot add: catalog slug '{s}' already cached at {}",
            expected.display()
        );
    } else {
        println!("aegis-boot add: catalog slug '{s}' — fetching before stage");
        println!();
        crate::fetch::try_run(&[s.to_string()])?;
        println!();
    }
    if !expected.is_file() {
        eprintln!(
            "aegis-boot add: fetch succeeded but cached ISO not found at {}",
            expected.display()
        );
        return Err(1);
    }
    Ok(Some(expected))
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

/// Maximum subfolder depth `scan_isos` will descend into. Chosen to
/// match the precedent in `iso-parser::find_iso_files` — deep enough
/// for realistic per-distro layouts (`ubuntu-24.04/lts/<iso>`) but
/// shallow enough that a malformed mount with runaway directory
/// nesting can't stall the listing. Hit with a level-4 ISO in the
/// `depth_cap_skips_level_4_iso` test.
const ISO_SCAN_MAX_DEPTH: usize = 3;

/// Recursively enumerate ISO files under `root` (#274 Phase 6a).
/// Returns one [`IsoEntry`] per `.iso` file found within the depth
/// cap. The `folder` field is set to the parent directory relative
/// to `root` (forward-slash separated) or `None` when the ISO sits
/// at the root. Flat layouts produce identical output to pre-Phase-6a
/// behavior by construction.
///
/// Safety invariants enforced during the walk:
/// - **Depth cap**: no descent beyond `ISO_SCAN_MAX_DEPTH` (contrarian
///   flag: prevent runaway scans on malformed mounts).
/// - **Symlink skip**: directories that are symlinks are never
///   descended into (contrarian flag: no symlink-loop exploit, no
///   walk-outside-mount traversal).
/// - **Dot-prefix skip**: directories starting with `.` are ignored
///   (matches `iso-parser::find_iso_files` at crates/iso-parser/src/lib.rs:688).
/// - **Sidecar locality**: `<iso>.sha256` / `<iso>.minisig` lookup is
///   per-folder — a sidecar in a different directory does NOT count.
pub(crate) fn scan_isos(root: &Path) -> Vec<IsoEntry> {
    let mut out = Vec::new();
    scan_isos_recursive(root, root, 0, &mut out);
    out.sort_by(|a, b| {
        let a_key = (a.folder.as_deref().unwrap_or(""), a.name.as_str());
        let b_key = (b.folder.as_deref().unwrap_or(""), b.name.as_str());
        a_key.cmp(&b_key)
    });
    out
}

fn scan_isos_recursive(root: &Path, dir: &Path, depth: usize, out: &mut Vec<IsoEntry>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut filenames: Vec<(String, u64)> = Vec::new();
    let mut sidecar_names: Vec<String> = Vec::new();
    let mut subdirs: Vec<PathBuf> = Vec::new();
    for e in entries.flatten() {
        let entry_path = e.path();
        // file_type() returns Ok(FileType) without resolving symlinks —
        // so we can cheaply tell "symlink" from "real dir" without
        // stat()ing the target. Skip symlinks entirely per the
        // contrarian's loop-safety flag.
        let Ok(file_type) = e.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        if file_type.is_dir() {
            if !name.starts_with('.') && depth < ISO_SCAN_MAX_DEPTH {
                subdirs.push(entry_path);
            }
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
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
    let folder = relative_folder(root, dir);
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
            folder: folder.clone(),
            size,
            has_sha256,
            has_minisig,
            display_name,
            description,
        });
    }
    for sub in subdirs {
        scan_isos_recursive(root, &sub, depth + 1, out);
    }
}

/// Convert `dir`'s path relative to `root` into a forward-slash path,
/// returning `None` for `dir == root`. Forward-slash regardless of
/// host OS because the stick filesystem (exFAT) canonicalizes to `/`
/// and consumers parsing the JSON envelope should not care what the
/// scanning host looked like.
fn relative_folder(root: &Path, dir: &Path) -> Option<String> {
    let rel = dir.strip_prefix(root).ok()?;
    if rel.as_os_str().is_empty() {
        return None;
    }
    let parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
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

/// #274 Phase 6b — compute the destination dir for an add operation.
/// When `folder` is `Some`, target `mount/<folder>/` and create it via
/// sudo if absent; flat layout (no `--folder`) returns `mount` itself.
/// Split out so `try_run_add` stays under the 100-line budget.
fn resolve_add_dest_dir(mount: &Path, folder: Option<&str>) -> Result<PathBuf, String> {
    let Some(f) = folder else {
        return Ok(mount.to_path_buf());
    };
    let subdir = mount.join(f);
    if !subdir.exists() {
        println!("  Creating folder {f}/ under {}...", mount.display());
        mkdir_with_sudo(&subdir)
            .map_err(|e| format!("mkdir -p {} failed: {e}", subdir.display()))?;
    }
    Ok(subdir)
}

/// #274 Phase 6b helper — `mkdir -p <dir>` via sudo because `AEGIS_ISOS`
/// is typically root-owned on a freshly-flashed stick. Mirrors the
/// error-propagation shape of [`copy_with_sudo`] so both destructive
/// operations look the same to the caller.
fn mkdir_with_sudo(dir: &Path) -> Result<(), String> {
    let out = Command::new("sudo")
        .args(["mkdir", "-p", &dir.display().to_string()])
        .output()
        .map_err(|e| format!("mkdir exec: {e}"))?;
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

    // ---- #274 Phase 6b: --folder flag -------------------------------------

    #[test]
    fn parse_add_args_recognises_folder_both_forms() {
        let a = parse_add_args(&[s("debian.iso"), s("--folder"), s("debian-12")]).unwrap();
        assert_eq!(a.folder.as_deref(), Some("debian-12"));
        let b = parse_add_args(&[s("debian.iso"), s("--folder=debian-12")]).unwrap();
        assert_eq!(b.folder.as_deref(), Some("debian-12"));
    }

    #[test]
    fn parse_add_args_rejects_dangling_folder_flag() {
        let err = parse_add_args(&[s("debian.iso"), s("--folder")]).unwrap_err();
        assert_eq!(err, 2);
    }

    #[test]
    fn parse_add_args_rejects_unsafe_folder_name() {
        // Path traversal — explicit .. must fail at argv-parse time.
        let err = parse_add_args(&[s("debian.iso"), s("--folder=../evil")]).unwrap_err();
        assert_eq!(err, 2);
        // Path separator — multi-segment path must fail.
        let err = parse_add_args(&[s("debian.iso"), s("--folder=a/b")]).unwrap_err();
        assert_eq!(err, 2);
        // Leading dot — matches Phase 6a's skip rule.
        let err = parse_add_args(&[s("debian.iso"), s("--folder=.hidden")]).unwrap_err();
        assert_eq!(err, 2);
        // Whitespace — cross-OS FS pain.
        let err = parse_add_args(&[s("debian.iso"), s("--folder=has space")]).unwrap_err();
        assert_eq!(err, 2);
        // exFAT-reserved colon.
        let err = parse_add_args(&[s("debian.iso"), s("--folder=win:bad")]).unwrap_err();
        assert_eq!(err, 2);
    }

    #[test]
    fn validate_folder_name_accepts_reasonable_names() {
        for good in [
            "ubuntu-24.04",
            "ubuntu24",
            "debian-12-network",
            "alpine-3.20-standard",
            "a",
            "numbers-123-and-dashes",
        ] {
            assert!(validate_folder_name(good).is_ok(), "should accept {good:?}");
        }
    }

    #[test]
    fn validate_folder_name_rejects_explicit_cases() {
        for (bad, _why) in [
            ("", "empty"),
            ("..", "literal dot-dot"),
            ("a..b", "embedded dot-dot"),
            (".hidden", "leading dot"),
            ("/absolute", "leading slash"),
            ("sub/path", "internal slash"),
            ("win\\path", "backslash"),
            ("has space", "whitespace"),
            ("win:bad", "colon"),
            ("win|bad", "pipe"),
            ("win?bad", "question mark"),
            ("win*bad", "asterisk"),
            ("win\"bad", "double quote"),
            ("win<bad", "less-than"),
            ("win>bad", "greater-than"),
        ] {
            assert!(validate_folder_name(bad).is_err(), "should reject {bad:?}");
        }
        // Length cap.
        let too_long = "a".repeat(65);
        assert!(validate_folder_name(&too_long).is_err());
        let exactly_64 = "a".repeat(64);
        assert!(validate_folder_name(&exactly_64).is_ok(), "64 is the cap");
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
        use crate::userfacing::{UserFacing, render_string};
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

    // ---- #274 Phase 6a: recursive subfolder scan --------------------------

    #[test]
    fn scan_isos_flat_layout_preserves_folder_none() {
        // Regression gate: existing flat-layout sticks must behave
        // exactly as before — `folder` is None and `name` is the
        // basename. A broken output here would silently break any
        // shell script that joined `mount_path + name`.
        use std::fs;
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("alpha.iso"), b"a").unwrap();
        fs::write(dir.path().join("beta.iso"), b"b").unwrap();
        let entries = scan_isos(dir.path());
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|e| e.folder.is_none()));
        assert_eq!(entries[0].name, "alpha.iso");
        assert_eq!(entries[1].name, "beta.iso");
    }

    #[test]
    fn scan_isos_recurses_into_subfolders_with_folder_set() {
        use std::fs;
        let root = tempfile::tempdir().unwrap();
        let sub = root.path().join("ubuntu-24.04");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("ubuntu-24.04.2-live-server.iso"), b"x").unwrap();
        let entries = scan_isos(root.path());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "ubuntu-24.04.2-live-server.iso");
        assert_eq!(entries[0].folder.as_deref(), Some("ubuntu-24.04"));
    }

    #[test]
    fn scan_isos_sidecars_are_per_folder() {
        // Security-adjacent invariant: a sha256 in a DIFFERENT folder
        // must NOT count toward a subfolder ISO's trust state, and
        // vice-versa. Prevents operators being misled into thinking
        // an ISO is hash-attested when the sibling sha256 actually
        // belongs to an unrelated file.
        use std::fs;
        let root = tempfile::tempdir().unwrap();
        let sub_a = root.path().join("a");
        let sub_b = root.path().join("b");
        fs::create_dir(&sub_a).unwrap();
        fs::create_dir(&sub_b).unwrap();
        fs::write(sub_a.join("a.iso"), b"x").unwrap();
        fs::write(sub_b.join("b.iso"), b"y").unwrap();
        // sidecar intentionally in WRONG folder
        fs::write(sub_b.join("a.iso.sha256"), b"0000").unwrap();
        let entries = scan_isos(root.path());
        assert_eq!(entries.len(), 2);
        let a = entries.iter().find(|e| e.name == "a.iso").unwrap();
        let b = entries.iter().find(|e| e.name == "b.iso").unwrap();
        assert!(
            !a.has_sha256,
            "cross-folder sha256 must not satisfy the trust claim for a.iso"
        );
        assert!(
            !b.has_sha256,
            "misfiled sha256 does not attest b.iso either"
        );
        // Put the sha256 in the CORRECT folder and confirm it counts.
        fs::write(sub_a.join("a.iso.sha256"), b"0000").unwrap();
        let entries = scan_isos(root.path());
        let a = entries.iter().find(|e| e.name == "a.iso").unwrap();
        assert!(a.has_sha256, "same-folder sha256 counts");
    }

    #[test]
    fn scan_isos_skips_dot_prefixed_dirs() {
        // Parity with iso-parser::find_iso_files (lib.rs:688) — dot
        // directories are OS-housekeeping (.Trashes, .Spotlight) and
        // must never be descended into.
        use std::fs;
        let root = tempfile::tempdir().unwrap();
        let hidden = root.path().join(".Trashes");
        fs::create_dir(&hidden).unwrap();
        fs::write(hidden.join("ghost.iso"), b"x").unwrap();
        let entries = scan_isos(root.path());
        assert_eq!(entries.len(), 0, "dot dirs must be skipped");
    }

    #[test]
    fn scan_isos_respects_depth_cap() {
        // Architect's requested test: ISO at depth 4 must NOT be
        // returned. The cap prevents runaway walks on malformed mounts
        // with pathological nesting. If the cap is ever changed, update
        // ISO_SCAN_MAX_DEPTH + this test together.
        use std::fs;
        let root = tempfile::tempdir().unwrap();
        let l1 = root.path().join("l1");
        let l2 = l1.join("l2");
        let l3 = l2.join("l3");
        let l4 = l3.join("l4");
        fs::create_dir_all(&l4).unwrap();
        // depth 1, 2, 3 should be included; depth 4 should not.
        fs::write(l1.join("lvl1.iso"), b"a").unwrap();
        fs::write(l2.join("lvl2.iso"), b"b").unwrap();
        fs::write(l3.join("lvl3.iso"), b"c").unwrap();
        fs::write(l4.join("lvl4.iso"), b"d").unwrap();
        let entries = scan_isos(root.path());
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"lvl1.iso"));
        assert!(names.contains(&"lvl2.iso"));
        assert!(names.contains(&"lvl3.iso"));
        assert!(
            !names.contains(&"lvl4.iso"),
            "depth-4 ISO must be skipped per the cap"
        );
    }

    #[test]
    #[cfg(unix)]
    fn scan_isos_does_not_follow_symlink_dirs() {
        // Contrarian's requested test: symlinked directories must NOT
        // be descended into. Guards against symlink loops and against
        // an attacker putting a symlink on the stick that points
        // outside the mount (e.g. → /etc). No resolution, no descent.
        use std::fs;
        let root = tempfile::tempdir().unwrap();
        let real = root.path().join("real");
        fs::create_dir(&real).unwrap();
        fs::write(real.join("inside.iso"), b"x").unwrap();
        let linked = root.path().join("linked");
        std::os::unix::fs::symlink(&real, &linked).unwrap();
        let entries = scan_isos(root.path());
        // Only the direct "real" subfolder yields the ISO. The
        // symlinked "linked" must not double-count.
        assert_eq!(
            entries.len(),
            1,
            "symlinked dir must not be descended (would double-count)"
        );
        assert_eq!(entries[0].folder.as_deref(), Some("real"));
    }

    #[test]
    fn scan_isos_sorts_folder_then_name() {
        use std::fs;
        let root = tempfile::tempdir().unwrap();
        // Deliberately out of lexicographic order across folders.
        let zeta = root.path().join("zeta");
        let alpha = root.path().join("alpha");
        fs::create_dir(&zeta).unwrap();
        fs::create_dir(&alpha).unwrap();
        fs::write(root.path().join("root.iso"), b"r").unwrap();
        fs::write(zeta.join("z1.iso"), b"z").unwrap();
        fs::write(alpha.join("a1.iso"), b"a").unwrap();
        let entries = scan_isos(root.path());
        assert_eq!(entries.len(), 3);
        // Root-level ISOs sort first (empty folder string < any real
        // folder), then alpha, then zeta.
        assert_eq!(entries[0].name, "root.iso");
        assert_eq!(entries[0].folder, None);
        assert_eq!(entries[1].folder.as_deref(), Some("alpha"));
        assert_eq!(entries[2].folder.as_deref(), Some("zeta"));
    }

    // ---- #352 UX-4: catalog-slug shortcut for `add` ------------------------

    #[test]
    fn resolve_iso_arg_returns_path_for_existing_file() {
        use std::fs;
        let dir = tempfile::tempdir().unwrap();
        let iso = dir.path().join("local.iso");
        fs::write(&iso, b"x").unwrap();
        let got = resolve_iso_arg(&iso).unwrap();
        assert_eq!(got.as_deref(), Some(iso.as_path()));
    }

    #[test]
    fn resolve_iso_arg_rejects_path_shaped_non_file() {
        // A path with a `/` is treated as a path attempt (not a slug),
        // so unknown → None → caller emits "not a file". Prevents
        // `add ./typo.iso` from accidentally being looked up as a slug.
        let got = resolve_iso_arg(Path::new("./does-not-exist.iso")).unwrap();
        assert_eq!(got, None);
    }

    #[test]
    fn resolve_iso_arg_rejects_whitespace_bearing_arg() {
        // A multi-word arg ("not a slug") is treated as a path attempt.
        let got = resolve_iso_arg(Path::new("not a slug")).unwrap();
        assert_eq!(got, None);
    }

    #[test]
    fn resolve_iso_arg_rejects_unknown_slug() {
        // Slug-shaped arg that doesn't match any catalog entry falls
        // back to the "not a file" error path — preserves the old
        // error for typos like `add ubntu-24.04`.
        let got = resolve_iso_arg(Path::new("totally-not-in-catalog-zzz")).unwrap();
        assert_eq!(got, None);
    }

    // ---- #479 --scan mode -------------------------------------------

    fn parse(args: &[&str]) -> Result<AddArgs, u8> {
        let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        parse_add_args(&owned)
    }

    #[test]
    fn parse_scan_with_no_positional_produces_scan_mode() {
        let args = parse(&["--scan"]).unwrap();
        assert!(args.scan);
        assert!(args.mount_arg.is_none());
    }

    #[test]
    fn parse_scan_treats_positional_as_mount_arg() {
        let args = parse(&["--scan", "/dev/sda2"]).unwrap();
        assert!(args.scan);
        assert_eq!(args.mount_arg.as_deref(), Some("/dev/sda2"));
    }

    #[test]
    fn parse_scan_rejects_copy_mode_flags() {
        assert_eq!(parse(&["--scan", "--description", "x"]).unwrap_err(), 2);
        assert_eq!(parse(&["--scan", "--folder", "x"]).unwrap_err(), 2);
        assert_eq!(parse(&["--scan", "--version", "x"]).unwrap_err(), 2);
        assert_eq!(parse(&["--scan", "--category", "x"]).unwrap_err(), 2);
    }

    #[test]
    fn parse_scan_rejects_two_positionals() {
        // `--scan` takes at most one positional (the mount arg).
        assert_eq!(parse(&["--scan", "/dev/sda2", "/mnt/x"]).unwrap_err(), 2);
    }

    #[test]
    fn sha256_sidecar_path_appends_extension() {
        let p = sha256_sidecar_path(Path::new("/mnt/AEGIS_ISOS/ubuntu.iso"));
        assert_eq!(p, PathBuf::from("/mnt/AEGIS_ISOS/ubuntu.iso.sha256"));
    }

    #[test]
    fn sha256_sidecar_path_handles_extensionless_file() {
        // No extension → .sha256 becomes the sole extension.
        let p = sha256_sidecar_path(Path::new("/mnt/AEGIS_ISOS/no-ext"));
        assert_eq!(p, PathBuf::from("/mnt/AEGIS_ISOS/no-ext.sha256"));
    }

    #[test]
    fn parse_sha256_sidecar_accepts_bare_hex() {
        let hex = "0".repeat(64);
        assert_eq!(parse_sha256_sidecar(&hex).as_deref(), Some(hex.as_str()));
    }

    #[test]
    fn parse_sha256_sidecar_accepts_coreutils_format() {
        let hex = "a".repeat(64);
        let body = format!("{hex}  ubuntu-24.04.iso\n");
        assert_eq!(parse_sha256_sidecar(&body).as_deref(), Some(hex.as_str()));
    }

    #[test]
    fn parse_sha256_sidecar_lowercases_hex() {
        let hex = "A".repeat(64);
        let out = parse_sha256_sidecar(&hex).unwrap();
        assert_eq!(out, "a".repeat(64));
    }

    #[test]
    fn parse_sha256_sidecar_rejects_short_or_non_hex() {
        assert!(parse_sha256_sidecar("").is_none());
        assert!(parse_sha256_sidecar("not-a-hash").is_none());
        assert!(parse_sha256_sidecar(&"z".repeat(64)).is_none());
        assert!(parse_sha256_sidecar(&"f".repeat(63)).is_none());
    }

    #[test]
    fn short_hex_truncates_long_digests() {
        let s = "a".repeat(64);
        let out = short_hex(&s);
        assert!(out.ends_with('…'));
        assert_eq!(out.chars().filter(|c| *c == 'a').count(), 12);
    }

    #[test]
    fn short_hex_passes_short_strings() {
        assert_eq!(short_hex("abc"), "abc");
        assert_eq!(short_hex("deadbeefdeadbe"), "deadbeefdeadbe");
    }

    #[test]
    fn scan_summary_default_is_empty() {
        let s = ScanSummary::default();
        assert_eq!(s.total, 0);
        assert!(s.upgraded.is_empty());
        assert!(s.already_verified.is_empty());
        assert!(s.tamper_flagged.is_empty());
        assert!(s.minisig_missing.is_empty());
        assert!(s.io_errors.is_empty());
    }

    #[test]
    fn scan_for_upgrades_on_empty_mount_returns_empty_summary() {
        let dir = tempfile::tempdir().unwrap();
        let mount = Mount {
            path: dir.path().to_path_buf(),
            temporary: false,
            device: None,
        };
        let summary = scan_for_upgrades(&mount);
        assert_eq!(summary.total, 0);
        assert!(summary.upgraded.is_empty());
    }

    #[test]
    fn scan_for_upgrades_generates_sidecar_for_bare_iso() {
        // Put a fake "ISO" (any bytes, iso-probe won't mount it here —
        // scan_isos walks by extension, not by mounting).
        let dir = tempfile::tempdir().unwrap();
        let iso = dir.path().join("bare.iso");
        std::fs::write(&iso, b"fake iso contents for hashing").unwrap();

        let mount = Mount {
            path: dir.path().to_path_buf(),
            temporary: false,
            device: None,
        };
        let summary = scan_for_upgrades(&mount);
        assert_eq!(summary.total, 1);
        assert_eq!(summary.upgraded.len(), 1);
        assert_eq!(summary.upgraded[0].rel_path, "bare.iso");

        // Sidecar should now exist with coreutils format.
        let sidecar = dir.path().join("bare.iso.sha256");
        let body = std::fs::read_to_string(&sidecar).unwrap();
        assert!(body.contains("  bare.iso\n"));
        assert_eq!(body.split_whitespace().next().unwrap().len(), 64);

        // minisig is absent → entry shows up in minisig_missing too.
        assert_eq!(summary.minisig_missing.len(), 1);
    }

    #[test]
    fn scan_for_upgrades_skips_already_verified_isos() {
        let dir = tempfile::tempdir().unwrap();
        let iso = dir.path().join("ok.iso");
        let bytes = b"content";
        std::fs::write(&iso, bytes).unwrap();

        // Pre-seed a matching .sha256 sidecar (sha256 of "content").
        use sha2::{Digest, Sha256};
        let hash = hex::encode(Sha256::digest(bytes));
        let sidecar = dir.path().join("ok.iso.sha256");
        std::fs::write(&sidecar, format!("{hash}  ok.iso\n")).unwrap();

        let mount = Mount {
            path: dir.path().to_path_buf(),
            temporary: false,
            device: None,
        };
        let summary = scan_for_upgrades(&mount);
        assert_eq!(summary.total, 1);
        assert!(summary.upgraded.is_empty());
        assert_eq!(summary.already_verified.len(), 1);
        assert!(summary.tamper_flagged.is_empty());
    }

    #[test]
    fn scan_for_upgrades_flags_tampered_sidecars_without_overwriting() {
        let dir = tempfile::tempdir().unwrap();
        let iso = dir.path().join("forged.iso");
        std::fs::write(&iso, b"real bytes").unwrap();

        // Pre-seed a sidecar with the WRONG hash.
        let wrong = "0".repeat(64);
        let sidecar = dir.path().join("forged.iso.sha256");
        std::fs::write(&sidecar, format!("{wrong}  forged.iso\n")).unwrap();

        let mount = Mount {
            path: dir.path().to_path_buf(),
            temporary: false,
            device: None,
        };
        let summary = scan_for_upgrades(&mount);
        assert_eq!(summary.total, 1);
        assert!(summary.upgraded.is_empty());
        assert!(summary.already_verified.is_empty());
        assert_eq!(summary.tamper_flagged.len(), 1);
        assert_eq!(summary.tamper_flagged[0].expected, wrong);

        // Sidecar must NOT have been overwritten — tamper protection.
        let after = std::fs::read_to_string(&sidecar).unwrap();
        assert!(after.contains(&wrong));
    }

    #[test]
    fn scan_for_upgrades_tracks_minisig_missing_alongside_upgrade() {
        // An ISO with no sidecars at all produces both an upgrade
        // (sha256 written) AND a minisig_missing entry (we can't
        // generate minisig without the operator's key).
        let dir = tempfile::tempdir().unwrap();
        let iso = dir.path().join("bare.iso");
        std::fs::write(&iso, b"x").unwrap();

        let mount = Mount {
            path: dir.path().to_path_buf(),
            temporary: false,
            device: None,
        };
        let summary = scan_for_upgrades(&mount);
        assert_eq!(summary.upgraded.len(), 1);
        assert_eq!(summary.minisig_missing.len(), 1);
    }
}
