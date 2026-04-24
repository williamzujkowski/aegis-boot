// SPDX-License-Identifier: MIT OR Apache-2.0

//! [#418] Phase 4b — macOS `--direct-install` pipeline composer.
//!
//! Chains the four stage modules into a runnable sequence, same
//! shape as [`crate::windows_direct_install::pipeline::run`]:
//!
//! 1. **Preflight** — [`super::preflight::preflight_diskutil`]:
//!    parse `diskutil info` + classify against safety gates.
//! 2. **Partition** — [`super::partition::partition_via_diskutil`]:
//!    run `diskutil partitionDisk` with the Phase 1 argv plan.
//! 3. **Stage ESP** — [`super::esp_stage::execute_copy_plan`]:
//!    `mkdir -p` + cp + `/bin/sync` against the auto-mounted
//!    `/Volumes/AEGIS_ESP`.
//! 4. **Unmount** — [`super::esp_stage::unmount_esp`]:
//!    `diskutil unmount /Volumes/AEGIS_ESP`.
//!
//! No `BitLocker` stage (no equivalent on macOS) and no separate
//! format stage (`diskutil partitionDisk` formats in the same
//! call). Four stages, not six.
//!
//! ## Why a trait + production impl split
//!
//! The pipeline is tested with a mock [`PhaseRunner`] on every
//! host — abort cascades, per-stage timing, stage ordering — so
//! a regression in the composer surfaces without a macOS VM. The
//! real-world [`MacosPhaseRunner`] is `#[cfg(target_os = "macos")]`
//! and dispatches each method to the module that owns the real
//! subprocess work.
//!
//! ## What this module deliberately does NOT do
//!
//! - Source-path resolution for the 6 signed-chain files
//!   (caller's responsibility; same layering as Windows).
//! - Raw-write of the `AEGIS_ISOS` data partition — the #418
//!   issue says "`/dev/rdiskN` raw write already works per
//!   #365 Phase A — unchanged," so the existing Linux dd path
//!   (which runs natively on macOS) handles that stage.
//! - Attestation manifest writing.
//! - The CLI surface (`aegis-boot flash --direct-install` for
//!   macOS) — that's Phase 4c.
//!
//! [#418]: https://github.com/aegis-boot/aegis-boot/issues/418

// Phase 4b lands the composer + mock-test infrastructure ahead of
// Phase 4c's CLI wiring. Unit tests exercise every public symbol
// against a MockPhaseRunner so regressions surface at CI time.
#![allow(dead_code)]

use std::time::{Duration, Instant};

use crate::macos_direct_install::esp_stage::CopyPlan;
use crate::macos_direct_install::partition::DiskutilPartitionPlan;
use crate::macos_direct_install::preflight::DiskInfo;
use crate::windows_direct_install::raw_write::EspStagingSources;

/// Which stage reported the error. Mirrors the shape of the
/// Linux-side `DirectInstallStage` + Windows `pipeline` enum so the
/// formatter reads the same regardless of host OS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirectInstallStage {
    Preflight,
    Partition,
    StageEsp,
    Unmount,
}

impl DirectInstallStage {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Preflight => "preflight:diskutil_info",
            Self::Partition => "partition:diskutil",
            Self::StageEsp => "stage_esp:cp+sync",
            Self::Unmount => "unmount:diskutil",
        }
    }
}

/// Error returned by the pipeline. Always carries the stage that
/// produced it so the CLI can print `Stage N (partition:diskutil)
/// failed: ...`.
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

/// Elapsed time per stage that ran. Stages skipped because an
/// earlier stage failed don't appear in the receipt.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DirectInstallReceipt {
    pub(crate) preflight: Option<Duration>,
    pub(crate) partition: Option<Duration>,
    pub(crate) stage_esp: Option<Duration>,
    pub(crate) unmount: Option<Duration>,
}

impl DirectInstallReceipt {
    /// Total elapsed across all completed stages.
    pub(crate) fn total(&self) -> Duration {
        [self.preflight, self.partition, self.stage_esp, self.unmount]
            .into_iter()
            .flatten()
            .sum()
    }
}

/// All inputs the pipeline needs. The caller (CLI dispatch) builds
/// this from the operator-supplied device argument + signed-chain
/// source resolution.
#[derive(Debug, Clone)]
pub(crate) struct DirectInstallPlan {
    pub(crate) device_id: String,
    pub(crate) sources: EspStagingSources,
}

/// Indirection for the four destructive stage dispatches so the
/// composer is testable without a macOS VM. The default
/// [`MacosPhaseRunner`] wires each method straight through to the
/// module that owns the real `diskutil` / `cp` / `/bin/sync` work;
/// tests supply a mock that returns canned results and records the
/// call order.
pub(crate) trait PhaseRunner {
    /// Returns the validated [`DiskInfo`] on acceptance.
    fn preflight(&self, device_id: &str) -> Result<DiskInfo, String>;

    /// Runs `diskutil partitionDisk` with the argv the Phase 1
    /// module built.
    fn partition(&self, plan: &DiskutilPartitionPlan) -> Result<(), String>;

    /// Runs the Phase 3 copy plan (mkdir → cp → `/bin/sync`).
    fn stage_esp(&self, plan: &CopyPlan) -> Result<(), String>;

    /// Unmounts the ESP via `diskutil unmount`.
    fn unmount_esp(&self, mount_point: &std::path::Path) -> Result<(), String>;
}

/// Production runner — dispatches each method to the module that
/// owns the real subprocess work. Compiles only on macOS; other
/// hosts use mocks in tests, and the CLI dispatcher refuses to
/// build the direct-install surface on non-macOS hosts (see Phase
/// 4c wiring, to follow).
#[cfg(target_os = "macos")]
pub(crate) struct MacosPhaseRunner;

#[cfg(target_os = "macos")]
impl PhaseRunner for MacosPhaseRunner {
    fn preflight(&self, device_id: &str) -> Result<DiskInfo, String> {
        super::preflight::preflight_diskutil(device_id)
    }

    fn partition(&self, plan: &DiskutilPartitionPlan) -> Result<(), String> {
        super::partition::partition_via_diskutil(plan)
    }

    fn stage_esp(&self, plan: &CopyPlan) -> Result<(), String> {
        super::esp_stage::execute_copy_plan(plan)
    }

    fn unmount_esp(&self, mount_point: &std::path::Path) -> Result<(), String> {
        super::esp_stage::unmount_esp(mount_point)
    }
}

/// Error payload + partial-progress receipt bundled so the `Err`
/// variant stays pointer-sized. Matches the Windows pipeline's
/// `RunFailure` shape.
pub(crate) type RunFailure = Box<(DirectInstallError, DirectInstallReceipt)>;

/// Run the full four-stage pipeline. Returns a receipt on success;
/// on failure, the boxed payload reports which stages actually
/// ran.
///
/// Stage ordering is linear with no retries — each failure causes
/// an immediate abort. The receipt's partial timing tells a
/// post-mortem operator exactly how much work burned before the
/// abort ("preflight passed in 40ms, partition failed at 3.2s —
/// stick is in an indeterminate state, operator must rerun").
///
/// # Errors
///
/// [`RunFailure`] — boxed [`DirectInstallError`] + partial receipt.
pub(crate) fn run(
    runner: &dyn PhaseRunner,
    plan: &DirectInstallPlan,
) -> Result<DirectInstallReceipt, RunFailure> {
    let mut receipt = DirectInstallReceipt::default();

    // Stage 1: preflight.
    let t = Instant::now();
    match runner.preflight(&plan.device_id) {
        Ok(_info) => {
            receipt.preflight = Some(t.elapsed());
        }
        Err(detail) => {
            receipt.preflight = Some(t.elapsed());
            return Err(Box::new((
                DirectInstallError {
                    stage: DirectInstallStage::Preflight,
                    detail,
                },
                receipt,
            )));
        }
    }

    // Stage 2: partition. Builds the argv from the device_id here
    // rather than taking a pre-built plan — the shape check for
    // `disk0` is already gated by preflight above, but if it ever
    // got here the partition builder catches it as a second line
    // of defense.
    let part_plan =
        match crate::macos_direct_install::partition::build_diskutil_plan(&plan.device_id) {
            Ok(p) => p,
            Err(e) => {
                return Err(Box::new((
                    DirectInstallError {
                        stage: DirectInstallStage::Partition,
                        detail: e.to_string(),
                    },
                    receipt,
                )));
            }
        };
    let t = Instant::now();
    if let Err(detail) = runner.partition(&part_plan) {
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

    // Stage 3: stage ESP. Build the CopyPlan with the canonical
    // mount point — same shape-safety guarantee as Phase 3's unit
    // tests pin.
    let mount = super::esp_stage::canonical_mount_point();
    let copy_plan = match super::esp_stage::build_copy_plan(&plan.sources, &mount) {
        Ok(p) => p,
        Err(e) => {
            return Err(Box::new((
                DirectInstallError {
                    stage: DirectInstallStage::StageEsp,
                    detail: e.to_string(),
                },
                receipt,
            )));
        }
    };
    let t = Instant::now();
    if let Err(detail) = runner.stage_esp(&copy_plan) {
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

    // Stage 4: unmount. Even if this fails the ESP is written —
    // surfacing the error lets the operator retry the unmount
    // without re-flashing.
    let t = Instant::now();
    if let Err(detail) = runner.unmount_esp(&mount) {
        receipt.unmount = Some(t.elapsed());
        return Err(Box::new((
            DirectInstallError {
                stage: DirectInstallStage::Unmount,
                detail,
            },
            receipt,
        )));
    }
    receipt.unmount = Some(t.elapsed());

    Ok(receipt)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::path::PathBuf;

    /// Canned responses a test wants a [`PhaseRunner`] to return.
    /// `None` means the stage's real (or mock-default) behavior
    /// runs; `Some(result)` overrides with a scripted value.
    #[derive(Debug, Default)]
    struct MockScript {
        preflight: Option<Result<DiskInfo, String>>,
        partition: Option<Result<(), String>>,
        stage_esp: Option<Result<(), String>>,
        unmount: Option<Result<(), String>>,
    }

    /// Test-only [`PhaseRunner`] implementation that records call
    /// order + returns scripted results. Lets unit tests assert
    /// abort-cascade behavior ("if partition fails, `stage_esp` and
    /// unmount must never be called") without spawning diskutil.
    struct MockPhaseRunner {
        script: MockScript,
        calls: RefCell<Vec<String>>,
    }

    impl MockPhaseRunner {
        fn new(script: MockScript) -> Self {
            Self {
                script,
                calls: RefCell::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<String> {
            self.calls.borrow().clone()
        }
    }

    impl PhaseRunner for MockPhaseRunner {
        fn preflight(&self, device_id: &str) -> Result<DiskInfo, String> {
            self.calls
                .borrow_mut()
                .push(format!("preflight({device_id})"));
            self.script
                .preflight
                .clone()
                .unwrap_or_else(|| Ok(dummy_disk_info(device_id)))
        }

        fn partition(&self, plan: &DiskutilPartitionPlan) -> Result<(), String> {
            self.calls
                .borrow_mut()
                .push(format!("partition({})", plan.device_id));
            self.script.partition.clone().unwrap_or(Ok(()))
        }

        fn stage_esp(&self, plan: &CopyPlan) -> Result<(), String> {
            self.calls
                .borrow_mut()
                .push(format!("stage_esp(files={})", plan.copies.len()));
            self.script.stage_esp.clone().unwrap_or(Ok(()))
        }

        fn unmount_esp(&self, mount_point: &std::path::Path) -> Result<(), String> {
            self.calls
                .borrow_mut()
                .push(format!("unmount_esp({})", mount_point.display()));
            self.script.unmount.clone().unwrap_or(Ok(()))
        }
    }

    fn dummy_disk_info(device_id: &str) -> DiskInfo {
        DiskInfo {
            device_id: device_id.to_string(),
            whole_disk: true,
            removable: true,
            external: true,
            size_bytes: 16 * 1024 * 1024 * 1024,
            media_name: "Test Media".to_string(),
        }
    }

    fn dummy_sources() -> EspStagingSources {
        EspStagingSources {
            shim_x64: PathBuf::from("/usr/lib/shim/shimx64.efi.signed"),
            grub_x64: PathBuf::from("/usr/lib/grub/x86_64-efi-signed/grubx64.efi.signed"),
            mm_x64: PathBuf::from("/usr/lib/shim/mmx64.efi.signed"),
            grub_cfg: PathBuf::from("/tmp/grub.cfg"),
            vmlinuz: PathBuf::from("/boot/vmlinuz-virtual"),
            initramfs: PathBuf::from("/tmp/initramfs.cpio.gz"),
        }
    }

    fn sample_plan() -> DirectInstallPlan {
        DirectInstallPlan {
            device_id: "disk5".to_string(),
            sources: dummy_sources(),
        }
    }

    #[test]
    fn happy_path_runs_all_four_stages_in_order() {
        let mock = MockPhaseRunner::new(MockScript::default());
        let receipt = run(&mock, &sample_plan()).expect("happy path should succeed");
        assert!(receipt.preflight.is_some());
        assert!(receipt.partition.is_some());
        assert!(receipt.stage_esp.is_some());
        assert!(receipt.unmount.is_some());

        let calls = mock.calls();
        assert_eq!(calls.len(), 4, "expected 4 stage calls, got {calls:?}");
        assert!(calls[0].starts_with("preflight("));
        assert!(calls[1].starts_with("partition("));
        assert!(calls[2].starts_with("stage_esp("));
        assert!(calls[3].starts_with("unmount_esp("));
    }

    #[test]
    fn preflight_failure_aborts_pipeline_immediately() {
        let mock = MockPhaseRunner::new(MockScript {
            preflight: Some(Err("not removable".to_string())),
            ..Default::default()
        });
        let err = run(&mock, &sample_plan()).unwrap_err();
        assert_eq!(err.0.stage, DirectInstallStage::Preflight);
        assert!(err.0.detail.contains("not removable"));

        // Only preflight should have been called.
        assert_eq!(mock.calls().len(), 1);
        // Receipt has preflight timing, nothing else.
        assert!(err.1.preflight.is_some());
        assert!(err.1.partition.is_none());
        assert!(err.1.stage_esp.is_none());
        assert!(err.1.unmount.is_none());
    }

    #[test]
    fn partition_failure_skips_stage_and_unmount() {
        let mock = MockPhaseRunner::new(MockScript {
            partition: Some(Err("diskutil exited 1".to_string())),
            ..Default::default()
        });
        let err = run(&mock, &sample_plan()).unwrap_err();
        assert_eq!(err.0.stage, DirectInstallStage::Partition);

        let calls = mock.calls();
        assert_eq!(calls.len(), 2);
        assert!(calls[0].starts_with("preflight("));
        assert!(calls[1].starts_with("partition("));
        assert!(err.1.preflight.is_some());
        assert!(err.1.partition.is_some());
        assert!(err.1.stage_esp.is_none());
        assert!(err.1.unmount.is_none());
    }

    #[test]
    fn stage_esp_failure_still_attempts_no_unmount() {
        let mock = MockPhaseRunner::new(MockScript {
            stage_esp: Some(Err("cp failed".to_string())),
            ..Default::default()
        });
        let err = run(&mock, &sample_plan()).unwrap_err();
        assert_eq!(err.0.stage, DirectInstallStage::StageEsp);

        let calls = mock.calls();
        assert_eq!(calls.len(), 3);
        // unmount was skipped — the rationale is in the module
        // docstring: "surfacing the error lets the operator retry
        // the unmount without re-flashing" means the caller handles
        // that, not the composer's abort path.
        assert!(err.1.stage_esp.is_some());
        assert!(err.1.unmount.is_none());
    }

    #[test]
    fn unmount_failure_surfaces_at_final_stage() {
        let mock = MockPhaseRunner::new(MockScript {
            unmount: Some(Err("resource busy".to_string())),
            ..Default::default()
        });
        let err = run(&mock, &sample_plan()).unwrap_err();
        assert_eq!(err.0.stage, DirectInstallStage::Unmount);
        // All four stage calls happened — unmount was attempted +
        // returned an error.
        assert_eq!(mock.calls().len(), 4);
        assert!(err.1.unmount.is_some());
    }

    #[test]
    fn partition_build_error_surfaces_without_partition_call() {
        // A device_id that fails build_diskutil_plan (e.g. "disk0")
        // must surface at the partition stage WITHOUT invoking the
        // runner's partition method — the plan never gets built.
        // Override the preflight mock so we reach the partition
        // stage (real preflight would also reject disk0).
        let mock = MockPhaseRunner::new(MockScript {
            preflight: Some(Ok(dummy_disk_info("disk0"))),
            ..Default::default()
        });
        let plan = DirectInstallPlan {
            device_id: "disk0".to_string(),
            sources: dummy_sources(),
        };
        let err = run(&mock, &plan).unwrap_err();
        assert_eq!(err.0.stage, DirectInstallStage::Partition);
        // Only preflight was called on the runner; partition build
        // rejected before runner.partition() ran.
        let calls = mock.calls();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].starts_with("preflight("));
    }

    #[test]
    fn stage_name_strings_are_stable() {
        // Operator-facing strings are part of the contract; pin
        // them so a refactor can't silently rename.
        assert_eq!(
            DirectInstallStage::Preflight.name(),
            "preflight:diskutil_info"
        );
        assert_eq!(DirectInstallStage::Partition.name(), "partition:diskutil");
        assert_eq!(DirectInstallStage::StageEsp.name(), "stage_esp:cp+sync");
        assert_eq!(DirectInstallStage::Unmount.name(), "unmount:diskutil");
    }

    #[test]
    fn error_display_formats_stage_and_detail() {
        let e = DirectInstallError {
            stage: DirectInstallStage::Partition,
            detail: "diskutil exited 1".to_string(),
        };
        assert_eq!(e.to_string(), "partition:diskutil: diskutil exited 1");
    }

    #[test]
    fn receipt_total_sums_all_completed_stages() {
        let r = DirectInstallReceipt {
            preflight: Some(Duration::from_millis(40)),
            partition: Some(Duration::from_millis(3200)),
            stage_esp: Some(Duration::from_millis(180)),
            unmount: Some(Duration::from_millis(60)),
        };
        assert_eq!(r.total(), Duration::from_millis(3480));
    }

    #[test]
    fn receipt_total_ignores_skipped_stages() {
        let r = DirectInstallReceipt {
            preflight: Some(Duration::from_millis(40)),
            partition: Some(Duration::from_millis(3200)),
            // stage_esp + unmount skipped (partition failed).
            stage_esp: None,
            unmount: None,
        };
        assert_eq!(r.total(), Duration::from_millis(3240));
    }
}
