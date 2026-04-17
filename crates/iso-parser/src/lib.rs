//! ISO Parser - Boot entry discovery from installation media
//!
//! Scans directories for ISO files, detects distribution layouts, and extracts
//! kernel/initrd paths for boot configuration.
//!
//! # Safety
//!
//! `forbid(unsafe_code)` at the crate level — `iso-parser` ships to crates.io
//! per [#51](https://github.com/williamzujkowski/aegis-boot/issues/51) and a
//! library that parses untrusted ISO content has no business calling raw
//! syscalls. The kexec syscall lives in `kexec-loader`, the only crate in the
//! workspace that's exempt from this constraint.
//!
//! # Supported Distributions
//! - **Arch Linux**: `/boot/` contains `vmlinuz` and `initrd.img`
//! - **Debian/Ubuntu**: `/install/` or `/casper/` contains `vmlinuz` and `initrd.gz`
//! - **Fedora**: `/images/pxeboot/` contains `vmlinuz` and `initrd.img`
//!
//! # Usage
//! ```ignore
//! use iso_parser::{IsoParser, OsIsoEnvironment};
//! use std::path::Path;
//!
//! async fn example() {
//!     let parser = IsoParser::new(OsIsoEnvironment::new());
//!     let entries = parser.scan_directory(Path::new("/media/isos")).await?;
//!     for entry in entries {
//!         println!("Found: {} ({:?})", entry.label, entry.distribution);
//!     }
//! }
//! ```

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;
use tracing::{debug, instrument};

#[cfg(test)]
#[path = "detection_tests.rs"]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::missing_panics_doc
)]
mod detection_tests;

/// Errors that can occur during ISO parsing
#[derive(Debug, Error)]
pub enum IsoError {
    /// Underlying I/O failure — path read, file stat, or directory
    /// listing. Wraps [`std::io::Error`] transparently.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Scan completed but no recognized boot entries were found inside
    /// the ISO. The inner string names the ISO path for context.
    #[error("No boot entries found in ISO: {0}")]
    NoBootEntries(String),

    /// `mount` (or the configured `IsoEnvironment`'s `mount_iso`) failed
    /// — inner string is the mounter's stderr or a descriptive message.
    #[error("Mount failed: {0}")]
    MountFailed(String),

    /// Requested path escaped the expected base directory (contains
    /// `..` components or doesn't live under the mount root). Inner
    /// string is the offending path.
    #[error("Path traversal attempt blocked: {0}")]
    PathTraversal(String),
}

/// Represents a discovered boot entry from an ISO
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BootEntry {
    /// Label for the boot menu (e.g., "Arch Linux `x86_64`")
    pub label: String,
    /// Path to kernel (relative to ISO mount point)
    pub kernel: PathBuf,
    /// Path to initrd (relative to ISO mount point)
    pub initrd: Option<PathBuf>,
    /// Kernel command line parameters
    pub kernel_args: Option<String>,
    /// Distribution identifier
    pub distribution: Distribution,
    /// ISO filename (for reference)
    pub source_iso: String,
    /// Full distro name with version, extracted from `/etc/os-release`
    /// (`PRETTY_NAME`) or fallback files on the mounted ISO. Populated
    /// by `scan_directory`; `None` when none of the probe paths exist
    /// (older installers or unfamiliar layouts). Surfaced as the
    /// primary label in downstream UI when present so operators see
    /// "Ubuntu 24.04.2 LTS (Noble Numbat)" instead of just "Ubuntu".
    /// (#119)
    #[serde(default)]
    pub pretty_name: Option<String>,
}

/// Supported distribution families.
///
/// Ordering of detection matters: more specific matches (Alpine's
/// `boot/vmlinuz-lts`, `NixOS`'s `boot/bzImage`, RHEL-family's `images/pxeboot`)
/// must run before the broader ones (Arch's generic `boot/` heuristic).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Distribution {
    /// Arch Linux install media (`arch/boot/x86_64/vmlinuz-linux`).
    Arch,
    /// Debian and Ubuntu live/install media (`casper/`, `install.amd/`, `live/`).
    Debian,
    /// Fedora install media (`images/pxeboot/`).
    Fedora,
    /// RHEL / Rocky / `AlmaLinux` — same `images/pxeboot` layout as Fedora
    /// but a distinct signing CA and stricter lockdown kexec policy.
    RedHat,
    /// Alpine Linux (`boot/vmlinuz-lts`).
    Alpine,
    /// `NixOS` install media (`boot/bzImage`).
    NixOS,
    /// Windows installer media. Recognized by `bootmgr`, `sources/boot.wim`,
    /// or `efi/microsoft/boot/`. **Not kexec-bootable**: Windows uses a
    /// fundamentally different boot protocol (NT loader, not Linux kernel).
    /// Surfaced so the TUI can give a specific diagnostic rather than fail
    /// silently.
    Windows,
    /// Layout not recognized.
    Unknown,
}

impl Distribution {
    /// Detect distribution from a kernel path observed inside an ISO.
    #[must_use]
    pub fn from_paths(kernel_path: &std::path::Path) -> Self {
        let path_str = kernel_path.to_string_lossy().to_lowercase();

        // Specific signals first — RHEL/Rocky/Alma carry distinctive markers in
        // their ISO volume labels and filenames, but at this path-only layer
        // we can't disambiguate from Fedora. Keep them separate variants; the
        // caller can upgrade detection once volume-label sniffing is added.
        if path_str.contains("bootmgr")
            || path_str.contains("sources/boot.wim")
            || path_str.contains("efi/microsoft")
            || path_str.contains("windows")
        {
            Distribution::Windows
        } else if path_str.contains("nixos") || path_str.ends_with("bzimage") {
            Distribution::NixOS
        } else if path_str.contains("alpine")
            // Alpine's kernel filename suffix is the authoritative
            // signal — `vmlinuz-lts` (Standard) and `vmlinuz-virt`
            // (Virt edition). Kept case-insensitive; path_str is
            // already lowercased. (#116)
            || path_str.contains("vmlinuz-lts")
            || path_str.contains("vmlinuz-virt")
            || path_str.contains("initramfs-lts")
            || path_str.contains("initramfs-virt")
        {
            Distribution::Alpine
        } else if path_str.contains("rhel")
            || path_str.contains("rocky")
            || path_str.contains("almalinux")
            || path_str.contains("centos")
        {
            Distribution::RedHat
        } else if path_str.contains("fedora")
            || path_str.contains("images")
            || path_str.contains("pxeboot")
        {
            Distribution::Fedora
        } else if path_str.contains("debian")
            || path_str.contains("ubuntu")
            || path_str.contains("casper")
        {
            Distribution::Debian
        } else if path_str.contains("arch")
            || (path_str.contains("boot")
                && !path_str.contains("efi")
                && !path_str.contains("images"))
        {
            Distribution::Arch
        } else {
            Distribution::Unknown
        }
    }
}

/// Environment abstraction for file system and OS operations
///
/// This trait enables unit testing without actual mounts by providing
/// a mockable interface for filesystem access and process execution.
pub trait IsoEnvironment: Send + Sync {
    /// List files in a directory.
    ///
    /// # Errors
    ///
    /// Returns [`std::io::Error`] on any read failure (missing path,
    /// permission denied, I/O error mid-read).
    fn list_dir(&self, path: &std::path::Path) -> std::io::Result<Vec<std::path::PathBuf>>;

    /// Check if a file exists.
    fn exists(&self, path: &std::path::Path) -> bool;

    /// Read file metadata.
    ///
    /// # Errors
    ///
    /// Returns [`std::io::Error`] when the path can't be stat'd
    /// (missing, permission denied, I/O error).
    fn metadata(&self, path: &std::path::Path) -> std::io::Result<std::fs::Metadata>;

    /// Mount an ISO file and return the mount point.
    ///
    /// # Errors
    ///
    /// Returns [`IsoError::MountFailed`] if the underlying mount
    /// command (or mock handler) returned non-zero, or
    /// [`IsoError::Io`] if a required helper (mkdir, losetup, mount)
    /// couldn't be spawned.
    fn mount_iso(&self, iso_path: &std::path::Path) -> Result<PathBuf, IsoError>;

    /// Unmount a previously mounted ISO.
    ///
    /// # Errors
    ///
    /// Returns [`IsoError::MountFailed`] if `umount` returned non-zero
    /// (busy mount, stale mount point), or [`IsoError::Io`] if the
    /// unmount helper couldn't be spawned.
    fn unmount(&self, mount_point: &std::path::Path) -> Result<(), IsoError>;

    /// Validate that `path` is rooted under `base` and contains no
    /// parent-directory escapes.
    ///
    /// Returns [`IsoError::PathTraversal`] when:
    ///   * any path component is `..` (could escape on normalization), OR
    ///   * `path` does not lie under `base` (absolute paths to elsewhere).
    ///
    /// Symlinks are NOT resolved — callers that mount untrusted media must
    /// constrain symlink-following at the mount layer (e.g. `nosymfollow`),
    /// not rely on this check.
    ///
    /// Previous implementation silently returned `Ok(path)` when
    /// `strip_prefix(base)` failed, meaning paths outside `base` were
    /// accepted. Fixed in #56.
    ///
    /// # Errors
    ///
    /// Returns [`IsoError::PathTraversal`] on either of the two
    /// traversal conditions above.
    fn validate_path(
        &self,
        base: &std::path::Path,
        path: &std::path::Path,
    ) -> Result<PathBuf, IsoError> {
        if path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(IsoError::PathTraversal(path.display().to_string()));
        }
        if !path.starts_with(base) {
            return Err(IsoError::PathTraversal(path.display().to_string()));
        }
        Ok(path.to_path_buf())
    }
}

/// OS-specific implementation of `IsoEnvironment`
pub struct OsIsoEnvironment {
    mount_base: PathBuf,
}

impl OsIsoEnvironment {
    /// Construct a default `OsIsoEnvironment` with mount points under
    /// `/tmp/iso-parser-mounts`. Callers that need a different base
    /// path should construct the struct directly.
    #[must_use]
    pub fn new() -> Self {
        Self {
            mount_base: PathBuf::from("/tmp/iso-parser-mounts"),
        }
    }

    /// Find a free loop device and attach `iso_path` to it. Tries
    /// util-linux semantics (`losetup -f --show -r`) first, then falls
    /// back to busybox semantics (scan `/dev/loop*` manually and attach
    /// via `losetup <dev> <iso>`). Returns the allocated device path on
    /// success.
    fn allocate_loop_device(iso_path: &std::path::Path) -> Option<String> {
        use std::process::Command;

        // Attempt A: util-linux `-f --show -r`.
        match Command::new("losetup")
            .args(["-f", "--show", "-r", &iso_path.to_string_lossy()])
            .output()
        {
            Ok(out) if out.status.success() => {
                let dev = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !dev.is_empty() && dev.starts_with("/dev/") {
                    return Some(dev);
                }
                // Success exit but stdout didn't name a loop device —
                // surface so operators see why "no ISOs found" when
                // losetup is present. (#138)
                tracing::warn!(
                    iso = %iso_path.display(),
                    stdout = %String::from_utf8_lossy(&out.stdout),
                    "iso-parser: util-linux losetup succeeded but returned no /dev/loop* device"
                );
            }
            Ok(out) => {
                tracing::warn!(
                    iso = %iso_path.display(),
                    exit = ?out.status.code(),
                    stderr = %String::from_utf8_lossy(&out.stderr),
                    "iso-parser: util-linux losetup -f --show failed; falling back to busybox scan"
                );
            }
            Err(e) => {
                tracing::warn!(
                    iso = %iso_path.display(),
                    error = %e,
                    "iso-parser: losetup exec failed (not on PATH?); falling back to busybox scan"
                );
            }
        }

        // Attempt B: busybox fallback. Find a free loop device manually
        // (one that's not currently bound — busybox `losetup LOOPDEV`
        // without args prints its binding or errors).
        for n in 0..16 {
            let dev = format!("/dev/loop{n}");
            if !std::path::Path::new(&dev).exists() {
                continue;
            }
            // Query — if it returns non-zero, device is free.
            let query = match Command::new("losetup").arg(&dev).output() {
                Ok(q) => q,
                Err(e) => {
                    tracing::warn!(
                        dev = %dev,
                        error = %e,
                        "iso-parser: losetup query exec failed; skipping device"
                    );
                    continue;
                }
            };
            if query.status.success() {
                continue; // already bound
            }
            // Try to attach.
            match Command::new("losetup")
                .args(["-r", &dev, &iso_path.to_string_lossy()])
                .output()
            {
                Ok(attach) if attach.status.success() => return Some(dev),
                Ok(attach) => {
                    tracing::warn!(
                        dev = %dev,
                        iso = %iso_path.display(),
                        exit = ?attach.status.code(),
                        stderr = %String::from_utf8_lossy(&attach.stderr),
                        "iso-parser: losetup attach failed; trying next device"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        dev = %dev,
                        iso = %iso_path.display(),
                        error = %e,
                        "iso-parser: losetup attach exec failed; giving up"
                    );
                    return None;
                }
            }
        }
        tracing::warn!(
            iso = %iso_path.display(),
            "iso-parser: exhausted /dev/loop0..15 without a free device; cannot mount ISO"
        );
        None
    }
}

impl Default for OsIsoEnvironment {
    fn default() -> Self {
        Self::new()
    }
}

impl IsoEnvironment for OsIsoEnvironment {
    fn list_dir(&self, path: &std::path::Path) -> std::io::Result<Vec<PathBuf>> {
        let mut entries = std::fs::read_dir(path)?
            .map(|e| e.map(|entry| entry.path()))
            .collect::<Result<Vec<_>, _>>()?;
        entries.sort();
        Ok(entries)
    }

    fn exists(&self, path: &std::path::Path) -> bool {
        path.exists()
    }

    fn metadata(&self, path: &std::path::Path) -> std::io::Result<std::fs::Metadata> {
        std::fs::metadata(path)
    }

    fn mount_iso(&self, iso_path: &std::path::Path) -> Result<PathBuf, IsoError> {
        use std::process::Command;

        // Generate unique mount point
        let iso_name = iso_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("iso");

        let mount_point = self.mount_base.join(format!("mount_{iso_name}"));
        std::fs::create_dir_all(&mount_point)?;

        // Attempt 1: `mount -o loop,ro`. Works with util-linux; may not
        // work with some busybox builds where the `loop` option is a
        // no-op (it mounts the file as if it were a raw block device,
        // which then fails). Try it first because it's one syscall on
        // util-linux-based systems.
        let output = Command::new("mount")
            .args([
                "-o",
                "loop,ro",
                "-t",
                "iso9660",
                &iso_path.to_string_lossy(),
                &mount_point.to_string_lossy(),
            ])
            .output();

        // If that fails AND we have `losetup` available, fall through to
        // the explicit loop-setup path below. Verify by checking if the
        // mount point now contains anything (mount silently succeeds with
        // nothing mounted under certain busybox builds — test by listing).
        let loop_attempt_ok = match &output {
            Ok(out) if out.status.success() => {
                // Verify the mount actually took by checking for directory
                // entries. An empty dir after a "successful" mount means
                // busybox loop-mode didn't work.
                std::fs::read_dir(&mount_point)
                    .ok()
                    .and_then(|mut entries| entries.next())
                    .is_some()
            }
            _ => false,
        };

        if !loop_attempt_ok {
            // Attempt 2: explicit losetup + mount. Handles both
            // util-linux (`losetup -f --show`) and busybox (`losetup -f`
            // prints the allocated device on stdout as a side effect;
            // `--show` is a util-linux long option that busybox doesn't
            // accept). Try util-linux form first; fall back to querying
            // /dev/loop* after a bare `losetup -f` attach.
            let loop_dev = Self::allocate_loop_device(iso_path);
            if let Some(loop_dev) = loop_dev {
                let mount_out = Command::new("mount")
                    .args([
                        "-r",
                        "-t",
                        "iso9660",
                        &loop_dev,
                        &mount_point.to_string_lossy(),
                    ])
                    .output();
                if let Ok(mo) = mount_out {
                    if mo.status.success() {
                        debug!(
                            "Mounted {} via losetup {} -> {:?}",
                            iso_path.display(),
                            loop_dev,
                            mount_point
                        );
                        return Ok(mount_point);
                    }
                }
                // losetup succeeded but mount failed — detach.
                let _ = Command::new("losetup").args(["-d", &loop_dev]).output();
            }
        }

        match output {
            Ok(out) if out.status.success() => {
                debug!("Mounted {} to {:?}", iso_path.display(), mount_point);
                Ok(mount_point)
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                // Try fallback with fuseiso
                let fuse_output = Command::new("fuseiso")
                    .arg(iso_path.to_string_lossy().as_ref())
                    .arg(mount_point.to_string_lossy().as_ref())
                    .output();

                match fuse_output {
                    Ok(fuse_out) if fuse_out.status.success() => {
                        debug!("Mounted {} via fuseiso", iso_path.display());
                        Ok(mount_point)
                    }
                    _ => {
                        // Cleanup mount point on failure
                        let _ = std::fs::remove_dir(&mount_point);
                        Err(IsoError::MountFailed(stderr.to_string()))
                    }
                }
            }
            Err(e) => Err(IsoError::Io(e)),
        }
    }

    fn unmount(&self, mount_point: &std::path::Path) -> Result<(), IsoError> {
        use std::process::Command;

        // Try umount first, then fusermount
        let umount_result = Command::new("umount").arg(mount_point).output();

        match umount_result {
            Ok(out) if out.status.success() => {
                let _ = std::fs::remove_dir(mount_point);
                Ok(())
            }
            _ => {
                // Try fusermount as fallback
                let fusermount = Command::new("fusermount")
                    .arg("-u")
                    .arg(mount_point)
                    .output();
                match fusermount {
                    Ok(out) if out.status.success() => {
                        let _ = std::fs::remove_dir(mount_point);
                        Ok(())
                    }
                    _ => Err(IsoError::MountFailed(format!(
                        "Failed to unmount {}",
                        mount_point.display()
                    ))),
                }
            }
        }
    }
}

/// ISO Parser - main entry point for boot discovery
///
/// Generic over environment to allow testing without actual filesystem/mounts.
pub struct IsoParser<E: IsoEnvironment> {
    env: E,
}

impl<E: IsoEnvironment> IsoParser<E> {
    /// Construct a parser bound to the given [`IsoEnvironment`].
    /// Typically [`OsIsoEnvironment`] in production; a mock in tests.
    pub fn new(env: E) -> Self {
        Self { env }
    }

    /// Scan a directory for ISO files and extract boot entries
    /// Scan a directory for `.iso` files, mount + parse each one, and
    /// return the collected [`BootEntry`] records.
    ///
    /// The `async` signature is retained for backwards source-compat
    /// with callers that `.await` it; the function itself performs no
    /// async work today.
    ///
    /// # Errors
    ///
    /// Returns [`IsoError::PathTraversal`] if `path` escapes
    /// `/` (degenerate), or [`IsoError::Io`] on a filesystem read
    /// failure during the ISO-file discovery walk. Per-ISO parse /
    /// mount failures are silently skipped and logged at `debug`; the
    /// overall scan succeeds as long as at least the walk works.
    #[instrument(skip(self))]
    #[allow(clippy::unused_async)]
    pub async fn scan_directory(&self, path: &std::path::Path) -> Result<Vec<BootEntry>, IsoError> {
        let mut entries = Vec::new();

        // Validate base path
        let validated_path = self.env.validate_path(std::path::Path::new("/"), path)?;

        debug!("Scanning directory: {:?}", validated_path);

        let iso_files = self.find_iso_files(&validated_path)?;
        let attempted = iso_files.len();
        let mut skipped = 0usize;

        for iso_path in iso_files {
            debug!("Processing ISO: {:?}", iso_path);

            match self.process_iso(&iso_path).await {
                Ok(mut iso_entries) => entries.append(&mut iso_entries),
                Err(e) => {
                    skipped += 1;
                    // Upgraded from debug → warn so silent-skip failures
                    // surface on the serial console without operators
                    // having to enable debug tracing. (#68)
                    tracing::warn!(
                        iso = %iso_path.display(),
                        error = %e,
                        "iso-parser: skipped ISO (mount/parse failed)"
                    );
                }
            }
        }

        tracing::info!(
            root = %validated_path.display(),
            found_isos = attempted,
            extracted_entries = entries.len(),
            skipped_isos = skipped,
            "iso-parser: scan summary"
        );

        if entries.is_empty() {
            return Err(IsoError::NoBootEntries(
                validated_path.to_string_lossy().to_string(),
            ));
        }

        Ok(entries)
    }

    /// Find all ISO files in a directory recursively
    fn find_iso_files(&self, path: &std::path::Path) -> Result<Vec<PathBuf>, IsoError> {
        let mut isos = Vec::new();

        for entry in self.env.list_dir(path)? {
            let entry_path = &entry;

            // Recurse into subdirectories
            if entry.is_dir() {
                // Skip certain directories
                let name = entry.file_name().and_then(|n| n.to_str()).unwrap_or("");

                if !name.starts_with('.') && name != "proc" && name != "sys" && name != "dev" {
                    if let Ok(mut sub_isos) = self.find_iso_files(entry_path) {
                        isos.append(&mut sub_isos);
                    }
                }
            } else if let Some(ext) = entry.extension().and_then(|s| s.to_str()) {
                if ext.eq_ignore_ascii_case("iso") {
                    isos.push(entry.clone());
                }
            }
        }

        Ok(isos)
    }

    /// Process a single ISO: mount, extract boot entries, unmount
    async fn process_iso(&self, iso_path: &Path) -> Result<Vec<BootEntry>, IsoError> {
        let mount_point = self.env.mount_iso(iso_path)?;

        let result = self.extract_boot_entries(&mount_point, iso_path).await;

        // Always attempt unmount
        let _ = self.env.unmount(&mount_point);

        result
    }

    /// Extract boot entries from a mounted ISO.
    #[allow(clippy::unused_async)]
    async fn extract_boot_entries(
        &self,
        mount_point: &Path,
        source_iso: &Path,
    ) -> Result<Vec<BootEntry>, IsoError> {
        let mut entries = Vec::new();

        // Try each distribution pattern
        entries.extend(self.try_arch_layout(mount_point, source_iso)?);
        entries.extend(self.try_debian_layout(mount_point, source_iso)?);
        entries.extend(self.try_fedora_layout(mount_point, source_iso)?);

        // Populate pretty_name from the mounted ISO's release files
        // before the caller unmounts. Best-effort — if none of the
        // known paths resolve, the field stays None and downstream UI
        // falls back to the distribution-family label. (#119)
        let pretty = read_pretty_name(mount_point);
        if pretty.is_some() {
            for entry in &mut entries {
                entry.pretty_name.clone_from(&pretty);
            }
        }

        Ok(entries)
    }

    /// Try Arch Linux layout: /boot/{vmlinuz,initrd.img}
    fn try_arch_layout(
        &self,
        mount_point: &Path,
        source_iso: &Path,
    ) -> Result<Vec<BootEntry>, IsoError> {
        let boot_dir = mount_point.join("boot");

        if !self.env.exists(&boot_dir) {
            return Ok(Vec::new());
        }

        let mut entries = Vec::new();

        // Find kernel files (vmlinuz*)
        for entry in self.env.list_dir(&boot_dir)? {
            let name = entry.file_name().and_then(|n| n.to_str()).unwrap_or("");

            if name.starts_with("vmlinuz") {
                let kernel = entry.clone();
                let mut initrd = boot_dir.join(format!(
                    "initrd.img{}",
                    name.strip_prefix("vmlinuz").unwrap_or("")
                ));

                // Try common initrd names
                if !self.env.exists(&initrd) {
                    initrd = boot_dir.join("initrd.img");
                }
                if !self.env.exists(&initrd) {
                    initrd = boot_dir.join(format!(
                        "initrd{}",
                        name.strip_prefix("vmlinuz").unwrap_or("")
                    ));
                }

                let has_initrd = self.env.exists(&initrd);

                // Classify from the actual kernel filename — `boot/vmlinuz-lts`
                // and `boot/vmlinuz-virt` are Alpine, not Arch, etc. This
                // layout matches multiple distros that share the
                // `/boot/vmlinuz*` convention; use the path classifier
                // rather than a hardcoded `Distribution::Arch`. (#116)
                let rel_kernel = kernel
                    .strip_prefix(mount_point)
                    .map(std::path::Path::to_path_buf)
                    .map_err(|_| {
                        IsoError::Io(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "Kernel path escape",
                        ))
                    })?;
                let distribution = Distribution::from_paths(&rel_kernel);
                let label = match distribution {
                    Distribution::Alpine => format!(
                        "Alpine {}",
                        name.strip_prefix("vmlinuz-").unwrap_or("").trim()
                    ),
                    Distribution::Arch => format!(
                        "Arch Linux {}",
                        name.strip_prefix("vmlinuz").unwrap_or("").trim()
                    ),
                    _ => format!(
                        "Linux {}",
                        name.strip_prefix("vmlinuz").unwrap_or("").trim()
                    ),
                };
                // Kernel args: only set for actual Arch; leave empty for
                // Alpine/unknown so the ISO's own boot config wins.
                let kernel_args = if distribution == Distribution::Arch {
                    Some(
                        "archisobasedir=arch archiso_http_server=https://mirror.archlinux.org"
                            .to_string(),
                    )
                } else {
                    None
                };

                entries.push(BootEntry {
                    label,
                    kernel: rel_kernel,
                    initrd: if has_initrd { Some(initrd) } else { None },
                    kernel_args,
                    distribution,
                    source_iso: source_iso
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("unknown")
                        .to_string(),
                    pretty_name: None,
                });
            }
        }

        Ok(entries)
    }

    /// Try Debian/Ubuntu layout: /install/vmlinuz, /casper/initrd.lz
    fn try_debian_layout(
        &self,
        mount_point: &Path,
        source_iso: &Path,
    ) -> Result<Vec<BootEntry>, IsoError> {
        let mut entries = Vec::new();

        // Debian-family ISOs have one or more of: /install (debian-
        // installer), /casper (ubuntu live), /.disk/info (both), or
        // /pool (package pool). Gate on at least one of those being
        // present — without the gate, try_debian_layout also matches
        // Alpine's /boot/vmlinuz-lts and produces spurious
        // "Debian/Ubuntu" entries. (#122)
        let debian_markers = [
            mount_point.join("install"),
            mount_point.join("casper"),
            mount_point.join(".disk"),
            mount_point.join("pool"),
            mount_point.join("dists"),
        ];
        if !debian_markers.iter().any(|p| self.env.exists(p)) {
            return Ok(entries);
        }

        // Try multiple potential locations
        let search_paths = [
            mount_point.join("install"),
            mount_point.join("casper"),
            mount_point.join("boot"),
        ];

        for search_dir in &search_paths {
            if !self.env.exists(search_dir) {
                continue;
            }

            // Find vmlinuz
            for entry in self.env.list_dir(search_dir)? {
                let name = entry.file_name().and_then(|n| n.to_str()).unwrap_or("");

                if name.starts_with("vmlinuz") {
                    let kernel = entry.clone();

                    // Look for initrd in same directory or common locations
                    let initrd_names = ["initrd.lz", "initrd.gz", "initrd.img", "initrd"];
                    let mut found_initrd = None;

                    for initrd_name in initrd_names {
                        let initrd_path = search_dir.join(initrd_name);
                        if self.env.exists(&initrd_path) {
                            found_initrd = Some(initrd_path);
                            break;
                        }
                    }

                    // Also check casper filesystem.squashfs for live boot
                    let kernel_args = if search_dir == &mount_point.join("casper") {
                        Some(
                            "boot=casper locale=en_US.UTF-8 keyboard-configuration/layoutcode=us"
                                .to_string(),
                        )
                    } else {
                        None
                    };

                    // Both casper and non-casper paths result in Debian family
                    entries.push(BootEntry {
                        label: format!(
                            "Debian/Ubuntu {}",
                            name.strip_prefix("vmlinuz").unwrap_or("").trim()
                        ),
                        kernel: kernel
                            .strip_prefix(mount_point)
                            .map(std::path::Path::to_path_buf)
                            .map_err(|_| {
                                IsoError::Io(std::io::Error::new(
                                    std::io::ErrorKind::InvalidData,
                                    "Kernel path escape",
                                ))
                            })?,
                        initrd: found_initrd
                            .map(|p| {
                                p.strip_prefix(mount_point)
                                    .map(std::path::Path::to_path_buf)
                                    .map_err(|_| {
                                        IsoError::Io(std::io::Error::new(
                                            std::io::ErrorKind::InvalidData,
                                            "Initrd path escape",
                                        ))
                                    })
                            })
                            .transpose()?,
                        kernel_args,
                        distribution: Distribution::Debian,
                        source_iso: source_iso
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("unknown")
                            .to_string(),
                        pretty_name: None,
                    });
                }
            }
        }

        Ok(entries)
    }

    /// Try Fedora layout: /images/pxeboot/vmlinuz, /images/pxeboot/initrd.img
    fn try_fedora_layout(
        &self,
        mount_point: &Path,
        source_iso: &Path,
    ) -> Result<Vec<BootEntry>, IsoError> {
        let images_dir = mount_point.join("images").join("pxeboot");

        if !self.env.exists(&images_dir) {
            // Try alternate: /isolinux/ (common Fedora live media)
            let alt_dir = mount_point.join("isolinux");
            if !self.env.exists(&alt_dir) {
                return Ok(Vec::new());
            }
            return self.process_fedora_isolinux(&alt_dir, mount_point, source_iso);
        }

        let mut entries = Vec::new();

        // Find kernel
        for entry in self.env.list_dir(&images_dir)? {
            let name = entry.file_name().and_then(|n| n.to_str()).unwrap_or("");

            if name.starts_with("vmlinuz") {
                let kernel = entry.clone();

                // Find matching initrd
                let version = name.strip_prefix("vmlinuz").unwrap_or("");
                let initrd_names = [
                    format!("initrd{version}.img"),
                    "initrd.img".to_string(),
                    format!("initrd{}.img", version.trim_end_matches('-')),
                ];

                let mut found_initrd = None;
                for initrd_name in &initrd_names {
                    let initrd_path = images_dir.join(initrd_name);
                    if self.env.exists(&initrd_path) {
                        found_initrd = Some(initrd_path);
                        break;
                    }
                }

                entries.push(BootEntry {
                    label: format!("Fedora {}", version.trim()),
                    kernel: kernel
                        .strip_prefix(mount_point)
                        .map(std::path::Path::to_path_buf)
                        .map_err(|_| {
                            IsoError::Io(std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                "Kernel path escape",
                            ))
                        })?,
                    initrd: found_initrd
                        .map(|p| {
                            p.strip_prefix(mount_point)
                                .map(std::path::Path::to_path_buf)
                                .map_err(|_| {
                                    IsoError::Io(std::io::Error::new(
                                        std::io::ErrorKind::InvalidData,
                                        "Initrd path escape",
                                    ))
                                })
                        })
                        .transpose()?,
                    kernel_args: Some("inst.stage2=hd:LABEL=Fedora-39-x86_64".to_string()),
                    distribution: Distribution::Fedora,
                    source_iso: source_iso
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("unknown")
                        .to_string(),
                    pretty_name: None,
                });
            }
        }

        Ok(entries)
    }

    fn process_fedora_isolinux(
        &self,
        isolinux_dir: &Path,
        mount_point: &Path,
        source_iso: &Path,
    ) -> Result<Vec<BootEntry>, IsoError> {
        let mut entries = Vec::new();

        for entry in self.env.list_dir(isolinux_dir)? {
            let name = entry.file_name().and_then(|n| n.to_str()).unwrap_or("");

            if name.starts_with("vmlinuz") {
                let kernel = entry.clone();

                // Look for initrd in images directory
                let images_dir = mount_point.join("images");
                let initrd_path = images_dir.join("initrd.img");

                entries.push(BootEntry {
                    label: format!(
                        "Fedora (isolinux) {}",
                        name.strip_prefix("vmlinuz").unwrap_or("").trim()
                    ),
                    kernel: kernel
                        .strip_prefix(mount_point)
                        .map(std::path::Path::to_path_buf)
                        .map_err(|_| {
                            IsoError::Io(std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                "Kernel path escape",
                            ))
                        })?,
                    initrd: if self.env.exists(&initrd_path) {
                        Some(
                            initrd_path
                                .strip_prefix(mount_point)
                                .map(std::path::Path::to_path_buf)
                                .map_err(|_| {
                                    IsoError::Io(std::io::Error::new(
                                        std::io::ErrorKind::InvalidData,
                                        "Initrd path escape",
                                    ))
                                })?,
                        )
                    } else {
                        None
                    },
                    kernel_args: Some("inst.stage2=hd:LABEL=Fedora".to_string()),
                    distribution: Distribution::Fedora,
                    source_iso: source_iso
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("unknown")
                        .to_string(),
                    pretty_name: None,
                });
            }
        }

        Ok(entries)
    }
}

/// Best-effort "friendly" distro name for a mounted ISO.
///
/// Reads the first file in this priority order and returns the first
/// useful value found:
///
/// 1. `/etc/os-release` `PRETTY_NAME` — systemd convention; all
///    modern distros ship this (Ubuntu, Fedora, Rocky, Alma, Debian 12+,
///    openSUSE, Arch, `NixOS` 22+, Alpine 3.17+).
/// 2. `/lib/os-release` `PRETTY_NAME` — symlink target on some distros;
///    handled independently in case the `/etc` copy is missing.
/// 3. `/.disk/info` — single line of free text, Ubuntu + Debian live/install
///    media tradition since circa Debian 6. Form: "Ubuntu 24.04.2 LTS ...".
/// 4. `/etc/alpine-release` — single version string (e.g. "3.20.3") on
///    Alpine. We prepend "Alpine " so the returned value is self-contained.
///
/// Returns `None` if none of the paths exist or all attempts produce an
/// empty string. This is advisory — every caller must tolerate `None`
/// and fall back to the `Distribution`-family label.
#[must_use]
pub fn read_pretty_name(mount_point: &Path) -> Option<String> {
    for rel in ["etc/os-release", "lib/os-release", "usr/lib/os-release"] {
        if let Some(name) = read_os_release(&mount_point.join(rel)) {
            return Some(name);
        }
    }
    if let Some(first_line) = read_first_nonempty_line(&mount_point.join(".disk/info")) {
        return Some(first_line);
    }
    if let Some(version) = read_first_nonempty_line(&mount_point.join("etc/alpine-release")) {
        return Some(format!("Alpine Linux {version}"));
    }
    None
}

/// Parse a systemd-style `os-release` file for the value of `PRETTY_NAME`.
/// Strips surrounding double quotes if present. Returns `None` on any
/// read error or if the key is missing / empty.
fn read_os_release(path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    parse_os_release_pretty_name(&content)
}

/// Pure-string version of the `os-release` parser — split out so we can
/// unit-test without touching the filesystem.
#[must_use]
pub(crate) fn parse_os_release_pretty_name(content: &str) -> Option<String> {
    for line in content.lines() {
        let Some(rest) = line.strip_prefix("PRETTY_NAME=") else {
            continue;
        };
        // Strip surrounding " or ' (systemd spec allows either, and we
        // want to be forgiving of wild-in-the-field variants).
        let trimmed = rest
            .trim()
            .trim_matches(|c| c == '"' || c == '\'')
            .to_string();
        if trimmed.is_empty() {
            return None;
        }
        return Some(trimmed);
    }
    None
}

/// Read the first non-empty trimmed line of a file. Used for free-text
/// release files (`/.disk/info`, `/etc/alpine-release`) that don't
/// follow the `KEY=VALUE` shape.
fn read_first_nonempty_line(path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    for line in content.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    None
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::missing_panics_doc,
    clippy::match_same_arms
)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// Mock environment for testing without actual filesystem
    struct MockIsoEnvironment {
        files: HashMap<PathBuf, MockEntry>,
        mount_points: Mutex<Vec<PathBuf>>,
    }

    #[derive(Debug, Clone)]
    enum MockEntry {
        File,
        Directory(Vec<PathBuf>),
    }

    impl MockIsoEnvironment {
        fn new() -> Self {
            Self {
                files: HashMap::new(),
                mount_points: Mutex::new(Vec::new()),
            }
        }

        fn with_iso(distribution: Distribution) -> Self {
            let mut env = Self::new();

            let mount_base = PathBuf::from("/mock_mount");

            match distribution {
                Distribution::Arch => {
                    // Arch: /boot/vmlinuz, /boot/initrd.img
                    env.files.insert(
                        mount_base.join("boot"),
                        MockEntry::Directory(vec![
                            mount_base.join("boot/vmlinuz"),
                            mount_base.join("boot/initrd.img"),
                        ]),
                    );
                    env.files
                        .insert(mount_base.join("boot/vmlinuz"), MockEntry::File);
                    env.files
                        .insert(mount_base.join("boot/initrd.img"), MockEntry::File);
                }
                Distribution::Debian => {
                    // Debian: /install/vmlinuz, /casper/initrd.lz
                    env.files.insert(
                        mount_base.join("install"),
                        MockEntry::Directory(vec![mount_base.join("install/vmlinuz")]),
                    );
                    env.files
                        .insert(mount_base.join("install/vmlinuz"), MockEntry::File);
                    env.files.insert(
                        mount_base.join("casper"),
                        MockEntry::Directory(vec![
                            mount_base.join("casper/initrd.lz"),
                            mount_base.join("casper/filesystem.squashfs"),
                        ]),
                    );
                    env.files
                        .insert(mount_base.join("casper/initrd.lz"), MockEntry::File);
                    env.files.insert(
                        mount_base.join("casper/filesystem.squashfs"),
                        MockEntry::File,
                    );
                }
                Distribution::Fedora => {
                    // Fedora: /images/pxeboot/vmlinuz, /images/pxeboot/initrd.img
                    env.files.insert(
                        mount_base.join("images"),
                        MockEntry::Directory(vec![mount_base.join("images/pxeboot")]),
                    );
                    env.files.insert(
                        mount_base.join("images/pxeboot"),
                        MockEntry::Directory(vec![
                            mount_base.join("images/pxeboot/vmlinuz"),
                            mount_base.join("images/pxeboot/initrd.img"),
                        ]),
                    );
                    env.files
                        .insert(mount_base.join("images/pxeboot/vmlinuz"), MockEntry::File);
                    env.files.insert(
                        mount_base.join("images/pxeboot/initrd.img"),
                        MockEntry::File,
                    );
                }
                // New variants reuse existing mock fixtures by analogue
                // (Alpine + NixOS behave like Arch at the path layer; RedHat
                // like Fedora). The scan_directory tests only care about the
                // 3 original categories, so nothing new to stage here.
                Distribution::RedHat
                | Distribution::Alpine
                | Distribution::NixOS
                | Distribution::Windows => {}
                Distribution::Unknown => {}
            }

            // Add ISO file in parent directory
            env.files.insert(
                PathBuf::from("/isos"),
                MockEntry::Directory(vec![PathBuf::from("/isos/test.iso")]),
            );
            env.files
                .insert(PathBuf::from("/isos/test.iso"), MockEntry::File);

            env
        }
    }

    impl IsoEnvironment for MockIsoEnvironment {
        fn list_dir(&self, path: &std::path::Path) -> std::io::Result<Vec<PathBuf>> {
            match self.files.get(path) {
                Some(MockEntry::Directory(entries)) => Ok(entries.clone()),
                Some(MockEntry::File) => Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "Not a directory",
                )),
                None => Ok(Vec::new()), // Empty for non-existent
            }
        }

        fn exists(&self, path: &std::path::Path) -> bool {
            self.files.contains_key(path)
        }

        fn metadata(&self, _path: &std::path::Path) -> std::io::Result<std::fs::Metadata> {
            // Fail closed: the previous implementation returned the real
            // metadata of `std::env::temp_dir()` for any path that existed
            // in the mock — which silently made size/mtime assertions pass
            // on fake data (they'd read /tmp's values, not the mock's).
            //
            // Since no caller in the workspace uses IsoEnvironment::metadata
            // today (the trait method is currently unused, per #138 audit),
            // and std::fs::Metadata has no public constructor, there is no
            // safe way to return a synthesized value from pure mock data.
            //
            // If a future caller needs this method, the correct fix is to
            // add real size/mtime fields to MockEntry and return them via a
            // wrapper type — not to paper over the hazard with /tmp values.
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "MockIsoEnvironment::metadata is not implemented — see #138 for the design note",
            ))
        }

        fn mount_iso(&self, iso_path: &std::path::Path) -> Result<PathBuf, IsoError> {
            let mount_point = PathBuf::from(format!(
                "/mock_mount/{}",
                iso_path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("iso")
            ));

            // Poison-safe lock: if a prior test panicked while holding the
            // mutex, `.lock()` returns `Err(PoisonError)`. `into_inner()`
            // recovers the guarded value so we don't cascade-fail every
            // subsequent test that happens to hit this path. Mock state is
            // append-or-trim only, so partial updates from a poisoned
            // critical section are safe to observe.
            self.mount_points
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(mount_point.clone());
            Ok(mount_point)
        }

        fn unmount(&self, mount_point: &std::path::Path) -> Result<(), IsoError> {
            let mut points = self
                .mount_points
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            points.retain(|p| p != mount_point);
            Ok(())
        }
    }

    #[test]
    fn test_path_traversal_blocked() {
        let env = MockIsoEnvironment::new();
        let result = env.validate_path(
            PathBuf::from("/safe").as_path(),
            PathBuf::from("/safe/../../../etc/passwd").as_path(),
        );

        assert!(result.is_err());
        match result {
            Err(IsoError::PathTraversal(_)) => {}
            _ => panic!("Expected PathTraversal error"),
        }
    }

    #[test]
    fn test_path_allowed() {
        let env = MockIsoEnvironment::new();
        let result = env.validate_path(
            PathBuf::from("/safe").as_path(),
            PathBuf::from("/safe/subdir/file").as_path(),
        );

        assert!(result.is_ok());
    }

    #[test]
    fn test_path_outside_base_rejected() {
        // Regression for #56: validate_path used to silently return Ok
        // when strip_prefix(base) failed, accepting absolute paths to
        // anywhere on the filesystem.
        let env = MockIsoEnvironment::new();
        let result = env.validate_path(
            PathBuf::from("/mnt/iso").as_path(),
            PathBuf::from("/etc/passwd").as_path(),
        );
        assert!(matches!(result, Err(IsoError::PathTraversal(_))));
    }

    #[test]
    fn test_path_sibling_of_base_rejected() {
        // /safe2 starts with the string "/safe" but is NOT under /safe —
        // Path::starts_with respects component boundaries, not prefix match.
        let env = MockIsoEnvironment::new();
        let result = env.validate_path(
            PathBuf::from("/safe").as_path(),
            PathBuf::from("/safe2/file").as_path(),
        );
        assert!(matches!(result, Err(IsoError::PathTraversal(_))));
    }

    #[tokio::test]
    async fn test_arch_detection() {
        let mock = MockIsoEnvironment::with_iso(Distribution::Arch);
        let parser = IsoParser::new(mock);

        let mount_base = PathBuf::from("/mock_mount");
        let entries = parser
            .extract_boot_entries(&mount_base, &PathBuf::from("test.iso"))
            .await
            .unwrap();

        // Should find at least the Arch entry (might also find via other layouts that scan /boot)
        assert!(!entries.is_empty());
        assert!(entries.iter().any(|e| e.distribution == Distribution::Arch));
        assert!(entries
            .iter()
            .any(|e| e.kernel.to_string_lossy().contains("vmlinuz")));
    }

    #[tokio::test]
    async fn test_debian_detection() {
        let mock = MockIsoEnvironment::with_iso(Distribution::Debian);
        let parser = IsoParser::new(mock);

        let mount_base = PathBuf::from("/mock_mount");
        let entries = parser
            .extract_boot_entries(&mount_base, &PathBuf::from("test.iso"))
            .await
            .unwrap();

        assert!(!entries.is_empty());
        assert!(entries
            .iter()
            .any(|e| e.distribution == Distribution::Debian));
    }

    #[tokio::test]
    async fn test_fedora_detection() {
        let mock = MockIsoEnvironment::with_iso(Distribution::Fedora);
        let parser = IsoParser::new(mock);

        let mount_base = PathBuf::from("/mock_mount");
        let entries = parser
            .extract_boot_entries(&mount_base, &PathBuf::from("test.iso"))
            .await
            .unwrap();

        assert!(!entries.is_empty());
        assert!(entries
            .iter()
            .any(|e| e.distribution == Distribution::Fedora));
    }

    #[test]
    fn test_distribution_from_paths() {
        assert_eq!(
            Distribution::from_paths(PathBuf::from("/boot/vmlinuz").as_path()),
            Distribution::Arch
        );
        assert_eq!(
            Distribution::from_paths(PathBuf::from("/casper/vmlinuz").as_path()),
            Distribution::Debian
        );
        assert_eq!(
            Distribution::from_paths(PathBuf::from("/images/pxeboot/vmlinuz").as_path()),
            Distribution::Fedora
        );
    }

    #[test]
    fn test_boot_entry_serialization() {
        let entry = BootEntry {
            label: "Test Linux".to_string(),
            kernel: PathBuf::from("boot/vmlinuz"),
            initrd: Some(PathBuf::from("boot/initrd.img")),
            kernel_args: Some("quiet".to_string()),
            distribution: Distribution::Arch,
            source_iso: "test.iso".to_string(),
            pretty_name: None,
        };

        let json = serde_json::to_string(&entry).unwrap();
        let decoded: BootEntry = serde_json::from_str(&json).unwrap();

        assert_eq!(decoded.label, "Test Linux");
        assert_eq!(decoded.distribution, Distribution::Arch);
    }

    // ---- #119: pretty-name detection --------------------------------

    #[test]
    fn parse_pretty_name_systemd_shape() {
        let content = r#"
NAME="Ubuntu"
VERSION_ID="24.04"
PRETTY_NAME="Ubuntu 24.04.2 LTS (Noble Numbat)"
ID=ubuntu
"#;
        assert_eq!(
            parse_os_release_pretty_name(content).as_deref(),
            Some("Ubuntu 24.04.2 LTS (Noble Numbat)"),
        );
    }

    #[test]
    fn parse_pretty_name_strips_single_quotes() {
        let content = "PRETTY_NAME='Alpine Linux v3.20'";
        assert_eq!(
            parse_os_release_pretty_name(content).as_deref(),
            Some("Alpine Linux v3.20"),
        );
    }

    #[test]
    fn parse_pretty_name_unquoted_value() {
        // Some distros omit the quotes; spec allows either.
        let content = "PRETTY_NAME=Arch Linux";
        assert_eq!(
            parse_os_release_pretty_name(content).as_deref(),
            Some("Arch Linux"),
        );
    }

    #[test]
    fn parse_pretty_name_empty_returns_none() {
        assert!(parse_os_release_pretty_name("PRETTY_NAME=\"\"").is_none());
        assert!(parse_os_release_pretty_name("").is_none());
    }

    #[test]
    fn parse_pretty_name_missing_returns_none() {
        let content = "NAME=\"Ubuntu\"\nID=ubuntu";
        assert!(parse_os_release_pretty_name(content).is_none());
    }

    #[test]
    fn parse_pretty_name_first_match_wins() {
        // Defensive: if a file has two PRETTY_NAME lines, take the first.
        let content = "PRETTY_NAME=\"First\"\nPRETTY_NAME=\"Second\"";
        assert_eq!(
            parse_os_release_pretty_name(content).as_deref(),
            Some("First"),
        );
    }

    #[test]
    fn read_pretty_name_finds_etc_os_release() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("etc")).unwrap();
        std::fs::write(
            tmp.path().join("etc/os-release"),
            "PRETTY_NAME=\"Rocky Linux 9.3 (Blue Onyx)\"\n",
        )
        .unwrap();
        assert_eq!(
            read_pretty_name(tmp.path()).as_deref(),
            Some("Rocky Linux 9.3 (Blue Onyx)"),
        );
    }

    #[test]
    fn read_pretty_name_falls_back_to_disk_info() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".disk")).unwrap();
        std::fs::write(
            tmp.path().join(".disk/info"),
            "Ubuntu 24.04.2 LTS \"Noble Numbat\" - Release amd64 (20250215)\n",
        )
        .unwrap();
        assert_eq!(
            read_pretty_name(tmp.path()).as_deref(),
            Some("Ubuntu 24.04.2 LTS \"Noble Numbat\" - Release amd64 (20250215)"),
        );
    }

    #[test]
    fn read_pretty_name_alpine_release_prepends_alpine_linux() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("etc")).unwrap();
        std::fs::write(tmp.path().join("etc/alpine-release"), "3.20.3\n").unwrap();
        assert_eq!(
            read_pretty_name(tmp.path()).as_deref(),
            Some("Alpine Linux 3.20.3"),
        );
    }

    #[test]
    fn read_pretty_name_prefers_etc_over_lib() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("etc")).unwrap();
        std::fs::create_dir_all(tmp.path().join("usr/lib")).unwrap();
        std::fs::write(
            tmp.path().join("etc/os-release"),
            "PRETTY_NAME=\"Etc Wins\"\n",
        )
        .unwrap();
        std::fs::write(
            tmp.path().join("usr/lib/os-release"),
            "PRETTY_NAME=\"Lib Loses\"\n",
        )
        .unwrap();
        assert_eq!(read_pretty_name(tmp.path()).as_deref(), Some("Etc Wins"),);
    }

    #[test]
    fn read_pretty_name_returns_none_for_empty_mount() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(read_pretty_name(tmp.path()).is_none());
    }

    #[test]
    fn read_pretty_name_skips_empty_disk_info_line() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".disk")).unwrap();
        std::fs::write(tmp.path().join(".disk/info"), "\n\n   \nDebian 12.8\n").unwrap();
        assert_eq!(read_pretty_name(tmp.path()).as_deref(), Some("Debian 12.8"),);
    }

    /// `MockIsoEnvironment::metadata` must fail closed — previously it
    /// returned the real metadata of `std::env::temp_dir()` for any path
    /// the mock knew about, which silently validated size/mtime assertions
    /// against `/tmp` values instead of mock data. Regression from #138.
    #[test]
    fn mock_metadata_fails_closed() {
        let env = MockIsoEnvironment::new();
        let err = env
            .metadata(std::path::Path::new("/mock_mount/boot/vmlinuz"))
            .expect_err("mock metadata() must surface an error");
        assert_eq!(err.kind(), std::io::ErrorKind::Unsupported);
    }

    /// Poisoned mount-points mutex must not cascade. Simulate a poisoning
    /// by panicking inside a lock-holding scope and confirm subsequent
    /// `mount_iso` / `unmount` calls still succeed. Regression from #138.
    #[test]
    fn mock_mount_lock_recovers_from_poison() {
        use std::sync::Arc;
        let env = Arc::new(MockIsoEnvironment::new());
        // Force a poisoned lock by panicking inside a critical section on
        // a scoped thread. The spawned thread's join result is expected
        // to be Err (the panic); that's what poisons the Mutex.
        let env_for_thread = env.clone();
        let join = std::thread::spawn(move || {
            let _guard = env_for_thread.mount_points.lock().unwrap();
            panic!("deliberately poisoning the mutex for this test");
        })
        .join();
        assert!(join.is_err(), "helper thread must have panicked");

        // Now verify the mock still functions — mount + unmount should
        // succeed without panicking via lock recovery.
        let iso = std::path::Path::new("/isos/test.iso");
        let mount = env
            .mount_iso(iso)
            .expect("mount_iso must recover from poison");
        env.unmount(&mount)
            .expect("unmount must recover from poison");
    }
}
