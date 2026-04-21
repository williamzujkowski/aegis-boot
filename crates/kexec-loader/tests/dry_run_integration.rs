// SPDX-License-Identifier: MIT OR Apache-2.0

//! Real `kexec_file_load(2)` integration test.
//!
//! Exercises [`kexec_loader::load_dry`] against the running kernel and
//! verifies `/sys/kernel/kexec_loaded` flips from `0` to `1`. Does NOT call
//! `reboot(LINUX_REBOOT_CMD_KEXEC)` — the test process survives.
//!
//! Gating: requires `CAP_SYS_BOOT` (typically root) + a readable kernel image
//! on `/boot/vmlinuz-*`. Opt in with:
//!
//! ```bash
//! sudo -E AEGIS_INTEGRATION_ROOT=1 cargo test -p kexec-loader -- --ignored
//! ```
//!
//! After the test runs, unload the staged image with:
//!
//! ```bash
//! sudo kexec -u
//! ```
//!
//! (The test also tries to unload via its own cleanup step; kexec -u is a
//! belt-and-suspenders fallback.)

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use kexec_loader::{load_dry, KexecRequest};

fn integration_enabled() -> bool {
    std::env::var("AEGIS_INTEGRATION_ROOT").is_ok()
}

fn find_readable_kernel() -> Option<PathBuf> {
    let dir = PathBuf::from("/boot");
    let entries = fs::read_dir(&dir).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("vmlinuz-") {
            continue;
        }
        if !(name.ends_with("-generic") || name.ends_with("-virtual")) {
            continue;
        }
        let path = entry.path();
        if fs::read(&path).is_ok() {
            return Some(path);
        }
    }
    None
}

fn read_kexec_loaded() -> String {
    fs::read_to_string("/sys/kernel/kexec_loaded")
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

#[test]
#[ignore = "needs root + CAP_SYS_BOOT; run with `sudo -E cargo test -p kexec-loader -- --ignored`"]
fn load_dry_flips_kexec_loaded() {
    if !integration_enabled() {
        eprintln!("skipping: set AEGIS_INTEGRATION_ROOT=1 to opt in");
        return;
    }

    let kernel = find_readable_kernel().unwrap_or_else(|| {
        panic!("no readable /boot/vmlinuz-*-{{generic,virtual}} found; can't run dry-run test")
    });
    eprintln!("using kernel: {}", kernel.display());

    // Guard: if something else already left a kexec image loaded, unload
    // first so we can assert the 0 -> 1 transition cleanly.
    if read_kexec_loaded() == "1" {
        let _ = Command::new("kexec").arg("-u").status();
    }
    assert_eq!(
        read_kexec_loaded(),
        "0",
        "precondition: kexec_loaded must be 0 before the test"
    );

    let req = KexecRequest {
        kernel: kernel.clone(),
        initrd: None,
        cmdline: "quiet".to_string(),
    };

    load_dry(&req).unwrap_or_else(|e| panic!("load_dry failed: {e}"));

    assert_eq!(
        read_kexec_loaded(),
        "1",
        "kexec_file_load should set /sys/kernel/kexec_loaded to 1"
    );

    // Cleanup so the next test run starts from a clean slate + so the CI
    // host isn't left in a "reboot -f → target kernel" trap state.
    let _ = Command::new("kexec").arg("-u").status();
}
