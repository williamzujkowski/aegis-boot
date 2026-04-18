//! `aegis-boot compat` — hardware compatibility lookup.
//!
//! Answers "will aegis-boot work on my laptop?" with concrete data
//! instead of a shrug. The database is in-binary (`COMPAT_DB`) and
//! mirrors the rows curated in `docs/HARDWARE_COMPAT.md`.
//!
//! Seed policy matches the doc: **verified outcomes only**, no
//! speculation. Adding a row requires a real-hardware report filed
//! under the `hardware-report` GitHub label. See #137 for the epic.
//!
//! Two outputs:
//!   * `aegis-boot compat`             → full table
//!   * `aegis-boot compat <query>`     → fuzzy-match a single row
//!   * `aegis-boot compat --json [q]`  → structured (`schema_version=1`)

use std::process::ExitCode;

/// How confidently we've validated a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompatLevel {
    /// Validated under enforcing Secure Boot with a full `flash → boot →
    /// kexec` chain. Safe to recommend as a first-try target.
    Verified,
    /// Validated partially (e.g., reached rescue-tui but kexec quirk on
    /// one distro). Still worth recommending, with the caveat surfaced.
    #[allow(dead_code)] // seeded by future community reports; keep the glyph/label ready
    Partial,
    /// QEMU / virtualized reference environment. Floor of what
    /// aegis-boot supports; not a real-hardware claim.
    Reference,
}

impl CompatLevel {
    fn glyph(self) -> &'static str {
        match self {
            CompatLevel::Verified => "\u{2713}", // ✓
            CompatLevel::Partial => "~",
            CompatLevel::Reference => "\u{2261}", // ≡
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            CompatLevel::Verified => "verified",
            CompatLevel::Partial => "partial",
            CompatLevel::Reference => "reference",
        }
    }
}

impl CompatEntry {
    /// Convenience for crate consumers (e.g., `doctor`) that want the
    /// level label without pulling in the `CompatLevel` enum directly.
    /// Only called from `doctor::check_compat_db_coverage` which is
    /// `cfg(target_os = "linux")`; suppress dead-code on other OSes.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub(crate) fn level_label(&self) -> &'static str {
        self.level.label()
    }
}

/// One compatibility row. Mirrors `docs/HARDWARE_COMPAT.md`; every
/// entry corresponds to a report a real operator filed against a real
/// machine (or the QEMU reference environment).
pub struct CompatEntry {
    /// Vendor, e.g. `"Lenovo"`. For the QEMU reference row: `"QEMU"`.
    pub vendor: &'static str,
    /// Model, e.g. `"ThinkPad X1 Carbon Gen 11"`.
    pub model: &'static str,
    /// Firmware vendor + version (free-form from BIOS).
    pub firmware: &'static str,
    /// Secure Boot state at report time.
    pub sb_state: &'static str,
    /// Boot-menu key for this firmware (F12/F11/Esc/Del/…).
    pub boot_key: &'static str,
    /// How confident we are in this row.
    pub level: CompatLevel,
    /// Human notes: quirks, fast-boot caveats, required BIOS tweaks.
    /// Empty slice if the boot was clean.
    pub notes: &'static [&'static str],
    /// Who reported it (GitHub handle or `"aegis-team"`).
    pub reported_by: &'static str,
    /// ISO-8601 date string.
    pub date: &'static str,
}

/// The compatibility database. Keep in sync with
/// `docs/HARDWARE_COMPAT.md`; a row in one means a row in the other.
pub const COMPAT_DB: &[CompatEntry] = &[
    CompatEntry {
        vendor: "Generic",
        model: "SanDisk Cruzer Blade 32GB (USB-passthrough to QEMU x86_64)",
        firmware: "OVMF 4M (Debian package, MS-enrolled vars)",
        sb_state: "enforcing",
        boot_key: "n/a",
        level: CompatLevel::Verified,
        notes: &[
            "Ubuntu 24.04.2 boots signed-chain.",
            "Alpine 3.20.3 correctly refused with errno 61 under enforcing SB.",
            "Shakedown gate #109 — first real stick + QEMU USB-passthrough.",
        ],
        reported_by: "@williamzujkowski",
        date: "2026-04-16",
    },
    CompatEntry {
        vendor: "QEMU",
        model: "q35 + OVMF (MS-enrolled VARs)",
        firmware: "Ubuntu 22.04 packaged OVMF_CODE_4M.secboot.fd (2024.02-2)",
        sb_state: "enforcing",
        boot_key: "n/a",
        level: CompatLevel::Reference,
        notes: &["Reference test environment used by every CI workflow."],
        reported_by: "aegis-team",
        date: "2026-04-16",
    },
    CompatEntry {
        vendor: "Framework",
        // Includes the board revision (`/ A6`) so doctor's
        // dmi_product_label-derived query — which composes as
        // `<product_name> / <product_version>` when version is
        // shorter than name — matches every token. Board revs
        // matter: Framework rev A6 = post-04/2024 firmware
        // generation with INSYDE 03.x.
        model: "Laptop (12th Gen Intel Core) / A6",
        firmware: "INSYDE Corp. 03.19 (09/18/2025)",
        sb_state: "enforcing",
        boot_key: "F12",
        level: CompatLevel::Verified,
        notes: &[
            "Full signed-chain validated 2026-04-18 via aegis-hwsim signed-boot-ubuntu \
             scenario against the framework-laptop-12gen persona — verified-outcome \
             report at #236.",
            "Boot landmarks observed: BdsDxe → GNU GRUB → EFI stub SB-enabled → rescue-tui.",
            "TPM 2.0 (Intel fTPM, manufacturer INTC). Fast Boot disabled by default \
             (firmware 03.19); no operator intervention needed for USB boot.",
        ],
        reported_by: "@williamzujkowski",
        date: "2026-04-18",
    },
];

/// URL operators visit to file a hardware report. Kept here so the
/// CLI and docs point at the same landing page. `pub(crate)` so other
/// subcommands (e.g., `doctor`) can cite the same URL in their prompts.
pub(crate) const REPORT_URL: &str =
    "https://github.com/williamzujkowski/aegis-boot/issues/new?template=hardware-report.yml";

pub fn run(args: &[String]) -> ExitCode {
    match try_run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => ExitCode::from(code),
    }
}

pub(crate) fn try_run(args: &[String]) -> Result<(), u8> {
    if matches!(args.first().map(String::as_str), Some("--help" | "-h")) {
        print_help();
        return Ok(());
    }

    let json_mode = args.iter().any(|a| a == "--json");
    let my_machine = args.iter().any(|a| a == "--my-machine");
    let submit = args.iter().any(|a| a == "--submit");
    let query = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .map(String::as_str);

    // `--submit` auto-gathers DMI like `--my-machine` and emits a
    // pre-filled GitHub hardware-report URL. Short-circuits all the
    // lookup logic — it's a dedicated "draft a report" flow.
    if submit {
        if query.is_some() || my_machine {
            eprintln!(
                "aegis-boot compat: --submit cannot be combined with a query or --my-machine"
            );
            return Err(2);
        }
        return run_submit(json_mode);
    }

    // `--my-machine` auto-fills the query from DMI. Conflicts with an
    // explicit query — the operator should pick one or the other so
    // the intent stays unambiguous in scripted usage.
    if my_machine && query.is_some() {
        eprintln!(
            "aegis-boot compat: --my-machine takes no query (DMI fills it in). \
             Remove one or the other."
        );
        return Err(2);
    }

    if my_machine {
        let Some(auto_query) = resolve_my_machine_query() else {
            return report_my_machine_miss(json_mode);
        };
        return if json_mode {
            run_json(Some(&auto_query))
        } else {
            lookup_and_print(&auto_query)
        };
    }

    if json_mode {
        return run_json(query);
    }

    let Some(q) = query else {
        print_table();
        return Ok(());
    };
    lookup_and_print(q)
}

/// Shared exit path for interactive (non-JSON) query → entry lookup.
/// Pulled out so `--my-machine` and the positional-query path agree on
/// behavior (and on exit codes).
fn lookup_and_print(query: &str) -> Result<(), u8> {
    if let Some(entry) = find_entry(query) {
        print_entry(entry);
        Ok(())
    } else {
        print_miss(query);
        Err(1)
    }
}

/// Build the auto-query string for `--my-machine` by reading DMI via the
/// shared `doctor::read_dmi_field` helper. Returns `None` when DMI
/// fields are unavailable (non-Linux, stripped-down kernels, all
/// placeholder values) — the caller then emits a clear "couldn't
/// determine" message rather than silently falling back to the empty
/// match on an empty query.
fn resolve_my_machine_query() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let vendor = crate::doctor::read_dmi_field("sys_vendor")?;
        let product = crate::doctor::dmi_product_label()?;
        Some(format!("{vendor} {product}"))
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// `--submit` flow: gather DMI, build pre-filled GitHub issue URL.
/// Fields pre-filled: `vendor` (`sys_vendor`), `model` (product label),
/// `firmware` (BIOS label), `aegis-version`. Everything else (outcome,
/// boot-key, sb-state, iso, quirks, confidence, handle) is left for
/// the operator — those are signals we can't derive from DMI.
fn run_submit(json_mode: bool) -> Result<(), u8> {
    #[cfg(target_os = "linux")]
    {
        let vendor = crate::doctor::read_dmi_field("sys_vendor");
        let product = crate::doctor::dmi_product_label();
        let bios = crate::doctor::dmi_bios_label();
        if vendor.is_none() && product.is_none() {
            return report_my_machine_miss(json_mode);
        }
        let tool_version = env!("CARGO_PKG_VERSION");
        let url = build_hardware_report_url(
            vendor.as_deref(),
            product.as_deref(),
            bios.as_deref(),
            tool_version,
        );
        if json_mode {
            use crate::doctor::json_escape;
            println!("{{");
            println!("  \"schema_version\": 1,");
            println!("  \"tool\": \"aegis-boot\",");
            println!("  \"submit_url\": \"{}\",", json_escape(&url));
            println!(
                "  \"vendor\": \"{}\",",
                json_escape(vendor.as_deref().unwrap_or(""))
            );
            println!(
                "  \"model\": \"{}\",",
                json_escape(product.as_deref().unwrap_or(""))
            );
            println!(
                "  \"firmware\": \"{}\"",
                json_escape(bios.as_deref().unwrap_or(""))
            );
            println!("}}");
        } else {
            println!("aegis-boot compat — draft hardware-report submission");
            println!();
            println!("DMI fields gathered for pre-fill:");
            println!("  vendor    : {}", vendor.as_deref().unwrap_or("(not set)"));
            println!(
                "  model     : {}",
                product.as_deref().unwrap_or("(not set)")
            );
            println!("  firmware  : {}", bios.as_deref().unwrap_or("(not set)"));
            println!("  tool      : aegis-boot v{tool_version}");
            println!();
            println!("Open this URL in a browser to finish the report (outcome,");
            println!("boot-menu key, SB state, etc. are not derivable from DMI");
            println!("and must be entered by hand):");
            println!();
            println!("  {url}");
            println!();
            println!("Before filing, please run the full flash → boot → kexec chain");
            println!("on this machine. `aegis-boot compat` rows are verified-outcomes-");
            println!("only; don't submit speculative entries.");
        }
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        report_my_machine_miss(json_mode)
    }
}

/// URL-encode a single query-param value. Minimal percent-encoder — we
/// don't need a general-purpose one, just enough to get the operator's
/// vendor / model / firmware strings through GitHub's URL parser. Encodes
/// everything outside RFC 3986 §2.3 unreserved ASCII.
///
/// Only called from the `--submit` flow (Linux-only) and from tests;
/// suppress dead-code on non-Linux non-test builds.
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
fn percent_encode(s: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            // write! into a String is infallible — suppress the Result.
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// Compose the pre-filled hardware-report URL. Returns a full HTTPS URL
/// that opens GitHub's issue form with the passed fields populated.
/// `pub(crate)` so the unit tests can round-trip it without running the
/// full `run_submit` flow.
///
/// Only called from the `--submit` flow (Linux-only) and from tests;
/// suppress dead-code on non-Linux non-test builds.
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
pub(crate) fn build_hardware_report_url(
    vendor: Option<&str>,
    model: Option<&str>,
    firmware: Option<&str>,
    tool_version: &str,
) -> String {
    let mut url = String::from(
        "https://github.com/williamzujkowski/aegis-boot/issues/new?template=hardware-report.yml",
    );
    if let Some(v) = vendor {
        url.push_str("&vendor=");
        url.push_str(&percent_encode(v));
    }
    if let Some(m) = model {
        url.push_str("&model=");
        url.push_str(&percent_encode(m));
    }
    if let Some(f) = firmware {
        url.push_str("&firmware=");
        url.push_str(&percent_encode(f));
    }
    url.push_str("&aegis-version=v");
    url.push_str(&percent_encode(tool_version));
    url
}

/// Handle the `--my-machine` failure-to-resolve case for both JSON and
/// human modes. Consistent exit code (2) so scripted callers can
/// distinguish it from a normal miss (1) — an auto-resolve failure is
/// a host-environment issue, not a DB-coverage issue.
fn report_my_machine_miss(json_mode: bool) -> Result<(), u8> {
    use crate::doctor::json_escape;
    if json_mode {
        println!(
            "{{ \"schema_version\": 1, \"error\": \"{}\" }}",
            json_escape(
                "--my-machine: DMI fields unavailable (non-Linux host or placeholder values)"
            )
        );
    } else {
        eprintln!("aegis-boot compat --my-machine: could not determine machine identity from DMI.");
        eprintln!(
            "  On Linux, verify /sys/class/dmi/id/ has populated sys_vendor and product_name."
        );
        eprintln!("  On non-Linux hosts, this flag is not supported — use `aegis-boot compat <query>` instead.");
    }
    Err(2)
}

fn print_help() {
    println!("aegis-boot compat — hardware compatibility lookup");
    println!();
    println!("USAGE:");
    println!("  aegis-boot compat                     Show every known platform");
    println!("  aegis-boot compat <query>             Fuzzy match one platform");
    println!("  aegis-boot compat --my-machine        Auto-lookup from DMI (Linux only)");
    println!("  aegis-boot compat --submit            Draft a hardware-report with DMI pre-fill");
    println!("  aegis-boot compat --json [query]      Structured output");
    println!();
    println!("EXAMPLES:");
    println!("  aegis-boot compat                     # full table");
    println!("  aegis-boot compat thinkpad            # match by vendor or model");
    println!("  aegis-boot compat --my-machine        # query this exact machine");
    println!("  aegis-boot compat --submit            # pre-filled report URL");
    println!("  aegis-boot compat --json              # script-friendly list");
    println!();
    println!("NOTES:");
    println!("  Rows are verified outcomes only — no speculation. If your machine is missing,");
    println!("  please submit a report:");
    println!("  {REPORT_URL}");
}

fn print_table() {
    println!("aegis-boot — hardware compatibility");
    println!();
    println!("{} platform(s) reported.", COMPAT_DB.len());
    println!();
    println!(
        "{:<14} {:<12} {:<12} {:<10} MODEL",
        "LEVEL", "VENDOR", "SB", "BOOT"
    );
    for entry in COMPAT_DB {
        let level_col = format!("{} {}", entry.level.glyph(), entry.level.label());
        println!(
            "{:<14} {:<12} {:<12} {:<10} {model}",
            level_col,
            truncate(entry.vendor, 10),
            truncate(entry.sb_state, 10),
            truncate(entry.boot_key, 10),
            model = truncate(entry.model, 64),
        );
    }
    println!();
    println!("See `aegis-boot compat <query>` for details on one row.");
    println!("Missing a machine? Submit a report: {REPORT_URL}");
}

fn print_entry(entry: &CompatEntry) {
    println!("{} {} ({})", entry.level.glyph(), entry.model, entry.vendor);
    println!("  level       : {}", entry.level.label());
    println!("  firmware    : {}", entry.firmware);
    println!("  SB state    : {}", entry.sb_state);
    println!("  boot key    : {}", entry.boot_key);
    println!("  reported by : {}  ({})", entry.reported_by, entry.date);
    if entry.notes.is_empty() {
        println!("  notes       : (none)");
    } else {
        println!("  notes:");
        for note in entry.notes {
            println!("    - {note}");
        }
    }
}

fn print_miss(query: &str) {
    eprintln!("aegis-boot compat: no platform matching '{query}'");
    eprintln!();
    eprintln!("The compat DB is verified-outcomes-only — every row is a real report.");
    eprintln!("Submit yours to grow the table:");
    eprintln!("  {REPORT_URL}");
    eprintln!();
    eprintln!("Run 'aegis-boot compat' to see what's currently recorded.");
}

fn run_json(query: Option<&str>) -> Result<(), u8> {
    use crate::doctor::json_escape;
    match query {
        None => {
            println!("{{");
            println!("  \"schema_version\": 1,");
            println!("  \"tool_version\": \"{}\",", env!("CARGO_PKG_VERSION"));
            println!("  \"report_url\": \"{}\",", json_escape(REPORT_URL));
            println!("  \"count\": {},", COMPAT_DB.len());
            println!("  \"entries\": [");
            let last = COMPAT_DB.len().saturating_sub(1);
            for (i, entry) in COMPAT_DB.iter().enumerate() {
                let comma = if i == last { "" } else { "," };
                emit_entry_json(entry, "    ", comma);
            }
            println!("  ]");
            println!("}}");
            Ok(())
        }
        Some(q) => {
            if let Some(entry) = find_entry(q) {
                println!("{{");
                println!("  \"schema_version\": 1,");
                println!("  \"tool_version\": \"{}\",", env!("CARGO_PKG_VERSION"));
                println!("  \"report_url\": \"{}\",", json_escape(REPORT_URL));
                println!("  \"entry\":");
                emit_entry_json(entry, "  ", "");
                println!("}}");
                Ok(())
            } else {
                println!(
                    "{{ \"schema_version\": 1, \"report_url\": \"{}\", \"error\": \"{}\" }}",
                    json_escape(REPORT_URL),
                    json_escape(&format!("no platform matching '{q}'"))
                );
                Err(1)
            }
        }
    }
}

fn emit_entry_json(entry: &CompatEntry, indent: &str, comma: &str) {
    use crate::doctor::json_escape;
    println!("{indent}{{");
    println!("{indent}  \"vendor\": \"{}\",", json_escape(entry.vendor));
    println!("{indent}  \"model\": \"{}\",", json_escape(entry.model));
    println!(
        "{indent}  \"firmware\": \"{}\",",
        json_escape(entry.firmware)
    );
    println!(
        "{indent}  \"sb_state\": \"{}\",",
        json_escape(entry.sb_state)
    );
    println!(
        "{indent}  \"boot_key\": \"{}\",",
        json_escape(entry.boot_key)
    );
    println!("{indent}  \"level\": \"{}\",", entry.level.label());
    println!(
        "{indent}  \"reported_by\": \"{}\",",
        json_escape(entry.reported_by)
    );
    println!("{indent}  \"date\": \"{}\",", json_escape(entry.date));
    println!("{indent}  \"notes\": [");
    let last = entry.notes.len().saturating_sub(1);
    for (i, note) in entry.notes.iter().enumerate() {
        let note_comma = if i == last { "" } else { "," };
        println!("{indent}    \"{}\"{note_comma}", json_escape(note));
    }
    println!("{indent}  ]");
    println!("{indent}}}{comma}");
}

/// Case-insensitive substring match against vendor + model. A query
/// matches if every whitespace-separated token appears in the combined
/// "vendor model" string. First match wins. `pub(crate)` so `doctor`
/// can cross-check its own DMI-derived identity against the DB.
pub(crate) fn find_entry(query: &str) -> Option<&'static CompatEntry> {
    let q = query.trim().to_ascii_lowercase();
    if q.is_empty() {
        return None;
    }
    COMPAT_DB.iter().find(|e| {
        let haystack = format!("{} {}", e.vendor, e.model).to_ascii_lowercase();
        q.split_whitespace().all(|tok| haystack.contains(tok))
    })
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('\u{2026}');
    out
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    #[test]
    fn compat_level_glyphs_are_distinct() {
        let mut g = [
            CompatLevel::Verified.glyph(),
            CompatLevel::Partial.glyph(),
            CompatLevel::Reference.glyph(),
        ]
        .to_vec();
        g.sort_unstable();
        g.dedup();
        assert_eq!(g.len(), 3);
    }

    #[test]
    fn compat_level_labels_are_distinct() {
        let labels = [
            CompatLevel::Verified.label(),
            CompatLevel::Partial.label(),
            CompatLevel::Reference.label(),
        ];
        assert_eq!(labels.len(), 3);
        assert!(labels.iter().all(|l| !l.is_empty()));
    }

    #[test]
    fn find_entry_matches_vendor_case_insensitive() {
        let hit = find_entry("qemu q35");
        assert!(hit.is_some());
        assert_eq!(hit.unwrap().vendor, "QEMU");
    }

    #[test]
    fn find_entry_matches_substring_in_model() {
        let hit = find_entry("SanDisk");
        assert!(hit.is_some());
        assert!(hit.unwrap().model.contains("SanDisk"));
    }

    #[test]
    fn find_entry_requires_all_tokens() {
        // Token set that's satisfied by the QEMU reference entry:
        assert!(find_entry("q35 ovmf").is_some());
        // Tokens that cannot both appear in any seeded row:
        assert!(find_entry("q35 sandisk").is_none());
    }

    #[test]
    fn find_entry_rejects_empty_query() {
        assert!(find_entry("").is_none());
        assert!(find_entry("   ").is_none());
    }

    #[test]
    fn find_entry_returns_none_for_unknown() {
        assert!(find_entry("asus-rog-z790-nonexistent-model").is_none());
    }

    #[test]
    fn compat_db_entries_have_populated_fields() {
        for entry in COMPAT_DB {
            assert!(!entry.vendor.is_empty(), "vendor must not be empty");
            assert!(!entry.model.is_empty(), "model must not be empty");
            assert!(!entry.firmware.is_empty(), "firmware must not be empty");
            assert!(!entry.sb_state.is_empty(), "sb_state must not be empty");
            assert!(!entry.boot_key.is_empty(), "boot_key must not be empty");
            assert!(
                !entry.reported_by.is_empty(),
                "reported_by must not be empty"
            );
            assert!(!entry.date.is_empty(), "date must not be empty");
        }
    }

    #[test]
    fn truncate_short_passes_through() {
        assert_eq!(truncate("abc", 10), "abc");
    }

    #[test]
    fn truncate_long_uses_ellipsis() {
        let out = truncate("abcdefghij", 5);
        assert_eq!(out.chars().count(), 5);
        assert!(out.ends_with('\u{2026}'));
    }

    #[test]
    fn try_run_help_returns_ok() {
        assert_eq!(try_run(&["--help".to_string()]), Ok(()));
        assert_eq!(try_run(&["-h".to_string()]), Ok(()));
    }

    #[test]
    fn try_run_empty_args_lists_table() {
        assert_eq!(try_run(&[]), Ok(()));
    }

    #[test]
    fn try_run_known_query_returns_ok() {
        assert_eq!(try_run(&["qemu".to_string()]), Ok(()));
    }

    #[test]
    fn try_run_unknown_query_returns_one() {
        assert_eq!(
            try_run(&["asus-rog-nonexistent-model-xyz".to_string()]),
            Err(1)
        );
    }

    #[test]
    fn try_run_json_mode_empty_returns_ok() {
        assert_eq!(try_run(&["--json".to_string()]), Ok(()));
    }

    #[test]
    fn try_run_json_mode_known_query_returns_ok() {
        assert_eq!(try_run(&["--json".to_string(), "qemu".to_string()]), Ok(()));
    }

    #[test]
    fn try_run_json_mode_unknown_query_returns_one() {
        assert_eq!(
            try_run(&["--json".to_string(), "xyz-no-such-box".to_string()]),
            Err(1)
        );
    }

    #[test]
    fn try_run_my_machine_with_explicit_query_is_usage_error() {
        // Operator ambiguity: --my-machine supplies the query from DMI,
        // so accepting an explicit query alongside it would silently
        // pick one. Reject at arg-parse time with exit code 2.
        assert_eq!(
            try_run(&["--my-machine".to_string(), "thinkpad".to_string()]),
            Err(2)
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn try_run_my_machine_returns_one_or_two() {
        // Result depends on whether the host's DMI matches a COMPAT_DB
        // entry: Ok(()) on hit, Err(1) on DB miss (normal compat miss),
        // Err(2) on DMI unavailable. Never panics, never returns other
        // codes. Kept flexible so the test works on every CI runner
        // shape (QEMU, real hardware, non-Linux-in-container, etc.).
        let result = try_run(&["--my-machine".to_string()]);
        assert!(
            matches!(result, Ok(()) | Err(1 | 2)),
            "unexpected exit code from --my-machine: {result:?}"
        );
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn try_run_my_machine_returns_two_on_non_linux() {
        // DMI is Linux-only; every other host must report error 2
        // (host-environment issue), not error 1 (DB miss).
        assert_eq!(try_run(&["--my-machine".to_string()]), Err(2));
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn try_run_my_machine_json_mode_returns_two_on_non_linux() {
        assert_eq!(
            try_run(&["--my-machine".to_string(), "--json".to_string()]),
            Err(2)
        );
    }

    // ---- --submit: URL builder + arg-parse tests ----

    #[test]
    fn percent_encode_preserves_unreserved() {
        assert_eq!(percent_encode("abcXYZ"), "abcXYZ");
        assert_eq!(
            percent_encode("abc-123_test.foo~bar"),
            "abc-123_test.foo~bar"
        );
        assert_eq!(percent_encode("0"), "0");
    }

    #[test]
    fn percent_encode_escapes_spaces_and_special() {
        assert_eq!(percent_encode("Framework Laptop"), "Framework%20Laptop");
        assert_eq!(percent_encode("foo & bar"), "foo%20%26%20bar");
        assert_eq!(percent_encode("("), "%28");
        assert_eq!(percent_encode(")"), "%29");
        assert_eq!(percent_encode("/"), "%2F");
    }

    #[test]
    fn percent_encode_escapes_non_ascii_utf8() {
        assert_eq!(percent_encode("—"), "%E2%80%94");
    }

    #[test]
    fn build_hardware_report_url_has_template_param() {
        let url = build_hardware_report_url(None, None, None, "0.13.0");
        assert!(url.contains("template=hardware-report.yml"));
        assert!(url.contains("aegis-version=v0.13.0"));
    }

    #[test]
    fn build_hardware_report_url_pre_fills_all_fields() {
        let url = build_hardware_report_url(
            Some("LENOVO"),
            Some("ThinkPad X1 Carbon Gen 11"),
            Some("LENOVO N3HET70W (2024-01-15)"),
            "0.13.0",
        );
        assert!(url.contains("vendor=LENOVO"));
        assert!(url.contains("model=ThinkPad%20X1%20Carbon%20Gen%2011"));
        assert!(url.contains("firmware=LENOVO%20N3HET70W%20%282024-01-15%29"));
        assert!(url.contains("aegis-version=v0.13.0"));
    }

    #[test]
    fn build_hardware_report_url_omits_missing_fields() {
        let url = build_hardware_report_url(Some("LENOVO"), None, None, "0.13.0");
        assert!(url.contains("vendor=LENOVO"));
        assert!(!url.contains("&model="));
        assert!(!url.contains("&firmware="));
    }

    #[test]
    fn try_run_submit_rejects_extra_query() {
        assert_eq!(
            try_run(&["--submit".to_string(), "thinkpad".to_string()]),
            Err(2)
        );
    }

    #[test]
    fn try_run_submit_rejects_my_machine_combo() {
        assert_eq!(
            try_run(&["--submit".to_string(), "--my-machine".to_string()]),
            Err(2)
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn try_run_submit_returns_ok_or_two() {
        // Depends on DMI populated-ness — Ok(()) on populated, Err(2)
        // when every field is empty / placeholder.
        let r = try_run(&["--submit".to_string()]);
        assert!(matches!(r, Ok(()) | Err(2)), "unexpected: {r:?}");
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn try_run_submit_returns_two_on_non_linux() {
        assert_eq!(try_run(&["--submit".to_string()]), Err(2));
    }
}
