# Per-distro ISO + kexec compatibility matrix

**Version:** 1.0 (aegis-boot v0.1.0 baseline)
**Issue:** [#6](https://github.com/williamzujkowski/aegis-boot/issues/6)
**Consumed by:** [`iso_probe::lookup_quirks`](../../crates/iso-probe/src/lib.rs)

This matrix is the ground truth for `iso-probe`'s quirk annotations. Each row reflects the distribution's *default* installation media; user-customized images may differ. Any change to a row should land in the same PR as the corresponding `lookup_quirks` update.

## Summary

| Distribution  | Boot layout                      | Kernel signed by           | `KEXEC_SIG` | Quirks                                   |
|---------------|----------------------------------|----------------------------|-------------|------------------------------------------|
| **Ubuntu**    | `casper/vmlinuz` + `casper/initrd` | Canonical UEFI CA        | accepts     | (none known)                             |
| **Debian**    | `install.amd/vmlinuz` + `live/initrd` | Debian UEFI CA        | accepts     | (none known)                             |
| **Fedora**    | `images/pxeboot/vmlinuz`         | Fedora UEFI CA             | accepts     | `CrossDistroKexecRefused`                |
| **Arch**      | `arch/boot/x86_64/vmlinuz-linux` | **unsigned** (default)     | rejects     | `UnsignedKernel`                         |
| **Alpine**    | `boot/vmlinuz-lts`               | **unsigned** (default)     | rejects     | `UnsignedKernel` (enum: `Unknown`)       |
| **NixOS**     | `boot/bzImage`                   | **unsigned** (default)     | rejects     | `UnsignedKernel` (enum: `Unknown`)       |

`Distribution` enum coverage in v0.1.0: `Debian`, `Fedora`, `Arch`, `Unknown`. Alpine and NixOS layouts are detected as `Unknown` and inherit the conservative default. Extending the enum to name them is tracked as future work.

## Detailed entries

### Ubuntu (20.04+)

- **ISO family:** `ubuntu-24.04.1-desktop-amd64.iso` and friends
- **Layout:** hybrid GPT. `casper/` holds the signed kernel + `initrd`. `EFI/BOOT/bootx64.efi` is Canonical-signed shim.
- **`KEXEC_SIG` disposition:** The shipped kernel is signed by `Canonical Ltd. Master Certificate Authority`. Any rescue kernel that includes Canonical's CA in its platform/MOK keyring will accept it.
- **Verified via:**
  ```bash
  qemu-system-x86_64 \
      -enable-kvm -m 2G -bios /usr/share/OVMF/OVMF_CODE.fd \
      -cdrom ubuntu-24.04.1-desktop-amd64.iso
  ```
- **Status in aegis-boot:** expected to work out-of-the-box once `rescue-tui` is paired with an Ubuntu-signed rescue kernel.

### Debian (12+)

- **ISO family:** `debian-live-12.*-amd64-standard.iso`
- **Layout:** hybrid. `install.amd/vmlinuz` + `live/initrd.img`.
- **`KEXEC_SIG` disposition:** kernel signed by `Debian Secure Boot CA`. Matches Debian-derived rescue kernels; kexec-ing Debian→Debian is verified. Debian → Ubuntu works because both accept each other's CAs in the shim keyring.
- **Status:** expected to work; same caveat as Ubuntu re rescue-kernel pairing.

### Fedora (39+)

- **ISO family:** `Fedora-Workstation-Live-x86_64-*.iso`
- **Layout:** `images/pxeboot/vmlinuz`. Signed by `fedoraca`.
- **Known quirk — `CrossDistroKexecRefused`:** RHEL-lineage kernels (Fedora, RHEL, Rocky, Alma) enforce an additional keyring check inside `kexec_file_load` that rejects kernels signed by a non-RHEL-family CA even when `KEXEC_SIG` itself would accept. If `aegis-boot` ships with a Debian-CA rescue kernel, kexec-ing *into* a Fedora ISO may fail with `EPERM` rather than `EKEYREJECTED`. `iso-probe` surfaces this so the TUI can preflight-warn.
- **Mitigation paths (deployment decisions):**
  1. Ship a Fedora-signed rescue kernel for Fedora-heavy deployments.
  2. Document that users should enroll their ISO's signing key via `mokutil` and retry.

### Arch Linux

- **ISO family:** `archlinux-*-x86_64.iso`
- **Layout:** `arch/boot/x86_64/vmlinuz-linux` + corresponding `initramfs-linux.img`.
- **Known quirk — `UnsignedKernel`:** Arch install media does **not** carry a signed EFI executable chain by default. There is no shim-review-board-approved shim for Arch; the kernel itself is unsigned relative to the Microsoft UEFI CA. Under Secure Boot, `kexec_file_load` returns `EKEYREJECTED`.
- **Remedy surfaced by TUI:** enroll the Arch signing key via `mokutil --import` and reboot; `aegis-boot` never suggests disabling Secure Boot.

### Alpine Linux

- **ISO family:** `alpine-standard-*-x86_64.iso`
- **Layout:** `boot/vmlinuz-lts` + `boot/initramfs-lts`.
- **Detected as:** `Distribution::Unknown` (the current `iso-parser` detector doesn't name Alpine; future work).
- **Quirk inheritance:** `UnsignedKernel` via the `Unknown` default.
- **Reality:** Alpine's standard ISOs are unsigned against the Microsoft UEFI chain. Alpine distributes UEFI-capable images, but without shim review-board membership. Same MOK remedy as Arch.

### NixOS

- **ISO family:** `nixos-*-minimal-x86_64-linux.iso`
- **Layout:** `boot/bzImage` + generation-specific initrd path.
- **Detected as:** `Distribution::Unknown`.
- **Quirk inheritance:** `UnsignedKernel`.
- **Reality:** NixOS's default ISO builder does not produce a signed chain. Users can build custom SB-enabled images; those would flip this row to "signed" but require project-specific provenance to auto-detect.

## Out of the matrix (explicitly)

| Category          | Why excluded                                      |
|-------------------|---------------------------------------------------|
| Windows installers | Proprietary; hybrid ISOs depend on `bootmgr.efi` rather than a Linux kernel. Not a kexec target. |
| VMware ESXi       | ISOs expect to be dd'd to a whole device; loop-mount discovery would find the kernel but kexec wouldn't boot cleanly. |
| ChromeOS recovery | Not a general-purpose bootable image.             |

Support for any of the above would be a new ADR, not an entry here.

## Updating this matrix

1. Add or edit a row in the summary table.
2. Add the detail section below.
3. Update [`iso_probe::lookup_quirks`](../../crates/iso-probe/src/lib.rs) and its tests in the same commit.
4. If the change requires a new `Distribution` variant, that's an `iso-parser` change — land it in a precursor PR first.

## Revisit triggers

- A new LTS Ubuntu / Debian release changes the default boot layout (recheck `casper/` vs `live/`).
- Fedora changes its `kexec` policy (would flip the `CrossDistroKexecRefused` quirk).
- Microsoft updates the UEFI CA in a way that causes mass shim re-issuance (would affect every row).

Tracked against [`revisit triggers` in ADR 0001](../adr/0001-runtime-architecture.md#revisit-triggers).
