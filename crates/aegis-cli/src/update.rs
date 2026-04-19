//! `aegis-boot update [device]` — eligibility check for in-place
//! signed-chain rotation.
//!
//! # Phase 1 (this module): read-only verification
//!
//! Validates that a target USB stick can be updated in-place without
//! re-flashing. No writes.
//!
//! Eligibility criteria, in order (first-fail wins):
//!   1. Device path exists and is a block device
//!   2. Has a GPT partition table (sgdisk -p)
//!   3. Partition 1 exists (would be the ESP)
//!   4. Partition 2 exists with a filesystem label `AEGIS_ISOS` or
//!      `AEGIS-ISOS` (case-insensitive; lsblk reports labels
//!      normalized by the filesystem driver)
//!   5. An attestation manifest exists whose `disk_guid` matches the
//!      target's GPT disk GUID from `sgdisk -p`
//!
//! When all five pass, the stick is "eligible" — a future phase will
//! actually perform the update. Right now we print a clear "eligible"
//! message with the matched attestation path so the operator can
//! verify ownership and time of flash.
//!
//! # What's deliberately NOT in this phase
//!
//! - Building a fresh ESP image to diff against (deferred to phase 1.5
//!   — requires either calling mkusb.sh or a Rust equivalent, and the
//!   diff is useful but not blocking for the safety story)
//! - Any write to the device (phase 2 — atomic file replace with
//!   backup)
//! - CA signature verification on the new chain (phase 3)
//!
//! Tracked under epic [#181](https://github.com/williamzujkowski/aegis-boot/issues/181).

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// Entry point for `aegis-boot update [device]`.
pub fn run(args: &[String]) -> ExitCode {
    match try_run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => ExitCode::from(code),
    }
}

/// Inner runner returning a typed result. Same contract as `run`.
pub(crate) fn try_run(args: &[String]) -> Result<(), u8> {
    let mut explicit_dev: Option<&str> = None;
    let mut json_mode = false;
    for a in args {
        match a.as_str() {
            "--help" | "-h" => {
                print_help();
                return Ok(());
            }
            "--json" => json_mode = true,
            arg if arg.starts_with("--") => {
                eprintln!("aegis-boot update: unknown option '{arg}'");
                eprintln!("(in-place update is under active development — only the");
                eprintln!(" eligibility check is shipped today; see issue #181)");
                return Err(2);
            }
            other => {
                if explicit_dev.is_some() {
                    eprintln!("aegis-boot update: only one device allowed");
                    return Err(2);
                }
                explicit_dev = Some(other);
            }
        }
    }

    let Some(d) = explicit_dev else {
        if json_mode {
            println!("{{ \"schema_version\": 1, \"error\": \"missing <device> argument\" }}");
        } else {
            eprintln!("aegis-boot update: missing <device> argument");
            eprintln!("usage: aegis-boot update /dev/sdX");
        }
        return Err(2);
    };
    let dev = PathBuf::from(d);

    if !json_mode {
        println!("aegis-boot update — eligibility check");
        println!();
        println!("Target device: {}", dev.display());
        println!();
    }

    match check_eligibility(&dev) {
        Eligibility::Eligible {
            attestation_path,
            disk_guid,
        } => {
            let chain = resolve_host_chain();
            if json_mode {
                print_update_json_eligible(&dev, &disk_guid, &attestation_path, &chain);
            } else {
                println!("Status: ELIGIBLE for in-place update.");
                println!();
                println!("  disk GUID:        {disk_guid}");
                println!("  attestation:      {}", attestation_path.display());
                println!("  AEGIS_ISOS:       will be preserved byte-for-byte");
                println!();
                // Phase 1.5 of #181: show the operator the host-side signed
                // chain that a future `aegis-boot update` would install.
                print_host_chain(&chain);
                println!();
                println!("NOTE: this is a read-only eligibility check (phase 1 of #181).");
                println!("The actual in-place update lands in a follow-up PR. No writes");
                println!("were made to {} during this command.", dev.display());
            }
            Ok(())
        }
        Eligibility::Ineligible(reason) => {
            if json_mode {
                print_update_json_ineligible(&dev, &reason);
            } else {
                let err = UpdateError::Ineligible {
                    reason,
                    device: dev.clone(),
                };
                eprint!("{}", crate::userfacing::render_string(&err));
            }
            Err(1)
        }
    }
}

/// Emit the eligible-case JSON envelope. `schema_version=1`; additive
/// only. The `host_chain` entries mirror the human-readable output but
/// carry full 64-char sha256 (no truncation) and an explicit `error`
/// field per slot when the file couldn't be hashed.
fn print_update_json_eligible(
    dev: &Path,
    disk_guid: &str,
    attestation_path: &Path,
    chain: &[HostChainEntry],
) {
    use crate::doctor::json_escape;
    println!("{{");
    println!("  \"schema_version\": 1,");
    println!("  \"tool_version\": \"{}\",", env!("CARGO_PKG_VERSION"));
    println!(
        "  \"device\": \"{}\",",
        json_escape(&dev.display().to_string())
    );
    println!("  \"eligibility\": \"ELIGIBLE\",");
    println!("  \"disk_guid\": \"{}\",", json_escape(disk_guid));
    println!(
        "  \"attestation_path\": \"{}\",",
        json_escape(&attestation_path.display().to_string())
    );
    println!("  \"host_chain\": [");
    let last = chain.len().saturating_sub(1);
    for (i, entry) in chain.iter().enumerate() {
        let comma = if i == last { "" } else { "," };
        let path_str = entry.path.display().to_string();
        match &entry.sha256 {
            Ok(hash) => println!(
                "    {{ \"role\": \"{}\", \"path\": \"{}\", \"sha256\": \"{}\" }}{comma}",
                entry.role,
                json_escape(&path_str),
                hash,
            ),
            Err(reason) => println!(
                "    {{ \"role\": \"{}\", \"path\": \"{}\", \"error\": \"{}\" }}{comma}",
                entry.role,
                json_escape(&path_str),
                json_escape(reason),
            ),
        }
    }
    println!("  ]");
    println!("}}");
}

fn print_update_json_ineligible(dev: &Path, reason: &str) {
    use crate::doctor::json_escape;
    println!("{{");
    println!("  \"schema_version\": 1,");
    println!("  \"tool_version\": \"{}\",", env!("CARGO_PKG_VERSION"));
    println!(
        "  \"device\": \"{}\",",
        json_escape(&dev.display().to_string())
    );
    println!("  \"eligibility\": \"INELIGIBLE\",");
    println!("  \"reason\": \"{}\"", json_escape(reason));
    println!("}}");
}

/// Print the host-side signed chain — the shim/grub/kernel/initrd
/// files mkusb.sh would install if the operator re-ran the flash
/// today. sha256 each so the operator has concrete bytes to compare
/// against (phase 2 will add stick-side hashing + diff; for now this
/// is a one-sided preview).
///
/// Failures to locate / hash a specific file are surfaced inline
/// (not fatal) — the operator can still see which files are missing.
/// This makes the "kernel not on PATH" case actionable: "shim: OK,
/// grub: OK, kernel: MISSING at /boot/vmlinuz-*-virtual".
fn print_host_chain(chain: &[HostChainEntry]) {
    println!("Host-side signed chain (what update would install):");
    for entry in chain {
        match &entry.sha256 {
            Ok(hash) => {
                let short = &hash[..hash.len().min(16)];
                println!(
                    "  {:<8} {}  sha256:{}…",
                    entry.role,
                    entry.path.display(),
                    short
                );
            }
            Err(reason) => {
                println!(
                    "  {:<8} {}  (unavailable: {reason})",
                    entry.role,
                    entry.path.display()
                );
            }
        }
    }
}

/// One resolved signed-chain slot — mirrors the `SHIM_SRC` / `GRUB_SRC` /
/// `KERNEL_SRC` / `INITRD_SRC` triple in `mkusb.sh`. `sha256` is the result
/// of the hash attempt: `Err` carries a human-readable reason when the
/// file couldn't be resolved or hashed.
struct HostChainEntry {
    role: &'static str,
    path: PathBuf,
    sha256: Result<String, String>,
}

/// Replicate `mkusb.sh`'s host-chain resolution in Rust. Looks at the
/// defaults used by `mkusb.sh`:
///   `SHIM_SRC=/usr/lib/shim/shimx64.efi.signed`
///   `GRUB_SRC=/usr/lib/grub/x86_64-efi-signed/grubx64.efi.signed`
///   `KERNEL_SRC`: first readable `/boot/vmlinuz-*-virtual` or `-generic`
///   `INITRD_SRC`: `/boot/initrd.img-<same suffix as kernel>`
///
/// Env overrides aren't honored here — this is an *informational*
/// preview against `mkusb.sh`'s defaults. An operator who overrides
/// those env vars will know to re-do the math manually.
fn resolve_host_chain() -> Vec<HostChainEntry> {
    let mut out = Vec::with_capacity(4);
    out.push(resolve_one(
        "shim",
        PathBuf::from("/usr/lib/shim/shimx64.efi.signed"),
    ));
    out.push(resolve_one(
        "grub",
        PathBuf::from("/usr/lib/grub/x86_64-efi-signed/grubx64.efi.signed"),
    ));
    let (kernel_path, kernel_ver) = find_kernel();
    out.push(resolve_one("kernel", kernel_path.clone()));
    let initrd_path = match kernel_ver {
        Some(v) => PathBuf::from(format!("/boot/initrd.img-{v}")),
        None => PathBuf::from("/boot/initrd.img-*"),
    };
    out.push(resolve_one("initrd", initrd_path));
    out
}

/// Find the first readable `vmlinuz-*-{virtual,generic}` in /boot,
/// matching mkusb.sh's iteration order. Returns the kernel path and
/// its version suffix (stripped of the `vmlinuz-` prefix) so we can
/// construct the matching initrd path.
fn find_kernel() -> (PathBuf, Option<String>) {
    for glob_suffix in ["-virtual", "-generic"] {
        if let Ok(entries) = std::fs::read_dir("/boot") {
            for entry in entries.flatten() {
                let path = entry.path();
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if name.starts_with("vmlinuz-") && name.ends_with(glob_suffix) {
                    let ver = name.trim_start_matches("vmlinuz-").to_string();
                    if std::fs::File::open(&path).is_ok() {
                        return (path, Some(ver));
                    }
                }
            }
        }
    }
    (PathBuf::from("/boot/vmlinuz-*-{virtual,generic}"), None)
}

fn resolve_one(role: &'static str, path: PathBuf) -> HostChainEntry {
    let sha256 = if path.is_file() {
        sha256_file(&path)
    } else {
        Err("not found or not readable".to_string())
    };
    HostChainEntry { role, path, sha256 }
}

/// Shell out to `sha256sum` rather than pulling in the `sha2` crate —
/// keeps the static-musl binary small and matches the doctor check
/// that already verifies sha256sum is on PATH.
fn sha256_file(path: &Path) -> Result<String, String> {
    let output = Command::new("sha256sum")
        .arg(path)
        .output()
        .map_err(|e| format!("sha256sum exec failed: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "sha256sum exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Output format: "<64 hex>  <path>\n"
    stdout
        .split_whitespace()
        .next()
        .filter(|s| s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit()))
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| format!("sha256sum output malformed: {stdout:?}"))
}

fn print_help() {
    println!("aegis-boot update — in-place signed-chain update (read-only check for now)");
    println!();
    println!("USAGE:");
    println!("  aegis-boot update <device>");
    println!("  aegis-boot update --help");
    println!();
    println!("BEHAVIOR (phase 1 of #181):");
    println!("  Validates that the target stick is a known aegis-boot stick and");
    println!("  that its attestation manifest matches the disk GUID. Reports");
    println!("  ELIGIBLE / NOT ELIGIBLE with a specific reason. Does NOT write.");
    println!();
    println!("  The actual atomic in-place update lands in follow-up PRs — see");
    println!("  issue #181 for the phased plan.");
    println!();
    println!("WHY YOU'D USE THIS (once full update ships):");
    println!("  - Apply shim/GRUB/kernel CVE fixes without wiping AEGIS_ISOS");
    println!("  - Rotate signing keys on the boot chain");
    println!("  - Bump aegis-boot releases without losing your ISO inventory");
}

/// Outcome of the pre-flight check. `Ineligible` carries an operator-
/// readable reason the TUI can surface verbatim.
#[derive(Debug)]
pub(crate) enum Eligibility {
    Eligible {
        attestation_path: PathBuf,
        disk_guid: String,
    },
    Ineligible(String),
}

/// Operator-visible errors from `aegis-boot update`. Implemented as a
/// `UserFacing` error so the structured renderer produces the "try one
/// of:" numbered list instead of the ad-hoc `eprintln!` block the
/// command used before #247 PR4.
#[derive(Debug)]
pub(crate) enum UpdateError {
    /// The target stick failed one of the five eligibility gates.
    /// `reason` is the operator-readable sentence from
    /// `check_eligibility`; `device` is echoed back into the second
    /// suggestion so operators can copy-paste the `aegis-boot init`
    /// line without substituting the path themselves.
    Ineligible { reason: String, device: PathBuf },
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ineligible { reason, .. } => {
                write!(f, "not eligible for in-place update: {reason}")
            }
        }
    }
}

impl std::error::Error for UpdateError {}

impl crate::userfacing::UserFacing for UpdateError {
    fn summary(&self) -> &str {
        match self {
            Self::Ineligible { .. } => "stick not eligible for in-place update",
        }
    }

    fn detail(&self) -> &str {
        match self {
            Self::Ineligible { reason, .. } => reason,
        }
    }

    fn suggestions(&self) -> Vec<String> {
        match self {
            Self::Ineligible { device, .. } => vec![
                "If this is a genuine aegis-boot stick but lacks an attestation, it was \
                 flashed before v0.13.0. Re-flash with `aegis-boot flash` to create a new \
                 attestation; your ISOs will be lost, so back them up first."
                    .to_string(),
                format!(
                    "If this is a fresh / non-aegis-boot USB stick, run `aegis-boot init {}` \
                     to initialize it.",
                    device.display()
                ),
            ],
        }
    }

    fn code(&self) -> Option<&str> {
        Some("UPDATE_INELIGIBLE")
    }
}

/// Run all five eligibility gates against the given device. Returns the
/// matched attestation + GUID on success; a specific human-readable
/// reason on failure.
pub(crate) fn check_eligibility(dev: &Path) -> Eligibility {
    if !dev.exists() {
        return Eligibility::Ineligible(format!(
            "device {} does not exist (unplugged? wrong path?)",
            dev.display()
        ));
    }

    // Gate 1: GPT partition table.
    let sgdisk = match Command::new("sgdisk").args(["-p"]).arg(dev).output() {
        Ok(o) if o.status.success() => o,
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            // Sniff for the permission-denied case. sgdisk surfaces it
            // as "Problem opening /dev/sdX for reading! Error is 13."
            // (errno 13 = EACCES) plus "You must run this program as
            // root or use sudo!". Detecting this lets us tell the
            // operator to retry with sudo instead of leaving them
            // confused that their stick is "NOT ELIGIBLE".
            if stderr.contains("must run this program as root")
                || stderr.contains("Error is 13")
                || stderr.contains("Permission denied")
            {
                return Eligibility::Ineligible(format!(
                    "permission denied reading {} (need root for raw block-device read). \
                     Re-run with sudo: `sudo aegis-boot update {}`.",
                    dev.display(),
                    dev.display()
                ));
            }
            return Eligibility::Ineligible(format!(
                "`sgdisk -p {}` exited non-zero: {}",
                dev.display(),
                stderr.trim()
            ));
        }
        Err(e) => {
            return Eligibility::Ineligible(format!(
                "cannot run sgdisk: {e} (is gptfdisk installed?)"
            ));
        }
    };
    let sgdisk_out = String::from_utf8_lossy(&sgdisk.stdout);

    // Gate 2: extract disk GUID. sgdisk emits "Disk identifier (GUID): XXXX-..."
    let Some(disk_guid) = parse_disk_guid(&sgdisk_out) else {
        return Eligibility::Ineligible(
            "sgdisk did not report a disk GUID (not GPT? corrupted?)".to_string(),
        );
    };

    // Gate 3: partition 1 + 2 present. Trust sgdisk's line format:
    //   "   1     2048      820207   400.0 MiB   EF00  EFI System"
    //   "   2   822256    31277055   14.5  GiB   8300  AEGIS_ISOS"
    let (has_esp, part2_label) = parse_partitions(&sgdisk_out);
    if !has_esp {
        return Eligibility::Ineligible(
            "partition 1 missing or not an ESP (type EF00)".to_string(),
        );
    }
    if !part2_label.eq_ignore_ascii_case("AEGIS_ISOS")
        && !part2_label.eq_ignore_ascii_case("AEGIS-ISOS")
    {
        return Eligibility::Ineligible(format!(
            "partition 2 label is {part2_label:?} — expected AEGIS_ISOS. \
             This stick was not flashed by aegis-boot."
        ));
    }

    // Gate 4: locate matching attestation by disk GUID.
    let Some(attestation_path) = find_attestation_by_guid(&disk_guid) else {
        return Eligibility::Ineligible(format!(
            "no attestation manifest found for disk GUID {disk_guid}. \
             Was this stick flashed on a different host, or before v0.13.0?"
        ));
    };

    Eligibility::Eligible {
        attestation_path,
        disk_guid,
    }
}

/// Extract the disk GUID from `sgdisk -p` output. Line is:
///   `Disk identifier (GUID): DDDDDDDD-DDDD-DDDD-DDDD-DDDDDDDDDDDD`
/// (lowercase hex with dashes, per GPT spec).
pub(crate) fn parse_disk_guid(sgdisk_out: &str) -> Option<String> {
    for line in sgdisk_out.lines() {
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix("Disk identifier (GUID):") else {
            continue;
        };
        let guid = rest.trim().to_ascii_lowercase();
        // Sanity: GPT GUIDs are 36 chars (32 hex + 4 dashes).
        if guid.len() == 36 && guid.matches('-').count() == 4 {
            return Some(guid);
        }
    }
    None
}

/// Parse `sgdisk`'s partition table. Returns (`has_part1_ef00`, `part2_name`).
///
/// Note: sgdisk abbreviates GUID partition types to 4-char codes
/// (`EF00` for EFI System, `8300` for Linux filesystem, etc). The
/// partition name is the free-text label set by `-c N:LABEL`, NOT the
/// filesystem label. We set it to `AEGIS_ISOS` during `mkusb.sh`.
pub(crate) fn parse_partitions(sgdisk_out: &str) -> (bool, String) {
    let mut has_esp = false;
    let mut part2_label = String::new();
    // Find the "Number" header line, then parse fixed-ish columns.
    let mut in_table = false;
    for line in sgdisk_out.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("Number ") && trimmed.contains("Code") {
            in_table = true;
            continue;
        }
        if !in_table || trimmed.is_empty() {
            continue;
        }
        // Split on whitespace; first token is the partition number.
        let mut tokens: Vec<&str> = trimmed.split_whitespace().collect();
        if tokens.len() < 7 {
            continue;
        }
        let num = tokens[0];
        // Find the "Code" column — it's a 4-char hex code like EF00.
        // sgdisk's format has it at position 5 (after Number, Start,
        // End, Size-value, Size-unit).
        let code = tokens[5];
        // Everything after position 5 is the name (can contain spaces).
        let name = tokens.split_off(6).join(" ");
        if num == "1" && code == "EF00" {
            has_esp = true;
        }
        if num == "2" {
            part2_label = name;
        }
    }
    (has_esp, part2_label)
}

/// Walk the attestations dir and return the path of the first manifest
/// whose `disk_guid` field matches `target_guid` (case-insensitive).
/// Returns `None` if no match is found OR the attestations dir doesn't
/// exist yet.
fn find_attestation_by_guid(target_guid: &str) -> Option<PathBuf> {
    let dir = attestation_dir()?;
    let entries = std::fs::read_dir(&dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        // We deliberately don't deserialize the whole Attestation here —
        // this keeps `update` decoupled from attest.rs's internal schema
        // if it evolves. Match on the raw JSON field instead.
        if body_contains_guid(&body, target_guid) {
            return Some(path);
        }
    }
    None
}

/// Matches `"disk_guid": "XXXX"` in a JSON body, case-insensitive on
/// the GUID value. Pure-string so it's fast and doesn't require a
/// JSON dep. Anchors on the closing `"` so that a short target GUID
/// can't false-match a prefix of a longer one (e.g. target `abcd`
/// matching stored `abcdef01-...` would be wrong).
pub(crate) fn body_contains_guid(body: &str, target_guid: &str) -> bool {
    let lower_body = body.to_ascii_lowercase();
    let needle = format!("\"disk_guid\": \"{}\"", target_guid.to_ascii_lowercase());
    lower_body.contains(&needle)
}

/// Same resolution rules as `attest::attestation_dir()` — duplicated here
/// to avoid exporting a new pub surface in attest.rs this PR. Kept
/// `pub(crate)` so a future `mv` into attest.rs is a mechanical rename.
///
/// Sudo handling: when running as root with `SUDO_USER` set (the
/// typical `sudo aegis-boot update` flow — sudo needed to read the
/// raw block device), prefer the original user's data dir rather
/// than root's. Otherwise update-via-sudo would never find the
/// attestations that flash-via-sudo wrote under the same user's
/// home — that's the exact bug this fix closes (caught 2026-04-18
/// during real-stick testing of #181).
pub(crate) fn attestation_dir() -> Option<PathBuf> {
    if let Some(sudo_data) = crate::attest::sudo_aware_data_dir() {
        return Some(sudo_data.join("aegis-boot").join("attestations"));
    }
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(base.join("aegis-boot").join("attestations"))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::missing_panics_doc)]
mod tests {
    use super::*;

    #[test]
    fn parse_disk_guid_extracts_valid_guid() {
        let out = "\
            Disk /dev/sdc: 30228480 sectors, 14.4 GiB\n\
            Sector size (logical/physical): 512/512 bytes\n\
            Disk identifier (GUID): abcdef01-2345-6789-abcd-ef0123456789\n\
            Partition table holds up to 128 entries\n\
        ";
        assert_eq!(
            parse_disk_guid(out).as_deref(),
            Some("abcdef01-2345-6789-abcd-ef0123456789"),
        );
    }

    #[test]
    fn parse_disk_guid_uppercase_normalized_to_lower() {
        let out = "Disk identifier (GUID): ABCDEF01-2345-6789-ABCD-EF0123456789";
        assert_eq!(
            parse_disk_guid(out).as_deref(),
            Some("abcdef01-2345-6789-abcd-ef0123456789"),
        );
    }

    #[test]
    fn parse_disk_guid_rejects_malformed() {
        assert!(parse_disk_guid("Disk identifier (GUID): not-a-guid").is_none());
        assert!(parse_disk_guid("no guid line here").is_none());
        assert!(parse_disk_guid("Disk identifier (GUID): ").is_none());
    }

    #[test]
    fn parse_partitions_detects_esp_and_label() {
        let out = "\
            Number  Start (sector)    End (sector)  Size       Code  Name\n   \
               1            2048          821247   400.0 MiB   EF00  EFI System\n   \
               2          821248        31277055   14.5 GiB    8300  AEGIS_ISOS\n\
        ";
        let (has_esp, label) = parse_partitions(out);
        assert!(has_esp, "missed ESP detection in: {out}");
        assert_eq!(label, "AEGIS_ISOS");
    }

    #[test]
    fn parse_partitions_no_esp_when_type_wrong() {
        let out = "\
            Number  Start (sector)    End (sector)  Size       Code  Name\n   \
               1            2048          821247   400.0 MiB   8300  Random\n   \
               2          821248        31277055   14.5 GiB    8300  AEGIS_ISOS\n\
        ";
        let (has_esp, _) = parse_partitions(out);
        assert!(!has_esp);
    }

    #[test]
    fn parse_partitions_reports_empty_label_when_part2_missing() {
        let out = "\
            Number  Start (sector)    End (sector)  Size       Code  Name\n   \
               1            2048          821247   400.0 MiB   EF00  EFI System\n\
        ";
        let (has_esp, label) = parse_partitions(out);
        assert!(has_esp);
        assert_eq!(label, "");
    }

    #[test]
    fn body_contains_guid_case_insensitive() {
        let body = r#"{"disk_guid": "ABCDEF01-2345-6789-ABCD-EF0123456789", "other": "x"}"#;
        assert!(body_contains_guid(
            body,
            "abcdef01-2345-6789-abcd-ef0123456789"
        ));
        assert!(body_contains_guid(
            body,
            "ABCDEF01-2345-6789-ABCD-EF0123456789"
        ));
    }

    #[test]
    fn body_contains_guid_prefix_match_rejected() {
        // Defensive: matching "abc" inside "abcd-..." would be a bug.
        // Our impl anchors on the full GUID inside quotes, so a
        // shorter target should not match.
        let body = r#"{"disk_guid": "abcdef01-2345-6789-abcd-ef0123456789"}"#;
        assert!(!body_contains_guid(body, "abcdef01"));
    }

    #[test]
    fn body_contains_guid_misses_different_guid() {
        let body = r#"{"disk_guid": "11111111-2222-3333-4444-555555555555"}"#;
        assert!(!body_contains_guid(
            body,
            "00000000-0000-0000-0000-000000000000"
        ));
    }

    #[test]
    fn update_error_ineligible_renders_structured_block_with_numbered_options() {
        use crate::userfacing::{render_string, UserFacing};
        let err = UpdateError::Ineligible {
            reason: "partition 2 label is \"\" — expected AEGIS_ISOS. \
                     This stick was not flashed by aegis-boot."
                .to_string(),
            device: PathBuf::from("/dev/sdc"),
        };
        // Code surfaces in the header so tooling can key on it.
        assert_eq!(err.code(), Some("UPDATE_INELIGIBLE"));
        let s = render_string(&err);
        assert!(
            s.starts_with("error[UPDATE_INELIGIBLE]: stick not eligible for in-place update"),
            "header mismatch: {s}",
        );
        assert!(
            s.contains("what happened: partition 2 label"),
            "detail missing: {s}",
        );
        // suggestions() numbered list, not the old "try: <single line>".
        assert!(s.contains("  try one of:"), "expected numbered list: {s}");
        assert!(
            s.contains("    1. If this is a genuine aegis-boot stick"),
            "option 1 missing: {s}",
        );
        // Option 2 interpolates the device path the operator just
        // typed — proof the `Vec<String>` signature (owned strings,
        // not `&[&str]`) carries dynamic data.
        assert!(
            s.contains("    2. If this is a fresh / non-aegis-boot USB stick, run `aegis-boot init /dev/sdc`"),
            "option 2 missing or missing device: {s}",
        );
    }

    #[test]
    fn update_error_ineligible_display_includes_reason() {
        // Display is required by std::error::Error; keep it useful for
        // callers that log the error directly (tests, panics, etc).
        let err = UpdateError::Ineligible {
            reason: "no attestation manifest found for disk GUID deadbeef".to_string(),
            device: PathBuf::from("/dev/sdc"),
        };
        let display = format!("{err}");
        assert!(
            display.contains("not eligible for in-place update"),
            "{display}"
        );
        assert!(display.contains("no attestation manifest"), "{display}");
    }

    #[test]
    fn check_eligibility_missing_device_is_specific() {
        let fake = PathBuf::from("/dev/this-device-does-not-exist-aegis-boot");
        let result = check_eligibility(&fake);
        match result {
            Eligibility::Ineligible(reason) => {
                assert!(
                    reason.contains("does not exist"),
                    "reason should name the missing-device case: {reason}",
                );
            }
            Eligibility::Eligible { .. } => panic!("expected Ineligible for missing device"),
        }
    }
}
