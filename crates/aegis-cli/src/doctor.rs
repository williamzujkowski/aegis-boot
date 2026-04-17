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
    /// When true, `add()` / `add_with_next()` skip the per-row
    /// `println!` so the only stdout content is the final JSON blob
    /// printed by `print_json_summary()`. Leaves stderr alone —
    /// tracing warnings, sudo prompts, and similar still show.
    json_mode: bool,
}

impl Report {
    fn new() -> Self {
        Self {
            rows: Vec::new(),
            next_action: None,
            json_mode: false,
        }
    }

    fn with_json_mode(mut self, json: bool) -> Self {
        self.json_mode = json;
        self
    }

    fn add(&mut self, verdict: Verdict, name: impl Into<String>, detail: impl Into<String>) {
        let name = name.into();
        let detail = detail.into();
        if !self.json_mode {
            Self::print_row(verdict, &name, &detail);
        }
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
        if !self.json_mode {
            Self::print_row(verdict, &name, &detail);
        }
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

    fn has_any_fail(&self) -> bool {
        self.rows.iter().any(|(v, _, _)| matches!(v, Verdict::Fail))
    }

    /// Print the report as a machine-readable JSON document on stdout.
    ///
    /// Schema (stable — downstream CI / monitoring tooling parses
    /// this; breaking changes require a `schema_version` bump):
    ///
    /// ```json
    /// {
    ///   "schema_version": 1,
    ///   "tool_version": "0.14.0-dev",
    ///   "score": 93,
    ///   "band": "EXCELLENT",
    ///   "has_any_fail": false,
    ///   "next_action": null,
    ///   "rows": [
    ///     { "verdict": "PASS", "name": "OS", "detail": "Linux 6.17.0" },
    ///     { "verdict": "WARN", "name": "Secure Boot (host)", "detail": "disabled" }
    ///   ]
    /// }
    /// ```
    ///
    /// Emitted via hand-rolled JSON to avoid a `serde_json` import in
    /// `doctor.rs` — keeps the binary size contribution minimal. Each
    /// string is escaped for `"` and `\`; no embedded newlines
    /// expected in check names/details (but the escaper handles them
    /// safely anyway).
    fn print_json_summary(&self) {
        let score = self.score();
        let band = band_for_score(score);
        println!("{{");
        println!("  \"schema_version\": 1,");
        println!("  \"tool_version\": \"{}\",", env!("CARGO_PKG_VERSION"));
        println!("  \"score\": {score},");
        println!("  \"band\": \"{band}\",");
        println!(
            "  \"has_any_fail\": {},",
            if self.has_any_fail() { "true" } else { "false" }
        );
        match &self.next_action {
            Some(na) => println!("  \"next_action\": \"{}\",", json_escape(na)),
            None => println!("  \"next_action\": null,"),
        }
        println!("  \"rows\": [");
        let last = self.rows.len().saturating_sub(1);
        for (i, (verdict, name, detail)) in self.rows.iter().enumerate() {
            let comma = if i == last { "" } else { "," };
            println!(
                "    {{ \"verdict\": \"{}\", \"name\": \"{}\", \"detail\": \"{}\" }}{comma}",
                verdict.label(),
                json_escape(name),
                json_escape(detail),
            );
        }
        println!("  ]");
        println!("}}");
    }
}

/// JSON band label for a 0–100 score. Extracted so both `print_summary`
/// and `print_json_summary` use the same thresholds.
fn band_for_score(score: u8) -> &'static str {
    match score {
        90..=100 => "EXCELLENT",
        70..=89 => "OK",
        40..=69 => "DEGRADED",
        _ => "BROKEN",
    }
}

/// Minimal JSON string escaper: handles the five characters RFC 8259
/// requires (`"`, `\`, `\n`, `\r`, `\t`), plus control characters
/// below `0x20` as `\u00XX`. Sufficient for check names / details —
/// doctor doesn't carry arbitrary user input. Shared with other
/// `--json` surfaces (`list --json`, `attest --json`) so every
/// aegis-boot structured-output formatter uses the same escape rules.
pub(crate) fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // Control chars beyond \t/\n/\r: emit as \u00XX.
            c if (c as u32) < 0x20 => {
                use std::fmt::Write as _;
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

/// Entry point for `aegis-boot doctor [--stick /dev/sdX]`.
pub fn run(args: &[String]) -> ExitCode {
    match try_run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => ExitCode::from(code),
    }
}

/// Inner runner returning a typed result so `aegis-boot init` can branch
/// on doctor outcome without comparing opaque `ExitCode`s. Semantics
/// match `run`: `Ok(())` on pass, `Err(1)` when any check reported
/// `Verdict::Fail` (i.e. score < 40 in the worst case, though the
/// fail-counter is the real gate).
pub(crate) fn try_run(args: &[String]) -> Result<(), u8> {
    if args.first().map(String::as_str) == Some("--help")
        || args.first().map(String::as_str) == Some("-h")
    {
        print_help();
        return Ok(());
    }

    // --json suppresses the human-readable row-by-row prints and
    // emits a single structured JSON blob at the end. Useful for
    // CI / monitoring / scripted pipelines. Detection is case-
    // sensitive and tolerates the flag appearing anywhere in args.
    let json_mode = args.iter().any(|a| a == "--json");

    let stick = parse_stick_arg(args);
    let mut report = Report::new().with_json_mode(json_mode);

    if !json_mode {
        println!("aegis-boot doctor — host + stick health check");
        println!();
        println!("Host checks:");
    }
    check_os(&mut report);
    check_machine_identity(&mut report);
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
    check_command_present(
        &mut report,
        "curl",
        "downloads catalog ISOs (`aegis-boot fetch`) and the install one-liner",
    );
    check_command_present(
        &mut report,
        "sha256sum",
        "verifies catalog ISO checksums (`aegis-boot fetch`)",
    );
    check_command_present(
        &mut report,
        "gpg",
        "verifies catalog SHA256SUMS signatures (`aegis-boot fetch`)",
    );
    check_secureboot_state(&mut report);
    check_removable_drives(&mut report);
    if !json_mode {
        println!();
        println!("Stick checks:");
    }
    if let Some(dev) = stick.or_else(autodetect_single_stick) {
        check_stick_partitions(&mut report, &dev);
        check_aegis_isos_mount(&mut report, &dev);
    } else {
        report.add(
            Verdict::Skip,
            "stick selection",
            "no --stick argument and no single removable drive auto-detected",
        );
        if !json_mode {
            println!(
                "  (pass `--stick /dev/sdX` to inspect a specific drive; \
                 with no removable USB drives plugged in, stick checks are skipped)"
            );
        }
    }
    if json_mode {
        report.print_json_summary();
    } else {
        println!();
        report.print_summary();
    }
    if report.has_any_fail() {
        Err(1)
    } else {
        Ok(())
    }
}

fn print_help() {
    println!("aegis-boot doctor — host + stick health check");
    println!();
    println!("USAGE:");
    println!("  aegis-boot doctor                      # human-readable table");
    println!("  aegis-boot doctor --stick /dev/sdX     # include stick checks");
    println!("  aegis-boot doctor --json               # machine-readable (CI / monitoring)");
    println!();
    println!("Reports a 0-100 health score with a single NEXT ACTION line.");
    println!("Exit code 0 = healthy (PASS or only WARN); 1 = at least one FAIL.");
    println!();
    println!("JSON schema (stable, schema_version=1):");
    println!("  {{ schema_version, tool_version, score, band, has_any_fail,");
    println!("    next_action, rows: [{{ verdict, name, detail }}, ...] }}");
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

/// Surface machine identity from `/sys/class/dmi/id/` on Linux. This is
/// purely informational (verdict `Pass` / `Skip`), never a failure — the
/// point is to give operators filing a `hardware-report` the exact
/// strings to paste, and to let them cross-check their output against
/// `aegis-boot compat`.
///
/// Fields read (all non-privileged): `sys_vendor`, `product_name`,
/// `product_version`, `bios_vendor`, `bios_version`, `bios_date`.
/// Lenovo puts the human-readable model string in `product_version`;
/// other OEMs usually put it in `product_name`. We prefer the longer
/// non-placeholder value so the row looks like a human would write it.
fn check_machine_identity(report: &mut Report) {
    #[cfg(target_os = "linux")]
    {
        let sys_vendor = read_dmi_field("sys_vendor");
        let product = dmi_product_label();
        let bios = dmi_bios_label();

        if let (Some(v), Some(p)) = (&sys_vendor, &product) {
            let detail = match bios {
                Some(b) => format!("{v} {p} — firmware: {b}"),
                None => format!("{v} {p}"),
            };
            report.add(Verdict::Pass, "machine identity", detail);
            // Immediately cross-check the DB so the compat verdict
            // sits visually next to the identity row.
            check_compat_db_coverage(report, v, p);
        } else {
            report.add(
                Verdict::Skip,
                "machine identity",
                "DMI fields unavailable (placeholder values or /sys/class/dmi/id not present)",
            );
            report.add(
                Verdict::Skip,
                "compat DB coverage",
                "cannot cross-check without machine identity",
            );
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        report.add(
            Verdict::Skip,
            "machine identity",
            "DMI lookup is Linux-only (non-Linux hosts skip this check)",
        );
        report.add(
            Verdict::Skip,
            "compat DB coverage",
            "cross-check is Linux-only (non-Linux hosts skip this check)",
        );
    }
}

/// Look up the host's DMI-derived identity in the in-binary compat DB.
/// This is the final link in the hardware-coverage loop: `doctor` can
/// tell an operator *"your machine is verified"* or *"your machine is
/// not yet documented — here's how to submit a report"* without them
/// running a second command.
///
/// Verdict logic:
///   * `Pass` — a row matched; include the row's level.
///   * `Warn` + next-action — no row matched; hint at `aegis-boot compat`
///     and the report URL.
#[cfg(target_os = "linux")]
fn check_compat_db_coverage(report: &mut Report, vendor: &str, product: &str) {
    // Build a query string the same way an operator would type it.
    // `find_entry` is whitespace-tokenized and requires every token to
    // appear in "vendor model"; vendor+product combined gives a strong
    // signal without being so specific that it misses near-matches.
    let query = format!("{vendor} {product}");
    if let Some(entry) = crate::compat::find_entry(&query) {
        report.add(
            Verdict::Pass,
            "compat DB coverage",
            format!(
                "this machine is documented ({} — reported by {})",
                entry.level_label(),
                entry.reported_by,
            ),
        );
    } else {
        // Warn (not Fail): missing coverage is informational — aegis-boot
        // still works on undocumented machines. We inline the guidance
        // into `detail` because `next_action` only surfaces on Fail.
        report.add(
            Verdict::Warn,
            "compat DB coverage",
            format!(
                "not yet in compat DB — file a report at {}",
                crate::compat::REPORT_URL,
            ),
        );
    }
}

/// Vendor placeholder strings many consumer OEMs ship verbatim. These
/// are the strings we filter out as "not actually set" when reading DMI.
#[cfg(target_os = "linux")]
const DMI_PLACEHOLDERS: &[&str] = &[
    "to be filled by o.e.m.",
    "system manufacturer",
    "system product name",
    "system version",
    "default string",
    "not applicable",
    "not specified",
    "oem",
    "o.e.m.",
    "none",
];

/// Read a DMI field from sysfs, trim whitespace, and filter vendor
/// placeholder strings. Returns `None` for missing, empty, or placeholder
/// values.
#[cfg(target_os = "linux")]
fn read_dmi_field(field: &str) -> Option<String> {
    let path = format!("/sys/class/dmi/id/{field}");
    let raw = std::fs::read_to_string(&path).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if DMI_PLACEHOLDERS.iter().any(|p| lower == *p) {
        return None;
    }
    Some(trimmed.to_string())
}

/// Compose the "product" half of the identity string. Lenovo puts the
/// human-readable model in `product_version` and a SKU in `product_name`;
/// Dell/HP/QEMU put the friendly name in `product_name`. Prefer
/// `product_version` when it differs meaningfully from `product_name`.
#[cfg(target_os = "linux")]
fn dmi_product_label() -> Option<String> {
    let name = read_dmi_field("product_name");
    let version = read_dmi_field("product_version");
    match (name, version) {
        (Some(n), Some(v)) if v.eq_ignore_ascii_case(&n) => Some(n),
        (Some(n), Some(v)) if v.len() > n.len() => Some(format!("{v} ({n})")),
        (Some(n), Some(v)) => Some(format!("{n} / {v}")),
        (Some(n), None) => Some(n),
        (None, Some(v)) => Some(v),
        (None, None) => None,
    }
}

/// Compose the BIOS half: "vendor version (date)" with graceful degradation
/// when any field is missing.
#[cfg(target_os = "linux")]
fn dmi_bios_label() -> Option<String> {
    let vendor = read_dmi_field("bios_vendor");
    let version = read_dmi_field("bios_version");
    let date = read_dmi_field("bios_date");
    match (vendor, version, date) {
        (Some(ve), Some(vi), Some(d)) => Some(format!("{ve} {vi} ({d})")),
        (Some(ve), Some(vi), None) => Some(format!("{ve} {vi}")),
        (None, Some(vi), Some(d)) => Some(format!("{vi} ({d})")),
        (None, Some(vi), None) => Some(vi),
        (Some(ve), None, _) => Some(ve),
        (None, None, _) => None,
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
            format!(
                "sgdisk failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ),
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

    // ---- --json mode --------------------------------------------------

    #[test]
    fn band_for_score_thresholds() {
        assert_eq!(band_for_score(100), "EXCELLENT");
        assert_eq!(band_for_score(90), "EXCELLENT");
        assert_eq!(band_for_score(89), "OK");
        assert_eq!(band_for_score(70), "OK");
        assert_eq!(band_for_score(69), "DEGRADED");
        assert_eq!(band_for_score(40), "DEGRADED");
        assert_eq!(band_for_score(39), "BROKEN");
        assert_eq!(band_for_score(0), "BROKEN");
    }

    #[test]
    fn json_escape_handles_quotes_and_backslash() {
        assert_eq!(json_escape(r#"hello "world""#), r#"hello \"world\""#);
        assert_eq!(json_escape(r"path\to\file"), r"path\\to\\file");
    }

    #[test]
    fn json_escape_handles_newline_and_tab() {
        assert_eq!(json_escape("line1\nline2"), "line1\\nline2");
        assert_eq!(json_escape("col1\tcol2"), "col1\\tcol2");
        assert_eq!(json_escape("\r\n"), "\\r\\n");
    }

    #[test]
    fn json_escape_handles_control_chars() {
        // NUL and SOH should render as \u00XX.
        assert_eq!(json_escape("a\x00b"), "a\\u0000b");
        assert_eq!(json_escape("\x01"), "\\u0001");
    }

    #[test]
    fn json_escape_leaves_ascii_unchanged() {
        assert_eq!(json_escape("plain ascii 123"), "plain ascii 123");
    }

    #[test]
    fn report_with_json_mode_silences_inline_prints() {
        // Black-box: creating + adding to a json-mode Report shouldn't
        // panic and shouldn't print (we can't easily intercept stdout
        // in unit tests, but we can assert the flag threads through).
        let r = Report::new().with_json_mode(true);
        assert!(r.json_mode);
    }

    #[test]
    fn report_without_json_mode_defaults_to_false() {
        let r = Report::new();
        assert!(!r.json_mode);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn check_machine_identity_adds_identity_and_compat_rows() {
        // Always emits two rows: machine identity + compat DB coverage.
        // Identity row is Pass (hardware) or Skip (DMI unavailable).
        // Coverage row is Pass (matched), Warn (unmatched), or Skip
        // (identity unavailable).
        let mut r = Report::new();
        check_machine_identity(&mut r);
        assert_eq!(r.rows.len(), 2);
        assert_eq!(r.rows[0].1, "machine identity");
        assert_eq!(r.rows[1].1, "compat DB coverage");
        assert!(
            matches!(r.rows[0].0, Verdict::Pass | Verdict::Skip),
            "identity verdict must be Pass or Skip, got {:?}",
            r.rows[0].0
        );
        assert!(
            matches!(r.rows[1].0, Verdict::Pass | Verdict::Warn | Verdict::Skip),
            "coverage verdict must be Pass/Warn/Skip, got {:?}",
            r.rows[1].0
        );
        // When identity is Skip, coverage must also be Skip.
        if matches!(r.rows[0].0, Verdict::Skip) {
            assert!(matches!(r.rows[1].0, Verdict::Skip));
        }
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn check_machine_identity_skips_both_rows_on_non_linux() {
        let mut r = Report::new();
        check_machine_identity(&mut r);
        assert_eq!(r.rows.len(), 2);
        assert!(matches!(r.rows[0].0, Verdict::Skip));
        assert!(matches!(r.rows[1].0, Verdict::Skip));
    }
}
