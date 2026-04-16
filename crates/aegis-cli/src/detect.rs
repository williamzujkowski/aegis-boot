//! Removable drive detection via sysfs.
//!
//! Enumerates `/sys/block/sd*` looking for removable USB mass storage
//! devices. Filters out system drives, `NVMe`, loop devices, and
//! anything that isn't flagged as removable by the kernel.

use std::fs;
use std::path::{Path, PathBuf};

/// A detected removable USB drive.
#[derive(Debug, Clone)]
pub struct Drive {
    /// Block device path (e.g. `/dev/sdc`).
    pub dev: PathBuf,
    /// Human-readable model string from sysfs.
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
