// SPDX-License-Identifier: MIT OR Apache-2.0

//! Linux removable-drive detection via sysfs.
//!
//! Enumerates `/sys/block/sd*` looking for removable USB mass storage
//! devices. Filters out system drives, `NVMe`, loop devices, and anything
//! not flagged as removable by the kernel.

use super::Drive;
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

fn read_sysfs_str(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()
}

fn read_sysfs_int(path: &Path) -> Option<i64> {
    read_sysfs_str(path)?.trim().parse().ok()
}

fn read_sysfs_int_u64(path: &Path) -> Option<u64> {
    read_sysfs_str(path)?.trim().parse().ok()
}
