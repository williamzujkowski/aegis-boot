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
        format!("unknown-{}", sanitize_for_filename(&drive.dev.display().to_string()))
    } else {
        disk_guid.to_lowercase()
    };
    let ts_for_filename = timestamp.replace(':', "-");
    let dest = dest_dir.join(format!("{id}-{ts_for_filename}.json"));

    let json = serde_json::to_string_pretty(&manifest)
        .map_err(|e| format!("serialize manifest: {e}"))?;
    fs::write(&dest, json).map_err(|e| format!("write {}: {e}", dest.display()))?;

    Ok(dest)
}

/// Entry point for `aegis-boot attest [list|show <file>] [--help]`.
pub fn run(args: &[String]) -> ExitCode {
    let sub = args.first().map(String::as_str);
    match sub {
        None | Some("--help" | "-h" | "help") => {
            print_help();
            ExitCode::SUCCESS
        }
        Some("list") => run_list(),
        Some("show") => run_show(&args[1..]),
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
    println!("  aegis-boot attest list           List all stored attestations");
    println!("  aegis-boot attest show <FILE>    Pretty-print one attestation");
    println!("  aegis-boot attest --help         This message");
    println!();
    println!("EXAMPLES:");
    println!("  aegis-boot attest list");
    println!("  aegis-boot attest show ~/.local/share/aegis-boot/attestations/abc123-2026-04-16T12-34-56Z.json");
}

fn run_list() -> ExitCode {
    let dir = data_dir().join("attestations");
    if !dir.is_dir() {
        println!("(no attestations recorded yet — run `aegis-boot flash` to create one)");
        return ExitCode::SUCCESS;
    }
    let mut entries: Vec<_> = match fs::read_dir(&dir) {
        Ok(it) => it
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .collect(),
        Err(e) => {
            eprintln!("aegis-boot attest list: read_dir {}: {e}", dir.display());
            return ExitCode::from(1);
        }
    };
    entries.sort_by_key(std::fs::DirEntry::path);
    if entries.is_empty() {
        println!("(no attestations in {})", dir.display());
        return ExitCode::SUCCESS;
    }
    println!("Attestations in {}:", dir.display());
    println!();
    println!("  {:<30}  {:<24}  TARGET MODEL", "DEVICE", "FLASHED");
    println!("  {:<30}  {:<24}  ------------", "-".repeat(30), "-".repeat(24));
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
    println!("{} total. Use `aegis-boot attest show <FILE>` for full detail.", entries.len());
    ExitCode::SUCCESS
}

fn run_show(args: &[String]) -> ExitCode {
    let Some(file_arg) = args.first() else {
        eprintln!("aegis-boot attest show: missing <FILE> argument");
        eprintln!("run 'aegis-boot attest list' to find files");
        return ExitCode::from(2);
    };
    let path = PathBuf::from(file_arg);
    let att = match read_attestation(&path) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("aegis-boot attest show: {e}");
            return ExitCode::from(1);
        }
    };
    print_attestation(&path, &att);
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
    println!("  size:          {} ({} bytes)", humanize(att.target.size_bytes), att.target.size_bytes);
    println!("  disk GUID:     {}", if att.target.disk_guid.is_empty() { "(not captured)" } else { &att.target.disk_guid });
    println!("  image SHA-256: {}", if att.target.image_sha256.is_empty() { "(not captured)" } else { &att.target.image_sha256 });
    println!("  image size:    {} ({} bytes)", humanize(att.target.image_size_bytes), att.target.image_size_bytes);
    println!();
    println!("ISOs ({} added since flash):", att.isos.len());
    if att.isos.is_empty() {
        println!("  (none yet — append with `aegis-boot add` once supported in a follow-up)");
    } else {
        for iso in &att.isos {
            println!("  - {} (added {}, {} bytes, sidecars: {:?})",
                iso.filename, iso.added_at, iso.size_bytes, iso.sidecars);
        }
    }
}

// --- helpers ---------------------------------------------------------------

fn read_attestation(path: &Path) -> Result<Attestation, String> {
    let bytes = fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("parse {}: {e}", path.display()))
}

fn data_dir() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("aegis-boot")
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
                        return if bytes[4] == 1 { "enforcing".to_string() } else { "disabled".to_string() };
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
        return Err(format!("sha256sum failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
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
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
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

    #[test]
    fn data_dir_uses_xdg_data_home_when_set() {
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
        let prev_xdg = std::env::var_os("XDG_DATA_HOME");
        let prev_home = std::env::var_os("HOME");
        std::env::remove_var("XDG_DATA_HOME");
        std::env::set_var("HOME", "/tmp/aegis-test-home2");
        let p = data_dir();
        assert_eq!(p, PathBuf::from("/tmp/aegis-test-home2/.local/share/aegis-boot"));
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
}
