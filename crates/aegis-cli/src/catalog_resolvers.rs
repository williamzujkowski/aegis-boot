// SPDX-License-Identifier: MIT OR Apache-2.0

//! Per-distro URL resolvers for the [`crate::catalog`] (#646).
//!
//! Static catalog entries embed a specific point release in their
//! filenames (`debian-13.4.0-amd64-netinst.iso`,
//! `pop-os_24.04_amd64_intel_9.iso`). Every release bump means a
//! manual catalog edit. The resolver framework discovers the current
//! filename programmatically by walking each project's directory
//! listing or following its "latest" redirect.
//!
//! ## Trust model
//!
//! Resolvers do NOT verify ISO contents — they only update the URL
//! we tell operators to download from. The signed SHA256SUMS still
//! anchors trust at fetch time; if a resolver returns a malicious
//! URL, the GPG signature on SHA256SUMS at that URL won't validate
//! against the project's known public key. So resolver compromise
//! is detectable via the existing trust chain.
//!
//! ## Why curl, not reqwest?
//!
//! Matches `crates/aegis-cli/src/fetch.rs`'s convention. Keeps the
//! static-musl release binary small (no Rust HTTP stack).

use std::process::Command;

/// Result of a successful URL resolution. Mirrors the static
/// catalog-entry fields — when present, callers prefer these over
/// the [`crate::catalog::Entry`]'s static defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::struct_field_names)] // mirrors Entry's *_url field names
pub struct ResolvedUrls {
    pub iso_url: String,
    pub sha256_url: String,
    pub sig_url: String,
}

/// Errors a resolver can raise.
#[derive(Debug, thiserror::Error)]
pub enum ResolverError {
    #[error("network: failed to fetch {url}: {detail}")]
    Network { url: String, detail: String },
    #[error("parse: no matching ISO filename in listing at {url}")]
    NoMatch { url: String },
    #[error("parse: response from {url} was not valid utf-8")]
    NotUtf8 { url: String },
}

/// Run `curl -fsSL <url>` and return the response body. Used by
/// resolvers that need to scrape a directory listing for the
/// current ISO filename.
fn http_get(url: &str) -> Result<String, ResolverError> {
    let out = Command::new("curl")
        .args(["-fsSL", "--max-time", "30", url])
        .output()
        .map_err(|e| ResolverError::Network {
            url: url.to_string(),
            detail: format!("spawn: {e}"),
        })?;
    if !out.status.success() {
        return Err(ResolverError::Network {
            url: url.to_string(),
            detail: format!(
                "curl exit {}: {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        });
    }
    String::from_utf8(out.stdout).map_err(|_| ResolverError::NotUtf8 {
        url: url.to_string(),
    })
}

/// Debian — list `cdimage.debian.org/debian-cd/current/amd64/iso-cd/`
/// and find the highest `debian-X.Y.Z-amd64-netinst.iso` filename.
/// SHA512SUMS / SHA512SUMS.sign live in the same directory.
pub fn debian_netinst() -> Result<ResolvedUrls, ResolverError> {
    const BASE: &str = "https://cdimage.debian.org/debian-cd/current/amd64/iso-cd/";
    let html = http_get(BASE)?;
    resolve_debian_netinst_from_html(&html, BASE)
}

fn resolve_debian_netinst_from_html(html: &str, base: &str) -> Result<ResolvedUrls, ResolverError> {
    // Extract `debian-X.Y.Z-amd64-netinst.iso` filenames from the
    // directory listing. Prefer the highest version.
    let re = simple_regex_match_all(html, r"debian-", "-amd64-netinst.iso");
    let mut versions: Vec<String> = re
        .into_iter()
        // s is the inner version segment like "13.4.0"; reassemble
        // into the full filename for sort + use.
        .map(|s| format!("debian-{s}-amd64-netinst.iso"))
        .collect();
    versions.sort();
    versions.dedup();
    let latest = versions.last().ok_or(ResolverError::NoMatch {
        url: base.to_string(),
    })?;
    Ok(ResolvedUrls {
        iso_url: format!("{base}{latest}"),
        sha256_url: format!("{base}SHA512SUMS"),
        sig_url: format!("{base}SHA512SUMS.sign"),
    })
}

/// Ubuntu — list `releases.ubuntu.com/24.04/` and find the highest
/// `ubuntu-24.04.X-live-server-amd64.iso` filename. The 24.04
/// directory stays the canonical LTS path; point releases (.0, .1,
/// .2, .3, ...) accumulate as new files in the same directory.
pub fn ubuntu_24_04_live_server() -> Result<ResolvedUrls, ResolverError> {
    const BASE: &str = "https://releases.ubuntu.com/24.04/";
    let html = http_get(BASE)?;
    resolve_ubuntu_24_04_with_html(&html, BASE, "live-server")
}

/// Ubuntu Desktop — same shape as live-server, different infix.
pub fn ubuntu_24_04_desktop() -> Result<ResolvedUrls, ResolverError> {
    const BASE: &str = "https://releases.ubuntu.com/24.04/";
    let html = http_get(BASE)?;
    resolve_ubuntu_24_04_with_html(&html, BASE, "desktop")
}

fn resolve_ubuntu_24_04_with_html(
    html: &str,
    base: &str,
    variant: &str,
) -> Result<ResolvedUrls, ResolverError> {
    // Look for `ubuntu-24.04.X-<variant>-amd64.iso` filenames.
    let prefix = "ubuntu-";
    let suffix = format!("-{variant}-amd64.iso");
    let mut versions: Vec<String> = simple_regex_match_all(html, prefix, &suffix);
    versions.sort();
    versions.dedup();
    let latest = versions.last().ok_or(ResolverError::NoMatch {
        url: base.to_string(),
    })?;
    let filename = format!("ubuntu-{latest}-{variant}-amd64.iso");
    Ok(ResolvedUrls {
        iso_url: format!("{base}{filename}"),
        sha256_url: format!("{base}SHA256SUMS"),
        sig_url: format!("{base}SHA256SUMS.gpg"),
    })
}

/// Kali Linux — list `cdimage.kali.org/current/` and find the
/// highest `kali-linux-X.Y-installer-amd64.iso` filename. The
/// `current` symlink redirects to the latest cycle; the version
/// embedded in the filename is what we need to surface.
pub fn kali_installer() -> Result<ResolvedUrls, ResolverError> {
    const BASE: &str = "https://cdimage.kali.org/current/";
    let html = http_get(BASE)?;
    resolve_kali_with_html(&html, BASE)
}

fn resolve_kali_with_html(html: &str, base: &str) -> Result<ResolvedUrls, ResolverError> {
    // Pattern: `kali-linux-2026.1-installer-amd64.iso`. The version
    // segment is Y.M (e.g. 2026.1).
    let mut versions: Vec<String> =
        simple_regex_match_all(html, "kali-linux-", "-installer-amd64.iso");
    versions.sort();
    versions.dedup();
    let latest = versions.last().ok_or(ResolverError::NoMatch {
        url: base.to_string(),
    })?;
    let filename = format!("kali-linux-{latest}-installer-amd64.iso");
    Ok(ResolvedUrls {
        iso_url: format!("{base}{filename}"),
        // Kali ships SHA256SUMS + SHA256SUMS.gpg in the same dir.
        sha256_url: format!("{base}SHA256SUMS"),
        sig_url: format!("{base}SHA256SUMS.gpg"),
    })
}

/// Linux Mint Cinnamon — list `/linuxmint/stable/` for the highest
/// `22.X/` subdirectory (point releases accumulate as new dirs in
/// the major-version family), then resolve the cinnamon ISO inside.
///
/// Major-version 22 is pinned: when 23 ships, this resolver needs a
/// new major or a major-agnostic variant (tracked under #646).
pub fn linuxmint_22_cinnamon() -> Result<ResolvedUrls, ResolverError> {
    const BASE: &str = "https://mirrors.edge.kernel.org/linuxmint/stable/";
    let html = http_get(BASE)?;
    resolve_linuxmint_22_with_html(&html, BASE, "cinnamon")
}

fn resolve_linuxmint_22_with_html(
    parent_html: &str,
    parent: &str,
    flavor: &str,
) -> Result<ResolvedUrls, ResolverError> {
    // Find highest 22.X/ subdir. "22/" alone is also a valid major
    // (the initial release before any point release shipped).
    // Lexical sort would put "22.10" < "22.9", so compare numeric
    // tuples (major, minor) instead.
    let candidates = parse_versioned_subdirs(parent_html, "22");
    let pick = candidates
        .into_iter()
        .max_by_key(|v| {
            // Parse "22.3" → (22, 3); bare "22" → (22, 0).
            let mut parts = v.splitn(2, '.');
            let major = parts
                .next()
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(0);
            let minor = parts
                .next()
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(0);
            (major, minor)
        })
        .ok_or(ResolverError::NoMatch {
            url: parent.to_string(),
        })?;
    let dir = format!("{parent}{pick}/");
    // The file inside is named after the same version (e.g.
    // linuxmint-22.3-cinnamon-64bit.iso). Don't fetch the subdir's
    // listing — we know the filename pattern.
    let filename = format!("linuxmint-{pick}-{flavor}-64bit.iso");
    Ok(ResolvedUrls {
        iso_url: format!("{dir}{filename}"),
        sha256_url: format!("{dir}sha256sum.txt"),
        sig_url: format!("{dir}sha256sum.txt.gpg"),
    })
}

/// Pull `<href="<major>(\.X)?/"` entries from a directory listing
/// and return the version strings ("22", "22.1", "22.2", "22.3"...).
/// Used by Linux Mint where point-release dirs accumulate alongside
/// the bare-major dir.
fn parse_versioned_subdirs(html: &str, major: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(start) = html[pos..].find("href=\"") {
        let after = pos + start + 6;
        let Some(end_off) = html[after..].find('"') else {
            break;
        };
        let candidate = &html[after..after + end_off];
        if let Some(stripped) = candidate.strip_suffix('/')
            && (stripped == major
                || (stripped.starts_with(major)
                    && stripped[major.len()..].starts_with('.')
                    && stripped[major.len() + 1..]
                        .chars()
                        .all(|c| c.is_ascii_digit() || c == '.')))
        {
            out.push(stripped.to_string());
        }
        pos = after + end_off + 1;
    }
    out
}

/// Pull "X.Y.Z" version segments from `<a href="debian-X.Y.Z-..."`
/// patterns. Conservative — operates on raw HTML without a real
/// parser — but the directory listings we care about are simple
/// auto-generated index pages where this works.
///
/// Multiple prefixes can appear in the haystack with different
/// suffixes (Ubuntu's listing has both `-live-server-amd64.iso` and
/// `-desktop-amd64.iso` entries). When the version validity check
/// fails — meaning the suffix we found wasn't the suffix for this
/// particular prefix occurrence — we advance only past the current
/// prefix so the next iteration sees subsequent prefix occurrences.
/// Without this, looking for `-desktop-amd64.iso` would skip past
/// the first prefix's `-live-server-amd64.iso` suffix and miss all
/// the desktop entries.
///
/// Returns the inner segments (without the prefix/suffix).
fn simple_regex_match_all(haystack: &str, prefix: &str, suffix: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(start) = haystack[pos..].find(prefix) {
        let after_prefix = pos + start + prefix.len();
        let Some(end_off) = haystack[after_prefix..].find(suffix) else {
            break;
        };
        let inner = &haystack[after_prefix..after_prefix + end_off];
        // Validate it looks like a version: digits + dots only.
        if !inner.is_empty() && inner.chars().all(|c| c.is_ascii_digit() || c == '.') {
            out.push(inner.to_string());
            pos = after_prefix + end_off + suffix.len();
        } else {
            // Suffix matched but inner isn't a clean version — the
            // suffix we found belongs to a later prefix occurrence,
            // not this one. Advance past the current prefix only so
            // the next iteration can check subsequent prefixes.
            pos = after_prefix;
        }
    }
    out
}

/// Pull the highest integer-named subdirectory from an HTML
/// directory listing. Reserved for resolvers whose layout uses
/// integer build numbers (e.g. Pop!_OS, once their server returns
/// HTML listings instead of HTTP 403).
#[allow(dead_code)] // wired in when the next resolver lands (#646)
fn parse_highest_numeric_subdir(html: &str) -> Option<u32> {
    // Find all `href="N/"` references where N is an integer.
    let mut max_n: Option<u32> = None;
    let mut pos = 0;
    while let Some(start) = html[pos..].find("href=\"") {
        let after = pos + start + "href=\"".len();
        if let Some(end_off) = html[after..].find('"') {
            let candidate = &html[after..after + end_off];
            if let Some(stripped) = candidate.strip_suffix('/')
                && let Ok(n) = stripped.parse::<u32>()
            {
                max_n = Some(max_n.map_or(n, |m| m.max(n)));
            }
            pos = after + end_off + 1;
        } else {
            break;
        }
    }
    max_n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debian_resolver_picks_highest_version_from_listing() {
        // Realistic snippet of cdimage.debian.org's directory index.
        let html = r#"<html><body>
            <a href="debian-13.3.0-amd64-netinst.iso">debian-13.3.0-amd64-netinst.iso</a>
            <a href="debian-13.4.0-amd64-netinst.iso">debian-13.4.0-amd64-netinst.iso</a>
            <a href="debian-13.4.0-amd64-netinst.iso.torrent">…</a>
        </body></html>"#;
        let r = resolve_debian_netinst_from_html(html, "https://example.test/dir/")
            .unwrap_or_else(|e| panic!("resolve: {e}"));
        assert_eq!(
            r.iso_url,
            "https://example.test/dir/debian-13.4.0-amd64-netinst.iso"
        );
        assert_eq!(r.sha256_url, "https://example.test/dir/SHA512SUMS");
        assert_eq!(r.sig_url, "https://example.test/dir/SHA512SUMS.sign");
    }

    #[test]
    fn debian_resolver_errors_on_empty_listing() {
        let html = "<html><body>nothing here</body></html>";
        let err = resolve_debian_netinst_from_html(html, "https://example.test/")
            .err()
            .unwrap_or_else(|| panic!("should fail"));
        assert!(matches!(err, ResolverError::NoMatch { .. }));
    }

    #[test]
    fn popos_subdir_parser_picks_highest_build() {
        let html = r#"<html><body>
            <a href="7/">7/</a>
            <a href="8/">8/</a>
            <a href="9/">9/</a>
            <a href="../">…</a>
        </body></html>"#;
        assert_eq!(parse_highest_numeric_subdir(html), Some(9));
    }

    #[test]
    fn popos_subdir_parser_returns_none_when_no_build_dirs() {
        let html = r#"<html><body><a href="../">…</a></body></html>"#;
        assert_eq!(parse_highest_numeric_subdir(html), None);
    }

    #[test]
    fn simple_regex_extracts_version_segments() {
        let s = "<a>debian-12.0.0-foo</a> <a>debian-12.5.0-foo</a> <a>debian-bad-foo</a>";
        let v = simple_regex_match_all(s, "debian-", "-foo");
        assert_eq!(v, vec!["12.0.0", "12.5.0"]);
    }

    #[test]
    fn ubuntu_resolver_picks_highest_point_release() {
        // Realistic snippet of releases.ubuntu.com/24.04/ where
        // operators see all point releases accumulating in one dir.
        let html = r#"<html><body>
            <a href="ubuntu-24.04.2-live-server-amd64.iso">…</a>
            <a href="ubuntu-24.04.3-live-server-amd64.iso">…</a>
            <a href="ubuntu-24.04.4-live-server-amd64.iso">…</a>
            <a href="ubuntu-24.04.4-desktop-amd64.iso">…</a>
        </body></html>"#;
        let r = resolve_ubuntu_24_04_with_html(html, "https://example.test/", "live-server")
            .unwrap_or_else(|e| panic!("resolve: {e}"));
        assert_eq!(
            r.iso_url,
            "https://example.test/ubuntu-24.04.4-live-server-amd64.iso"
        );

        // Desktop variant filters separately.
        let r2 = resolve_ubuntu_24_04_with_html(html, "https://example.test/", "desktop")
            .unwrap_or_else(|e| panic!("resolve: {e}"));
        assert_eq!(
            r2.iso_url,
            "https://example.test/ubuntu-24.04.4-desktop-amd64.iso"
        );
    }

    #[test]
    fn linuxmint_resolver_picks_highest_point_release_within_major() {
        // Realistic snippet of mirrors.edge.kernel.org/linuxmint/stable/.
        let html = r#"<html><body>
            <a href="../">…</a>
            <a href="20.3/">20.3/</a>
            <a href="21/">21/</a>
            <a href="22/">22/</a>
            <a href="22.1/">22.1/</a>
            <a href="22.2/">22.2/</a>
            <a href="22.3/">22.3/</a>
        </body></html>"#;
        let r = resolve_linuxmint_22_with_html(html, "https://example.test/", "cinnamon")
            .unwrap_or_else(|e| panic!("resolve: {e}"));
        assert_eq!(
            r.iso_url,
            "https://example.test/22.3/linuxmint-22.3-cinnamon-64bit.iso"
        );
        assert_eq!(r.sha256_url, "https://example.test/22.3/sha256sum.txt");
    }

    #[test]
    fn linuxmint_version_compare_is_numeric_not_lexical() {
        // Future-proofing: when Mint hits 22.10, lexical sort would
        // pick "22.9" as highest. Numeric tuple compare picks 22.10.
        let html = r#"<html><body>
            <a href="22.9/">22.9/</a>
            <a href="22.10/">22.10/</a>
        </body></html>"#;
        let r = resolve_linuxmint_22_with_html(html, "https://example.test/", "cinnamon")
            .unwrap_or_else(|e| panic!("resolve: {e}"));
        assert!(
            r.iso_url.contains("22.10"),
            "expected 22.10 to win over 22.9: {}",
            r.iso_url
        );
    }

    #[test]
    fn linuxmint_resolver_errors_when_no_matching_major() {
        let html = r#"<html><body>
            <a href="20.3/">20.3/</a>
            <a href="21/">21/</a>
        </body></html>"#;
        let err = resolve_linuxmint_22_with_html(html, "https://example.test/", "cinnamon")
            .err()
            .unwrap_or_else(|| panic!("should fail"));
        assert!(matches!(err, ResolverError::NoMatch { .. }));
    }

    #[test]
    fn kali_resolver_picks_highest_release() {
        // Realistic snippet of cdimage.kali.org/current/.
        let html = r#"<html><body>
            <a href="kali-linux-2025.4-installer-amd64.iso">…</a>
            <a href="kali-linux-2026.1-installer-amd64.iso">…</a>
            <a href="kali-linux-2026.1-hyperv-amd64.7z">…</a>
        </body></html>"#;
        let r = resolve_kali_with_html(html, "https://example.test/")
            .unwrap_or_else(|e| panic!("resolve: {e}"));
        assert_eq!(
            r.iso_url,
            "https://example.test/kali-linux-2026.1-installer-amd64.iso"
        );
        assert_eq!(r.sha256_url, "https://example.test/SHA256SUMS");
    }
}
