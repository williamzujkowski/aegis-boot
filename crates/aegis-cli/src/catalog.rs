// SPDX-License-Identifier: MIT OR Apache-2.0

//! `aegis-boot recommend` — operator-facing rendering of the curated
//! catalog. The catalog data + URL resolvers live in the `aegis-catalog`
//! workspace crate (#655 Phase 2A); this module owns only the CLI
//! dispatch + table/entry/help printing + the `--refresh --write`
//! source-mutation logic.
//!
//! Two outputs:
//!   * `aegis-boot recommend`           → prints the table
//!   * `aegis-boot recommend <slug>`    → prints download + verify recipe
//!
//! See `aegis_catalog` for the catalog policy / trust model rationale.

use std::process::ExitCode;

use aegis_catalog::{
    CATALOG, Category, Entry, ResolvedUrls, SbStatus, find_entry, humanize, truncate,
};

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

    // --refresh (#646): walk every Entry that has a resolver attached
    // and print a diff against the static URL. By default doesn't
    // mutate the catalog file. With --write, mutates the source
    // file in-place — the auto-PR CI workflow uses this to open a
    // PR with the diff.
    if args.first().map(String::as_str) == Some("--refresh") {
        let write = args.iter().any(|a| a == "--write");
        return run_refresh(write);
    }

    // --json [slug]: structured full-catalog output (or single-entry
    // when a slug follows the flag).
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

/// `aegis-boot recommend --refresh` — walk every Entry that has a
/// resolver, call it, and print a diff against the static URL. Does
/// NOT mutate the catalog file unless `--write` is set.
///
/// Exit codes:
///   0 — no drift (or no resolvers configured)
///   1 — at least one resolver returned a URL different from static
///   2 — at least one resolver errored (network / parse)
fn run_refresh(write: bool) -> ExitCode {
    let mut any_drift = false;
    let mut any_error = false;
    let mut drifts: Vec<(&'static str, ResolvedUrls)> = Vec::new();
    println!(
        "aegis-boot recommend --refresh{} — checking resolvers (#646)\n",
        if write { " --write" } else { "" }
    );
    for entry in CATALOG {
        let Some(resolver) = entry.resolver else {
            continue;
        };
        match resolver() {
            Ok(live) => {
                let drifted = live.iso_url != entry.iso_url
                    || live.sha256_url != entry.sha256_url
                    || live.sig_url != entry.sig_url;
                if drifted {
                    any_drift = true;
                    println!("[DRIFT] {}", entry.slug);
                    if live.iso_url != entry.iso_url {
                        println!("    iso     static: {}", entry.iso_url);
                        println!("            current: {}", live.iso_url);
                    }
                    if live.sha256_url != entry.sha256_url {
                        println!("    sha     static: {}", entry.sha256_url);
                        println!("            current: {}", live.sha256_url);
                    }
                    if live.sig_url != entry.sig_url {
                        println!("    sig     static: {}", entry.sig_url);
                        println!("            current: {}", live.sig_url);
                    }
                    drifts.push((entry.slug, live));
                } else {
                    println!("[OK]    {} (matches static)", entry.slug);
                }
            }
            Err(e) => {
                any_error = true;
                println!("[ERROR] {}: {}", entry.slug, e);
            }
        }
    }
    println!();
    if write && !drifts.is_empty() {
        match write_catalog_drifts(&drifts) {
            Ok(path) => println!("wrote {} drift fix(es) to {}", drifts.len(), path),
            Err(e) => {
                println!("--write failed: {e}");
                return ExitCode::from(2);
            }
        }
    }
    if any_error {
        ExitCode::from(2)
    } else if any_drift {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

/// Locate the in-tree `aegis-catalog/src/lib.rs` source file and
/// rewrite the URL fields for each drifted entry. The auto-PR CI
/// workflow (#650) uses this to land catalog updates without manual
/// edits.
///
/// After Phase 2A, the catalog data lives in `aegis-catalog`, NOT in
/// `aegis-cli/src/catalog.rs` — the candidate paths reflect that.
fn write_catalog_drifts(drifts: &[(&'static str, ResolvedUrls)]) -> Result<String, String> {
    let candidates = [
        std::path::PathBuf::from("crates/aegis-catalog/src/lib.rs"),
        std::path::PathBuf::from("../aegis-catalog/src/lib.rs"),
        std::path::PathBuf::from("../../crates/aegis-catalog/src/lib.rs"),
    ];
    let path = candidates
        .iter()
        .find(|p| p.is_file())
        .ok_or_else(|| {
            "couldn't locate crates/aegis-catalog/src/lib.rs from cwd; run from repo root"
                .to_string()
        })?
        .clone();
    let original =
        std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let mut text = original.clone();
    for (slug, live) in drifts {
        text = rewrite_entry_urls(&text, slug, live)?;
    }
    if text == original {
        return Ok(format!("{} (no changes)", path.display()));
    }
    std::fs::write(&path, text).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(path.display().to_string())
}

/// Rewrite a single entry's `iso_url`, `sha256_url`, `sig_url`
/// fields. Used internally by `write_catalog_drifts`.
fn rewrite_entry_urls(source: &str, slug: &str, live: &ResolvedUrls) -> Result<String, String> {
    let needle = format!("slug: \"{slug}\"");
    let start = source
        .find(&needle)
        .ok_or_else(|| format!("entry slug={slug:?} not found in catalog source"))?;
    let block_start = source[..start]
        .rfind("Entry {")
        .ok_or_else(|| format!("entry start `Entry {{` not found before slug={slug:?}"))?;
    let block_end_rel = source[start..]
        .find("\n    },")
        .ok_or_else(|| format!("entry end `\\n    }},` not found after slug={slug:?}"))?;
    let block_end = start + block_end_rel + "\n    },".len();
    let block = &source[block_start..block_end];
    let new_block = block
        .lines()
        .map(|line| {
            if let Some(prefix) = line.strip_prefix("        iso_url: \"") {
                let _ = prefix;
                format!("        iso_url: \"{}\",", live.iso_url)
            } else if let Some(prefix) = line.strip_prefix("        sha256_url: \"") {
                let _ = prefix;
                format!("        sha256_url: \"{}\",", live.sha256_url)
            } else if let Some(prefix) = line.strip_prefix("        sig_url: \"") {
                let _ = prefix;
                format!("        sig_url: \"{}\",", live.sig_url)
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(format!(
        "{}{}{}",
        &source[..block_start],
        new_block,
        &source[block_end..]
    ))
}

/// `aegis-boot recommend --json [slug]` — emit catalog entries as
/// structured JSON via the typed [`aegis_wire_formats::RecommendReport`]
/// envelope.
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
    for category in Category::print_order() {
        let group: Vec<&Entry> = CATALOG.iter().filter(|e| e.category == *category).collect();
        if group.is_empty() {
            continue;
        }
        println!("{} ({})", category.header(), group.len());
        println!(
            "  {:<32}  {:<38}  {:>7}  SECURE BOOT",
            "SLUG", "NAME", "SIZE"
        );
        println!(
            "  {}  {}  {}  {}",
            "-".repeat(32),
            "-".repeat(38),
            "-".repeat(7),
            "-".repeat(28),
        );
        for e in group {
            println!(
                "  {:<32}  {:<38}  {:>7}  {} {}",
                e.slug,
                truncate(e.name, 38),
                humanize(e.size_mib),
                e.sb.glyph(),
                e.sb.label()
            );
        }
        println!();
    }
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

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]
    use super::*;

    #[test]
    fn slugs_only_flag_is_recognized() {
        // Shell-completion scripts rely on --slugs-only emitting one
        // slug per line with exit code 0 and nothing else on stdout.
        let result = run(&["--slugs-only".to_string()]);
        let rendered = format!("{result:?}");
        assert!(
            rendered.contains("(0)") || rendered == "ExitCode(unix_exit_status(0))",
            "--slugs-only should exit 0, got {rendered}"
        );
    }

    #[test]
    fn rewrite_entry_urls_swaps_three_fields() {
        let source = r#"// preamble
pub const CATALOG: &[Entry] = &[
    Entry {
        slug: "demo",
        name: "Demo Distro 1.2.3",
        arch: "x86_64",
        size_mib: 1000,
        iso_url: "https://old.example/demo-1.2.3-amd64.iso",
        sha256_url: "https://old.example/SHA256SUMS",
        sig_url: "https://old.example/SHA256SUMS.gpg",
        sb: SbStatus::Signed("Demo CA"),
        purpose: "Demo entry.",
        resolver: None,
    },
];
"#;
        let live = ResolvedUrls {
            iso_url: "https://new.example/demo-1.2.4-amd64.iso".to_string(),
            sha256_url: "https://new.example/SHA256SUMS".to_string(),
            sig_url: "https://new.example/SHA256SUMS.gpg".to_string(),
        };
        let result = rewrite_entry_urls(source, "demo", &live).unwrap_or_else(|e| panic!("{e}"));
        assert!(result.contains("https://new.example/demo-1.2.4-amd64.iso"));
        assert!(result.contains("https://new.example/SHA256SUMS"));
        assert!(result.contains("https://new.example/SHA256SUMS.gpg"));
        assert!(result.contains("Demo Distro 1.2.3"));
        assert!(result.contains("size_mib: 1000"));
        assert!(result.contains("Demo CA"));
        assert!(!result.contains("https://old.example/demo-1.2.3-amd64.iso"));
    }

    #[test]
    fn rewrite_entry_urls_errors_on_unknown_slug() {
        let source = "pub const CATALOG: &[Entry] = &[];";
        let live = ResolvedUrls {
            iso_url: "x".to_string(),
            sha256_url: "y".to_string(),
            sig_url: "z".to_string(),
        };
        let err = rewrite_entry_urls(source, "nope", &live)
            .err()
            .unwrap_or_else(|| panic!("should fail on unknown slug"));
        assert!(err.contains("nope"));
    }

    #[test]
    fn rewrite_entry_urls_preserves_comments_inside_entry_block() {
        // Comments interspersed with URL fields must survive — they
        // carry the rationale (e.g. "Debian publishes SHA512SUMS").
        let source = r#"pub const CATALOG: &[Entry] = &[
    Entry {
        slug: "demo",
        name: "X",
        arch: "x86_64",
        size_mib: 1,
        iso_url: "https://old/iso",
        // important rationale comment
        sha256_url: "https://old/sha",
        sig_url: "https://old/sig",
        sb: SbStatus::Signed("X"),
        purpose: ".",
        resolver: None,
    },
];
"#;
        let live = ResolvedUrls {
            iso_url: "https://new/iso".to_string(),
            sha256_url: "https://new/sha".to_string(),
            sig_url: "https://new/sig".to_string(),
        };
        let result = rewrite_entry_urls(source, "demo", &live).unwrap_or_else(|e| panic!("{e}"));
        assert!(result.contains("// important rationale comment"));
        assert!(result.contains("https://new/iso"));
    }
}
