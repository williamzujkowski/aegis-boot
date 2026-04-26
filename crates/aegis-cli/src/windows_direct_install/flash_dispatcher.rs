// SPDX-License-Identifier: MIT OR Apache-2.0

//! #497 piece 4 — Windows `aegis-boot flash --direct-install`
//! dispatcher. Composes drive enumeration + source resolution +
//! the pipeline composer into the operator-facing CLI entry path.
//!
//! ## Operator flow
//!
//! ```text
//! aegis-boot flash --direct-install <drive> --out-dir <path>
//! ```
//!
//! - `<drive>` accepts `1`, `PhysicalDrive1`, or `\\.\PhysicalDrive1`.
//!   Refusing to accept the raw `\\.\` prefix without escaping would
//!   force operators into `cmd /c` quoting gymnastics.
//! - `--out-dir` defaults to `./out` (matches the Linux path); contains
//!   the 6 signed-chain files per [`super::source_resolution`].
//!
//! If `<drive>` is omitted, the dispatcher calls
//! [`super::drive_enumeration::enumerate_flashable_drives`] and prints
//! the flashable candidates for the operator to choose from — then
//! exits with a hint to re-run with the chosen number. No interactive
//! prompt: the common path is `WinRM` / SSH-invoked-from-Linux, where
//! stdin is often closed, and a silent prompt hang is worse than a
//! clear "re-run with arg" message.
//!
//! ## Why a separate module from `flash.rs`
//!
//! The Linux `flash_direct_install` is tightly bound to `sgdisk` +
//! `mtools` + Debian-style signed-chain paths. Keeping Windows-side
//! composition in `windows_direct_install/` keeps the host-specific
//! code co-located with its phase modules and leaves `flash.rs` as
//! the thin dispatcher that picks a platform implementation.

#![allow(dead_code)]

use std::path::Path;
use std::time::Duration;

use crate::windows_direct_install::drive_enumeration::PhysicalDisk;
#[cfg(target_os = "windows")]
use crate::windows_direct_install::drive_enumeration::enumerate_flashable_drives;
#[cfg(target_os = "windows")]
use crate::windows_direct_install::pipeline::WindowsPhaseRunner;
use crate::windows_direct_install::pipeline::{
    DirectInstallError, DirectInstallPlan, DirectInstallReceipt, DirectInstallStage, PhaseRunner,
    run,
};
use crate::windows_direct_install::source_resolution::{
    SourceResolutionError, build_staging_sources,
};

/// Parse an operator-supplied drive identifier into the physical
/// disk number [`DirectInstallPlan`] takes. Accepts:
///
/// - `"1"` — bare integer
/// - `"PhysicalDrive1"` — Windows-convention without the `\\.\` prefix
/// - `r"\\.\PhysicalDrive1"` — full device path (escaped or not)
/// - `"disk1"` — Linux-habit alias (some operators carry this over
///   from `diskpart` `list disk` output which uses `Disk N`)
pub(crate) fn parse_drive_id(raw: &str) -> Result<u32, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("aegis-boot flash: drive identifier is empty".into());
    }
    // Strip common prefixes, case-insensitive.
    let lowered = trimmed.to_ascii_lowercase();
    let digit_start = lowered
        .strip_prefix(r"\\.\physicaldrive")
        .or_else(|| lowered.strip_prefix("physicaldrive"))
        .or_else(|| lowered.strip_prefix("disk"))
        .unwrap_or(&lowered);
    digit_start.parse::<u32>().map_err(|e| {
        format!(
            "aegis-boot flash: can't parse drive identifier {raw:?} as a physical-drive \
             number: {e}. Expected `1`, `PhysicalDrive1`, or `\\\\.\\PhysicalDrive1`."
        )
    })
}

/// Top-level dispatch error: either the plan couldn't be built
/// (source files missing, drive arg bogus) or the pipeline ran and
/// aborted at one of its six stages.
#[derive(Debug)]
pub(crate) enum DispatchError {
    /// `explicit_dev` was `None` — we enumerated candidates and want
    /// the operator to re-run with a specific choice. Not really an
    /// error as much as "we can't proceed non-interactively." Caller
    /// should render the candidate list and exit with a typical
    /// "usage" status (2).
    NeedsExplicitDrive(Vec<PhysicalDisk>),
    /// Drive arg supplied but unparsable.
    BadDriveArg(String),
    /// Source resolution failed (missing files, etc.).
    Sources(SourceResolutionError),
    /// Drive enumeration subprocess failed.
    Enumeration(String),
    /// Pipeline aborted at one of its stages; receipt reports which
    /// stages actually ran.
    Pipeline(Box<(DirectInstallError, DirectInstallReceipt)>),
}

impl std::fmt::Display for DispatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NeedsExplicitDrive(candidates) => {
                writeln!(
                    f,
                    "aegis-boot flash --direct-install: no drive specified. \
                     Candidates on this host:"
                )?;
                for d in candidates {
                    writeln!(
                        f,
                        "  PhysicalDrive{:<3} {:<24} {:>10}  [{}]",
                        d.number,
                        d.friendly_name,
                        d.human_size(),
                        d.partition_style,
                    )?;
                }
                write!(
                    f,
                    "Re-run with an explicit drive argument: \
                     `aegis-boot flash --direct-install <N>`"
                )
            }
            Self::BadDriveArg(detail) => write!(f, "{detail}"),
            Self::Sources(e) => write!(f, "{e}"),
            Self::Enumeration(detail) => {
                write!(f, "aegis-boot flash --direct-install: {detail}")
            }
            Self::Pipeline(err_and_receipt) => {
                let (err, receipt) = err_and_receipt.as_ref();
                writeln!(f, "aegis-boot flash --direct-install: {err}")?;
                write!(f, "{}", format_receipt(receipt))
            }
        }
    }
}

/// Format the partial-progress receipt into a multi-line
/// `Stage N done in XXXms` report matching the Linux flash path's
/// ending lines.
pub(crate) fn format_receipt(r: &DirectInstallReceipt) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let mut push = |stage: DirectInstallStage, d: Option<Duration>| {
        if let Some(dur) = d {
            let _ = writeln!(out, "  {:<22} {}", stage.name(), format_elapsed(dur));
        }
    };
    push(
        DirectInstallStage::PreflightElevation,
        r.preflight_elevation,
    );
    push(
        DirectInstallStage::PreflightBitLocker,
        r.preflight_bitlocker,
    );
    push(DirectInstallStage::Partition, r.partition);
    push(DirectInstallStage::FormatEsp, r.format_esp);
    push(DirectInstallStage::FormatData, r.format_data);
    push(DirectInstallStage::StageEsp, r.stage_esp);
    let _ = writeln!(out, "  {:<22} {}", "total", format_elapsed(r.total()));
    out
}

/// Human-readable elapsed time. Mirrors `flash::format_elapsed` —
/// kept private here so the Linux-only gate on that one doesn't leak
/// into the Windows dispatcher.
fn format_elapsed(d: Duration) -> String {
    let secs = d.as_secs_f64();
    if secs < 60.0 {
        format!("{secs:.1}s")
    } else {
        let mins = d.as_secs() / 60;
        let remaining = d.as_secs() % 60;
        format!("{mins}m {remaining:02}s")
    }
}

/// Shared pure core — runs the dispatcher against an injected
/// enumerator + `PhaseRunner` so tests don't need `PowerShell`. The
/// real `run_direct_install` on Windows supplies the production
/// enumerator + `WindowsPhaseRunner`.
pub(crate) fn run_direct_install_using<E>(
    explicit_dev: Option<&str>,
    out_dir: &Path,
    enumerate: E,
    runner: &dyn PhaseRunner,
) -> Result<DirectInstallReceipt, DispatchError>
where
    E: FnOnce() -> Result<Vec<PhysicalDisk>, String>,
{
    // Drive selection: parse explicit arg, or enumerate + bail with
    // the candidate list.
    let Some(raw) = explicit_dev else {
        let candidates = enumerate().map_err(DispatchError::Enumeration)?;
        return Err(DispatchError::NeedsExplicitDrive(candidates));
    };
    let drive_num = parse_drive_id(raw).map_err(DispatchError::BadDriveArg)?;

    // Source resolution.
    let sources = build_staging_sources(out_dir).map_err(DispatchError::Sources)?;
    let plan = DirectInstallPlan {
        physical_drive: drive_num,
        sources,
    };

    // Hand off to the composer; flatten Box-wrapped failure.
    run(runner, &plan).map_err(DispatchError::Pipeline)
}

/// Top-level entry for the Windows-only CLI dispatch. Compiles only
/// on Windows; the Linux / macOS builds skip this entirely by virtue
/// of the cfg gate on the caller.
#[cfg(target_os = "windows")]
pub(crate) fn run_direct_install(
    explicit_dev: Option<&str>,
    out_dir: &Path,
) -> Result<DirectInstallReceipt, DispatchError> {
    run_direct_install_using(
        explicit_dev,
        out_dir,
        enumerate_flashable_drives,
        &WindowsPhaseRunner,
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::cell::RefCell;

    use super::*;
    use crate::windows_direct_install::drive_enumeration::BusType;
    use crate::windows_direct_install::format::FormatTarget;
    use crate::windows_direct_install::preflight::BitLockerStatus;
    use crate::windows_direct_install::raw_write::EspStagingSources;

    struct MockRunner {
        calls: RefCell<Vec<String>>,
    }

    impl MockRunner {
        fn new() -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
            }
        }
    }

    impl PhaseRunner for MockRunner {
        fn is_running_as_admin(&self) -> Result<bool, String> {
            self.calls.borrow_mut().push("admin".into());
            Ok(true)
        }
        fn check_bitlocker_status(&self, _d: u32) -> Result<BitLockerStatus, String> {
            self.calls.borrow_mut().push("bitlocker".into());
            Ok(BitLockerStatus::FullyDecrypted)
        }
        fn partition_via_diskpart(&self, _d: u32) -> Result<(), String> {
            self.calls.borrow_mut().push("partition".into());
            Ok(())
        }
        fn format_partition(&self, _d: u32, _t: FormatTarget) -> Result<(), String> {
            self.calls.borrow_mut().push("format".into());
            Ok(())
        }
        fn stage_esp(&self, _d: u32, _s: &EspStagingSources) -> Result<(), String> {
            self.calls.borrow_mut().push("stage".into());
            Ok(())
        }
    }

    /// Populate a tempdir with the 6 default-named signed-chain files.
    /// Returns the tempdir handle (drop removes the dir).
    fn prepared_out_dir() -> tempfile::TempDir {
        use crate::windows_direct_install::source_resolution::ENV_DEFAULT_PAIRS;
        let dir = tempfile::tempdir().unwrap();
        for (_, filename) in ENV_DEFAULT_PAIRS {
            std::fs::write(dir.path().join(filename), b"stub").unwrap();
        }
        dir
    }

    #[test]
    fn parse_drive_id_accepts_bare_integer() {
        assert_eq!(parse_drive_id("1").unwrap(), 1);
        assert_eq!(parse_drive_id("42").unwrap(), 42);
    }

    #[test]
    fn parse_drive_id_accepts_physicaldrive_prefix() {
        assert_eq!(parse_drive_id("PhysicalDrive1").unwrap(), 1);
        assert_eq!(parse_drive_id("physicaldrive3").unwrap(), 3);
    }

    #[test]
    fn parse_drive_id_accepts_full_device_path() {
        assert_eq!(parse_drive_id(r"\\.\PhysicalDrive2").unwrap(), 2);
        assert_eq!(parse_drive_id(r"\\.\physicaldrive7").unwrap(), 7);
    }

    #[test]
    fn parse_drive_id_accepts_disk_alias() {
        assert_eq!(parse_drive_id("disk1").unwrap(), 1);
        assert_eq!(parse_drive_id("Disk5").unwrap(), 5);
    }

    #[test]
    fn parse_drive_id_rejects_empty() {
        assert!(parse_drive_id("").is_err());
        assert!(parse_drive_id("   ").is_err());
    }

    #[test]
    fn parse_drive_id_rejects_non_numeric() {
        assert!(parse_drive_id("PhysicalDrive").is_err());
        assert!(parse_drive_id("sda").is_err());
        assert!(parse_drive_id("C:").is_err());
    }

    #[test]
    fn run_direct_install_happy_path_invokes_every_phase() {
        let dir = prepared_out_dir();
        let runner = MockRunner::new();
        // Enumerator: shouldn't be called — explicit arg given.
        let receipt = run_direct_install_using(
            Some("1"),
            dir.path(),
            || panic!("enumerator must not run when explicit drive given"),
            &runner,
        )
        .expect("happy path should succeed");

        assert!(receipt.preflight_elevation.is_some());
        assert!(receipt.preflight_bitlocker.is_some());
        assert!(receipt.partition.is_some());
        assert!(receipt.format_esp.is_some());
        assert!(receipt.format_data.is_some());
        assert!(receipt.stage_esp.is_some());
        // 5 phase methods (format gets called twice for ESP + data).
        let calls = runner.calls.borrow().clone();
        assert_eq!(
            calls,
            vec![
                "admin",
                "bitlocker",
                "partition",
                "format",
                "format",
                "stage"
            ]
        );
    }

    #[test]
    fn run_direct_install_without_drive_returns_needs_explicit_with_candidates() {
        let dir = prepared_out_dir();
        let runner = MockRunner::new();
        let candidate = PhysicalDisk {
            number: 1,
            friendly_name: "SanDisk Cruzer".into(),
            size_bytes: 8 * 1024 * 1024 * 1024,
            bus_type: BusType::Usb,
            is_boot: false,
            is_system: false,
            is_offline: false,
            is_read_only: false,
            partition_style: "RAW".into(),
        };
        let err =
            run_direct_install_using(None, dir.path(), || Ok(vec![candidate.clone()]), &runner)
                .unwrap_err();

        match err {
            DispatchError::NeedsExplicitDrive(ref disks) => {
                assert_eq!(disks.len(), 1);
                assert_eq!(disks[0].number, 1);
            }
            _ => panic!("expected NeedsExplicitDrive, got {err:?}"),
        }
        // Runner must NOT have been invoked — no destructive action
        // fires when the operator hasn't picked a drive yet.
        assert!(runner.calls.borrow().is_empty());
    }

    #[test]
    fn run_direct_install_propagates_bad_drive_arg() {
        let dir = prepared_out_dir();
        let runner = MockRunner::new();
        let err = run_direct_install_using(
            Some("C:"),
            dir.path(),
            || panic!("enumerator must not run when parse fails"),
            &runner,
        )
        .unwrap_err();
        assert!(matches!(err, DispatchError::BadDriveArg(_)));
        assert!(runner.calls.borrow().is_empty());
    }

    #[test]
    fn run_direct_install_propagates_source_missing() {
        // Empty out_dir — all 6 files absent.
        let dir = tempfile::tempdir().unwrap();
        let runner = MockRunner::new();
        let err =
            run_direct_install_using(Some("1"), dir.path(), || Ok(vec![]), &runner).unwrap_err();
        assert!(matches!(err, DispatchError::Sources(_)));
        // Runner must not run — sources missing means no destructive
        // action fires.
        assert!(runner.calls.borrow().is_empty());
    }

    #[test]
    fn run_direct_install_wraps_enumeration_failure() {
        let dir = prepared_out_dir();
        let runner = MockRunner::new();
        let err = run_direct_install_using(
            None,
            dir.path(),
            || Err("spawn powershell: not found".into()),
            &runner,
        )
        .unwrap_err();
        assert!(matches!(err, DispatchError::Enumeration(_)));
    }

    #[test]
    fn format_receipt_includes_total_line() {
        let r = DirectInstallReceipt {
            preflight_elevation: Some(Duration::from_millis(50)),
            preflight_bitlocker: Some(Duration::from_millis(200)),
            partition: Some(Duration::from_secs(2)),
            format_esp: Some(Duration::from_millis(500)),
            format_data: Some(Duration::from_millis(500)),
            stage_esp: Some(Duration::from_millis(800)),
        };
        let out = format_receipt(&r);
        assert!(out.contains("preflight:elevation"));
        assert!(out.contains("stage_esp"));
        assert!(out.contains("total"));
    }

    #[test]
    fn format_receipt_skips_unrun_stages_but_still_prints_total() {
        // Partition failed — ESP + data + stage never ran.
        let r = DirectInstallReceipt {
            preflight_elevation: Some(Duration::from_millis(50)),
            preflight_bitlocker: Some(Duration::from_millis(200)),
            partition: Some(Duration::from_secs(2)),
            format_esp: None,
            format_data: None,
            stage_esp: None,
        };
        let out = format_receipt(&r);
        assert!(out.contains("partition:diskpart"));
        assert!(!out.contains("format:esp"));
        assert!(!out.contains("stage_esp"));
        assert!(out.contains("total"));
    }

    #[test]
    fn format_elapsed_formats_sub_minute_and_multi_minute() {
        assert_eq!(format_elapsed(Duration::from_millis(1500)), "1.5s");
        assert_eq!(format_elapsed(Duration::from_secs(125)), "2m 05s");
    }

    #[test]
    fn dispatch_error_display_for_needs_explicit_lists_candidates() {
        let err = DispatchError::NeedsExplicitDrive(vec![PhysicalDisk {
            number: 1,
            friendly_name: "SanDisk Cruzer".into(),
            size_bytes: 8 * 1024 * 1024 * 1024,
            bus_type: BusType::Usb,
            is_boot: false,
            is_system: false,
            is_offline: false,
            is_read_only: false,
            partition_style: "RAW".into(),
        }]);
        let s = format!("{err}");
        assert!(s.contains("no drive specified"));
        assert!(s.contains("PhysicalDrive1"));
        assert!(s.contains("SanDisk Cruzer"));
        assert!(s.contains("Re-run with an explicit drive argument"));
    }
}
