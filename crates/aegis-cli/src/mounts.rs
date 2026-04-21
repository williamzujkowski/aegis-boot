// SPDX-License-Identifier: MIT OR Apache-2.0

//! Shared helpers for reading `/proc/mounts` and deriving
//! block-device paths. Consolidates three near-identical implementations
//! that lived in `inventory.rs` and `attest.rs` (each doing their own
//! longest-prefix match + disk suffix stripper).
//!
//! All helpers are side-effect-free (except reading `/proc/mounts`)
//! and panic-free by construction. `None` returns are the canonical
//! "I couldn't determine this" signal — callers must have a graceful
//! fallback.

use std::path::{Path, PathBuf};

/// Reverse-lookup the backing device for a mount path via `/proc/mounts`.
/// Returns the longest-prefix-matching device field (e.g. `/dev/sda2`
/// for a mount at `/media/william/AEGIS_ISOS`). Filters out non-block
/// mounts (tmpfs, cgroup, proc, etc.) by requiring the device path to
/// begin with `/dev/`.
///
/// Longest-prefix matching matters: `/run/media/operator/AEGIS_ISOS`
/// would otherwise match both `/` (root) and the actual stick mount —
/// we want the stick.
#[allow(dead_code)] // used by attest.rs and inventory.rs (cross-module reuse)
pub(crate) fn device_for_mount(target: &Path) -> Option<PathBuf> {
    let mounts = std::fs::read_to_string("/proc/mounts").ok()?;
    let canonical = target
        .canonicalize()
        .ok()
        .unwrap_or_else(|| target.to_path_buf());
    let target_s = canonical.to_string_lossy();
    let mut best: Option<(usize, PathBuf)> = None;
    for line in mounts.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 2 {
            continue;
        }
        let dev = fields[0];
        let mp = fields[1];
        if !dev.starts_with("/dev/") {
            continue;
        }
        if (target_s == mp || target_s.starts_with(&format!("{mp}/")))
            && best.as_ref().is_none_or(|(prev, _)| mp.len() > *prev)
        {
            best = Some((mp.len(), PathBuf::from(dev)));
        }
    }
    best.map(|(_, dev)| dev)
}

/// Return the filesystem type reported by `/proc/mounts` for the
/// best-matching mount point. Unlike `device_for_mount`, this accepts
/// any mount source (including tmpfs and overlayfs) so the caller
/// can distinguish a vfat-mounted `AEGIS_ISOS` from, say, an operator
/// who accidentally pointed us at a tmpfs directory.
#[allow(dead_code)]
pub(crate) fn filesystem_type(path: &Path) -> Option<String> {
    let mounts = std::fs::read_to_string("/proc/mounts").ok()?;
    let target = path.to_string_lossy();
    let mut best: Option<(&str, usize)> = None;
    for line in mounts.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 3 {
            continue;
        }
        let mp = fields[1];
        let fs = fields[2];
        if (target == mp || target.starts_with(&format!("{mp}/")))
            && best.is_none_or(|(_, prev)| mp.len() > prev)
        {
            best = Some((fs, mp.len()));
        }
    }
    best.map(|(f, _)| f.to_string())
}

/// Strip the trailing partition suffix from a block device path so
/// callers that need the whole-disk form (e.g. `aegis-boot flash`
/// which refuses partition devices) get it directly.
///
/// Handles two Linux naming conventions:
///
/// * **sata / virtio / xen / hd** — whole disk ends in a letter (`sda`);
///   partition appends digits directly (`sda2`, `sdb15`, `vda1`).
/// * **nvme / mmcblk / loop** — whole disk ends in a digit (`nvme0n1`,
///   `mmcblk0`); partition uses a `p` separator (`nvme0n1p2`,
///   `mmcblk0p1`, `loop0p1`).
///
/// Returns `None` when the input is already a whole-disk path or
/// doesn't match either convention — safer than emitting a wrong disk
/// and risking a destructive operation on the wrong device.
#[allow(dead_code)]
pub(crate) fn parent_disk(partition: &Path) -> Option<PathBuf> {
    let s = partition.to_str()?;
    let stem = s.strip_prefix("/dev/")?;
    let bytes = stem.as_bytes();
    if bytes.is_empty() {
        return None;
    }
    // Find the length of the trailing digit run.
    let mut digits_start = bytes.len();
    while digits_start > 0 && bytes[digits_start - 1].is_ascii_digit() {
        digits_start -= 1;
    }
    if digits_start == bytes.len() || digits_start == 0 {
        // No trailing digits, or the whole name is digits.
        return None;
    }
    let char_before_digits = bytes[digits_start - 1];

    // nvme / mmcblk / loop convention: stem ends in a digit, partition
    // inserts `p` before its own digit run. Strip the `p<digits>`.
    if char_before_digits == b'p' && digits_start >= 2 && bytes[digits_start - 2].is_ascii_digit() {
        let parent = &stem[..digits_start - 1];
        return Some(PathBuf::from(format!("/dev/{parent}")));
    }

    // sata-style — constrain by known prefix so mmcblk0/nvme0n1
    // (whole disks that happen to end in digit+after-alpha) don't get
    // mis-stripped. The kernel's block-device naming for sata/virtio/
    // xen/hd uses `sd<a..z>[a..z]*<N>` where the stem is an alpha run.
    // Anything outside these prefixes and not matching the `pN` rule
    // above is either a whole disk or an unrecognized naming scheme.
    let sata_prefix = ["sd", "vd", "hd", "xvd"]
        .iter()
        .any(|p| stem.starts_with(p));
    if sata_prefix && char_before_digits.is_ascii_alphabetic() {
        let parent = &stem[..digits_start];
        return Some(PathBuf::from(format!("/dev/{parent}")));
    }

    None
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    #[test]
    fn parent_disk_strips_sata_partition_digits() {
        assert_eq!(
            parent_disk(Path::new("/dev/sda2")).as_deref(),
            Some(Path::new("/dev/sda"))
        );
        assert_eq!(
            parent_disk(Path::new("/dev/sdc1")).as_deref(),
            Some(Path::new("/dev/sdc"))
        );
        assert_eq!(
            parent_disk(Path::new("/dev/sdb15")).as_deref(),
            Some(Path::new("/dev/sdb"))
        );
        assert_eq!(
            parent_disk(Path::new("/dev/vda3")).as_deref(),
            Some(Path::new("/dev/vda"))
        );
        assert_eq!(
            parent_disk(Path::new("/dev/hda1")).as_deref(),
            Some(Path::new("/dev/hda"))
        );
    }

    #[test]
    fn parent_disk_strips_nvme_p_partition() {
        assert_eq!(
            parent_disk(Path::new("/dev/nvme0n1p2")).as_deref(),
            Some(Path::new("/dev/nvme0n1"))
        );
        assert_eq!(
            parent_disk(Path::new("/dev/nvme1n1p15")).as_deref(),
            Some(Path::new("/dev/nvme1n1"))
        );
        assert_eq!(
            parent_disk(Path::new("/dev/mmcblk0p1")).as_deref(),
            Some(Path::new("/dev/mmcblk0"))
        );
        assert_eq!(
            parent_disk(Path::new("/dev/loop0p1")).as_deref(),
            Some(Path::new("/dev/loop0"))
        );
    }

    #[test]
    fn parent_disk_declines_whole_disk_input() {
        // Must not mangle a disk path — callers keep their placeholder.
        assert_eq!(parent_disk(Path::new("/dev/sda")), None);
        assert_eq!(parent_disk(Path::new("/dev/nvme0n1")), None);
        assert_eq!(parent_disk(Path::new("/dev/mmcblk0")), None);
        // /dev/loop0 is a whole loop device, not a partition.
        assert_eq!(parent_disk(Path::new("/dev/loop0")), None);
    }

    #[test]
    fn parent_disk_declines_non_dev_paths() {
        assert_eq!(parent_disk(Path::new("/tmp/fake2")), None);
        assert_eq!(parent_disk(Path::new("sda2")), None);
        assert_eq!(parent_disk(Path::new("")), None);
    }
}
