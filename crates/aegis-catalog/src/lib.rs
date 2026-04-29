// SPDX-License-Identifier: MIT OR Apache-2.0

//! Curated ISO catalog + per-distro URL resolvers for aegis-boot.
//!
//! The catalog is an in-binary list of ISO entries with their canonical
//! download URLs and the URLs of their project-signed SHA256SUMS files.
//! No checksums are pinned in this file: they're fetched from the
//! project's own SHA256SUMS at verify time, with the GPG/minisign
//! signature on that file providing the trust anchor (whoever the
//! project trusts to sign their releases is who we trust here).
//!
//! ## Why a separate crate (#655 Phase 2A)
//!
//! Both `aegis-cli` (host CLI) and `rescue-tui` (in-rescue Catalog
//! screen) need read access to the same `CATALOG` slice + types.
//! Extracting them into a workspace crate avoids two parallel sources
//! of truth and keeps the resolver framework + data co-located.
//!
//! `aegis-cli` owns the operator-facing `recommend` rendering and the
//! `--refresh --write` source-mutation logic that consumes this crate;
//! `rescue-tui` only reads `CATALOG` for its in-rescue download flow.
//!
//! ## Why not pin SHA-256 in this file?
//!
//! Distros release point versions on a cadence we can't track in
//! commits. Pinning a hash here would make the catalog wrong within
//! weeks of every release. Pointing at the project's *signed*
//! SHA256SUMS keeps the catalog evergreen while preserving cryptographic
//! verification — the project's release-signing key is the trust anchor.

use std::process::Command;

// =====================================================================
// Public types
// =====================================================================

/// Secure Boot posture of the ISO's kernel under aegis-boot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    /// Single-character glyph used in the recommend table.
    #[must_use]
    pub fn glyph(self) -> &'static str {
        match self {
            SbStatus::Signed(_) => "\u{2713}",        // ✓
            SbStatus::UnsignedNeedsMok => "\u{2717}", // ✗
            SbStatus::Unknown => "?",
        }
    }

    /// Human-readable label rendered next to the glyph.
    #[must_use]
    pub fn label(self) -> String {
        match self {
            SbStatus::Signed(ca) => format!("signed ({ca})"),
            SbStatus::UnsignedNeedsMok => "unsigned (MOK needed)".to_string(),
            SbStatus::Unknown => "unknown".to_string(),
        }
    }
}

/// Operator-facing usage category for `aegis-boot recommend` grouping.
/// Internal-only — not flowed into the `RecommendEntry` wire format
/// (would bump the JSON schema version). When the catalog grows large
/// enough that JSON consumers want to filter by category, promote
/// this into `aegis_wire_formats::RecommendEntry` and bump the
/// `RECOMMEND_REPORT_SCHEMA_VERSION`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    /// Daily-driver desktop / workstation install media.
    Desktop,
    /// Server install / network-install media.
    Server,
    /// Generic OS installer (DVD, minimal, netinst — not server-targeted).
    Installer,
    /// Minimal / forensic / live-rescue shell.
    Rescue,
}

impl Category {
    /// Section header used by `aegis-boot recommend`'s grouped output.
    #[must_use]
    pub fn header(self) -> &'static str {
        match self {
            Category::Desktop => "DESKTOP",
            Category::Server => "SERVER",
            Category::Installer => "INSTALLER",
            Category::Rescue => "RESCUE / FORENSIC",
        }
    }

    /// Stable display order — Desktop (most common) → Server → Installer → Rescue.
    #[must_use]
    pub fn print_order() -> &'static [Category] {
        &[
            Category::Desktop,
            Category::Server,
            Category::Installer,
            Category::Rescue,
        ]
    }
}

/// Cryptographic signature-verification pattern an [`Entry`] expects.
///
/// Distros publish three distinct shapes today, all driven by the
/// same `iso_url` / `sha256_url` / `sig_url` triple but with
/// different semantics for what the signature actually authenticates:
///
/// - [`SigPattern::ClearsignedSums`] — the SUMS file is a PGP
///   cleartext-signed envelope (RFC 9580 §7) wrapping the
///   plaintext checksum lines. `sha256_url == sig_url` for these
///   entries because the signature is embedded in the file. Used
///   by `AlmaLinux`, Fedora, Rocky.
/// - [`SigPattern::DetachedSigOnSums`] — `sig_url` is a detached
///   binary or armored signature over the SUMS file. `sha256_url
///   != sig_url`. Used by Debian, Ubuntu, Kali, Linux Mint,
///   `GParted`, openSUSE, Pop!\_OS.
/// - [`SigPattern::DetachedSigOnIso`] — `sig_url` is a detached
///   signature over the ISO bytes themselves; the `.sha256`
///   sidecar is unsigned and used only for byte-integrity
///   reporting. Used by Alpine, Manjaro, MX Linux, `SystemRescue`.
///
/// `aegis-fetch` dispatches on this enum exhaustively. Choosing
/// the wrong variant verifies the signature against the wrong
/// bytes and silently downgrades trust, so the field is required
/// (no `Default`) and the variant is reviewed on every catalog
/// addition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SigPattern {
    /// Signature is embedded in the SUMS file as a PGP cleartext
    /// envelope. `sha256_url == sig_url`.
    ClearsignedSums,
    /// Signature is a separate file authenticating the SUMS file.
    /// `sha256_url != sig_url`; both URLs must be fetched.
    DetachedSigOnSums,
    /// Signature is a separate file authenticating the ISO itself.
    /// The `sha256_url` sidecar (when present) is unsigned and
    /// informational; the cryptographic gate is the ISO signature.
    DetachedSigOnIso,
}

/// Identifier for the project / organization that signed an
/// [`Entry`]'s release artifacts. Drives keyring lookup in
/// `aegis-fetch` — each variant maps to a single PGP cert in
/// `crates/aegis-catalog/keyring/<slug>.asc` whose fingerprint is
/// pinned in `crates/aegis-catalog/keyring/fingerprints.toml`.
///
/// New variants are added in the same PR that adds the
/// corresponding `<slug>.asc` keyring file. The lockstep is
/// enforced by a unit test that asserts every catalog entry's
/// vendor has a keyring file present.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Vendor {
    /// `AlmaLinux` — RHEL rebuild signed by the `AlmaLinux` release key.
    AlmaLinux,
    /// Alpine Linux — signed by the Alpine release key.
    Alpine,
    /// Debian — signed by the Debian CD-signing key.
    Debian,
    /// Fedora — signed by the Fedora release-signing key.
    Fedora,
    /// `GParted` Live — signed by the `GParted` release key (Curtis Gedak).
    Gparted,
    /// Kali Linux — signed by the Kali release-signing key.
    Kali,
    /// Linux Mint — signed by the Linux Mint release key.
    LinuxMint,
    /// Manjaro Linux — signed by the Manjaro release-signing key.
    Manjaro,
    /// MX Linux — signed by the MX Linux release key.
    Mx,
    /// openSUSE — signed by the openSUSE project signing key.
    Opensuse,
    /// Rocky Linux — RHEL rebuild signed by the Rocky release key.
    Rocky,
    /// `SystemRescue` — signed by the `SystemRescue` release key.
    SystemRescue,
    /// System76 — signs Pop!_OS releases.
    System76,
    /// Ubuntu — signed by the Ubuntu CD-signing key (Canonical).
    Ubuntu,
}

impl Vendor {
    /// Stable lowercase slug used as the keyring filename stem
    /// (`<slug>.asc` and `<slug>.txt`) and as the
    /// `fingerprints.toml` table key.
    #[must_use]
    pub fn slug(self) -> &'static str {
        match self {
            Vendor::AlmaLinux => "almalinux",
            Vendor::Alpine => "alpine",
            Vendor::Debian => "debian",
            Vendor::Fedora => "fedora",
            Vendor::Gparted => "gparted",
            Vendor::Kali => "kali",
            Vendor::LinuxMint => "linuxmint",
            Vendor::Manjaro => "manjaro",
            Vendor::Mx => "mx",
            Vendor::Opensuse => "opensuse",
            Vendor::Rocky => "rocky",
            Vendor::SystemRescue => "system-rescue",
            Vendor::System76 => "system76",
            Vendor::Ubuntu => "ubuntu",
        }
    }

    /// All vendors currently referenced by the catalog. Used by
    /// the keyring loader test to assert every vendor has a
    /// keyring file present, and by the catalog-refresh
    /// workflow to iterate the upstream key fetch loop.
    #[must_use]
    pub fn all() -> &'static [Vendor] {
        &[
            Vendor::AlmaLinux,
            Vendor::Alpine,
            Vendor::Debian,
            Vendor::Fedora,
            Vendor::Gparted,
            Vendor::Kali,
            Vendor::LinuxMint,
            Vendor::Manjaro,
            Vendor::Mx,
            Vendor::Opensuse,
            Vendor::Rocky,
            Vendor::SystemRescue,
            Vendor::System76,
            Vendor::Ubuntu,
        ]
    }
}

/// One catalog entry.
///
/// `Debug` is auto-derived for logging. Equality is by `slug`
/// alone: the slug is the unique catalog identifier and two
/// entries with the same slug are by construction the same
/// catalog row even if a downstream consumer mutated other
/// fields. This avoids relying on function-pointer comparison
/// for the optional resolver — fn-pointer equality is not
/// load-bearing here, so we don't pretend it is.
#[derive(Debug)]
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
    /// Operator-facing usage category — drives `recommend` table grouping.
    pub category: Category,
    /// Project / organization whose PGP key signs this entry's
    /// release artifacts. Drives keyring lookup in `aegis-fetch`.
    pub vendor: Vendor,
    /// Cryptographic shape of the signature for this entry. See
    /// [`SigPattern`] for the three patterns. `aegis-fetch`
    /// dispatches verify behavior exhaustively on this field.
    pub verify: SigPattern,
    /// Optional URL resolver that walks the project's directory
    /// listing or follows a "latest" redirect to discover the current
    /// ISO filename + sibling SHA / sig URLs (#646). Used by
    /// `aegis-boot recommend --refresh` to detect when the static
    /// fields here are out of date. The static fields stay
    /// authoritative for fast-path use; the resolver is opt-in
    /// freshness check.
    pub resolver: Option<fn() -> Result<ResolvedUrls, ResolverError>>,
}

impl PartialEq for Entry {
    fn eq(&self, other: &Self) -> bool {
        self.slug == other.slug
    }
}

impl Eq for Entry {}

// =====================================================================
// Resolver framework (#646)
// =====================================================================

/// Result of a successful URL resolution. Mirrors the static
/// catalog-entry fields — when present, callers prefer these over
/// the [`Entry`]'s static defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::struct_field_names)] // mirrors Entry's *_url field names
pub struct ResolvedUrls {
    /// Canonical download URL for the ISO at the resolved version.
    pub iso_url: String,
    /// URL of the signed SHA256SUMS / CHECKSUMS file at the resolved version.
    pub sha256_url: String,
    /// URL of the detached GPG / minisign / clearsign signature.
    pub sig_url: String,
}

/// Errors a resolver can raise.
#[derive(Debug, thiserror::Error)]
pub enum ResolverError {
    /// Network fetch failed (DNS, TLS, HTTP non-2xx, timeout).
    #[error("network: failed to fetch {url}: {detail}")]
    Network {
        /// Upstream URL that the resolver was trying to GET.
        url: String,
        /// Cause string forwarded from the underlying transport.
        detail: String,
    },
    /// Listing fetched OK but no filename matched the resolver's pattern.
    #[error("parse: no matching ISO filename in listing at {url}")]
    NoMatch {
        /// Upstream URL whose body was searched.
        url: String,
    },
    /// Listing was not valid UTF-8.
    #[error("parse: response from {url} was not valid utf-8")]
    NotUtf8 {
        /// Upstream URL whose body wasn't UTF-8.
        url: String,
    },
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
///
/// # Errors
///
/// Returns [`ResolverError::Network`] on transport / non-2xx,
/// [`ResolverError::NoMatch`] if the listing has no matching ISO,
/// [`ResolverError::NotUtf8`] if the body isn't UTF-8.
pub fn debian_netinst() -> Result<ResolvedUrls, ResolverError> {
    const BASE: &str = "https://cdimage.debian.org/debian-cd/current/amd64/iso-cd/";
    let html = http_get(BASE)?;
    resolve_debian_netinst_from_html(&html, BASE)
}

fn resolve_debian_netinst_from_html(html: &str, base: &str) -> Result<ResolvedUrls, ResolverError> {
    let re = simple_regex_match_all(html, r"debian-", "-amd64-netinst.iso");
    let mut versions: Vec<String> = re
        .into_iter()
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
/// `ubuntu-24.04.X-live-server-amd64.iso` filename.
///
/// # Errors
///
/// See [`debian_netinst`].
pub fn ubuntu_24_04_live_server() -> Result<ResolvedUrls, ResolverError> {
    const BASE: &str = "https://releases.ubuntu.com/24.04/";
    let html = http_get(BASE)?;
    resolve_ubuntu_24_04_with_html(&html, BASE, "live-server")
}

/// Ubuntu Desktop — same shape as live-server, different infix.
///
/// # Errors
///
/// See [`debian_netinst`].
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
/// highest `kali-linux-X.Y-installer-amd64.iso` filename.
///
/// # Errors
///
/// See [`debian_netinst`].
pub fn kali_installer() -> Result<ResolvedUrls, ResolverError> {
    const BASE: &str = "https://cdimage.kali.org/current/";
    let html = http_get(BASE)?;
    resolve_kali_with_html(&html, BASE)
}

fn resolve_kali_with_html(html: &str, base: &str) -> Result<ResolvedUrls, ResolverError> {
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
        sha256_url: format!("{base}SHA256SUMS"),
        sig_url: format!("{base}SHA256SUMS.gpg"),
    })
}

/// Linux Mint Cinnamon — list `/linuxmint/stable/` for the highest
/// `22.X/` subdirectory, then resolve the cinnamon ISO inside.
///
/// # Errors
///
/// See [`debian_netinst`].
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
    let candidates = parse_versioned_subdirs(parent_html, "22");
    let pick = candidates
        .into_iter()
        .max_by_key(|v| {
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
    let filename = format!("linuxmint-{pick}-{flavor}-64bit.iso");
    Ok(ResolvedUrls {
        iso_url: format!("{dir}{filename}"),
        sha256_url: format!("{dir}sha256sum.txt"),
        sig_url: format!("{dir}sha256sum.txt.gpg"),
    })
}

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

/// Pull "X.Y.Z" version segments from `<a href="<prefix>X.Y.Z<suffix>"`
/// patterns. Conservative — operates on raw HTML without a real
/// parser — but the directory listings we care about are simple
/// auto-generated index pages where this works.
///
/// Multiple prefixes can appear in the haystack with different
/// suffixes (Ubuntu's listing has both `-live-server-amd64.iso` and
/// `-desktop-amd64.iso` entries). When the version validity check
/// fails, advance only past the current prefix so subsequent
/// occurrences are still found.
fn simple_regex_match_all(haystack: &str, prefix: &str, suffix: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(start) = haystack[pos..].find(prefix) {
        let after_prefix = pos + start + prefix.len();
        let Some(end_off) = haystack[after_prefix..].find(suffix) else {
            break;
        };
        let inner = &haystack[after_prefix..after_prefix + end_off];
        if !inner.is_empty() && inner.chars().all(|c| c.is_ascii_digit() || c == '.') {
            out.push(inner.to_string());
            pos = after_prefix + end_off + suffix.len();
        } else {
            pos = after_prefix;
        }
    }
    out
}

/// Pull the highest integer-named subdirectory from an HTML listing.
/// Reserved for resolvers whose layout uses integer build numbers.
#[allow(dead_code)] // wired in when the next resolver lands (#646)
fn parse_highest_numeric_subdir(html: &str) -> Option<u32> {
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

// =====================================================================
// Helpers (rendering-friendly, used by `aegis-cli` printing path)
// =====================================================================

/// Slug lookup with case-insensitive exact match falling back to a
/// unique-prefix match. `aegis-boot recommend ubuntu-24` resolves
/// uniquely to the live-server entry, but bare `ubuntu` is ambiguous
/// across server + desktop and returns `None`.
#[must_use]
pub fn find_entry(slug: &str) -> Option<&'static Entry> {
    let s = slug.to_ascii_lowercase();
    if let Some(e) = CATALOG.iter().find(|e| e.slug.eq_ignore_ascii_case(&s)) {
        return Some(e);
    }
    let prefix_matches: Vec<_> = CATALOG
        .iter()
        .filter(|e| e.slug.to_ascii_lowercase().starts_with(&s))
        .collect();
    if prefix_matches.len() == 1 {
        return Some(prefix_matches[0]);
    }
    None
}

/// Format MiB as a humane size string — "198 MiB" under 1 GiB,
/// "2.5 GiB" at or above. Used by both the host CLI's recommend
/// table and the rescue-tui Catalog screen.
#[must_use]
pub fn humanize(mib: u32) -> String {
    if mib >= 1024 {
        format!("{:.1} GiB", f64::from(mib) / 1024.0)
    } else {
        format!("{mib} MiB")
    }
}

/// Truncate `s` to at most `max` characters (counting Unicode code
/// points), appending an ellipsis if trimmed.
#[must_use]
pub fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut out = s.chars().take(max - 1).collect::<String>();
        out.push('\u{2026}'); // …
        out
    }
}

// =====================================================================
// Embedded vendor keyring (#655 Phase 2B PR-B)
// =====================================================================

/// Vendor → ASCII-armored `OpenPGP` cert bytes baked in at compile
/// time via `include_bytes!` from `crates/aegis-catalog/keyring/`.
///
/// `aegis-fetch`'s `VendorKeyring::embedded()` consumes this slice
/// to build the production keyring. Vendors referenced by
/// [`CATALOG`] but not in this slice cause [`Vendor::UnknownVendor`]
/// failures at fetch time — that's the documented partial-coverage
/// state for vendors whose upstream keyring URL is still being
/// sourced (see `keyring/fingerprints.toml` for the TODO list).
///
/// Set membership of this slice + `EMBEDDED_FINGERPRINTS` is
/// enforced by a unit test that loads each `.asc` and asserts the
/// extracted primary-fingerprint set equals the pinned set.
pub const EMBEDDED_KEYRING: &[(Vendor, &[u8])] = &[
    (Vendor::Ubuntu, include_bytes!("../keyring/ubuntu.asc")),
    (Vendor::Debian, include_bytes!("../keyring/debian.asc")),
    (Vendor::Fedora, include_bytes!("../keyring/fedora.asc")),
    (
        Vendor::AlmaLinux,
        include_bytes!("../keyring/almalinux.asc"),
    ),
    (Vendor::Rocky, include_bytes!("../keyring/rocky.asc")),
    (Vendor::Kali, include_bytes!("../keyring/kali.asc")),
    (Vendor::Alpine, include_bytes!("../keyring/alpine.asc")),
    (Vendor::Manjaro, include_bytes!("../keyring/manjaro.asc")),
    // PR-B2: completing the partial-coverage set (#655).
    (
        Vendor::LinuxMint,
        include_bytes!("../keyring/linuxmint.asc"),
    ),
    (Vendor::Mx, include_bytes!("../keyring/mx.asc")),
    (Vendor::Opensuse, include_bytes!("../keyring/opensuse.asc")),
    (Vendor::Gparted, include_bytes!("../keyring/gparted.asc")),
    (Vendor::System76, include_bytes!("../keyring/system76.asc")),
    (
        Vendor::SystemRescue,
        include_bytes!("../keyring/system-rescue.asc"),
    ),
];

/// Vendor → set of pinned primary-key fingerprints (uppercase hex,
/// no spaces). Mirrors `keyring/fingerprints.toml` for runtime
/// enforcement; the `.toml` file is the human-reviewable source
/// of truth and the catalog-refresh workflow keeps both in sync.
///
/// The loader asserts set-equality between the parsed cert
/// fingerprints and the pinned slice on each `embedded()` call.
/// Mismatch = trust boundary breach = refuse to load.
///
/// Vendors absent from this slice are PR-B partial-coverage
/// vendors; their entries in `keyring/fingerprints.toml` carry
/// the TODO marker.
pub const EMBEDDED_FINGERPRINTS: &[(Vendor, &[&str])] = &[
    (
        Vendor::Ubuntu,
        &["843938DF228D22F7B3742BC0D94AA3F0EFE21092"],
    ),
    (
        Vendor::Debian,
        &["04B54C3CDCA79751B16BC6B5225629DF75B188BD"],
    ),
    (
        Vendor::Fedora,
        &[
            "B0F4950458F69E1150C6C5EDC8AC4916105EF944",
            "C6E7F081CF80E13146676E88829B606631645531",
            "36F612DCF27F7D1A48A835E4DBFCF71C6D9F90A6",
            "4F50A6114CD5C6976A7F1179655A4B02F577861E",
        ],
    ),
    (
        Vendor::AlmaLinux,
        &["BF18AC2876178908D6E71267D36CB86CB86B3716"],
    ),
    (Vendor::Rocky, &["7051C470A929F454CEBE37B715AF5DAC6D745A60"]),
    (Vendor::Kali, &["827C8569F2518CC677FECA1AED65462EC8D5E4C5"]),
    (
        Vendor::Alpine,
        &["0482D84022F52DF1C4E7CD43293ACD0907D9495A"],
    ),
    // Manjaro is the developer-keyring bundle (27 certs). Pinning
    // every fingerprint here would be churny on every key
    // rotation; the loader instead validates "all certs in .asc
    // parse cleanly" without strict-set-equality. The
    // catalog-refresh workflow surfaces any new cert as a
    // reviewable PR.
    (Vendor::Manjaro, &[]),
    // PR-B2: 6 follow-up vendors completing partial coverage (#655).
    (
        Vendor::LinuxMint,
        &["27DEB15644C6B3CF3BD7D291300F846BA25BAE09"],
    ),
    (Vendor::Mx, &["F62EDEAA3AE70A9C99DAC4189B68A1E8B9B6375C"]),
    (
        Vendor::Opensuse,
        &["AD485664E901B867051AB15F35A2F86E29B700A4"],
    ),
    (
        Vendor::Gparted,
        &["EB1DD5BF6F88820BBCF5356C8E94C9CD163E3FB0"],
    ),
    (
        Vendor::System76,
        &["63C46DF0140D738961429F4E204DD8AEC33A7AFF"],
    ),
    (
        Vendor::SystemRescue,
        &["0FF11AF081E98345594812037091115F8320B897"],
    ),
];

// =====================================================================
// The catalog
// =====================================================================

/// The catalog itself. Keep entries alphabetically sorted by slug.
///
/// Pinned URLs are the project's "current stable" pages
/// (releases.ubuntu.com, getfedora.org, etc.). When point releases
/// bump, the URL stays valid for at least one cycle; older
/// releases tend to move to /old-releases/ paths. Update entries
/// when a major version ships, not for every point release.
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
        sig_url: "https://repo.almalinux.org/almalinux/9/isos/x86_64/CHECKSUM",
        sb: SbStatus::Signed("Red Hat / AlmaLinux"),
        purpose: "Free RHEL-rebuild minimal installer. Cross-distro kexec quirk possible.",
        category: Category::Installer,
        vendor: Vendor::AlmaLinux,
        verify: SigPattern::ClearsignedSums,
        resolver: None,
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
        category: Category::Rescue,
        vendor: Vendor::Alpine,
        verify: SigPattern::DetachedSigOnIso,
        resolver: None,
    },
    Entry {
        slug: "alpine-3.20-standard-arm64",
        name: "Alpine Linux 3.20 Standard (arm64)",
        arch: "aarch64",
        size_mib: 200,
        iso_url: "https://dl-cdn.alpinelinux.org/alpine/v3.20/releases/aarch64/alpine-standard-3.20.3-aarch64.iso",
        sha256_url: "https://dl-cdn.alpinelinux.org/alpine/v3.20/releases/aarch64/alpine-standard-3.20.3-aarch64.iso.sha256",
        sig_url: "https://dl-cdn.alpinelinux.org/alpine/v3.20/releases/aarch64/alpine-standard-3.20.3-aarch64.iso.asc",
        sb: SbStatus::UnsignedNeedsMok,
        purpose: "Minimal recovery shell for arm64 hosts (Pi 4/5, ARM servers).",
        category: Category::Rescue,
        vendor: Vendor::Alpine,
        verify: SigPattern::DetachedSigOnIso,
        resolver: None,
    },
    Entry {
        // DistroWatch top-12-month rank #4 (April 2026).
        slug: "debian-13-netinst",
        name: "Debian 13 (trixie) Netinst",
        arch: "x86_64",
        size_mib: 720,
        iso_url: "https://cdimage.debian.org/debian-cd/current/amd64/iso-cd/debian-13.4.0-amd64-netinst.iso",
        // Debian publishes SHA512SUMS (not SHA256). Catalog field is
        // named sha256_url for back-compat; revalidate verifies the
        // URL returns 2xx regardless of digest algorithm.
        sha256_url: "https://cdimage.debian.org/debian-cd/current/amd64/iso-cd/SHA512SUMS",
        sig_url: "https://cdimage.debian.org/debian-cd/current/amd64/iso-cd/SHA512SUMS.sign",
        sb: SbStatus::Signed("Debian CA via shim"),
        purpose: "Minimal Debian network installer. DistroWatch top-5 popularity.",
        category: Category::Installer,
        vendor: Vendor::Debian,
        verify: SigPattern::DetachedSigOnSums,
        resolver: Some(debian_netinst),
    },
    Entry {
        slug: "debian-13-netinst-arm64",
        name: "Debian 13 (trixie) Netinst (arm64)",
        arch: "aarch64",
        size_mib: 720,
        iso_url: "https://cdimage.debian.org/debian-cd/current/arm64/iso-cd/debian-13.4.0-arm64-netinst.iso",
        sha256_url: "https://cdimage.debian.org/debian-cd/current/arm64/iso-cd/SHA512SUMS",
        sig_url: "https://cdimage.debian.org/debian-cd/current/arm64/iso-cd/SHA512SUMS.sign",
        sb: SbStatus::Signed("Debian CA via shim"),
        purpose: "Debian network installer for arm64 (Pi, ARM servers, AWS Graviton).",
        category: Category::Installer,
        vendor: Vendor::Debian,
        verify: SigPattern::DetachedSigOnSums,
        resolver: None,
    },
    Entry {
        slug: "fedora-43-server",
        name: "Fedora 43 Server (DVD)",
        arch: "x86_64",
        size_mib: 2300,
        iso_url: "https://download.fedoraproject.org/pub/fedora/linux/releases/43/Server/x86_64/iso/Fedora-Server-dvd-x86_64-43-1.6.iso",
        sha256_url: "https://download.fedoraproject.org/pub/fedora/linux/releases/43/Server/x86_64/iso/Fedora-Server-43-1.6-x86_64-CHECKSUM",
        sig_url: "https://download.fedoraproject.org/pub/fedora/linux/releases/43/Server/x86_64/iso/Fedora-Server-43-1.6-x86_64-CHECKSUM",
        sb: SbStatus::Signed("Red Hat / Fedora"),
        purpose: "Fedora server install media (full DVD; non-live).",
        category: Category::Server,
        vendor: Vendor::Fedora,
        verify: SigPattern::ClearsignedSums,
        resolver: None,
    },
    Entry {
        // DistroWatch top-12-month rank #9 (April 2026).
        slug: "fedora-43-workstation",
        name: "Fedora 43 Workstation Live",
        arch: "x86_64",
        size_mib: 2600,
        iso_url: "https://download.fedoraproject.org/pub/fedora/linux/releases/43/Workstation/x86_64/iso/Fedora-Workstation-Live-43-1.6.x86_64.iso",
        sha256_url: "https://download.fedoraproject.org/pub/fedora/linux/releases/43/Workstation/x86_64/iso/Fedora-Workstation-43-1.6-x86_64-CHECKSUM",
        // Fedora ships a PGP-clearsigned CHECKSUM (same pattern as
        // AlmaLinux + Rocky).
        sig_url: "https://download.fedoraproject.org/pub/fedora/linux/releases/43/Workstation/x86_64/iso/Fedora-Workstation-43-1.6-x86_64-CHECKSUM",
        sb: SbStatus::Signed("Red Hat / Fedora"),
        purpose: "Fedora desktop live ISO. Cross-distro kexec quirk possible.",
        category: Category::Desktop,
        vendor: Vendor::Fedora,
        verify: SigPattern::ClearsignedSums,
        resolver: None,
    },
    Entry {
        slug: "fedora-43-workstation-arm64",
        name: "Fedora 43 Workstation Live (arm64)",
        arch: "aarch64",
        size_mib: 2400,
        iso_url: "https://download.fedoraproject.org/pub/fedora/linux/releases/43/Workstation/aarch64/iso/Fedora-Workstation-Live-43-1.6.aarch64.iso",
        sha256_url: "https://download.fedoraproject.org/pub/fedora/linux/releases/43/Workstation/aarch64/iso/Fedora-Workstation-43-1.6-aarch64-CHECKSUM",
        sig_url: "https://download.fedoraproject.org/pub/fedora/linux/releases/43/Workstation/aarch64/iso/Fedora-Workstation-43-1.6-aarch64-CHECKSUM",
        sb: SbStatus::Signed("Red Hat / Fedora"),
        purpose: "Fedora desktop live ISO for arm64 (Pi, ARM laptops).",
        category: Category::Desktop,
        vendor: Vendor::Fedora,
        verify: SigPattern::ClearsignedSums,
        resolver: None,
    },
    Entry {
        slug: "gparted-live-1.8.1",
        name: "GParted Live 1.8.1-3",
        arch: "x86_64",
        size_mib: 500,
        // GParted Live hosts the ISO on SourceForge (download mirror
        // redirect) and the signed CHECKSUMS on gparted.org directly.
        iso_url: "https://downloads.sourceforge.net/gparted/gparted-live-1.8.1-3-amd64.iso",
        sha256_url: "https://gparted.org/gparted-live/stable/CHECKSUMS.TXT",
        sig_url: "https://gparted.org/gparted-live/stable/CHECKSUMS.TXT.gpg",
        sb: SbStatus::Signed("Debian shim chain (GParted)"),
        purpose: "Partition editor live ISO. Resize, repair, image disks.",
        category: Category::Rescue,
        vendor: Vendor::Gparted,
        verify: SigPattern::DetachedSigOnSums,
        resolver: None,
    },
    Entry {
        // Not on DistroWatch top-15 but the de-facto pentest distro
        // operators expect to see in a rescue-stick catalog.
        slug: "kali-2026.1-installer",
        name: "Kali Linux 2026.1 (installer)",
        arch: "x86_64",
        size_mib: 4500,
        // Kali publishes a `/current/` symlink that always points at
        // the latest release; the kali_installer resolver
        // canonicalizes to that path.
        iso_url: "https://cdimage.kali.org/current/kali-linux-2026.1-installer-amd64.iso",
        sha256_url: "https://cdimage.kali.org/current/SHA256SUMS",
        sig_url: "https://cdimage.kali.org/current/SHA256SUMS.gpg",
        sb: SbStatus::Signed("Kali / Debian shim chain"),
        purpose: "Pentesting + forensics installer. Debian-derived signed chain.",
        category: Category::Installer,
        vendor: Vendor::Kali,
        verify: SigPattern::DetachedSigOnSums,
        resolver: Some(kali_installer),
    },
    Entry {
        slug: "linuxmint-22-cinnamon",
        name: "Linux Mint 22.3 Cinnamon",
        arch: "x86_64",
        size_mib: 2900,
        // Promoted from /22/ to /22.3/ by #646 phase 3 resolver
        // detecting drift on mirrors.edge.kernel.org/linuxmint/stable/.
        iso_url: "https://mirrors.edge.kernel.org/linuxmint/stable/22.3/linuxmint-22.3-cinnamon-64bit.iso",
        sha256_url: "https://mirrors.edge.kernel.org/linuxmint/stable/22.3/sha256sum.txt",
        sig_url: "https://mirrors.edge.kernel.org/linuxmint/stable/22.3/sha256sum.txt.gpg",
        sb: SbStatus::Signed("Linux Mint"),
        purpose: "Friendly Ubuntu-derived desktop. Common operator install target.",
        category: Category::Desktop,
        vendor: Vendor::LinuxMint,
        verify: SigPattern::DetachedSigOnSums,
        resolver: Some(linuxmint_22_cinnamon),
    },
    Entry {
        // DistroWatch top-12-month rank #8 (April 2026). Manjaro
        // kernels are unsigned for SB; operators on enforcing
        // hardware need to MOK-enroll before kexec.
        slug: "manjaro-26-kde",
        name: "Manjaro 26.0.4 KDE",
        arch: "x86_64",
        size_mib: 3700,
        iso_url: "https://download.manjaro.org/kde/26.0.4/manjaro-kde-26.0.4-260327-linux618.iso",
        sha256_url: "https://download.manjaro.org/kde/26.0.4/manjaro-kde-26.0.4-260327-linux618.iso.sha256",
        sig_url: "https://download.manjaro.org/kde/26.0.4/manjaro-kde-26.0.4-260327-linux618.iso.sig",
        sb: SbStatus::UnsignedNeedsMok,
        purpose: "Arch-derived rolling release with KDE Plasma. Unsigned kernel.",
        category: Category::Desktop,
        vendor: Vendor::Manjaro,
        verify: SigPattern::DetachedSigOnIso,
        resolver: None,
    },
    Entry {
        // DistroWatch top-12-month rank #3 (April 2026).
        slug: "mx-25.1-ahs-xfce",
        name: "MX Linux 25.1 AHS Xfce",
        arch: "x86_64",
        size_mib: 3200,
        iso_url: "https://downloads.sourceforge.net/project/mx-linux/Final/Xfce/MX-25.1_Xfce_ahs_x64.iso",
        sha256_url: "https://downloads.sourceforge.net/project/mx-linux/Final/Xfce/MX-25.1_Xfce_ahs_x64.iso.sha256",
        sig_url: "https://downloads.sourceforge.net/project/mx-linux/Final/Xfce/MX-25.1_Xfce_ahs_x64.iso.sig",
        sb: SbStatus::Signed("Debian shim chain (MX kernel)"),
        purpose: "Debian-based Xfce desktop with newer kernel. Top-3 popularity.",
        category: Category::Desktop,
        vendor: Vendor::Mx,
        verify: SigPattern::DetachedSigOnIso,
        resolver: None,
    },
    Entry {
        // DistroWatch top-12-month rank #12 (April 2026).
        slug: "opensuse-leap-15.6-dvd",
        name: "openSUSE Leap 15.6 DVD",
        arch: "x86_64",
        size_mib: 4400,
        iso_url: "https://download.opensuse.org/distribution/leap/15.6/iso/openSUSE-Leap-15.6-DVD-x86_64-Media.iso",
        sha256_url: "https://download.opensuse.org/distribution/leap/15.6/iso/openSUSE-Leap-15.6-DVD-x86_64-Media.iso.sha256",
        sig_url: "https://download.opensuse.org/distribution/leap/15.6/iso/openSUSE-Leap-15.6-DVD-x86_64-Media.iso.sha256.asc",
        sb: SbStatus::Signed("SUSE CA"),
        purpose: "Enterprise-derived stable distribution. Full DVD installer.",
        category: Category::Installer,
        vendor: Vendor::Opensuse,
        verify: SigPattern::DetachedSigOnSums,
        resolver: None,
    },
    Entry {
        // DistroWatch top-12-month rank #5 (April 2026). Build
        // number (`_9`) bumps frequently — re-check before catalog
        // revalidation.
        slug: "popos-24.04-intel",
        name: "Pop!_OS 24.04 (Intel)",
        arch: "x86_64",
        size_mib: 2900,
        iso_url: "https://iso.pop-os.org/24.04/amd64/intel/9/pop-os_24.04_amd64_intel_9.iso",
        sha256_url: "https://iso.pop-os.org/24.04/amd64/intel/9/SHA256SUMS",
        sig_url: "https://iso.pop-os.org/24.04/amd64/intel/9/SHA256SUMS.gpg",
        sb: SbStatus::Signed("System76 / Canonical CA"),
        purpose: "Ubuntu-derived desktop tuned for System76 hardware.",
        category: Category::Desktop,
        vendor: Vendor::System76,
        verify: SigPattern::DetachedSigOnSums,
        // No resolver yet: iso.pop-os.org returns HTTP 403 on
        // directory listings (nginx autoindex off). Tracked under #646.
        resolver: None,
    },
    Entry {
        slug: "rocky-9-minimal",
        name: "Rocky Linux 9 Minimal",
        arch: "x86_64",
        size_mib: 1900,
        iso_url: "https://download.rockylinux.org/pub/rocky/9/isos/x86_64/Rocky-9-latest-x86_64-minimal.iso",
        sha256_url: "https://download.rockylinux.org/pub/rocky/9/isos/x86_64/CHECKSUM",
        // Rocky ships a PGP-clearsigned CHECKSUM (same pattern as AlmaLinux).
        sig_url: "https://download.rockylinux.org/pub/rocky/9/isos/x86_64/CHECKSUM",
        sb: SbStatus::Signed("Rocky Linux"),
        purpose: "Free RHEL-rebuild minimal installer. Cross-distro kexec quirk possible.",
        category: Category::Installer,
        vendor: Vendor::Rocky,
        verify: SigPattern::ClearsignedSums,
        resolver: None,
    },
    Entry {
        slug: "systemrescue-13.00",
        name: "SystemRescue 13.00",
        arch: "x86_64",
        size_mib: 900,
        // SystemRescue's CDN is fastly-cdn.system-rescue.org; the .asc
        // is a detached PGP signature on the ISO itself (not on a
        // SHA256SUMS file — different from the Debian/Ubuntu pattern).
        iso_url: "https://fastly-cdn.system-rescue.org/releases/13.00/systemrescue-13.00-amd64.iso",
        sha256_url: "https://fastly-cdn.system-rescue.org/releases/13.00/systemrescue-13.00-amd64.iso.sha256",
        sig_url: "https://fastly-cdn.system-rescue.org/releases/13.00/systemrescue-13.00-amd64.iso.asc",
        // Arch-derived; SystemRescue ships their own kernel signed by
        // their key, but stock-SB machines need MOK enrollment.
        sb: SbStatus::UnsignedNeedsMok,
        purpose: "Comprehensive rescue toolkit: parted, testdisk, ddrescue, clamav.",
        category: Category::Rescue,
        vendor: Vendor::SystemRescue,
        verify: SigPattern::DetachedSigOnIso,
        resolver: None,
    },
    Entry {
        slug: "ubuntu-24.04-live-server",
        name: "Ubuntu Server 24.04.4 LTS (live-server)",
        arch: "x86_64",
        size_mib: 2600,
        // The /24.04/ directory accumulates point releases; the
        // ubuntu_24_04_live_server resolver picks the highest.
        iso_url: "https://releases.ubuntu.com/24.04/ubuntu-24.04.4-live-server-amd64.iso",
        sha256_url: "https://releases.ubuntu.com/24.04/SHA256SUMS",
        sig_url: "https://releases.ubuntu.com/24.04/SHA256SUMS.gpg",
        sb: SbStatus::Signed("Canonical CA"),
        purpose: "Ubuntu LTS server installer. Validated under aegis-boot v0.12.0 #109.",
        category: Category::Server,
        vendor: Vendor::Ubuntu,
        verify: SigPattern::DetachedSigOnSums,
        resolver: Some(ubuntu_24_04_live_server),
    },
    Entry {
        slug: "ubuntu-24.04-live-server-arm64",
        name: "Ubuntu Server 24.04.4 LTS (arm64)",
        arch: "aarch64",
        size_mib: 2700,
        // Ubuntu hosts arm64 server on cdimage.ubuntu.com.
        iso_url: "https://cdimage.ubuntu.com/releases/noble/release/ubuntu-24.04.4-live-server-arm64.iso",
        sha256_url: "https://cdimage.ubuntu.com/releases/noble/release/SHA256SUMS",
        sig_url: "https://cdimage.ubuntu.com/releases/noble/release/SHA256SUMS.gpg",
        sb: SbStatus::Signed("Canonical CA"),
        purpose: "Ubuntu LTS server installer for arm64 (Graviton, Ampere, Pi 4/5).",
        category: Category::Server,
        vendor: Vendor::Ubuntu,
        verify: SigPattern::DetachedSigOnSums,
        resolver: None,
    },
    Entry {
        slug: "ubuntu-24.04-desktop",
        name: "Ubuntu Desktop 24.04.4 LTS",
        arch: "x86_64",
        size_mib: 5800,
        iso_url: "https://releases.ubuntu.com/24.04/ubuntu-24.04.4-desktop-amd64.iso",
        sha256_url: "https://releases.ubuntu.com/24.04/SHA256SUMS",
        sig_url: "https://releases.ubuntu.com/24.04/SHA256SUMS.gpg",
        sb: SbStatus::Signed("Canonical CA"),
        purpose: "Ubuntu LTS desktop installer. >4 GB → requires ext4 data partition.",
        category: Category::Desktop,
        vendor: Vendor::Ubuntu,
        verify: SigPattern::DetachedSigOnSums,
        resolver: Some(ubuntu_24_04_desktop),
    },
];

// =====================================================================
// Tests for the moved code
// =====================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]
    use super::*;

    // ---- Catalog data sanity ------------------------------------

    #[test]
    fn catalog_has_at_least_four_entries() {
        // Sanity gate: someone shouldn't accidentally empty out the
        // catalog and have CI still pass.
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
    fn every_category_in_print_order_has_an_entry() {
        for category in Category::print_order() {
            let count = CATALOG.iter().filter(|e| e.category == *category).count();
            assert!(
                count > 0,
                "category {category:?} declared in print_order but has zero entries"
            );
        }
    }

    #[test]
    fn arch_field_is_x86_64_or_aarch64() {
        for e in CATALOG {
            assert!(
                matches!(e.arch, "x86_64" | "aarch64"),
                "{} has unexpected arch {}",
                e.slug,
                e.arch
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

    // ---- find_entry / humanize / truncate / SbStatus ------------

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
        let e = find_entry("alma").expect("alma prefix matches uniquely");
        assert!(e.slug.starts_with("alma"));
    }

    #[test]
    fn find_entry_ambiguous_prefix_returns_none() {
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

    // ---- Resolver framework -------------------------------------

    #[test]
    fn debian_resolver_picks_highest_version_from_listing() {
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

        let r2 = resolve_ubuntu_24_04_with_html(html, "https://example.test/", "desktop")
            .unwrap_or_else(|e| panic!("resolve: {e}"));
        assert_eq!(
            r2.iso_url,
            "https://example.test/ubuntu-24.04.4-desktop-amd64.iso"
        );
    }

    #[test]
    fn linuxmint_resolver_picks_highest_point_release_within_major() {
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

    // ---- SigPattern / Vendor coherence --------------------------

    #[test]
    fn every_entry_has_sigpattern_consistent_with_url_shape() {
        // Cross-check the explicit `verify` field against the URL
        // triple's observable shape today. Drift here means a vendor
        // changed their layout or a new entry was tagged with the
        // wrong pattern — either way the trust path needs review,
        // not silent re-classification.
        for e in CATALOG {
            let iso_filename = e.iso_url.rsplit('/').next().unwrap_or("");
            let sums_eq_sig = e.sha256_url == e.sig_url;
            let sig_targets_iso = !iso_filename.is_empty()
                && (e.sig_url.ends_with(&format!("{iso_filename}.asc"))
                    || e.sig_url.ends_with(&format!("{iso_filename}.sig")));
            match e.verify {
                SigPattern::ClearsignedSums => assert!(
                    sums_eq_sig,
                    "{} declared ClearsignedSums but sha256_url != sig_url",
                    e.slug
                ),
                SigPattern::DetachedSigOnIso => assert!(
                    sig_targets_iso && !sums_eq_sig,
                    "{} declared DetachedSigOnIso but sig_url does not target the ISO filename",
                    e.slug
                ),
                SigPattern::DetachedSigOnSums => assert!(
                    !sums_eq_sig && !sig_targets_iso,
                    "{} declared DetachedSigOnSums but URL shape suggests another pattern",
                    e.slug
                ),
            }
        }
    }

    #[test]
    fn every_vendor_in_catalog_is_in_vendor_all() {
        // Vendor::all() drives the keyring loader test (#655 PR-B)
        // and the catalog-refresh fetch loop. Any catalog entry
        // tagged with a Vendor must appear in all() so the
        // upstream fetch loop can iterate over every vendor we ship.
        use std::collections::HashSet;
        let referenced: HashSet<Vendor> = CATALOG.iter().map(|e| e.vendor).collect();
        let listed: HashSet<Vendor> = Vendor::all().iter().copied().collect();
        for v in &referenced {
            assert!(
                listed.contains(v),
                "Vendor {v:?} is used in CATALOG but not in Vendor::all()"
            );
        }
    }

    #[test]
    fn vendor_slugs_are_distinct_kebab_case() {
        let mut slugs: Vec<&str> = Vendor::all().iter().map(|v| v.slug()).collect();
        slugs.sort_unstable();
        let pre = slugs.len();
        slugs.dedup();
        assert_eq!(pre, slugs.len(), "duplicate Vendor::slug()");
        for s in Vendor::all().iter().map(|v| v.slug()) {
            assert!(!s.is_empty(), "empty vendor slug");
            assert!(
                s.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "vendor slug not kebab-case: {s}"
            );
        }
    }

    #[test]
    fn kali_resolver_picks_highest_release() {
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
