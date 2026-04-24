// SPDX-License-Identifier: MIT OR Apache-2.0

//! [#418] Phase 4a — macOS preflight checks.
//!
//! Sibling of [`crate::windows_direct_install::preflight`]. Parses
//! `diskutil info <device>` output into a typed [`DiskInfo`] record,
//! then gates the target against aegis-boot's safety invariants
//! before any destructive subprocess runs.
//!
//! Checks:
//!
//! 1. **Refuse `disk0`.** Already enforced at the partition-plan
//!    layer ([`super::partition::build_diskutil_plan`]); repeated
//!    here so the pipeline's preflight stage fails with a distinct
//!    error variant before the partitioner even gets called.
//! 2. **Whole-disk only.** A partition identifier (`disk5s1`) is
//!    refused — the flash pipeline operates on the whole device.
//! 3. **Removable OR external.** Internal non-removable drives are
//!    refused. `Removable Media: Removable` OR `Device Location:
//!    External` is enough to accept (thumb drives are sometimes
//!    "Fixed" but "External" — both get through).
//! 4. **Minimum size gate.** Sticks smaller than ~1 GiB couldn't
//!    fit the signed chain + any usable ISO. A pre-destructive
//!    rejection is friendlier than a mid-partition "no space left."
//!
//! ## Why plain-text parsing (not `-plist`)
//!
//! `diskutil info -plist <device>` emits an Apple plist XML that's
//! the "stable contract" for scripted consumers. Parsing it would
//! mean adding a `plist` crate dep — ~1 MiB of compile-time weight
//! for a build-tool we invoke once per flash. The plain-text
//! output is English-only but Apple has kept its field names
//! stable for ~15 years; we only read 6 specific keys and refuse
//! cleanly (`MissingField`) on any absent one, so a future rename
//! surfaces as an actionable error rather than a silent acceptance.
//!
//! [#418]: https://github.com/aegis-boot/aegis-boot/issues/418

// Phase 4a lands the pure-fn parser + classifier + cfg-gated
// subprocess wrapper ahead of Phase 4b's pipeline composer. The
// unit-test suite exercises every public symbol, so regressions
// still surface at CI time.
#![allow(dead_code)]

/// Minimum total disk size we accept for a flash target. Smaller
/// than a 1 GiB stick can't hold the signed chain + any practical
/// ISO — reject before partitioning rather than mid-write.
pub(crate) const MIN_FLASH_TARGET_SIZE_BYTES: u64 = 1_073_741_824; // 1 GiB

/// Parsed subset of `diskutil info <device>` output. Only the fields
/// the preflight stage actually inspects are retained; anything
/// else the command prints is discarded to keep the contract small.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiskInfo {
    /// Bare device identifier (e.g. `"disk5"`). Matches the value
    /// [`super::partition::build_diskutil_plan`] would accept.
    pub device_id: String,
    /// True iff `diskutil info` reported `Whole: Yes`. A partition
    /// (`disk5s1`) has `Whole: No`.
    pub whole_disk: bool,
    /// True iff `Removable Media: Removable` (also accepts other
    /// removability markers like `Hot-pluggable`).
    pub removable: bool,
    /// True iff `Device Location: External`. Some thumb drives are
    /// "Fixed" in the removable axis but "External" in the location
    /// axis; either gets through the preflight gate.
    pub external: bool,
    /// Disk size in bytes, parsed from the `Disk Size:` line's
    /// parenthesized byte-count.
    pub size_bytes: u64,
    /// `Device / Media Name:` field — display-only, operator can
    /// cross-reference with what's printed on the stick.
    pub media_name: String,
}

/// Why `diskutil info` output couldn't be parsed into a
/// [`DiskInfo`]. Distinguishes "format drifted" (missing field)
/// from "field was there but unparsable" so a future localization
/// or `diskutil` upgrade can be narrowed quickly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DiskInfoParseError {
    /// A required key wasn't anywhere in the output. Typically a
    /// localized / non-English `diskutil`, or the target was
    /// invalid and the command returned a single-line error.
    MissingField {
        /// The exact key we looked for (e.g. `"Device Identifier"`).
        field: &'static str,
    },
    /// The field was present but the value didn't match the expected
    /// shape (e.g. a non-numeric byte count in `Disk Size:`).
    MalformedValue {
        /// The field whose value failed to parse.
        field: &'static str,
        /// The raw value, echoed for operator log-grep.
        value: String,
    },
}

impl std::fmt::Display for DiskInfoParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingField { field } => write!(
                f,
                "diskutil info output missing required field {field:?} — the macOS version may have renamed it, or diskutil returned an error line instead of a record"
            ),
            Self::MalformedValue { field, value } => write!(
                f,
                "diskutil info field {field:?} had unexpected value {value:?}"
            ),
        }
    }
}

/// Why a target failed the preflight safety gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RefusalReason {
    /// Caller passed `disk0` — the host boot drive on macOS. Caught
    /// by the partition layer too; repeated here so the pipeline's
    /// preflight stage fails with a distinct error variant before
    /// the partitioner is called at all.
    BootDisk,
    /// Target is a partition (e.g. `disk5s1`), not a whole disk.
    /// The flash pipeline operates on the whole device so it can
    /// rewrite the partition table.
    NotWholeDisk {
        /// The offending device identifier.
        device_id: String,
    },
    /// Target is internal + non-removable. A brand-new
    /// `aegis-boot flash --direct-install` on a user's SSD would
    /// wipe their OS — refuse on shape alone.
    NotRemovableOrExternal {
        /// Device identifier we checked.
        device_id: String,
    },
    /// Target is smaller than [`MIN_FLASH_TARGET_SIZE_BYTES`].
    TooSmall {
        /// Bytes reported by `diskutil info`.
        got: u64,
        /// Minimum we accept.
        minimum: u64,
    },
}

impl std::fmt::Display for RefusalReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BootDisk => write!(
                f,
                "refusing /dev/disk0 — that's virtually always the macOS internal boot drive"
            ),
            Self::NotWholeDisk { device_id } => write!(
                f,
                "{device_id} is a partition, not a whole disk — pass the parent identifier (e.g. `disk5`, not `disk5s1`)"
            ),
            Self::NotRemovableOrExternal { device_id } => write!(
                f,
                "{device_id} is not removable or external — aegis-boot refuses to flash an internal drive"
            ),
            Self::TooSmall { got, minimum } => write!(
                f,
                "target is only {got} bytes; aegis-boot needs at least {minimum} bytes"
            ),
        }
    }
}

/// Argv for `diskutil info <device>`. `-plist` deliberately omitted
/// — see module-level comment on why we parse plain text.
#[must_use]
pub(crate) fn build_diskutil_info_argv(device_id: &str) -> Vec<String> {
    vec!["info".to_string(), device_id.to_string()]
}

/// Parse `diskutil info` plain-text output into a [`DiskInfo`].
///
/// Key fields we look for (left column is exact; matching is on
/// the `key:` stem, anything after the colon is the value):
///
/// * `Device Identifier`
/// * `Whole`  (expected `Yes` / `No`)
/// * `Removable Media`  (accepted forms: `Removable`, `Hot-pluggable`)
/// * `Device Location`  (accepted form for external: `External`)
/// * `Device / Media Name`  (display only)
/// * `Disk Size`  (expected form with `(NNNN Bytes)` suffix)
///
/// # Errors
///
/// [`DiskInfoParseError::MissingField`] if any key isn't present;
/// [`DiskInfoParseError::MalformedValue`] if `Disk Size` lacks the
/// expected byte-count parenthetical.
pub(crate) fn parse_diskutil_info(output: &str) -> Result<DiskInfo, DiskInfoParseError> {
    let device_id = find_field(output, "Device Identifier")?;
    let whole_raw = find_field(output, "Whole")?;
    let removable_raw = find_field(output, "Removable Media")?;
    let location_raw = find_field(output, "Device Location")?;
    let media_name = find_field(output, "Device / Media Name").unwrap_or_default();
    let size_raw = find_field(output, "Disk Size")?;

    let whole_disk = whole_raw.eq_ignore_ascii_case("Yes");
    let removable = removable_raw.eq_ignore_ascii_case("Removable")
        || removable_raw.eq_ignore_ascii_case("Hot-pluggable");
    let external = location_raw.eq_ignore_ascii_case("External");
    let size_bytes = parse_size_bytes(&size_raw)?;

    Ok(DiskInfo {
        device_id,
        whole_disk,
        removable,
        external,
        size_bytes,
        media_name,
    })
}

/// Look up a `key: value` line in `diskutil info` output. Returns
/// the trimmed value or an error if the key doesn't appear.
fn find_field(output: &str, key: &'static str) -> Result<String, DiskInfoParseError> {
    for line in output.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix(key) {
            if let Some(value) = rest.strip_prefix(':') {
                return Ok(value.trim().to_string());
            }
        }
    }
    Err(DiskInfoParseError::MissingField { field: key })
}

/// Parse a `Disk Size:` value like `"15.6 GB (15569256448 Bytes)..."`
/// into a u64 byte-count. We extract the integer inside the first
/// `(NNNN Bytes)` parenthetical; anything before or after is
/// ignored.
fn parse_size_bytes(raw: &str) -> Result<u64, DiskInfoParseError> {
    // Look for the pattern `(<digits> Bytes)`. `diskutil` always
    // emits English "Bytes" regardless of locale for this specific
    // field (the human-readable prefix IS localized but the
    // parenthetical isn't, per Apple's documented convention).
    let start = raw
        .find('(')
        .ok_or_else(|| DiskInfoParseError::MalformedValue {
            field: "Disk Size",
            value: raw.to_string(),
        })?;
    let rest = &raw[start + 1..];
    let end = rest
        .find(" Bytes")
        .ok_or_else(|| DiskInfoParseError::MalformedValue {
            field: "Disk Size",
            value: raw.to_string(),
        })?;
    let digits = &rest[..end];
    digits
        .trim()
        .parse::<u64>()
        .map_err(|_| DiskInfoParseError::MalformedValue {
            field: "Disk Size",
            value: raw.to_string(),
        })
}

/// Classify a parsed [`DiskInfo`] against aegis-boot's preflight
/// safety invariants. Returns `Ok(())` on an acceptable target.
///
/// # Errors
///
/// Returns a [`RefusalReason`] — each variant surfaces a distinct
/// operator-facing error variant.
pub(crate) fn classify(info: &DiskInfo) -> Result<(), RefusalReason> {
    if info.device_id == "disk0" {
        return Err(RefusalReason::BootDisk);
    }
    if !info.whole_disk {
        return Err(RefusalReason::NotWholeDisk {
            device_id: info.device_id.clone(),
        });
    }
    if !(info.removable || info.external) {
        return Err(RefusalReason::NotRemovableOrExternal {
            device_id: info.device_id.clone(),
        });
    }
    if info.size_bytes < MIN_FLASH_TARGET_SIZE_BYTES {
        return Err(RefusalReason::TooSmall {
            got: info.size_bytes,
            minimum: MIN_FLASH_TARGET_SIZE_BYTES,
        });
    }
    Ok(())
}

/// Run `diskutil info <device>` + classify. macOS-only; the
/// `#[cfg]` gate keeps Linux + Windows builds dep-free while
/// `x86_64-apple-darwin` CI cross-compile catches type errors.
///
/// # Errors
///
/// Returns a descriptive `String` folding subprocess, parse, and
/// classification errors.
#[cfg(target_os = "macos")]
pub(crate) fn preflight_diskutil(device_id: &str) -> Result<DiskInfo, String> {
    use std::process::Command;

    let argv = build_diskutil_info_argv(device_id);
    let out = Command::new("/usr/sbin/diskutil")
        .args(&argv)
        .output()
        .map_err(|e| format!("spawn /usr/sbin/diskutil: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "diskutil info exited {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let info = parse_diskutil_info(&String::from_utf8_lossy(&out.stdout))
        .map_err(|e| format!("parse diskutil info output: {e}"))?;
    if let Err(reason) = classify(&info) {
        return Err(format!("preflight refused: {reason}"));
    }
    Ok(info)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// Sample output for a healthy 16 GB external USB thumb drive.
    /// Pinned so a future `diskutil` format change fails a test
    /// rather than silently re-interpreting live output.
    const SAMPLE_EXTERNAL_THUMB: &str = "\
   Device Identifier:         disk5
   Device Node:               /dev/disk5
   Whole:                     Yes
   Part of Whole:             disk5

   Device / Media Name:       SanDisk Cruzer Blade

   Volume Name:               Not applicable (no file system)
   Mounted:                   Not applicable (no file system)

   File System:               None

   Device Location:           External
   Removable Media:           Removable

   Disk Size:                 15.6 GB (15569256448 Bytes) (exactly 30408704 512-Byte-Units)
   Device Block Size:         512 Bytes
";

    /// Sample output for an internal boot SSD on a modern Mac. Same
    /// field format, different values — whole disk + not removable.
    const SAMPLE_INTERNAL_SSD: &str = "\
   Device Identifier:         disk0
   Device Node:               /dev/disk0
   Whole:                     Yes

   Device / Media Name:       APPLE SSD AP0512R

   Device Location:           Internal
   Removable Media:           Fixed

   Disk Size:                 500.3 GB (500277790720 Bytes) (exactly 977105060 512-Byte-Units)
";

    /// Sample output for a partition slice, not a whole disk.
    const SAMPLE_PARTITION: &str = "\
   Device Identifier:         disk5s1
   Device Node:               /dev/disk5s1
   Whole:                     No
   Part of Whole:             disk5

   Device / Media Name:       EFI

   Device Location:           External
   Removable Media:           Removable

   Disk Size:                 400.0 MB (419430400 Bytes) (exactly 819200 512-Byte-Units)
";

    #[test]
    fn parses_external_thumb() {
        let info = parse_diskutil_info(SAMPLE_EXTERNAL_THUMB).expect("parse ok");
        assert_eq!(info.device_id, "disk5");
        assert!(info.whole_disk);
        assert!(info.removable);
        assert!(info.external);
        assert_eq!(info.size_bytes, 15_569_256_448);
        assert_eq!(info.media_name, "SanDisk Cruzer Blade");
    }

    #[test]
    fn parses_internal_ssd() {
        let info = parse_diskutil_info(SAMPLE_INTERNAL_SSD).expect("parse ok");
        assert_eq!(info.device_id, "disk0");
        assert!(info.whole_disk);
        assert!(!info.removable);
        assert!(!info.external);
        assert_eq!(info.size_bytes, 500_277_790_720);
    }

    #[test]
    fn parses_partition_identifier() {
        let info = parse_diskutil_info(SAMPLE_PARTITION).expect("parse ok");
        assert_eq!(info.device_id, "disk5s1");
        assert!(!info.whole_disk);
    }

    #[test]
    fn parse_rejects_truncated_output() {
        let err = parse_diskutil_info("   Device Identifier: disk5\n").unwrap_err();
        match err {
            DiskInfoParseError::MissingField { field } => {
                assert!(
                    ["Whole", "Removable Media", "Device Location", "Disk Size"].contains(&field),
                    "first missing field reported: {field}"
                );
            }
            DiskInfoParseError::MalformedValue { .. } => panic!("wrong variant: {err:?}"),
        }
    }

    #[test]
    fn parse_rejects_malformed_size() {
        let body = "\
   Device Identifier:         disk5
   Whole:                     Yes
   Device / Media Name:       X
   Device Location:           External
   Removable Media:           Removable
   Disk Size:                 15.6 GB (not a number Bytes)
";
        let err = parse_diskutil_info(body).unwrap_err();
        assert!(matches!(err, DiskInfoParseError::MalformedValue { .. }));
    }

    #[test]
    fn parse_accepts_hot_pluggable_as_removable() {
        // FireWire / eSATA devices historically emit "Hot-pluggable"
        // where USB thumb drives emit "Removable." The preflight
        // gate accepts both as removable. Replace only the value
        // column — `Removable Media:` is the field name that must
        // stay intact.
        let body = SAMPLE_EXTERNAL_THUMB.replace(
            "Removable Media:           Removable",
            "Removable Media:           Hot-pluggable",
        );
        let info = parse_diskutil_info(&body).expect("parse ok");
        assert!(info.removable, "Hot-pluggable must count as removable");
    }

    #[test]
    fn classify_accepts_healthy_external_thumb() {
        let info = parse_diskutil_info(SAMPLE_EXTERNAL_THUMB).unwrap();
        assert!(classify(&info).is_ok());
    }

    #[test]
    fn classify_refuses_boot_disk() {
        let info = parse_diskutil_info(SAMPLE_INTERNAL_SSD).unwrap();
        // disk0 fails on the first check regardless of other fields
        match classify(&info) {
            Err(RefusalReason::BootDisk) => {}
            other => panic!("wrong refusal: {other:?}"),
        }
    }

    #[test]
    fn classify_refuses_non_boot_internal_drive() {
        // Same shape as SAMPLE_INTERNAL_SSD but with a device_id
        // that isn't disk0 — e.g. a second internal SSD on a Mac
        // Pro. The NotRemovableOrExternal variant is the right
        // refusal here, not BootDisk.
        let body = SAMPLE_INTERNAL_SSD.replace("disk0", "disk2");
        let info = parse_diskutil_info(&body).unwrap();
        match classify(&info) {
            Err(RefusalReason::NotRemovableOrExternal { device_id }) => {
                assert_eq!(device_id, "disk2");
            }
            other => panic!("wrong refusal: {other:?}"),
        }
    }

    #[test]
    fn classify_refuses_partition() {
        let info = parse_diskutil_info(SAMPLE_PARTITION).unwrap();
        match classify(&info) {
            Err(RefusalReason::NotWholeDisk { device_id }) => {
                assert_eq!(device_id, "disk5s1");
            }
            other => panic!("wrong refusal: {other:?}"),
        }
    }

    #[test]
    fn classify_refuses_too_small() {
        let mut info = parse_diskutil_info(SAMPLE_EXTERNAL_THUMB).unwrap();
        info.size_bytes = 500 * 1024 * 1024; // 500 MiB
        match classify(&info) {
            Err(RefusalReason::TooSmall { got, minimum }) => {
                assert_eq!(got, 500 * 1024 * 1024);
                assert_eq!(minimum, MIN_FLASH_TARGET_SIZE_BYTES);
            }
            other => panic!("wrong refusal: {other:?}"),
        }
    }

    #[test]
    fn classify_accepts_external_even_when_not_removable() {
        // USB-C drives on the 2022+ MacBook Pros are sometimes
        // reported as "Fixed" + "External" — the external marker
        // alone should be enough to accept.
        let mut info = parse_diskutil_info(SAMPLE_EXTERNAL_THUMB).unwrap();
        info.removable = false;
        // `external` stays true from the sample.
        assert!(classify(&info).is_ok());
    }

    #[test]
    fn classify_refuses_when_neither_removable_nor_external() {
        let mut info = parse_diskutil_info(SAMPLE_EXTERNAL_THUMB).unwrap();
        info.removable = false;
        info.external = false;
        assert!(matches!(
            classify(&info),
            Err(RefusalReason::NotRemovableOrExternal { .. })
        ));
    }

    #[test]
    fn build_diskutil_info_argv_matches_shape() {
        assert_eq!(build_diskutil_info_argv("disk5"), vec!["info", "disk5"]);
    }

    #[test]
    fn parse_size_bytes_extracts_inner_parenthetical() {
        // Every field other than Disk Size is opaque to parse_size_bytes;
        // pin the inner parser behavior directly.
        assert_eq!(
            parse_size_bytes("15.6 GB (15569256448 Bytes) trailing").unwrap(),
            15_569_256_448
        );
        assert_eq!(
            parse_size_bytes("400.0 MB (419430400 Bytes)").unwrap(),
            419_430_400
        );
    }

    #[test]
    fn parse_size_bytes_rejects_non_numeric() {
        assert!(matches!(
            parse_size_bytes("foo (not a number Bytes)"),
            Err(DiskInfoParseError::MalformedValue { .. })
        ));
    }

    #[test]
    fn parse_size_bytes_rejects_missing_parenthetical() {
        assert!(matches!(
            parse_size_bytes("15.6 GB"),
            Err(DiskInfoParseError::MalformedValue { .. })
        ));
    }

    #[test]
    fn error_displays_echo_relevant_context() {
        let s = RefusalReason::BootDisk.to_string();
        assert!(s.contains("disk0"));
        let s = RefusalReason::NotWholeDisk {
            device_id: "disk5s1".to_string(),
        }
        .to_string();
        assert!(s.contains("disk5s1"));
        let s = RefusalReason::TooSmall {
            got: 500,
            minimum: 1024,
        }
        .to_string();
        assert!(s.contains("500"));
        assert!(s.contains("1024"));
    }
}
