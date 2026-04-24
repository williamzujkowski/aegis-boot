// SPDX-License-Identifier: MIT OR Apache-2.0

//! Integration test for the full `iso-probe` discover → prepare flow.
//!
//! Builds a minimal ISO9660 fixture with `xorriso`, then exercises:
//!   1. [`iso_probe::discover`] — finds the ISO, reports kernel/initrd paths
//!   2. [`iso_probe::prepare`]  — loop-mounts, exposes absolute paths
//!   3. `Drop(PreparedIso)`     — unmounts cleanly
//!
//! Gating: requires `CAP_SYS_ADMIN` for loop-mount and `xorriso` on `$PATH`.
//! Opt in with `sudo -E cargo test -p iso-probe -- --ignored`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use iso_probe::{DiscoveredIso, Distribution};

fn have_xorriso() -> bool {
    Command::new("xorriso")
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Build a tiny ISO9660 image at `out` containing `casper/vmlinuz` +
/// `casper/initrd` with known content. Returns the kernel magic bytes so the
/// test can verify round-trip readback after mount.
fn build_fixture_iso(out: &Path) -> Vec<u8> {
    let tmp = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
    let stage = tmp.path().join("stage");
    fs::create_dir_all(stage.join("casper")).unwrap_or_else(|e| panic!("mkdir: {e}"));

    let kernel_magic: Vec<u8> = b"AEGIS-TEST-KERNEL-FIXTURE-0001".to_vec();
    fs::write(stage.join("casper/vmlinuz"), &kernel_magic)
        .unwrap_or_else(|e| panic!("write kernel: {e}"));
    fs::write(stage.join("casper/initrd"), b"initrd-fixture-0001")
        .unwrap_or_else(|e| panic!("write initrd: {e}"));

    let status = Command::new("xorriso")
        .args([
            "-as",
            "mkisofs",
            "-quiet",
            "-r",
            "-J",
            "-V",
            "AEGIS_TEST",
            "-o",
        ])
        .arg(out)
        .arg(stage)
        .status()
        .unwrap_or_else(|e| panic!("xorriso: {e}"));
    assert!(status.success(), "xorriso failed to build fixture ISO");

    kernel_magic
}

fn integration_enabled() -> bool {
    std::env::var("AEGIS_INTEGRATION_ROOT").is_ok()
}

#[test]
#[ignore = "needs root + xorriso; run with `sudo -E cargo test -p iso-probe -- --ignored`"]
fn discover_and_prepare_round_trip() {
    if !integration_enabled() {
        eprintln!("skipping: set AEGIS_INTEGRATION_ROOT=1 to opt in");
        return;
    }
    assert!(
        have_xorriso(),
        "xorriso is not installed — `apt install xorriso`"
    );

    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
    let iso_path = dir.path().join("fixture.iso");
    let kernel_magic = build_fixture_iso(&iso_path);

    let search_roots: Vec<PathBuf> = vec![dir.path().to_path_buf()];
    let report = iso_probe::discover(&search_roots)
        .unwrap_or_else(|e| panic!("discover returned error: {e}"));
    assert!(
        !report.isos.is_empty(),
        "discover must return at least one DiscoveredIso"
    );
    assert!(
        report.failed.is_empty(),
        "fixture should parse cleanly; got failures: {:?}",
        report.failed
    );

    // The fixture uses casper/ layout → Debian/Ubuntu family.
    let iso: &DiscoveredIso = report
        .isos
        .iter()
        .find(|d| d.distribution == Distribution::Debian)
        .unwrap_or_else(|| panic!("expected a Debian-layout discovery from fixture"));
    assert_eq!(iso.kernel, Path::new("casper/vmlinuz"));

    let mount_point = {
        let prepared =
            iso_probe::prepare(iso).unwrap_or_else(|e| panic!("prepare returned error: {e}"));
        let mp = prepared.mount_point().to_path_buf();

        // Absolute kernel path must be readable and match the fixture bytes.
        let actual = fs::read(&prepared.kernel).unwrap_or_else(|e| panic!("read kernel: {e}"));
        assert_eq!(actual, kernel_magic, "round-trip kernel content mismatch");
        mp
        // prepared drops here → unmount
    };

    // After drop, the mount should be gone.
    let mounts = fs::read_to_string("/proc/mounts").unwrap_or_default();
    let still_mounted = mounts
        .lines()
        .any(|l| l.split_whitespace().nth(1) == Some(mount_point.to_string_lossy().as_ref()));
    assert!(
        !still_mounted,
        "PreparedIso drop did not unmount {}",
        mount_point.display()
    );
}
