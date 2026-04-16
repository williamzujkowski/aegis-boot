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
            SbStatus::Signed(_) => "\u{2713}", // ✓
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
pub const CATALOG: &[Entry] = &[
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
        slug: "archlinux-current",
        name: "Arch Linux (current monthly)",
        arch: "x86_64",
        size_mib: 1200,
        iso_url: "https://archlinux.org/iso/latest/archlinux-x86_64.iso",
        sha256_url: "https://archlinux.org/iso/latest/sha256sums.txt",
        sig_url: "https://archlinux.org/iso/latest/archlinux-x86_64.iso.sig",
        sb: SbStatus::UnsignedNeedsMok,
        purpose: "Rolling-release recovery shell with current kernels + tooling.",
    },
    Entry {
        slug: "clonezilla-live-stable",
        name: "Clonezilla Live (stable)",
        arch: "x86_64",
        size_mib: 380,
        iso_url: "https://sourceforge.net/projects/clonezilla/files/clonezilla_live_stable/latest/download",
        sha256_url: "https://sourceforge.net/projects/clonezilla/files/clonezilla_live_stable/latest/CHECKSUMS.txt",
        sig_url: "https://sourceforge.net/projects/clonezilla/files/clonezilla_live_stable/latest/CHECKSUMS.txt.gpg",
        sb: SbStatus::Signed("Clonezilla / DRBL"),
        purpose: "Disk imaging + restore. The IR / migration workhorse.",
    },
    Entry {
        slug: "debian-12-netinst",
        name: "Debian 12 (Bookworm) netinst",
        arch: "x86_64",
        size_mib: 700,
        iso_url: "https://cdimage.debian.org/cdimage/release/current/amd64/iso-cd/debian-12.7.0-amd64-netinst.iso",
        sha256_url: "https://cdimage.debian.org/cdimage/release/current/amd64/iso-cd/SHA256SUMS",
        sig_url: "https://cdimage.debian.org/cdimage/release/current/amd64/iso-cd/SHA256SUMS.sign",
        sb: SbStatus::Signed("Debian"),
        purpose: "Stable Debian installer. Network install via the smallest ISO.",
    },
    Entry {
        slug: "fedora-41-workstation",
        name: "Fedora Workstation 41",
        arch: "x86_64",
        size_mib: 2400,
        iso_url: "https://download.fedoraproject.org/pub/fedora/linux/releases/41/Workstation/x86_64/iso/Fedora-Workstation-Live-x86_64-41-1.4.iso",
        sha256_url: "https://download.fedoraproject.org/pub/fedora/linux/releases/41/Workstation/x86_64/iso/Fedora-Workstation-41-1.4-x86_64-CHECKSUM",
        sig_url: "https://getfedora.org/static/fedora.gpg",
        sb: SbStatus::Signed("Fedora"),
        purpose: "Full Fedora live + installer; signed under Fedora's CA.",
    },
    Entry {
        slug: "gparted-live-stable",
        name: "GParted Live (stable)",
        arch: "x86_64",
        size_mib: 480,
        iso_url: "https://sourceforge.net/projects/gparted/files/gparted-live-stable/latest/download",
        sha256_url: "https://sourceforge.net/projects/gparted/files/gparted-live-stable/latest/sha256sum.txt",
        sig_url: "https://sourceforge.net/projects/gparted/files/gparted-live-stable/latest/sha256sum.txt.sig",
        sb: SbStatus::Signed("GParted / Steven Shiau"),
        purpose: "Partition surgery before installs / repairs. Boots fast.",
    },
    Entry {
        slug: "kali-current",
        name: "Kali Linux (current live)",
        arch: "x86_64",
        size_mib: 4400,
        iso_url: "https://cdimage.kali.org/current/kali-linux-current-live-amd64.iso",
        sha256_url: "https://cdimage.kali.org/current/SHA256SUMS",
        sig_url: "https://cdimage.kali.org/current/SHA256SUMS.gpg",
        sb: SbStatus::Signed("Offensive Security"),
        purpose: "Pentest distro with extensive offensive tooling. >4GB → use ext4 data partition.",
    },
    Entry {
        slug: "memtest86plus-7",
        name: "Memtest86+ v7 (free)",
        arch: "x86_64",
        size_mib: 5,
        iso_url: "https://memtest.org/download/v7.20/mt86plus_7.20.iso.zip",
        sha256_url: "https://memtest.org/download/v7.20/SHA256SUMS",
        sig_url: "https://memtest.org/download/v7.20/SHA256SUMS.gpg",
        sb: SbStatus::Signed("Memtest86+"),
        purpose: "RAM diagnostics. Standalone — does not kexec into Linux.",
    },
    Entry {
        slug: "nixos-24.05-minimal",
        name: "NixOS 24.05 Minimal",
        arch: "x86_64",
        size_mib: 950,
        iso_url: "https://channels.nixos.org/nixos-24.05/latest-nixos-minimal-x86_64-linux.iso",
        sha256_url: "https://channels.nixos.org/nixos-24.05/latest-nixos-minimal-x86_64-linux.iso.sha256",
        sig_url: "https://channels.nixos.org/nixos-24.05/latest-nixos-minimal-x86_64-linux.iso.sig",
        sb: SbStatus::UnsignedNeedsMok,
        purpose: "Reproducible-system installer. Minimal image without graphical session.",
    },
    Entry {
        slug: "systemrescue-current",
        name: "SystemRescue (current)",
        arch: "x86_64",
        size_mib: 850,
        iso_url: "https://www.system-rescue.org/releases/latest/iso/",
        sha256_url: "https://www.system-rescue.org/releases/latest/iso/sha256sums.txt",
        sig_url: "https://www.system-rescue.org/releases/latest/iso/sha256sums.txt.asc",
        sb: SbStatus::Signed("SystemRescue"),
        purpose: "All-purpose rescue: filesystems, network, recovery, partitioning.",
    },
    Entry {
        slug: "tails-current",
        name: "Tails (current)",
        arch: "x86_64",
        size_mib: 1400,
        iso_url: "https://download.tails.net/tails/stable/tails-amd64-LATEST/tails-amd64-LATEST.iso",
        sha256_url: "https://tails.net/install/v2/Tails/amd64/stable/latest.json",
        sig_url: "https://tails.net/tails-signing.key",
        sb: SbStatus::Signed("Tails"),
        purpose: "Amnesic privacy live OS. Routes through Tor by default.",
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

/// Entry point for `aegis-boot recommend [slug]`.
pub fn run(args: &[String]) -> ExitCode {
    if args.first().map(String::as_str) == Some("--help")
        || args.first().map(String::as_str) == Some("-h")
    {
        print_help();
        return ExitCode::SUCCESS;
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

fn print_help() {
    println!("aegis-boot recommend — curated ISO catalog");
    println!();
    println!("USAGE:");
    println!("  aegis-boot recommend           List all catalog entries");
    println!("  aegis-boot recommend <slug>    Show download + verify recipe");
    println!();
    println!("EXAMPLES:");
    println!("  aegis-boot recommend");
    println!("  aegis-boot recommend ubuntu-24.04-live-server");
    println!("  aegis-boot recommend alpine-3.20-standard");
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
    println!("  Approx size:  {} ({} MiB)", humanize(e.size_mib), e.size_mib);
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
    fn catalog_has_at_least_ten_entries() {
        // Sanity gate: someone shouldn't accidentally empty out the catalog
        // and have CI still pass. 10 is well below the current size; bump
        // when the catalog grows enough to make this floor meaningful.
        assert!(
            CATALOG.len() >= 10,
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
            assert!(e.iso_url.starts_with("https://"), "{} iso_url not https", e.slug);
            assert!(e.sha256_url.starts_with("https://"), "{} sha256_url not https", e.slug);
            assert!(e.sig_url.starts_with("https://"), "{} sig_url not https", e.slug);
        }
    }

    #[test]
    fn catalog_sizes_are_plausible() {
        for e in CATALOG {
            assert!(e.size_mib >= 1, "{} size_mib too small", e.slug);
            assert!(e.size_mib < 16_000, "{} size_mib > 16 GiB seems wrong", e.slug);
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
        // "memtest86plus-7" should match a unique prefix
        let e = find_entry("memtest").expect("memtest prefix matches uniquely");
        assert!(e.slug.starts_with("memtest"));
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
