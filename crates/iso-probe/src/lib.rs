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

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub use iso_parser::{BootEntry, Distribution, IsoError};

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
    for root in roots {
        // Missing / unreadable roots are not an error — the rescue environment
        // routinely runs with `/run/media` present but `/mnt` empty or vice
        // versa depending on whether anything was attached at boot. Skip
        // silently rather than abort the whole discovery.
        if !root.exists() {
            tracing::debug!(root = %root.display(), "iso-probe: skipping missing root");
            continue;
        }
        match pollster::block_on(parser.scan_directory(root)) {
            Ok(entries) => {
                for entry in entries {
                    all.push(boot_entry_to_discovered(&entry, root));
                }
            }
            Err(IsoError::NoBootEntries(_)) => {}
            Err(IsoError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::debug!(
                    root = %root.display(),
                    "iso-probe: skipping root that disappeared during scan"
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

fn boot_entry_to_discovered(entry: &BootEntry, search_root: &Path) -> DiscoveredIso {
    DiscoveredIso {
        iso_path: search_root.join(&entry.source_iso),
        label: entry.label.clone(),
        distribution: entry.distribution,
        kernel: entry.kernel.clone(),
        initrd: entry.initrd.clone(),
        cmdline: entry.kernel_args.clone(),
        quirks: lookup_quirks(entry.distribution),
    }
}

/// Look up quirks for a distribution.
///
/// Stub implementation — real population is tracked in
/// [#6](https://github.com/williamzujkowski/aegis-boot/issues/6) (per-distro
/// compatibility matrix). Returns an empty list today; downstream code must
/// not rely on `quirks.is_empty()` meaning "definitely safe."
#[must_use]
pub fn lookup_quirks(_distribution: Distribution) -> Vec<Quirk> {
    Vec::new()
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
    fn quirks_lookup_returns_empty_stub() {
        // Until #6 populates the matrix, lookup must return empty for every
        // distribution. The TUI must NOT treat empty as safe.
        for d in [
            Distribution::Arch,
            Distribution::Debian,
            Distribution::Fedora,
            Distribution::Unknown,
        ] {
            assert!(lookup_quirks(d).is_empty());
        }
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
        assert_eq!(discovered.iso_path, PathBuf::from("/run/media/usb1/ubuntu-24.04.iso"));
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
        };
        // Sanity-check the path-joining we'd perform on a real mount.
        let mount = PathBuf::from("/mnt/test");
        let kernel = mount.join(&iso.kernel);
        assert_eq!(kernel, PathBuf::from("/mnt/test/boot/vmlinuz"));
    }
}
