//! Runtime ISO discovery on the live aegis-boot rescue environment.
//!
//! Walks attached block devices, loop-mounts candidates, and returns a
//! structured list of bootable ISOs using the lower-level `iso-parser` crate
//! for on-media analysis.
//!
//! # Scope
//!
//! This crate is the runtime-side consumer of `iso-parser`. It is meant to
//! run inside the signed Linux rescue initramfs (see
//! [ADR 0001](../../../docs/adr/0001-runtime-architecture.md)), not in a
//! pre-OS UEFI context.
//!
//! # Status
//!
//! Skeleton only. Implementation is tracked in follow-up issues.

#![forbid(unsafe_code)]

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// A discovered, loop-mountable ISO with enough metadata to render in the TUI
/// and to hand off to `kexec-loader`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredIso {
    /// Absolute path to the ISO file on the probed filesystem.
    pub path: PathBuf,
    /// Best-effort human label (volume id, distro name).
    pub label: Option<String>,
    /// Extracted kernel path inside the ISO, if discovery succeeded.
    pub kernel: Option<PathBuf>,
    /// Extracted initrd path inside the ISO, if present.
    pub initrd: Option<PathBuf>,
    /// Kernel command line the ISO expects (from isolinux/grub config).
    pub cmdline: Option<String>,
}

/// Errors returned during probing.
#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    /// Underlying I/O failure.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// ISO did not match any known layout.
    #[error("unsupported ISO layout")]
    UnsupportedLayout,
}

/// Discover all bootable ISOs reachable from the given roots.
///
/// # Errors
///
/// Returns [`ProbeError`] on I/O failure. Unrecognized layouts are skipped
/// silently and do not abort the scan.
pub fn discover(_roots: &[PathBuf]) -> Result<Vec<DiscoveredIso>, ProbeError> {
    // TODO(#4): loop-device enumeration, GPT/ISO9660 sniffing, El Torito walk.
    Ok(Vec::new())
}
