// SPDX-License-Identifier: MIT OR Apache-2.0

//! [#418] Phase 1 — macOS partition-plan builder.
//!
//! Produces the argv for `diskutil partitionDisk` that writes
//! aegis-boot's canonical 2-partition GPT layout (ESP + `AEGIS_ISOS`)
//! onto a macOS disk identifier.
//!
//! ## Canonical argv shape
//!
//! ```text
//! diskutil partitionDisk /dev/<device_id> 2 GPT \
//!     MS-DOS\ FAT32 AEGIS_ESP 400M \
//!     ExFAT AEGIS_ISOS R
//! ```
//!
//! Where:
//!
//! * `2` — partition count.
//! * `GPT` — partition scheme; required for UEFI + shim chain.
//! * `MS-DOS FAT32` — filesystem identifier macOS uses for the ESP.
//!   `diskutil list`'s naming is different from Linux's FAT32 /
//!   fat-32 labels; the exact string here is what macOS accepts.
//! * `AEGIS_ESP` — volume label. Uppercase preserved on FAT32.
//! * `400M` — ESP size, tracking `constants::ESP_SIZE_MB`.
//! * `ExFAT` — filesystem for the data partition. Matches the Linux
//!   flash path (#243) — an exFAT `AEGIS_ISOS` partition is
//!   cross-platform readable + supports ISO files larger than 4 GiB.
//! * `R` — remaining space. `diskutil`'s documented size shorthand.
//!
//! ## Why pure-fn + typed argv
//!
//! The subprocess wrapper that actually invokes `diskutil` (Phase 2
//! of #418) does so only on `#[cfg(target_os = "macos")]`. The
//! pure-fn here is always compiled and always exercised by unit
//! tests, so the macOS-specific side can evolve without regressing
//! the argv contract the rest of aegis-boot's flash pipeline
//! depends on.
//!
//! ## Safety invariants
//!
//! 1. **Refuse `disk0`.** On macOS the internal boot drive is
//!    virtually always `/dev/disk0`; `diskutil partitionDisk` on it
//!    would nuke the host OS. Pure-fn side catches this before any
//!    subprocess runs.
//! 2. **Refuse empty / obviously-invalid device IDs.** Pure-fn
//!    validation protects the subprocess wrapper from getting fed
//!    `""` or `../diskN` via a malformed config file.
//! 3. **Require removable.** Out of scope for Phase 1 — belongs in
//!    the Phase 2 preflight module (mirror of
//!    `windows_direct_install::preflight`). Here the pure-fn builder
//!    only gates on disk0 + shape, not mount state.
//!
//! [#418]: https://github.com/aegis-boot/aegis-boot/issues/418

// Phase 1 lands the pure-fn layout builder ahead of the Phase 2
// subprocess wrapper + the Phase 3 flash-pipeline wiring. Until
// those land nothing in main.rs imports this module; the unit-test
// suite exercises every public symbol, so regressions still surface
// at CI time. Mirror of the pattern in
// `windows_direct_install::partition`.
#![allow(dead_code)]

use crate::constants::ESP_SIZE_MB;

/// Why a partition request was rejected at the pure-fn layer.
///
/// Each variant maps to a distinct operator-facing NEXT ACTION line
/// — callers that pretty-print the error don't need to string-match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MacosPartitionError {
    /// Caller passed `disk0`. On macOS that's virtually always the
    /// host's internal boot drive — refuse without even asking.
    /// Siblings on Windows: [`crate::windows_direct_install::partition::PartitionBuildError::BootDriveRefused`].
    BootDiskRefused,
    /// Device identifier was empty. A brand-new `diskutil list` run
    /// always returns at least one device, so an empty ID is either
    /// a config-file parse bug or an argv omission; either way,
    /// refuse rather than let `diskutil` see `""` as argv.
    EmptyDeviceId,
    /// Device identifier has a character set beyond what `diskutil`
    /// names use. macOS device IDs are strictly `diskN[sM]` with N
    /// and M positive integers (sometimes followed by partition
    /// suffixes in child contexts, but at the disk level we want
    /// the bare whole-disk id). The offending string is echoed for
    /// log-grep.
    InvalidDeviceId {
        /// The offending id, echoed verbatim for operator logs.
        id: String,
    },
}

impl std::fmt::Display for MacosPartitionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BootDiskRefused => write!(
                f,
                "refusing to partition /dev/disk0 — that's virtually always the macOS internal boot drive; pick a removable external target (disk1 / disk2 / ...)"
            ),
            Self::EmptyDeviceId => write!(
                f,
                "empty macOS device identifier — pass a value like `disk5` (not `/dev/disk5`)"
            ),
            Self::InvalidDeviceId { id } => write!(
                f,
                "{id:?} is not a valid diskutil disk identifier (expected form `diskN`, with N a positive integer)"
            ),
        }
    }
}

/// The parsed + validated argv plan for one
/// `diskutil partitionDisk ...` invocation.
///
/// Construct via [`build_diskutil_plan`]; never hand-build — the
/// disk-identifier invariants aren't re-checked by the subprocess
/// wrapper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiskutilPartitionPlan {
    /// The whole-disk id (e.g. `"disk5"`). `/dev/` prefix stripped —
    /// both `diskutil` and the raw-write paths want the bare
    /// identifier.
    pub device_id: String,
    /// Full argv after the `diskutil` binary name. Fed to
    /// `Command::new("diskutil").args(&plan.argv)` by the Phase 2
    /// subprocess wrapper.
    pub argv: Vec<String>,
}

/// Build the `diskutil partitionDisk` argv for aegis-boot's canonical
/// 2-partition GPT layout on `/dev/<device_id>`.
///
/// `device_id` is the bare identifier (e.g. `"disk5"`) — callers that
/// hold a `"/dev/disk5"` form should strip the prefix themselves,
/// since the same identifier is reused for the raw-write pass later.
///
/// # Errors
///
/// See [`MacosPartitionError`] — three variants, each with a
/// distinct operator-facing message.
pub(crate) fn build_diskutil_plan(
    device_id: &str,
) -> Result<DiskutilPartitionPlan, MacosPartitionError> {
    if device_id == "disk0" {
        return Err(MacosPartitionError::BootDiskRefused);
    }
    if device_id.is_empty() {
        return Err(MacosPartitionError::EmptyDeviceId);
    }
    if !is_valid_whole_disk_id(device_id) {
        return Err(MacosPartitionError::InvalidDeviceId {
            id: device_id.to_string(),
        });
    }

    let argv = vec![
        "partitionDisk".to_string(),
        format!("/dev/{device_id}"),
        "2".to_string(),
        "GPT".to_string(),
        "MS-DOS FAT32".to_string(),
        "AEGIS_ESP".to_string(),
        format!("{ESP_SIZE_MB}M"),
        "ExFAT".to_string(),
        "AEGIS_ISOS".to_string(),
        "R".to_string(),
    ];

    Ok(DiskutilPartitionPlan {
        device_id: device_id.to_string(),
        argv,
    })
}

/// Whether `s` is a macOS whole-disk identifier in the form `diskN`.
///
/// Rejects partition suffixes (`disk5s1`) — those are handled by a
/// dedicated `mount_esp`-style function in Phase 2; at the partition
/// layer we want the whole disk, not a slice of it.
fn is_valid_whole_disk_id(s: &str) -> bool {
    let Some(rest) = s.strip_prefix("disk") else {
        return false;
    };
    if rest.is_empty() {
        return false;
    }
    rest.bytes().all(|b| b.is_ascii_digit())
}

/// Partition the target disk via `diskutil`, fed the argv produced
/// by [`build_diskutil_plan`]. macOS-only — the \[`cfg`\] gate means
/// Linux + Windows builds never pull in this function, but the
/// `x86_64-apple-darwin` CI cross-compile step catches compile
/// errors on every PR.
///
/// macOS's `diskutil` doesn't require root for external / removable
/// drives (operator-session privilege is enough); if the target is
/// internal or in use, `diskutil` itself surfaces the actionable
/// error. The subprocess's stderr is propagated unchanged to the
/// caller — we don't try to translate `diskutil`'s messages.
///
/// # Errors
///
/// Returns the subprocess exit code + stderr as a `String` on any
/// non-zero exit or spawn failure. Sibling of
/// [`crate::windows_direct_install::partition::partition_via_diskpart`].
#[cfg(target_os = "macos")]
pub(crate) fn partition_via_diskutil(plan: &DiskutilPartitionPlan) -> Result<(), String> {
    use std::process::Command;

    let out = Command::new("/usr/sbin/diskutil")
        .args(&plan.argv)
        .output()
        .map_err(|e| format!("spawn /usr/sbin/diskutil: {e}"))?;

    if !out.status.success() {
        return Err(format!(
            "diskutil exited {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn refuses_disk_zero() {
        let err = build_diskutil_plan("disk0").unwrap_err();
        assert_eq!(err, MacosPartitionError::BootDiskRefused);
    }

    #[test]
    fn accepts_non_boot_whole_disks() {
        for id in ["disk1", "disk2", "disk5", "disk9", "disk99"] {
            let plan = build_diskutil_plan(id).expect("non-zero whole disk should accept");
            assert_eq!(plan.device_id, id);
            assert_eq!(plan.argv[0], "partitionDisk");
            assert_eq!(plan.argv[1], format!("/dev/{id}"));
        }
    }

    #[test]
    fn rejects_empty_device_id() {
        let err = build_diskutil_plan("").unwrap_err();
        assert_eq!(err, MacosPartitionError::EmptyDeviceId);
    }

    #[test]
    fn rejects_dev_prefix() {
        // Callers must strip `/dev/` themselves — passing it through
        // would surface as an invalid id here.
        let err = build_diskutil_plan("/dev/disk5").unwrap_err();
        assert!(matches!(err, MacosPartitionError::InvalidDeviceId { .. }));
    }

    #[test]
    fn rejects_partition_suffix() {
        // disk5s1 is a partition, not a whole disk. Refuse — Phase 2
        // will have a dedicated helper that accepts these for the
        // ESP mount path.
        let err = build_diskutil_plan("disk5s1").unwrap_err();
        assert!(matches!(err, MacosPartitionError::InvalidDeviceId { .. }));
    }

    #[test]
    fn rejects_non_disk_prefix() {
        for bogus in ["usb5", "dsk5", "diskX", "disk 5", "disk5 ", " disk5"] {
            let err = build_diskutil_plan(bogus).unwrap_err();
            assert!(
                matches!(err, MacosPartitionError::InvalidDeviceId { .. }),
                "expected InvalidDeviceId for {bogus:?}, got {err:?}"
            );
        }
    }

    #[test]
    fn argv_matches_canonical_shape() {
        // Pin the full argv sequence so subprocess-wrapper
        // regressions in Phase 2 can't silently change the on-disk
        // layout.
        let plan = build_diskutil_plan("disk5").unwrap();
        assert_eq!(
            plan.argv,
            vec![
                "partitionDisk",
                "/dev/disk5",
                "2",
                "GPT",
                "MS-DOS FAT32",
                "AEGIS_ESP",
                &format!("{ESP_SIZE_MB}M"),
                "ExFAT",
                "AEGIS_ISOS",
                "R",
            ]
        );
    }

    #[test]
    fn esp_size_tracks_shared_constant() {
        let plan = build_diskutil_plan("disk5").unwrap();
        // The size arg is at index 6 ("400M"). If constants::ESP_SIZE_MB
        // changes, this asserts the partition plan picks it up.
        assert!(plan.argv[6].ends_with('M'));
        let size: u64 = plan.argv[6].trim_end_matches('M').parse().unwrap();
        assert_eq!(size, ESP_SIZE_MB);
    }

    #[test]
    fn error_display_names_the_safety_reason() {
        let s = MacosPartitionError::BootDiskRefused.to_string();
        assert!(s.contains("disk0"));
        assert!(s.contains("boot drive"));
    }

    #[test]
    fn invalid_id_display_echoes_offender() {
        let s = MacosPartitionError::InvalidDeviceId {
            id: "bad-id".to_string(),
        }
        .to_string();
        assert!(s.contains("bad-id"));
    }

    #[test]
    fn is_valid_whole_disk_id_whitelist() {
        for ok in ["disk0", "disk1", "disk10", "disk999"] {
            assert!(is_valid_whole_disk_id(ok), "{ok} should be valid");
        }
    }

    #[test]
    fn is_valid_whole_disk_id_blacklist() {
        for bad in [
            "",
            "disk",
            "disks",
            "disk5s1",
            "diskX",
            "/dev/disk5",
            " disk5",
            "disk5 ",
        ] {
            assert!(!is_valid_whole_disk_id(bad), "{bad} should be invalid");
        }
    }
}
