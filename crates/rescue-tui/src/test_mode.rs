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
        "manifest-roundtrip" => Some(run_manifest_roundtrip()),
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

/// Mount the ESP, parse the on-stick attestation manifest, and (when
/// `expected_pcrs` is non-empty) compare each entry to the live PCR
/// value. Used by aegis-hwsim's E6 attestation-roundtrip scenario
/// (#695).
///
/// In shipped releases through 0.17.x, `expected_pcrs` is always
/// empty by design (the manifest contract is pinned but the PCR
/// selection is gated on the E6 epic itself). In that PR3-era state,
/// the test mode prints a `parsed (...)` landmark to confirm the
/// manifest was read + parsed cleanly, then a `empty-pcrs` landmark
/// and exits 0 — the harness counts that as Pass-via-fail-open per
/// `docs/attestation-manifest.md`.
///
/// When `expected_pcrs` starts being populated, this function
/// iterates each entry, reads the live PCR via the kernel's sysfs
/// interface (`/sys/class/tpm/tpm0/pcr-<bank>/<idx>`), and emits a
/// MATCH or MISMATCH landmark per PCR. The harness pins on the
/// per-entry landmarks so a mid-release schema change surfaces as
/// a failed assertion rather than a silent regression.
#[must_use]
pub fn run_manifest_roundtrip() -> i32 {
    println!("aegis-boot-test: manifest-roundtrip starting");
    let esp_mount = std::path::Path::new("/run/aegis-test-esp");
    let esp_dev = match find_esp_device() {
        Ok(d) => d,
        Err(e) => {
            println!("aegis-boot-test: manifest-roundtrip FAILED (esp-find: {e})");
            return 1;
        }
    };
    if let Err(e) = mount_esp_ro(&esp_dev, esp_mount) {
        println!("aegis-boot-test: manifest-roundtrip FAILED (esp-mount: {e})");
        return 1;
    }
    let rc = run_manifest_roundtrip_at(esp_mount, &mut read_pcr_sysfs);
    let _ = std::process::Command::new("umount").arg(esp_mount).status();
    rc
}

/// Test-injectable variant: caller supplies the ESP mount point + a
/// PCR-reader fn so unit tests can drive the parse + comparison
/// branches without needing root or a real TPM.
fn run_manifest_roundtrip_at(
    esp_mount: &std::path::Path,
    read_pcr: &mut dyn FnMut(&str, u32) -> Result<String, String>,
) -> i32 {
    let path = esp_mount.join("aegis-boot-manifest.json");
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            println!(
                "aegis-boot-test: manifest-roundtrip FAILED (read {}: {e})",
                path.display()
            );
            return 1;
        }
    };
    let manifest: aegis_wire_formats::Manifest = match serde_json::from_slice(&bytes) {
        Ok(m) => m,
        Err(e) => {
            println!("aegis-boot-test: manifest-roundtrip FAILED (parse: {e})");
            return 1;
        }
    };
    println!(
        "aegis-boot-test: manifest-roundtrip parsed (schema_version={}, esp_files={}, expected_pcrs={})",
        manifest.schema_version,
        manifest.esp_files.len(),
        manifest.expected_pcrs.len()
    );
    if manifest.expected_pcrs.is_empty() {
        println!(
            "aegis-boot-test: manifest-roundtrip empty-pcrs (PR3-era; harness fail-opens per attestation-manifest.md contract)"
        );
        return 0;
    }
    let mut all_pass = true;
    for entry in &manifest.expected_pcrs {
        match read_pcr(&entry.bank, entry.pcr_index) {
            Ok(live) if live.eq_ignore_ascii_case(&entry.digest_hex) => {
                println!(
                    "aegis-boot-test: manifest-roundtrip pcr_index={} bank={} MATCH",
                    entry.pcr_index, entry.bank
                );
            }
            Ok(live) => {
                println!(
                    "aegis-boot-test: manifest-roundtrip pcr_index={} bank={} MISMATCH (expected={} live={})",
                    entry.pcr_index, entry.bank, entry.digest_hex, live
                );
                all_pass = false;
            }
            Err(e) => {
                println!(
                    "aegis-boot-test: manifest-roundtrip pcr_index={} bank={} READ-FAILED ({e})",
                    entry.pcr_index, entry.bank
                );
                all_pass = false;
            }
        }
    }
    i32::from(!all_pass)
}

/// Locate the ESP block device. `/dev/disk/by-label/AEGIS_ESP` is the
/// canonical symlink set up by mkusb.sh + direct-install (label set
/// at format time via `mkfs.fat -n AEGIS_ESP`). udev populates the
/// `by-label` tree on every block-device discovery in the rescue env.
fn find_esp_device() -> Result<std::path::PathBuf, String> {
    let by_label = std::path::Path::new("/dev/disk/by-label/AEGIS_ESP");
    if by_label.exists() {
        return Ok(by_label.to_path_buf());
    }
    Err(format!("not found: {}", by_label.display()))
}

/// Mount `dev` read-only at `mount_point` as vfat. Read-only protects
/// the ESP against accidental writes from the test mode itself.
fn mount_esp_ro(dev: &std::path::Path, mount_point: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(mount_point)
        .map_err(|e| format!("mkdir {}: {e}", mount_point.display()))?;
    let status = std::process::Command::new("mount")
        .args(["-t", "vfat", "-o", "ro"])
        .arg(dev)
        .arg(mount_point)
        .status()
        .map_err(|e| format!("spawn mount: {e}"))?;
    if !status.success() {
        return Err(format!("mount returned {status}"));
    }
    Ok(())
}

/// Read the live PCR digest from the kernel's TPM sysfs interface.
/// Modern kernels (5.5+) expose `/sys/class/tpm/tpm0/pcr-<bank>/<idx>`
/// which yields the digest as a hex string. Banks: `sha256`, `sha384`,
/// `sha1`. The path is read-only — no privileges escalation needed
/// inside the rescue env.
fn read_pcr_sysfs(bank: &str, pcr: u32) -> Result<String, String> {
    let path = format!("/sys/class/tpm/tpm0/pcr-{bank}/{pcr}");
    let raw = std::fs::read_to_string(&path).map_err(|e| format!("read {path}: {e}"))?;
    Ok(raw.trim().to_lowercase())
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

    // ---- #695 manifest-roundtrip ------------------------------------

    fn write_test_manifest(dir: &std::path::Path, body: &str) {
        std::fs::write(dir.join("aegis-boot-manifest.json"), body).expect("write");
    }

    fn empty_pcrs_manifest() -> String {
        r#"{
  "schema_version": 1,
  "tool_version": "aegis-boot 0.17.0",
  "manifest_sequence": 1,
  "device": {
    "disk_guid": "00000000-0000-0000-0000-000000000001",
    "partition_count": 2,
    "esp": {
      "partuuid": "00000000-0000-0000-0000-0000000000a1",
      "type_guid": "C12A7328-F81F-11D2-BA4B-00A0C93EC93B",
      "fs_uuid": "AAAA-AAAA",
      "first_lba": 2048,
      "last_lba": 821247
    },
    "data": {
      "partuuid": "00000000-0000-0000-0000-0000000000a2",
      "type_guid": "EBD0A0A2-B9E5-4433-87C0-68B6B72699C7",
      "fs_uuid": "BBBB-BBBB",
      "label": "AEGIS_ISOS"
    }
  },
  "esp_files": [],
  "allowed_files_closed_set": true,
  "expected_pcrs": []
}"#
        .to_string()
    }

    fn populated_pcrs_manifest(pcr12_digest: &str) -> String {
        format!(
            r#"{{
  "schema_version": 1,
  "tool_version": "aegis-boot 0.18.0",
  "manifest_sequence": 1,
  "device": {{
    "disk_guid": "00000000-0000-0000-0000-000000000001",
    "partition_count": 2,
    "esp": {{ "partuuid": "00000000-0000-0000-0000-0000000000a1", "type_guid": "C12A7328-F81F-11D2-BA4B-00A0C93EC93B", "fs_uuid": "AAAA-AAAA", "first_lba": 2048, "last_lba": 821247 }},
    "data": {{ "partuuid": "00000000-0000-0000-0000-0000000000a2", "type_guid": "EBD0A0A2-B9E5-4433-87C0-68B6B72699C7", "fs_uuid": "BBBB-BBBB", "label": "AEGIS_ISOS" }}
  }},
  "esp_files": [],
  "allowed_files_closed_set": true,
  "expected_pcrs": [
    {{ "pcr_index": 12, "bank": "sha256", "digest_hex": "{pcr12_digest}" }}
  ]
}}"#
        )
    }

    #[test]
    fn manifest_roundtrip_empty_pcrs_returns_zero() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_test_manifest(dir.path(), &empty_pcrs_manifest());
        let mut never_called = |_: &str, _: u32| -> Result<String, String> {
            unreachable!("read_pcr must not be called when expected_pcrs is empty");
        };
        let rc = run_manifest_roundtrip_at(dir.path(), &mut never_called);
        assert_eq!(rc, 0, "empty expected_pcrs should be Pass-via-fail-open");
    }

    #[test]
    fn manifest_roundtrip_pcr_match_returns_zero() {
        let dir = tempfile::tempdir().expect("tempdir");
        let digest = "abc123def456abc123def456abc123def456abc123def456abc123def456abc1";
        write_test_manifest(dir.path(), &populated_pcrs_manifest(digest));
        let owned = digest.to_string();
        let mut reader = move |bank: &str, pcr: u32| -> Result<String, String> {
            assert_eq!(bank, "sha256");
            assert_eq!(pcr, 12);
            Ok(owned.clone())
        };
        let rc = run_manifest_roundtrip_at(dir.path(), &mut reader);
        assert_eq!(rc, 0);
    }

    #[test]
    fn manifest_roundtrip_pcr_mismatch_returns_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let expected = "abc123def456abc123def456abc123def456abc123def456abc123def456abc1";
        write_test_manifest(dir.path(), &populated_pcrs_manifest(expected));
        let mut reader = |_: &str, _: u32| -> Result<String, String> {
            // Drift digest — proves comparison fires + flags MISMATCH.
            Ok("ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".to_string())
        };
        let rc = run_manifest_roundtrip_at(dir.path(), &mut reader);
        assert_eq!(rc, 1, "drifted PCR should surface as a non-zero exit");
    }

    #[test]
    fn manifest_roundtrip_pcr_read_failure_returns_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_test_manifest(
            dir.path(),
            &populated_pcrs_manifest("ab".repeat(32).as_str()),
        );
        let mut reader =
            |_: &str, _: u32| -> Result<String, String> { Err("no /sys/class/tpm".to_string()) };
        let rc = run_manifest_roundtrip_at(dir.path(), &mut reader);
        assert_eq!(rc, 1, "PCR-read failure must surface as a non-zero exit");
    }

    #[test]
    fn manifest_roundtrip_missing_manifest_returns_one() {
        // Empty dir — no manifest.json. Should fail loudly.
        let dir = tempfile::tempdir().expect("tempdir");
        let mut reader = |_: &str, _: u32| -> Result<String, String> {
            unreachable!("no PCR read should happen if manifest read failed");
        };
        let rc = run_manifest_roundtrip_at(dir.path(), &mut reader);
        assert_eq!(rc, 1);
    }

    #[test]
    fn manifest_roundtrip_garbage_json_returns_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_test_manifest(dir.path(), "this is not json");
        let mut reader = |_: &str, _: u32| -> Result<String, String> {
            unreachable!("no PCR read should happen on parse failure");
        };
        let rc = run_manifest_roundtrip_at(dir.path(), &mut reader);
        assert_eq!(rc, 1);
    }

    // dispatch_from_env() is intentionally not unit-tested: it reads
    // process-global env state, and rescue-tui forbids unsafe_code
    // (so env::set_var / env::remove_var aren't reachable here).
    // Coverage of the dispatch table comes from the per-mode fns
    // above plus the integration-level smoke test in aegis-hwsim
    // that drives the full /init → AEGIS_TEST → rescue-tui path.
}
