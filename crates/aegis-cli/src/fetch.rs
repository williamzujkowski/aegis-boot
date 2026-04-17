//! `aegis-boot fetch <slug>` — download + verify a catalog ISO.
//!
//! Resolves a slug from the catalog, downloads the ISO, the project's
//! signed `SHA256SUMS`, and the signature on `SHA256SUMS`, then runs
//! the verification recipe a careful operator would type by hand:
//!
//!   1. `sha256sum -c SHA256SUMS --ignore-missing` against the ISO
//!   2. `gpg --verify SHA256SUMS.sig SHA256SUMS` (best-effort: gpg
//!      won't trust unfamiliar keys; we surface the gpg output so the
//!      operator can decide whether to import the project's key)
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
//! Network + verification both shell out to system tools (curl,
//! sha256sum, gpg) rather than pulling in reqwest + sha2 + gpgme as
//! Rust deps. Keeps the static-musl binary small and the trust
//! boundary explicit (system tools are visible in `aegis-boot
//! doctor`'s prerequisite check).

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use crate::catalog::{find_entry, Entry, SbStatus};

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
    let mut out_dir: Option<PathBuf> = None;
    let mut skip_gpg = false;
    let mut slug: Option<String> = None;
    let mut iter = args.iter();
    while let Some(a) = iter.next() {
        match a.as_str() {
            "--help" | "-h" => {
                print_help();
                return Ok(());
            }
            "--out" => {
                let Some(v) = iter.next() else {
                    eprintln!("aegis-boot fetch: --out requires a directory argument");
                    return Err(2);
                };
                out_dir = Some(PathBuf::from(v));
            }
            "--no-gpg" => {
                skip_gpg = true;
            }
            arg if arg.starts_with("--out=") => {
                out_dir = Some(PathBuf::from(arg.trim_start_matches("--out=")));
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

    let Some(entry) = find_entry(&slug) else {
        eprintln!("aegis-boot fetch: no catalog entry matching '{slug}'");
        eprintln!("run 'aegis-boot recommend' to see available slugs");
        return Err(1);
    };

    let dest = out_dir.unwrap_or_else(|| default_cache_dir(entry.slug));
    if let Err(e) = std::fs::create_dir_all(&dest) {
        eprintln!("aegis-boot fetch: cannot create {}: {e}", dest.display());
        return Err(1);
    }

    println!("Fetching {} into {}", entry.name, dest.display());
    println!();

    let iso_filename = filename_from_url(entry.iso_url);
    let sha_filename = filename_from_url(entry.sha256_url);
    let sig_filename = filename_from_url(entry.sig_url);

    if let Err(e) = download(entry.iso_url, &dest.join(&iso_filename)) {
        eprintln!("aegis-boot fetch: ISO download failed: {e}");
        return Err(1);
    }
    if let Err(e) = download(entry.sha256_url, &dest.join(&sha_filename)) {
        eprintln!("aegis-boot fetch: SHA256SUMS download failed: {e}");
        return Err(1);
    }
    if let Err(e) = download(entry.sig_url, &dest.join(&sig_filename)) {
        // Sig download is best-effort if the user opts out of GPG, but
        // useful to have on disk regardless.
        eprintln!("aegis-boot fetch: signature download failed: {e}");
        if !skip_gpg {
            return Err(1);
        }
    }

    println!();
    println!("Verifying SHA-256 of {iso_filename} against {sha_filename}...");
    if !verify_sha256(&dest, &iso_filename, &sha_filename) {
        eprintln!();
        eprintln!("aegis-boot fetch: SHA-256 verification FAILED");
        eprintln!(
            "the ISO at {} does not match the project's published checksum",
            dest.join(&iso_filename).display()
        );
        eprintln!("re-fetch from a different network if you suspect MITM, or check");
        eprintln!("the project's release page for an updated checksum file");
        return Err(1);
    }
    println!("  SHA-256: OK");

    if skip_gpg {
        println!();
        println!("(GPG verification skipped per --no-gpg)");
    } else if let Some(code) = handle_gpg_step(&dest, &sha_filename, &sig_filename, &slug) {
        return Err(code);
    }

    print_success(entry, &dest, &iso_filename);
    Ok(())
}

/// Run + report GPG verification. Returns `Some(code)` to abort the
/// whole `fetch` command (BAD signature, gpg missing); `None` to
/// continue (OK or unknown-key — both are non-fatal because the
/// operator can review and re-run).
fn handle_gpg_step(dest: &Path, sums: &str, sig: &str, slug: &str) -> Option<u8> {
    println!();
    println!("Verifying GPG signature of {sums}...");
    match verify_gpg(dest, sums, sig) {
        GpgVerdict::Ok => {
            println!("  GPG: OK");
            None
        }
        GpgVerdict::UnknownKey(stderr) => {
            println!("  GPG: signature present but signing key not in your keyring.");
            println!();
            println!("  This is normal the first time you fetch from a project. Inspect");
            println!("  the gpg output below — if you trust the project, import their key");
            println!("  (typically a `gpg --keyserver keys.openpgp.org --recv-keys ...`)");
            println!("  and re-run `aegis-boot fetch {slug}`.");
            println!();
            println!("  --- gpg --verify ---");
            for line in stderr.lines() {
                println!("  {line}");
            }
            println!("  --- end ---");
            None
        }
        GpgVerdict::Bad(stderr) => {
            eprintln!();
            eprintln!("aegis-boot fetch: GPG signature is INVALID for this signing key");
            eprintln!("the SHA256SUMS file appears to have been tampered with, OR you've");
            eprintln!("downloaded a stale signature; re-fetch from a different network");
            eprintln!();
            eprintln!("--- gpg --verify ---");
            eprintln!("{stderr}");
            eprintln!("--- end ---");
            Some(1)
        }
        GpgVerdict::GpgMissing => {
            eprintln!();
            eprintln!("aegis-boot fetch: gpg not found in PATH");
            eprintln!("install gpg (e.g. `sudo apt-get install gnupg`) and re-run, or pass");
            eprintln!("--no-gpg to skip signature verification (NOT recommended)");
            Some(1)
        }
    }
}

fn print_help() {
    println!("aegis-boot fetch — download + verify a catalog ISO");
    println!();
    println!("USAGE:");
    println!("  aegis-boot fetch <slug>");
    println!("  aegis-boot fetch --out /path/to/dir <slug>");
    println!("  aegis-boot fetch --no-gpg <slug>      # SHA-256 only (NOT recommended)");
    println!("  aegis-boot fetch --help");
    println!();
    println!("OPTIONS:");
    println!("  --out DIR     Destination directory (default: $XDG_CACHE_HOME/aegis-boot/<slug>)");
    println!("  --no-gpg      Skip GPG signature verification on SHA256SUMS");
    println!("  --help        This message");
    println!();
    println!("EXAMPLES:");
    println!("  aegis-boot fetch ubuntu-24.04-live-server");
    println!("  aegis-boot fetch --out ~/Downloads alpine-3.20-standard");
    println!();
    println!("`aegis-boot fetch` does not write to a USB stick; it downloads + verifies");
    println!("the ISO and prints the `aegis-boot add` command to copy it onto a stick.");
}

fn default_cache_dir(slug: &str) -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("aegis-boot").join(slug)
}

fn filename_from_url(url: &str) -> String {
    url.rsplit('/').next().unwrap_or("download").to_string()
}

fn download(url: &str, dest: &Path) -> Result<(), String> {
    if dest.is_file() {
        // Already downloaded; skip. Sha verification will catch a
        // half-finished partial-download from a previous failed run.
        println!("  skip: {} already present", dest.display());
        return Ok(());
    }
    println!("  GET {url}");
    let status = Command::new("curl")
        .args([
            "--proto",
            "=https",
            "--tlsv1.2",
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--output",
        ])
        .arg(dest)
        .arg(url)
        .status()
        .map_err(|e| format!("curl exec failed: {e} (is curl installed?)"))?;
    if !status.success() {
        // Clean up partial file so a retry doesn't see it as "already
        // downloaded" via the is_file() check above.
        let _ = std::fs::remove_file(dest);
        return Err(format!("curl exited with status {status}"));
    }
    Ok(())
}

fn verify_sha256(dir: &Path, iso: &str, sums: &str) -> bool {
    let out = Command::new("sha256sum")
        .args(["-c", "--ignore-missing", sums])
        .current_dir(dir)
        .output();
    match out {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            // sha256sum output: "<file>: OK" or "<file>: FAILED"
            // We want the line for our specific iso.
            for line in stdout.lines() {
                if line.starts_with(iso) {
                    println!("  {line}");
                    return line.ends_with(": OK");
                }
            }
            // ISO not in checksums file at all? print stderr and fail.
            eprintln!("(no entry for {iso} in {sums})");
            eprintln!("{}", String::from_utf8_lossy(&out.stderr));
            false
        }
        Err(e) => {
            eprintln!("sha256sum exec failed: {e} (is sha256sum installed?)");
            false
        }
    }
}

enum GpgVerdict {
    Ok,
    UnknownKey(String),
    Bad(String),
    GpgMissing,
}

fn verify_gpg(dir: &Path, sums: &str, sig: &str) -> GpgVerdict {
    // gpg writes the verdict to stderr, not stdout.
    let out = Command::new("gpg")
        .args(["--verify", sig, sums])
        .current_dir(dir)
        .output();
    let Ok(out) = out else {
        return GpgVerdict::GpgMissing;
    };
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    if out.status.success() {
        return GpgVerdict::Ok;
    }
    // gpg exit 2 + "Can't check signature: No public key" → unknown key
    // gpg exit 1 + "BAD signature" → genuine fail
    let lower = stderr.to_lowercase();
    if lower.contains("no public key") || lower.contains("can't check signature") {
        GpgVerdict::UnknownKey(stderr)
    } else {
        GpgVerdict::Bad(stderr)
    }
}

fn print_success(entry: &Entry, dest: &Path, iso_filename: &str) {
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
    use super::*;

    #[test]
    fn filename_from_basic_url() {
        assert_eq!(
            filename_from_url("https://example.com/path/to/file.iso"),
            "file.iso"
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

    #[test]
    fn default_cache_uses_xdg_cache_home() {
        // Save and restore env to avoid leaking into other tests.
        let prev = std::env::var_os("XDG_CACHE_HOME");
        // SAFETY: tests run sequentially in this module; mutation is scoped.
        std::env::set_var("XDG_CACHE_HOME", "/tmp/aegis-test-xdg");
        let p = default_cache_dir("ubuntu-24.04-live-server");
        assert_eq!(
            p,
            PathBuf::from("/tmp/aegis-test-xdg/aegis-boot/ubuntu-24.04-live-server")
        );
        match prev {
            Some(v) => std::env::set_var("XDG_CACHE_HOME", v),
            None => std::env::remove_var("XDG_CACHE_HOME"),
        }
    }

    #[test]
    fn default_cache_falls_back_to_home_dot_cache() {
        let prev_xdg = std::env::var_os("XDG_CACHE_HOME");
        let prev_home = std::env::var_os("HOME");
        std::env::remove_var("XDG_CACHE_HOME");
        std::env::set_var("HOME", "/tmp/aegis-test-home");
        let p = default_cache_dir("alpine-3.20-standard");
        assert_eq!(
            p,
            PathBuf::from("/tmp/aegis-test-home/.cache/aegis-boot/alpine-3.20-standard")
        );
        match prev_xdg {
            Some(v) => std::env::set_var("XDG_CACHE_HOME", v),
            None => std::env::remove_var("XDG_CACHE_HOME"),
        }
        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }
}
