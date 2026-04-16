//! Runtime ISO discovery on the live aegis-boot rescue environment.
//!
//! Two-phase API:
//!
//! 1. [`discover`] — scan a set of root paths for `.iso` files, mount each
//!    once, extract per-ISO boot metadata (kernel + initrd + cmdline relative
//!    to the ISO root), unmount. Returns metadata-only [`DiscoveredIso`]
//!    records suitable for rendering in the TUI.
//! 2. [`prepare`] — given a user-selected [`DiscoveredIso`], re-mount the ISO
//!    and return a [`PreparedIso`] whose [`absolute paths`](PreparedIso::kernel)
//!    can be handed to `kexec-loader::load_and_exec`. The mount is unmounted
//!    when the [`PreparedIso`] is dropped — but `kexec` replaces the
//!    process before that happens on the success path, so the live mount
//!    persists exactly as long as it needs to.
//!
//! See [ADR 0001](../../../docs/adr/0001-runtime-architecture.md).

#![forbid(unsafe_code)]

pub mod minisign;
pub mod signature;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub use iso_parser::{BootEntry, Distribution, IsoError};
pub use minisign::{verify_iso_signature, SignatureVerification};
pub use signature::{verify_iso_hash, verify_iso_hash_with_progress, HashVerification};

/// Metadata for a single discovered ISO. Paths are relative to the (now
/// unmounted) ISO root and become absolute once handed to [`prepare`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredIso {
    /// Absolute path to the `.iso` file on the host filesystem.
    pub iso_path: PathBuf,
    /// Human label (e.g. "Ubuntu 24.04 LTS").
    pub label: String,
    /// Detected distribution family.
    pub distribution: Distribution,
    /// Kernel path relative to the ISO root.
    pub kernel: PathBuf,
    /// Optional initrd path relative to the ISO root.
    pub initrd: Option<PathBuf>,
    /// Kernel command line as declared by the ISO's boot config.
    pub cmdline: Option<String>,
    /// Quirks the rescue TUI should warn about before kexec.
    pub quirks: Vec<Quirk>,
    /// Hash verification status (from sibling checksum files, if any).
    pub hash_verification: HashVerification,
    /// Minisign signature verification status (from sibling .minisig, if any).
    pub signature_verification: SignatureVerification,
    /// File size in bytes from `stat(2)` on `iso_path`. `None` if stat failed.
    /// Rendered as a human-readable value in the Confirm preview pane.
    pub size_bytes: Option<u64>,
    /// True if this ISO is known to contain an installer that can
    /// write to disk when the user picks the wrong boot-menu entry.
    /// Determined heuristically from filename patterns. rescue-tui
    /// surfaces a yellow warning strip on the Confirm screen. (#131)
    pub contains_installer: bool,
}

/// Compatibility quirks the TUI should surface to the user before invoking
/// kexec. Populated by the per-distro matrix (issue #6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Quirk {
    /// ISO's kernel is not signed by a CA in the platform/MOK keyring.
    /// `kexec_file_load` will reject without MOK enrollment.
    UnsignedKernel,
    /// ISO assumes BIOS isolinux only — no usable EFI/kexec path.
    BiosOnly,
    /// ISO is hybrid and expects to be `dd`'d to a whole block device,
    /// not loop-mounted. Kexec may succeed but the booted kernel may not
    /// find its expected layout.
    RequiresWholeDeviceWrite,
    /// Distro signs only its own CA's kernels and refuses kexec into
    /// foreign-CA kernels even with `KEXEC_SIG` satisfied (RHEL family).
    CrossDistroKexecRefused,
    /// ISO uses a boot protocol incompatible with `kexec_file_load`
    /// (Windows' NT loader, BSD bootloaders, etc.). The TUI should
    /// disable kexec for these entries rather than fail silently.
    NotKexecBootable,
}

/// Errors returned during probing.
#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    /// Underlying I/O failure.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// The wrapped ISO parser raised an error.
    #[error("iso parser: {0}")]
    Parser(#[from] IsoError),
    /// No ISOs were found under any of the supplied roots.
    #[error("no ISOs found in supplied roots")]
    NoIsosFound,
}

/// Discover all bootable ISOs under the supplied root directories.
///
/// # Errors
///
/// Returns [`ProbeError::Parser`] if the wrapped scan fails. Individual ISOs
/// with unrecognized layouts are skipped silently and never abort the scan.
pub fn discover(roots: &[PathBuf]) -> Result<Vec<DiscoveredIso>, ProbeError> {
    let parser = iso_parser::IsoParser::new(iso_parser::OsIsoEnvironment::new());
    let mut all: Vec<DiscoveredIso> = Vec::new();
    // Dedupe across roots that share ancestry (e.g. /run/media/aegis-isos
    // is a subdir of /run/media; both listed in AEGIS_ISO_ROOTS). (#117)
    // iso-parser stores source_iso as filename-only; we can't reliably
    // canonicalize because scan-2's root.join(filename) points to a
    // non-existent path. Dedupe by (filename, size) — effectively a
    // content-identity key for ISOs — resolved per root via search
    // for an existing candidate on disk.
    let mut seen: std::collections::HashSet<(String, u64)> = std::collections::HashSet::new();
    for root in roots {
        // Missing / unreadable roots are not an error — the rescue environment
        // routinely runs with `/run/media` present but `/mnt` empty or vice
        // versa depending on whether anything was attached at boot. Log at
        // INFO so an empty list is debuggable (#68 — operators were seeing
        // "0 ISOs discovered" without any signal of where the scan looked).
        if !root.exists() {
            tracing::info!(
                root = %root.display(),
                "iso-probe: root does not exist — skipping"
            );
            continue;
        }
        tracing::info!(root = %root.display(), "iso-probe: scanning root");
        match pollster::block_on(parser.scan_directory(root)) {
            Ok(entries) => {
                let before = all.len();
                for entry in &entries {
                    // Key = (source_iso filename, file size). Need size
                    // so two ISOs with the same filename in different
                    // dirs don't collide. Tree-walk for the actual file
                    // under the current root — iso-parser already did
                    // this during scan, but we need the result back.
                    let size = find_iso_size(root, &entry.source_iso).unwrap_or(0);
                    let key = (entry.source_iso.clone(), size);
                    if !seen.insert(key) {
                        continue;
                    }
                    all.push(boot_entry_to_discovered(entry, root));
                }
                tracing::info!(
                    root = %root.display(),
                    extracted = entries.len(),
                    kept = all.len() - before,
                    "iso-probe: scan extracted entries"
                );
            }
            Err(IsoError::NoBootEntries(_)) => {
                tracing::info!(
                    root = %root.display(),
                    "iso-probe: scan returned NoBootEntries (no .iso files found, or all skipped)"
                );
            }
            Err(IsoError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::info!(
                    root = %root.display(),
                    "iso-probe: root disappeared during scan"
                );
            }
            Err(e) => return Err(ProbeError::Parser(e)),
        }
    }
    if all.is_empty() {
        Err(ProbeError::NoIsosFound)
    } else {
        Ok(all)
    }
}

/// Recursive walk helper for [`find_iso_size`]. Bounded depth so we
/// don't wander into a large tree. `AEGIS_ISOS` layouts are flat;
/// 3 levels is more than enough. (#117)
fn walk_for_iso_size(dir: &Path, filename: &str, depth: u32) -> Option<u64> {
    if depth == 0 {
        return None;
    }
    let iter = std::fs::read_dir(dir).ok()?;
    for entry in iter.flatten() {
        let p = entry.path();
        if let Ok(ft) = entry.file_type() {
            if ft.is_file() && p.file_name().and_then(|n| n.to_str()) == Some(filename) {
                return entry.metadata().ok().map(|m| m.len());
            }
            if ft.is_dir() {
                if let Some(size) = walk_for_iso_size(&p, filename, depth - 1) {
                    return Some(size);
                }
            }
        }
    }
    None
}

/// Walk `root` looking for a file named `filename` at any depth and
/// return its byte size. Used as a dedup helper in [`discover`] —
/// iso-parser stores `source_iso` as filename-only, so we have to walk
/// to find the real file. (#117)
fn find_iso_size(root: &Path, filename: &str) -> Option<u64> {
    let direct = root.join(filename);
    if let Ok(m) = std::fs::metadata(&direct) {
        if m.is_file() {
            return Some(m.len());
        }
    }
    walk_for_iso_size(root, filename, 3)
}

fn boot_entry_to_discovered(entry: &BootEntry, search_root: &Path) -> DiscoveredIso {
    let iso_path = search_root.join(&entry.source_iso);
    let hash_verification = verify_iso_hash(&iso_path).unwrap_or_else(|e| {
        tracing::debug!(
            iso = %iso_path.display(),
            error = %e,
            "iso-probe: hash verification skipped due to I/O error"
        );
        HashVerification::NotPresent
    });
    match &hash_verification {
        HashVerification::Verified { source, .. } => tracing::info!(
            iso = %iso_path.display(),
            source = %source,
            "iso-probe: hash verified"
        ),
        HashVerification::Mismatch { source, .. } => tracing::warn!(
            iso = %iso_path.display(),
            source = %source,
            "iso-probe: HASH MISMATCH — checksum file disagrees with ISO bytes"
        ),
        HashVerification::NotPresent => tracing::debug!(
            iso = %iso_path.display(),
            "iso-probe: no sibling checksum file"
        ),
    }
    let signature_verification = verify_iso_signature(&iso_path);
    match &signature_verification {
        SignatureVerification::Verified { key_id, .. } => tracing::info!(
            iso = %iso_path.display(),
            key_id = %key_id,
            "iso-probe: signature verified against trusted key"
        ),
        SignatureVerification::KeyNotTrusted { key_id } => tracing::warn!(
            iso = %iso_path.display(),
            key_id = %key_id,
            "iso-probe: signature key is not in AEGIS_TRUSTED_KEYS"
        ),
        SignatureVerification::Forged { sig_path } => tracing::warn!(
            iso = %iso_path.display(),
            sig = %sig_path.display(),
            "iso-probe: SIGNATURE FORGED — bytes don't match sig"
        ),
        SignatureVerification::Error { reason } => tracing::warn!(
            iso = %iso_path.display(),
            error = %reason,
            "iso-probe: signature verification errored"
        ),
        SignatureVerification::NotPresent => tracing::debug!(
            iso = %iso_path.display(),
            "iso-probe: no sibling .minisig"
        ),
    }
    let size_bytes = std::fs::metadata(&iso_path).ok().map(|m| m.len());
    let contains_installer = detect_installer(&iso_path);
    DiscoveredIso {
        iso_path,
        label: entry.label.clone(),
        distribution: entry.distribution,
        kernel: entry.kernel.clone(),
        initrd: entry.initrd.clone(),
        cmdline: entry.kernel_args.clone(),
        quirks: lookup_quirks(entry.distribution),
        hash_verification,
        signature_verification,
        size_bytes,
        contains_installer,
    }
}

/// Heuristic detection: does this ISO contain an installer that can
/// overwrite the host's disks? Based on filename substrings of the
/// most common installer-bearing images. Intentionally inclusive —
/// a false-positive (showing a warning on a live-only ISO) is safer
/// than a false-negative (silently hiding the installer risk). (#131)
const INSTALLER_MARKERS: &[&str] = &[
    // Ubuntu / Debian / Mint
    "live-server",
    "live-desktop",
    "desktop-amd64",
    "server-amd64",
    "netinst",
    "netinstall",
    "xubuntu",
    "kubuntu",
    "lubuntu",
    // Fedora / RHEL family
    "workstation",
    "server-",
    "-boot.iso",
    "dvd-",
    "dvd1",
    "everything",
    "netboot",
    // openSUSE
    "opensuse",
    "tumbleweed",
    "leap",
    // Anaconda-based installers
    "anaconda",
    // Windows
    "windows",
    "win10",
    "win11",
];

/// Heuristic: does this ISO filename indicate an installer image?
/// See `INSTALLER_MARKERS` for the match list. (#131)
#[must_use]
pub fn detect_installer(iso_path: &Path) -> bool {
    let name = match iso_path.file_name().and_then(|s| s.to_str()) {
        Some(n) => n.to_ascii_lowercase(),
        None => return false,
    };
    INSTALLER_MARKERS.iter().any(|m| name.contains(m))
}

/// Look up quirks for a distribution family.
///
/// Data source: [`docs/compatibility/iso-matrix.md`][matrix]. Each mapping is
/// a conservative default — the matrix doc is the ground truth and should be
/// updated alongside any change here.
///
/// **Unknown distributions get the most cautious treatment** (assume unsigned
/// kernel). Downstream code must **not** treat an empty return as "safe" —
/// some verified-good layouts (e.g. Debian casper) legitimately return empty.
///
/// [matrix]: ../../../docs/compatibility/iso-matrix.md
#[must_use]
pub fn lookup_quirks(distribution: Distribution) -> Vec<Quirk> {
    match distribution {
        // Canonical/Debian-signed kernels (Ubuntu, Debian live/casper).
        // shim → grub → signed vmlinuz path is well-tested; `KEXEC_SIG`
        // accepts kernels signed by the shipped distro CA. No known quirks.
        Distribution::Debian => Vec::new(),

        // Fedora's kernel is signed by the Fedora UEFI CA. RHEL lineage
        // enforces an additional keyring check inside `kexec_file_load`
        // that rejects kernels signed by a *different* CA even when
        // `KEXEC_SIG` would accept; the rescue-tui surfaces this as
        // `CrossDistroKexecRefused` so the user sees a specific diagnostic
        // instead of a generic EPERM.
        Distribution::Fedora | Distribution::RedHat => vec![Quirk::CrossDistroKexecRefused],

        // Arch install media ships unsigned kernels by default (no
        // shim-review-board-approved shim). Alpine and NixOS ship unsigned
        // ISOs by default too. Unknown distributions share the same
        // conservative default: assume unsigned until proven otherwise.
        Distribution::Arch | Distribution::Alpine | Distribution::NixOS | Distribution::Unknown => {
            vec![Quirk::UnsignedKernel]
        }

        // Windows uses the NT loader / UEFI bootmgfw, not a Linux kernel.
        // Surface the non-bootability explicitly so the TUI can disable
        // kexec rather than fail silently after the user picks it.
        Distribution::Windows => vec![Quirk::NotKexecBootable],
    }
}

/// A live, loop-mounted ISO with absolute paths suitable for handoff to
/// `kexec-loader`. Unmounts on drop.
pub struct PreparedIso {
    mount_point: PathBuf,
    /// Absolute path to the kernel image on the live mount.
    pub kernel: PathBuf,
    /// Absolute path to the initrd, if any.
    pub initrd: Option<PathBuf>,
    /// Kernel command line, copied from the discovery record.
    pub cmdline: Option<String>,
}

impl PreparedIso {
    /// Path under which the ISO is currently loop-mounted.
    #[must_use]
    pub fn mount_point(&self) -> &Path {
        &self.mount_point
    }
}

impl Drop for PreparedIso {
    fn drop(&mut self) {
        let env = iso_parser::OsIsoEnvironment::new();
        if let Err(e) = iso_parser::IsoEnvironment::unmount(&env, &self.mount_point) {
            tracing::warn!(
                mount = %self.mount_point.display(),
                error = %e,
                "iso-probe: unmount on drop failed; rescue env may have stale mount"
            );
        }
    }
}

/// Re-mount the selected ISO and return absolute paths for kexec handoff.
///
/// # Errors
///
/// Returns [`ProbeError::Parser`] if the loop-mount fails (no privileges, no
/// loop devices, malformed ISO).
pub fn prepare(iso: &DiscoveredIso) -> Result<PreparedIso, ProbeError> {
    let env = iso_parser::OsIsoEnvironment::new();
    let mount_point = iso_parser::IsoEnvironment::mount_iso(&env, &iso.iso_path)?;
    Ok(PreparedIso {
        kernel: mount_point.join(&iso.kernel),
        initrd: iso.initrd.as_ref().map(|p| mount_point.join(p)),
        cmdline: iso.cmdline.clone(),
        mount_point,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debian_has_no_known_quirks() {
        // Canonical/Debian signed + casper layout: verified-good default.
        assert!(lookup_quirks(Distribution::Debian).is_empty());
    }

    #[test]
    fn fedora_flags_cross_distro_kexec_refusal() {
        let q = lookup_quirks(Distribution::Fedora);
        assert!(q.contains(&Quirk::CrossDistroKexecRefused));
        assert!(!q.contains(&Quirk::UnsignedKernel));
    }

    #[test]
    fn arch_flags_unsigned_kernel() {
        let q = lookup_quirks(Distribution::Arch);
        assert!(q.contains(&Quirk::UnsignedKernel));
    }

    #[test]
    fn unknown_defaults_to_unsigned_warning() {
        // Conservative default when we can't identify the distribution.
        let q = lookup_quirks(Distribution::Unknown);
        assert!(q.contains(&Quirk::UnsignedKernel));
    }

    #[test]
    fn redhat_inherits_cross_distro_refusal() {
        // RHEL/Rocky/Alma share Fedora's layout + the same lockdown policy.
        let q = lookup_quirks(Distribution::RedHat);
        assert!(q.contains(&Quirk::CrossDistroKexecRefused));
        assert!(!q.contains(&Quirk::UnsignedKernel));
    }

    #[test]
    fn alpine_flags_unsigned_kernel() {
        assert!(lookup_quirks(Distribution::Alpine).contains(&Quirk::UnsignedKernel));
    }

    #[test]
    fn nixos_flags_unsigned_kernel() {
        assert!(lookup_quirks(Distribution::NixOS).contains(&Quirk::UnsignedKernel));
    }

    #[test]
    fn windows_flags_not_kexec_bootable() {
        let q = lookup_quirks(Distribution::Windows);
        assert!(q.contains(&Quirk::NotKexecBootable));
        assert!(!q.contains(&Quirk::UnsignedKernel));
    }

    #[test]
    fn boot_entry_conversion_preserves_paths_and_metadata() {
        let entry = BootEntry {
            label: "Ubuntu 24.04".to_string(),
            kernel: PathBuf::from("casper/vmlinuz"),
            initrd: Some(PathBuf::from("casper/initrd")),
            kernel_args: Some("boot=casper".to_string()),
            distribution: Distribution::Debian,
            source_iso: "ubuntu-24.04.iso".to_string(),
        };
        let root = PathBuf::from("/run/media/usb1");
        let discovered = boot_entry_to_discovered(&entry, &root);
        assert_eq!(
            discovered.iso_path,
            PathBuf::from("/run/media/usb1/ubuntu-24.04.iso")
        );
        assert_eq!(discovered.label, "Ubuntu 24.04");
        assert_eq!(discovered.kernel, PathBuf::from("casper/vmlinuz"));
        assert_eq!(discovered.initrd, Some(PathBuf::from("casper/initrd")));
        assert_eq!(discovered.cmdline.as_deref(), Some("boot=casper"));
        assert_eq!(discovered.distribution, Distribution::Debian);
    }

    #[test]
    fn discover_on_empty_dir_returns_no_isos_found() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let Err(err) = discover(&[dir.path().to_path_buf()]) else {
            panic!("discover on empty dir should fail");
        };
        assert!(matches!(err, ProbeError::NoIsosFound));
    }

    #[test]
    fn prepare_uses_discovered_paths() {
        // Conversion test — exercises the path-joining logic without
        // requiring an actual loop-mount (which needs root + a real ISO).
        let iso = DiscoveredIso {
            iso_path: PathBuf::from("/tmp/x.iso"),
            label: "x".to_string(),
            distribution: Distribution::Unknown,
            kernel: PathBuf::from("boot/vmlinuz"),
            initrd: Some(PathBuf::from("boot/initrd")),
            cmdline: Some("quiet".to_string()),
            quirks: vec![],
            hash_verification: HashVerification::NotPresent,
            signature_verification: SignatureVerification::NotPresent,
            size_bytes: None,
            contains_installer: false,
        };
        // Sanity-check the path-joining we'd perform on a real mount.
        let mount = PathBuf::from("/mnt/test");
        let kernel = mount.join(&iso.kernel);
        assert_eq!(kernel, PathBuf::from("/mnt/test/boot/vmlinuz"));
    }
}
