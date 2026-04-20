//! On-stick failure microreport writer (#342 Phase 2, Tier A).
//!
//! When rescue-tui or early initramfs hits a classifiable boot
//! failure, a tiny anonymous JSON record is written so the
//! operator can later include it in an `aegis-boot bug-report`
//! bundle. Two-stage persistence:
//!
//! 1. **Tmpfs first** — write to `/run/aegis-boot-logs/` as soon
//!    as the failure is known. Always writable, survives all
//!    non-reboot failure modes.
//! 2. **Migrate to `AEGIS_ISOS`** — once the data partition is
//!    mounted at `/run/media/aegis-isos/`, drain the tmpfs
//!    spool into `AEGIS_ISOS/aegis-boot-logs/` so the record
//!    survives reboot.
//!
//! This module owns **only Tier A** (anonymous microreport) in
//! Phase 2. Tier B (full structured log, operator-consented) is
//! deferred to #342 Phase 3 with a distinct `schema_version`
//! track.
//!
//! The [`FailureMicroreport`] envelope lives in the
//! `aegis-wire-formats` crate so the host-side bug-report
//! collector (Phase 3) can parse logs without a rescue-tui
//! dep. JSON Schema is committed under `docs/reference/schemas/`.

use std::path::{Path, PathBuf};

use aegis_wire_formats::{FailureMicroreport, FAILURE_MICROREPORT_SCHEMA_VERSION};
use sha2::{Digest, Sha256};

/// Tmpfs root where microreports land during a failure-in-flight.
/// Always writable; survives all non-reboot failure modes inside
/// the rescue-tui / initramfs environment.
pub(crate) const TMPFS_SPOOL_DIR: &str = "/run/aegis-boot-logs";

/// Canonical mount point for the `AEGIS_ISOS` data partition when
/// the rescue-tui has reached a stable state. Matches the existing
/// `save_error_log` pattern from #92.
pub(crate) const AEGIS_ISOS_MOUNT: &str = "/run/media/aegis-isos";

/// Subdirectory inside `AEGIS_ISOS` where microreports persist
/// across reboot.
pub(crate) const AEGIS_ISOS_LOG_DIR: &str = "aegis-boot-logs";

/// Rotation ceiling. Keep the most-recent N records per tier;
/// older ones are deleted on the next successful boot migration.
pub(crate) const MAX_RETAINED_LOGS: usize = 10;

/// Build a Tier-A [`FailureMicroreport`] from the operator-visible
/// context. Every field is loosely-bucketed or opaquely-hashed;
/// nothing identifies a specific operator or machine beyond
/// vendor + year.
///
/// Callers should be small factories — one per classification
/// site — so the classified `failure_class` / `boot_step_reached`
/// strings stay stable across rescue-tui versions.
pub(crate) fn build_microreport(
    aegis_boot_version: &str,
    sys_vendor: Option<&str>,
    bios_date: Option<&str>,
    boot_step_reached: &str,
    failure_class: &str,
    full_error_text: &str,
    collected_at: &str,
) -> FailureMicroreport {
    FailureMicroreport {
        schema_version: FAILURE_MICROREPORT_SCHEMA_VERSION,
        tier: "A".to_string(),
        collected_at: collected_at.to_string(),
        aegis_boot_version: aegis_boot_version.to_string(),
        vendor_family: vendor_family_of(sys_vendor),
        bios_year: bios_year_of(bios_date),
        boot_step_reached: boot_step_reached.to_string(),
        failure_class: failure_class.to_string(),
        failure_hash: opaque_hash(full_error_text),
    }
}

/// Lowercased first whitespace-delimited token of the DMI
/// `sys_vendor` string. Falls back to `"unknown"` for missing or
/// placeholder vendors so the field is always present.
fn vendor_family_of(sys_vendor: Option<&str>) -> String {
    let Some(raw) = sys_vendor else {
        return "unknown".to_string();
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() || is_oem_placeholder(trimmed) {
        return "unknown".to_string();
    }
    trimmed
        .split_whitespace()
        .next()
        .unwrap_or("unknown")
        .to_lowercase()
}

fn is_oem_placeholder(s: &str) -> bool {
    matches!(
        s,
        "To be filled by O.E.M."
            | "To Be Filled By O.E.M."
            | "Default string"
            | "System manufacturer"
            | "Not Specified"
            | "Not Applicable"
            | "OEM"
    )
}

/// Four-digit year extracted from a DMI `bios_date`. Accepts the
/// common `MM/DD/YYYY` and `YYYY-MM-DD` forms; falls back to
/// `"unknown"` for anything else.
fn bios_year_of(bios_date: Option<&str>) -> String {
    let Some(raw) = bios_date else {
        return "unknown".to_string();
    };
    let trimmed = raw.trim();
    if let Some(year) = trimmed.rsplit('/').next().filter(|s| is_four_digit_year(s)) {
        return year.to_string();
    }
    if let Some(year) = trimmed.split('-').next().filter(|s| is_four_digit_year(s)) {
        return year.to_string();
    }
    "unknown".to_string()
}

fn is_four_digit_year(s: &str) -> bool {
    s.len() == 4 && s.chars().all(|c| c.is_ascii_digit())
}

fn opaque_hash(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    let digest = hasher.finalize();
    format!("sha256:{}", hex::encode(digest))
}

/// Serialize a [`FailureMicroreport`] to a JSON bytestring.
///
/// Returns a string rather than writing directly so the writer
/// fn can apply it to whichever directory ended up writable.
pub(crate) fn serialize(report: &FailureMicroreport) -> Result<String, String> {
    serde_json::to_string_pretty(report).map_err(|e| format!("serialize microreport: {e}"))
}

/// Write a microreport to the best-available location. Prefers
/// `AEGIS_ISOS/aegis-boot-logs/` when the data partition is
/// mounted and writable; falls back to the tmpfs spool otherwise.
///
/// Returns the absolute path of the written file on success.
pub(crate) fn write(report: &FailureMicroreport) -> Result<PathBuf, String> {
    let body = serialize(report)?;
    let filename = format!(
        "{}-{}.json",
        report.collected_at,
        short_hash(&report.failure_hash)
    );
    write_to_preferred_dir(&filename, body.as_bytes())
}

fn short_hash(full: &str) -> String {
    full.strip_prefix("sha256:")
        .unwrap_or(full)
        .chars()
        .take(12)
        .collect()
}

fn write_to_preferred_dir(filename: &str, body: &[u8]) -> Result<PathBuf, String> {
    // Preference order:
    //   1. AEGIS_ISOS data partition (survives reboot — best case)
    //   2. Tmpfs spool (always writable — rescue path)
    let candidates = [
        Path::new(AEGIS_ISOS_MOUNT).join(AEGIS_ISOS_LOG_DIR),
        PathBuf::from(TMPFS_SPOOL_DIR),
    ];
    for dir in &candidates {
        if try_write_into(dir, filename, body).is_ok() {
            return Ok(dir.join(filename));
        }
    }
    Err(format!(
        "no writable target for failure microreport (tried {})",
        candidates
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

fn try_write_into(dir: &Path, filename: &str, body: &[u8]) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    // Only attempt to write if the parent is actually a directory
    // after the mkdir call — belt-and-suspenders against a weird
    // race where dir exists as a file.
    if !dir.is_dir() {
        return Err(format!("{} is not a directory after mkdir", dir.display()));
    }
    let path = dir.join(filename);
    std::fs::write(&path, body).map_err(|e| format!("write {}: {e}", path.display()))
}

/// Drain the tmpfs spool into the `AEGIS_ISOS` partition when it's
/// available. Returns the set of source paths that were
/// successfully migrated (i.e., copied to `AEGIS_ISOS` and deleted
/// from tmpfs).
///
/// No-op when `AEGIS_ISOS` isn't mounted / writable. Safe to call
/// every few seconds from a rescue-tui event loop.
pub(crate) fn migrate_tmpfs_to_aegis_isos() -> Vec<PathBuf> {
    migrate_between(Path::new(TMPFS_SPOOL_DIR), &aegis_isos_log_dir())
}

fn aegis_isos_log_dir() -> PathBuf {
    Path::new(AEGIS_ISOS_MOUNT).join(AEGIS_ISOS_LOG_DIR)
}

fn migrate_between(src_dir: &Path, dst_dir: &Path) -> Vec<PathBuf> {
    let mut migrated = Vec::new();
    let Ok(entries) = std::fs::read_dir(src_dir) else {
        return migrated;
    };
    if std::fs::create_dir_all(dst_dir).is_err() {
        return migrated;
    }
    if !dst_dir.is_dir() {
        return migrated;
    }
    for entry in entries.flatten() {
        let src = entry.path();
        if src.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(name) = src.file_name() else {
            continue;
        };
        let dst = dst_dir.join(name);
        if std::fs::copy(&src, &dst).is_err() {
            continue;
        }
        if std::fs::remove_file(&src).is_ok() {
            migrated.push(src);
        }
    }
    migrated
}

/// Keep the most-recent `MAX_RETAINED_LOGS` microreports under
/// `AEGIS_ISOS/aegis-boot-logs/`; delete older ones. Returns the
/// number of files deleted. No-op when the partition isn't
/// mounted.
pub(crate) fn rotate_aegis_isos() -> usize {
    rotate_dir(&aegis_isos_log_dir(), MAX_RETAINED_LOGS)
}

fn rotate_dir(dir: &Path, keep_n: usize) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut logs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("json"))
        .collect();
    if logs.len() <= keep_n {
        return 0;
    }
    // Sort by filename — the collected_at timestamp prefix makes
    // alphabetic order match chronological order. Keep the tail
    // (most recent); delete the head.
    logs.sort();
    let to_delete = logs.len().saturating_sub(keep_n);
    let mut deleted = 0;
    for path in logs.iter().take(to_delete) {
        if std::fs::remove_file(path).is_ok() {
            deleted += 1;
        }
    }
    deleted
}

/// High-level integration hook for the rescue-tui F10 save-log path.
/// Classifies the current error state, builds the Tier-A microreport,
/// writes it (`AEGIS_ISOS` preferred, tmpfs fallback), migrates any
/// previously-spooled tmpfs logs, and rotates to the retention
/// ceiling.
///
/// Best-effort: any failure along the way is logged via `tracing`
/// and swallowed — the rescue flow must never block on bug-report
/// bookkeeping.
///
/// `full_error_text` is the operator-visible error string (from
/// `State::error_evidence_text`). The raw text does NOT leave the
/// target; only its sha256 goes into the Tier-A envelope.
pub fn record_failure(full_error_text: &str, failure_class: &str, boot_step_reached: &str) {
    let collected_at = iso8601_now();
    let sys_vendor = read_dmi_field("sys_vendor");
    let bios_date = read_dmi_field("bios_date");
    let report = build_microreport(
        env!("CARGO_PKG_VERSION"),
        sys_vendor.as_deref(),
        bios_date.as_deref(),
        boot_step_reached,
        failure_class,
        full_error_text,
        &collected_at,
    );
    match write(&report) {
        Ok(path) => tracing::info!(
            path = %path.display(),
            tier = %report.tier,
            class = %report.failure_class,
            "failure microreport written"
        ),
        Err(e) => tracing::warn!(error = %e, "failure microreport write failed"),
    }
    // Opportunistic drain-and-rotate. Safe to call every time; both
    // are no-ops when the partition isn't mounted or under the
    // retention ceiling.
    let migrated = migrate_tmpfs_to_aegis_isos();
    if !migrated.is_empty() {
        tracing::info!(
            count = migrated.len(),
            "migrated pending failure microreports from tmpfs → AEGIS_ISOS"
        );
    }
    let rotated = rotate_aegis_isos();
    if rotated > 0 {
        tracing::info!(count = rotated, "rotated oldest microreports");
    }
}

fn read_dmi_field(field: &str) -> Option<String> {
    let path = format!("/sys/class/dmi/id/{field}");
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn iso8601_now() -> String {
    // Same shell-out pattern as bug_report.rs / attest.rs. Produces
    // RFC-3339 UTC without taking on a chrono / jiff runtime dep.
    let output = std::process::Command::new("date")
        .args(["-u", "+%Y-%m-%dT%H:%M:%SZ"])
        .output()
        .ok()
        .filter(|o| o.status.success());
    match output {
        Some(o) => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        None => "1970-01-01T00:00:00Z".to_string(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn fixture_report(ts: &str) -> FailureMicroreport {
        build_microreport(
            "0.15.0",
            Some("Framework"),
            Some("09/18/2025"),
            "rescue_tui",
            "kexec_signature_rejected",
            "kexec: security: image signature rejection, errno 61",
            ts,
        )
    }

    #[test]
    fn vendor_family_extracts_lowercase_first_token() {
        assert_eq!(vendor_family_of(Some("Framework")), "framework");
        assert_eq!(vendor_family_of(Some("Lenovo")), "lenovo");
        assert_eq!(
            vendor_family_of(Some("LENOVO Think Centre")),
            "lenovo",
            "lowercased + first-token"
        );
    }

    #[test]
    fn vendor_family_handles_missing_and_placeholder() {
        assert_eq!(vendor_family_of(None), "unknown");
        assert_eq!(vendor_family_of(Some("")), "unknown");
        assert_eq!(vendor_family_of(Some("To be filled by O.E.M.")), "unknown");
        assert_eq!(vendor_family_of(Some("Default string")), "unknown");
    }

    #[test]
    fn bios_year_parses_common_date_forms() {
        assert_eq!(bios_year_of(Some("09/18/2025")), "2025");
        assert_eq!(bios_year_of(Some("2024-04-16")), "2024");
        assert_eq!(bios_year_of(None), "unknown");
        assert_eq!(bios_year_of(Some("nonsense")), "unknown");
    }

    #[test]
    fn opaque_hash_is_deterministic_and_prefixed() {
        let a = opaque_hash("errno 61");
        let b = opaque_hash("errno 61");
        let c = opaque_hash("errno 62");
        assert_eq!(a, b, "same input same hash");
        assert_ne!(a, c, "different input different hash");
        assert!(a.starts_with("sha256:"));
        assert_eq!(a.len(), "sha256:".len() + 64);
    }

    #[test]
    fn build_microreport_fills_schema_and_tier() {
        let r = fixture_report("2026-04-20T12:34:56Z");
        assert_eq!(r.schema_version, FAILURE_MICROREPORT_SCHEMA_VERSION);
        assert_eq!(r.tier, "A");
        assert_eq!(r.vendor_family, "framework");
        assert_eq!(r.bios_year, "2025");
        assert_eq!(r.boot_step_reached, "rescue_tui");
        assert_eq!(r.failure_class, "kexec_signature_rejected");
        assert!(r.failure_hash.starts_with("sha256:"));
    }

    #[test]
    fn serialize_produces_stable_json() {
        let r = fixture_report("2026-04-20T12:34:56Z");
        let body = serialize(&r).unwrap();
        assert!(body.contains("\"schema_version\": 1"));
        assert!(body.contains("\"tier\": \"A\""));
        assert!(body.contains("\"vendor_family\": \"framework\""));
        assert!(body.contains("\"bios_year\": \"2025\""));
        // Pretty-printed JSON: newline + indentation.
        assert!(body.contains("\n  "));
    }

    #[test]
    fn write_and_migrate_roundtrip() {
        // Use temp dirs so the test doesn't touch the real system
        // paths. `migrate_between` takes the src/dst explicitly.
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();

        // Seed tmpfs spool with two log files + one unrelated file.
        std::fs::write(src.path().join("2026-04-20T12:00:00Z-abc123.json"), b"{}").unwrap();
        std::fs::write(src.path().join("2026-04-20T12:05:00Z-def456.json"), b"{}").unwrap();
        std::fs::write(src.path().join("unrelated.txt"), b"not a log").unwrap();

        let migrated = migrate_between(src.path(), dst.path());
        assert_eq!(migrated.len(), 2, "two json files migrated");

        // Both json files should now be in dst, the txt file should
        // still be in src.
        assert!(dst
            .path()
            .join("2026-04-20T12:00:00Z-abc123.json")
            .is_file());
        assert!(dst
            .path()
            .join("2026-04-20T12:05:00Z-def456.json")
            .is_file());
        assert!(src.path().join("unrelated.txt").is_file());
        assert!(!src
            .path()
            .join("2026-04-20T12:00:00Z-abc123.json")
            .is_file());
    }

    #[test]
    fn rotate_dir_keeps_last_n() {
        let dir = tempfile::tempdir().unwrap();
        // Write 13 logs with monotonically-increasing timestamps.
        for i in 0..13 {
            let ts = format!("2026-04-20T12:{i:02}:00Z");
            let hash = format!("{i:012x}");
            std::fs::write(dir.path().join(format!("{ts}-{hash}.json")), b"{}").unwrap();
        }
        let deleted = rotate_dir(dir.path(), 10);
        assert_eq!(deleted, 3, "13 - 10 keep_n = 3 deleted");
        let remaining: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().into_string().unwrap())
            .collect();
        assert_eq!(remaining.len(), 10);
        // The three oldest (12:00, 12:01, 12:02) should be gone;
        // the 10 most recent (12:03..12:12) should remain.
        for kept in &remaining {
            assert!(
                !kept.starts_with("2026-04-20T12:00")
                    && !kept.starts_with("2026-04-20T12:01")
                    && !kept.starts_with("2026-04-20T12:02"),
                "oldest 3 should have been deleted, saw {kept}"
            );
        }
    }

    #[test]
    fn rotate_dir_noop_when_under_limit() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..5 {
            std::fs::write(
                dir.path()
                    .join(format!("2026-04-20T12:0{i}:00Z-{i:012x}.json")),
                b"{}",
            )
            .unwrap();
        }
        assert_eq!(rotate_dir(dir.path(), 10), 0);
    }

    #[test]
    fn try_write_into_creates_parent() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("nested/logs");
        try_write_into(&dir, "x.json", b"{}").unwrap();
        assert!(dir.join("x.json").is_file());
    }

    #[test]
    fn short_hash_takes_12_after_prefix() {
        assert_eq!(short_hash("sha256:abcdef0123456789abcdef"), "abcdef012345");
        assert_eq!(short_hash("abcdef0123456789"), "abcdef012345");
    }
}
