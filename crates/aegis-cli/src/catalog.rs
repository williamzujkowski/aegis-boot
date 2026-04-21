// SPDX-License-Identifier: MIT OR Apache-2.0

//! `aegis-boot recommend` — curated catalog of known-good ISOs.
//!
//! The catalog is an in-binary list of ISO entries with their canonical
//! download URLs and the URLs of their project-signed SHA256SUMS files.
//! No checksums are pinned in this file: they're fetched from the
//! project's own SHA256SUMS at verify time, with the GPG/minisign
//! signature on that file providing the trust anchor (whoever the
//! project trusts to sign their releases is who we trust here).
//!
//! Two outputs:
//!   * `aegis-boot recommend`           → prints the table
//!   * `aegis-boot recommend <slug>`    → prints download + verify recipe
//!
//! A future `aegis-boot fetch <slug>` will automate the manual recipe
//! shown by the second form. Tracked under epic #136.
//!
//! ## Why not pin SHA-256 in this file?
//!
//! Distros release point versions on a cadence we can't track in
//! commits. Pinning a hash here would make the catalog wrong within
//! weeks of every release. Pointing at the project's *signed*
//! SHA256SUMS keeps the catalog evergreen while preserving cryptographic
//! verification — the project's release-signing key is the trust anchor.

use std::process::ExitCode;

/// Secure Boot posture of the ISO's kernel under aegis-boot.
#[derive(Debug, Clone, Copy)]
pub enum SbStatus {
    /// Kernel is signed by a CA in shim's built-in keyring; boots
    /// without MOK enrollment. The string names the signing CA.
    Signed(&'static str),
    /// Kernel is unsigned (or signed by a CA shim doesn't trust);
    /// operator must MOK-enroll the distro's signing key first.
    /// See `docs/UNSIGNED_KERNEL.md`.
    UnsignedNeedsMok,
    /// We haven't validated this ISO end-to-end. Use at your own risk;
    /// please file a result via `aegis-boot doctor --report` (when
    /// available) so we can promote it to one of the above.
    #[allow(dead_code)] // reserved for future catalog entries we're not yet sure about
    Unknown,
}

impl SbStatus {
    fn glyph(self) -> &'static str {
        match self {
            SbStatus::Signed(_) => "\u{2713}",        // ✓
            SbStatus::UnsignedNeedsMok => "\u{2717}", // ✗
            SbStatus::Unknown => "?",
        }
    }

    fn label(self) -> String {
        match self {
            SbStatus::Signed(ca) => format!("signed ({ca})"),
            SbStatus::UnsignedNeedsMok => "unsigned (MOK needed)".to_string(),
            SbStatus::Unknown => "unknown".to_string(),
        }
    }
}

/// One catalog entry.
pub struct Entry {
    /// Stable slug operators type: `ubuntu-24.04-live-server`.
    pub slug: &'static str,
    /// Human-readable name.
    pub name: &'static str,
    /// Architecture marker (`x86_64` / `aarch64`).
    pub arch: &'static str,
    /// Approximate size in MiB; informational.
    pub size_mib: u32,
    /// Canonical download URL for the ISO.
    pub iso_url: &'static str,
    /// URL of the project's signed SHA256SUMS file.
    pub sha256_url: &'static str,
    /// URL of the GPG/minisign signature on SHA256SUMS.
    pub sig_url: &'static str,
    /// SB posture under aegis-boot.
    pub sb: SbStatus,
    /// One-line reason an operator might want this image.
    pub purpose: &'static str,
}

/// The catalog itself. Keep entries alphabetically sorted by slug.
///
/// Pinned URLs are the project's "current stable" pages (releases.ubuntu.com,
/// getfedora.org, etc.). When point releases bump, the URL stays valid for
/// at least one cycle; older releases tend to move to /old-releases/ paths.
/// Update entries when a major version ships, not for every point release.
// Only entries whose URLs verify under `scripts/catalog-revalidate.sh`
// are listed here. Many speculative / upstream-rotted entries were
// removed in a cleanup pass — see issue #156 + CATALOG_POLICY.md.
//
// When proposing an addition: run the revalidate script locally
// before opening the PR and confirm all three URLs return 2xx.
pub const CATALOG: &[Entry] = &[
    Entry {
        slug: "almalinux-9-minimal",
        name: "AlmaLinux 9 Minimal",
        arch: "x86_64",
        size_mib: 1700,
        iso_url: "https://repo.almalinux.org/almalinux/9/isos/x86_64/AlmaLinux-9-latest-x86_64-minimal.iso",
        sha256_url: "https://repo.almalinux.org/almalinux/9/isos/x86_64/CHECKSUM",
        // AlmaLinux ships a PGP-clearsigned CHECKSUM — the signature
        // is embedded in the same file, no separate .asc exists.
        // Pointing sig_url at the same CHECKSUM URL means revalidate
        // sees OK; `aegis-boot fetch` will need clearsign-aware
        // verification (tracked in fetch follow-up).
        sig_url: "https://repo.almalinux.org/almalinux/9/isos/x86_64/CHECKSUM",
        sb: SbStatus::Signed("Red Hat / AlmaLinux"),
        purpose: "Free RHEL-rebuild minimal installer. Cross-distro kexec quirk possible.",
    },
    Entry {
        slug: "alpine-3.20-standard",
        name: "Alpine Linux 3.20 Standard",
        arch: "x86_64",
        size_mib: 198,
        iso_url: "https://dl-cdn.alpinelinux.org/alpine/v3.20/releases/x86_64/alpine-standard-3.20.3-x86_64.iso",
        sha256_url: "https://dl-cdn.alpinelinux.org/alpine/v3.20/releases/x86_64/alpine-standard-3.20.3-x86_64.iso.sha256",
        sig_url: "https://dl-cdn.alpinelinux.org/alpine/v3.20/releases/x86_64/alpine-standard-3.20.3-x86_64.iso.asc",
        sb: SbStatus::UnsignedNeedsMok,
        purpose: "Minimal recovery / forensic shell. Tiny footprint.",
    },
    Entry {
        slug: "linuxmint-22-cinnamon",
        name: "Linux Mint 22 Cinnamon",
        arch: "x86_64",
        size_mib: 2900,
        iso_url: "https://mirrors.edge.kernel.org/linuxmint/stable/22/linuxmint-22-cinnamon-64bit.iso",
        sha256_url: "https://mirrors.edge.kernel.org/linuxmint/stable/22/sha256sum.txt",
        sig_url: "https://mirrors.edge.kernel.org/linuxmint/stable/22/sha256sum.txt.gpg",
        sb: SbStatus::Signed("Linux Mint"),
        purpose: "Friendly Ubuntu-derived desktop. Common operator install target.",
    },
    Entry {
        slug: "rocky-9-minimal",
        name: "Rocky Linux 9 Minimal",
        arch: "x86_64",
        size_mib: 1900,
        iso_url: "https://download.rockylinux.org/pub/rocky/9/isos/x86_64/Rocky-9-latest-x86_64-minimal.iso",
        sha256_url: "https://download.rockylinux.org/pub/rocky/9/isos/x86_64/CHECKSUM",
        // Rocky ships a PGP-clearsigned CHECKSUM (same pattern as
        // AlmaLinux). See the note on almalinux-9-minimal.
        sig_url: "https://download.rockylinux.org/pub/rocky/9/isos/x86_64/CHECKSUM",
        sb: SbStatus::Signed("Rocky Linux"),
        purpose: "Free RHEL-rebuild minimal installer. Cross-distro kexec quirk possible.",
    },
    Entry {
        slug: "ubuntu-24.04-live-server",
        name: "Ubuntu Server 24.04.2 LTS (live-server)",
        arch: "x86_64",
        size_mib: 2600,
        iso_url: "https://releases.ubuntu.com/24.04.2/ubuntu-24.04.2-live-server-amd64.iso",
        sha256_url: "https://releases.ubuntu.com/24.04.2/SHA256SUMS",
        sig_url: "https://releases.ubuntu.com/24.04.2/SHA256SUMS.gpg",
        sb: SbStatus::Signed("Canonical CA"),
        purpose: "Ubuntu LTS server installer. Validated under aegis-boot v0.12.0 #109.",
    },
    Entry {
        slug: "ubuntu-24.04-desktop",
        name: "Ubuntu Desktop 24.04.2 LTS",
        arch: "x86_64",
        size_mib: 5800,
        iso_url: "https://releases.ubuntu.com/24.04.2/ubuntu-24.04.2-desktop-amd64.iso",
        sha256_url: "https://releases.ubuntu.com/24.04.2/SHA256SUMS",
        sig_url: "https://releases.ubuntu.com/24.04.2/SHA256SUMS.gpg",
        sb: SbStatus::Signed("Canonical CA"),
        purpose: "Ubuntu LTS desktop installer. >4 GB → requires ext4 data partition.",
    },
];

/// Entry point for `aegis-boot recommend [slug] | [--slugs-only]`.
pub fn run(args: &[String]) -> ExitCode {
    if args.first().map(String::as_str) == Some("--help")
        || args.first().map(String::as_str) == Some("-h")
    {
        print_help();
        return ExitCode::SUCCESS;
    }

    // Machine-readable slug enumeration for shell completion scripts
    // and other tooling. One slug per line on stdout; no table, no
    // header. Keep this format stable — completion scripts depend on
    // it line-for-line.
    if args.first().map(String::as_str) == Some("--slugs-only") {
        for entry in CATALOG {
            println!("{}", entry.slug);
        }
        return ExitCode::SUCCESS;
    }

    // --json [slug]: structured full-catalog output (or single-entry
    // when a slug follows the flag). Complements --slugs-only (line-
    // per-slug) with the full field set — each entry's URLs, size,
    // and Secure-Boot posture in one document. Stable schema_version=1.
    let json_mode = args.iter().any(|a| a == "--json");
    if json_mode {
        let slug_arg = args.iter().find(|a| !a.starts_with("--"));
        return run_json(slug_arg.map(String::as_str));
    }

    let Some(slug) = args.first() else {
        print_table();
        return ExitCode::SUCCESS;
    };
    if let Some(entry) = find_entry(slug) {
        print_entry(entry);
        ExitCode::SUCCESS
    } else {
        eprintln!("aegis-boot recommend: no catalog entry matching '{slug}'");
        eprintln!("run 'aegis-boot recommend' to see available slugs");
        ExitCode::from(1)
    }
}

/// `aegis-boot recommend --json [slug]` — emit catalog entries as
/// structured JSON via the typed [`aegis_wire_formats::RecommendReport`]
/// envelope. Phase 4b-6 of #286 migrated this from hand-rolled
/// `println!()` chains. Wire contract pinned by
/// `docs/reference/schemas/aegis-boot-recommend.schema.json`.
fn run_json(slug: Option<&str>) -> ExitCode {
    match slug {
        None => {
            let entries: Vec<aegis_wire_formats::RecommendEntry> =
                CATALOG.iter().map(entry_to_recommend).collect();
            let report = aegis_wire_formats::RecommendReport::Catalog(
                aegis_wire_formats::RecommendCatalogReport {
                    schema_version: aegis_wire_formats::RECOMMEND_SCHEMA_VERSION,
                    tool_version: env!("CARGO_PKG_VERSION").to_string(),
                    count: u32::try_from(entries.len()).unwrap_or(u32::MAX),
                    entries,
                },
            );
            emit_recommend_report(&report);
            ExitCode::SUCCESS
        }
        Some(slug) => {
            let Some(entry) = find_entry(slug) else {
                let report = aegis_wire_formats::RecommendReport::Miss(
                    aegis_wire_formats::RecommendMissReport {
                        schema_version: aegis_wire_formats::RECOMMEND_SCHEMA_VERSION,
                        error: format!("no catalog entry matching '{slug}'"),
                    },
                );
                emit_recommend_report(&report);
                return ExitCode::from(1);
            };
            let report = aegis_wire_formats::RecommendReport::Single(
                aegis_wire_formats::RecommendSingleReport {
                    schema_version: aegis_wire_formats::RECOMMEND_SCHEMA_VERSION,
                    tool_version: env!("CARGO_PKG_VERSION").to_string(),
                    entry: entry_to_recommend(entry),
                },
            );
            emit_recommend_report(&report);
            ExitCode::SUCCESS
        }
    }
}

fn emit_recommend_report(report: &aegis_wire_formats::RecommendReport) {
    match serde_json::to_string_pretty(report) {
        Ok(body) => println!("{body}"),
        Err(e) => eprintln!("aegis-boot recommend: failed to serialize --json envelope: {e}"),
    }
}

/// Map the local `Entry` struct onto the wire-format
/// [`aegis_wire_formats::RecommendEntry`]. Local type uses `SbStatus`
/// enum; wire format flattens to a string via the existing
/// `"signed:<vendor>"` / `"unsigned-needs-mok"` / `"unknown"`
/// convention.
fn entry_to_recommend(entry: &Entry) -> aegis_wire_formats::RecommendEntry {
    let sb = match entry.sb {
        SbStatus::Signed(vendor) => format!("signed:{vendor}"),
        SbStatus::UnsignedNeedsMok => "unsigned-needs-mok".to_string(),
        SbStatus::Unknown => "unknown".to_string(),
    };
    aegis_wire_formats::RecommendEntry {
        slug: entry.slug.to_string(),
        name: entry.name.to_string(),
        arch: entry.arch.to_string(),
        size_mib: entry.size_mib,
        iso_url: entry.iso_url.to_string(),
        sha256_url: entry.sha256_url.to_string(),
        sig_url: entry.sig_url.to_string(),
        sb,
        purpose: entry.purpose.to_string(),
    }
}

fn print_help() {
    println!("aegis-boot recommend — curated ISO catalog");
    println!();
    println!("USAGE:");
    println!("  aegis-boot recommend               List all catalog entries (human table)");
    println!("  aegis-boot recommend <slug>        Show download + verify recipe");
    println!("  aegis-boot recommend --slugs-only  One slug per line (for shell completion)");
    println!("  aegis-boot recommend --json [slug] Full entry details as JSON");
    println!();
    println!("EXAMPLES:");
    println!("  aegis-boot recommend");
    println!("  aegis-boot recommend ubuntu-24.04-live-server");
    println!("  aegis-boot recommend --json | jq '.entries[].slug'");
}

fn print_table() {
    println!("Curated ISO catalog ({} entries):", CATALOG.len());
    println!();
    println!(
        "  {:<28}  {:<38}  {:>7}  SECURE BOOT",
        "SLUG", "NAME", "SIZE"
    );
    println!(
        "  {:<28}  {:<38}  {:>7}  {}",
        "-".repeat(28),
        "-".repeat(38),
        "-".repeat(7),
        "-".repeat(28),
    );
    for e in CATALOG {
        println!(
            "  {:<28}  {:<38}  {:>7}  {} {}",
            e.slug,
            truncate(e.name, 38),
            humanize(e.size_mib),
            e.sb.glyph(),
            e.sb.label()
        );
    }
    println!();
    println!("Use 'aegis-boot recommend <SLUG>' for download + verify instructions.");
    println!("Entries marked '\u{2717} unsigned (MOK needed)' require explicit MOK");
    println!("enrollment of the distro's signing key — see docs/UNSIGNED_KERNEL.md.");
}

fn print_entry(e: &Entry) {
    println!("{} — {}", e.name, e.sb.label());
    println!();
    println!("  Slug:        {}", e.slug);
    println!("  Architecture: {}", e.arch);
    println!(
        "  Approx size:  {} ({} MiB)",
        humanize(e.size_mib),
        e.size_mib
    );
    println!("  Purpose:      {}", e.purpose);
    println!();
    println!("  ISO URL:      {}", e.iso_url);
    println!("  SHA256SUMS:   {}", e.sha256_url);
    println!("  Signature:    {}", e.sig_url);
    println!();
    println!("Manual download + verify + add (Linux host):");
    println!();
    println!("  curl -LO '{}'", e.iso_url);
    println!("  curl -LO '{}'", e.sha256_url);
    println!("  curl -LO '{}'", e.sig_url);
    match e.sb {
        SbStatus::Signed(_) | SbStatus::Unknown => {
            println!();
            println!("  # Verify the SHA256SUMS file's signature using the project's");
            println!("  # signing key (consult the project for key fingerprint), then:");
            println!("  sha256sum -c <SHA256SUMS> --ignore-missing");
            println!("  aegis-boot add <iso-filename>");
        }
        SbStatus::UnsignedNeedsMok => {
            println!();
            println!("  # The ISO's kernel is unsigned. After verifying the ISO checksum");
            println!("  # against the signed SHA256SUMS, you also need to MOK-enroll the");
            println!("  # distro's kernel signing key — see docs/UNSIGNED_KERNEL.md.");
            println!("  sha256sum -c <SHA256SUMS> --ignore-missing");
            println!("  aegis-boot add <iso-filename>");
            println!("  # Place the distro's signing public key alongside the ISO:");
            println!("  cp <distro-signing-key>.pub /run/media/aegis-isos/<iso-filename>.pub");
        }
    }
    println!();
    println!("Once verified + on the stick, the rescue-tui will show this ISO with the");
    println!("verification verdict. Tracked in epic #136 for future `aegis-boot fetch <slug>`");
    println!("which will automate the manual recipe above.");
}

pub(crate) fn find_entry(slug: &str) -> Option<&'static Entry> {
    let s = slug.to_ascii_lowercase();
    // Exact match first.
    if let Some(e) = CATALOG.iter().find(|e| e.slug.eq_ignore_ascii_case(&s)) {
        return Some(e);
    }
    // Prefix match if exactly one entry matches.
    let prefix_matches: Vec<_> = CATALOG
        .iter()
        .filter(|e| e.slug.to_ascii_lowercase().starts_with(&s))
        .collect();
    if prefix_matches.len() == 1 {
        return Some(prefix_matches[0]);
    }
    None
}

fn humanize(mib: u32) -> String {
    if mib >= 1024 {
        format!("{:.1} GiB", f64::from(mib) / 1024.0)
    } else {
        format!("{mib} MiB")
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut out = s.chars().take(max - 1).collect::<String>();
        out.push('\u{2026}'); // …
        out
    }
}

#[cfg(test)]
mod tests {
    // Tests routinely use expect/unwrap for clarity — workspace lints
    // upgrade these to ERROR under -D warnings, so we opt out at the
    // test-module scope.
    #![allow(clippy::expect_used, clippy::unwrap_used)]
    use super::*;

    #[test]
    fn catalog_has_at_least_four_entries() {
        // Sanity gate: someone shouldn't accidentally empty out the
        // catalog and have CI still pass. 4 matches the post-rot-cleanup
        // minimum (#156) — only entries with fully-verified URLs ship.
        // Raise this floor in a dedicated PR as the catalog grows back.
        assert!(
            CATALOG.len() >= 4,
            "catalog shrank to {} entries — intentional?",
            CATALOG.len()
        );
    }

    #[test]
    fn catalog_slugs_are_unique() {
        let mut slugs: Vec<_> = CATALOG.iter().map(|e| e.slug).collect();
        slugs.sort_unstable();
        let pre = slugs.len();
        slugs.dedup();
        assert_eq!(pre, slugs.len(), "duplicate slug in CATALOG");
    }

    #[test]
    fn slugs_only_flag_is_recognized() {
        // Shell-completion scripts rely on --slugs-only emitting one
        // slug per line with exit code 0 and nothing else on stdout.
        // This test guards the argument-parsing contract; the actual
        // printing is covered by running the binary in CI. (Completion)
        let result = run(&["--slugs-only".to_string()]);
        // ExitCode doesn't impl PartialEq; render via Debug to probe.
        let rendered = format!("{result:?}");
        assert!(
            rendered.contains("(0)") || rendered == "ExitCode(unix_exit_status(0))",
            "--slugs-only should exit 0, got {rendered}"
        );
    }

    #[test]
    fn catalog_slugs_are_kebab_case() {
        for e in CATALOG {
            assert!(
                e.slug
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.'),
                "slug not kebab-case: {}",
                e.slug
            );
            assert!(!e.slug.is_empty(), "empty slug");
        }
    }

    #[test]
    fn catalog_urls_are_https() {
        for e in CATALOG {
            assert!(
                e.iso_url.starts_with("https://"),
                "{} iso_url not https",
                e.slug
            );
            assert!(
                e.sha256_url.starts_with("https://"),
                "{} sha256_url not https",
                e.slug
            );
            assert!(
                e.sig_url.starts_with("https://"),
                "{} sig_url not https",
                e.slug
            );
        }
    }

    #[test]
    fn catalog_sizes_are_plausible() {
        for e in CATALOG {
            assert!(e.size_mib >= 1, "{} size_mib too small", e.slug);
            assert!(
                e.size_mib < 16_000,
                "{} size_mib > 16 GiB seems wrong",
                e.slug
            );
        }
    }

    #[test]
    fn find_entry_exact_match() {
        let e = find_entry("ubuntu-24.04-live-server").expect("present in catalog");
        assert_eq!(e.slug, "ubuntu-24.04-live-server");
    }

    #[test]
    fn find_entry_case_insensitive() {
        let e = find_entry("UBUNTU-24.04-LIVE-SERVER");
        assert!(e.is_some());
    }

    #[test]
    fn find_entry_prefix_when_unique() {
        // "almalinux-9-minimal" is the only entry starting with "alma"
        // in the current (post-cleanup, #156) catalog.
        let e = find_entry("alma").expect("alma prefix matches uniquely");
        assert!(e.slug.starts_with("alma"));
    }

    #[test]
    fn find_entry_ambiguous_prefix_returns_none() {
        // "ubuntu" is a prefix of two entries → no unique match
        let e = find_entry("ubuntu");
        assert!(e.is_none());
    }

    #[test]
    fn find_entry_unknown_returns_none() {
        assert!(find_entry("not-a-real-distro").is_none());
    }

    #[test]
    fn humanize_under_1gib() {
        assert_eq!(humanize(198), "198 MiB");
        assert_eq!(humanize(1023), "1023 MiB");
    }

    #[test]
    fn humanize_at_or_over_1gib() {
        assert_eq!(humanize(1024), "1.0 GiB");
        assert_eq!(humanize(2600), "2.5 GiB");
    }

    #[test]
    fn truncate_short_passes_through() {
        assert_eq!(truncate("abc", 10), "abc");
    }

    #[test]
    fn truncate_long_uses_ellipsis() {
        let out = truncate("abcdefghij", 5);
        // 4 chars + ellipsis
        assert_eq!(out.chars().count(), 5);
        assert!(out.ends_with('\u{2026}'));
    }

    #[test]
    fn sb_status_glyph_is_distinct() {
        let mut glyphs = [
            SbStatus::Signed("X").glyph(),
            SbStatus::UnsignedNeedsMok.glyph(),
            SbStatus::Unknown.glyph(),
        ]
        .to_vec();
        glyphs.sort_unstable();
        glyphs.dedup();
        assert_eq!(glyphs.len(), 3, "glyphs should be distinct");
    }
}
