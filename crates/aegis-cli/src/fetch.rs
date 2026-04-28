// SPDX-License-Identifier: MIT OR Apache-2.0

//! `aegis-boot fetch <slug>` — download + verify a catalog ISO.
//!
//! Resolves a slug from the catalog and delegates the download +
//! signed-chain verification to [`aegis_fetch::fetch_catalog_entry`]
//! (#655 PR-C). The verification dispatches on the entry's
//! [`aegis_catalog::SigPattern`]:
//!
//! - `ClearsignedSums` (`AlmaLinux`, Fedora, Rocky)
//! - `DetachedSigOnSums` (Debian, Ubuntu, Kali, Linux Mint, `GParted`,
//!   openSUSE, Pop!\_OS)
//! - `DetachedSigOnIso` (Alpine, Manjaro, MX Linux, `SystemRescue`)
//!
//! On success, prints the absolute path to the downloaded ISO + a
//! single-line `aegis-boot add` command. We do NOT auto-add because
//! the operator may want to choose which stick gets the ISO; this
//! tool's job is to deliver a verified ISO to disk, not to make the
//! stick-write decision.
//!
//! Storage: defaults to `$XDG_CACHE_HOME/aegis-boot/<slug>/` (or
//! `$HOME/.cache/aegis-boot/<slug>/`). Override with `--out DIR`.
//!
//! ## Trust boundary
//!
//! Network: `ureq` + rustls + ring + Mozilla CA bundle (statically
//! linked into the binary; no system curl). Verification: rpgp
//! pure-Rust `OpenPGP` against the embedded vendor keyring shipped
//! in `crates/aegis-catalog/keyring/<vendor>.asc` with primary
//! fingerprints pinned in `EMBEDDED_FINGERPRINTS`. No system gpg
//! involved — `aegis-boot doctor`'s prerequisite check no longer
//! lists curl / sha256sum / gpg for this codepath.
//!
//! `--no-gpg` is now a deprecation no-op: verification always
//! runs. Operators with vendors not yet in the embedded keyring
//! (the partial-coverage set of `LinuxMint` / MX / `SystemRescue` /
//! `GParted` / System76 / openSUSE — see #655 PR-B follow-up)
//! get `FetchError::UnknownVendor` and should use
//! `aegis-boot add <iso-path>` with a manually-downloaded ISO
//! until the vendor key lands.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use aegis_catalog::{Entry, SbStatus, find_entry};
use aegis_fetch::{FetchError, FetchEvent, FetchProgress, VendorKeyring};

/// Entry point for `aegis-boot fetch [--out DIR] [--no-gpg] <slug>`.
pub fn run(args: &[String]) -> ExitCode {
    match try_run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => ExitCode::from(code),
    }
}

/// Inner runner returning a typed result so `aegis-boot init` can branch
/// on success/failure. Same semantics as `run`.
pub(crate) fn try_run(args: &[String]) -> Result<(), u8> {
    let parsed = match parse_flags(args) {
        Ok(Some(p)) => p,
        Ok(None) => return Ok(()), // --help printed, clean exit
        Err(code) => return Err(code),
    };
    let FetchFlags {
        out_dir,
        skip_gpg,
        dry_run,
        no_progress,
        slug,
    } = parsed;

    // Progress-bar policy: honor explicit --progress / --no-progress
    // flags, else auto-detect based on whether stdout is a terminal.
    // Non-TTY stdout (CI logs, pipes, redirects) gets no progress
    // bar since the carriage-return re-renders would trash the log.
    let show_progress = match no_progress {
        Some(true) => false,
        Some(false) => true,
        None => std::io::IsTerminal::is_terminal(&std::io::stdout()),
    };

    let Some(entry) = find_entry(&slug) else {
        eprintln!("aegis-boot fetch: no catalog entry matching '{slug}'");
        eprintln!("run 'aegis-boot recommend' to see available slugs");
        return Err(1);
    };

    let dest = out_dir.unwrap_or_else(|| default_cache_dir(entry.slug));

    if dry_run {
        print_dry_run(entry, &dest, skip_gpg);
        return Ok(());
    }

    if skip_gpg {
        // Deprecation: --no-gpg is now a no-op. The embedded
        // vendor keyring + rpgp verifier means we always have
        // the right key available, so opting out is no longer
        // useful. Print a one-line notice and proceed with the
        // full verification.
        eprintln!(
            "aegis-boot fetch: --no-gpg is deprecated and now a no-op (#655 PR-C). \
             Verification always runs against the embedded vendor keyring."
        );
    }

    let keyring = match VendorKeyring::embedded() {
        Ok(k) => k,
        Err(e) => {
            eprintln!("aegis-boot fetch: vendor keyring failed to load: {e}");
            return Err(1);
        }
    };

    println!("Fetching {} into {}", entry.name, dest.display());
    println!();

    let mut renderer = ProgressRenderer::new(show_progress);
    let outcome = match aegis_fetch::fetch_catalog_entry(entry, &dest, &keyring, &mut |event| {
        renderer.handle(&event);
    }) {
        Ok(o) => {
            renderer.finish_done();
            o
        }
        Err(e) => {
            renderer.finish_failed();
            return Err(report_fetch_error(&e, entry));
        }
    };

    let iso_filename = outcome
        .iso_path
        .file_name()
        .and_then(|s| s.to_str())
        .map_or_else(|| filename_from_url(entry.iso_url), str::to_string);
    print_success(entry, &dest, &iso_filename);
    println!();
    println!(
        "Authenticated by vendor key fingerprint {}",
        outcome.key_fingerprint
    );
    Ok(())
}

/// Map a [`FetchError`] from `aegis-fetch` into operator-readable
/// stderr output + the appropriate process exit code. Distinguishes
/// the rich error variants so the caller gets a remediation hint
/// rather than a raw `Display` string.
fn report_fetch_error(err: &FetchError, entry: &Entry) -> u8 {
    eprintln!();
    match err {
        FetchError::Network { url, detail } => {
            eprintln!("aegis-boot fetch: network error fetching {url}");
            eprintln!("  {detail}");
            eprintln!("re-try on a different network or wait — vendor mirrors flap occasionally");
            1
        }
        FetchError::Filesystem { detail } => {
            eprintln!("aegis-boot fetch: filesystem error");
            eprintln!("  {detail}");
            1
        }
        FetchError::Sha256Mismatch {
            expected,
            actual,
            iso,
        } => {
            eprintln!("aegis-boot fetch: SHA-256 verification FAILED for {iso}");
            eprintln!("  expected {expected}");
            eprintln!("  actual   {actual}");
            eprintln!("the ISO does not match the (cryptographically authenticated) sums file.");
            eprintln!("re-fetch on a different network if you suspect MITM.");
            1
        }
        FetchError::SignatureVerifyFailed {
            entry: slug,
            detail,
        } => {
            eprintln!("aegis-boot fetch: signature verification FAILED for {slug}");
            eprintln!("  {detail}");
            eprintln!("either the artifact was tampered with in transit, or the vendor rotated");
            eprintln!("their signing key and the embedded keyring is stale. update aegis-boot to");
            eprintln!("a newer release and re-try.");
            1
        }
        FetchError::UnknownVendor { vendor } => {
            eprintln!("aegis-boot fetch: vendor {vendor:?} has no key in the embedded keyring");
            eprintln!("this catalog entry is partial-coverage in #655 Phase 2B PR-B; the vendor");
            eprintln!("key is awaiting follow-up sourcing. workarounds:");
            eprintln!(
                "  - download the ISO + sums + sig manually from {} and",
                entry.iso_url
            );
            eprintln!("    `aegis-boot add <iso-path>` to stage on a stick");
            eprintln!("  - track #655 for the keyring-completion follow-up PR");
            1
        }
        FetchError::IsoNotInSums { iso } => {
            eprintln!("aegis-boot fetch: vendor sums file does not list {iso}");
            eprintln!("this means the catalog's iso_url is out of date — vendor has rotated");
            eprintln!("the published filename. file an issue or wait for the next");
            eprintln!("`catalog-refresh` auto-PR.");
            1
        }
        FetchError::MalformedSums => {
            eprintln!("aegis-boot fetch: vendor sums file did not contain any sha256 lines");
            eprintln!("the vendor may have switched to a different digest algorithm; file an");
            eprintln!("issue with the vendor URL.");
            1
        }
        FetchError::NotClearsigned => {
            eprintln!("aegis-boot fetch: vendor's CHECKSUM file is not a clearsigned envelope");
            eprintln!("the catalog declared this entry as ClearsignedSums; either the vendor");
            eprintln!("switched format or the entry's verify field is wrong. file an issue.");
            1
        }
    }
}

/// Render `aegis-fetch` lifecycle events as terminal output.
///
/// Maps `FetchEvent` to `indicatif` progress bars when stdout is a
/// TTY, plain `println!` lines otherwise. The bar shows
/// bytes-per-sec + ETA during the download phase and a spinner
/// during the verify phases.
struct ProgressRenderer {
    show_progress: bool,
    download_bar: Option<indicatif::ProgressBar>,
}

impl ProgressRenderer {
    fn new(show_progress: bool) -> Self {
        Self {
            show_progress,
            download_bar: None,
        }
    }

    fn handle(&mut self, event: &FetchEvent) {
        match event {
            FetchEvent::Connecting => {
                if !self.show_progress {
                    println!("  Connecting...");
                }
            }
            FetchEvent::Downloading(progress) => self.update_download(progress),
            FetchEvent::VerifyingHash => {
                self.finish_download();
                println!("  Verifying SHA-256...");
            }
            FetchEvent::VerifyingSig => {
                self.finish_download();
                println!("  Verifying PGP signature...");
            }
            FetchEvent::Done(_) => {}
        }
    }

    fn update_download(&mut self, progress: &FetchProgress) {
        if !self.show_progress {
            return;
        }
        let bar = self.download_bar.get_or_insert_with(|| {
            let total = progress.total.unwrap_or(0);
            let pb = if total > 0 {
                indicatif::ProgressBar::new(total)
            } else {
                indicatif::ProgressBar::new_spinner()
            };
            let template = if total > 0 {
                "  [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta}, {bytes_per_sec})"
            } else {
                "  {spinner} {bytes} ({bytes_per_sec})"
            };
            #[allow(clippy::expect_used)] // hard-coded template string
            pb.set_style(
                indicatif::ProgressStyle::with_template(template).expect("static template parses"),
            );
            pb
        });
        bar.set_position(progress.bytes);
    }

    fn finish_download(&mut self) {
        if let Some(bar) = self.download_bar.take() {
            bar.finish_and_clear();
        }
    }

    fn finish_done(&mut self) {
        self.finish_download();
    }

    fn finish_failed(&mut self) {
        if let Some(bar) = self.download_bar.take() {
            bar.abandon();
        }
    }
}

/// Parsed flag state for `aegis-boot fetch`. Splitting this into its
/// own struct + `parse_flags` keeps `try_run` under the workspace-wide
/// 100-line limit.
struct FetchFlags {
    out_dir: Option<PathBuf>,
    skip_gpg: bool,
    dry_run: bool,
    /// When `Some(true)`, suppress the curl progress bar even on a
    /// TTY (scripted usage, CI logs). When `Some(false)`, force the
    /// progress bar even if stdout doesn't appear to be a TTY (rare,
    /// useful for tests). When `None` (default), auto-detect based
    /// on `std::io::IsTerminal`. #311.
    no_progress: Option<bool>,
    slug: String,
}

/// Parse `fetch` CLI args. Returns:
///   - `Ok(Some(FetchFlags))` on successful parse
///   - `Ok(None)` when `--help` was printed (caller should clean-exit)
///   - `Err(exit_code)` on usage error
fn parse_flags(args: &[String]) -> Result<Option<FetchFlags>, u8> {
    let mut out_dir: Option<PathBuf> = None;
    let mut skip_gpg = false;
    let mut dry_run = false;
    let mut no_progress: Option<bool> = None;
    let mut slug: Option<String> = None;
    let mut iter = args.iter();
    while let Some(a) = iter.next() {
        match a.as_str() {
            "--help" | "-h" => {
                print_help();
                return Ok(None);
            }
            // #541: --out / --out-dir / --cache-base are interchangeable
            // aliases across fetch, flash, fetch-image, and fetch-trust-chain.
            // Operator muscle memory from `curl -o`, `cp --target-directory`,
            // `tar -C` doesn't agree on a single name, so we accept all three.
            "--out" | "--out-dir" | "--cache-base" => {
                let Some(v) = iter.next() else {
                    eprintln!("aegis-boot fetch: {a} requires a directory argument");
                    return Err(2);
                };
                out_dir = Some(PathBuf::from(v));
            }
            "--no-gpg" => skip_gpg = true,
            "--dry-run" => dry_run = true,
            "--no-progress" => no_progress = Some(true),
            "--progress" => no_progress = Some(false),
            arg if arg.starts_with("--out=") => {
                out_dir = Some(PathBuf::from(arg.trim_start_matches("--out=")));
            }
            arg if arg.starts_with("--out-dir=") => {
                out_dir = Some(PathBuf::from(arg.trim_start_matches("--out-dir=")));
            }
            arg if arg.starts_with("--cache-base=") => {
                out_dir = Some(PathBuf::from(arg.trim_start_matches("--cache-base=")));
            }
            arg if arg.starts_with("--") => {
                eprintln!("aegis-boot fetch: unknown option '{arg}'");
                return Err(2);
            }
            other => {
                if slug.is_some() {
                    eprintln!(
                        "aegis-boot fetch: only one slug allowed (got '{other}' after '{}')",
                        slug.unwrap_or_else(|| "?".into())
                    );
                    return Err(2);
                }
                slug = Some(other.to_string());
            }
        }
    }
    let Some(slug) = slug else {
        eprintln!("aegis-boot fetch: missing <slug> argument");
        eprintln!("run 'aegis-boot recommend' to see available slugs");
        return Err(2);
    };
    Ok(Some(FetchFlags {
        out_dir,
        skip_gpg,
        dry_run,
        no_progress,
        slug,
    }))
}

fn print_help() {
    println!("aegis-boot fetch — download + verify a catalog ISO");
    println!();
    println!("USAGE:");
    println!("  aegis-boot fetch <slug>");
    println!("  aegis-boot fetch --out /path/to/dir <slug>");
    println!("  aegis-boot fetch --no-gpg <slug>      # SHA-256 only (NOT recommended)");
    println!("  aegis-boot fetch --dry-run <slug>     # Preview — print recipe, no downloads");
    println!("  aegis-boot fetch --help");
    println!();
    println!("OPTIONS:");
    println!(
        "  --out DIR       Destination directory (default: $XDG_CACHE_HOME/aegis-boot/<slug>)"
    );
    println!("  --no-gpg        Skip GPG signature verification on SHA256SUMS");
    println!("  --dry-run       Print what would be downloaded without doing it");
    println!("  --no-progress   Suppress curl progress bar (for scripted usage / CI logs)");
    println!("  --progress      Force progress bar even when stdout is not a TTY (rare)");
    println!("  --help          This message");
    println!();
    println!("EXAMPLES:");
    println!("  aegis-boot fetch ubuntu-24.04-live-server");
    println!("  aegis-boot fetch --dry-run alpine-3.20-standard  # see URLs + sizes first");
    println!("  aegis-boot fetch --out ~/Downloads alpine-3.20-standard");
    println!();
    println!("`aegis-boot fetch` does not write to a USB stick; it downloads + verifies");
    println!("the ISO and prints the `aegis-boot add` command to copy it onto a stick.");
}

/// Preview what `aegis-boot fetch <slug>` would do. Prints the three
/// URLs it would hit (ISO, SHA256SUMS, .sig), the destination dir,
/// and the GPG policy. For already-cached files (previous fetch that
/// didn't run `rm` on the cache dir), report the on-disk size so
/// the operator knows the next real fetch will be a no-op for that
/// file. No network, no writes. (#181-adjacent UX sharpening)
fn print_dry_run(entry: &Entry, dest: &Path, skip_gpg: bool) {
    let iso_filename = filename_from_url(entry.iso_url);
    let sha_filename = filename_from_url(entry.sha256_url);
    let sig_filename = filename_from_url(entry.sig_url);
    println!("aegis-boot fetch — dry run (no network, no writes)");
    println!();
    println!("Would fetch:  {} ({})", entry.name, entry.slug);
    println!("Destination:  {}", dest.display());
    if dest.is_dir() {
        println!("              (already exists)");
    } else {
        println!("              (would create)");
    }
    println!();
    println!("Sources:");
    report_source_url(entry.iso_url, &dest.join(&iso_filename), "ISO");
    report_source_url(entry.sha256_url, &dest.join(&sha_filename), "SHA256SUMS");
    report_source_url(entry.sig_url, &dest.join(&sig_filename), "signature");
    println!();
    println!("Verification:");
    println!("  sha256sum -c against {sha_filename}");
    if skip_gpg {
        println!("  GPG: SKIPPED (--no-gpg)");
    } else {
        println!("  gpg --verify {sig_filename} {sha_filename}  (UnknownKey is non-fatal)");
    }
    if matches!(entry.sb, SbStatus::UnsignedNeedsMok) {
        println!();
        println!(
            "Note: this ISO's kernel is unsigned. `aegis-boot fetch` will print a \
             MOK-enrollment reminder on completion; see docs/UNSIGNED_KERNEL.md."
        );
    }
    println!();
    println!(
        "Run `aegis-boot fetch {}` (without --dry-run) to proceed.",
        entry.slug
    );
}

/// One line of dry-run detail for a source URL: label, URL, and
/// "(cached, N bytes)" when the target already exists on disk.
fn report_source_url(url: &str, local_path: &Path, label: &str) {
    let cached_note = match std::fs::metadata(local_path) {
        Ok(m) if m.is_file() => format!(" (cached: {} bytes on disk)", m.len()),
        _ => String::new(),
    };
    println!("  {label:<11}  {url}{cached_note}");
}

/// Default cache directory for a catalog slug. Exposed via
/// `pub(crate)` so `aegis-boot add <slug>` (#352 UX-4) can resolve
/// the post-fetch ISO path without duplicating the XDG / HOME /
/// /tmp fallback chain.
pub(crate) fn default_cache_dir(slug: &str) -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("aegis-boot").join(slug)
}

/// Extract the trailing filename from a catalog URL. Exposed via
/// `pub(crate)` so `add <slug>` can compute the cached ISO filename
/// without another round-trip to the catalog entry (#352 UX-4).
pub(crate) fn filename_from_url(url: &str) -> String {
    url.rsplit('/').next().unwrap_or("download").to_string()
}

/// Resolve the post-fetch ISO path for a catalog slug. Returns `None`
/// if the slug is unknown to the catalog. The returned path may not
/// exist yet — callers should check and fetch if absent.
pub(crate) fn cached_iso_path(slug: &str) -> Option<PathBuf> {
    let entry = find_entry(slug)?;
    Some(default_cache_dir(entry.slug).join(filename_from_url(entry.iso_url)))
}

fn print_success(entry: &Entry, dest: &std::path::Path, iso_filename: &str) {
    let abs_iso = dest.join(iso_filename);
    println!();
    println!("Done. Verified ISO at:");
    println!("  {}", abs_iso.display());
    println!();
    println!("Add it to an aegis-boot stick:");
    println!("  aegis-boot add {}", abs_iso.display());
    println!();
    if matches!(entry.sb, SbStatus::UnsignedNeedsMok) {
        println!("This ISO's kernel is unsigned. Place the distro's kernel signing key");
        println!("public file alongside the ISO on the stick before booting:");
        println!("  cp <distro-signing-key>.pub /run/media/aegis-isos/{iso_filename}.pub");
        println!("See docs/UNSIGNED_KERNEL.md for the per-distro key rotation notes.");
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]
    // Edition-2024 made std::env::set_var / remove_var unsafe; some
    // tests mutate XDG_CACHE_HOME + HOME to exercise cache-dir
    // resolution. Serialized via ENV_MUTEX, so the "not safe across
    // threads" unsafety requirement doesn't apply. Scoped allow so
    // production-code unsafe_code = "deny" stays intact.
    #![allow(unsafe_code)]
    use super::*;

    #[test]
    fn filename_from_basic_url() {
        assert_eq!(
            filename_from_url("https://example.com/path/to/file.iso"),
            "file.iso"
        );
    }

    #[test]
    fn parse_flags_no_progress_is_unset_by_default() {
        // No explicit flag → caller derives from IsTerminal. #311.
        let parsed = parse_flags(&["ubuntu-24.04-live-server".to_string()])
            .expect("parse ok")
            .expect("not --help");
        assert_eq!(parsed.no_progress, None);
    }

    #[test]
    fn parse_flags_no_progress_flag_forces_silent() {
        let parsed = parse_flags(&[
            "--no-progress".to_string(),
            "ubuntu-24.04-live-server".to_string(),
        ])
        .expect("parse ok")
        .expect("not --help");
        assert_eq!(parsed.no_progress, Some(true));
    }

    #[test]
    fn parse_flags_progress_flag_forces_bar() {
        // `--progress` lets a test or unusual invocation force the
        // progress bar even when stdout isn't a TTY. Rare but
        // useful for reproducing CI behavior locally.
        let parsed = parse_flags(&[
            "--progress".to_string(),
            "ubuntu-24.04-live-server".to_string(),
        ])
        .expect("parse ok")
        .expect("not --help");
        assert_eq!(parsed.no_progress, Some(false));
    }

    // ---- #541: --out / --out-dir / --cache-base aliases -------------------

    #[test]
    fn parse_flags_accepts_out_flag_split_form() {
        let p = parse_flags(&[
            "--out".to_string(),
            "/tmp/aegis".to_string(),
            "ubuntu-24.04-live-server".to_string(),
        ])
        .expect("parse ok")
        .expect("not --help");
        assert_eq!(
            p.out_dir.as_deref(),
            Some(std::path::Path::new("/tmp/aegis"))
        );
    }

    #[test]
    fn parse_flags_accepts_out_dir_alias_split_form() {
        // #541: --out-dir is a fetch alias for --out so muscle memory from
        // `aegis-boot flash --out-dir` doesn't get a usage error here.
        let p = parse_flags(&[
            "--out-dir".to_string(),
            "/tmp/aegis-od".to_string(),
            "ubuntu-24.04-live-server".to_string(),
        ])
        .expect("parse ok")
        .expect("not --help");
        assert_eq!(
            p.out_dir.as_deref(),
            Some(std::path::Path::new("/tmp/aegis-od"))
        );
    }

    #[test]
    fn parse_flags_accepts_cache_base_alias_split_form() {
        // #541: --cache-base alias from `aegis-boot fetch-trust-chain`.
        let p = parse_flags(&[
            "--cache-base".to_string(),
            "/tmp/aegis-cb".to_string(),
            "ubuntu-24.04-live-server".to_string(),
        ])
        .expect("parse ok")
        .expect("not --help");
        assert_eq!(
            p.out_dir.as_deref(),
            Some(std::path::Path::new("/tmp/aegis-cb"))
        );
    }

    #[test]
    fn parse_flags_accepts_out_dir_alias_equals_form() {
        let p = parse_flags(&[
            "--out-dir=/tmp/aegis-eq".to_string(),
            "ubuntu-24.04-live-server".to_string(),
        ])
        .expect("parse ok")
        .expect("not --help");
        assert_eq!(
            p.out_dir.as_deref(),
            Some(std::path::Path::new("/tmp/aegis-eq"))
        );
    }

    #[test]
    fn parse_flags_accepts_cache_base_alias_equals_form() {
        let p = parse_flags(&[
            "--cache-base=/tmp/aegis-eq2".to_string(),
            "ubuntu-24.04-live-server".to_string(),
        ])
        .expect("parse ok")
        .expect("not --help");
        assert_eq!(
            p.out_dir.as_deref(),
            Some(std::path::Path::new("/tmp/aegis-eq2"))
        );
    }

    #[test]
    fn filename_from_url_no_path() {
        assert_eq!(filename_from_url("https://example.com/file"), "file");
    }

    #[test]
    fn filename_from_url_root_only() {
        // Edge case: trailing slash → empty filename. Caller's responsibility
        // to ensure URL is sensible; we just produce something non-panicking.
        let r = filename_from_url("https://example.com/");
        assert_eq!(r, "");
    }

    // Env-mutating tests below must not run in parallel with each other
    // (they both twiddle XDG_CACHE_HOME / HOME, which is process-global).
    // `cargo test` runs tests in a module in parallel by default; this
    // Mutex serializes the pair.
    use std::sync::Mutex;
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn default_cache_uses_xdg_cache_home() {
        let _g = ENV_MUTEX
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let prev_xdg = std::env::var_os("XDG_CACHE_HOME");
        // SAFETY: ENV_MUTEX serializes env-mutating tests in this module.
        // SAFETY: ENV_MUTEX serializes env-mutating tests in this module; #[cfg(test)] only.
        // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
        unsafe { std::env::set_var("XDG_CACHE_HOME", "/tmp/aegis-test-xdg") };
        let p = default_cache_dir("ubuntu-24.04-live-server");
        assert_eq!(
            p,
            PathBuf::from("/tmp/aegis-test-xdg/aegis-boot/ubuntu-24.04-live-server")
        );
        match prev_xdg {
            // SAFETY: ENV_MUTEX serializes env-mutating tests in this module; #[cfg(test)] only.
            // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
            Some(v) => unsafe { std::env::set_var("XDG_CACHE_HOME", v) },
            // SAFETY: ENV_MUTEX serializes env-mutating tests in this module; #[cfg(test)] only.
            // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
            None => unsafe { std::env::remove_var("XDG_CACHE_HOME") },
        }
    }

    #[test]
    fn default_cache_falls_back_to_home_dot_cache() {
        let _g = ENV_MUTEX
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let prev_xdg = std::env::var_os("XDG_CACHE_HOME");
        let prev_home = std::env::var_os("HOME");
        // SAFETY: ENV_MUTEX serializes env-mutating tests in this module; #[cfg(test)] only.
        // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
        unsafe { std::env::remove_var("XDG_CACHE_HOME") };
        // SAFETY: ENV_MUTEX serializes env-mutating tests in this module; #[cfg(test)] only.
        // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
        unsafe { std::env::set_var("HOME", "/tmp/aegis-test-home") };
        let p = default_cache_dir("alpine-3.20-standard");
        assert_eq!(
            p,
            PathBuf::from("/tmp/aegis-test-home/.cache/aegis-boot/alpine-3.20-standard")
        );
        match prev_xdg {
            // SAFETY: ENV_MUTEX serializes env-mutating tests in this module; #[cfg(test)] only.
            // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
            Some(v) => unsafe { std::env::set_var("XDG_CACHE_HOME", v) },
            // SAFETY: ENV_MUTEX serializes env-mutating tests in this module; #[cfg(test)] only.
            // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
            None => unsafe { std::env::remove_var("XDG_CACHE_HOME") },
        }
        match prev_home {
            // SAFETY: ENV_MUTEX serializes env-mutating tests in this module; #[cfg(test)] only.
            // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            // SAFETY: ENV_MUTEX serializes env-mutating tests in this module; #[cfg(test)] only.
            // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
            None => unsafe { std::env::remove_var("HOME") },
        }
    }
}
