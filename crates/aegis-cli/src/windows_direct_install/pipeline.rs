// SPDX-License-Identifier: MIT OR Apache-2.0

//! #483 — Windows `--direct-install` pipeline composer.
//!
//! Wires the four Phase modules of [epic
//! #419](https://github.com/aegis-boot/aegis-boot/issues/419) into a
//! single runnable sequence, same shape as Linux's
//! `flash_direct_install`:
//!
//! 1. **Preflight** — `preflight::is_running_as_admin()` +
//!    `preflight::check_bitlocker_status(drive)`. Aborts before any
//!    destructive action if elevation missing or `BitLocker` on.
//! 2. **Partition** — `partition::partition_via_diskpart(drive)`.
//! 3. **Format ESP** — `format::format_partition(drive, Esp)`.
//! 4. **Format `AEGIS_ISOS`** — `format::format_partition(drive, AegisIsos)`.
//! 5. **Stage ESP** — `raw_write::stage_esp(drive, sources)`.
//!
//! ## Design notes
//!
//! The phase dispatch is routed through a trait — [`PhaseRunner`] —
//! so the composition logic itself unit-tests cleanly on any host
//! (the Windows-only subprocess + syscall paths stay behind the
//! default [`WindowsPhaseRunner`] impl). Mocks in tests can fail any
//! stage on demand and the composer's abort-cascade + per-stage
//! timing logic still exercises without a Windows VM.
//!
//! ## What this module deliberately does NOT do
//!
//! - Drive selection / enumeration (that's the CLI-dispatch side).
//! - Source-path resolution for the 6 signed-chain files (the
//!   Linux flash path hardcodes `/usr/share/aegis-boot/...`; the
//!   Windows equivalent will be determined by the `AEGIS_BOOT_OUT_DIR`
//!   the operator points at, tracked as follow-up).
//! - Attestation manifest writing (Linux-specific today; Windows
//!   equivalent is a separate follow-up once the signing-key lifecycle
//!   supports non-Linux hosts).
//!
//! These are scoped out because #483 is the composer, not the full
//! operator-facing CLI. See the issue body for the layering.

#![allow(dead_code)]

use std::time::{Duration, Instant};

use crate::windows_direct_install::format::FormatTarget;
use crate::windows_direct_install::preflight::BitLockerStatus;
use crate::windows_direct_install::raw_write::EspStagingSources;

/// Which stage of the pipeline reported the error. Mirrors the
/// Linux-side `DirectInstallStage` enum in `flash.rs` so operator
/// messages read the same regardless of host OS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirectInstallStage {
    PreflightElevation,
    PreflightBitLocker,
    Partition,
    FormatEsp,
    FormatData,
    StageEsp,
}

impl DirectInstallStage {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::PreflightElevation => "preflight:elevation",
            Self::PreflightBitLocker => "preflight:bitlocker",
            Self::Partition => "partition:diskpart",
            Self::FormatEsp => "format:esp",
            Self::FormatData => "format:aegis_isos",
            Self::StageEsp => "stage_esp",
        }
    }
}

/// Error returned by the pipeline. Always carries the stage that
/// produced it so the CLI can print `Stage 3 (format:esp) failed: …`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DirectInstallError {
    pub(crate) stage: DirectInstallStage,
    pub(crate) detail: String,
}

impl std::fmt::Display for DirectInstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.stage.name(), self.detail)
    }
}

/// Elapsed time per stage that ran. Stages that were skipped (because
/// an earlier stage failed) don't appear. The caller can format these
/// into the same `Stage N done in XXXms` lines the Linux path emits.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct DirectInstallReceipt {
    pub(crate) preflight_elevation: Option<Duration>,
    pub(crate) preflight_bitlocker: Option<Duration>,
    pub(crate) partition: Option<Duration>,
    pub(crate) format_esp: Option<Duration>,
    pub(crate) format_data: Option<Duration>,
    pub(crate) stage_esp: Option<Duration>,
}

impl DirectInstallReceipt {
    /// Total elapsed across all completed stages — matches the
    /// "total elapsed" line Linux prints at the end.
    pub(crate) fn total(&self) -> Duration {
        [
            self.preflight_elevation,
            self.preflight_bitlocker,
            self.partition,
            self.format_esp,
            self.format_data,
            self.stage_esp,
        ]
        .into_iter()
        .flatten()
        .sum()
    }
}

/// All inputs the pipeline needs. The caller (CLI dispatch) builds
/// this from the operator-supplied drive argument + signed-chain
/// source resolution; the pipeline itself is purely reactive.
#[derive(Debug, Clone)]
pub(crate) struct DirectInstallPlan {
    pub(crate) physical_drive: u32,
    pub(crate) sources: EspStagingSources,
}

/// Indirection for the 5 destructive phase dispatches so the
/// composer is testable without a Windows VM. The default
/// [`WindowsPhaseRunner`] wires each method straight through to the
/// phase module; tests supply a mock that returns canned results and
/// records the call order.
pub(crate) trait PhaseRunner {
    fn is_running_as_admin(&self) -> Result<bool, String>;
    fn check_bitlocker_status(&self, physical_drive: u32) -> Result<BitLockerStatus, String>;
    fn partition_via_diskpart(&self, physical_drive: u32) -> Result<(), String>;
    fn format_partition(&self, physical_drive: u32, target: FormatTarget) -> Result<(), String>;
    fn stage_esp(&self, physical_drive: u32, sources: &EspStagingSources) -> Result<(), String>;
}

/// Production phase runner — dispatches each method to the module
/// that owns the real Win32 / PowerShell subprocess work. Compiles
/// only on Windows; other hosts route through a mock (tests) or
/// refuse to build the CLI dispatcher (see `flash.rs` gating).
#[cfg(target_os = "windows")]
pub(crate) struct WindowsPhaseRunner;

#[cfg(target_os = "windows")]
impl PhaseRunner for WindowsPhaseRunner {
    fn is_running_as_admin(&self) -> Result<bool, String> {
        crate::windows_direct_install::preflight::is_running_as_admin()
    }

    fn check_bitlocker_status(&self, physical_drive: u32) -> Result<BitLockerStatus, String> {
        crate::windows_direct_install::preflight::check_bitlocker_status(physical_drive)
    }

    fn partition_via_diskpart(&self, physical_drive: u32) -> Result<(), String> {
        crate::windows_direct_install::partition::partition_via_diskpart(physical_drive)
    }

    fn format_partition(&self, physical_drive: u32, target: FormatTarget) -> Result<(), String> {
        crate::windows_direct_install::format::format_partition(physical_drive, target)
    }

    fn stage_esp(&self, physical_drive: u32, sources: &EspStagingSources) -> Result<(), String> {
        crate::windows_direct_install::raw_write::stage_esp(physical_drive, sources)
    }
}

/// Error payload + partial-progress receipt bundled so the `Err`
/// variant stays at pointer size (Clippy `result_large_err` was
/// otherwise triggered by the 6-field receipt inline).
pub(crate) type RunFailure = Box<(DirectInstallError, DirectInstallReceipt)>;

/// Run the full pipeline. Returns a receipt on success; on failure,
/// the receipt in the boxed payload reports which stages actually
/// ran (useful for post-mortem: "Partition took 3s, format ESP failed
/// at 1.2s — so 4.2s total burned before the abort").
// Linear stage dispatcher — each stage's error path records a timing
// and boxes a failure. Splitting into helpers would fragment the
// linear "stage runs → record timing → propagate on error" pattern.
#[allow(clippy::too_many_lines)]
pub(crate) fn run(
    runner: &dyn PhaseRunner,
    plan: &DirectInstallPlan,
) -> Result<DirectInstallReceipt, RunFailure> {
    let mut receipt = DirectInstallReceipt::default();

    // Preflight — stage 1: elevation.
    let t = Instant::now();
    match runner.is_running_as_admin() {
        Ok(true) => receipt.preflight_elevation = Some(t.elapsed()),
        Ok(false) => {
            receipt.preflight_elevation = Some(t.elapsed());
            return Err(Box::new((
                DirectInstallError {
                    stage: DirectInstallStage::PreflightElevation,
                    detail: crate::windows_direct_install::preflight::elevation_required_message()
                        .to_string(),
                },
                receipt,
            )));
        }
        Err(detail) => {
            receipt.preflight_elevation = Some(t.elapsed());
            return Err(Box::new((
                DirectInstallError {
                    stage: DirectInstallStage::PreflightElevation,
                    detail,
                },
                receipt,
            )));
        }
    }

    // Preflight — stage 2: BitLocker. FullyDecrypted is the only pass.
    let t = Instant::now();
    match runner.check_bitlocker_status(plan.physical_drive) {
        Ok(BitLockerStatus::FullyDecrypted) => {
            receipt.preflight_bitlocker = Some(t.elapsed());
        }
        Ok(other) => {
            receipt.preflight_bitlocker = Some(t.elapsed());
            return Err(Box::new((
                DirectInstallError {
                    stage: DirectInstallStage::PreflightBitLocker,
                    detail: format!(
                        "{} (status: {:?})",
                        crate::windows_direct_install::preflight::bitlocker_protected_message(
                            plan.physical_drive
                        ),
                        other,
                    ),
                },
                receipt,
            )));
        }
        Err(detail) => {
            receipt.preflight_bitlocker = Some(t.elapsed());
            return Err(Box::new((
                DirectInstallError {
                    stage: DirectInstallStage::PreflightBitLocker,
                    detail,
                },
                receipt,
            )));
        }
    }

    // Partition.
    let t = Instant::now();
    if let Err(detail) = runner.partition_via_diskpart(plan.physical_drive) {
        receipt.partition = Some(t.elapsed());
        return Err(Box::new((
            DirectInstallError {
                stage: DirectInstallStage::Partition,
                detail,
            },
            receipt,
        )));
    }
    receipt.partition = Some(t.elapsed());

    // Format ESP.
    let t = Instant::now();
    if let Err(detail) = runner.format_partition(plan.physical_drive, FormatTarget::Esp) {
        receipt.format_esp = Some(t.elapsed());
        return Err(Box::new((
            DirectInstallError {
                stage: DirectInstallStage::FormatEsp,
                detail,
            },
            receipt,
        )));
    }
    receipt.format_esp = Some(t.elapsed());

    // Format AEGIS_ISOS.
    let t = Instant::now();
    if let Err(detail) = runner.format_partition(plan.physical_drive, FormatTarget::AegisIsos) {
        receipt.format_data = Some(t.elapsed());
        return Err(Box::new((
            DirectInstallError {
                stage: DirectInstallStage::FormatData,
                detail,
            },
            receipt,
        )));
    }
    receipt.format_data = Some(t.elapsed());

    // Stage ESP.
    let t = Instant::now();
    if let Err(detail) = runner.stage_esp(plan.physical_drive, &plan.sources) {
        receipt.stage_esp = Some(t.elapsed());
        return Err(Box::new((
            DirectInstallError {
                stage: DirectInstallStage::StageEsp,
                detail,
            },
            receipt,
        )));
    }
    receipt.stage_esp = Some(t.elapsed());

    Ok(receipt)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::cell::RefCell;
    use std::path::PathBuf;

    use super::*;

    /// Script of canned results each phase method returns, in call
    /// order. Panics if exhausted — the test expected that method
    /// to be called fewer times than it was.
    #[derive(Default)]
    struct MockRunner {
        calls: RefCell<Vec<String>>,
        is_admin: RefCell<Vec<Result<bool, String>>>,
        bitlocker: RefCell<Vec<Result<BitLockerStatus, String>>>,
        partition: RefCell<Vec<Result<(), String>>>,
        format: RefCell<Vec<Result<(), String>>>,
        stage: RefCell<Vec<Result<(), String>>>,
    }

    impl MockRunner {
        fn all_ok() -> Self {
            Self {
                is_admin: RefCell::new(vec![Ok(true)]),
                bitlocker: RefCell::new(vec![Ok(BitLockerStatus::FullyDecrypted)]),
                partition: RefCell::new(vec![Ok(())]),
                format: RefCell::new(vec![Ok(()), Ok(())]), // ESP + AegisIsos
                stage: RefCell::new(vec![Ok(())]),
                ..Default::default()
            }
        }
    }

    impl PhaseRunner for MockRunner {
        fn is_running_as_admin(&self) -> Result<bool, String> {
            self.calls.borrow_mut().push("is_running_as_admin".into());
            self.is_admin.borrow_mut().remove(0)
        }
        fn check_bitlocker_status(&self, drive: u32) -> Result<BitLockerStatus, String> {
            self.calls
                .borrow_mut()
                .push(format!("check_bitlocker_status({drive})"));
            self.bitlocker.borrow_mut().remove(0)
        }
        fn partition_via_diskpart(&self, drive: u32) -> Result<(), String> {
            self.calls
                .borrow_mut()
                .push(format!("partition_via_diskpart({drive})"));
            self.partition.borrow_mut().remove(0)
        }
        fn format_partition(&self, drive: u32, target: FormatTarget) -> Result<(), String> {
            self.calls
                .borrow_mut()
                .push(format!("format_partition({drive}, {target:?})"));
            self.format.borrow_mut().remove(0)
        }
        fn stage_esp(&self, drive: u32, _sources: &EspStagingSources) -> Result<(), String> {
            self.calls.borrow_mut().push(format!("stage_esp({drive})"));
            self.stage.borrow_mut().remove(0)
        }
    }

    fn sample_plan() -> DirectInstallPlan {
        DirectInstallPlan {
            physical_drive: 1,
            sources: EspStagingSources {
                shim_x64: PathBuf::from("/tmp/shim"),
                grub_x64: PathBuf::from("/tmp/grub"),
                mm_x64: PathBuf::from("/tmp/mm"),
                grub_cfg: PathBuf::from("/tmp/grub.cfg"),
                vmlinuz: PathBuf::from("/tmp/vmlinuz"),
                initramfs: PathBuf::from("/tmp/initramfs"),
            },
        }
    }

    #[test]
    fn run_happy_path_invokes_all_six_phase_calls_in_order() {
        let runner = MockRunner::all_ok();
        let plan = sample_plan();
        let receipt = run(&runner, &plan).expect("happy path should succeed");

        let calls = runner.calls.borrow().clone();
        assert_eq!(
            calls,
            vec![
                "is_running_as_admin".to_string(),
                "check_bitlocker_status(1)".to_string(),
                "partition_via_diskpart(1)".to_string(),
                "format_partition(1, Esp)".to_string(),
                "format_partition(1, AegisIsos)".to_string(),
                "stage_esp(1)".to_string(),
            ]
        );
        // All six stages ran → all six timings populated.
        assert!(receipt.preflight_elevation.is_some());
        assert!(receipt.preflight_bitlocker.is_some());
        assert!(receipt.partition.is_some());
        assert!(receipt.format_esp.is_some());
        assert!(receipt.format_data.is_some());
        assert!(receipt.stage_esp.is_some());
    }

    #[test]
    fn run_aborts_at_elevation_when_not_admin() {
        let runner = MockRunner {
            is_admin: RefCell::new(vec![Ok(false)]),
            ..Default::default()
        };
        let (err, receipt) = *run(&runner, &sample_plan()).unwrap_err();
        assert_eq!(err.stage, DirectInstallStage::PreflightElevation);
        // Only the elevation check ran.
        assert_eq!(runner.calls.borrow().len(), 1);
        assert!(receipt.preflight_elevation.is_some());
        assert!(receipt.preflight_bitlocker.is_none());
        assert!(receipt.partition.is_none());
    }

    #[test]
    fn run_aborts_at_bitlocker_when_protected() {
        let runner = MockRunner {
            is_admin: RefCell::new(vec![Ok(true)]),
            bitlocker: RefCell::new(vec![Ok(BitLockerStatus::Protected)]),
            ..Default::default()
        };
        let (err, receipt) = *run(&runner, &sample_plan()).unwrap_err();
        assert_eq!(err.stage, DirectInstallStage::PreflightBitLocker);
        assert!(err.detail.contains("BitLocker"));
        // Two calls: is_running_as_admin + check_bitlocker_status.
        assert_eq!(runner.calls.borrow().len(), 2);
        assert!(receipt.partition.is_none(), "partition must not run");
    }

    #[test]
    fn run_aborts_at_bitlocker_when_unknown() {
        // Fail-closed: Unknown is treated the same as Protected — no
        // destructive actions run.
        let runner = MockRunner {
            is_admin: RefCell::new(vec![Ok(true)]),
            bitlocker: RefCell::new(vec![Ok(BitLockerStatus::Unknown)]),
            ..Default::default()
        };
        let (err, _) = *run(&runner, &sample_plan()).unwrap_err();
        assert_eq!(err.stage, DirectInstallStage::PreflightBitLocker);
        assert!(err.detail.contains("Unknown"));
    }

    #[test]
    fn run_aborts_at_partition_when_diskpart_fails() {
        let runner = MockRunner {
            is_admin: RefCell::new(vec![Ok(true)]),
            bitlocker: RefCell::new(vec![Ok(BitLockerStatus::FullyDecrypted)]),
            partition: RefCell::new(vec![Err("diskpart exited 5".into())]),
            ..Default::default()
        };
        let (err, receipt) = *run(&runner, &sample_plan()).unwrap_err();
        assert_eq!(err.stage, DirectInstallStage::Partition);
        assert!(err.detail.contains("diskpart"));
        // Format + stage must not run.
        assert!(receipt.format_esp.is_none());
        assert!(receipt.format_data.is_none());
        assert!(receipt.stage_esp.is_none());
    }

    #[test]
    fn run_aborts_at_format_esp_leaves_data_and_stage_unrun() {
        let runner = MockRunner {
            is_admin: RefCell::new(vec![Ok(true)]),
            bitlocker: RefCell::new(vec![Ok(BitLockerStatus::FullyDecrypted)]),
            partition: RefCell::new(vec![Ok(())]),
            format: RefCell::new(vec![Err("Format-Volume denied".into())]),
            ..Default::default()
        };
        let (err, receipt) = *run(&runner, &sample_plan()).unwrap_err();
        assert_eq!(err.stage, DirectInstallStage::FormatEsp);
        assert!(receipt.partition.is_some());
        assert!(receipt.format_esp.is_some());
        assert!(receipt.format_data.is_none());
        assert!(receipt.stage_esp.is_none());
    }

    #[test]
    fn run_aborts_at_format_data_leaves_stage_unrun() {
        let runner = MockRunner {
            is_admin: RefCell::new(vec![Ok(true)]),
            bitlocker: RefCell::new(vec![Ok(BitLockerStatus::FullyDecrypted)]),
            partition: RefCell::new(vec![Ok(())]),
            format: RefCell::new(vec![Ok(()), Err("exFAT format failed".into())]),
            ..Default::default()
        };
        let (err, receipt) = *run(&runner, &sample_plan()).unwrap_err();
        assert_eq!(err.stage, DirectInstallStage::FormatData);
        assert!(receipt.format_esp.is_some());
        assert!(receipt.format_data.is_some());
        assert!(receipt.stage_esp.is_none());
    }

    #[test]
    fn run_reports_stage_esp_failure_with_all_prior_timings() {
        let runner = MockRunner {
            is_admin: RefCell::new(vec![Ok(true)]),
            bitlocker: RefCell::new(vec![Ok(BitLockerStatus::FullyDecrypted)]),
            partition: RefCell::new(vec![Ok(())]),
            format: RefCell::new(vec![Ok(()), Ok(())]),
            stage: RefCell::new(vec![Err("WriteFile failed: 32".into())]),
            ..Default::default()
        };
        let (err, receipt) = *run(&runner, &sample_plan()).unwrap_err();
        assert_eq!(err.stage, DirectInstallStage::StageEsp);
        // All 5 prior stages + the failing stage_esp have timings.
        assert!(receipt.preflight_elevation.is_some());
        assert!(receipt.preflight_bitlocker.is_some());
        assert!(receipt.partition.is_some());
        assert!(receipt.format_esp.is_some());
        assert!(receipt.format_data.is_some());
        assert!(receipt.stage_esp.is_some());
    }

    #[test]
    fn receipt_total_sums_all_populated_stages() {
        let r = DirectInstallReceipt {
            preflight_elevation: Some(Duration::from_millis(10)),
            preflight_bitlocker: Some(Duration::from_millis(20)),
            partition: Some(Duration::from_millis(100)),
            format_esp: Some(Duration::from_millis(200)),
            format_data: Some(Duration::from_millis(300)),
            stage_esp: Some(Duration::from_millis(400)),
        };
        assert_eq!(r.total(), Duration::from_millis(1030));
    }

    #[test]
    fn receipt_total_is_zero_on_default() {
        let r = DirectInstallReceipt::default();
        assert_eq!(r.total(), Duration::ZERO);
    }

    #[test]
    fn direct_install_error_display_includes_stage_and_detail() {
        let e = DirectInstallError {
            stage: DirectInstallStage::StageEsp,
            detail: "WriteFile failed".into(),
        };
        let s = format!("{e}");
        assert!(s.contains("stage_esp"));
        assert!(s.contains("WriteFile failed"));
    }

    #[test]
    fn stage_names_match_issue_body_dispatch_table() {
        // Operator-facing strings the CLI surfaces — treat these as a
        // stable contract with the issue body's "Order of operations"
        // table in #483.
        assert_eq!(
            DirectInstallStage::PreflightElevation.name(),
            "preflight:elevation"
        );
        assert_eq!(
            DirectInstallStage::PreflightBitLocker.name(),
            "preflight:bitlocker"
        );
        assert_eq!(DirectInstallStage::Partition.name(), "partition:diskpart");
        assert_eq!(DirectInstallStage::FormatEsp.name(), "format:esp");
        assert_eq!(DirectInstallStage::FormatData.name(), "format:aegis_isos");
        assert_eq!(DirectInstallStage::StageEsp.name(), "stage_esp");
    }
}
