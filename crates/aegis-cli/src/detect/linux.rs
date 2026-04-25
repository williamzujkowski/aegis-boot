// SPDX-License-Identifier: MIT OR Apache-2.0

//! Linux removable-drive detection via sysfs.
//!
//! Enumerates `/sys/block/sd*` looking for removable USB mass storage
//! devices. Filters out system drives, `NVMe`, loop devices, and anything
//! not flagged as removable by the kernel.

use super::{BlockDevice, BlockDeviceTransport, Drive};
use std::fs;
use std::path::{Path, PathBuf};

/// Scan sysfs for removable USB block devices suitable for flashing.
/// Returns them sorted by device name.
#[must_use]
pub fn list_removable_drives() -> Vec<Drive> {
    let mut drives = Vec::new();
    let Ok(entries) = fs::read_dir("/sys/block") else {
        return drives;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        // Only sd* devices (SCSI/USB mass storage).
        if !name_str.starts_with("sd") {
            continue;
        }
        let sysdir = entry.path();
        // Must be removable.
        if read_sysfs_int(&sysdir.join("removable")) != Some(1) {
            continue;
        }
        // Read model + size.
        let model = read_sysfs_str(&sysdir.join("device/model"))
            .unwrap_or_else(|| "(unknown model)".to_string());
        let size_bytes = read_sysfs_int_u64(&sysdir.join("size")).unwrap_or(0) * 512;
        // Count partitions (sdX1, sdX2, ...).
        let partitions = fs::read_dir(&sysdir)
            .map(|iter| {
                iter.flatten()
                    .filter(|e| {
                        e.file_name()
                            .to_string_lossy()
                            .starts_with(name_str.as_ref())
                    })
                    .count()
            })
            .unwrap_or(0);

        drives.push(Drive {
            dev: PathBuf::from(format!("/dev/{name_str}")),
            model: model.trim().to_string(),
            size_bytes,
            partitions,
        });
    }
    drives.sort_by(|a, b| a.dev.cmp(&b.dev));
    drives
}

/// Scan `/sys/block` for ALL block devices an operator might care about,
/// regardless of removable bit. Used by `doctor` to surface a full disk
/// inventory (#560).
///
/// Includes: `sd*` (SCSI/SATA/USB), `nvme*n*` (`NVMe` namespaces), `vd*`
/// (virtio), `mmcblk*` (SD/eMMC). Excludes: `loop*`, `ram*`, `dm-*`,
/// `sr*` (optical), `zram*` — none of which are persistent installable
/// targets and would just clutter the inventory row.
///
/// Best-effort: missing sysfs files surface as `Unknown` transport,
/// `(unknown model)` text, or `0` size. The doctor row is informational
/// only, never a Fail trigger.
#[must_use]
pub fn list_block_devices() -> Vec<BlockDevice> {
    let Ok(entries) = fs::read_dir("/sys/block") else {
        return Vec::new();
    };
    let mut devices: Vec<BlockDevice> = entries
        .flatten()
        .filter_map(|entry| build_block_device(&entry.path()))
        .collect();
    devices.sort_by(|a, b| a.dev.cmp(&b.dev));
    devices
}

fn build_block_device(sysdir: &Path) -> Option<BlockDevice> {
    let name_os = sysdir.file_name()?;
    let name = name_os.to_string_lossy().into_owned();
    if !is_inventoried_block_device(&name) {
        return None;
    }
    let model = read_sysfs_str(&sysdir.join("device/model"))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "(unknown model)".to_string());
    let size_bytes = read_sysfs_int_u64(&sysdir.join("size")).unwrap_or(0) * 512;
    let removable = read_sysfs_int(&sysdir.join("removable")) == Some(1);
    let transport = classify_transport(sysdir, &name);
    Some(BlockDevice {
        dev: PathBuf::from(format!("/dev/{name}")),
        model,
        size_bytes,
        removable,
        transport,
    })
}

fn is_inventoried_block_device(name: &str) -> bool {
    // Whitelist persistent storage prefixes; drop pseudo / volatile / optical.
    name.starts_with("sd")
        || name.starts_with("nvme")
        || name.starts_with("vd")
        || name.starts_with("mmcblk")
        || name.starts_with("xvd")
}

fn classify_transport(sysdir: &Path, name: &str) -> BlockDeviceTransport {
    if name.starts_with("nvme") {
        return BlockDeviceTransport::Nvme;
    }
    if name.starts_with("vd") || name.starts_with("xvd") {
        return BlockDeviceTransport::Virtio;
    }
    if name.starts_with("mmcblk") {
        return BlockDeviceTransport::Mmc;
    }
    // For sd* the bus type is reported under device/../subsystem (a symlink
    // pointing into /sys/bus/{usb,scsi,ata,...}). Read the resolved target's
    // last component as the transport label.
    let subsystem = sysdir.join("device/subsystem");
    if let Ok(resolved) = fs::read_link(&subsystem) {
        if let Some(last) = resolved.file_name() {
            return match last.to_string_lossy().as_ref() {
                "usb" => BlockDeviceTransport::Usb,
                "scsi" => BlockDeviceTransport::Scsi,
                "ata" | "sata" => BlockDeviceTransport::Sata,
                _ => BlockDeviceTransport::Unknown,
            };
        }
    }
    BlockDeviceTransport::Unknown
}

fn read_sysfs_str(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()
}

fn read_sysfs_int(path: &Path) -> Option<i64> {
    read_sysfs_str(path)?.trim().parse().ok()
}

fn read_sysfs_int_u64(path: &Path) -> Option<u64> {
    read_sysfs_str(path)?.trim().parse().ok()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::missing_panics_doc)]
mod tests {
    use super::*;

    #[test]
    fn is_inventoried_keeps_persistent_prefixes() {
        for keep in ["sda", "sdb1", "nvme0n1", "vda", "xvdc", "mmcblk0"] {
            assert!(
                is_inventoried_block_device(keep),
                "expected {keep} to be inventoried"
            );
        }
    }

    #[test]
    fn is_inventoried_drops_pseudo_and_optical() {
        for drop in ["loop0", "ram0", "dm-0", "sr0", "zram0"] {
            assert!(
                !is_inventoried_block_device(drop),
                "expected {drop} to be excluded"
            );
        }
    }

    #[test]
    fn classify_transport_dispatches_on_name_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        // Name-only dispatch covers nvme/virtio/mmc without needing sysfs
        // symlinks.
        assert_eq!(
            classify_transport(tmp.path(), "nvme0n1"),
            BlockDeviceTransport::Nvme
        );
        assert_eq!(
            classify_transport(tmp.path(), "vda"),
            BlockDeviceTransport::Virtio
        );
        assert_eq!(
            classify_transport(tmp.path(), "xvdc"),
            BlockDeviceTransport::Virtio
        );
        assert_eq!(
            classify_transport(tmp.path(), "mmcblk0"),
            BlockDeviceTransport::Mmc
        );
    }

    #[test]
    fn classify_transport_returns_unknown_for_sd_without_subsystem() {
        let tmp = tempfile::tempdir().unwrap();
        // No device/subsystem symlink → Unknown (graceful fallback).
        assert_eq!(
            classify_transport(tmp.path(), "sda"),
            BlockDeviceTransport::Unknown
        );
    }
}
