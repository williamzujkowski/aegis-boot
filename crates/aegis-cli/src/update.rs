// SPDX-License-Identifier: MIT OR Apache-2.0

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
//! # ESP diff (acceptance criterion from #181 Phase 1)
//!
//! When the five eligibility gates pass, we additionally compute
//! the per-file sha256 diff between the stick's current ESP (via
//! `mtype` piped into `sha256sum`) and the host-side signed-chain
//! sources that `mkusb.sh` / direct-install would write today.
//!
//! The diff is **informational** — it does not gate eligibility.
//! A partial read (e.g. `mtype` not installed, one file missing,
//! permission denied) surfaces as an error row in the diff so the
//! operator sees exactly which comparisons were inconclusive.
//!
//! # What's deliberately NOT in this phase
//!
//! - Any write to the device (phase 2 — atomic file replace with
//!   backup)
//! - CA signature verification on the new chain (phase 3)
//! - A full pre-rendered grub.cfg to hash against (direct-install
//!   builds grub.cfg in-process via `build_grub_cfg_body`; Phase 2
//!   will wire that into the fresh-side so grub.cfg gets a real
//!   diff row — today it surfaces as `UNKNOWN` with an explicit
//!   Phase-2 pointer)
//!
//! Tracked under epic [#181](https://github.com/aegis-boot/aegis-boot/issues/181).

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
    let mut apply_mode = false;
    let mut experimental_apply = false;
    for a in args {
        match a.as_str() {
            "--help" | "-h" => {
                print_help();
                return Ok(());
            }
            "--json" => json_mode = true,
            // #181 Phase 2a — double-flag to opt into the rotation
            // planner surface. `--apply` alone prints guidance and
            // refuses; `--experimental-apply` must also be present.
            // Phase 2a's apply path is planner-only (no writes); the
            // executor ships in Phase 2b once OVMF E2E validates.
            "--apply" => apply_mode = true,
            "--experimental-apply" => experimental_apply = true,
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

    if apply_mode && !experimental_apply {
        eprintln!("aegis-boot update --apply: requires --experimental-apply");
        eprintln!();
        eprintln!("  `--apply` is gated behind `--experimental-apply` until the");
        eprintln!("  executor ships in Phase 2b (#181). Phase 2a (this build)");
        eprintln!("  ships the planner only — no writes occur. Add");
        eprintln!("  `--experimental-apply` to see the rotation plan.");
        return Err(2);
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
        } => handle_eligible(
            &dev,
            &attestation_path,
            &disk_guid,
            json_mode,
            apply_mode && experimental_apply,
        ),
        Eligibility::Ineligible(reason) => handle_ineligible(&dev, reason, json_mode),
    }
}

/// Eligible-branch renderer split out so [`try_run`] stays under the
/// 100-line soft cap. Computes the Phase-1 ESP diff, renders the
/// eligibility block (JSON or human), and — when Phase 2a's
/// double-flag is set — follows with the rotation plan preview.
/// Return type mirrors [`handle_ineligible`] so both branches slot
/// into [`try_run`]'s `match` uniformly; this path is always `Ok`.
#[allow(clippy::unnecessary_wraps)]
fn handle_eligible(
    dev: &Path,
    attestation_path: &Path,
    disk_guid: &str,
    json_mode: bool,
    apply_plan: bool,
) -> Result<(), u8> {
    let chain = resolve_host_chain();
    let esp_part = partition_path(dev, 1);
    let diff = build_esp_diff(&esp_part, &chain);
    if json_mode {
        print_update_json_eligible(dev, disk_guid, attestation_path, &chain, &diff);
        return Ok(());
    }
    println!("Status: ELIGIBLE for in-place update.");
    println!();
    println!("  disk GUID:        {disk_guid}");
    println!("  attestation:      {}", attestation_path.display());
    println!("  AEGIS_ISOS:       will be preserved byte-for-byte");
    println!();
    print_host_chain(&chain);
    println!();
    print_esp_diff(&diff);
    println!();
    if apply_plan {
        print_rotation_plan(&diff);
        println!();
        println!(
            "NOTE: #181 Phase 2a — planner only. No writes made to {}.",
            dev.display()
        );
        println!("The destructive executor ships in Phase 2b after OVMF E2E.");
    } else {
        println!("NOTE: this is a read-only eligibility check (phase 1 of #181).");
        println!("The actual in-place update lands in a follow-up PR. No writes");
        println!("were made to {} during this command.", dev.display());
    }
    Ok(())
}

/// Ineligible-branch renderer split out alongside [`handle_eligible`].
fn handle_ineligible(dev: &Path, reason: String, json_mode: bool) -> Result<(), u8> {
    if json_mode {
        print_update_json_ineligible(dev, &reason);
    } else {
        let err = UpdateError::Ineligible {
            reason,
            device: dev.to_path_buf(),
        };
        eprint!("{}", crate::userfacing::render_string(&err));
    }
    Err(1)
}

/// Emit the eligible-case JSON envelope via the typed
/// [`aegis_wire_formats::UpdateReport`]. Phase 4b-5 of #286
/// migrated the hand-rolled `println!()` chain to the wire-format
/// crate. Phase 1 of #181 added the `esp_diff` payload.
///
/// Wire contract pinned via
/// `docs/reference/schemas/aegis-boot-update.schema.json`.
fn print_update_json_eligible(
    dev: &Path,
    disk_guid: &str,
    attestation_path: &Path,
    chain: &[HostChainEntry],
    diff: &[EspFileDiff],
) {
    let host_chain = chain
        .iter()
        .map(|entry| aegis_wire_formats::UpdateChainEntry {
            role: entry.role.to_string(),
            path: entry.path.display().to_string(),
            result: match &entry.sha256 {
                Ok(hash) => aegis_wire_formats::UpdateChainResult::Ok {
                    sha256: hash.clone(),
                },
                Err(reason) => aegis_wire_formats::UpdateChainResult::Error {
                    error: reason.clone(),
                },
            },
        })
        .collect();
    let esp_diff = diff.iter().map(to_wire_file_diff).collect();
    let report = aegis_wire_formats::UpdateReport {
        schema_version: aegis_wire_formats::UPDATE_SCHEMA_VERSION,
        tool_version: env!("CARGO_PKG_VERSION").to_string(),
        device: dev.display().to_string(),
        eligibility: aegis_wire_formats::UpdateEligibility::Eligible {
            disk_guid: disk_guid.to_string(),
            attestation_path: attestation_path.display().to_string(),
            host_chain,
            esp_diff,
        },
    };
    emit_update_report(&report);
}

/// Map an internal [`EspFileDiff`] row to the wire-format
/// [`aegis_wire_formats::UpdateFileDiff`] shape. Splits each
/// `Result<String, String>` pair into the mutually-exclusive
/// `sha256` / `error` wire fields.
fn to_wire_file_diff(d: &EspFileDiff) -> aegis_wire_formats::UpdateFileDiff {
    let (current_sha256, current_error) = match &d.current {
        Ok(h) => (Some(h.clone()), None),
        Err(e) => (None, Some(e.clone())),
    };
    let (fresh_sha256, fresh_error) = match &d.fresh {
        Ok(h) => (Some(h.clone()), None),
        Err(e) => (None, Some(e.clone())),
    };
    aegis_wire_formats::UpdateFileDiff {
        role: d.role.to_string(),
        esp_path: d.esp_path.to_string(),
        current_sha256,
        current_error,
        fresh_sha256,
        fresh_error,
        would_change: d.would_change(),
    }
}

fn print_update_json_ineligible(dev: &Path, reason: &str) {
    let report = aegis_wire_formats::UpdateReport {
        schema_version: aegis_wire_formats::UPDATE_SCHEMA_VERSION,
        tool_version: env!("CARGO_PKG_VERSION").to_string(),
        device: dev.display().to_string(),
        eligibility: aegis_wire_formats::UpdateEligibility::Ineligible {
            reason: reason.to_string(),
        },
    };
    emit_update_report(&report);
}

fn emit_update_report(report: &aegis_wire_formats::UpdateReport) {
    match serde_json::to_string_pretty(report) {
        Ok(body) => println!("{body}"),
        Err(e) => eprintln!("aegis-boot update: failed to serialize --json envelope: {e}"),
    }
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
pub(crate) struct HostChainEntry {
    pub(crate) role: &'static str,
    pub(crate) path: PathBuf,
    pub(crate) sha256: Result<String, String>,
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

/// One row of the ESP diff — one canonical destination path,
/// paired with the two hash attempts (current stick + fresh host
/// source). Each side carries the sha256 on success or an
/// operator-readable reason on failure.
#[derive(Debug, Clone)]
pub(crate) struct EspFileDiff {
    /// Role label (`shim`, `grub`, `grub_cfg_boot`,
    /// `grub_cfg_ubuntu`, `kernel`, `initrd`). Differs from the
    /// host-side chain roles because the ESP has two grub.cfg
    /// copies (boot + ubuntu).
    pub role: &'static str,
    /// Destination path on the ESP, with a leading `/` and no
    /// mtools `::` prefix. Example: `/EFI/BOOT/BOOTX64.EFI`.
    pub esp_path: &'static str,
    /// sha256 of the file currently on the stick's ESP, or the
    /// reason we couldn't read it.
    pub current: Result<String, String>,
    /// sha256 of the host-side source a fresh flash would install,
    /// or the reason we couldn't hash it. Matches the
    /// [`HostChainEntry`] we already computed for the preview.
    pub fresh: Result<String, String>,
}

impl EspFileDiff {
    /// Phase-1 verdict: `true` only when both sides are present
    /// AND differ. Missing-either-side is not a "would change" —
    /// see [`aegis_wire_formats::UpdateFileDiff`] docs.
    pub(crate) fn would_change(&self) -> bool {
        match (&self.current, &self.fresh) {
            (Ok(a), Ok(b)) => a != b,
            _ => false,
        }
    }

    /// Human-readable status label: `CHANGED`, `UNCHANGED`, or
    /// `UNKNOWN` (when either side couldn't be hashed).
    pub(crate) fn status_label(&self) -> &'static str {
        match (&self.current, &self.fresh) {
            (Ok(a), Ok(b)) if a == b => "UNCHANGED",
            (Ok(_), Ok(_)) => "CHANGED",
            _ => "UNKNOWN",
        }
    }
}

/// Canonical ESP layout that `mkusb.sh` / direct-install writes.
/// Kept as a table of `(role, esp_path, source_role)` so the diff
/// has a single source of truth for which files we compare.
///
/// Two rows (`grub_cfg_boot`, `grub_cfg_ubuntu`) both point to the
/// same host-side source `grub_cfg` because `mkusb.sh` writes the
/// same grub.cfg bytes to both destinations.
///
/// `source_role` matches the role strings in [`HostChainEntry`];
/// when it's `grub_cfg` (no host-chain entry today) the fresh
/// side surfaces as an UNKNOWN with a Phase-2 pointer.
const ESP_DIFF_SLOTS: &[(&str, &str, &str)] = &[
    // (role, esp_path_on_stick, host_source_role)
    ("shim", "/EFI/BOOT/BOOTX64.EFI", "shim"),
    ("grub", "/EFI/BOOT/grubx64.efi", "grub"),
    ("grub_cfg_boot", "/EFI/BOOT/grub.cfg", "grub_cfg"),
    ("grub_cfg_ubuntu", "/EFI/ubuntu/grub.cfg", "grub_cfg"),
    ("kernel", "/vmlinuz", "kernel"),
    ("initrd", "/initrd.img", "initrd"),
];

/// Build the per-file diff. Reads each canonical ESP slot via
/// `mtype` on the stick's partition 1, hashes via `sha256sum`,
/// and pairs each row with the matching host-side source's
/// sha256 (already computed by [`resolve_host_chain`]).
///
/// The `grub.cfg` rows carry a Phase-1-specific error on the
/// fresh side because direct-install renders grub.cfg
/// in-process (see `direct_install::build_grub_cfg_body`) rather
/// than reading it from a file — so we have no stable host-side
/// hash for it today. Phase 2 will close that gap.
pub(crate) fn build_esp_diff(esp_part: &Path, chain: &[HostChainEntry]) -> Vec<EspFileDiff> {
    ESP_DIFF_SLOTS
        .iter()
        .map(|(role, esp_path, source_role)| {
            let current = mtype_sha256(esp_part, esp_path);
            let fresh = lookup_chain_sha(chain, source_role);
            EspFileDiff {
                role,
                esp_path,
                current,
                fresh,
            }
        })
        .collect()
}

/// Look up a host-chain entry by role and return its hash
/// `Result`. Absent role → Phase-1-specific "not sampled on host
/// side" message (grub.cfg today, flagged for Phase 2).
fn lookup_chain_sha(chain: &[HostChainEntry], role: &str) -> Result<String, String> {
    match chain.iter().find(|e| e.role == role) {
        Some(entry) => entry.sha256.clone(),
        None => Err(format!(
            "host-side source for role '{role}' not sampled in Phase 1 \
             (grub.cfg is rendered in-process; Phase 2 will wire this up)"
        )),
    }
}

/// Read a file from the FAT32 ESP at `esp_part` via `mtype`,
/// pipe into `sha256sum`, and return the hex digest. On any
/// failure return an operator-readable string.
///
/// We use `mtype -i <part>` rather than mounting because (a) it
/// requires no root, (b) it's the inverse of the `mcopy` calls
/// direct-install already uses to write the ESP, and (c) it
/// doesn't risk leaving the stick mounted if we panic mid-hash.
///
/// `esp_path` must start with `/` — we prepend the mtools `::`
/// convention internally to form the drive-relative path.
pub(crate) fn mtype_sha256(esp_part: &Path, esp_path: &str) -> Result<String, String> {
    if !esp_path.starts_with('/') {
        return Err(format!(
            "ESP path must be absolute (start with '/'), got {esp_path:?}"
        ));
    }
    let dev = esp_part.display().to_string();
    let mtools_target = format!("::{esp_path}");
    // `mtype -i <dev> -- <::path>` — the `--` guards against the
    // ::path being misread as a flag on some mtools versions.
    let mtype_out = Command::new("mtype")
        .arg("-i")
        .arg(&dev)
        .arg("--")
        .arg(&mtools_target)
        .output()
        .map_err(|e| format!("mtype exec failed: {e} (is mtools installed?)"))?;
    if !mtype_out.status.success() {
        let stderr = String::from_utf8_lossy(&mtype_out.stderr);
        return Err(format!(
            "mtype {mtools_target} exited {}: {}",
            mtype_out.status,
            stderr.trim()
        ));
    }
    sha256_stdin(&mtype_out.stdout)
}

/// Feed `bytes` into `sha256sum` on stdin and parse the 64-hex
/// digest. Factored out of [`mtype_sha256`] so the test module
/// can exercise it with known payloads.
pub(crate) fn sha256_stdin(bytes: &[u8]) -> Result<String, String> {
    use std::io::Write;
    let mut child = Command::new("sha256sum")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("sha256sum exec failed: {e}"))?;
    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| "sha256sum stdin unavailable".to_string())?;
        stdin
            .write_all(bytes)
            .map_err(|e| format!("sha256sum write failed: {e}"))?;
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("sha256sum wait failed: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "sha256sum exited {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    parse_sha256_stdout(&String::from_utf8_lossy(&out.stdout))
}

/// Parse a 64-hex-char digest out of a `sha256sum` stdout line.
/// Stdin-fed output is `<64hex>  -\n` — we accept any whitespace-
/// separated first token.
pub(crate) fn parse_sha256_stdout(stdout: &str) -> Result<String, String> {
    stdout
        .split_whitespace()
        .next()
        .filter(|s| s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit()))
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| format!("sha256sum output malformed: {stdout:?}"))
}

/// Print the per-file ESP diff as a human-readable summary.
/// Matches the Phase-1 outcome sentence from #181: "this stick is
/// eligible — 3 files would change, 2 unchanged, `AEGIS_ISOS`
/// preserved".
///
/// Output shape (per row):
///   `CHANGED   /EFI/BOOT/BOOTX64.EFI  sha256:abc…→xyz…`
///   `UNCHANGED /EFI/ubuntu/grub.cfg   sha256:abc…`
///   `UNKNOWN   /vmlinuz               (current unreadable: …)`
pub(crate) fn print_esp_diff(diff: &[EspFileDiff]) {
    let mut changed = 0usize;
    let mut unchanged = 0usize;
    let mut unknown = 0usize;
    for row in diff {
        match row.status_label() {
            "CHANGED" => changed += 1,
            "UNCHANGED" => unchanged += 1,
            _ => unknown += 1,
        }
    }
    println!("ESP diff (current stick vs. fresh flash would install):");
    for row in diff {
        let status = row.status_label();
        let detail = match (&row.current, &row.fresh) {
            (Ok(cur), Ok(fresh)) => {
                if cur == fresh {
                    format!("sha256:{}…", &cur[..cur.len().min(16)])
                } else {
                    format!(
                        "sha256:{}… -> {}…",
                        &cur[..cur.len().min(16)],
                        &fresh[..fresh.len().min(16)],
                    )
                }
            }
            (Err(e), _) => format!("(current unreadable: {e})"),
            (_, Err(e)) => format!("(fresh unavailable: {e})"),
        };
        println!("  {status:<9} {:<30}  {detail}", row.esp_path);
    }
    println!();
    println!(
        "Summary: {changed} would change, {unchanged} unchanged, \
         {unknown} inconclusive. AEGIS_ISOS preserved byte-for-byte."
    );
}

/// #181 Phase 2a — human-readable rotation plan render.
/// Runs ONLY on `--apply --experimental-apply`; plain `update` stays
/// planner-free. Output lists each rotation step's role, destination
/// path, and pre/post sha256 prefixes so the operator can see what
/// the executor WOULD do before Phase 2b ships.
pub(crate) fn print_rotation_plan(diff: &[EspFileDiff]) {
    let plan = crate::update_apply::plan_rotation(diff);
    if plan.is_empty() {
        println!("Rotation plan: no-op — every canonical ESP slot is already current.");
        return;
    }
    println!("Rotation plan ({} step(s), in order):", plan.len());
    for (idx, step) in plan.iter().enumerate() {
        let cur = &step.current_sha256;
        let new = &step.fresh_sha256;
        println!(
            "  [{n}] {role:<16}  {path}",
            n = idx + 1,
            role = step.role,
            path = step.esp_path,
        );
        println!(
            "      current: sha256:{}…   →   new: sha256:{}…",
            &cur[..cur.len().min(16)],
            &new[..new.len().min(16)],
        );
    }
    println!();
    println!("Each step (executor, Phase 2b):");
    println!("  1. backup current to <esp_path>.bak");
    println!("  2. stage new bytes as <esp_path>.new");
    println!("  3. verify <esp_path>.new sha256 matches the planner's fresh_sha256");
    println!("  4. mdel + mren <esp_path>.new over <esp_path>");
    println!("  5. re-verify sha256 post-rename");
    println!("  6. leave .bak in place for future `update --rollback`");
}

/// `/dev/sda` + N → `/dev/sdaN` (SCSI/SATA), `/dev/nvme0n1` + N →
/// `/dev/nvme0n1pN` (NVMe), etc. Duplicated from
/// `flash::partition_path` to avoid cross-module pub-surface churn
/// in this read-only PR — Phase 2 will consolidate (the destructive
/// path will share the same resolution).
#[allow(clippy::doc_markdown)]
pub(crate) fn partition_path(dev: &Path, n: u32) -> PathBuf {
    let s = dev.display().to_string();
    let needs_p =
        s.starts_with("/dev/nvme") || s.starts_with("/dev/mmcblk") || s.starts_with("/dev/loop");
    if needs_p {
        PathBuf::from(format!("{s}p{n}"))
    } else {
        PathBuf::from(format!("{s}{n}"))
    }
}

fn print_help() {
    println!("aegis-boot update — in-place signed-chain update (read-only check for now)");
    println!();
    println!("USAGE:");
    println!("  aegis-boot update <device>");
    println!(
        "  aegis-boot update <device> --apply --experimental-apply  (#181 Phase 2a, planner only)"
    );
    println!("  aegis-boot update --help");
    println!();
    println!("BEHAVIOR (phase 1 of #181):");
    println!("  Validates that the target stick is a known aegis-boot stick and");
    println!("  that its attestation manifest matches the disk GUID. Reports");
    println!("  ELIGIBLE / NOT ELIGIBLE with a specific reason. Does NOT write.");
    println!();
    println!("BEHAVIOR (phase 2a of #181, --apply + --experimental-apply):");
    println!("  Runs the Phase-1 eligibility check + ESP diff, then prints the");
    println!("  ordered rotation plan the Phase-2b executor WOULD follow. No");
    println!("  writes are made. Both flags are required — the double-flag");
    println!("  prevents accidental invocation of a destructive-looking mode.");
    println!();
    println!("  The actual atomic in-place update (the executor) lands in Phase");
    println!("  2b after OVMF E2E validation — see issue #181.");
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
    let dir = attestation_dir();
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

/// Path to the attestations directory. Delegates to the shared
/// resolver (#375 Phase 1) which honors `AEGIS_STATE_DIR`, sudo-aware
/// `HOME` (so `sudo aegis-boot update` lands in the same dir
/// `aegis-boot flash` wrote to), and the XDG/fallback chain — in one
/// place, so the lookup can't drift between callers the way it did
/// pre-refactor. Kept `pub(crate)` because this module has consumers
/// that are easier to leave on the shim than rewire at call-sites.
pub(crate) fn attestation_dir() -> PathBuf {
    crate::paths::attestations_dir()
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
        use crate::userfacing::{UserFacing, render_string};
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

    // ---------- Phase 1 of #181: ESP diff unit tests ----------

    #[test]
    fn partition_path_handles_scsi_nvme_and_loop() {
        assert_eq!(
            partition_path(Path::new("/dev/sda"), 1),
            PathBuf::from("/dev/sda1"),
        );
        assert_eq!(
            partition_path(Path::new("/dev/nvme0n1"), 2),
            PathBuf::from("/dev/nvme0n1p2"),
        );
        assert_eq!(
            partition_path(Path::new("/dev/mmcblk0"), 1),
            PathBuf::from("/dev/mmcblk0p1"),
        );
        assert_eq!(
            partition_path(Path::new("/dev/loop0"), 1),
            PathBuf::from("/dev/loop0p1"),
        );
    }

    #[test]
    fn parse_sha256_stdout_extracts_64_hex_token() {
        // sha256sum prints "<64hex>  <path>\n" or "<64hex>  -\n".
        let out = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  -\n";
        let got = parse_sha256_stdout(out).expect("should parse");
        assert_eq!(got.len(), 64);
        assert!(got.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn parse_sha256_stdout_rejects_malformed_and_short_tokens() {
        assert!(parse_sha256_stdout("").is_err());
        assert!(parse_sha256_stdout("nope  -\n").is_err());
        // 63 chars — too short.
        assert!(parse_sha256_stdout(&format!("{}  -\n", "a".repeat(63))).is_err());
        // 65 chars — the filter hits the len() == 64 guard.
        assert!(parse_sha256_stdout(&format!("{}  -\n", "a".repeat(65))).is_err());
    }

    #[test]
    fn parse_sha256_stdout_normalizes_case_to_lower() {
        let out = "E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855  -";
        let got = parse_sha256_stdout(out).expect("parse");
        assert_eq!(got, got.to_lowercase());
    }

    #[test]
    fn sha256_stdin_produces_canonical_empty_string_digest() {
        // Well-known: sha256("") = e3b0c442... . If sha256sum isn't
        // on PATH, skip — unit tests must not depend on environment.
        if which_sha256sum().is_err() {
            return;
        }
        let got = sha256_stdin(b"").expect("hash empty bytes");
        assert_eq!(
            got,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        );
    }

    #[test]
    fn sha256_stdin_hashes_known_payload() {
        // sha256("hello\n") = ...
        if which_sha256sum().is_err() {
            return;
        }
        let got = sha256_stdin(b"hello\n").expect("hash");
        assert_eq!(
            got,
            "5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03",
        );
    }

    #[test]
    fn mtype_sha256_rejects_non_absolute_esp_path() {
        // Guard rail: callers MUST pass a leading-slash path.
        // Without this guard, "/EFI/BOOT/BOOTX64.EFI" vs
        // "EFI/BOOT/BOOTX64.EFI" would both compose to different
        // mtools targets.
        let dev = PathBuf::from("/dev/loopnonexistent1");
        let err = mtype_sha256(&dev, "EFI/BOOT/BOOTX64.EFI").expect_err("should reject");
        assert!(err.contains("must be absolute"), "{err}");
    }

    #[test]
    fn esp_file_diff_would_change_semantics() {
        let same = EspFileDiff {
            role: "shim",
            esp_path: "/EFI/BOOT/BOOTX64.EFI",
            current: Ok("a".repeat(64)),
            fresh: Ok("a".repeat(64)),
        };
        assert!(!same.would_change());
        assert_eq!(same.status_label(), "UNCHANGED");

        let differ = EspFileDiff {
            role: "shim",
            esp_path: "/EFI/BOOT/BOOTX64.EFI",
            current: Ok("a".repeat(64)),
            fresh: Ok("b".repeat(64)),
        };
        assert!(differ.would_change());
        assert_eq!(differ.status_label(), "CHANGED");

        // Either side missing must NOT report CHANGED — it's
        // UNKNOWN. This invariant matters: a naive impl that
        // `Ok != Err` would incorrectly say "would change".
        let current_missing = EspFileDiff {
            role: "shim",
            esp_path: "/EFI/BOOT/BOOTX64.EFI",
            current: Err("mtype not installed".to_string()),
            fresh: Ok("a".repeat(64)),
        };
        assert!(!current_missing.would_change());
        assert_eq!(current_missing.status_label(), "UNKNOWN");

        let fresh_missing = EspFileDiff {
            role: "shim",
            esp_path: "/EFI/BOOT/BOOTX64.EFI",
            current: Ok("a".repeat(64)),
            fresh: Err("shim package not installed".to_string()),
        };
        assert!(!fresh_missing.would_change());
        assert_eq!(fresh_missing.status_label(), "UNKNOWN");
    }

    #[test]
    fn build_esp_diff_has_six_canonical_rows_matching_direct_install() {
        // The six ESP destinations in `direct_install::stage_esp`
        // MUST each get a diff row — this test catches silent
        // drift if stage_esp adds a new file and build_esp_diff
        // forgets to sample it.
        let dev = PathBuf::from("/dev/nonexistent-test-loop");
        let chain: Vec<HostChainEntry> = vec![];
        let diff = build_esp_diff(&dev, &chain);
        let paths: Vec<&str> = diff.iter().map(|d| d.esp_path).collect();
        assert_eq!(diff.len(), 6);
        assert!(paths.contains(&"/EFI/BOOT/BOOTX64.EFI"));
        assert!(paths.contains(&"/EFI/BOOT/grubx64.efi"));
        assert!(paths.contains(&"/EFI/BOOT/grub.cfg"));
        assert!(paths.contains(&"/EFI/ubuntu/grub.cfg"));
        assert!(paths.contains(&"/vmlinuz"));
        assert!(paths.contains(&"/initrd.img"));
    }

    #[test]
    fn build_esp_diff_pairs_fresh_side_from_host_chain_by_role() {
        // Fresh side comes from the host chain via role matching.
        // Here we hand it a synthetic chain — the real one calls
        // sha256sum which we can't rely on in tests.
        let dev = PathBuf::from("/dev/nonexistent-test-loop");
        let chain = vec![
            HostChainEntry {
                role: "shim",
                path: PathBuf::from("/usr/lib/shim/shimx64.efi.signed"),
                sha256: Ok("a".repeat(64)),
            },
            HostChainEntry {
                role: "kernel",
                path: PathBuf::from("/boot/vmlinuz-6.8.0-virtual"),
                sha256: Ok("b".repeat(64)),
            },
        ];
        let diff = build_esp_diff(&dev, &chain);
        let shim = diff
            .iter()
            .find(|d| d.esp_path == "/EFI/BOOT/BOOTX64.EFI")
            .expect("shim row");
        // current fails because dev doesn't exist — that's OK.
        assert!(shim.current.is_err());
        // fresh comes from the synthetic chain.
        assert_eq!(shim.fresh.as_ref().ok(), Some(&"a".repeat(64)));
        // grub.cfg rows get the Phase-1 "not sampled" error on
        // the fresh side because the chain has no grub_cfg entry.
        let grub_cfg = diff
            .iter()
            .find(|d| d.esp_path == "/EFI/BOOT/grub.cfg")
            .expect("grub.cfg row");
        let err = grub_cfg.fresh.as_ref().expect_err("fresh is Err today");
        assert!(err.contains("not sampled in Phase 1"), "{err}");
    }

    #[test]
    fn to_wire_file_diff_maps_ok_side_into_sha256_field() {
        let internal = EspFileDiff {
            role: "shim",
            esp_path: "/EFI/BOOT/BOOTX64.EFI",
            current: Ok("a".repeat(64)),
            fresh: Ok("b".repeat(64)),
        };
        let wire = to_wire_file_diff(&internal);
        assert_eq!(wire.role, "shim");
        assert_eq!(wire.esp_path, "/EFI/BOOT/BOOTX64.EFI");
        assert_eq!(
            wire.current_sha256.as_deref(),
            Some("a".repeat(64).as_str())
        );
        assert!(wire.current_error.is_none());
        assert_eq!(wire.fresh_sha256.as_deref(), Some("b".repeat(64).as_str()));
        assert!(wire.fresh_error.is_none());
        assert!(wire.would_change);
    }

    #[test]
    fn to_wire_file_diff_maps_err_side_into_error_field() {
        let internal = EspFileDiff {
            role: "kernel",
            esp_path: "/vmlinuz",
            current: Err("mtype not installed".to_string()),
            fresh: Ok("b".repeat(64)),
        };
        let wire = to_wire_file_diff(&internal);
        assert!(wire.current_sha256.is_none());
        assert_eq!(wire.current_error.as_deref(), Some("mtype not installed"));
        assert_eq!(wire.fresh_sha256.as_deref(), Some("b".repeat(64).as_str()));
        // Missing current side => comparison inconclusive =>
        // would_change MUST be false (not true).
        assert!(!wire.would_change);
    }

    /// Helper: `which sha256sum` without pulling in a new dep.
    /// Tests that need sha256sum skip when it's unavailable.
    fn which_sha256sum() -> Result<(), ()> {
        Command::new("sha256sum")
            .arg("--version")
            .output()
            .map(|_| ())
            .map_err(|_| ())
    }

    fn which_tool(bin: &str) -> Result<(), ()> {
        Command::new(bin)
            .arg("--version")
            .output()
            .map(|_| ())
            .map_err(|_| ())
    }

    /// End-to-end fixture test: build a FAT32 image, `mcopy` a
    /// known payload into the canonical `/EFI/BOOT/BOOTX64.EFI`
    /// slot, then ask [`mtype_sha256`] to read it back. Asserts
    /// the hash matches the host-side sha256sum of the same
    /// payload.
    ///
    /// Skips silently when the mtools stack (`mkfs.vfat`,
    /// `mcopy`, `mtype`, `sha256sum`, `dd`) isn't available —
    /// we don't want to block CI runners that lack these
    /// utilities; the lower-level unit tests above cover the
    /// argument-plumbing logic.
    #[test]
    fn mtype_sha256_reads_back_known_payload_from_fat32_image() {
        for tool in ["mkfs.vfat", "mcopy", "mtype", "sha256sum", "dd"] {
            if which_tool(tool).is_err() {
                return;
            }
        }
        // test-only: tempdir for synthesizing a FAT32 image + running
        // mtype against it. pid-suffixed, test-scope, test cleans up
        // at the end of its own body. Not a security boundary — same
        // pattern as flash.rs:617 for direct-install's work dir.
        // nosemgrep: rust.lang.security.temp-dir.temp-dir
        let tmp_root = std::env::temp_dir();
        let tmp = tmp_root.join(format!("aegis-update-phase1-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("mkdir tmp");
        let img = tmp.join("esp.img");
        // 4 MiB FAT32 image — smallest that mkfs.vfat will format
        // cleanly as FAT32 on Debian 12 mtools.
        let out = Command::new("dd")
            .arg("if=/dev/zero")
            .arg(format!("of={}", img.display()))
            .arg("bs=1M")
            .arg("count=16")
            .arg("status=none")
            .status()
            .expect("dd");
        assert!(out.success(), "dd failed");
        let out = Command::new("mkfs.vfat")
            .arg("-F")
            .arg("32")
            .arg("-n")
            .arg("ESP")
            .arg(&img)
            .output()
            .expect("mkfs.vfat");
        assert!(
            out.status.success(),
            "mkfs.vfat failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        // Stage a known payload at the canonical shim location.
        let payload = tmp.join("shim.bin");
        std::fs::write(&payload, b"aegis-boot phase-1 fixture payload\n").expect("write payload");
        let expected =
            sha256_stdin(b"aegis-boot phase-1 fixture payload\n").expect("hash payload on host");
        // mmd to create ::/EFI/BOOT, then mcopy the payload in.
        let s = img.display().to_string();
        let mmd = Command::new("mmd")
            .arg("-i")
            .arg(&s)
            .arg("::/EFI")
            .arg("::/EFI/BOOT")
            .output()
            .expect("mmd");
        assert!(
            mmd.status.success(),
            "mmd failed: {}",
            String::from_utf8_lossy(&mmd.stderr)
        );
        let mcp = Command::new("mcopy")
            .arg("-i")
            .arg(&s)
            .arg("--")
            .arg(&payload)
            .arg("::/EFI/BOOT/BOOTX64.EFI")
            .output()
            .expect("mcopy");
        assert!(
            mcp.status.success(),
            "mcopy failed: {}",
            String::from_utf8_lossy(&mcp.stderr)
        );
        // Now the code under test: read back via mtype_sha256.
        let got = mtype_sha256(&img, "/EFI/BOOT/BOOTX64.EFI")
            .expect("mtype_sha256 should read the fixture");
        assert_eq!(got, expected);
        // Negative case: missing file should produce an Err, not panic.
        let missing = mtype_sha256(&img, "/EFI/BOOT/grubx64.efi");
        assert!(missing.is_err(), "missing file should be Err");
        // Cleanup best-effort — don't fail the test on leftover tmp.
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
