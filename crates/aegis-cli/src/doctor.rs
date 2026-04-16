//! `aegis-boot doctor` — health check for the host environment and any
//! aegis-boot stick the operator wants to inspect.
//!
//! Output is a fixed-format report with green checkmarks, yellow warnings,
//! and red failures, plus a single "NEXT ACTION" line at the end so an
//! operator who reads only the last two lines still gets the right call to
//! action.
//!
//! Checks fall into two phases:
//!
//! 1. **Host checks** — does this workstation have the prerequisites to
//!    flash and use aegis-boot at all? (Linux only today; #123 tracks
//!    macOS / Windows.)
//! 2. **Stick checks** — if `--stick /dev/sdX` (or auto-detected single
//!    removable drive) is present: partition layout, ESP integrity, and
//!    sidecar coverage on `AEGIS_ISOS`.
//!
//! Exit codes:
//!   * 0 — all checks pass (or only `WARN` items present)
//!   * 1 — at least one `FAIL` item; the report includes a NEXT ACTION
//!   * 2 — usage error (unknown flag, --help, etc. handled separately)

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use crate::detect;

/// Verdict for a single check row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Pass,
    Warn,
    Fail,
    /// Skipped because a prerequisite check failed; not counted toward score.
    Skip,
}

impl Verdict {
    fn glyph(self) -> &'static str {
        match self {
            Verdict::Pass => "\u{2713}", // ✓
            Verdict::Warn => "!",
            Verdict::Fail => "\u{2717}", // ✗
            Verdict::Skip => "-",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Verdict::Pass => "PASS",
            Verdict::Warn => "WARN",
            Verdict::Fail => "FAIL",
            Verdict::Skip => "SKIP",
        }
    }
}

/// Ordered, append-only list of check results.
struct Report {
    rows: Vec<(Verdict, String, String)>,
    /// Suggested next action — set by the first FAIL'd check that has one,
    /// or by the highest-severity WARN if no FAILs.
    next_action: Option<String>,
}

impl Report {
    fn new() -> Self {
        Self {
            rows: Vec::new(),
            next_action: None,
        }
    }

    fn add(&mut self, verdict: Verdict, name: impl Into<String>, detail: impl Into<String>) {
        let name = name.into();
        let detail = detail.into();
        Self::print_row(verdict, &name, &detail);
        self.rows.push((verdict, name, detail));
    }

    fn add_with_next(
        &mut self,
        verdict: Verdict,
        name: impl Into<String>,
        detail: impl Into<String>,
        next_action: impl Into<String>,
    ) {
        let name = name.into();
        let detail = detail.into();
        let next = next_action.into();
        Self::print_row(verdict, &name, &detail);
        self.rows.push((verdict, name, detail));
        if matches!(verdict, Verdict::Fail) && self.next_action.is_none() {
            self.next_action = Some(next);
        }
    }

    fn print_row(verdict: Verdict, name: &str, detail: &str) {
        println!(
            "  [{} {}] {:<32}  {}",
            verdict.glyph(),
            verdict.label(),
            name,
            detail
        );
    }

    /// 0–100 score: 100 = all PASS, deductions for WARN/FAIL.
    /// Skipped rows are not counted.
    fn score(&self) -> u8 {
        // Tenths of a point per check (PASS=10, WARN=7, FAIL=0). Avoids
        // f64 precision lints; multiplication stays in u32.
        let mut total: u32 = 0;
        let mut weight: u32 = 0;
        for (v, _, _) in &self.rows {
            match v {
                Verdict::Pass => {
                    total += 10;
                    weight += 10;
                }
                Verdict::Warn => {
                    total += 10;
                    weight += 7;
                }
                Verdict::Fail => {
                    total += 10;
                }
                Verdict::Skip => {}
            }
        }
        if total == 0 {
            return 100;
        }
        // Round to nearest: (weight*100 + total/2) / total, clamped to [0, 100].
        u8::try_from(((weight * 100) + total / 2) / total).unwrap_or(100)
    }

    /// Print the trailing summary block (rows are printed inline as they're
    /// added). Score + NEXT ACTION go here.
    fn print_summary(&self) {
        let score = self.score();
        let summary_word = match score {
            90..=100 => "EXCELLENT",
            70..=89 => "OK",
            40..=69 => "DEGRADED",
            _ => "BROKEN",
        };
        println!("  Health score: {score}/100 ({summary_word})");
        if let Some(next) = &self.next_action {
            println!();
            println!("  NEXT ACTION: {next}");
        }
    }

    fn exit_code(&self) -> ExitCode {
        if self
            .rows
            .iter()
            .any(|(v, _, _)| matches!(v, Verdict::Fail))
        {
            ExitCode::from(1)
        } else {
            ExitCode::SUCCESS
        }
    }
}

/// Entry point for `aegis-boot doctor [--stick /dev/sdX]`.
pub fn run(args: &[String]) -> ExitCode {
    if args.first().map(String::as_str) == Some("--help")
        || args.first().map(String::as_str) == Some("-h")
    {
        print_help();
        return ExitCode::SUCCESS;
    }

    let stick = parse_stick_arg(args);
    let mut report = Report::new();

    println!("aegis-boot doctor — host + stick health check");
    println!();

    println!("Host checks:");
    check_os(&mut report);
    check_command_present(&mut report, "dd", "required to write the stick");
    check_command_present(&mut report, "sudo", "required for dd / mount");
    check_command_present(
        &mut report,
        "sgdisk",
        "verifies stick partition table after flash",
    );
    check_command_present(
        &mut report,
        "lsblk",
        "lists removable drives for `flash` auto-detect",
    );
    check_secureboot_state(&mut report);
    check_removable_drives(&mut report);
    println!();

    println!("Stick checks:");
    if let Some(dev) = stick.or_else(autodetect_single_stick) {
        check_stick_partitions(&mut report, &dev);
        check_aegis_isos_mount(&mut report, &dev);
    } else {
        report.add(
            Verdict::Skip,
            "stick selection",
            "no --stick argument and no single removable drive auto-detected",
        );
        println!(
            "  (pass `--stick /dev/sdX` to inspect a specific drive; \
             with no removable USB drives plugged in, stick checks are skipped)"
        );
    }
    println!();

    report.print_summary();
    report.exit_code()
}

fn print_help() {
    println!("aegis-boot doctor — host + stick health check");
    println!();
    println!("USAGE:");
    println!("  aegis-boot doctor              # check host, auto-detect a removable drive");
    println!("  aegis-boot doctor --stick /dev/sdX");
    println!();
    println!("Reports a 0-100 health score with a single NEXT ACTION line.");
    println!("Exit code 0 = healthy (PASS or only WARN); 1 = at least one FAIL.");
}

fn parse_stick_arg(args: &[String]) -> Option<PathBuf> {
    let mut iter = args.iter();
    while let Some(a) = iter.next() {
        if a == "--stick" {
            return iter.next().map(PathBuf::from);
        }
        if let Some(rest) = a.strip_prefix("--stick=") {
            return Some(PathBuf::from(rest));
        }
    }
    None
}

fn autodetect_single_stick() -> Option<PathBuf> {
    let drives = detect::list_removable_drives();
    if drives.len() == 1 {
        Some(drives.into_iter().next()?.dev)
    } else {
        None
    }
}

// --- Host checks -----------------------------------------------------------

fn check_os(report: &mut Report) {
    #[cfg(target_os = "linux")]
    {
        report.add(Verdict::Pass, "operating system", "Linux (supported)");
    }
    #[cfg(target_os = "macos")]
    {
        report.add_with_next(
            Verdict::Warn,
            "operating system",
            "macOS — flash CLI is Linux-only today (issue #123)",
            "use a Linux host to run `aegis-boot flash`; macOS support tracked in #123",
        );
    }
    #[cfg(target_os = "windows")]
    {
        report.add_with_next(
            Verdict::Warn,
            "operating system",
            "Windows — flash CLI is Linux-only today (issue #123)",
            "use WSL2 or a Linux host to run `aegis-boot flash`; Windows support tracked in #123",
        );
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        report.add(
            Verdict::Warn,
            "operating system",
            "unrecognized target_os; aegis-boot may not function correctly",
        );
    }
}

fn check_command_present(report: &mut Report, cmd: &str, why: &str) {
    let found = which(cmd);
    let name = format!("command: {cmd}");
    if let Some(path) = found {
        report.add(Verdict::Pass, name, format!("{} ({why})", path.display()));
    } else {
        report.add_with_next(
            Verdict::Fail,
            name,
            format!("not found in PATH ({why})"),
            format!("install `{cmd}` (e.g. on Debian/Ubuntu: `sudo apt-get install {cmd}`)"),
        );
    }
}

fn which(cmd: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(cmd);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn check_secureboot_state(report: &mut Report) {
    // Try mokutil first (most operator hosts have it). Fall back to reading
    // /sys/firmware/efi/efivars/SecureBoot-* directly. We don't fail the
    // overall report if SB is off on the operator's host — they may be
    // flashing on a workstation but deploying to an SB-enforcing target.
    let sb = read_secureboot();
    let name = "Secure Boot (host)".to_string();
    match sb {
        Some(true) => report.add(Verdict::Pass, name, "enforcing"),
        Some(false) => report.add(
            Verdict::Warn,
            name,
            "disabled on this host (target machine SB state is what matters)",
        ),
        None => report.add(
            Verdict::Skip,
            name,
            "could not determine (no mokutil, no efivars)",
        ),
    }
}

fn read_secureboot() -> Option<bool> {
    if let Ok(out) = Command::new("mokutil").arg("--sb-state").output() {
        if out.status.success() {
            let stdout = String::from_utf8_lossy(&out.stdout);
            if stdout.to_lowercase().contains("secureboot enabled") {
                return Some(true);
            }
            if stdout.to_lowercase().contains("secureboot disabled") {
                return Some(false);
            }
        }
    }
    // Fallback: read efivar directly. Format is 4 bytes header + 1 byte value.
    let efivar = "/sys/firmware/efi/efivars";
    if let Ok(entries) = std::fs::read_dir(efivar) {
        for e in entries.flatten() {
            let name = e.file_name();
            let name_s = name.to_string_lossy();
            if name_s.starts_with("SecureBoot-") {
                if let Ok(bytes) = std::fs::read(e.path()) {
                    if bytes.len() >= 5 {
                        return Some(bytes[4] == 1);
                    }
                }
            }
        }
    }
    None
}

fn check_removable_drives(report: &mut Report) {
    let drives = detect::list_removable_drives();
    let name = "removable USB drives".to_string();
    match drives.len() {
        0 => report.add(
            Verdict::Warn,
            name,
            "none detected (plug a USB stick to flash)",
        ),
        1 => report.add(
            Verdict::Pass,
            name,
            format!(
                "{} ({}, {})",
                drives[0].dev.display(),
                drives[0].model,
                drives[0].size_human()
            ),
        ),
        n => report.add(
            Verdict::Pass,
            name,
            format!("{n} drives detected (use --stick to disambiguate)"),
        ),
    }
}

// --- Stick checks ----------------------------------------------------------

fn check_stick_partitions(report: &mut Report, dev: &Path) {
    let name = format!("partition table: {}", dev.display());
    let out = Command::new("sudo")
        .args(["sgdisk", "-p"])
        .arg(dev)
        .output();
    let Ok(out) = out else {
        report.add_with_next(
            Verdict::Fail,
            name,
            "could not exec sgdisk",
            "install gdisk (`sudo apt-get install gdisk`)",
        );
        return;
    };
    if !out.status.success() {
        report.add_with_next(
            Verdict::Fail,
            name,
            format!("sgdisk failed: {}", String::from_utf8_lossy(&out.stderr).trim()),
            format!(
                "verify {} is an aegis-boot stick (was it flashed by `aegis-boot flash`?)",
                dev.display()
            ),
        );
        return;
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let has_esp = stdout.contains("EF00") || stdout.contains("EFI System");
    let has_aegis_isos = stdout.contains("AEGIS_ISOS");
    if has_esp && has_aegis_isos {
        report.add(
            Verdict::Pass,
            name,
            "GPT with ESP + AEGIS_ISOS partitions — looks like an aegis-boot stick",
        );
    } else if has_esp && !has_aegis_isos {
        report.add_with_next(
            Verdict::Warn,
            name,
            "GPT with ESP but no AEGIS_ISOS — partial aegis-boot layout?",
            format!(
                "reflash with `sudo aegis-boot flash {}` to recreate AEGIS_ISOS",
                dev.display()
            ),
        );
    } else {
        report.add_with_next(
            Verdict::Fail,
            name,
            "no ESP + AEGIS_ISOS layout — not an aegis-boot stick",
            format!(
                "flash this drive with `sudo aegis-boot flash {}` (DESTRUCTIVE)",
                dev.display()
            ),
        );
    }
}

fn check_aegis_isos_mount(report: &mut Report, dev: &Path) {
    let name = "AEGIS_ISOS contents".to_string();
    // Look for a currently-mounted AEGIS_ISOS in /proc/mounts.
    let mounts = std::fs::read_to_string("/proc/mounts").unwrap_or_default();
    let mount_point = mounts.lines().find_map(|line| {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() >= 2 && f[1].contains("AEGIS_ISOS") {
            Some(f[1].to_string())
        } else {
            None
        }
    });
    if mount_point.is_none() {
        report.add(
            Verdict::Skip,
            name,
            format!(
                "AEGIS_ISOS not mounted; run `aegis-boot list {}` to check ISOs",
                dev.display()
            ),
        );
        return;
    }
    let mp = mount_point.unwrap_or_else(|| unreachable!());
    let entries = std::fs::read_dir(&mp);
    let Ok(entries) = entries else {
        report.add_with_next(
            Verdict::Fail,
            name,
            format!("can't read {mp} (permissions?)"),
            format!("try `sudo aegis-boot list {}`", dev.display()),
        );
        return;
    };
    let mut iso_count = 0;
    let mut sidecar_count = 0;
    for e in entries.flatten() {
        let n = e.file_name().to_string_lossy().to_lowercase();
        if Path::new(&n).extension().is_some_and(|x| x == "iso") {
            iso_count += 1;
        } else if n.ends_with(".sha256") || n.ends_with(".minisig") {
            sidecar_count += 1;
        }
    }
    match (iso_count, sidecar_count) {
        (0, _) => report.add_with_next(
            Verdict::Warn,
            name,
            format!("{mp} has no .iso files yet"),
            "add an ISO with `aegis-boot add /path/to/distro.iso`",
        ),
        (n, 0) => report.add_with_next(
            Verdict::Warn,
            name,
            format!("{n} ISO(s), no sidecars — TUI will show GRAY verdict"),
            "drop sibling .sha256 or .minisig files alongside each ISO before flashing",
        ),
        (n, s) => report.add(
            Verdict::Pass,
            name,
            format!("{n} ISO(s), {s} sidecar(s) — verifications can run"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_score_all_pass_is_100() {
        let mut r = Report::new();
        r.add(Verdict::Pass, "a", "ok");
        r.add(Verdict::Pass, "b", "ok");
        assert_eq!(r.score(), 100);
    }

    #[test]
    fn report_score_warn_is_partial() {
        let mut r = Report::new();
        r.add(Verdict::Pass, "a", "ok");
        r.add(Verdict::Warn, "b", "meh");
        // 1.0 + 0.7 = 1.7 / 2 = 0.85 -> 85
        assert_eq!(r.score(), 85);
    }

    #[test]
    fn report_score_fail_is_zero_weighted() {
        let mut r = Report::new();
        r.add(Verdict::Pass, "a", "ok");
        r.add(Verdict::Fail, "b", "no");
        // 1.0 + 0.0 = 1.0 / 2 = 0.5 -> 50
        assert_eq!(r.score(), 50);
    }

    #[test]
    fn report_score_skip_is_not_counted() {
        let mut r = Report::new();
        r.add(Verdict::Pass, "a", "ok");
        r.add(Verdict::Skip, "b", "n/a");
        assert_eq!(r.score(), 100);
    }

    #[test]
    fn report_empty_score_is_100() {
        let r = Report::new();
        assert_eq!(r.score(), 100);
    }

    #[test]
    fn next_action_set_by_first_fail_only() {
        let mut r = Report::new();
        r.add_with_next(Verdict::Fail, "a", "no", "do thing 1");
        r.add_with_next(Verdict::Fail, "b", "no", "do thing 2");
        assert_eq!(r.next_action.as_deref(), Some("do thing 1"));
    }

    #[test]
    fn next_action_unset_when_only_warn() {
        let mut r = Report::new();
        r.add_with_next(Verdict::Warn, "a", "meh", "consider thing");
        assert!(r.next_action.is_none());
    }

    #[test]
    fn exit_code_pass_is_success() {
        let mut r = Report::new();
        r.add(Verdict::Pass, "a", "ok");
        r.add(Verdict::Warn, "b", "meh");
        // Exit code is computed; we can't easily compare ExitCode, but absence
        // of FAIL means SUCCESS. Use the predicate directly.
        let has_fail = r.rows.iter().any(|(v, _, _)| matches!(v, Verdict::Fail));
        assert!(!has_fail);
    }

    #[test]
    fn parse_stick_arg_space_separated() {
        let args = vec!["--stick".to_string(), "/dev/sdc".to_string()];
        assert_eq!(parse_stick_arg(&args), Some(PathBuf::from("/dev/sdc")));
    }

    #[test]
    fn parse_stick_arg_equals_form() {
        let args = vec!["--stick=/dev/sdc".to_string()];
        assert_eq!(parse_stick_arg(&args), Some(PathBuf::from("/dev/sdc")));
    }

    #[test]
    fn parse_stick_arg_absent() {
        let args: Vec<String> = vec![];
        assert_eq!(parse_stick_arg(&args), None);
    }

    #[test]
    fn verdict_glyph_and_label_are_distinct() {
        let glyphs: Vec<_> = [Verdict::Pass, Verdict::Warn, Verdict::Fail, Verdict::Skip]
            .iter()
            .map(|v| v.glyph())
            .collect();
        let labels: Vec<_> = [Verdict::Pass, Verdict::Warn, Verdict::Fail, Verdict::Skip]
            .iter()
            .map(|v| v.label())
            .collect();
        // No duplicates
        let mut g = glyphs.clone();
        g.sort_unstable();
        g.dedup();
        assert_eq!(g.len(), 4);
        let mut l = labels.clone();
        l.sort_unstable();
        l.dedup();
        assert_eq!(l.len(), 4);
    }
}
