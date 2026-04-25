// SPDX-License-Identifier: MIT OR Apache-2.0

//! Removable-drive detection. Platform-dispatched: Linux uses sysfs,
//! macOS uses `diskutil list -plist | plutil -convert json`, other
//! platforms currently return an empty list + a clear error from the
//! callers that need a drive.
//!
//! The public surface — [`Drive`] struct + [`list_removable_drives`] —
//! is identical across platforms; only the implementation differs. This
//! keeps `flash.rs`, `doctor.rs`, and `eject.rs` fully platform-agnostic.

use std::path::PathBuf;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::list_removable_drives;

// On macOS, the `macos` module compiles for the subprocess path AND on
// other platforms so `parse_diskutil_json` can be unit-tested against a
// committed plist-derived JSON fixture. Only `list_removable_drives` is
// macOS-gated.
mod macos;

#[cfg(target_os = "macos")]
pub use macos::list_removable_drives;

// Same pattern for Windows: `parse_get_disk_json` is always compiled
// (testable from Linux CI via committed fixtures); only
// `list_removable_drives` is Windows-gated.
mod windows;

#[cfg(target_os = "windows")]
pub use windows::list_removable_drives;

/// Stub for unsupported platforms. Callers surface a platform-specific
/// error so operators don't silently get "no drives detected".
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
#[must_use]
pub fn list_removable_drives() -> Vec<Drive> {
    Vec::new()
}

/// Cross-platform full block-device inventory used by `doctor` (#560).
/// Linux returns `Some(devices)`; macOS and Windows return `None` so the
/// doctor row falls back to a Skip with a platform note rather than
/// silently emitting "0 disks detected".
///
/// `clippy::unnecessary_wraps` is silenced here because the `Option`
/// disappears on Linux under the cfg gate — clippy only sees the active
/// branch and assumes `None` is unreachable. The cross-platform contract
/// is what matters; allowing the lint preserves the API shape.
#[must_use]
#[allow(clippy::unnecessary_wraps)]
pub fn list_block_devices() -> Option<Vec<BlockDevice>> {
    #[cfg(target_os = "linux")]
    {
        Some(linux::list_block_devices())
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// A persistent block device the operator might flash, image, or use as
/// an install target. Surfaces in `aegis-boot doctor` output as one row
/// per device. Includes both removable (USB sticks, SD cards) and fixed
/// (`NVMe`, SATA SSDs) media — `removable` distinguishes them.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub struct BlockDevice {
    /// Block device path (e.g. `/dev/nvme0n1`, `/dev/sda`).
    pub dev: PathBuf,
    /// Human-readable model string from sysfs `device/model`.
    pub model: String,
    /// Capacity in bytes (zero if sysfs `size` is missing).
    pub size_bytes: u64,
    /// Kernel-reported removable flag (`/sys/block/<dev>/removable == 1`).
    pub removable: bool,
    /// Bus / transport classification — informational, not load-bearing.
    pub transport: BlockDeviceTransport,
}

impl BlockDevice {
    /// Human-readable capacity (mirrors `Drive::size_human`).
    #[must_use]
    #[allow(clippy::cast_precision_loss, dead_code)]
    pub fn size_human(&self) -> String {
        let gb = self.size_bytes as f64 / 1_073_741_824.0;
        if gb >= 1.0 {
            format!("{gb:.1} GB")
        } else {
            let mb = self.size_bytes as f64 / 1_048_576.0;
            format!("{mb:.0} MB")
        }
    }
}

/// Bus classification for a [`BlockDevice`]. Derived from the device-name
/// prefix or — for SCSI/SATA `sd*` devices — the resolved
/// `device/subsystem` symlink target. `Unknown` when neither yields a
/// definitive answer; the doctor row still surfaces the device.
///
/// `dead_code` is allowed because the constructor lives in the
/// Linux-only `detect::linux` module — on macOS/Windows the variants are
/// reachable through public API but never constructed in-tree, which the
/// dead-code lint flags. The enum is part of the cross-platform surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum BlockDeviceTransport {
    Nvme,
    Sata,
    Scsi,
    Usb,
    Virtio,
    Mmc,
    Unknown,
}

impl BlockDeviceTransport {
    /// Lowercase short label suitable for inline doctor-row prose.
    #[must_use]
    #[allow(dead_code)]
    pub fn label(self) -> &'static str {
        match self {
            BlockDeviceTransport::Nvme => "nvme",
            BlockDeviceTransport::Sata => "sata",
            BlockDeviceTransport::Scsi => "scsi",
            BlockDeviceTransport::Usb => "usb",
            BlockDeviceTransport::Virtio => "virtio",
            BlockDeviceTransport::Mmc => "mmc",
            BlockDeviceTransport::Unknown => "unknown-bus",
        }
    }
}

/// A detected removable drive. Identical across platforms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drive {
    /// Block/character device path (e.g. `/dev/sdc` on Linux,
    /// `/dev/disk5` on macOS).
    pub dev: PathBuf,
    /// Human-readable model string — kernel-reported on Linux,
    /// diskutil `MediaName` on macOS.
    pub model: String,
    /// Capacity in bytes.
    pub size_bytes: u64,
    /// Number of existing partitions.
    pub partitions: usize,
}

impl Drive {
    /// Human-readable capacity.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn size_human(&self) -> String {
        let gb = self.size_bytes as f64 / 1_073_741_824.0;
        if gb >= 1.0 {
            format!("{gb:.1} GB")
        } else {
            let mb = self.size_bytes as f64 / 1_048_576.0;
            format!("{mb:.0} MB")
        }
    }
}

/// The current platform's display name, for use in error messages.
/// ("Linux", "macOS", or "this platform"). Used by `flash.rs` on
/// non-Linux platforms to compose the "build requires Linux" hint;
/// dead code on Linux.
#[allow(dead_code)]
#[must_use]
pub fn platform_display_name() -> &'static str {
    if cfg!(target_os = "linux") {
        "Linux"
    } else if cfg!(target_os = "macos") {
        "macOS"
    } else if cfg!(target_os = "windows") {
        "Windows"
    } else {
        "this platform"
    }
}
