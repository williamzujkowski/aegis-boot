// SPDX-License-Identifier: MIT OR Apache-2.0

//! `aegis-fitness` — repo, build, and artifact health audit for aegis-boot.
//!
//! Runs a fixed set of cheap checks against the working tree and reports a
//! /100 score. Inspired by `nexus-agents fitness-audit`. Designed to be
//! callable from CI and from `dev-test.sh` so regressions in repo hygiene
//! surface alongside test failures.
//!
//! Output formats:
//!   * default — colorless human table + score footer
//!   * `--json` — machine-readable for CI gating
//!   * `--list-checks` — just print the registry (id + weight) and exit
//!
//! Exit codes:
//!   * 0 — score >= threshold (default 90)
//!   * 1 — score < threshold
//!   * 2 — internal error (couldn't even run checks)

#![forbid(unsafe_code)]

use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde::Serialize;

const DEFAULT_THRESHOLD: u32 = 90;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Status {
    Pass,
    Warn,
    Fail,
}

#[derive(Debug, Clone, Serialize)]
struct CheckResult {
    name: &'static str,
    status: Status,
    weight: u32,
    detail: String,
}

#[derive(Debug, Serialize)]
struct Report {
    score: u32,
    threshold: u32,
    passed: bool,
    checks: Vec<CheckResult>,
}

/// Registry entry for a fitness check.
///
/// Each check is a stateless `(name, weight, run)` triple. Adding a new
/// check is a one-line append to [`CHECKS`] plus a small `fn(&Path) ->
/// (Status, String)`. #601 chose this fn-pointer table over the
/// `trait Check { ... }` + unit-struct shape suggested in the issue
/// because the checks carry no per-check state — the data table is the
/// simpler form, doesn't need dyn dispatch, and keeps the inventory
/// grep-friendly in one place.
struct Check {
    /// Human-readable label shown in the audit table and emitted to JSON.
    /// Treat as stable: a name change is a CLI break for any tool that
    /// greps the report.
    name: &'static str,
    /// Score weight; the sum across all checks is the /100 denominator.
    weight: u32,
    /// Execute the check against the repo root. Returns the verdict +
    /// a one-line detail message rendered into the audit table.
    run: fn(&Path) -> (Status, String),
}

/// Single source of truth for the fitness audit inventory. New checks
/// are added here; the registry order is also the report row order.
const CHECKS: &[Check] = &[
    Check {
        name: "Cargo.lock committed",
        weight: 10,
        run: check_cargo_lock,
    },
    Check {
        name: "Workspace crates present",
        weight: 15,
        run: check_required_crates,
    },
    Check {
        name: "No tracked secrets at root",
        weight: 10,
        run: check_no_secrets,
    },
    Check {
        name: "initramfs.cpio.gz built",
        weight: 15,
        run: check_initramfs_artifact,
    },
    Check {
        name: "initramfs size <= 20 MiB",
        weight: 10,
        run: check_initramfs_size_budget,
    },
    Check {
        name: "initramfs sha256 sidecar",
        weight: 5,
        run: check_initramfs_checksum,
    },
    Check {
        name: "Required scripts present",
        weight: 15,
        run: check_scripts_present,
    },
    Check {
        name: "CHANGELOG.md present",
        weight: 10,
        run: check_changelog_current,
    },
    Check {
        name: "Operator docs present",
        weight: 10,
        run: check_docs_present,
    },
];

fn main() -> ExitCode {
    // Standard CLI dispatch. argv[0] is dropped (`.skip(1)`); remaining
    // args are flag names the user already controls. No security
    // decision keys off argv.
    // nosemgrep: rust.lang.security.args.args
    let args: Vec<String> = env::args().skip(1).collect();
    let json_mode = args.iter().any(|a| a == "--json");
    let list_mode = args.iter().any(|a| a == "--list-checks");
    let threshold = args
        .iter()
        .position(|a| a == "--threshold")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(DEFAULT_THRESHOLD);
    if args.iter().any(|a| a == "-h" || a == "--help") {
        print_help();
        return ExitCode::SUCCESS;
    }

    if list_mode {
        print_check_inventory();
        return ExitCode::SUCCESS;
    }

    let Some(repo_root) = find_repo_root() else {
        eprintln!("aegis-fitness: not inside an aegis-boot checkout (no Cargo.toml)");
        return ExitCode::from(2);
    };

    let checks = run_checks(&repo_root);
    let score = compute_score(&checks);
    let report = Report {
        score,
        threshold,
        passed: score >= threshold,
        checks,
    };

    if json_mode {
        if let Ok(s) = serde_json::to_string_pretty(&report) {
            println!("{s}");
        } else {
            eprintln!("aegis-fitness: serialization failed");
            return ExitCode::from(2);
        }
    } else {
        print_human(&report);
    }

    if report.passed {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn print_help() {
    println!("aegis-fitness — repo / build / artifact health audit");
    println!();
    println!("USAGE:");
    println!("  aegis-fitness [--json] [--threshold N] [--list-checks]");
    println!();
    println!("OPTIONS:");
    println!("  --json             machine-readable output");
    println!("  --threshold N      pass threshold (default {DEFAULT_THRESHOLD})");
    println!("  --list-checks      print the check registry and exit (no scoring)");
    println!("  -h, --help         this message");
}

/// Print the static check inventory (name + weight) without scoring.
/// Useful for CI dashboards or `aegis-fitness --list-checks | wc -l`
/// drift assertions in tooling.
fn print_check_inventory() {
    println!("aegis-fitness — check inventory ({} checks)", CHECKS.len());
    println!("{}", "─".repeat(60));
    let total: u32 = CHECKS.iter().map(|c| c.weight).sum();
    for c in CHECKS {
        println!("  ({:>2}pt) {}", c.weight, c.name);
    }
    println!("{}", "─".repeat(60));
    println!("  total weight: {total}");
}

fn find_repo_root() -> Option<PathBuf> {
    let mut cur = env::current_dir().ok()?;
    loop {
        if cur.join("Cargo.toml").is_file() && cur.join("crates").is_dir() {
            return Some(cur);
        }
        if !cur.pop() {
            return None;
        }
    }
}

/// Walk the static [`CHECKS`] registry and assemble a [`CheckResult`]
/// for each. The output order matches the registry order so the audit
/// table is deterministic across runs.
fn run_checks(root: &Path) -> Vec<CheckResult> {
    CHECKS
        .iter()
        .map(|c| {
            let (status, detail) = (c.run)(root);
            CheckResult {
                name: c.name,
                status,
                weight: c.weight,
                detail,
            }
        })
        .collect()
}

// ---- check implementations --------------------------------------------------
//
// Each `check_*` returns `(Status, String)`. The `name` and `weight` live in
// the `CHECKS` registry above so they can't drift between sites and so a
// `--list-checks` view is trivial.

fn check_cargo_lock(root: &Path) -> (Status, String) {
    let path = root.join("Cargo.lock");
    if path.is_file() {
        (Status::Pass, "present at workspace root".to_string())
    } else {
        (
            Status::Fail,
            "missing — required for reproducible builds".to_string(),
        )
    }
}

fn check_required_crates(root: &Path) -> (Status, String) {
    let required = [
        "iso-parser",
        "iso-probe",
        "kexec-loader",
        "rescue-tui",
        "aegis-fitness",
    ];
    let missing: Vec<&str> = required
        .iter()
        .filter(|c| !root.join("crates").join(c).join("Cargo.toml").is_file())
        .copied()
        .collect();
    if missing.is_empty() {
        (
            Status::Pass,
            format!("all {} expected crates present", required.len()),
        )
    } else {
        (Status::Fail, format!("missing: {}", missing.join(", ")))
    }
}

fn check_no_secrets(root: &Path) -> (Status, String) {
    let bad = [".env", ".env.local", "secrets.toml", "id_rsa"];
    let found: Vec<&str> = bad
        .iter()
        .filter(|f| root.join(f).exists())
        .copied()
        .collect();
    if found.is_empty() {
        (
            Status::Pass,
            "no .env / id_rsa / secrets.toml present".to_string(),
        )
    } else {
        (Status::Fail, format!("present: {}", found.join(", ")))
    }
}

fn check_initramfs_artifact(root: &Path) -> (Status, String) {
    let path = root.join("out").join("initramfs.cpio.gz");
    if path.is_file() {
        (Status::Pass, format!("found at {}", path.display()))
    } else {
        (
            Status::Warn,
            "out/initramfs.cpio.gz absent — run scripts/build-initramfs.sh".to_string(),
        )
    }
}

fn check_initramfs_size_budget(root: &Path) -> (Status, String) {
    const BUDGET: u64 = 20 * 1024 * 1024;
    let path = root.join("out").join("initramfs.cpio.gz");
    let Ok(meta) = std::fs::metadata(&path) else {
        return (
            Status::Warn,
            "no artifact to measure (run build-initramfs.sh first)".to_string(),
        );
    };
    let size = meta.len();
    if size <= BUDGET {
        (Status::Pass, format!("{size} bytes / 20 MiB budget"))
    } else {
        (Status::Fail, format!("{size} bytes exceeds 20 MiB budget"))
    }
}

fn check_initramfs_checksum(root: &Path) -> (Status, String) {
    let path = root.join("out").join("initramfs.cpio.gz.sha256");
    if path.is_file() {
        (Status::Pass, "sha256 sidecar present".to_string())
    } else {
        (
            Status::Warn,
            "no sha256 sidecar (build-initramfs.sh writes it)".to_string(),
        )
    }
}

fn check_scripts_present(root: &Path) -> (Status, String) {
    let required = [
        "build-initramfs.sh",
        "mkusb.sh",
        "qemu-try.sh",
        "qemu-kexec-e2e.sh",
        "dev-test.sh",
    ];
    let scripts = root.join("scripts");
    let missing: Vec<&str> = required
        .iter()
        .filter(|s| !scripts.join(s).is_file())
        .copied()
        .collect();
    if missing.is_empty() {
        (
            Status::Pass,
            format!("all {} scripts present", required.len()),
        )
    } else {
        (Status::Fail, format!("missing: {}", missing.join(", ")))
    }
}

fn check_changelog_current(root: &Path) -> (Status, String) {
    let path = root.join("CHANGELOG.md");
    let Ok(content) = std::fs::read_to_string(&path) else {
        return (Status::Fail, "missing".to_string());
    };
    if content.contains("## ") {
        (
            Status::Pass,
            format!("{} bytes, has version sections", content.len()),
        )
    } else {
        (Status::Warn, "no version sections found".to_string())
    }
}

fn check_docs_present(root: &Path) -> (Status, String) {
    let required = ["README.md", "docs/USB_LAYOUT.md", "docs/LOCAL_TESTING.md"];
    let missing: Vec<&str> = required
        .iter()
        .filter(|d| !root.join(d).is_file())
        .copied()
        .collect();
    if missing.is_empty() {
        (Status::Pass, format!("all {} docs present", required.len()))
    } else {
        (Status::Warn, format!("missing: {}", missing.join(", ")))
    }
}

fn compute_score(checks: &[CheckResult]) -> u32 {
    let total: u32 = checks.iter().map(|c| c.weight).sum();
    if total == 0 {
        return 0;
    }
    let earned: u32 = checks
        .iter()
        .map(|c| match c.status {
            Status::Pass => c.weight,
            Status::Warn => c.weight / 2,
            Status::Fail => 0,
        })
        .sum();
    (earned * 100) / total
}

fn print_human(report: &Report) {
    println!("aegis-boot fitness audit");
    println!("{}", "─".repeat(60));
    for c in &report.checks {
        let marker = match c.status {
            Status::Pass => "PASS",
            Status::Warn => "WARN",
            Status::Fail => "FAIL",
        };
        println!("  [{marker}] ({:>2}pt) {} — {}", c.weight, c.name, c.detail);
    }
    println!("{}", "─".repeat(60));
    let verdict = if report.passed { "PASSED" } else { "FAILED" };
    println!(
        "  score: {}/100   threshold: {}   {}",
        report.score, report.threshold, verdict
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(name: &'static str, status: Status, weight: u32) -> CheckResult {
        CheckResult {
            name,
            status,
            weight,
            detail: String::new(),
        }
    }

    #[test]
    fn score_all_pass_is_100() {
        let checks = vec![mk("a", Status::Pass, 50), mk("b", Status::Pass, 50)];
        assert_eq!(compute_score(&checks), 100);
    }

    #[test]
    fn score_all_fail_is_0() {
        let checks = vec![mk("a", Status::Fail, 50), mk("b", Status::Fail, 50)];
        assert_eq!(compute_score(&checks), 0);
    }

    #[test]
    fn score_warn_counts_as_half() {
        let checks = vec![mk("a", Status::Warn, 100)];
        assert_eq!(compute_score(&checks), 50);
    }

    #[test]
    fn score_mixed() {
        let checks = vec![
            mk("a", Status::Pass, 60), // 60
            mk("b", Status::Warn, 20), // 10
            mk("c", Status::Fail, 20), // 0
        ];
        assert_eq!(compute_score(&checks), 70);
    }

    #[test]
    fn score_empty_is_zero() {
        assert_eq!(compute_score(&[]), 0);
    }

    /// #601: the registry is the single source of truth for the audit
    /// inventory. Lock down its size + total weight so reordering or
    /// silent additions during refactors get caught at test time.
    /// Update both numbers in lockstep when intentionally adding a check.
    #[test]
    fn registry_inventory_locked() {
        assert_eq!(
            CHECKS.len(),
            9,
            "CHECKS registry size changed; bump this test deliberately"
        );
        let total: u32 = CHECKS.iter().map(|c| c.weight).sum();
        assert_eq!(
            total, 100,
            "CHECKS weights must sum to 100 (the /100 denominator)"
        );
    }

    /// #601: catch a refactor that accidentally renames a check (which
    /// would break any tooling grepping the report) by asserting the
    /// names against the historical list.
    #[test]
    fn registry_names_are_stable() {
        let names: Vec<&str> = CHECKS.iter().map(|c| c.name).collect();
        assert_eq!(
            names,
            vec![
                "Cargo.lock committed",
                "Workspace crates present",
                "No tracked secrets at root",
                "initramfs.cpio.gz built",
                "initramfs size <= 20 MiB",
                "initramfs sha256 sidecar",
                "Required scripts present",
                "CHANGELOG.md present",
                "Operator docs present",
            ]
        );
    }
}
