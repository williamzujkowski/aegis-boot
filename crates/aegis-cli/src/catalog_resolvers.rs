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

/// Pull "X.Y.Z" version segments from `<a href="debian-X.Y.Z-..."`
/// patterns. Conservative — operates on raw HTML without a real
/// parser — but the directory listings we care about are simple
/// auto-generated index pages where this works.
///
/// Returns the inner segments (without the prefix/suffix).
fn simple_regex_match_all(haystack: &str, prefix: &str, suffix: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(start) = haystack[pos..].find(prefix) {
        let after_prefix = pos + start + prefix.len();
        if let Some(end_off) = haystack[after_prefix..].find(suffix) {
            let inner = &haystack[after_prefix..after_prefix + end_off];
            // Validate it looks like a version: digits + dots only.
            if !inner.is_empty() && inner.chars().all(|c| c.is_ascii_digit() || c == '.') {
                out.push(inner.to_string());
            }
            pos = after_prefix + end_off + suffix.len();
        } else {
            break;
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
}
