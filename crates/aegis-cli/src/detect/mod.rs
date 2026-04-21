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
