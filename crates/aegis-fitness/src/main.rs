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

fn main() -> ExitCode {
    // Standard CLI dispatch. argv[0] is dropped (`.skip(1)`); remaining
    // args are flag names the user already controls. No security
    // decision keys off argv.
    // nosemgrep: rust.lang.security.args.args
    let args: Vec<String> = env::args().skip(1).collect();
    let json_mode = args.iter().any(|a| a == "--json");
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
    println!("  aegis-fitness [--json] [--threshold N]");
    println!();
    println!("OPTIONS:");
    println!("  --json             machine-readable output");
    println!("  --threshold N      pass threshold (default {DEFAULT_THRESHOLD})");
    println!("  -h, --help         this message");
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

fn run_checks(root: &Path) -> Vec<CheckResult> {
    vec![
        check_cargo_lock(root),
        check_required_crates(root),
        check_no_secrets(root),
        check_initramfs_artifact(root),
        check_initramfs_size_budget(root),
        check_initramfs_checksum(root),
        check_scripts_present(root),
        check_changelog_current(root),
        check_docs_present(root),
    ]
}

fn check_cargo_lock(root: &Path) -> CheckResult {
    let path = root.join("Cargo.lock");
    if path.is_file() {
        CheckResult {
            name: "Cargo.lock committed",
            status: Status::Pass,
            weight: 10,
            detail: "present at workspace root".to_string(),
        }
    } else {
        CheckResult {
            name: "Cargo.lock committed",
            status: Status::Fail,
            weight: 10,
            detail: "missing — required for reproducible builds".to_string(),
        }
    }
}

fn check_required_crates(root: &Path) -> CheckResult {
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
        CheckResult {
            name: "Workspace crates present",
            status: Status::Pass,
            weight: 15,
            detail: format!("all {} expected crates present", required.len()),
        }
    } else {
        CheckResult {
            name: "Workspace crates present",
            status: Status::Fail,
            weight: 15,
            detail: format!("missing: {}", missing.join(", ")),
        }
    }
}

fn check_no_secrets(root: &Path) -> CheckResult {
    let bad = [".env", ".env.local", "secrets.toml", "id_rsa"];
    let found: Vec<&str> = bad
        .iter()
        .filter(|f| root.join(f).exists())
        .copied()
        .collect();
    if found.is_empty() {
        CheckResult {
            name: "No tracked secrets at root",
            status: Status::Pass,
            weight: 10,
            detail: "no .env / id_rsa / secrets.toml present".to_string(),
        }
    } else {
        CheckResult {
            name: "No tracked secrets at root",
            status: Status::Fail,
            weight: 10,
            detail: format!("present: {}", found.join(", ")),
        }
    }
}

fn check_initramfs_artifact(root: &Path) -> CheckResult {
    let path = root.join("out").join("initramfs.cpio.gz");
    if path.is_file() {
        CheckResult {
            name: "initramfs.cpio.gz built",
            status: Status::Pass,
            weight: 15,
            detail: format!("found at {}", path.display()),
        }
    } else {
        CheckResult {
            name: "initramfs.cpio.gz built",
            status: Status::Warn,
            weight: 15,
            detail: "out/initramfs.cpio.gz absent — run scripts/build-initramfs.sh".to_string(),
        }
    }
}

fn check_initramfs_size_budget(root: &Path) -> CheckResult {
    const BUDGET: u64 = 20 * 1024 * 1024;
    let path = root.join("out").join("initramfs.cpio.gz");
    let Ok(meta) = std::fs::metadata(&path) else {
        return CheckResult {
            name: "initramfs size <= 20 MiB",
            status: Status::Warn,
            weight: 10,
            detail: "no artifact to measure (run build-initramfs.sh first)".to_string(),
        };
    };
    let size = meta.len();
    if size <= BUDGET {
        CheckResult {
            name: "initramfs size <= 20 MiB",
            status: Status::Pass,
            weight: 10,
            detail: format!("{size} bytes / 20 MiB budget"),
        }
    } else {
        CheckResult {
            name: "initramfs size <= 20 MiB",
            status: Status::Fail,
            weight: 10,
            detail: format!("{size} bytes exceeds 20 MiB budget"),
        }
    }
}

fn check_initramfs_checksum(root: &Path) -> CheckResult {
    let path = root.join("out").join("initramfs.cpio.gz.sha256");
    if path.is_file() {
        CheckResult {
            name: "initramfs sha256 sidecar",
            status: Status::Pass,
            weight: 5,
            detail: "sha256 sidecar present".to_string(),
        }
    } else {
        CheckResult {
            name: "initramfs sha256 sidecar",
            status: Status::Warn,
            weight: 5,
            detail: "no sha256 sidecar (build-initramfs.sh writes it)".to_string(),
        }
    }
}

fn check_scripts_present(root: &Path) -> CheckResult {
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
        CheckResult {
            name: "Required scripts present",
            status: Status::Pass,
            weight: 15,
            detail: format!("all {} scripts present", required.len()),
        }
    } else {
        CheckResult {
            name: "Required scripts present",
            status: Status::Fail,
            weight: 15,
            detail: format!("missing: {}", missing.join(", ")),
        }
    }
}

fn check_changelog_current(root: &Path) -> CheckResult {
    let path = root.join("CHANGELOG.md");
    let Ok(content) = std::fs::read_to_string(&path) else {
        return CheckResult {
            name: "CHANGELOG.md present",
            status: Status::Fail,
            weight: 10,
            detail: "missing".to_string(),
        };
    };
    if content.contains("## ") {
        CheckResult {
            name: "CHANGELOG.md present",
            status: Status::Pass,
            weight: 10,
            detail: format!("{} bytes, has version sections", content.len()),
        }
    } else {
        CheckResult {
            name: "CHANGELOG.md present",
            status: Status::Warn,
            weight: 10,
            detail: "no version sections found".to_string(),
        }
    }
}

fn check_docs_present(root: &Path) -> CheckResult {
    let required = ["README.md", "docs/USB_LAYOUT.md", "docs/LOCAL_TESTING.md"];
    let missing: Vec<&str> = required
        .iter()
        .filter(|d| !root.join(d).is_file())
        .copied()
        .collect();
    if missing.is_empty() {
        CheckResult {
            name: "Operator docs present",
            status: Status::Pass,
            weight: 10,
            detail: format!("all {} docs present", required.len()),
        }
    } else {
        CheckResult {
            name: "Operator docs present",
            status: Status::Warn,
            weight: 10,
            detail: format!("missing: {}", missing.join(", ")),
        }
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
}
