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
    fn header(self) -> &'static str {
        match self {
            Category::Desktop => "DESKTOP",
            Category::Server => "SERVER",
            Category::Installer => "INSTALLER",
            Category::Rescue => "RESCUE / FORENSIC",
        }
    }

    /// Stable display order — Desktop (most common) → Server → Installer → Rescue.
    fn print_order() -> &'static [Category] {
        &[
            Category::Desktop,
            Category::Server,
            Category::Installer,
            Category::Rescue,
        ]
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
    /// Operator-facing usage category — drives `recommend` table grouping.
    pub category: Category,
    /// Optional URL resolver that walks the project's directory
    /// listing or follows a "latest" redirect to discover the current
    /// ISO filename + sibling SHA / sig URLs (#646). Used by
    /// `aegis-boot recommend --refresh` to detect when the static
    /// fields here are out of date. The static fields stay
    /// authoritative for fast-path use; the resolver is opt-in
    /// freshness check.
    pub resolver: Option<
        fn() -> Result<
            crate::catalog_resolvers::ResolvedUrls,
            crate::catalog_resolvers::ResolverError,
        >,
    >,
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
        category: Category::Installer,
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
        resolver: Some(crate::catalog_resolvers::debian_netinst),
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
        // arm64 path mirrors the amd64 one verbatim (just s/amd64/arm64/g);
        // a follow-up resolver could share parsing with debian_netinst.
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
        // AlmaLinux + Rocky). The signature is embedded in the
        // CHECKSUM file; no separate .asc.
        sig_url: "https://download.fedoraproject.org/pub/fedora/linux/releases/43/Workstation/x86_64/iso/Fedora-Workstation-43-1.6-x86_64-CHECKSUM",
        sb: SbStatus::Signed("Red Hat / Fedora"),
        purpose: "Fedora desktop live ISO. Cross-distro kexec quirk possible.",
        category: Category::Desktop,
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
        // canonicalizes to that path. Filename still encodes the
        // version so #646 phase 2 can detect when 2026.1 → 2026.2.
        iso_url: "https://cdimage.kali.org/current/kali-linux-2026.1-installer-amd64.iso",
        sha256_url: "https://cdimage.kali.org/current/SHA256SUMS",
        sig_url: "https://cdimage.kali.org/current/SHA256SUMS.gpg",
        sb: SbStatus::Signed("Kali / Debian shim chain"),
        purpose: "Pentesting + forensics installer. Debian-derived signed chain.",
        category: Category::Installer,
        resolver: Some(crate::catalog_resolvers::kali_installer),
    },
    Entry {
        slug: "linuxmint-22-cinnamon",
        name: "Linux Mint 22.3 Cinnamon",
        arch: "x86_64",
        size_mib: 2900,
        // Promoted from /22/ to /22.3/ by #646 phase 3 resolver
        // detecting drift on mirrors.edge.kernel.org/linuxmint/stable/.
        // Mint accumulates point-release dirs (22, 22.1, 22.2, 22.3...)
        // and the linuxmint_22_cinnamon resolver picks the highest
        // within the 22 major.
        iso_url: "https://mirrors.edge.kernel.org/linuxmint/stable/22.3/linuxmint-22.3-cinnamon-64bit.iso",
        sha256_url: "https://mirrors.edge.kernel.org/linuxmint/stable/22.3/sha256sum.txt",
        sig_url: "https://mirrors.edge.kernel.org/linuxmint/stable/22.3/sha256sum.txt.gpg",
        sb: SbStatus::Signed("Linux Mint"),
        purpose: "Friendly Ubuntu-derived desktop. Common operator install target.",
        category: Category::Desktop,
        resolver: Some(crate::catalog_resolvers::linuxmint_22_cinnamon),
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
        // No resolver yet: iso.pop-os.org returns HTTP 403 on
        // directory listings (nginx autoindex off). The published
        // build number lives in the JSON payload at /api/v1 instead.
        // Tracked under #646 as a follow-up resolver to add once the
        // Debian one (this PR's MVP) ships.
        resolver: None,
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
        category: Category::Installer,
        resolver: None,
    },
    Entry {
        slug: "ubuntu-24.04-live-server",
        name: "Ubuntu Server 24.04.4 LTS (live-server)",
        arch: "x86_64",
        size_mib: 2600,
        // The /24.04/ directory accumulates point releases; the
        // ubuntu_24_04_live_server resolver picks the highest. As of
        // April 2026 that's 24.04.4 (resolver detected drift from
        // the prior static 24.04.2 — promoted here in #646 phase 2).
        iso_url: "https://releases.ubuntu.com/24.04/ubuntu-24.04.4-live-server-amd64.iso",
        sha256_url: "https://releases.ubuntu.com/24.04/SHA256SUMS",
        sig_url: "https://releases.ubuntu.com/24.04/SHA256SUMS.gpg",
        sb: SbStatus::Signed("Canonical CA"),
        purpose: "Ubuntu LTS server installer. Validated under aegis-boot v0.12.0 #109.",
        category: Category::Server,
        resolver: Some(crate::catalog_resolvers::ubuntu_24_04_live_server),
    },
    Entry {
        slug: "ubuntu-24.04-live-server-arm64",
        name: "Ubuntu Server 24.04.4 LTS (arm64)",
        arch: "aarch64",
        size_mib: 2700,
        // Ubuntu hosts arm64 server on cdimage.ubuntu.com (not
        // releases.ubuntu.com — that's amd64 only). The /noble/release/
        // path tracks the latest LTS point release.
        iso_url: "https://cdimage.ubuntu.com/releases/noble/release/ubuntu-24.04.4-live-server-arm64.iso",
        sha256_url: "https://cdimage.ubuntu.com/releases/noble/release/SHA256SUMS",
        sig_url: "https://cdimage.ubuntu.com/releases/noble/release/SHA256SUMS.gpg",
        sb: SbStatus::Signed("Canonical CA"),
        purpose: "Ubuntu LTS server installer for arm64 (Graviton, Ampere, Pi 4/5).",
        category: Category::Server,
        resolver: None,
    },
    Entry {
        slug: "ubuntu-24.04-desktop",
        name: "Ubuntu Desktop 24.04.4 LTS",
        arch: "x86_64",
        size_mib: 5800,
        // Promoted from 24.04.2 → 24.04.4 by #646 phase 2 resolver
        // detecting drift on releases.ubuntu.com/24.04/.
        iso_url: "https://releases.ubuntu.com/24.04/ubuntu-24.04.4-desktop-amd64.iso",
        sha256_url: "https://releases.ubuntu.com/24.04/SHA256SUMS",
        sig_url: "https://releases.ubuntu.com/24.04/SHA256SUMS.gpg",
        sb: SbStatus::Signed("Canonical CA"),
        purpose: "Ubuntu LTS desktop installer. >4 GB → requires ext4 data partition.",
        category: Category::Desktop,
        resolver: Some(crate::catalog_resolvers::ubuntu_24_04_desktop),
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

/// `aegis-boot recommend --refresh` — walk every Entry that has a
/// resolver, call it, and print a diff against the static URL. Does
/// NOT mutate the catalog file: the operator (or a CI auto-PR
/// workflow per #646) decides whether to promote the resolver-
/// discovered URL.
///
/// Exit codes:
///   0 — no drift (or no resolvers configured)
///   1 — at least one resolver returned a URL different from static
///   2 — at least one resolver errored (network / parse)
///
/// Network is best-effort. A resolver failure is reported but
/// doesn't poison the rest of the run; we want operators to see
/// "Pop!_OS resolver failed; Debian + Manjaro drifted" not just
/// "first one failed, stopped."
fn run_refresh(write: bool) -> ExitCode {
    let mut any_drift = false;
    let mut any_error = false;
    let mut drifts: Vec<(&'static str, crate::catalog_resolvers::ResolvedUrls)> = Vec::new();
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

/// Find this crate's `catalog.rs` source file and rewrite the URL
/// fields for each drifted entry. Used by `--refresh --write` so a
/// CI workflow can open an auto-PR with the URL bumps.
///
/// Mutation strategy: read the file, find each entry's `Entry { ...
/// slug: "<slug>" ... iso_url: "...", sha256_url: "...", sig_url: "...",
/// ... }` block, replace the three URL string literals. The catalog
/// format is consistent enough that targeted regex-based replacement
/// works without needing a syn-based AST walk.
///
/// The catalog file is found by walking up from `CARGO_MANIFEST_DIR`
/// at compile time and resolving `crates/aegis-cli/src/catalog.rs`.
/// At runtime we look for it relative to the current working
/// directory (typical: `cargo run` from repo root) or as a sibling
/// to the running binary's `target/release` parent (typical: invoked
/// from a CI checkout).
fn write_catalog_drifts(
    drifts: &[(&'static str, crate::catalog_resolvers::ResolvedUrls)],
) -> Result<String, String> {
    let candidates = [
        std::path::PathBuf::from("crates/aegis-cli/src/catalog.rs"),
        std::path::PathBuf::from("../aegis-cli/src/catalog.rs"),
        std::path::PathBuf::from("../../crates/aegis-cli/src/catalog.rs"),
    ];
    let path = candidates
        .iter()
        .find(|p| p.is_file())
        .ok_or_else(|| {
            "couldn't locate crates/aegis-cli/src/catalog.rs from cwd; run from repo root"
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
/// fields. Uses byte-offset slicing on the source string; the
/// catalog format's regular `Entry { ... slug: "<slug>", ... }`
/// shape is what makes this tractable without a real Rust parser.
fn rewrite_entry_urls(
    source: &str,
    slug: &str,
    live: &crate::catalog_resolvers::ResolvedUrls,
) -> Result<String, String> {
    // Locate the entry block by its slug literal.
    let needle = format!("slug: \"{slug}\"");
    let start = source
        .find(&needle)
        .ok_or_else(|| format!("entry slug={slug:?} not found in catalog source"))?;
    // Block bounds: from the `Entry {` before this slug to the
    // matching `},` after.
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
            // Replace ONLY the three URL fields. Other lines pass
            // through verbatim — including operator-curated comments
            // and the Entry struct's other fields.
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
    fn every_category_in_print_order_has_an_entry() {
        // Sanity gate: if a category becomes empty, drop it from
        // print_order (or add an entry). Avoids printing an empty
        // section header in the table.
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

    #[test]
    fn rewrite_entry_urls_swaps_three_fields() {
        // Synthetic catalog block matching the real format.
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
        let live = crate::catalog_resolvers::ResolvedUrls {
            iso_url: "https://new.example/demo-1.2.4-amd64.iso".to_string(),
            sha256_url: "https://new.example/SHA256SUMS".to_string(),
            sig_url: "https://new.example/SHA256SUMS.gpg".to_string(),
        };
        let result = rewrite_entry_urls(source, "demo", &live).unwrap_or_else(|e| panic!("{e}"));
        assert!(result.contains("https://new.example/demo-1.2.4-amd64.iso"));
        assert!(result.contains("https://new.example/SHA256SUMS"));
        assert!(result.contains("https://new.example/SHA256SUMS.gpg"));
        // Other fields untouched.
        assert!(result.contains("Demo Distro 1.2.3"));
        assert!(result.contains("size_mib: 1000"));
        assert!(result.contains("Demo CA"));
        // Old URLs gone.
        assert!(!result.contains("https://old.example/demo-1.2.3-amd64.iso"));
    }

    #[test]
    fn rewrite_entry_urls_errors_on_unknown_slug() {
        let source = "pub const CATALOG: &[Entry] = &[];";
        let live = crate::catalog_resolvers::ResolvedUrls {
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
        let live = crate::catalog_resolvers::ResolvedUrls {
            iso_url: "https://new/iso".to_string(),
            sha256_url: "https://new/sha".to_string(),
            sig_url: "https://new/sig".to_string(),
        };
        let result = rewrite_entry_urls(source, "demo", &live).unwrap_or_else(|e| panic!("{e}"));
        assert!(result.contains("// important rationale comment"));
        assert!(result.contains("https://new/iso"));
    }
}
