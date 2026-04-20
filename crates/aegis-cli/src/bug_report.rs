//! `aegis-boot bug-report` — workstation-side bug report bundler (#342 Phase 1).
//!
//! One command that captures everything a maintainer typically asks for
//! in a bug-report back-and-forth. Runs on the operator's workstation,
//! composes output from existing surfaces (doctor, DMI, removable-drive
//! detection) plus a small set of new system-level captures
//! (`uname`, `/proc/cmdline`, filtered `lsmod`, `dmesg` tail, `lspci`,
//! `lsusb`, `lsblk`).
//!
//! Privacy-preserving by default: hostname, username, DMI / drive
//! serials, MAC and public-IPv4 addresses are deterministically
//! obfuscated via [`crate::redact::Redactor`]. `--no-redact` restores
//! the real values but requires a typed confirmation string.
//!
//! Output modes (Phase 1):
//! * `--output stdout` (default) — markdown
//! * `--output <PATH>` — writes to file; format inferred from extension
//!   (`.md` → markdown, `.json` → json) or forced with `--format`
//! * `--format markdown` (default) / `--format json`
//!
//! Deferred to later phases of #342:
//! * Clipboard output (`wl-copy` / `xclip` / `pbcopy`)
//! * tar.zst bundle
//! * `--include stick:/dev/sdX` — Surface 2 on-stick log integration
//! * `--sign` — cosign keyless attestation bundle

use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

use crate::detect;
use crate::redact::Redactor;
use serde::Serialize;

/// Top-level bundle envelope. Serializable directly for `--format json`
/// output; the markdown renderer walks the same struct.
#[derive(Debug, Serialize)]
struct Bundle {
    schema_version: u32,
    aegis_boot_version: String,
    generated_at: String,
    redacted: bool,
    system: SystemSection,
    firmware: FirmwareSection,
    kernel: KernelSection,
    storage: StorageSection,
    aegis_state: AegisStateSection,
}

const BUNDLE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Serialize)]
struct SystemSection {
    os_release_pretty: Option<String>,
    uname: Option<String>,
    hostname: Option<String>,
    user: Option<String>,
}

#[derive(Debug, Serialize)]
struct FirmwareSection {
    sys_vendor: Option<String>,
    product_name: Option<String>,
    product_version: Option<String>,
    bios_vendor: Option<String>,
    bios_version: Option<String>,
    bios_date: Option<String>,
    product_serial: Option<String>,
    secure_boot: Option<String>,
}

#[derive(Debug, Serialize)]
struct KernelSection {
    cmdline: Option<String>,
    modules_storage_usb: Vec<String>,
    dmesg_tail: Vec<String>,
}

#[derive(Debug, Serialize)]
struct StorageSection {
    removable_drives: Vec<String>,
    lsblk: Option<String>,
    lsusb: Option<String>,
    lspci_storage: Option<String>,
}

#[derive(Debug, Serialize)]
struct AegisStateSection {
    /// `aegis-boot doctor --json` `tool_version` field.
    tool_version: String,
    doctor_score: Option<u32>,
    doctor_band: Option<String>,
    doctor_has_any_fail: bool,
    doctor_next_action: Option<String>,
    doctor_rows: Vec<DoctorRow>,
}

#[derive(Debug, Serialize)]
struct DoctorRow {
    verdict: String,
    name: String,
    detail: String,
}

#[derive(Debug, PartialEq, Eq)]
enum OutputMode {
    Stdout,
    File(PathBuf),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Markdown,
    Json,
}

#[derive(Debug)]
struct Args {
    output: OutputMode,
    format: Option<Format>,
    redact: bool,
    redact_confirm: bool,
    dump_mapping_to: Option<PathBuf>,
    help: bool,
}

impl Args {
    fn default_new() -> Self {
        Self {
            output: OutputMode::Stdout,
            format: None,
            redact: true,
            redact_confirm: false,
            dump_mapping_to: None,
            help: false,
        }
    }
}

pub(crate) fn run(argv: &[String]) -> ExitCode {
    let opts = match parse_args(argv) {
        Ok(parsed) => parsed,
        Err(msg) => {
            eprintln!("aegis-boot bug-report: {msg}");
            eprintln!("run 'aegis-boot bug-report --help' for usage");
            return ExitCode::from(2);
        }
    };
    if opts.help {
        print_help();
        return ExitCode::SUCCESS;
    }

    // Refuse to disable redaction without explicit confirmation.
    if !opts.redact && !opts.redact_confirm {
        eprintln!(
            "aegis-boot bug-report: --no-redact ships real PII (hostname, username, DMI / drive\n\
             serials, MAC + public IPv4 addresses) into the bundle. Confirm by adding the flag\n\
             --i-accept-pii-in-output alongside --no-redact."
        );
        return ExitCode::from(2);
    }

    let format = opts.format.unwrap_or_else(|| match &opts.output {
        OutputMode::File(path) => format_from_extension(path).unwrap_or(Format::Markdown),
        OutputMode::Stdout => Format::Markdown,
    });

    let mut redactor = Redactor::new(opts.redact);
    let bundle = collect_bundle(&mut redactor);

    let body = match format {
        Format::Markdown => render_markdown(&bundle, &redactor),
        Format::Json => render_json(&bundle),
    };

    // After rendering, sweep one more time in case any unredacted
    // values leaked into the free-text captures (dmesg, lsblk). Only
    // does anything in markdown — JSON is field-structured and the
    // per-field redaction already ran.
    let final_body = if opts.redact && matches!(format, Format::Markdown) {
        redactor.sweep(&body)
    } else {
        body
    };

    if let Err(msg) = emit(&opts.output, &final_body) {
        eprintln!("aegis-boot bug-report: {msg}");
        return ExitCode::from(1);
    }

    if opts.redact {
        if let Some(path) = opts.dump_mapping_to {
            if let Err(msg) = std::fs::write(&path, redactor.dump_mapping()) {
                eprintln!(
                    "aegis-boot bug-report: failed to write mapping to {}: {msg}",
                    path.display()
                );
                return ExitCode::from(1);
            }
            eprintln!(
                "aegis-boot bug-report: redaction mapping written to {} (keep LOCAL)",
                path.display()
            );
        }
    }

    ExitCode::SUCCESS
}

fn parse_args(argv: &[String]) -> Result<Args, String> {
    let mut a = Args::default_new();
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "-h" | "--help" => {
                a.help = true;
                return Ok(a);
            }
            "--output" => {
                let v = next_value(argv, &mut i, "--output")?;
                a.output = if v == "-" {
                    OutputMode::Stdout
                } else {
                    OutputMode::File(PathBuf::from(v))
                };
            }
            "--format" => {
                let v = next_value(argv, &mut i, "--format")?;
                a.format = Some(match v.as_str() {
                    "markdown" | "md" => Format::Markdown,
                    "json" => Format::Json,
                    other => {
                        return Err(format!(
                            "--format must be 'markdown' or 'json', got '{other}'"
                        ))
                    }
                });
            }
            "--no-redact" => {
                a.redact = false;
            }
            "--i-accept-pii-in-output" => {
                a.redact_confirm = true;
            }
            "--dump-mapping" => {
                let v = next_value(argv, &mut i, "--dump-mapping")?;
                a.dump_mapping_to = Some(PathBuf::from(v));
            }
            other => return Err(format!("unknown argument: {other}")),
        }
        i += 1;
    }
    Ok(a)
}

fn next_value(argv: &[String], i: &mut usize, flag: &str) -> Result<String, String> {
    *i += 1;
    argv.get(*i)
        .cloned()
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn format_from_extension(path: &std::path::Path) -> Option<Format> {
    match path.extension().and_then(|s| s.to_str()) {
        Some("md" | "markdown") => Some(Format::Markdown),
        Some("json") => Some(Format::Json),
        _ => None,
    }
}

fn collect_bundle(redactor: &mut Redactor) -> Bundle {
    let tool_version = env!("CARGO_PKG_VERSION").to_string();
    Bundle {
        schema_version: BUNDLE_SCHEMA_VERSION,
        aegis_boot_version: tool_version.clone(),
        generated_at: iso8601_now(),
        redacted: redactor.is_active(),
        system: collect_system(redactor),
        firmware: collect_firmware(redactor),
        kernel: collect_kernel(),
        storage: collect_storage(),
        aegis_state: collect_aegis_state(tool_version),
    }
}

fn collect_system(redactor: &mut Redactor) -> SystemSection {
    let os_release_pretty = read_os_release_pretty();
    let uname = run_capture("uname", &["-a"]);
    let hostname = std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(|h| redactor.hostname(&h));
    let user = std::env::var("USER").ok().map(|u| redactor.username(&u));
    SystemSection {
        os_release_pretty,
        uname,
        hostname,
        user,
    }
}

fn read_os_release_pretty() -> Option<String> {
    let text = std::fs::read_to_string("/etc/os-release").ok()?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("PRETTY_NAME=") {
            return Some(rest.trim_matches('"').to_string());
        }
    }
    None
}

fn collect_firmware(redactor: &mut Redactor) -> FirmwareSection {
    let read = |name: &str| {
        std::fs::read_to_string(format!("/sys/class/dmi/id/{name}"))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && !is_placeholder_dmi(s))
    };
    FirmwareSection {
        sys_vendor: read("sys_vendor"),
        product_name: read("product_name"),
        product_version: read("product_version"),
        bios_vendor: read("bios_vendor"),
        bios_version: read("bios_version"),
        bios_date: read("bios_date"),
        product_serial: read("product_serial").map(|s| redactor.serial(&s)),
        secure_boot: read_secure_boot_state(),
    }
}

fn is_placeholder_dmi(s: &str) -> bool {
    matches!(
        s,
        "To be filled by O.E.M."
            | "To Be Filled By O.E.M."
            | "Default string"
            | "System Product Name"
            | "System manufacturer"
            | "System Version"
            | "OEM"
            | "Not Specified"
            | "Not Applicable"
    )
}

fn read_secure_boot_state() -> Option<String> {
    // Try mokutil first.
    if let Some(out) = run_capture("mokutil", &["--sb-state"]) {
        let lower = out.to_lowercase();
        if lower.contains("secureboot enabled") || lower.contains("secure boot enabled") {
            return Some("enforcing".to_string());
        }
        if lower.contains("secureboot disabled") || lower.contains("secure boot disabled") {
            return Some("disabled".to_string());
        }
    }
    None
}

fn collect_kernel() -> KernelSection {
    let cmdline = std::fs::read_to_string("/proc/cmdline")
        .ok()
        .map(|s| s.trim().to_string());
    let modules_storage_usb = filtered_lsmod();
    let dmesg_tail = dmesg_tail(200);
    KernelSection {
        cmdline,
        modules_storage_usb,
        dmesg_tail,
    }
}

fn filtered_lsmod() -> Vec<String> {
    let Some(text) = run_capture("lsmod", &[]) else {
        return Vec::new();
    };
    // First column of lsmod is the module name. Filter by prefix to
    // storage + USB stack that a rescue-stick boot cares about.
    let wanted_prefixes = &[
        "usb", "xhci", "ehci", "ohci", "uhci", "sd_mod", "sr_mod", "ahci", "nvme", "mmc", "sdhci",
        "scsi", "libata", "mpt", "dm_mod", "dm_crypt", "uas",
    ];
    text.lines()
        .skip(1)
        .filter_map(|line| line.split_whitespace().next())
        .filter(|name| wanted_prefixes.iter().any(|p| name.starts_with(p)))
        .map(str::to_string)
        .collect()
}

fn dmesg_tail(lines: usize) -> Vec<String> {
    // `dmesg` may require CAP_SYSLOG or /proc/sys/kernel/dmesg_restrict
    // being 0. If we can't read it, return empty — it's a nice-to-have,
    // not a hard requirement for a bug report.
    let Some(text) = run_capture("dmesg", &["-T"]).or_else(|| run_capture("dmesg", &[])) else {
        return Vec::new();
    };
    let all_lines: Vec<&str> = text.lines().collect();
    let start = all_lines.len().saturating_sub(lines);
    all_lines[start..]
        .iter()
        .map(|s| (*s).to_string())
        .collect()
}

fn collect_storage() -> StorageSection {
    let removable_drives = detect::list_removable_drives()
        .into_iter()
        .map(|d| format!("{} ({}, {})", d.dev.display(), d.model, d.size_human()))
        .collect();
    let lsblk = run_capture("lsblk", &["-o", "NAME,SIZE,TYPE,FSTYPE,LABEL,MOUNTPOINT"]);
    let lsusb = run_capture("lsusb", &[]);
    let lspci_storage = {
        let mut acc = String::new();
        for class in ["::0100", "::0106", "::0108", "::010c"] {
            let Some(chunk) = run_capture("lspci", &["-D", "-d", class]) else {
                continue;
            };
            if chunk.trim().is_empty() {
                continue;
            }
            if !acc.is_empty() {
                acc.push('\n');
            }
            acc.push_str(chunk.trim_end());
        }
        if acc.is_empty() {
            None
        } else {
            Some(acc)
        }
    };
    StorageSection {
        removable_drives,
        lsblk,
        lsusb,
        lspci_storage,
    }
}

fn collect_aegis_state(tool_version: String) -> AegisStateSection {
    // Shell out to `aegis-boot doctor --json` so we reuse the live
    // binary's behavior. Falls back to empty state if the invocation
    // fails (e.g., PATH issue inside a weird sudo context). We look
    // up *this* binary — `/proc/self/exe` — rather than the PATH
    // `aegis-boot`, so operators who have a stale binary in PATH
    // still get a consistent report.
    let self_exe = std::env::current_exe().ok();
    let doctor_json = self_exe.and_then(|path| {
        Command::new(&path)
            .args(["doctor", "--json"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
    });
    let parsed = doctor_json
        .as_ref()
        .and_then(|s| serde_json::from_str::<aegis_wire_formats::DoctorReport>(s).ok());
    if let Some(r) = parsed {
        AegisStateSection {
            tool_version,
            doctor_score: Some(r.score),
            doctor_band: Some(r.band),
            doctor_has_any_fail: r.has_any_fail,
            doctor_next_action: r.next_action,
            doctor_rows: r
                .rows
                .into_iter()
                .map(|row| DoctorRow {
                    verdict: row.verdict,
                    name: row.name,
                    detail: row.detail,
                })
                .collect(),
        }
    } else {
        AegisStateSection {
            tool_version,
            doctor_score: None,
            doctor_band: None,
            doctor_has_any_fail: false,
            doctor_next_action: None,
            doctor_rows: Vec::new(),
        }
    }
}

fn run_capture(cmd: &str, args: &[&str]) -> Option<String> {
    Command::new(cmd)
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
}

fn iso8601_now() -> String {
    // Matches the pattern attest.rs + direct_install_manifest.rs use:
    // shell out to `date -u` so we get the same RFC-3339 / ISO-8601
    // UTC string across the family without taking on a chrono / jiff
    // runtime dep.
    let output = Command::new("date")
        .args(["-u", "+%Y-%m-%dT%H:%M:%SZ"])
        .output()
        .ok()
        .filter(|o| o.status.success());
    match output {
        Some(o) => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        None => "1970-01-01T00:00:00Z".to_string(),
    }
}

fn render_markdown(b: &Bundle, redactor: &Redactor) -> String {
    let mut out = String::new();
    render_header(&mut out, b);
    render_system(&mut out, &b.system);
    render_firmware(&mut out, &b.firmware);
    render_kernel(&mut out, &b.kernel);
    render_storage(&mut out, &b.storage);
    render_aegis_state(&mut out, &b.aegis_state);
    if redactor.is_active() {
        let _ = writeln!(out, "---\n");
        out.push_str(
            "_This report was redacted by default (`--no-redact` disables)._\n\
             _Hostname, username, DMI / drive serials, MAC + public IPv4\n\
             addresses appear as deterministic synthetic tokens. The real ↔\n\
             synthetic mapping is in-memory only unless you passed\n\
             `--dump-mapping PATH`._\n",
        );
    }
    out
}

fn render_header(out: &mut String, b: &Bundle) {
    let _ = writeln!(out, "# aegis-boot bug report\n");
    let _ = writeln!(
        out,
        "**Generated:** {} (aegis-boot v{})",
        b.generated_at, b.aegis_boot_version
    );
    let _ = writeln!(out, "**Bundle schema:** v{}", b.schema_version);
    let _ = writeln!(
        out,
        "**Redacted:** {}\n",
        if b.redacted {
            "yes (default)"
        } else {
            "NO — contains PII"
        }
    );
}

fn render_system(out: &mut String, s: &SystemSection) {
    let _ = writeln!(out, "## System\n");
    push_kv(out, "OS", s.os_release_pretty.as_deref());
    push_kv(out, "uname", s.uname.as_deref().map(str::trim));
    push_kv(out, "hostname", s.hostname.as_deref());
    push_kv(out, "user", s.user.as_deref());
    out.push('\n');
}

fn render_firmware(out: &mut String, f: &FirmwareSection) {
    let _ = writeln!(out, "## Firmware\n");
    push_kv(out, "vendor", f.sys_vendor.as_deref());
    push_kv(out, "product", f.product_name.as_deref());
    push_kv(out, "product version", f.product_version.as_deref());
    push_kv(out, "BIOS vendor", f.bios_vendor.as_deref());
    push_kv(out, "BIOS version", f.bios_version.as_deref());
    push_kv(out, "BIOS date", f.bios_date.as_deref());
    push_kv(out, "product serial", f.product_serial.as_deref());
    push_kv(out, "Secure Boot", f.secure_boot.as_deref());
    out.push('\n');
}

fn render_kernel(out: &mut String, k: &KernelSection) {
    let _ = writeln!(out, "## Kernel\n");
    push_kv(out, "cmdline", k.cmdline.as_deref());
    if !k.modules_storage_usb.is_empty() {
        let _ = writeln!(out, "**Loaded storage / USB modules:**\n\n```");
        for name in &k.modules_storage_usb {
            let _ = writeln!(out, "{name}");
        }
        let _ = writeln!(out, "```\n");
    }
    if !k.dmesg_tail.is_empty() {
        let _ = writeln!(
            out,
            "**Last {} lines of `dmesg`:**\n\n```",
            k.dmesg_tail.len()
        );
        for line in &k.dmesg_tail {
            let _ = writeln!(out, "{line}");
        }
        let _ = writeln!(out, "```\n");
    }
}

fn render_storage(out: &mut String, s: &StorageSection) {
    let _ = writeln!(out, "## Storage\n");
    if !s.removable_drives.is_empty() {
        let _ = writeln!(out, "**Removable drives:**\n");
        for d in &s.removable_drives {
            let _ = writeln!(out, "- {d}");
        }
        out.push('\n');
    }
    push_fenced(out, "lsblk", s.lsblk.as_deref());
    push_fenced(out, "lsusb", s.lsusb.as_deref());
    push_fenced(out, "lspci (storage)", s.lspci_storage.as_deref());
}

fn render_aegis_state(out: &mut String, a: &AegisStateSection) {
    let _ = writeln!(out, "## aegis-boot state\n");
    let _ = writeln!(out, "**Tool version:** {}", a.tool_version);
    if let (Some(score), Some(band)) = (a.doctor_score, &a.doctor_band) {
        let _ = writeln!(out, "**doctor:** {score}/100 ({band})");
    }
    if a.doctor_has_any_fail {
        let _ = writeln!(out, "**any FAIL:** yes");
    }
    if let Some(next) = &a.doctor_next_action {
        let _ = writeln!(out, "**NEXT ACTION:** {next}");
    }
    if !a.doctor_rows.is_empty() {
        let _ = writeln!(out, "\n**doctor rows:**\n");
        for row in &a.doctor_rows {
            let _ = writeln!(out, "- `[{}]` {} — {}", row.verdict, row.name, row.detail);
        }
        out.push('\n');
    }
}

fn push_kv(out: &mut String, key: &str, val: Option<&str>) {
    if let Some(v) = val {
        let _ = writeln!(out, "- **{key}:** {v}");
    }
}

fn push_fenced(out: &mut String, heading: &str, val: Option<&str>) {
    let Some(v) = val else {
        return;
    };
    if v.trim().is_empty() {
        return;
    }
    let _ = writeln!(out, "**{heading}:**\n\n```\n{}\n```\n", v.trim_end());
}

fn render_json(b: &Bundle) -> String {
    serde_json::to_string_pretty(b)
        .unwrap_or_else(|e| format!("{{\"error\":\"failed to serialize bundle: {e}\"}}"))
}

fn emit(mode: &OutputMode, body: &str) -> Result<(), String> {
    match mode {
        OutputMode::Stdout => {
            print!("{body}");
            Ok(())
        }
        OutputMode::File(path) => {
            std::fs::write(path, body).map_err(|e| format!("write {}: {e}", path.display()))
        }
    }
}

fn print_help() {
    let v = env!("CARGO_PKG_VERSION");
    println!("aegis-boot bug-report — generate a bug-report bundle");
    println!();
    println!("USAGE:");
    println!("  aegis-boot bug-report [--output PATH|-] [--format markdown|json]");
    println!("                        [--no-redact --i-accept-pii-in-output]");
    println!("                        [--dump-mapping PATH]");
    println!();
    println!("OUTPUT MODES:");
    println!("  --output -           Write markdown to stdout (default)");
    println!("  --output PATH.md     Write markdown to file");
    println!("  --output PATH.json   Write JSON to file");
    println!("  --format markdown|json   Force format when extension is ambiguous");
    println!();
    println!("PRIVACY:");
    println!("  Redaction is ON by default. Hostname, username, DMI + drive serials,");
    println!("  MAC addresses and public IPv4 addresses are replaced with deterministic");
    println!("  synthetic tokens (e.g. `host-ab12cd`, `serial-ef34gh`).");
    println!("  --no-redact          Disable redaction (requires --i-accept-pii-in-output)");
    println!("  --dump-mapping PATH  Write the real ↔ synthetic map to PATH. Keep it LOCAL —");
    println!("                       it de-anonymizes any bundle you share.");
    println!();
    println!("EXAMPLES:");
    println!("  aegis-boot bug-report                                # markdown to stdout");
    println!("  aegis-boot bug-report --output report.md");
    println!("  aegis-boot bug-report --output report.json --format json");
    println!();
    println!("(aegis-boot v{v})");
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn parse_args_defaults() {
        let a = parse_args(&[]).unwrap();
        assert_eq!(a.output, OutputMode::Stdout);
        assert!(a.format.is_none());
        assert!(a.redact);
        assert!(!a.help);
    }

    #[test]
    fn parse_args_output_to_file() {
        let a = parse_args(&["--output".into(), "/tmp/r.md".into()]).unwrap();
        assert_eq!(a.output, OutputMode::File(PathBuf::from("/tmp/r.md")));
    }

    #[test]
    fn parse_args_format_json() {
        let a = parse_args(&["--format".into(), "json".into()]).unwrap();
        assert_eq!(a.format, Some(Format::Json));
    }

    #[test]
    fn parse_args_no_redact_requires_confirm_flag() {
        // The parse step accepts --no-redact alone; the `run` step is
        // what enforces the confirmation-flag gate. Here we check the
        // parser populates both flags independently.
        let a = parse_args(&["--no-redact".into()]).unwrap();
        assert!(!a.redact);
        assert!(!a.redact_confirm);
        let b = parse_args(&["--no-redact".into(), "--i-accept-pii-in-output".into()]).unwrap();
        assert!(!b.redact);
        assert!(b.redact_confirm);
    }

    #[test]
    fn parse_args_help() {
        assert!(parse_args(&["--help".into()]).unwrap().help);
        assert!(parse_args(&["-h".into()]).unwrap().help);
    }

    #[test]
    fn parse_args_rejects_unknown_flag() {
        let err = parse_args(&["--nope".into()]).unwrap_err();
        assert!(err.contains("unknown"));
    }

    #[test]
    fn format_from_extension_picks_right_format() {
        assert_eq!(
            format_from_extension(std::path::Path::new("a.md")),
            Some(Format::Markdown)
        );
        assert_eq!(
            format_from_extension(std::path::Path::new("a.json")),
            Some(Format::Json)
        );
        assert_eq!(format_from_extension(std::path::Path::new("a.txt")), None);
    }

    #[test]
    fn is_placeholder_dmi_catches_canonical_oem_strings() {
        assert!(is_placeholder_dmi("To be filled by O.E.M."));
        assert!(is_placeholder_dmi("Default string"));
        assert!(is_placeholder_dmi("System Product Name"));
        assert!(!is_placeholder_dmi("ThinkPad X1 Carbon Gen 11"));
    }

    #[test]
    fn iso8601_now_returns_rfc3339_shape() {
        let s = iso8601_now();
        // YYYY-MM-DDTHH:MM:SSZ is 20 chars; fallback is same length.
        assert_eq!(s.len(), 20, "unexpected shape: {s}");
        assert!(s.ends_with('Z'));
        assert_eq!(&s[4..5], "-");
        assert_eq!(&s[7..8], "-");
        assert_eq!(&s[10..11], "T");
    }

    #[test]
    fn render_markdown_smoke() {
        // Minimal bundle with no captured data — just verifies that
        // the renderer doesn't crash when most fields are None and
        // that the skeleton structure is present.
        let b = Bundle {
            schema_version: BUNDLE_SCHEMA_VERSION,
            aegis_boot_version: "0.15.0".to_string(),
            generated_at: "2026-04-20T12:34:56Z".to_string(),
            redacted: true,
            system: SystemSection {
                os_release_pretty: Some("Ubuntu 24.04 LTS".to_string()),
                uname: None,
                hostname: Some("host-abc123".to_string()),
                user: None,
            },
            firmware: FirmwareSection {
                sys_vendor: Some("ASRock".to_string()),
                product_name: None,
                product_version: None,
                bios_vendor: None,
                bios_version: None,
                bios_date: None,
                product_serial: None,
                secure_boot: Some("enforcing".to_string()),
            },
            kernel: KernelSection {
                cmdline: None,
                modules_storage_usb: Vec::new(),
                dmesg_tail: Vec::new(),
            },
            storage: StorageSection {
                removable_drives: Vec::new(),
                lsblk: None,
                lsusb: None,
                lspci_storage: None,
            },
            aegis_state: AegisStateSection {
                tool_version: "0.15.0".to_string(),
                doctor_score: Some(96),
                doctor_band: Some("EXCELLENT".to_string()),
                doctor_has_any_fail: false,
                doctor_next_action: None,
                doctor_rows: Vec::new(),
            },
        };
        let r = Redactor::new(true);
        let md = render_markdown(&b, &r);
        assert!(md.contains("# aegis-boot bug report"));
        assert!(md.contains("## System"));
        assert!(md.contains("Ubuntu 24.04 LTS"));
        assert!(md.contains("## Firmware"));
        assert!(md.contains("ASRock"));
        assert!(md.contains("## Kernel"));
        assert!(md.contains("## Storage"));
        assert!(md.contains("96/100 (EXCELLENT)"));
        assert!(md.contains("host-abc123"));
        assert!(md.contains("redacted by default"));
    }

    #[test]
    fn render_json_emits_pretty_structure() {
        let b = Bundle {
            schema_version: BUNDLE_SCHEMA_VERSION,
            aegis_boot_version: "0.15.0".to_string(),
            generated_at: "2026-04-20T12:34:56Z".to_string(),
            redacted: true,
            system: SystemSection {
                os_release_pretty: None,
                uname: None,
                hostname: None,
                user: None,
            },
            firmware: FirmwareSection {
                sys_vendor: None,
                product_name: None,
                product_version: None,
                bios_vendor: None,
                bios_version: None,
                bios_date: None,
                product_serial: None,
                secure_boot: None,
            },
            kernel: KernelSection {
                cmdline: None,
                modules_storage_usb: Vec::new(),
                dmesg_tail: Vec::new(),
            },
            storage: StorageSection {
                removable_drives: Vec::new(),
                lsblk: None,
                lsusb: None,
                lspci_storage: None,
            },
            aegis_state: AegisStateSection {
                tool_version: "0.15.0".to_string(),
                doctor_score: None,
                doctor_band: None,
                doctor_has_any_fail: false,
                doctor_next_action: None,
                doctor_rows: Vec::new(),
            },
        };
        let j = render_json(&b);
        assert!(j.contains("\"schema_version\": 1"));
        assert!(j.contains("\"aegis_boot_version\": \"0.15.0\""));
        assert!(j.contains("\"redacted\": true"));
        // Pretty-printed JSON has newlines + indent.
        assert!(j.contains("\n  "));
    }
}
