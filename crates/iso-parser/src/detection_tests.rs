//! Detection tests for `Distribution::from_paths` against the new variants.
//!
//! Kept in a sibling file so the existing `#[cfg(test)] mod tests` block in
//! `lib.rs` stays focused on the scan/mount flow. Included via `#[path]`
//! attribute in lib.rs.

use super::Distribution;
use std::path::PathBuf;

#[test]
fn alpine_detected_from_vmlinuz_lts() {
    assert_eq!(
        Distribution::from_paths(&PathBuf::from("/boot/vmlinuz-lts")),
        Distribution::Alpine
    );
}

#[test]
fn alpine_detected_from_path_marker() {
    assert_eq!(
        Distribution::from_paths(&PathBuf::from("/alpine/boot/vmlinuz")),
        Distribution::Alpine
    );
}

#[test]
fn nixos_detected_from_bzimage() {
    assert_eq!(
        Distribution::from_paths(&PathBuf::from("/boot/bzImage")),
        Distribution::NixOS
    );
}

#[test]
fn nixos_detected_from_path_marker() {
    assert_eq!(
        Distribution::from_paths(&PathBuf::from("/nixos/boot/vmlinuz")),
        Distribution::NixOS
    );
}

#[test]
fn rhel_markers_win_over_fedora() {
    for path in [
        "/rhel/images/pxeboot/vmlinuz",
        "/rocky/images/pxeboot/vmlinuz",
        "/almalinux/images/pxeboot/vmlinuz",
        "/centos/images/pxeboot/vmlinuz",
    ] {
        assert_eq!(
            Distribution::from_paths(&PathBuf::from(path)),
            Distribution::RedHat,
            "{path} should be RedHat"
        );
    }
}

#[test]
fn fedora_still_detected_without_rhel_markers() {
    assert_eq!(
        Distribution::from_paths(&PathBuf::from("/images/pxeboot/vmlinuz")),
        Distribution::Fedora
    );
}

#[test]
fn debian_still_detected_from_casper() {
    assert_eq!(
        Distribution::from_paths(&PathBuf::from("/casper/vmlinuz")),
        Distribution::Debian
    );
}

#[test]
fn arch_still_detected_from_generic_boot() {
    assert_eq!(
        Distribution::from_paths(&PathBuf::from("/boot/vmlinuz")),
        Distribution::Arch
    );
}

#[test]
fn windows_detected_from_bootmgr() {
    for path in [
        "/bootmgr.efi",
        "/sources/boot.wim",
        "/efi/microsoft/boot/bootmgfw.efi",
        "/windows/system32/boot/winload.efi",
    ] {
        assert_eq!(
            Distribution::from_paths(&PathBuf::from(path)),
            Distribution::Windows,
            "{path} should be Windows"
        );
    }
}
