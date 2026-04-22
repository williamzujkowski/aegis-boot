# Troubleshooting

Common errors operators hit, what they mean, and how to fix them. Organized by where you're likely to see the problem (workstation, firmware, rescue-tui, kexec).

If your problem isn't here, file an issue with the output of `aegis-boot --version`, the host distro, the target firmware vendor + UEFI mode, and the relevant logs (kernel ring buffer for kexec issues; rescue-tui's stderr / journalctl for TUI issues).

---

## Workstation (writing the stick)

### "No removable USB drives detected"

`aegis-boot flash` enumerates `/sys/block/sd*` filtering by `removable=1`. If your stick doesn't show:

1. `lsblk -o NAME,SIZE,TRAN,RM` — confirm the kernel sees it as `RM=1`. If `RM=0`, the device is not flagged removable (some "USB SSDs" present as fixed). Pass the device explicitly: `sudo aegis-boot flash /dev/sdX`.
2. `dmesg | tail` — look for the device enumeration line. If absent, the host didn't see the plug event. Replug.
3. Some USB-C hubs drop hot-plug events. Plug the stick directly into the host.

### "/dev/sdX is not a removable drive (or not detected as one)"

You passed an explicit device, but the kernel reports `removable=0`. Either you're targeting the wrong device (verify with `lsblk`) or the stick presents as fixed. To override the safety check, you'd need to use `dd` directly — but only after you're certain. Mistargeting destroys the wrong drive.

### "mkusb.sh exited with N"

The build script failed before `dd` ran. Inspect the script's stderr (printed inline by `aegis-boot flash`). Common causes:

- `chmod 0644 /boot/vmlinuz-* /boot/initrd.img-*` not run after a kernel update — the script can't read the kernel.
- Missing `shim-signed` / `grub-efi-amd64-signed` packages.
- `mtools` / `dosfstools` / `exfatprogs` / `gdisk` not installed.

See [BUILDING.md](../BUILDING.md) and [LOCAL_TESTING.md § "One-time setup"](./LOCAL_TESTING.md#one-time-setup).

### `dd` exited with a non-zero status partway through

The stick may have failed (worn-out NAND), the cable may be flaky, or the host may have hit ENOSPC on the source side. Re-run after a replug; if the second attempt also fails on the same write offset, the stick is bad — get another.

### "not enough free space" from `aegis-boot add`

You're trying to add an ISO larger than what's free on `AEGIS_ISOS`. Either:

- Free space: `rm` something via the host mount, or use `aegis-boot list` to see what's there and decide.
- Reflash with a larger image: `DISK_SIZE_MB=8192 sudo aegis-boot flash` (or set the env on `mkusb.sh` if you build manually).

### ISO too large for FAT32

This only fires on **legacy `DATA_FS=fat32` sticks** — the default since [#243](https://github.com/aegis-boot/aegis-boot/issues/243) is exFAT, which has no per-file size limit. If you hit this on a current-default stick, you've reflashed with `DATA_FS=fat32` somewhere along the way.

You'll see an error like:

```
aegis-boot add: Win11_25H2_English_x64_v2.iso is 7.4 GiB — exceeds
  FAT32's 4 GiB per-file ceiling.
  The AEGIS_ISOS partition is formatted as vfat, which cannot store
  files at or above 4 GiB. Reflash with the new exfat default to lift
  the ceiling (preserves cross-OS r/w on Linux + macOS + Windows):

      sudo aegis-boot flash /dev/sdX

  Or, for a Linux-only stick, use ext4:

      DATA_FS=ext4 sudo aegis-boot flash /dev/sdX
```

FAT32 caps individual files at 4 GiB minus one byte — this affects Ubuntu LTS Desktop (~5.8 GB), Rocky 9 DVD (~10 GB), Windows 10 installer (~5.5 GB), and Windows 11 installer (~7.9 GB). The cleanest fix is to drop the `DATA_FS=fat32` opt-in and reflash with the default exfat:

```bash
sudo aegis-boot flash /dev/sdX
```

The destructive reflash will wipe existing ISOs — back up first with `aegis-boot list /dev/sdX` to see what's there.

If for some reason exfat is unsuitable (very old firmware that doesn't enumerate exfat partitions in its boot menu), use ext4 — but be aware ext4 isn't natively writable from macOS / Windows (Linux-only "drop files on the host" workflow, or use `ext4fuse` on macOS / `Ext2Fsd` on Windows).

### Windows installer ISO on the stick doesn't boot

Expected. Windows installers use `bootmgr.efi` + the NT loader, not a Linux kernel — they can't be kexec-booted through aegis-boot's signed chain. rescue-tui surfaces them with an `[X] not kexec-bootable` glyph for exactly this reason (rather than silently hiding them).

To install Windows on a target machine, write the Windows ISO directly to a separate stick using `dd` or Rufus. aegis-boot's signed-chain model is specific to Linux kernels that participate in Secure Boot's PE signature check.

---

## Firmware / boot menu

### Stick won't appear in the firmware boot menu

1. Confirm Secure Boot is **enforcing** (not disabled, not "Audit"). aegis-boot is built for enforcing SB; on a disabled-SB host it will still boot, but you're not getting the protections you wanted.
2. Confirm the firmware is in **UEFI mode**, not CSM/Legacy. aegis-boot ships only a UEFI boot image; CSM won't see it. Boot menu options like "USB-HDD" (legacy) vs "UEFI: USB Storage" (UEFI) are the tell.
3. Some vendors require the stick to be plugged in *before* power-on for the firmware to enumerate it.
4. Try a different USB port. xHCI ports usually work; older EHCI/UHCI ports sometimes need a kernel driver the firmware doesn't have.

### Firmware says "Secure Boot violation" loading the stick

shim's vendor cert isn't in your firmware's `db`. This is rare on consumer hardware (Microsoft's CA is essentially universal) but happens on locked-down corporate fleets. Options:

- Enroll Microsoft's UEFI CA in your firmware's `db` (vendor-specific UI).
- Re-sign shim with your own KEK (advanced; requires custom signing chain — see [#24](https://github.com/aegis-boot/aegis-boot/issues/24) tracking).

---

## rescue-tui

### TUI shows no ISOs

1. Did you actually copy ISOs to `AEGIS_ISOS`? Verify from the host with `aegis-boot list`.
2. The TUI scans `AEGIS_ISO_ROOTS` (default `/run/media:/mnt`). The initramfs auto-mounts the partition under `/run/media/aegis-isos/`. If you've changed `AEGIS_ISO_ROOTS` on the kernel cmdline, the partition might not be in the search path.
3. Check the journal: at the rescue shell, `journalctl -b 0 -u rescue-tui` (or read `/var/log/messages`).

### TUI shows ISOs but no kernel detected

`iso-probe` couldn't parse the ISO's boot config. Most distros use isolinux / GRUB / EFI configs that we know how to parse, but some custom or very old images don't follow the standard layout. The Confirm screen will show "no kernel found" and refuse to boot. Workarounds:

- Use a current image from the distro vendor instead of a re-spun community build.
- File an issue with the ISO source URL — we may want to add parser support.

### Why is my ISO GRAY instead of GREEN?

GRAY = no verification sidecars present. The ISO will boot but rescue-tui requires a typed `boot` confirmation as a friction step (we don't silently accept an unverified ISO).

To make it GREEN, drop sidecars next to the ISO:

```bash
# Before flashing:
cp ubuntu-24.04.2-live-server-amd64.iso ./
sha256sum ubuntu-24.04.2-live-server-amd64.iso > ubuntu-24.04.2-live-server-amd64.iso.sha256
# Then:
aegis-boot add ubuntu-24.04.2-live-server-amd64.iso   # picks up the sidecar automatically
```

For minisign signatures: distros that publish `.minisig` files (or `SHA256SUMS.gpg` you've validated yourself) — use those when you can. The verification status in the TUI is just data; the operator decides whether to accept it.

---

## kexec hand-off

### `errno 61 (ENODATA)` — "kernel signature rejected"

Expected. The ISO's kernel isn't signed by a CA in the platform keyring. This is correct Secure Boot behavior — `kexec_file_load(2)` enforces the same signature policy as the boot loader. Two paths forward:

1. Use a distro-signed ISO (Ubuntu, Fedora, Debian, RHEL).
2. Enroll the distro's signing key via MOK — see [UNSIGNED_KERNEL.md](./UNSIGNED_KERNEL.md).

**Never** run `mokutil --disable-validation` to "fix" this. That's not a fix; it's disabling Secure Boot on the host.

### `errno 1 (EPERM)` — "operation not permitted"

Two common causes:

1. The TUI is running without `CAP_SYS_BOOT`. Shouldn't happen in the shipped initramfs, but if you've customized `/init`, verify.
2. Kernel lockdown is in `confidentiality` mode and refuses kexec entirely. This is rare on Ubuntu; some hardened distros set it. Check `/sys/kernel/security/lockdown`.

### `CrossDistroKexecRefused` quirk

Some kernels refuse to kexec other-vendor kernels even when the signature would otherwise verify. This isn't aegis-boot policy — it's the target kernel's own check. Documented per-distro in [docs/compatibility/iso-matrix.md](./compatibility/iso-matrix.md). Workaround: use an ISO whose kernel matches (or is closely related to) the rescue kernel's vendor.

### `kexec_core: Starting new kernel` then immediate hang

The new kernel loaded but its early init didn't survive the hand-off. Most common cause is an initrd that expects to be loaded by a specific bootloader and not by `kexec_file_load`. Open an issue with the kernel ring buffer captured before the hang (`dmesg -w` from the rescue shell on a separate session).

### Screen goes blank after the handoff banner — is it hung?

Probably not. The handoff banner ([#127](https://github.com/aegis-boot/aegis-boot/issues/127)) prints a "screen may go blank briefly while the new kernel loads" notice exactly because the framebuffer can drop signal during kexec. Wait 10-30 seconds. If still nothing:

- Some kernels need an explicit framebuffer cmdline (`fbcon=map:0` or `console=tty0 console=ttyS0,115200n8`). The ISO's own bootloader sets these; aegis-boot tries to preserve the ISO's `cmdline` but parsing failures fall back to a minimal cmdline.
- If you have serial access (real hardware with a serial header, or QEMU serial console), check there.

---

## Accessibility

### Colors are unreadable (low contrast / framebuffer / HDMI capture)

Set `AEGIS_THEME=high-contrast` (kernel cmdline: `aegis.theme=high-contrast`). Uses the bright ANSI variants only.

### Red/green verdicts are indistinguishable (color vision)

Set `AEGIS_THEME=okabe-ito` (aliases `cb` / `colorblind`). Swaps the verdict trio to the Okabe-Ito colorblind-safe palette: bluish-green for success, orange for warning, vermillion for error. Distinguishable under deuteranopia and protanopia.

### Serial console or screen reader strips color

Set `AEGIS_THEME=monochrome` (aliases `mono` / `none`) so verdicts render as the default terminal foreground. Combine with `AEGIS_A11Y=text` (or `TERM=dumb`, which auto-enables it) to skip the ratatui alt-screen and get a numbered-menu text renderer.

Every state transition also emits an `ANN: {human text}` line on stderr — a parallel stream readers can consume without parsing ratatui's draw buffer.

### Available themes

| Name            | Aliases                                     | Use case                                 |
| --------------- | ------------------------------------------- | ---------------------------------------- |
| `default`       | (unknown names fall back here)              | Standard VT100 16-color console          |
| `monochrome`    | `mono`, `none`                              | Serial / screen-reader / unreliable ANSI |
| `high-contrast` | `hc`, `high_contrast`                       | Low-contrast framebuffers, HDMI capture  |
| `okabe-ito`     | `cb`, `colorblind`, `okabe_ito`, `okabeito` | Colorblind-safe verdict trio             |
| `aegis`         | `brand`                                     | Project brand palette                    |

---

## When in doubt

Drop to the rescue shell from the List screen (navigate to the `[#] rescue shell` entry, press Enter). You're in a busybox shell with the full initramfs around you. Useful commands:

```sh
ls /run/media/aegis-isos/         # see what's mounted
dmesg | tail -50                   # recent kernel messages
cat /sys/kernel/security/lockdown  # kernel lockdown mode
mokutil --sb-state                 # SB enforcing state (if mokutil is shipped)
```

To exit and reboot: `reboot -f` (or power-cycle).

---

## Filing a bug

`.github/ISSUE_TEMPLATE/bug.yml` will prompt you for:

- aegis-boot version (`aegis-boot --version`)
- Host workstation distro + kernel (`uname -a`)
- Target firmware vendor + UEFI mode + Secure Boot state
- ISO that failed (vendor + version + URL if public)
- Reproduction steps
- Relevant logs

For security-relevant findings, **do not file a public issue** — see [SECURITY.md](../SECURITY.md) for the private path.
