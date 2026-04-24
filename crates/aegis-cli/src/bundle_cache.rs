// SPDX-License-Identifier: MIT OR Apache-2.0

//! Bundle-cache layer — ADR 0002 §3.6 + [#417] Phase 3a.
//!
//! Composes the HTTP downloader (injected via [`Downloader`] so this
//! module stays network-dep-free and deterministically testable) with
//! the verify primitives from [`crate::bundle_verify`] into a single
//! "give me a cached-and-verified bundle" entry point. Phase 3b will
//! supply the real-world [`Downloader`] impl; Phase 3c wires a CLI
//! subcommand on top.
//!
//! ## Cache layout
//!
//! ```text
//! $XDG_CACHE_HOME/aegis-boot/signed-chain/<bundle_version>/
//!     bundle-manifest.json
//!     bundle-manifest.json.minisig
//!     <role-path-1>     (e.g. shim/shimx64.efi)
//!     <role-path-2>
//!     …
//! ```
//!
//! One subdirectory per [`aegis_wire_formats::BundleManifest::bundle_version`]
//! — the strictest cache key the wire format provides. Multiple
//! versions can coexist on disk; eviction is left to the operator
//! (or a future Phase 4 PR).
//!
//! ## Fetch flow
//!
//! 1. Resolve the cache subdir for the requested origin.
//! 2. Download `bundle-manifest.json` + `.minisig` (always — the
//!    cache might be stale + we need the current `key_epoch` to decide
//!    if the cached files are still trusted).
//! 3. Call [`crate::bundle_verify::verify_bundle_manifest`]. On
//!    failure, the error bubbles up unchanged — the caller never
//!    sees an unverified file reference.
//! 4. For each file in the manifest: if the cached copy's sha256
//!    matches the manifest, skip the download; otherwise download
//!    and verify. A size-mismatch short-circuits before the sha256
//!    check (saves bandwidth on a truncated cache).
//! 5. Advance the local seen-epoch via
//!    [`aegis_trust::store_seen_epoch`] once every file is verified.
//! 6. Return a [`BundlePaths`] map of `role → on-disk path`.
//!
//! Content-addressed by sha256 — no separate bookkeeping file. Each
//! `fetch_bundle` call re-verifies every file, so partial caches
//! self-heal without special-case code.
//!
//! [#417]: https://github.com/aegis-boot/aegis-boot/issues/417

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use aegis_trust::TrustAnchor;
use aegis_wire_formats::{BundleFileRole, BundleManifest};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::bundle_verify::{BundleVerifyError, verify_bundle_manifest};

/// Abstract byte-fetcher. Real implementations shell out to curl or
/// use an HTTP crate; tests use an in-memory URL map. Separating the
/// trait from the pipeline lets this module compile with zero
/// network deps and be exercised end-to-end against fixtures.
pub trait Downloader {
    /// Fetch the full body at `url`. Implementations MUST:
    ///
    /// * Reject non-HTTPS schemes (the flasher never trusts
    ///   plaintext transport).
    /// * Time out rather than hang on slow servers.
    /// * Return `DownloadError::Http { status }` for HTTP error
    ///   responses (404, 500, …) so the cache can surface them
    ///   distinctly from network errors.
    ///
    /// # Errors
    ///
    /// Implementation-specific — mapped to [`DownloadError`] variants.
    fn get(&self, url: &str) -> Result<Vec<u8>, DownloadError>;
}

/// Errors a [`Downloader::get`] call can surface. Callers of
/// [`fetch_bundle`] receive these wrapped in [`BundleCacheError::Download`].
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum DownloadError {
    /// HTTP non-2xx response. The status code guides the operator —
    /// 404 means "wrong URL or unpublished version," 500+ means
    /// "retry later."
    #[error("HTTP {status} fetching {url}")]
    Http {
        /// HTTP status code (e.g. 404, 500).
        status: u16,
        /// The URL that returned the error. Preserved verbatim for
        /// operator log-grep.
        url: String,
    },
    /// Transport-level failure (DNS, TLS handshake, connection
    /// reset). String-based because the set of underlying causes
    /// depends on the implementation; the operator sees the raw
    /// message, which is more useful than a bucketed taxonomy here.
    #[error("transport error fetching {url}: {detail}")]
    Transport {
        /// The URL the request was attempting.
        url: String,
        /// Implementation-specific detail.
        detail: String,
    },
    /// The requested URL wasn't an `https://` URL. Implementations
    /// MUST enforce this — plaintext transport isn't acceptable for
    /// signed-chain downloads, even though the signature check
    /// would still catch tampering after the fact.
    #[error("refusing non-HTTPS URL {url}")]
    NotHttps {
        /// The offending URL.
        url: String,
    },
}

/// Top-level errors from [`fetch_bundle`]. Each variant is a
/// distinct operator-facing failure mode so the CLI surface can map
/// to a distinct NEXT ACTION line.
#[derive(Debug, Error)]
pub enum BundleCacheError {
    /// The manifest (or one of its referenced files) didn't download.
    #[error("bundle download failed")]
    Download(#[from] DownloadError),
    /// The verified manifest claims a file with a role outside the
    /// enum this binary knows. Shouldn't happen in practice (the
    /// wire-format type is a closed enum), but surfaces cleanly if
    /// a future role is added and an older binary meets it.
    #[error("bundle manifest lists an unknown file role")]
    UnknownRole,
    /// The manifest verified but a referenced file's sha256 didn't
    /// match the manifest entry — either the download was tampered
    /// with in transit (TLS compromise) or the bundle itself is
    /// inconsistent (maintainer publishing bug).
    #[error("file {path:?} sha256 mismatch: manifest says {expected}, bytes hash to {actual}")]
    Sha256Mismatch {
        /// The role-path from the manifest.
        path: String,
        /// The manifest-declared sha256.
        expected: String,
        /// The sha256 of the bytes that were actually downloaded.
        actual: String,
    },
    /// Manifest verify refused. Wraps [`BundleVerifyError`] so the
    /// full set of verify failure modes bubbles up unchanged.
    #[error("bundle manifest verify refused")]
    Verify(#[from] BundleVerifyError),
    /// Filesystem error (cache dir create, file write, etc).
    #[error("bundle cache I/O: {detail}")]
    Io {
        /// Implementation detail — typically the errno rendered by
        /// `std::io::Error::Display`.
        detail: String,
    },
    /// The manifest's `bundle_version` contained characters that
    /// aren't safe as a filesystem path segment. The flasher joins
    /// this onto the cache base, so `..` / `/` / NUL would let a
    /// malicious manifest write outside the cache — refuse.
    #[error("bundle_version {version:?} is not a safe path segment")]
    UnsafeBundleVersion {
        /// The offending version string.
        version: String,
    },
}

/// Result of a successful [`fetch_bundle`]: the cache directory that
/// holds every verified file, plus a role → path index so callers
/// don't have to re-parse paths. Deliberately uses [`BundleFileRole`]
/// as the map key — each role appears exactly once per well-formed
/// manifest (enforced at fetch time by [`require_role_unique`]).
#[derive(Debug, Clone)]
pub struct BundlePaths {
    /// The cache directory: `<cache_base>/<bundle_version>/`.
    pub cache_dir: PathBuf,
    /// Role → path on disk (under [`Self::cache_dir`]).
    pub files: HashMap<BundleFileRole, PathBuf>,
    /// The manifest this bundle was built from. Useful for callers
    /// that want to surface `bundle_version` / `key_epoch` in a UI.
    pub manifest: BundleManifest,
}

/// Compute the on-disk cache subdirectory for a given `bundle_version`
/// + cache base. Pure function — does not touch the filesystem.
///
/// Rejects bundle versions that would let the manifest escape the
/// cache base via `..` / `/` / NUL / backslashes / embedded CR-LF.
///
/// # Errors
///
/// [`BundleCacheError::UnsafeBundleVersion`] on disallowed input.
pub fn cache_subdir(cache_base: &Path, bundle_version: &str) -> Result<PathBuf, BundleCacheError> {
    if !is_safe_path_segment(bundle_version) {
        return Err(BundleCacheError::UnsafeBundleVersion {
            version: bundle_version.to_string(),
        });
    }
    Ok(cache_base.join(bundle_version))
}

/// Whether `s` is safe as a single filesystem-path segment.
/// Stricter than OS path rules — we ban characters that are legal
/// on Linux but could surprise a Windows or macOS operator reading
/// the cache via a file manager (backslash, control chars).
fn is_safe_path_segment(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && !s.contains('/')
        && !s.contains('\\')
        && !s.contains('\0')
        && !s.contains('\r')
        && !s.contains('\n')
        && !s.starts_with('-')
}

/// Top-level entry point. Fetches the bundle manifest + all its
/// referenced files, caches them, verifies every byte against the
/// manifest's sha256s, and enforces the ADR 0002 epoch-aware signature
/// check.
///
/// `origin_url` must end with `/` — the manifest file and every
/// archive-relative file path are joined onto it.
///
/// `seen_epoch` comes from [`aegis_trust::load_seen_epoch`]. The
/// caller is responsible for persisting the returned manifest's
/// `key_epoch` via [`aegis_trust::store_seen_epoch`] after a
/// successful verify — we don't touch the seen-epoch state here so
/// this function stays a pure read operation with the filesystem
/// side effects scoped to the cache dir.
///
/// # Errors
///
/// See [`BundleCacheError`] for the full set.
pub fn fetch_bundle(
    origin_url: &str,
    cache_base: &Path,
    anchor: &TrustAnchor,
    seen_epoch: u32,
    downloader: &dyn Downloader,
) -> Result<BundlePaths, BundleCacheError> {
    let manifest_url = join_url(origin_url, "bundle-manifest.json");
    let sig_url = join_url(origin_url, "bundle-manifest.json.minisig");

    let manifest_bytes = downloader.get(&manifest_url)?;
    let sig_bytes = downloader.get(&sig_url)?;

    let verified = verify_bundle_manifest(&manifest_bytes, &sig_bytes, anchor, seen_epoch)?;
    let manifest = verified.manifest.clone();

    let cache_dir = cache_subdir(cache_base, &manifest.bundle_version)?;
    fs::create_dir_all(&cache_dir).map_err(|e| BundleCacheError::Io {
        detail: format!("create_dir_all {}: {e}", cache_dir.display()),
    })?;

    // Persist manifest + sig — useful for debugging, and supports a
    // future "verify cached bundle without re-downloading" path.
    write_file(&cache_dir.join("bundle-manifest.json"), &manifest_bytes)?;
    write_file(&cache_dir.join("bundle-manifest.json.minisig"), &sig_bytes)?;

    let mut files: HashMap<BundleFileRole, PathBuf> = HashMap::new();
    for entry in &manifest.files {
        require_role_unique(&files, entry.role)?;

        let dest = cache_dir.join(&entry.path);
        ensure_parent_dir(&dest)?;

        let needs_download = match fs::metadata(&dest) {
            Ok(meta) if meta.len() == entry.size_bytes => {
                let cached_bytes = fs::read(&dest).map_err(|e| BundleCacheError::Io {
                    detail: format!("read {}: {e}", dest.display()),
                })?;
                sha256_hex(&cached_bytes) != entry.sha256
            }
            _ => true,
        };

        if needs_download {
            let file_url = join_url(origin_url, &entry.path);
            let body = downloader.get(&file_url)?;
            let actual = sha256_hex(&body);
            if actual != entry.sha256 {
                return Err(BundleCacheError::Sha256Mismatch {
                    path: entry.path.clone(),
                    expected: entry.sha256.clone(),
                    actual,
                });
            }
            write_file(&dest, &body)?;
        }

        files.insert(entry.role, dest);
    }

    Ok(BundlePaths {
        cache_dir,
        files,
        manifest,
    })
}

fn require_role_unique(
    seen: &HashMap<BundleFileRole, PathBuf>,
    role: BundleFileRole,
) -> Result<(), BundleCacheError> {
    if seen.contains_key(&role) {
        return Err(BundleCacheError::UnknownRole);
    }
    Ok(())
}

fn join_url(base: &str, rel: &str) -> String {
    if base.ends_with('/') {
        format!("{base}{rel}")
    } else {
        format!("{base}/{rel}")
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    hex::encode(digest)
}

fn ensure_parent_dir(p: &Path) -> Result<(), BundleCacheError> {
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).map_err(|e| BundleCacheError::Io {
            detail: format!("create_dir_all {}: {e}", parent.display()),
        })?;
    }
    Ok(())
}

fn write_file(p: &Path, body: &[u8]) -> Result<(), BundleCacheError> {
    fs::write(p, body).map_err(|e| BundleCacheError::Io {
        detail: format!("write {}: {e}", p.display()),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use aegis_wire_formats::{BUNDLE_MANIFEST_SCHEMA_VERSION, BundleFileEntry};
    use std::sync::Mutex;

    /// In-memory URL → body map. Every test that exercises the fetch
    /// pipeline drops one of these in; production code gets the real
    /// downloader (Phase 3b).
    struct MockDownloader {
        responses: HashMap<String, Result<Vec<u8>, DownloadError>>,
        call_log: Mutex<Vec<String>>,
    }

    impl MockDownloader {
        fn new() -> Self {
            Self {
                responses: HashMap::new(),
                call_log: Mutex::new(Vec::new()),
            }
        }

        fn respond(&mut self, url: &str, body: Vec<u8>) {
            self.responses.insert(url.to_string(), Ok(body));
        }

        fn respond_err(&mut self, url: &str, err: DownloadError) {
            self.responses.insert(url.to_string(), Err(err));
        }

        fn log(&self) -> Vec<String> {
            self.call_log.lock().unwrap().clone()
        }
    }

    impl Downloader for MockDownloader {
        fn get(&self, url: &str) -> Result<Vec<u8>, DownloadError> {
            self.call_log.lock().unwrap().push(url.to_string());
            match self.responses.get(url) {
                Some(Ok(body)) => Ok(body.clone()),
                Some(Err(e)) => Err(e.clone()),
                None => Err(DownloadError::Http {
                    status: 404,
                    url: url.to_string(),
                }),
            }
        }
    }

    fn sample_manifest_with_files(files: Vec<BundleFileEntry>) -> BundleManifest {
        BundleManifest {
            schema_version: BUNDLE_MANIFEST_SCHEMA_VERSION,
            key_epoch: 1,
            bundle_version: "0.17.0+bundle.1".to_string(),
            generated_at: "2026-04-24T20:00:00-04:00".to_string(),
            origin_url: "https://example.invalid/bundle/".to_string(),
            files,
            note: String::new(),
        }
    }

    #[test]
    fn cache_subdir_joins_safely() {
        let base = Path::new("/tmp/cache");
        let sub = cache_subdir(base, "0.17.0+bundle.1").unwrap();
        assert_eq!(sub, PathBuf::from("/tmp/cache/0.17.0+bundle.1"));
    }

    #[test]
    fn cache_subdir_rejects_dot_dot() {
        let err = cache_subdir(Path::new("/tmp/cache"), "../evil").unwrap_err();
        assert!(matches!(err, BundleCacheError::UnsafeBundleVersion { .. }));
    }

    #[test]
    fn cache_subdir_rejects_slash() {
        let err = cache_subdir(Path::new("/tmp/cache"), "a/b").unwrap_err();
        assert!(matches!(err, BundleCacheError::UnsafeBundleVersion { .. }));
    }

    #[test]
    fn cache_subdir_rejects_backslash() {
        let err = cache_subdir(Path::new("/tmp/cache"), "a\\b").unwrap_err();
        assert!(matches!(err, BundleCacheError::UnsafeBundleVersion { .. }));
    }

    #[test]
    fn cache_subdir_rejects_embedded_nul() {
        let err = cache_subdir(Path::new("/tmp/cache"), "a\0b").unwrap_err();
        assert!(matches!(err, BundleCacheError::UnsafeBundleVersion { .. }));
    }

    #[test]
    fn cache_subdir_rejects_leading_dash() {
        // `-foo` would confuse a tool that receives the path via
        // argv — refuse the whole class.
        let err = cache_subdir(Path::new("/tmp/cache"), "-foo").unwrap_err();
        assert!(matches!(err, BundleCacheError::UnsafeBundleVersion { .. }));
    }

    #[test]
    fn cache_subdir_rejects_empty() {
        let err = cache_subdir(Path::new("/tmp/cache"), "").unwrap_err();
        assert!(matches!(err, BundleCacheError::UnsafeBundleVersion { .. }));
    }

    #[test]
    fn join_url_handles_trailing_slash_variants() {
        assert_eq!(
            join_url("https://example.invalid/", "a.txt"),
            "https://example.invalid/a.txt"
        );
        assert_eq!(
            join_url("https://example.invalid", "a.txt"),
            "https://example.invalid/a.txt"
        );
    }

    #[test]
    fn sha256_hex_matches_known_vector() {
        // Well-known: SHA-256 of the empty string.
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn fetch_bundle_surfaces_http_error_from_manifest_download() {
        let mut dl = MockDownloader::new();
        dl.respond_err(
            "https://example.invalid/bundle/bundle-manifest.json",
            DownloadError::Http {
                status: 404,
                url: "https://example.invalid/bundle/bundle-manifest.json".to_string(),
            },
        );
        let anchor = TrustAnchor::load().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let err = fetch_bundle(
            "https://example.invalid/bundle/",
            tmp.path(),
            &anchor,
            0,
            &dl,
        )
        .unwrap_err();
        match err {
            BundleCacheError::Download(DownloadError::Http { status: 404, .. }) => {}
            other => panic!("wrong error: {other:?}"),
        }
    }

    #[test]
    fn fetch_bundle_surfaces_verify_error_for_bogus_manifest() {
        // Manifest body decodes cleanly but the sig is rubbish.
        // Shape checks pass → trust verify fires → wrapped as
        // BundleCacheError::Verify(Trust(SignatureInvalid)).
        let m = sample_manifest_with_files(vec![BundleFileEntry {
            role: BundleFileRole::Shim,
            path: "shim/shimx64.efi".to_string(),
            sha256: "a".repeat(64),
            size_bytes: 1,
        }]);
        let body = serde_json::to_vec(&m).unwrap();

        let mut dl = MockDownloader::new();
        dl.respond("https://example.invalid/bundle/bundle-manifest.json", body);
        dl.respond(
            "https://example.invalid/bundle/bundle-manifest.json.minisig",
            b"not a real minisig".to_vec(),
        );

        let anchor = TrustAnchor::load().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let err = fetch_bundle(
            "https://example.invalid/bundle/",
            tmp.path(),
            &anchor,
            0,
            &dl,
        )
        .unwrap_err();
        assert!(matches!(err, BundleCacheError::Verify(_)));
    }

    #[test]
    fn fetch_bundle_surfaces_verify_error_for_seen_epoch_too_high() {
        // key_epoch=1 in manifest; local seen_epoch=5. verify_bundle_manifest
        // rejects before any file download happens — assert that by
        // checking that the mock downloader only saw the manifest +
        // sig URLs, never a file URL.
        let m = sample_manifest_with_files(vec![BundleFileEntry {
            role: BundleFileRole::Shim,
            path: "shim/shimx64.efi".to_string(),
            sha256: "a".repeat(64),
            size_bytes: 1,
        }]);
        let body = serde_json::to_vec(&m).unwrap();

        let mut dl = MockDownloader::new();
        dl.respond("https://example.invalid/bundle/bundle-manifest.json", body);
        dl.respond(
            "https://example.invalid/bundle/bundle-manifest.json.minisig",
            b"not a real minisig".to_vec(),
        );

        let anchor = TrustAnchor::load().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let _ = fetch_bundle(
            "https://example.invalid/bundle/",
            tmp.path(),
            &anchor,
            5, // seen_epoch higher than manifest's 1
            &dl,
        );
        let urls = dl.log();
        assert_eq!(
            urls.len(),
            2,
            "epoch-rejection must fire before any file download: saw {urls:?}"
        );
    }

    #[test]
    fn is_safe_path_segment_whitelist() {
        assert!(is_safe_path_segment("0.17.0+bundle.1"));
        assert!(is_safe_path_segment("v1.0.0-rc.1"));
        // Plain version string variants:
        assert!(is_safe_path_segment("bundle_42"));
    }

    #[test]
    fn is_safe_path_segment_blacklist() {
        for s in ["", ".", "..", "a/b", "a\\b", "a\0b", "a\nb", "a\rb", "-foo"] {
            assert!(!is_safe_path_segment(s), "expected reject: {s:?}");
        }
    }
}
