// SPDX-License-Identifier: MIT OR Apache-2.0

//! `aegis-boot fetch-trust-chain` — ADR 0002 §3.6 + [#417] Phase 3b/3c.
//!
//! Composes the [`crate::bundle_cache`] pipeline with a real-world
//! [`Downloader`] impl (curl subprocess, matching the existing
//! `fetch-image` convention) and a CLI surface.
//!
//! ## Why curl
//!
//! Same rationale as `fetch_image.rs`: `curl` is already a host dep
//! we check for in `aegis-boot doctor`, so leaning on it costs zero
//! new crate deps and zero new binary bytes. Rust HTTP clients
//! (`ureq`, `reqwest`) would add ~1 MiB to the release binary plus a
//! supply-chain surface we don't need for the ~5 files this tool
//! downloads per run.
//!
//! ## Trust chain
//!
//! 1. TLS — curl is pinned to `--proto =https --tlsv1.2+`. A MITM
//!    on the origin URL would still have to produce a valid minisig,
//!    so TLS is redundant with the signature check but defense-in-
//!    depth against a cert store compromise.
//! 2. Minisig — `aegis_trust::TrustAnchor::verify_with_epoch`. The
//!    epoch must clear both `MIN_REQUIRED_EPOCH` (binary-embedded
//!    floor) AND the local `seen_epoch` (rollback defense).
//! 3. Per-file sha256 — `bundle_cache::fetch_bundle`'s contract.
//!
//! [#417]: https://github.com/aegis-boot/aegis-boot/issues/417

use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

use aegis_trust::{TrustAnchor, load_seen_epoch, store_seen_epoch};

use crate::bundle_cache::{BundleCacheError, BundlePaths, DownloadError, Downloader, fetch_bundle};

/// Curl-subprocess [`Downloader`]. Enforces HTTPS + sane timeouts;
/// rejects any URL that isn't `https://`. Every construction is
/// zero-cost — the struct carries no state so callers can share it
/// across threads.
pub struct CurlDownloader;

impl CurlDownloader {
    /// Build a new downloader. Separate from a plain struct literal
    /// so future Phase 4 additions (e.g. a `with_proxy()` escape
    /// hatch) don't break existing callers.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Default for CurlDownloader {
    fn default() -> Self {
        Self::new()
    }
}

impl Downloader for CurlDownloader {
    fn get(&self, url: &str) -> Result<Vec<u8>, DownloadError> {
        if !validate_https_url(url) {
            return Err(DownloadError::NotHttps {
                url: url.to_string(),
            });
        }

        let output = Command::new("curl")
            .args([
                "--fail",       // exit non-zero on HTTP 4xx/5xx
                "--silent",     // no progress bar
                "--show-error", // but do print errors to stderr
                "--location",   // follow redirects (release assets)
                "--proto",
                "=https",
                "--tlsv1.2",
                "--max-time",
                "600",
                "--connect-timeout",
                "30",
                "--output",
                "-",
                url,
            ])
            .output()
            .map_err(|e| DownloadError::Transport {
                url: url.to_string(),
                detail: format!("invoke curl: {e}"),
            })?;

        if output.status.success() {
            return Ok(output.stdout);
        }

        // Map curl's exit code + stderr into a DownloadError variant.
        let stderr = String::from_utf8_lossy(&output.stderr);
        let exit_code = output.status.code().unwrap_or(-1);
        classify_curl_failure(url, exit_code, &stderr)
    }
}

/// Whether `url` is acceptable for a signed-chain download. Stricter
/// than RFC 3986: must start with `https://` and must not contain
/// control chars (defensive against injection via an environment-
/// sourced origin URL).
#[must_use]
pub fn validate_https_url(url: &str) -> bool {
    if !url.starts_with("https://") {
        return false;
    }
    !url.chars().any(char::is_control)
}

/// Classify a curl failure. Exit 22 is `--fail`'s HTTP-error
/// surface; everything else is a transport issue. Parses the HTTP
/// status from curl's English stderr message when it's present,
/// else leaves it at `0`.
fn classify_curl_failure(
    url: &str,
    exit_code: i32,
    stderr: &str,
) -> Result<Vec<u8>, DownloadError> {
    if exit_code == 22 {
        return Err(DownloadError::Http {
            status: parse_http_status(stderr).unwrap_or(0),
            url: url.to_string(),
        });
    }
    Err(DownloadError::Transport {
        url: url.to_string(),
        detail: format!("curl exit {exit_code}: {}", stderr.trim()),
    })
}

/// Parse a 3-digit HTTP status from curl's `--fail` stderr message.
/// Typical format: `curl: (22) The requested URL returned error: 404`.
/// Returns `None` if no status could be found; callers fall back to `0`.
fn parse_http_status(stderr: &str) -> Option<u16> {
    const NEEDLE: &str = "returned error: ";
    let idx = stderr.find(NEEDLE)?;
    let after = &stderr[idx + NEEDLE.len()..];
    let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// Resolve the base cache directory per XDG conventions. Mirrors
/// the pattern in `aegis_trust::seen_epoch_path`, except the base
/// dir is `XDG_CACHE_HOME` rather than `XDG_STATE_HOME` — the trust
/// chain is regenerable cache data, not monotonic state.
#[must_use]
pub fn cache_base() -> PathBuf {
    cache_base_from(
        std::env::var("XDG_CACHE_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
    )
}

/// Explicit-env variant of [`cache_base`] for unit tests.
#[must_use]
pub fn cache_base_from(xdg_cache: Option<&str>, home: Option<&str>) -> PathBuf {
    let base = match (xdg_cache, home) {
        (Some(x), _) if !x.is_empty() => PathBuf::from(x),
        (_, Some(h)) if !h.is_empty() => {
            let mut p = PathBuf::from(h);
            p.push(".cache");
            p
        }
        _ => PathBuf::from(".cache"),
    };
    base.join("aegis-boot").join("signed-chain")
}

/// CLI entry point for `aegis-boot fetch-trust-chain <origin-url>`.
pub fn run(args: &[String]) -> ExitCode {
    match try_run(args) {
        Ok(paths) => {
            // Stdout: cache_dir only, so the subcommand composes
            // cleanly via `$(aegis-boot fetch-trust-chain ...)`.
            // Stderr: a short inventory of which role resolved to
            // which on-disk file, useful for operator-visible logs
            // without polluting the composable stdout value.
            eprintln!(
                "aegis-boot fetch-trust-chain: verified {} file(s) at key_epoch={}:",
                paths.files.len(),
                paths.manifest.key_epoch
            );
            let mut sorted: Vec<_> = paths.files.iter().collect();
            sorted.sort_by_key(|(role, _)| format!("{role:?}"));
            for (role, path) in sorted {
                eprintln!("  {role:?}: {}", path.display());
            }
            println!("{}", paths.cache_dir.display());
            ExitCode::SUCCESS
        }
        Err(code) => ExitCode::from(code),
    }
}

fn try_run(args: &[String]) -> Result<BundlePaths, u8> {
    let parsed = parse_args(args)?;
    if parsed.help_requested {
        print_help();
        return Err(0);
    }

    let origin = parsed.origin.ok_or_else(|| {
        eprintln!("aegis-boot fetch-trust-chain: missing <origin-url>. Try --help.");
        2u8
    })?;

    if !validate_https_url(&origin) {
        eprintln!(
            "aegis-boot fetch-trust-chain: origin {origin:?} must start with `https://` \
             (signed-chain downloads refuse plaintext transport)."
        );
        return Err(2);
    }

    let anchor = TrustAnchor::load().map_err(|e| {
        eprintln!("aegis-boot fetch-trust-chain: trust-anchor load failed: {e}");
        1u8
    })?;

    let seen = load_seen_epoch().map(|s| s.epoch).unwrap_or(0);

    let base = parsed.cache_base.unwrap_or_else(cache_base);
    fs::create_dir_all(&base).map_err(|e| {
        if e.kind() == ErrorKind::PermissionDenied {
            eprintln!(
                "aegis-boot fetch-trust-chain: cannot create {} (permission denied). \
                 Try setting $XDG_CACHE_HOME to a writable path.",
                base.display()
            );
            1u8
        } else {
            eprintln!(
                "aegis-boot fetch-trust-chain: cannot create {}: {e}",
                base.display()
            );
            1u8
        }
    })?;

    let downloader = CurlDownloader::new();
    let paths = fetch_bundle(&origin, &base, &anchor, seen, &downloader).map_err(|e| {
        render_fetch_error(&e);
        1u8
    })?;

    // Advance seen-epoch to the observed key_epoch. store_seen_epoch
    // refuses regression, so this is a no-op when we've already seen
    // the same-or-newer epoch.
    if let Err(e) = store_seen_epoch(paths.manifest.key_epoch) {
        eprintln!(
            "aegis-boot fetch-trust-chain: WARNING — cache verified OK but seen-epoch \
             advance failed: {e}. Next run will re-verify at the current floor."
        );
    }

    Ok(paths)
}

fn render_fetch_error(e: &BundleCacheError) {
    eprintln!("aegis-boot fetch-trust-chain: {e}");
    match e {
        BundleCacheError::Download(DownloadError::Http { status, url }) => {
            eprintln!("  URL: {url}");
            if *status == 404 {
                eprintln!(
                    "  NEXT ACTION: check the origin URL + version tag. \
                     The bundle may not be published for this release yet."
                );
            } else if *status >= 500 {
                eprintln!("  NEXT ACTION: retry later; origin is returning {status}.");
            }
        }
        BundleCacheError::Download(DownloadError::Transport { url, .. }) => {
            eprintln!("  URL: {url}");
            eprintln!(
                "  NEXT ACTION: check DNS / TLS / firewall. Run `curl -v {url}` to \
                 see the full handshake."
            );
        }
        BundleCacheError::Verify(inner) => {
            eprintln!("  verify: {inner}");
        }
        _ => {}
    }
}

/// Parsed CLI args for `fetch-trust-chain`.
#[derive(Debug, Default)]
struct Args {
    origin: Option<String>,
    cache_base: Option<PathBuf>,
    help_requested: bool,
}

fn parse_args(args: &[String]) -> Result<Args, u8> {
    let mut out = Args::default();
    let mut iter = args.iter();
    while let Some(a) = iter.next() {
        match a.as_str() {
            "-h" | "--help" => out.help_requested = true,
            // #541: --out / --out-dir / --cache-base are interchangeable
            // across the four output-writing subcommands.
            "--cache-base" | "--out" | "--out-dir" => {
                let v = iter.next().ok_or_else(|| {
                    eprintln!("aegis-boot fetch-trust-chain: {a} requires an argument");
                    2u8
                })?;
                out.cache_base = Some(PathBuf::from(v));
            }
            s if s.starts_with("--cache-base=") => {
                out.cache_base = Some(PathBuf::from(&s["--cache-base=".len()..]));
            }
            s if s.starts_with("--out=") => {
                out.cache_base = Some(PathBuf::from(&s["--out=".len()..]));
            }
            s if s.starts_with("--out-dir=") => {
                out.cache_base = Some(PathBuf::from(&s["--out-dir=".len()..]));
            }
            s if s.starts_with('-') => {
                eprintln!("aegis-boot fetch-trust-chain: unknown flag {s:?}");
                return Err(2);
            }
            other => {
                if out.origin.is_some() {
                    eprintln!(
                        "aegis-boot fetch-trust-chain: extra positional arg {other:?} \
                         (only one <origin-url> accepted)"
                    );
                    return Err(2);
                }
                out.origin = Some(other.to_string());
            }
        }
    }
    Ok(out)
}

fn print_help() {
    println!("aegis-boot fetch-trust-chain — download + verify a signed-chain bundle");
    println!();
    println!("USAGE:");
    println!("  aegis-boot fetch-trust-chain <origin-url>");
    println!("  aegis-boot fetch-trust-chain --cache-base <dir> <origin-url>");
    println!();
    println!("<origin-url> must be an https:// URL ending with /. The tool appends");
    println!("`bundle-manifest.json` + `bundle-manifest.json.minisig` (and each");
    println!("per-file path from the manifest) to form download URLs.");
    println!();
    println!("On success the verified cache directory is printed to stdout; on any");
    println!("verification failure the stick is not flashed and exit=1.");
    println!();
    println!("OPTIONS:");
    println!(
        "  --cache-base <dir>   Override the default $XDG_CACHE_HOME/aegis-boot/signed-chain/"
    );
    println!("  -h, --help           Show this help");
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn validate_https_url_accepts_well_formed() {
        assert!(validate_https_url(
            "https://github.com/aegis-boot/aegis-boot/releases/download/v0.17.0/bundle/"
        ));
        assert!(validate_https_url("https://example.invalid/"));
    }

    #[test]
    fn validate_https_url_rejects_plaintext_and_weird() {
        assert!(!validate_https_url("http://example.invalid/"));
        assert!(!validate_https_url("ftp://example.invalid/"));
        assert!(!validate_https_url("https://example.invalid/\r\n"));
        assert!(!validate_https_url("https://example.invalid/\0"));
        assert!(!validate_https_url(""));
    }

    #[test]
    fn parse_http_status_extracts_404() {
        let stderr = "curl: (22) The requested URL returned error: 404 Not Found\n";
        assert_eq!(parse_http_status(stderr), Some(404));
    }

    #[test]
    fn parse_http_status_extracts_500() {
        let stderr = "curl: (22) The requested URL returned error: 500\n";
        assert_eq!(parse_http_status(stderr), Some(500));
    }

    #[test]
    fn parse_http_status_returns_none_for_unrelated_stderr() {
        assert_eq!(parse_http_status("curl: (6) Couldn't resolve host"), None);
        assert_eq!(parse_http_status(""), None);
    }

    #[test]
    fn classify_curl_failure_exit_22_becomes_http_error() {
        let err = classify_curl_failure(
            "https://example.invalid/x",
            22,
            "curl: (22) The requested URL returned error: 404 Not Found",
        )
        .unwrap_err();
        match err {
            DownloadError::Http { status: 404, .. } => {}
            other => panic!("wrong error: {other:?}"),
        }
    }

    #[test]
    fn classify_curl_failure_exit_6_becomes_transport() {
        let err = classify_curl_failure(
            "https://nonexistent.invalid/x",
            6,
            "curl: (6) Couldn't resolve host 'nonexistent.invalid'",
        )
        .unwrap_err();
        assert!(matches!(err, DownloadError::Transport { .. }));
    }

    #[test]
    fn classify_curl_failure_exit_28_becomes_transport() {
        // Exit 28 = operation timeout.
        let err = classify_curl_failure(
            "https://slow.invalid/x",
            28,
            "curl: (28) Operation timed out after 600000 milliseconds",
        )
        .unwrap_err();
        assert!(matches!(err, DownloadError::Transport { .. }));
    }

    #[test]
    fn cache_base_prefers_xdg_cache_home() {
        let p = cache_base_from(Some("/tmp/xdg-cache"), Some("/home/op"));
        assert_eq!(p, PathBuf::from("/tmp/xdg-cache/aegis-boot/signed-chain"));
    }

    #[test]
    fn cache_base_falls_back_to_home_dot_cache() {
        let p = cache_base_from(None, Some("/home/op"));
        assert_eq!(p, PathBuf::from("/home/op/.cache/aegis-boot/signed-chain"));
    }

    #[test]
    fn cache_base_empty_xdg_treated_as_unset() {
        let p = cache_base_from(Some(""), Some("/home/op"));
        assert_eq!(p, PathBuf::from("/home/op/.cache/aegis-boot/signed-chain"));
    }

    #[test]
    fn parse_args_accepts_positional_origin() {
        let args = vec!["https://example.invalid/".to_string()];
        let out = parse_args(&args).unwrap();
        assert_eq!(out.origin.as_deref(), Some("https://example.invalid/"));
        assert!(!out.help_requested);
    }

    #[test]
    fn parse_args_accepts_cache_base_split_form() {
        let args = vec![
            "--cache-base".to_string(),
            "/tmp/cache".to_string(),
            "https://example.invalid/".to_string(),
        ];
        let out = parse_args(&args).unwrap();
        assert_eq!(out.cache_base.as_deref(), Some(Path::new("/tmp/cache")));
        assert_eq!(out.origin.as_deref(), Some("https://example.invalid/"));
    }

    #[test]
    fn parse_args_accepts_cache_base_eq_form() {
        let args = vec![
            "--cache-base=/tmp/cache".to_string(),
            "https://example.invalid/".to_string(),
        ];
        let out = parse_args(&args).unwrap();
        assert_eq!(out.cache_base.as_deref(), Some(Path::new("/tmp/cache")));
    }

    // ---- #541: --out / --out-dir aliases for fetch-trust-chain --cache-base

    #[test]
    fn parse_args_accepts_out_alias_split_form() {
        let args = vec![
            "--out".to_string(),
            "/tmp/aegis-bf".to_string(),
            "https://example.invalid/".to_string(),
        ];
        let out = parse_args(&args).unwrap();
        assert_eq!(out.cache_base.as_deref(), Some(Path::new("/tmp/aegis-bf")));
    }

    #[test]
    fn parse_args_accepts_out_dir_alias_split_form() {
        let args = vec![
            "--out-dir".to_string(),
            "/tmp/aegis-bf-od".to_string(),
            "https://example.invalid/".to_string(),
        ];
        let out = parse_args(&args).unwrap();
        assert_eq!(
            out.cache_base.as_deref(),
            Some(Path::new("/tmp/aegis-bf-od"))
        );
    }

    #[test]
    fn parse_args_accepts_out_alias_eq_form() {
        let args = vec![
            "--out=/tmp/aegis-bf-eq".to_string(),
            "https://example.invalid/".to_string(),
        ];
        let out = parse_args(&args).unwrap();
        assert_eq!(
            out.cache_base.as_deref(),
            Some(Path::new("/tmp/aegis-bf-eq"))
        );
    }

    #[test]
    fn parse_args_accepts_out_dir_alias_eq_form() {
        let args = vec![
            "--out-dir=/tmp/aegis-bf-od-eq".to_string(),
            "https://example.invalid/".to_string(),
        ];
        let out = parse_args(&args).unwrap();
        assert_eq!(
            out.cache_base.as_deref(),
            Some(Path::new("/tmp/aegis-bf-od-eq"))
        );
    }

    #[test]
    fn parse_args_rejects_second_positional() {
        let args = vec![
            "https://a.invalid/".to_string(),
            "https://b.invalid/".to_string(),
        ];
        assert_eq!(parse_args(&args).unwrap_err(), 2);
    }

    #[test]
    fn parse_args_rejects_unknown_flag() {
        let args = vec!["--wat".to_string()];
        assert_eq!(parse_args(&args).unwrap_err(), 2);
    }

    #[test]
    fn parse_args_honors_help_flags() {
        for flag in ["-h", "--help"] {
            let args = vec![flag.to_string()];
            let out = parse_args(&args).unwrap();
            assert!(out.help_requested);
        }
    }

    #[test]
    fn curl_downloader_rejects_http_url_without_invoking_curl() {
        let d = CurlDownloader::new();
        let err = d.get("http://example.invalid/").unwrap_err();
        assert!(matches!(err, DownloadError::NotHttps { .. }));
    }
}
