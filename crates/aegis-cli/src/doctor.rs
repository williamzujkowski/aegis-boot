// SPDX-License-Identifier: MIT OR Apache-2.0

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
        let report = aegis_wire_formats::DoctorReport {
            schema_version: aegis_wire_formats::DOCTOR_SCHEMA_VERSION,
            tool_version: env!("CARGO_PKG_VERSION").to_string(),
            score: u32::from(score),
            band: band.to_string(),
            has_any_fail: self.has_any_fail(),
            next_action: self.next_action.clone(),
            rows: self
                .rows
                .iter()
                .map(|(verdict, name, detail)| aegis_wire_formats::DoctorRow {
                    verdict: verdict.label().to_string(),
                    name: name.clone(),
                    detail: detail.clone(),
                })
                .collect(),
        };
        match serde_json::to_string_pretty(&report) {
            Ok(body) => println!("{body}"),
            Err(e) => eprintln!("aegis-boot doctor: failed to serialize --json envelope: {e}"),
        }
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

// The `json_escape` helper (formerly here) was retired in
// Phase 4b / #306 once every hand-rolled `--json` emitter in
// aegis-cli migrated to typed envelopes in the `aegis-wire-formats`
// crate. `serde_json` now handles JSON string escaping for every
// `--json` surface, so a crate-local escaper is no longer needed.

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
    check_host_commands(&mut report);
    check_cosign_optional(&mut report);
    check_secureboot_state(&mut report);
    check_removable_drives(&mut report);
    if !json_mode {
        println!();
        println!("Stick checks:");
    }
    if let Some(dev) = stick.or_else(autodetect_single_stick) {
        check_stick_partitions(&mut report, &dev);
        check_aegis_isos_mount(&mut report, &dev);
        check_manifest_sequence(&mut report, &dev);
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
        // still works on undocumented machines. Point at the one-command
        // draft-report path (`compat --submit`) rather than the raw URL;
        // the URL is long and operators running on a terminal can't
        // click it, whereas they can copy-paste the subcommand.
        report.add(
            Verdict::Warn,
            "compat DB coverage",
            "not yet in compat DB — run `aegis-boot compat --submit` to draft a report",
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
///
/// `pub(crate)` so other subcommands (e.g., `compat --my-machine`) can
/// reuse the same sysfs read + placeholder-filter semantics without
/// re-implementing the whole set of OEM-placeholder strings.
#[cfg(target_os = "linux")]
pub(crate) fn read_dmi_field(field: &str) -> Option<String> {
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
///
/// `pub(crate)` so `compat --my-machine` can cite the same label as
/// `doctor`'s machine-identity row — both surfaces must agree on what
/// "this machine" is named.
#[cfg(target_os = "linux")]
pub(crate) fn dmi_product_label() -> Option<String> {
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
/// when any field is missing. `pub(crate)` so other subcommands (e.g.,
/// `compat --submit`) can cite the same BIOS string as `doctor`'s
/// machine-identity row.
#[cfg(target_os = "linux")]
pub(crate) fn dmi_bios_label() -> Option<String> {
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

/// Soft-check for `cosign` on PATH (#235). Unlike the hard commands
/// above, cosign is **optional**: `fetch-image` graceful-degrades when
/// it's missing (surfaces a warning, skips the signature layer). So
/// this check emits `Pass` when present and `Warn` when absent, rather
/// than `Fail` — operators who never use `fetch-image` don't need it.
fn check_cosign_optional(report: &mut Report) {
    let name = "command: cosign (optional)".to_string();
    if let Some(path) = which("cosign") {
        report.add(
            Verdict::Pass,
            name,
            format!(
                "{} (auto-verifies `aegis-boot fetch-image` downloads against aegis-boot's release workflow)",
                path.display()
            ),
        );
    } else {
        report.add_with_next(
            Verdict::Warn,
            name,
            "not found in PATH — `fetch-image` cannot cosign-verify signed images".to_string(),
            "install cosign: https://docs.sigstore.dev/cosign/system_config/installation/ \
             (not required unless you use `aegis-boot fetch-image`)"
                .to_string(),
        );
    }
}

/// Runs every command-presence check doctor does for the host.
/// Extracted from `try_run` so the top-level stays focused on orchestration
/// (also keeps `try_run` under the clippy `too_many_lines` budget).
///
/// Ordering matches what operators see on stdout — do not reorder without
/// updating any docs / screenshots that show the expected output.
fn check_host_commands(report: &mut Report) {
    // #333: bare binary names produced wrong remedies (e.g. "apt-get
    // install dd"). Fix is per-family pkg names — see `PkgNames`.
    check_command_present_with_pkg(report, "dd", "coreutils", "required to write the stick");
    check_command_present(report, "sudo", "required for dd / mount");
    check_command_present_with_pkgs(
        report,
        "sgdisk",
        PkgNames {
            apt: "gdisk",
            dnf: "gdisk",
            pacman: "gptfdisk",
        },
        "verifies stick partition table after flash",
    );
    check_command_present_with_pkg(
        report,
        "lsblk",
        "util-linux",
        "lists removable drives for `flash` auto-detect",
    );
    // mkusb.sh dependencies (#313). These are required by the build
    // path `aegis-boot init` invokes — mcopy stages the ESP, mkfs.vfat
    // formats the ESP, mkfs.exfat formats the AEGIS_ISOS data
    // partition (default since #243). Pre-flighting them here catches
    // the class of late failure that motivated operator-reported
    // bug #282.
    check_command_present_with_pkg(
        report,
        "mcopy",
        "mtools",
        "copies the signed boot chain onto the ESP (`aegis-boot flash`)",
    );
    check_command_present_with_pkg(
        report,
        "mkfs.vfat",
        "dosfstools",
        "formats the ESP partition FAT32 (`aegis-boot flash`)",
    );
    check_command_present_with_pkg(
        report,
        "mkfs.exfat",
        "exfatprogs",
        "formats the AEGIS_ISOS data partition exFAT (`aegis-boot flash`)",
    );
    check_command_present(
        report,
        "curl",
        "downloads catalog ISOs (`aegis-boot fetch`) and the install one-liner",
    );
    check_command_present_with_pkg(
        report,
        "sha256sum",
        "coreutils",
        "verifies catalog ISO checksums (`aegis-boot fetch`)",
    );
    check_command_present_with_pkgs(
        report,
        "gpg",
        PkgNames {
            apt: "gnupg",
            dnf: "gnupg2",
            pacman: "gnupg",
        },
        "verifies catalog SHA256SUMS signatures (`aegis-boot fetch`)",
    );
}

fn check_command_present(report: &mut Report, cmd: &str, why: &str) {
    check_command_present_with_pkg(report, cmd, cmd, why);
}

/// Per-family package names for a single command. Used when a
/// binary ships in a differently-named package on different distro
/// families — e.g. `sgdisk` is in `gdisk` on Debian/Fedora but
/// `gptfdisk` on Arch/openSUSE. See #333 for the audit that
/// surfaced these mismatches.
#[derive(Debug, Clone, Copy)]
struct PkgNames<'a> {
    apt: &'a str,
    dnf: &'a str,
    pacman: &'a str,
}

impl<'a> PkgNames<'a> {
    /// The common case: one package name across all three families.
    const fn same(name: &'a str) -> Self {
        Self {
            apt: name,
            dnf: name,
            pacman: name,
        }
    }
}

/// Like [`check_command_present`] but lets callers specify the
/// package name when it differs from the binary name (e.g. the
/// `mkfs.vfat` binary ships in the `dosfstools` package). Used for
/// the mkusb.sh dependency preflight (#313).
fn check_command_present_with_pkg(report: &mut Report, cmd: &str, pkg: &str, why: &str) {
    check_command_present_with_pkgs(report, cmd, PkgNames::same(pkg), why);
}

/// Like [`check_command_present_with_pkg`] but supports per-family
/// package names for binaries whose packaging diverges across
/// distros. See [`PkgNames`] for the family slots.
fn check_command_present_with_pkgs(report: &mut Report, cmd: &str, pkgs: PkgNames, why: &str) {
    let found = which(cmd);
    let name = format!("command: {cmd}");
    if let Some(path) = found {
        report.add(Verdict::Pass, name, format!("{} ({why})", path.display()));
    } else {
        report.add_with_next(
            Verdict::Fail,
            name,
            format!("not found in PATH ({why})"),
            format!(
                "install `{cmd}` (on Debian/Ubuntu: `sudo apt-get install {}`; \
                 on Fedora/RHEL: `sudo dnf install {}`; \
                 on Arch: `sudo pacman -S {}`)",
                pkgs.apt, pkgs.dnf, pkgs.pacman,
            ),
        );
    }
}

// `which` lives in the `cmd_path` module so `fetch-image` and every
// other command-presence caller use the same probe. See #332 for why
// unified lookup matters.
use crate::cmd_path::which;

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
    let Some(mp) = mount_point else {
        report.add(
            Verdict::Skip,
            name,
            format!(
                "AEGIS_ISOS not mounted; run `aegis-boot list {}` to check ISOs",
                dev.display()
            ),
        );
        return;
    };

    // #274 Phase 6b: emit per-ISO trust-state rows using the same
    // recursive scan rescue-tui + `aegis-boot list` use (Phase 6a).
    // Shape is one umbrella row ("AEGIS_ISOS trust coverage") giving
    // the count summary, plus one `[GREEN/YELLOW/RED] <folder/iso>`
    // row per ISO so operators see exactly which stick contents will
    // show which verdict in rescue-tui.
    let mount_path = Path::new(&mp);
    let isos = crate::inventory::scan_isos(mount_path);
    render_aegis_isos_trust_coverage(report, &mp, &isos, dev);
}

/// Pure-ish trust-coverage row renderer. Extracted so
/// `check_aegis_isos_mount` stays under the 100-line budget AND so
/// the per-ISO row classification is unit-testable against a slice
/// of `IsoEntry` without touching the filesystem.
fn render_aegis_isos_trust_coverage(
    report: &mut Report,
    mount_display: &str,
    isos: &[crate::inventory::IsoEntry],
    dev: &Path,
) {
    let umbrella = "AEGIS_ISOS trust coverage".to_string();

    if isos.is_empty() {
        report.add_with_next(
            Verdict::Warn,
            umbrella,
            format!("{mount_display} has no .iso files yet"),
            "add an ISO with `aegis-boot add /path/to/distro.iso` or a catalog slug (#352 UX-4)",
        );
        return;
    }

    let green = isos
        .iter()
        .filter(|e| e.has_sha256 && e.has_minisig)
        .count();
    let yellow = isos
        .iter()
        .filter(|e| e.has_sha256 && !e.has_minisig)
        .count();
    let red = isos.iter().filter(|e| !e.has_sha256).count();

    let summary_verdict = if red > 0 || yellow > 0 {
        Verdict::Warn
    } else {
        Verdict::Pass
    };
    let summary_detail = format!(
        "{} ISO(s): {green} GREEN (sha256+minisig), {yellow} YELLOW (sha256 only), {red} RED (no sidecars)",
        isos.len()
    );
    if red > 0 || yellow > 0 {
        report.add_with_next(
            summary_verdict,
            umbrella,
            summary_detail,
            format!(
                "RED ISOs trigger typed-boot confirm in rescue-tui; drop sibling \
                 .sha256 + .minisig next to each ISO on {} to clear",
                dev.display()
            ),
        );
    } else {
        report.add(summary_verdict, umbrella, summary_detail);
    }

    for entry in isos {
        let verdict = classify_trust_state(entry);
        let path = match &entry.folder {
            Some(f) => format!("{f}/{}", entry.name),
            None => entry.name.clone(),
        };
        let sidecars = match (entry.has_sha256, entry.has_minisig) {
            (true, true) => "sha256 + minisig",
            (true, false) => "sha256 only (no minisig)",
            (false, true) => "minisig only (no sha256)",
            (false, false) => "no sidecars — rescue-tui will show GRAY verdict",
        };
        report.add(verdict, format!("  {path}"), sidecars);
    }
}

/// Trust-state classification for a single ISO:
/// - GREEN (Pass): both `.sha256` and `.minisig` sidecars present
/// - YELLOW (Warn): one sidecar present (usually `.sha256`, occasionally `.minisig`)
/// - RED (Fail): neither sidecar — rescue-tui shows GRAY and requires typed 'boot'
fn classify_trust_state(entry: &crate::inventory::IsoEntry) -> Verdict {
    match (entry.has_sha256, entry.has_minisig) {
        (true, true) => Verdict::Pass,
        (true, false) | (false, true) => Verdict::Warn,
        (false, false) => Verdict::Fail,
    }
}

/// #181 Phase 4: surface the attestation manifest's `sequence` and
/// `tool_version` so operators can see whether `aegis-boot update
/// --apply` has been run against this stick, and if so with what
/// version. Silent-skip if no attestation is reachable — that's not
/// a health issue by itself; the other stick checks already fail
/// loudly when the stick isn't aegis-boot-flashed.
fn check_manifest_sequence(report: &mut Report, dev: &Path) {
    let name = format!("manifest sequence: {}", dev.display());
    let Some(att_path) = find_attestation_for_dev(dev) else {
        report.add(
            Verdict::Skip,
            name,
            "no host-side attestation matching this stick's disk GUID",
        );
        return;
    };
    let body = match std::fs::read_to_string(&att_path) {
        Ok(b) => b,
        Err(e) => {
            report.add(
                Verdict::Warn,
                name,
                format!("could not read {}: {e}", att_path.display()),
            );
            return;
        }
    };
    let manifest: aegis_wire_formats::Manifest = match serde_json::from_str(&body) {
        Ok(m) => m,
        Err(e) => {
            report.add(
                Verdict::Warn,
                name,
                format!("parse error on {}: {e}", att_path.display()),
            );
            return;
        }
    };
    report.add(
        Verdict::Pass,
        name,
        format!(
            "sequence={} tool_version={}",
            manifest.sequence, manifest.tool_version
        ),
    );
}

/// Look up the host-side attestation manifest file for a stick by
/// disk GUID. Small mirror of the resolver in `update::find_attestation_by_guid`;
/// kept local because doctor shouldn't take a cross-module dependency
/// through the update path (update is destructive-capable and this
/// doctor check is read-only).
fn find_attestation_for_dev(dev: &Path) -> Option<PathBuf> {
    let out = Command::new("sudo")
        .args(["sgdisk", "-p"])
        .arg(dev)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let guid = extract_disk_guid(&text)?;
    let lower = guid.to_ascii_lowercase();
    let dir = crate::paths::attestations_dir();
    for entry in std::fs::read_dir(&dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        // Substring match on the lowercased body; GUID is anchored by
        // the `"disk_guid":` key so false-positive prefix matches can't
        // occur on typical manifest shapes.
        if body.to_ascii_lowercase().contains(&lower) {
            return Some(path);
        }
    }
    None
}

/// Extract `Disk identifier (GUID): <guid>` from `sgdisk -p` output.
/// Duplicates the logic in `update::parse_disk_guid` locally to keep
/// doctor independent of the update module.
fn extract_disk_guid(out: &str) -> Option<String> {
    for line in out.lines() {
        if let Some(rest) = line.trim().strip_prefix("Disk identifier (GUID): ") {
            let g = rest.trim().to_ascii_lowercase();
            if g.len() == 36 && g.matches('-').count() == 4 {
                return Some(g);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::IsoEntry;

    // ---- #274 Phase 6b: trust-state classification ------------------------

    #[test]
    fn classify_trust_state_green_requires_both_sidecars() {
        let e = IsoEntry::new_for_test("ubuntu.iso", None, true, true);
        assert_eq!(classify_trust_state(&e), Verdict::Pass);
    }

    #[test]
    fn classify_trust_state_yellow_for_sha256_only() {
        let e = IsoEntry::new_for_test("alpine.iso", None, true, false);
        assert_eq!(classify_trust_state(&e), Verdict::Warn);
    }

    #[test]
    fn classify_trust_state_yellow_for_minisig_only() {
        // Operator may have dropped a minisig without the sha256;
        // symmetrically YELLOW, same as sha256-only.
        let e = IsoEntry::new_for_test("debian.iso", None, false, true);
        assert_eq!(classify_trust_state(&e), Verdict::Warn);
    }

    #[test]
    fn classify_trust_state_red_when_no_sidecars() {
        let e = IsoEntry::new_for_test("random.iso", None, false, false);
        assert_eq!(classify_trust_state(&e), Verdict::Fail);
    }

    #[test]
    fn render_trust_coverage_emits_warn_when_empty_isos_list() {
        // Empty stick — expected umbrella Warn row telling operator
        // to add an ISO. No per-ISO rows.
        let mut r = Report::new().with_json_mode(true);
        render_aegis_isos_trust_coverage(&mut r, "/mnt/aegis", &[], Path::new("/dev/sdx"));
        assert_eq!(r.rows.len(), 1);
        let (verdict, name, _detail) = &r.rows[0];
        assert_eq!(*verdict, Verdict::Warn);
        assert!(name.contains("AEGIS_ISOS trust coverage"));
    }

    #[test]
    fn render_trust_coverage_green_summary_when_all_sidecars_present() {
        let isos = vec![
            IsoEntry::new_for_test("a.iso", Some("ubuntu-24.04".into()), true, true),
            IsoEntry::new_for_test("b.iso", None, true, true),
        ];
        let mut r = Report::new().with_json_mode(true);
        render_aegis_isos_trust_coverage(&mut r, "/mnt", &isos, Path::new("/dev/sdx"));
        // 1 summary row + 2 per-ISO rows = 3 total
        assert_eq!(r.rows.len(), 3);
        assert_eq!(r.rows[0].0, Verdict::Pass);
        assert!(r.rows[0].2.contains("2 GREEN"));
        assert_eq!(r.rows[1].0, Verdict::Pass);
        assert_eq!(r.rows[2].0, Verdict::Pass);
    }

    #[test]
    fn render_trust_coverage_per_iso_rows_use_folder_slash_name_path() {
        let isos = vec![
            IsoEntry::new_for_test("server.iso", Some("ubuntu-24.04".into()), true, true),
            IsoEntry::new_for_test("desktop.iso", None, false, false),
        ];
        let mut r = Report::new().with_json_mode(true);
        render_aegis_isos_trust_coverage(&mut r, "/mnt", &isos, Path::new("/dev/sdx"));
        // Check that the ubuntu-24.04/server.iso row uses the folder prefix
        let Some(subfolder_row) = r
            .rows
            .iter()
            .find(|(_, name, _)| name.contains("ubuntu-24.04/server.iso"))
        else {
            panic!(
                "expected a row for ubuntu-24.04/server.iso, got {:?}",
                r.rows
            );
        };
        assert_eq!(subfolder_row.0, Verdict::Pass);
        // Root-level ISO renders without a folder prefix
        let Some(root_row) = r
            .rows
            .iter()
            .find(|(_, name, _)| name.trim() == "desktop.iso")
        else {
            panic!("expected a row for desktop.iso, got {:?}", r.rows);
        };
        assert_eq!(root_row.0, Verdict::Fail);
    }

    #[test]
    fn render_trust_coverage_mixed_summary_is_warn_with_next_action() {
        // One green + one yellow + one red → overall Warn + next_action
        // set so the operator sees exactly what to fix.
        let isos = vec![
            IsoEntry::new_for_test("ok.iso", None, true, true),
            IsoEntry::new_for_test("partial.iso", None, true, false),
            IsoEntry::new_for_test("bad.iso", None, false, false),
        ];
        let mut r = Report::new().with_json_mode(true);
        render_aegis_isos_trust_coverage(&mut r, "/mnt", &isos, Path::new("/dev/sdx"));
        assert_eq!(r.rows[0].0, Verdict::Warn);
        assert!(r.rows[0].2.contains("1 GREEN"));
        assert!(r.rows[0].2.contains("1 YELLOW"));
        assert!(r.rows[0].2.contains("1 RED"));
    }

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

    // `json_escape` tests were retired in Phase 4b / #306 alongside
    // the helper itself — every `--json` emitter now goes through
    // `serde_json`, which has its own escape-correctness test suite.

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

    #[test]
    fn check_command_present_with_pkg_finds_existing_binary() {
        // Known-good probe per platform — `ls` is universal on
        // POSIX; `cmd.exe` is present on every Windows install. The
        // `.exe` suffix is required because `crate::cmd_path::which`
        // does not currently auto-append `PATHEXT` extensions on
        // Windows (tracked as a separate improvement — the doctor
        // surface for Windows operators is still a gap).
        let probe = if cfg!(target_os = "windows") {
            "cmd.exe"
        } else {
            "ls"
        };
        let mut r = Report::new();
        check_command_present_with_pkg(&mut r, probe, "coreutils", "canary");
        assert_eq!(r.rows.len(), 1);
        assert!(
            matches!(r.rows[0].0, Verdict::Pass),
            "expected Pass for `{probe}`, got {:?}",
            r.rows[0].0
        );
        assert!(r.rows[0].2.contains("canary"));
    }

    #[test]
    fn check_command_present_with_pkgs_splits_per_family() {
        // #333: sgdisk is `gdisk` on apt/dnf but `gptfdisk` on pacman.
        // The remedy must name each family's correct package, not
        // the same name three times.
        let mut r = Report::new();
        check_command_present_with_pkgs(
            &mut r,
            "aegis-probe-sgdisk-never-installed",
            PkgNames {
                apt: "gdisk",
                dnf: "gdisk",
                pacman: "gptfdisk",
            },
            "probe",
        );
        let na = r.next_action.as_deref().unwrap_or("");
        assert!(
            na.contains("apt-get install gdisk"),
            "remedy must name apt pkg, got: {na}"
        );
        assert!(
            na.contains("dnf install gdisk"),
            "remedy must name dnf pkg, got: {na}"
        );
        assert!(
            na.contains("pacman -S gptfdisk"),
            "remedy must name pacman pkg separately, got: {na}"
        );
    }

    #[test]
    fn check_command_present_with_pkg_reports_package_name_on_miss() {
        // The package name is load-bearing — `mkfs.vfat` ships in
        // `dosfstools`, not in a hypothetical `mkfs.vfat` package.
        // On a miss, the remedy text must name the package, not
        // the binary. #313 acceptance criterion.
        let mut r = Report::new();
        check_command_present_with_pkg(
            &mut r,
            "aegis-probe-never-installed-binary-for-test",
            "dosfstools",
            "guards the pkg-name surfaces in remedy text",
        );
        assert_eq!(r.rows.len(), 1);
        assert!(
            matches!(r.rows[0].0, Verdict::Fail),
            "expected Fail for missing binary, got {:?}",
            r.rows[0].0
        );
        // First-Fail remedy gets promoted to Report::next_action.
        // Verify it names the package + the three common distro
        // families (the #313 acceptance criterion is a multi-distro
        // hint, not a per-distro auto-detection).
        let na = r.next_action.as_deref().unwrap_or("");
        assert!(
            na.contains("dosfstools"),
            "remedy must name the package, got: {na}"
        );
        assert!(
            na.contains("apt-get") && na.contains("dnf") && na.contains("pacman"),
            "remedy must list the three common distro families, got: {na}"
        );
    }
}
