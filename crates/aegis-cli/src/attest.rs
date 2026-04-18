//! `aegis-boot attest` — attestation receipts for flashed sticks.
//!
//! A flash operation produces a JSON manifest recording exactly what
//! went onto the stick + the host environment that wrote it. The
//! manifest is stored at `$XDG_DATA_HOME/aegis-boot/attestations/`
//! (or `~/.local/share/aegis-boot/attestations/` if `XDG_DATA_HOME` is
//! unset). The first thing that gets stored on the stick (added with
//! `aegis-boot add` later) is the original flash manifest as a copy
//! at `/EFI/aegis-attestation.json` — but that's a follow-up; v0
//! ships the host-side store only.
//!
//! # Why this exists
//!
//! Every other USB-imaging tool is silent after flash. aegis-boot
//! is built on a "prove what's on the stick" trust narrative — the
//! attestation receipt is the artifact that operationalizes that
//! claim. Forensics gets chain-of-custody; sysadmin fleets get
//! per-stick inventory; security gets an audit trail.
//!
//! # Cryptographic signing
//!
//! v0 ships unsigned manifests. The trust anchor is "you ran this
//! command on this host, the timestamps and hashes are evidence."
//! TPM PCR attestation + minisign signing is tracked under epic #139
//! and will land alongside the TPM measured-boot work — bolted onto
//! v0's manifest as additional fields, not a schema rewrite.
//!
//! # Schema versioning
//!
//! Every manifest carries `schema_version: 1`. Future fields are
//! additive; consumers should ignore unknown fields. Breaking
//! schema changes bump this number.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use serde::{Deserialize, Serialize};

use crate::detect::Drive;

/// Schema version for the attestation manifest.
pub const SCHEMA_VERSION: u32 = 1;

/// One flash + zero-or-more ISO additions, captured as a single JSON
/// document on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attestation {
    pub schema_version: u32,
    pub tool_version: String,
    /// RFC 3339 / ISO 8601 timestamp of the flash. Generated via the
    /// host's `date -u +%FT%TZ` so we don't pull a chrono dep.
    pub flashed_at: String,
    pub operator: String,
    pub host: HostInfo,
    pub target: TargetInfo,
    /// Empty at flash time. Each `aegis-boot add` appends an entry.
    /// (Append-on-add lands in a follow-up; v0 just creates this empty.)
    pub isos: Vec<IsoRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostInfo {
    pub kernel: String,
    /// One of "enforcing" / "disabled" / "unknown".
    pub secure_boot: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetInfo {
    pub device: String,
    pub model: String,
    pub size_bytes: u64,
    /// Hex SHA-256 of the dd'd image.
    pub image_sha256: String,
    pub image_size_bytes: u64,
    /// GPT disk GUID, captured from sgdisk after partprobe.
    /// May be empty if sgdisk fails or the drive isn't partitioned.
    pub disk_guid: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IsoRecord {
    pub filename: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub sidecars: Vec<String>,
    pub added_at: String,
}

/// One-line summary of the attestation matching a mounted stick. Used
/// by `aegis-boot list` to surface chain-of-custody info above the ISO
/// table. Returns None if no matching attestation is on disk — the
/// operator may have flashed this stick on a different host, or
/// attestation recording failed.
#[derive(Debug, Clone)]
pub struct AttestSummary {
    pub flashed_at: String,
    pub operator: String,
    pub isos_recorded: usize,
    pub manifest_path: PathBuf,
}

/// Look up + summarize the attestation matching the stick mounted at
/// `mount_path`. Silent on miss — the caller decides whether to
/// surface that to the operator.
pub fn summary_for_mount(mount_path: &Path) -> Option<AttestSummary> {
    let dir = data_dir().join("attestations");
    if !dir.is_dir() {
        return None;
    }
    let (manifest_path, _source) = locate_attestation_for_mount(&dir, mount_path).ok()?;
    let manifest = read_attestation(&manifest_path).ok()?;
    Some(AttestSummary {
        flashed_at: manifest.flashed_at,
        operator: manifest.operator,
        isos_recorded: manifest.isos.len(),
        manifest_path,
    })
}

/// Append an `IsoRecord` to the most recent attestation matching the
/// destination stick. Used by `aegis-boot add`. Returns the manifest
/// path on success.
///
/// Lookup: derive the stick's disk GUID from `mount_path → /dev/sdXn
/// → /dev/sdX → sgdisk -p`, then find the newest `<guid>-*.json`
/// in `$XDG_DATA_HOME/aegis-boot/attestations/`. If no GUID can be
/// captured, falls back to the most recent attestation overall — which
/// is correct for the common single-stick workflow but ambiguous in
/// multi-stick sessions; that case prints a warning.
///
/// Failure to update an attestation must NOT fail the add — the ISO
/// is on the stick regardless. Caller (`inventory::run_add`) prints
/// a warning on Err and proceeds.
pub fn record_iso_added(
    mount_path: &Path,
    iso_path: &Path,
    sidecars: Vec<String>,
) -> Result<PathBuf, String> {
    let iso_filename = iso_path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("ISO path has no filename: {}", iso_path.display()))?
        .to_string();
    let iso_size = fs::metadata(iso_path)
        .map_err(|e| format!("stat {}: {e}", iso_path.display()))?
        .len();
    let iso_sha = sha256_file(iso_path)?;
    let added_at = current_iso_timestamp();

    // Locate the matching attestation file.
    let dest_dir = data_dir().join("attestations");
    let (manifest_path, source) = locate_attestation_for_mount(&dest_dir, mount_path)?;
    if matches!(source, LookupSource::FallbackNewest) {
        eprintln!(
            "attest: could not match disk GUID for {} — appended to most recent attestation: {}",
            mount_path.display(),
            manifest_path.display()
        );
    }

    let mut manifest = read_attestation(&manifest_path)?;
    manifest.isos.push(IsoRecord {
        filename: iso_filename,
        sha256: iso_sha,
        size_bytes: iso_size,
        sidecars,
        added_at,
    });
    let json =
        serde_json::to_string_pretty(&manifest).map_err(|e| format!("serialize manifest: {e}"))?;
    fs::write(&manifest_path, json)
        .map_err(|e| format!("write {}: {e}", manifest_path.display()))?;
    Ok(manifest_path)
}

/// Where the lookup actually found a result, for the caller to decide
/// whether to warn the operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LookupSource {
    /// Disk GUID matched a manifest filename prefix.
    GuidMatch,
    /// GUID couldn't be resolved or didn't match; picked most recent.
    FallbackNewest,
}

/// Locate the attestation manifest for a stick mounted at `mount_path`.
/// Resolution: read `/proc/mounts` → owning device (e.g. /dev/sdc2) →
/// strip partition suffix → disk GUID → newest matching manifest.
/// Falls back to "most recent overall" when GUID can't be resolved.
/// Silent — the returned `LookupSource` lets callers print whatever
/// context they want.
fn locate_attestation_for_mount(
    dir: &Path,
    mount_path: &Path,
) -> Result<(PathBuf, LookupSource), String> {
    if !dir.is_dir() {
        return Err(format!("no attestations dir at {}", dir.display()));
    }
    let entries: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|e| format!("read_dir {}: {e}", dir.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    if entries.is_empty() {
        return Err(format!("no attestation files in {}", dir.display()));
    }

    if let Some(guid) = crate::mounts::device_for_mount(mount_path)
        .and_then(|p| crate::mounts::parent_disk(&p))
        .and_then(|d| read_disk_guid(&d))
    {
        let lower = guid.to_lowercase();
        // Match files whose stem starts with the GUID, take newest by mtime.
        let mut matching: Vec<_> = entries
            .iter()
            .filter(|p| {
                p.file_stem()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.to_lowercase().starts_with(&lower))
            })
            .collect();
        matching.sort_by_key(|p| fs::metadata(p).and_then(|m| m.modified()).ok());
        if let Some(newest) = matching.last() {
            return Ok(((*newest).clone(), LookupSource::GuidMatch));
        }
    }

    // Fallback: most recent file by mtime.
    let mut all = entries;
    all.sort_by_key(|p| fs::metadata(p).and_then(|m| m.modified()).ok());
    all.last()
        .cloned()
        .map(|p| (p, LookupSource::FallbackNewest))
        .ok_or_else(|| "no attestation files found".to_string())
}

// device_for_mount + disk_for_partition moved to `mounts.rs` (as
// `mounts::device_for_mount` and `mounts::parent_disk`) so inventory.rs
// can share the implementation. See mounts.rs for the canonical docs.

/// Record a flash operation and return the path of the saved manifest.
/// On any failure, returns Err — the caller (`flash::flash`) prints
/// the error and proceeds with the rest of the post-flash output.
/// Failure to record an attestation must NOT fail the flash itself;
/// the data is on the stick regardless.
pub fn record_flash(drive: &Drive, img_path: &Path, img_size: u64) -> Result<PathBuf, String> {
    let timestamp = current_iso_timestamp();
    let operator = current_operator();
    let kernel = current_kernel();
    let sb = current_sb_state();
    let img_sha256 = sha256_file(img_path).unwrap_or_else(|e| {
        eprintln!("attest: SHA-256 of {} failed: {e}", img_path.display());
        String::new()
    });
    let disk_guid = read_disk_guid(&drive.dev).unwrap_or_default();

    let manifest = Attestation {
        schema_version: SCHEMA_VERSION,
        tool_version: env!("CARGO_PKG_VERSION").to_string(),
        flashed_at: timestamp.clone(),
        operator,
        host: HostInfo {
            kernel,
            secure_boot: sb,
        },
        target: TargetInfo {
            device: drive.dev.display().to_string(),
            model: drive.model.clone(),
            size_bytes: drive.size_bytes,
            image_sha256: img_sha256,
            image_size_bytes: img_size,
            disk_guid: disk_guid.clone(),
        },
        isos: Vec::new(),
    };

    let dest_dir = data_dir().join("attestations");
    fs::create_dir_all(&dest_dir)
        .map_err(|e| format!("create_dir_all {}: {e}", dest_dir.display()))?;

    // File name uses disk GUID (or "unknown" + sanitized device path) and
    // a sortable timestamp, so multiple flashes of the same stick produce
    // a chronological history rather than overwriting each other.
    let id = if disk_guid.is_empty() {
        format!(
            "unknown-{}",
            sanitize_for_filename(&drive.dev.display().to_string())
        )
    } else {
        disk_guid.to_lowercase()
    };
    let ts_for_filename = timestamp.replace(':', "-");
    let dest = dest_dir.join(format!("{id}-{ts_for_filename}.json"));

    let json =
        serde_json::to_string_pretty(&manifest).map_err(|e| format!("serialize manifest: {e}"))?;
    fs::write(&dest, json).map_err(|e| format!("write {}: {e}", dest.display()))?;

    Ok(dest)
}

/// Entry point for `aegis-boot attest [list|show <file>] [--help]`.
pub fn run(args: &[String]) -> ExitCode {
    let sub = args.first().map(String::as_str);
    // --json is accepted on `list` (applies to the enumeration).
    let json_mode = args.iter().any(|a| a == "--json");
    match sub {
        None | Some("--help" | "-h" | "help") => {
            print_help();
            ExitCode::SUCCESS
        }
        Some("list") => run_list(json_mode),
        Some("show") => run_show(&args[1..]),
        // Bare `aegis-boot attest --json` == `aegis-boot attest list --json`
        Some("--json") => run_list(true),
        Some(other) => {
            eprintln!("aegis-boot attest: unknown subcommand '{other}'");
            eprintln!("run 'aegis-boot attest --help' for usage");
            ExitCode::from(2)
        }
    }
}

fn print_help() {
    println!("aegis-boot attest — attestation receipts for flashed sticks");
    println!();
    println!("Each `aegis-boot flash` writes a JSON manifest recording exactly");
    println!("what went onto the stick and the host environment that wrote it.");
    println!("Manifests live in $XDG_DATA_HOME/aegis-boot/attestations/.");
    println!();
    println!("USAGE:");
    println!("  aegis-boot attest list              List all stored attestations");
    println!("  aegis-boot attest list --json       Machine-readable summary");
    println!("  aegis-boot attest show <FILE>       Pretty-print one attestation");
    println!("  aegis-boot attest show --json <FILE> Raw manifest JSON (full detail)");
    println!("  aegis-boot attest --help            This message");
    println!();
    println!("EXAMPLES:");
    println!("  aegis-boot attest list");
    println!("  aegis-boot attest show ~/.local/share/aegis-boot/attestations/abc123-2026-04-16T12-34-56Z.json");
    println!("  aegis-boot attest list --json | jq '.attestations[].flashed_at'");
}

fn run_list(json_mode: bool) -> ExitCode {
    let dir = data_dir().join("attestations");
    if !dir.is_dir() {
        if json_mode {
            println!("{{ \"schema_version\": 1, \"attestations\": [] }}");
        } else {
            println!("(no attestations recorded yet — run `aegis-boot flash` to create one)");
        }
        return ExitCode::SUCCESS;
    }
    let mut entries: Vec<_> = match fs::read_dir(&dir) {
        Ok(it) => it
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .collect(),
        Err(e) => {
            if json_mode {
                println!(
                    "{{ \"schema_version\": 1, \"error\": \"{}\" }}",
                    crate::doctor::json_escape(&format!("read_dir {}: {e}", dir.display()))
                );
            } else {
                eprintln!("aegis-boot attest list: read_dir {}: {e}", dir.display());
            }
            return ExitCode::from(1);
        }
    };
    entries.sort_by_key(std::fs::DirEntry::path);
    if json_mode {
        print_attest_list_json(&dir, &entries);
        return ExitCode::SUCCESS;
    }
    if entries.is_empty() {
        println!("(no attestations in {})", dir.display());
        return ExitCode::SUCCESS;
    }
    println!("Attestations in {}:", dir.display());
    println!();
    println!("  {:<30}  {:<24}  TARGET MODEL", "DEVICE", "FLASHED");
    println!(
        "  {:<30}  {:<24}  ------------",
        "-".repeat(30),
        "-".repeat(24)
    );
    for entry in &entries {
        let path = entry.path();
        match read_attestation(&path) {
            Ok(att) => println!(
                "  {:<30}  {:<24}  {}",
                truncate(&att.target.device, 30),
                att.flashed_at,
                att.target.model
            ),
            Err(e) => println!("  {} :: parse error: {e}", path.display()),
        }
    }
    println!();
    println!(
        "{} total. Use `aegis-boot attest show <FILE>` for full detail.",
        entries.len()
    );
    ExitCode::SUCCESS
}

/// Emit the attestation list as a stable `schema_version=1` JSON
/// document on stdout. Each entry carries the parsed attestation
/// summary plus the on-disk manifest path so downstream tooling can
/// follow the chain to `aegis-boot attest show <file>` for full detail.
fn print_attest_list_json(dir: &Path, entries: &[std::fs::DirEntry]) {
    use crate::doctor::json_escape;
    println!("{{");
    println!("  \"schema_version\": 1,");
    println!("  \"tool_version\": \"{}\",", env!("CARGO_PKG_VERSION"));
    println!(
        "  \"attestations_dir\": \"{}\",",
        json_escape(&dir.display().to_string())
    );
    println!("  \"count\": {},", entries.len());
    println!("  \"attestations\": [");
    let last = entries.len().saturating_sub(1);
    for (i, entry) in entries.iter().enumerate() {
        let path = entry.path();
        let comma = if i == last { "" } else { "," };
        match read_attestation(&path) {
            Ok(att) => {
                // Emit a subset of the full manifest — enough to drive
                // a CI/monitoring dashboard without requiring the
                // consumer to re-parse each file. Full detail is one
                // `aegis-boot attest show <path>` away.
                println!("    {{");
                println!(
                    "      \"manifest_path\": \"{}\",",
                    json_escape(&path.display().to_string())
                );
                println!("      \"schema_version\": {},", att.schema_version);
                println!(
                    "      \"tool_version\": \"{}\",",
                    json_escape(&att.tool_version)
                );
                println!(
                    "      \"flashed_at\": \"{}\",",
                    json_escape(&att.flashed_at)
                );
                println!("      \"operator\": \"{}\",", json_escape(&att.operator));
                println!(
                    "      \"target_device\": \"{}\",",
                    json_escape(&att.target.device)
                );
                println!(
                    "      \"target_model\": \"{}\",",
                    json_escape(&att.target.model)
                );
                println!(
                    "      \"disk_guid\": \"{}\",",
                    json_escape(&att.target.disk_guid)
                );
                println!("      \"iso_count\": {}", att.isos.len());
                println!("    }}{comma}");
            }
            Err(e) => {
                println!("    {{");
                println!(
                    "      \"manifest_path\": \"{}\",",
                    json_escape(&path.display().to_string())
                );
                println!(
                    "      \"error\": \"{}\"",
                    json_escape(&format!("parse failed: {e}"))
                );
                println!("    }}{comma}");
            }
        }
    }
    println!("  ]");
    println!("}}");
}

fn run_show(args: &[String]) -> ExitCode {
    // --json can appear anywhere; the positional is the FILE path.
    let json_mode = args.iter().any(|a| a == "--json");
    let Some(file_arg) = args.iter().find(|a| !a.starts_with("--")) else {
        eprintln!("aegis-boot attest show: missing <FILE> argument");
        eprintln!("run 'aegis-boot attest list' to find files");
        return ExitCode::from(2);
    };
    let path = PathBuf::from(file_arg);
    let att = match read_attestation(&path) {
        Ok(a) => a,
        Err(e) => {
            if json_mode {
                println!(
                    "{{ \"schema_version\": 1, \"error\": \"{}\" }}",
                    crate::doctor::json_escape(&e)
                );
            } else {
                eprintln!("aegis-boot attest show: {e}");
            }
            return ExitCode::from(1);
        }
    };
    if json_mode {
        // The on-disk manifest IS JSON — emit it verbatim. This gives
        // downstream consumers the full Attestation schema (including
        // every IsoRecord and host/target detail) without requiring
        // aegis-cli to re-serialize. A separate `attest list --json`
        // gives the summary; this is the full detail.
        match fs::read_to_string(&path) {
            Ok(body) => {
                // `body` already ends with a newline from serde_json
                // pretty-print; use print! to avoid doubling.
                print!("{body}");
                if !body.ends_with('\n') {
                    println!();
                }
            }
            Err(e) => {
                println!(
                    "{{ \"schema_version\": 1, \"error\": \"read {}: {e}\" }}",
                    crate::doctor::json_escape(&path.display().to_string())
                );
                return ExitCode::from(1);
            }
        }
    } else {
        print_attestation(&path, &att);
    }
    ExitCode::SUCCESS
}

fn print_attestation(path: &Path, att: &Attestation) {
    println!("Attestation: {}", path.display());
    println!("  schema:        v{}", att.schema_version);
    println!("  tool version:  {}", att.tool_version);
    println!("  flashed at:    {}", att.flashed_at);
    println!("  operator:      {}", att.operator);
    println!();
    println!("Host:");
    println!("  kernel:        {}", att.host.kernel);
    println!("  Secure Boot:   {}", att.host.secure_boot);
    println!();
    println!("Target stick:");
    println!("  device:        {}", att.target.device);
    println!("  model:         {}", att.target.model);
    println!(
        "  size:          {} ({} bytes)",
        humanize(att.target.size_bytes),
        att.target.size_bytes
    );
    println!(
        "  disk GUID:     {}",
        if att.target.disk_guid.is_empty() {
            "(not captured)"
        } else {
            &att.target.disk_guid
        }
    );
    println!(
        "  image SHA-256: {}",
        if att.target.image_sha256.is_empty() {
            "(not captured)"
        } else {
            &att.target.image_sha256
        }
    );
    println!(
        "  image size:    {} ({} bytes)",
        humanize(att.target.image_size_bytes),
        att.target.image_size_bytes
    );
    println!();
    println!("ISOs ({} added since flash):", att.isos.len());
    if att.isos.is_empty() {
        println!("  (none yet — append with `aegis-boot add` once supported in a follow-up)");
    } else {
        for iso in &att.isos {
            println!(
                "  - {} (added {}, {} bytes, sidecars: {:?})",
                iso.filename, iso.added_at, iso.size_bytes, iso.sidecars
            );
        }
    }
}

// --- helpers ---------------------------------------------------------------

fn read_attestation(path: &Path) -> Result<Attestation, String> {
    let bytes = fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("parse {}: {e}", path.display()))
}

fn data_dir() -> PathBuf {
    // When running under sudo, prefer the original user's data dir
    // rather than root's (/root/.local/share/...). Otherwise `aegis-
    // boot flash` (run via sudo for dd) would write attestations to
    // root's home, but `aegis-boot attest list` (run as the operator)
    // would look under their own home and find nothing — see the
    // companion `sudo_aware_data_dir` in update.rs::attestation_dir.
    if let Some(sudo_data) = sudo_aware_data_dir() {
        return sudo_data.join("aegis-boot");
    }
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("aegis-boot")
}

/// When the process runs under `sudo`, the kernel preserves
/// `SUDO_USER` (and usually `SUDO_UID`); HOME has already been
/// rewritten to root's. Look up `SUDO_USER`'s home from `/etc/passwd`
/// and prefer it over root's so attestations + cached state land
/// in the operator's actual home dir.
///
/// Returns `Some(<user>/.local/share)` only when:
/// - Effective UID is 0 AND
/// - `SUDO_USER` is set AND
/// - `getent passwd <user>` returns a valid home dir
///
/// Otherwise returns `None`, signaling the caller to fall back to
/// the standard XDG/HOME resolution. Pure: shells out to `getent`
/// (a glibc tool present on every system aegis-boot supports).
pub(crate) fn sudo_aware_data_dir() -> Option<PathBuf> {
    if !is_running_as_root() {
        return None;
    }
    let sudo_user = std::env::var_os("SUDO_USER")?;
    let sudo_user = sudo_user.to_str()?;
    if sudo_user == "root" {
        return None;
    }
    let home = lookup_home_via_getent(sudo_user)?;
    Some(home.join(".local/share"))
}

fn is_running_as_root() -> bool {
    // Read /proc/self/status to avoid an unsafe libc::geteuid() call.
    // `Uid:` line format: "Uid:\t<real>\t<effective>\t<saved>\t<fs>".
    // Effective UID is column 2 (0-indexed: index 2 after splitwhitespace
    // including the Uid: prefix... actually 2 if we skip "Uid:" first).
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("Uid:") {
            let mut tokens = rest.split_whitespace();
            // tokens: <real> <effective> <saved> <fs>
            tokens.next(); // real
            if let Some(euid) = tokens.next() {
                return euid == "0";
            }
        }
    }
    false
}

fn lookup_home_via_getent(user: &str) -> Option<PathBuf> {
    // getent passwd <user> output: "<user>:<x>:<uid>:<gid>:<gecos>:<home>:<shell>"
    // 6th field (index 5) is the home dir.
    let out = Command::new("getent")
        .args(["passwd", user])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let line = String::from_utf8_lossy(&out.stdout);
    let line = line.trim();
    let fields: Vec<&str> = line.split(':').collect();
    if fields.len() < 6 {
        return None;
    }
    let home = fields[5];
    if home.is_empty() {
        return None;
    }
    Some(PathBuf::from(home))
}

fn current_iso_timestamp() -> String {
    Command::new("date")
        .args(["-u", "+%Y-%m-%dT%H:%M:%SZ"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map_or_else(
            || "1970-01-01T00:00:00Z".to_string(),
            |s| s.trim().to_string(),
        )
}

fn current_operator() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

fn current_kernel() -> String {
    Command::new("uname")
        .arg("-sr")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map_or_else(|| "unknown".to_string(), |s| s.trim().to_string())
}

fn current_sb_state() -> String {
    if let Ok(out) = Command::new("mokutil").arg("--sb-state").output() {
        if out.status.success() {
            let stdout = String::from_utf8_lossy(&out.stdout).to_lowercase();
            if stdout.contains("secureboot enabled") {
                return "enforcing".to_string();
            }
            if stdout.contains("secureboot disabled") {
                return "disabled".to_string();
            }
        }
    }
    // Fallback to efivar.
    if let Ok(entries) = fs::read_dir("/sys/firmware/efi/efivars") {
        for e in entries.flatten() {
            let name = e.file_name();
            if name.to_string_lossy().starts_with("SecureBoot-") {
                if let Ok(bytes) = fs::read(e.path()) {
                    if bytes.len() >= 5 {
                        return if bytes[4] == 1 {
                            "enforcing".to_string()
                        } else {
                            "disabled".to_string()
                        };
                    }
                }
            }
        }
    }
    "unknown".to_string()
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let out = Command::new("sha256sum")
        .arg(path)
        .output()
        .map_err(|e| format!("sha256sum exec: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "sha256sum failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    // Output: "<hex>  <path>"
    Ok(stdout.split_whitespace().next().unwrap_or("").to_string())
}

fn read_disk_guid(dev: &Path) -> Option<String> {
    let out = Command::new("sudo")
        .args(["sgdisk", "-p"])
        .arg(dev)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    // sgdisk -p line: "Disk identifier (GUID): 7DD588C9-3A85-48CF-822F-BFBC4D8DD784"
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("Disk identifier (GUID): ") {
            return Some(rest.trim().to_string());
        }
    }
    None
}

fn sanitize_for_filename(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
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

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut out = s.chars().take(max - 1).collect::<String>();
        out.push('\u{2026}');
        out
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]
    use super::*;

    #[test]
    fn schema_version_is_one() {
        // Bumping this is intentional and downstream-visible; gate it.
        assert_eq!(SCHEMA_VERSION, 1);
    }

    #[test]
    fn manifest_roundtrips_through_json() {
        let m = Attestation {
            schema_version: 1,
            tool_version: "0.13.0".to_string(),
            flashed_at: "2026-04-16T12:34:56Z".to_string(),
            operator: "william".to_string(),
            host: HostInfo {
                kernel: "Linux 6.17.0".to_string(),
                secure_boot: "disabled".to_string(),
            },
            target: TargetInfo {
                device: "/dev/sdc".to_string(),
                model: "SanDisk Cruzer Blade".to_string(),
                size_bytes: 32_010_240_000,
                image_sha256: "abc123".repeat(10),
                image_size_bytes: 1_073_741_824,
                disk_guid: "7DD588C9-3A85-48CF-822F-BFBC4D8DD784".to_string(),
            },
            isos: Vec::new(),
        };
        let json = serde_json::to_string(&m).expect("serialize");
        let back: Attestation = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.tool_version, m.tool_version);
        assert_eq!(back.target.disk_guid, m.target.disk_guid);
    }

    #[test]
    fn unknown_fields_are_tolerated() {
        // Forward compat: future versions add fields; current code shouldn't
        // fail to parse if it sees an extra one.
        let with_extra = r#"{
            "schema_version": 1,
            "tool_version": "0.13.0",
            "flashed_at": "2026-04-16T12:34:56Z",
            "operator": "x",
            "host": {"kernel": "k", "secure_boot": "disabled"},
            "target": {
                "device": "/dev/sdc",
                "model": "X",
                "size_bytes": 1,
                "image_sha256": "abc",
                "image_size_bytes": 1,
                "disk_guid": "G"
            },
            "isos": [],
            "future_field": "ignored"
        }"#;
        let _: Attestation = serde_json::from_str(with_extra).expect("tolerates unknown fields");
    }

    /// Mutex serializing the env-mutating tests below. Without it,
    /// cargo's parallel test runner interleaves their `set_var` /
    /// `remove_var` calls and the assertions race — surfaced 2026-04-18
    /// in CI as `data_dir_falls_back_to_home_local_share` returning the
    /// XDG path because the sibling test had set `XDG_DATA_HOME` and
    /// not yet reverted it.
    static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn data_dir_uses_xdg_data_home_when_set() {
        let _guard = ENV_MUTEX
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let prev = std::env::var_os("XDG_DATA_HOME");
        std::env::set_var("XDG_DATA_HOME", "/tmp/aegis-test-xdg-data");
        let p = data_dir();
        assert_eq!(p, PathBuf::from("/tmp/aegis-test-xdg-data/aegis-boot"));
        match prev {
            Some(v) => std::env::set_var("XDG_DATA_HOME", v),
            None => std::env::remove_var("XDG_DATA_HOME"),
        }
    }

    #[test]
    fn data_dir_falls_back_to_home_local_share() {
        let _guard = ENV_MUTEX
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let prev_xdg = std::env::var_os("XDG_DATA_HOME");
        let prev_home = std::env::var_os("HOME");
        std::env::remove_var("XDG_DATA_HOME");
        std::env::set_var("HOME", "/tmp/aegis-test-home2");
        let p = data_dir();
        assert_eq!(
            p,
            PathBuf::from("/tmp/aegis-test-home2/.local/share/aegis-boot")
        );
        match prev_xdg {
            Some(v) => std::env::set_var("XDG_DATA_HOME", v),
            None => std::env::remove_var("XDG_DATA_HOME"),
        }
        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }

    #[test]
    fn sanitize_filename_keeps_safe_chars() {
        assert_eq!(sanitize_for_filename("/dev/sdc"), "_dev_sdc");
        assert_eq!(sanitize_for_filename("ubuntu-22.04"), "ubuntu-22_04");
        assert_eq!(sanitize_for_filename("plain"), "plain");
    }

    #[test]
    fn humanize_bytes() {
        assert_eq!(humanize(0), "0 MiB");
        assert_eq!(humanize(1024 * 1024), "1 MiB");
        assert_eq!(humanize(2 * 1024 * 1024 * 1024), "2.0 GiB");
    }

    #[test]
    fn truncate_does_not_panic_on_short_input() {
        assert_eq!(truncate("abc", 10), "abc");
        let long = truncate("0123456789abcdef", 6);
        assert_eq!(long.chars().count(), 6);
        assert!(long.ends_with('\u{2026}'));
    }

    // disk_for_partition tests moved to mounts.rs (the canonical
    // home); kept the smoke-test for the attest-specific call path
    // via the summary_for_mount → device_for_mount → parent_disk chain.

    #[test]
    fn iso_record_appends_to_isos_vec() {
        // Roundtrip with an ISO record to confirm the append shape.
        let mut m = Attestation {
            schema_version: 1,
            tool_version: "0.13.0".to_string(),
            flashed_at: "2026-04-16T12:34:56Z".to_string(),
            operator: "x".to_string(),
            host: HostInfo {
                kernel: "k".to_string(),
                secure_boot: "disabled".to_string(),
            },
            target: TargetInfo {
                device: "/dev/sdc".to_string(),
                model: "M".to_string(),
                size_bytes: 1,
                image_sha256: "h".to_string(),
                image_size_bytes: 1,
                disk_guid: "g".to_string(),
            },
            isos: vec![],
        };
        m.isos.push(IsoRecord {
            filename: "ubuntu.iso".to_string(),
            sha256: "deadbeef".to_string(),
            size_bytes: 100,
            sidecars: vec!["sha256".to_string(), "minisig".to_string()],
            added_at: "2026-04-16T13:00:00Z".to_string(),
        });
        let json = serde_json::to_string(&m).expect("serialize");
        let back: Attestation = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.isos.len(), 1);
        assert_eq!(back.isos[0].sidecars, vec!["sha256", "minisig"]);
    }
}
