// SPDX-License-Identifier: MIT OR Apache-2.0

//! `aegis-boot verify [device|mount]` — re-verify every ISO on a stick.
//!
//! Closes the trust-narrative loop (flash → add → *verify*). For each
//! `.iso` under the `AEGIS_ISOS` partition, re-runs `iso_probe::verify_iso_hash`
//! against its sibling checksum file and aggregates the result.
//!
//! This is deliberately separate from rescue-tui's in-TUI `v` key —
//! that's operator-initiated during a boot, while this is a host-side
//! preflight operators can run before ejecting a stick into a customer
//! environment.
//!
//! # Exit codes
//!
//! - `0` — every ISO verified OR the only verdicts are `NotPresent`
//!   (no sidecar checksum to verify against; not an error)
//! - `1` — at least one `Mismatch`, `Forged`, or `Unreadable` verdict
//! - `2` — invalid arguments or could not resolve the target
//!
//! The "`NotPresent` is OK" semantics matches what rescue-tui does —
//! an operator adding a random ISO without a `.sha256` sidecar isn't
//! making a security claim, so `verify()` shouldn't reject it.
//!
//! Tracked as the "aegis-boot verify" item consensus-voted highest-
//! leverage after the `update` epic #181.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use iso_probe::HashVerification;

use crate::inventory::{resolve_mount, unmount_temp};

/// Entry point for `aegis-boot verify [device|mount]`.
pub fn run(args: &[String]) -> ExitCode {
    match try_run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => ExitCode::from(code),
    }
}

/// Inner runner returning a typed result.
pub(crate) fn try_run(args: &[String]) -> Result<(), u8> {
    if args.first().map(String::as_str) == Some("--help")
        || args.first().map(String::as_str) == Some("-h")
    {
        print_help();
        return Ok(());
    }

    // --json suppresses per-ISO and summary prints; emits one structured
    // document at the end. Matches the doctor/list/attest pattern. Flag
    // detection tolerates the flag appearing anywhere in args; positional
    // args (the mount target) are whatever doesn't start with `--`.
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
                    Err(err) => eprintln!("aegis-boot verify: serialize error envelope: {err}"),
                }
            } else {
                eprintln!("aegis-boot verify: {e}");
            }
            return Err(2);
        }
    };

    let isos = scan_iso_files(&mount.path);
    if isos.is_empty() {
        if json_mode {
            print_verify_json_empty(&mount.path);
        } else {
            println!(
                "No .iso files on {} — nothing to verify.",
                mount.path.display()
            );
        }
        if mount.temporary {
            unmount_temp(&mount);
        }
        return Ok(());
    }

    if !json_mode {
        println!(
            "Verifying {} ISO(s) on {}...",
            isos.len(),
            mount.path.display()
        );
        println!();
    }

    let mut tally = Tally::default();
    // When json_mode, we collect verdicts for the final structured
    // emit rather than streaming them. Vec<(iso_name, verdict)>.
    let mut verdicts: Vec<(String, HashVerification)> = Vec::with_capacity(isos.len());
    for iso in &isos {
        let verdict = iso_probe::verify_iso_hash(iso).unwrap_or_else(|e| {
            // Failure to read the ISO file itself — distinct from
            // failure to read the sidecar. Map to Unreadable with the
            // ISO path as source so the operator sees the cause.
            HashVerification::Unreadable {
                source: iso.display().to_string(),
                reason: e.to_string(),
            }
        });
        let iso_name = iso
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("(unknown)")
            .to_string();
        if !json_mode {
            print_verdict(&iso_name, &verdict);
        }
        tally.record(&verdict);
        verdicts.push((iso_name, verdict));
    }

    if json_mode {
        print_verify_json(&mount.path, &verdicts, &tally);
    } else {
        println!();
        tally.print_summary();
    }

    if mount.temporary {
        unmount_temp(&mount);
    }

    // Exit code: any Mismatch, Forged, or Unreadable is a fail. NotPresent
    // alone is OK — see module doc for the rationale.
    if tally.any_failure() {
        Err(1)
    } else {
        Ok(())
    }
}

/// Empty-stick JSON envelope. Stable `schema_version=1`; every field
/// is present (even if 0 or empty array) so a downstream consumer can
/// parse without conditionals.
///
/// Phase 4b-4 of #286 migrated this + [`print_verify_json`] from
/// hand-rolled `println!()` chains to the typed
/// [`aegis_wire_formats::VerifyReport`] envelope. Wire contract pinned
/// via `docs/reference/schemas/aegis-boot-verify.schema.json`.
fn print_verify_json_empty(mount_path: &std::path::Path) {
    let report = aegis_wire_formats::VerifyReport {
        schema_version: aegis_wire_formats::VERIFY_SCHEMA_VERSION,
        tool_version: env!("CARGO_PKG_VERSION").to_string(),
        mount_path: mount_path.display().to_string(),
        summary: aegis_wire_formats::VerifySummary {
            total: 0,
            verified: 0,
            mismatch: 0,
            unreadable: 0,
            not_present: 0,
            any_failure: false,
        },
        isos: Vec::new(),
    };
    emit_verify_report(&report);
}

/// Populated-stick JSON envelope. Schema matches `print_verify_json_empty`
/// with a non-empty `isos` array.
fn print_verify_json(
    mount_path: &std::path::Path,
    verdicts: &[(String, HashVerification)],
    tally: &Tally,
) {
    let report = aegis_wire_formats::VerifyReport {
        schema_version: aegis_wire_formats::VERIFY_SCHEMA_VERSION,
        tool_version: env!("CARGO_PKG_VERSION").to_string(),
        mount_path: mount_path.display().to_string(),
        summary: aegis_wire_formats::VerifySummary {
            total: u32::try_from(tally.total()).unwrap_or(u32::MAX),
            verified: u32::try_from(tally.verified).unwrap_or(u32::MAX),
            mismatch: u32::try_from(tally.mismatch).unwrap_or(u32::MAX),
            unreadable: u32::try_from(tally.unreadable).unwrap_or(u32::MAX),
            not_present: u32::try_from(tally.not_present).unwrap_or(u32::MAX),
            any_failure: tally.any_failure(),
        },
        isos: verdicts
            .iter()
            .map(|(name, v)| aegis_wire_formats::VerifyEntry {
                name: name.clone(),
                verdict: convert_verdict(v),
            })
            .collect(),
    };
    emit_verify_report(&report);
}

fn emit_verify_report(report: &aegis_wire_formats::VerifyReport) {
    match serde_json::to_string_pretty(report) {
        Ok(body) => println!("{body}"),
        Err(e) => eprintln!("aegis-boot verify: failed to serialize --json envelope: {e}"),
    }
}

/// Map the local [`HashVerification`] enum (from iso-probe) onto the
/// wire-format [`aegis_wire_formats::VerifyVerdict`] enum. Both have the
/// same 4 variants with the same fields, so this is a pure
/// structural translation.
fn convert_verdict(v: &HashVerification) -> aegis_wire_formats::VerifyVerdict {
    match v {
        HashVerification::Verified { digest, source } => {
            aegis_wire_formats::VerifyVerdict::Verified {
                digest: digest.clone(),
                source: source.clone(),
            }
        }
        HashVerification::Mismatch {
            actual,
            expected,
            source,
        } => aegis_wire_formats::VerifyVerdict::Mismatch {
            actual: actual.clone(),
            expected: expected.clone(),
            source: source.clone(),
        },
        HashVerification::Unreadable { source, reason } => {
            aegis_wire_formats::VerifyVerdict::Unreadable {
                source: source.clone(),
                reason: reason.clone(),
            }
        }
        HashVerification::NotPresent => aegis_wire_formats::VerifyVerdict::NotPresent,
    }
}

fn print_help() {
    println!("aegis-boot verify — re-verify every ISO on a stick against its sidecar checksum");
    println!();
    println!("USAGE:");
    println!("  aegis-boot verify                 Auto-find mounted AEGIS_ISOS");
    println!("  aegis-boot verify /dev/sdX        Mount partition 2 and verify");
    println!("  aegis-boot verify /mnt/iso-dir    Use explicit mount path");
    println!("  aegis-boot verify --json [target] Machine-readable (CI / monitoring)");
    println!("  aegis-boot verify --help");
    println!();
    println!("EXIT CODES:");
    println!("  0  every ISO verified (or only NotPresent verdicts — no sidecars)");
    println!("  1  any Mismatch / Forged / Unreadable verdict");
    println!("  2  invalid arguments or could not resolve the target");
    println!();
    println!("RELATED:");
    println!("  aegis-boot list      — view verification state + attestation summary");
    println!("  aegis-boot add       — copy + verify a new ISO onto the stick");
    println!("  (In rescue-tui, press `v` on an ISO for the in-boot re-verification.)");
}

/// List every `.iso` file at the top level of `dir`. Non-recursive —
/// `AEGIS_ISOS` is a flat layout. Files are sorted alphabetically so
/// verify output is deterministic.
fn scan_iso_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("iso"))
            && p.is_file()
        {
            out.push(p);
        }
    }
    out.sort();
    out
}

fn print_verdict(iso_name: &str, verdict: &HashVerification) {
    match verdict {
        HashVerification::Verified { source, .. } => {
            let sidecar = short_source(source);
            println!("  [\u{2713} verified ] {iso_name}   (sidecar: {sidecar})");
        }
        HashVerification::Mismatch {
            actual,
            expected,
            source,
        } => {
            let sidecar = short_source(source);
            println!("  [\u{2717} MISMATCH] {iso_name}   (sidecar: {sidecar})");
            println!("      expected: {expected}");
            println!("      actual:   {actual}");
        }
        HashVerification::Unreadable { source, reason } => {
            let sidecar = short_source(source);
            println!("  [! UNREAD  ] {iso_name}   (sidecar: {sidecar})");
            println!("      reason: {reason}");
        }
        HashVerification::NotPresent => {
            println!("  [ no sidecar] {iso_name}   (no .sha256 or SHA256SUMS)");
        }
    }
}

/// Short-form the sidecar path so a multi-hundred-char /run/media
/// path doesn't hide the operative signal.
fn short_source(source: &str) -> String {
    // Take the last two path components if there's a '/'; otherwise
    // return as-is.
    let p = Path::new(source);
    let parent = p.parent().and_then(|pp| pp.file_name());
    let basename = p.file_name();
    match (parent, basename) {
        (Some(par), Some(base)) => format!("{}/{}", par.to_string_lossy(), base.to_string_lossy()),
        _ => source.to_string(),
    }
}

#[derive(Default)]
struct Tally {
    verified: usize,
    mismatch: usize,
    unreadable: usize,
    not_present: usize,
}

impl Tally {
    fn record(&mut self, v: &HashVerification) {
        match v {
            HashVerification::Verified { .. } => self.verified += 1,
            HashVerification::Mismatch { .. } => self.mismatch += 1,
            HashVerification::Unreadable { .. } => self.unreadable += 1,
            HashVerification::NotPresent => self.not_present += 1,
        }
    }

    fn any_failure(&self) -> bool {
        self.mismatch > 0 || self.unreadable > 0
    }

    fn total(&self) -> usize {
        self.verified + self.mismatch + self.unreadable + self.not_present
    }

    fn print_summary(&self) {
        println!("Summary: {} ISO(s) total", self.total());
        println!("  verified:     {}", self.verified);
        if self.not_present > 0 {
            println!(
                "  no sidecar:   {}  (NotPresent — not a failure)",
                self.not_present
            );
        }
        if self.mismatch > 0 {
            println!("  MISMATCH:     {}  (failure)", self.mismatch);
        }
        if self.unreadable > 0 {
            println!("  UNREADABLE:   {}  (failure)", self.unreadable);
        }
        println!();
        if self.any_failure() {
            println!("Overall: FAIL — do not ship this stick without resolving the above.");
        } else if self.verified == 0 {
            println!(
                "Overall: no verification performed — no .sha256 / SHA256SUMS sidecars present."
            );
        } else {
            println!("Overall: OK — every ISO with a sidecar verifies.");
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::missing_panics_doc)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn verified() -> HashVerification {
        HashVerification::Verified {
            digest: "abc123".to_string(),
            source: "/mnt/iso/u.iso.sha256".to_string(),
        }
    }

    fn mismatch() -> HashVerification {
        HashVerification::Mismatch {
            actual: "aaa".to_string(),
            expected: "bbb".to_string(),
            source: "/mnt/iso/u.iso.sha256".to_string(),
        }
    }

    fn unreadable() -> HashVerification {
        HashVerification::Unreadable {
            source: "/mnt/iso/u.iso.sha256".to_string(),
            reason: "permission denied".to_string(),
        }
    }

    #[test]
    fn tally_counts_each_variant() {
        let mut t = Tally::default();
        t.record(&verified());
        t.record(&verified());
        t.record(&mismatch());
        t.record(&HashVerification::NotPresent);
        t.record(&unreadable());
        assert_eq!(t.verified, 2);
        assert_eq!(t.mismatch, 1);
        assert_eq!(t.not_present, 1);
        assert_eq!(t.unreadable, 1);
        assert_eq!(t.total(), 5);
    }

    #[test]
    fn any_failure_true_for_mismatch() {
        let mut t = Tally::default();
        t.record(&mismatch());
        assert!(t.any_failure());
    }

    #[test]
    fn any_failure_true_for_unreadable() {
        let mut t = Tally::default();
        t.record(&unreadable());
        assert!(t.any_failure());
    }

    #[test]
    fn any_failure_false_for_verified_plus_not_present() {
        // The non-negotiable exit-code rule: NotPresent alone is OK.
        let mut t = Tally::default();
        t.record(&verified());
        t.record(&HashVerification::NotPresent);
        t.record(&HashVerification::NotPresent);
        assert!(!t.any_failure());
    }

    #[test]
    fn any_failure_false_for_all_not_present() {
        // Every ISO lacks a sidecar — not a failure, just no
        // verification material.
        let mut t = Tally::default();
        for _ in 0..5 {
            t.record(&HashVerification::NotPresent);
        }
        assert!(!t.any_failure());
    }

    #[test]
    fn short_source_keeps_last_two_components() {
        assert_eq!(
            short_source("/run/media/aegis-isos/ubuntu.iso.sha256"),
            "aegis-isos/ubuntu.iso.sha256"
        );
    }

    #[test]
    fn short_source_handles_bare_filename() {
        assert_eq!(short_source("ubuntu.iso.sha256"), "ubuntu.iso.sha256");
    }

    #[test]
    fn scan_iso_files_returns_sorted_iso_filenames() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("b.iso"), b"").unwrap();
        std::fs::write(root.join("a.iso"), b"").unwrap();
        std::fs::write(root.join("readme.txt"), b"").unwrap(); // non-iso
        std::fs::write(root.join("c.ISO"), b"").unwrap(); // upper-case ext
        let found = scan_iso_files(root);
        let names: Vec<_> = found
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["a.iso", "b.iso", "c.ISO"]);
    }

    #[test]
    fn scan_iso_files_empty_on_empty_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let found = scan_iso_files(tmp.path());
        assert!(found.is_empty());
    }

    #[test]
    fn scan_iso_files_empty_on_nonexistent_dir() {
        let found = scan_iso_files(&PathBuf::from(
            "/this-path-does-not-exist-for-aegis-boot-verify-test",
        ));
        assert!(found.is_empty());
    }
}
