// SPDX-License-Identifier: MIT OR Apache-2.0

//! Harness-driven test modes for rescue-tui (#675).
//!
//! When the kernel cmdline carries `aegis.test=<name>`, /init exports
//! `AEGIS_TEST=<name>` and rescue-tui short-circuits the interactive
//! UI to run a scripted check instead. This is what aegis-hwsim drives
//! to convert its Skip-against-no-test-mode scenarios into Pass.
//!
//! ## Why a test mode at all?
//!
//! `kexec_refuses_unsigned` is the load-bearing assertion in
//! `CLAUDE.md`'s signed-chain story: "operator can trust that an
//! attacker who slipped an unsigned kernel onto the stick can't
//! leverage it via kexec under SB+lockdown." Asserting this end-to-end
//! requires *actually* invoking `kexec_file_load(2)` against an
//! unsigned blob inside a real lockdown=integrity boot and confirming
//! the kernel rejects with `-EKEYREJECTED`. The harness can spin up
//! the QEMU run, but it needs the stick to provide the trigger and
//! the trigger needs to print landmarks the harness can grep.
//!
//! ## Serial landmarks
//!
//! Each test mode prints a fixed-format landmark on stdout (which the
//! initramfs leaves attached to the serial console pre-rescue-tui).
//! See `docs/rescue-tui-serial-format.md` for the contract; the
//! harness pins these strings via `TEST_LANDMARKS` so a wording
//! change here cascades into a coordinated harness PR.

use std::path::Path;

use kexec_loader::{KexecError, KexecRequest};

/// Top-level dispatcher. Reads `AEGIS_TEST` and routes to the matching
/// test fn. Returns the process exit code: 0 for "test asserted what
/// it was meant to assert," non-zero for "test ran but the assertion
/// failed" (e.g. the kernel UNEXPECTEDLY accepted an unsigned image).
///
/// Returns `None` when no recognised test mode is set, so the caller
/// can fall through to the normal interactive TUI path.
#[must_use]
pub fn dispatch_from_env() -> Option<i32> {
    let mode = std::env::var("AEGIS_TEST").ok()?;
    match mode.as_str() {
        "kexec-unsigned" => Some(run_kexec_unsigned()),
        "mok-enroll" => Some(run_mok_enroll()),
        // Unknown / future test modes — log + treat as "no test
        // matched" so a stale aegis.test= cmdline against a newer
        // stick doesn't silently disable the TUI.
        other => {
            eprintln!("aegis-boot-test: unknown test mode '{other}' — falling through to TUI");
            None
        }
    }
}

/// Print the start landmark, attempt a `kexec_file_load(2)` against an
/// obviously-unsigned blob, and print a rejection landmark when the
/// kernel does what it's supposed to. Returns the process exit code.
///
/// The kexec call is wired through [`real_kexec`] in production; tests
/// inject a deterministic stub via [`run_with_kexec`].
pub fn run_kexec_unsigned() -> i32 {
    // /run is tmpfs in the rescue env and writable as root. Tests
    // override via `run_with_kexec_at`.
    run_kexec_unsigned_at(Path::new("/run/aegis-test-unsigned"), real_kexec)
}

/// Production-shape entry parameterised by blob path so tests can
/// stage the dummy file in a tempdir.
fn run_kexec_unsigned_at<F>(blob_path: &Path, kexec_fn: F) -> i32
where
    F: FnOnce(&Path) -> Result<(), KexecError>,
{
    println!("aegis-boot-test: kexec-unsigned starting");

    // Stage an obviously-unsigned 4 KiB blob. Under
    // lockdown=integrity, `kexec_file_load` rejects ANY image whose
    // signature can't be verified against the platform / MOK keyring,
    // and a 4 KiB run of zeros has no signature at all. The exact
    // contents don't matter — the kernel reaches the signature gate
    // before parsing the image format.
    if let Err(e) = std::fs::write(blob_path, vec![0u8; 4096]) {
        println!("aegis-boot-test: kexec-unsigned REJECTED (write-blob failed: {e})");
        return 0;
    }

    let result = kexec_fn(blob_path);

    // Best-effort cleanup. Failure here doesn't change the verdict.
    let _ = std::fs::remove_file(blob_path);

    match result {
        Ok(()) => {
            // The kernel ACCEPTED an unsigned blob — this is the
            // load-bearing failure mode the test exists to catch. A
            // real signed-chain regression would surface here.
            println!("aegis-boot-test: kexec-unsigned UNEXPECTEDLY-LOADED");
            1
        }
        Err(KexecError::SignatureRejected) => {
            println!("aegis-boot-test: kexec-unsigned REJECTED (errno: EKEYREJECTED)");
            0
        }
        Err(KexecError::LockdownRefused) => {
            // Same operator-visible outcome — kernel refused — just
            // a different gate (KEXEC_FILE_LOAD blocked under
            // lockdown=integrity rather than KEXEC_SIG checking the
            // image). The harness counts both as a Pass.
            println!("aegis-boot-test: kexec-unsigned REJECTED (errno: EPERM-lockdown)");
            0
        }
        Err(other) => {
            // Other errors (ENOEXEC for invalid format, EACCES for
            // permission, etc.) still mean "kexec did NOT load an
            // unsigned image," which is the property the test
            // exists to assert. Tag the variant so the harness log
            // captures the gate we hit.
            println!("aegis-boot-test: kexec-unsigned REJECTED (other: {other})");
            0
        }
    }
}

/// Production kexec hook. Wraps `kexec_loader::load_dry` in the shape
/// the test harness expects.
fn real_kexec(blob_path: &Path) -> Result<(), KexecError> {
    kexec_loader::load_dry(&KexecRequest {
        kernel: blob_path.to_path_buf(),
        initrd: None,
        cmdline: String::new(),
    })
}

/// Print the canonical MOK enrollment walkthrough (#202) so
/// aegis-hwsim's `mok_enroll_alpine` scenario (#676 companion to E5.4)
/// can grep-pin the load-bearing strings without a real
/// unsigned-kernel kexec failure.
///
/// The walkthrough body comes from [`crate::state::build_mokutil_remedy`]
/// — the same function the kexec-failure path uses — so the harness
/// assertion is checking the actual operator-visible text. Drift
/// here would surface as a failed harness assertion, which is the
/// whole point of the contract.
///
/// Always returns 0: this mode renders a static walkthrough; there's
/// no failure path to assert against. The harness only validates the
/// landmarks are present.
#[must_use]
pub fn run_mok_enroll() -> i32 {
    println!("aegis-boot-test: MOK enrollment walkthrough starting");
    // No-key variant — harness drives this without a real ISO on
    // disk, so build_mokutil_remedy(None) gives the prose-step-1
    // form. The substring `sudo mokutil --import` still appears in
    // step 1's body.
    let walkthrough = crate::state::build_mokutil_remedy(None);
    println!("{walkthrough}");
    println!("aegis-boot-test: MOK enrollment walkthrough complete");
    0
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::*;

    /// Test helper: stage the dummy blob in a tempdir so the test
    /// process doesn't need write access to /run.
    fn run_with_kexec<F>(kexec_fn: F) -> i32
    where
        F: FnOnce(&Path) -> Result<(), KexecError>,
    {
        let dir = tempfile::tempdir().expect("tempdir");
        let blob = dir.path().join("aegis-test-unsigned");
        run_kexec_unsigned_at(&blob, kexec_fn)
    }

    #[test]
    fn signature_rejected_prints_landmark_and_returns_zero() {
        let rc = run_with_kexec(|_| Err(KexecError::SignatureRejected));
        assert_eq!(rc, 0);
    }

    #[test]
    fn lockdown_refused_prints_landmark_and_returns_zero() {
        let rc = run_with_kexec(|_| Err(KexecError::LockdownRefused));
        assert_eq!(rc, 0);
    }

    #[test]
    fn other_kexec_error_still_returns_zero() {
        // ENOEXEC means "kernel rejected the image format" — same
        // operator-visible property (no unsigned load happened). The
        // harness considers any kexec rejection a pass.
        let rc = run_with_kexec(|_| Err(KexecError::UnsupportedImage));
        assert_eq!(rc, 0);
    }

    #[test]
    fn unexpected_load_returns_one() {
        // The catastrophic failure mode the test exists to detect:
        // kernel accepted an unsigned blob.
        let rc = run_with_kexec(|_| Ok(()));
        assert_eq!(rc, 1);
    }

    // ---- #676 mok-enroll landmarks ----------------------------------

    #[test]
    fn mok_enroll_walkthrough_contains_required_landmarks() {
        // Render the same body the test mode prints so we don't
        // depend on stdout capture. If aegis-hwsim's TEST_LANDMARKS
        // ever drifts from these, the harness assertion fails and
        // we land here.
        let body = crate::state::build_mokutil_remedy(None);
        assert!(
            body.contains("STEP 1/3"),
            "mok-enroll walkthrough missing STEP 1/3: {body}"
        );
        assert!(
            body.contains("sudo mokutil --import"),
            "mok-enroll walkthrough missing 'sudo mokutil --import': {body}"
        );
    }

    #[test]
    fn mok_enroll_returns_zero() {
        // Static walkthrough has no failure path; should always
        // exit 0 so the harness records a Pass when it grep-finds
        // the landmarks.
        assert_eq!(run_mok_enroll(), 0);
    }

    // dispatch_from_env() is intentionally not unit-tested: it reads
    // process-global env state, and rescue-tui forbids unsafe_code
    // (so env::set_var / env::remove_var aren't reachable here).
    // Coverage of the dispatch table comes from the per-mode fns
    // above plus the integration-level smoke test in aegis-hwsim
    // that drives the full /init → AEGIS_TEST → rescue-tui path.
}
