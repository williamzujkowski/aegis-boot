# USB image layout

This document describes the on-disk layout of an aegis-boot USB stick built by [`scripts/mkusb.sh`](../scripts/mkusb.sh), and explains how users drop ISO files onto the data partition.

## At a glance

```
┌─────────────────────────────────────────────────────────┐
│  GPT partition table                                    │
├─────────────────────────────────────────────────────────┤
│  Part 1 — ESP (FAT32, label AEGIS_ESP, ~<!-- constants:BEGIN:ESP_SIZE_MB -->400 MB<!-- constants:END:ESP_SIZE_MB -->)         │
│    /EFI/BOOT/BOOTX64.EFI   ← MS-signed shim             │
│    /EFI/BOOT/grubx64.efi   ← Canonical-signed grub      │
│    /EFI/BOOT/grub.cfg                                   │
│    /EFI/ubuntu/grub.cfg    (same; signed grub looks here) │
│    /vmlinuz                ← Canonical-signed kernel    │
│    /initrd.img             ← distro initrd + aegis initramfs │
├─────────────────────────────────────────────────────────┤
│  Part 2 — Data (exFAT, label AEGIS_ISOS, remainder)     │
│    ubuntu-24.04.1-desktop-amd64.iso                     │
│    fedora-workstation-41-x86_64.iso                     │
│    debian-live-12.7.0-amd64-standard.iso                │
│    (user drops .iso files here)                         │
└─────────────────────────────────────────────────────────┘
```

## Why two partitions

- **ESP** holds the signed boot chain. Its contents don't change between boots — read-only in normal operation. FAT32 is required by the UEFI spec; partition type GUID is `C12A7328-F81F-11D2-BA4B-00A0C93EC93B` (sgdisk type `ef00`).
- **AEGIS_ISOS** holds the user's own content. The user mounts this partition on their host, drops `.iso` files into it, unmounts, and boots. The rescue environment scans it and populates the TUI list.

Splitting them means:
- The user can **replace the ISO set** without reflashing the boot chain.
- The signed boot chain is **immutable in practice** — reduces the surface for accidental corruption.
- The data partition can be **reformatted** if needed without touching the ESP.

## Filesystem choice for AEGIS_ISOS

The data partition defaults to **exFAT** as of [#243](https://github.com/williamzujkowski/aegis-boot/issues/243) (was FAT32 prior). exFAT is natively read/write on Linux 5.7+, macOS, and Windows, and has **no per-file size limit** — Win11 / Rocky DVD / Ubuntu Desktop drop straight onto the stick.

The `DATA_FS` environment variable selects the filesystem at mkusb time:

| `DATA_FS` value | Per-file limit | Linux | macOS | Windows | Use case |
|---|---|---|---|---|---|
| `exfat` (default) | none | r/w (5.7+) | r/w native | r/w native | the obvious default |
| `fat32` (legacy) | **4 GB − 1 byte** | r/w native | r/w native | r/w native | maximum compatibility, capped |
| `ext4` | none | r/w native | r/w via FUSE | r/w via Ext2Fsd / Paragon | Linux-only fleet |

```bash
# Default (exfat, no per-file cap, cross-OS r/w):
sudo aegis-boot flash /dev/sdX

# Opt-in legacy fat32 (capped):
DATA_FS=fat32 sudo aegis-boot flash /dev/sdX

# Opt-in ext4 (Linux-only writes from the host):
DATA_FS=ext4 sudo aegis-boot flash /dev/sdX
```

For an opt-in `DATA_FS=fat32` stick, `aegis-boot add` refuses oversized ISOs before the copy starts, naming both the exfat default and the ext4 escape hatch. See [TROUBLESHOOTING.md § "ISO too large for FAT32"](./TROUBLESHOOTING.md#iso-too-large-for-fat32) for the exact error text.

**Windows installer caveat.** Win10/Win11 ISOs surface in rescue-tui's list with the `[X] not kexec-bootable` glyph. They cannot actually be kexec-booted — Windows uses `bootmgr.efi` + the NT loader, not a Linux kernel. aegis-boot surfaces them as a specific diagnostic rather than silently hiding them so the operator isn't left wondering where their ISO went.

The data-partition GUID is `0700` (Microsoft Basic Data) for both exFAT and FAT32, and `8300` (Linux filesystem) for ext4. All three mount fine from Linux; the type only matters for auto-mount behavior on other OSes.

## Building

```bash
# Needs: shim-signed, grub-efi-amd64-signed, linux-image-virtual or -generic,
# mtools, dosfstools, exfatprogs (for the #243 default), gdisk. All in
# the standard Ubuntu/Debian repos (exfatprogs is in main since 22.04).
sudo apt-get install -y \
    shim-signed grub-efi-amd64-signed linux-image-virtual \
    mtools dosfstools exfatprogs gdisk

# Kernel reads require root (kernels are mode 0600 by default on modern Ubuntu).
# Catches both -virtual and -generic suffixes.
sudo chmod 0644 /boot/vmlinuz-* /boot/initrd.img-*

./scripts/build-initramfs.sh   # produces out/initramfs.cpio.gz
./scripts/mkusb.sh             # produces out/aegis-boot.img
```

Default output: 2 GB image. Override with `DISK_SIZE_MB=8192 ./scripts/mkusb.sh` for more ISO capacity.

## Adding ISOs (loop mount)

```bash
sudo losetup -fP out/aegis-boot.img
# losetup assigns /dev/loopN; find which one:
LOOP=$(sudo losetup -j out/aegis-boot.img | cut -d: -f1)
mkdir -p /mnt/aegis-isos
sudo mount "${LOOP}p2" /mnt/aegis-isos    # partition 2 is AEGIS_ISOS
sudo cp ~/Downloads/ubuntu-24.04.iso /mnt/aegis-isos/
sudo umount /mnt/aegis-isos
sudo losetup -d "$LOOP"
```

## Testing under QEMU

Verify your image boots before committing to `dd`-ing to a real stick:

```bash
./scripts/qemu-try.sh
# Boots with OVMF SecBoot enforcing — Ctrl-A X to exit.
```

## Deploying to a real USB stick

```bash
# WARNING: this destroys all data on /dev/sdX. Verify the device!
lsblk                                               # find your stick
sudo dd if=out/aegis-boot.img of=/dev/sdX \
       bs=4M oflag=direct status=progress conv=fsync
sync
```

After `dd` completes, the data partition is still writable. Remount it on your host and drop ISOs onto it at any time:

```bash
sudo mount /dev/sdX2 /mnt/aegis-isos
sudo cp *.iso /mnt/aegis-isos/
sudo umount /mnt/aegis-isos
```

## Storage modules shipped in the initramfs

For rescue-tui to see the AEGIS_ISOS partition (or any internal disk the operator might want to scan), the initramfs has to include the kernel modules for the storage controller. Most modern Ubuntu generic kernels (6.8+) compile these as modules, not built-in. As of v0.7.0 (#72), `build-initramfs.sh` ships (current as of v0.12.0):

- `libahci`, `ahci` — SATA AHCI controllers (most desktops + older laptops)
- `nvme-core`, `nvme` — NVMe SSDs (most modern laptops)
- `usb-storage`, `uas` — USB mass storage (the deployment path)
- `nls_utf8` — vfat fallback charset (the default `iocharset=utf8` is otherwise a missing module)

`scsi_mod`, `sd_mod`, `usbcore`, `xhci-hcd/pci`, `ehci-hcd/pci`, and `loop`/`isofs`/`udf` are typically built-in on Ubuntu kernels and skipped at build time with an INFO log. `/init` modprobes the full set early; built-in modules return successfully without doing anything.

Real-hardware shakedown was completed in v0.12.0 ([#109](https://github.com/williamzujkowski/aegis-boot/issues/109)) on a SanDisk Cruzer 32 GB stick via QEMU USB-passthrough: Alpine 3.20.3 returned the expected `errno 61` refusal under enforcing Secure Boot, and Ubuntu 24.04.2 successfully kexec'd through (`kexec_core: Starting new kernel`). Multi-vendor real-hardware sweep (Framework / ThinkPad / Dell direct boot) is tracked in [#51](https://github.com/williamzujkowski/aegis-boot/issues/51) for v1.0.0.

## Chain of trust recap

See [THREAT_MODEL.md](../THREAT_MODEL.md) for the full model. In short:

- UEFI firmware validates `/EFI/BOOT/BOOTX64.EFI` (shim) against `db`/`dbx`.
- shim validates `grubx64.efi` via its built-in vendor cert.
- grub validates `/vmlinuz` via shim's keyring.
- Kernel unpacks the combined initrd (distro + ours) and runs our `/init`.
- `rescue-tui` loads, user picks an ISO, `kexec_file_load` validates the target kernel against the platform keyring (enforced by `KEXEC_SIG` when SB is on).
- Unsigned ISO kernels surface `SignatureRejected` — the TUI tells the user to enroll the key via `mokutil`, never to disable SB.

## Limitations

- **Single-file 4 GB FAT32 cap** on the opt-in `DATA_FS=fat32` legacy data partition (the default since #243 is exFAT, which has no per-file limit).
- **User responsibility**: ISOs placed on the data partition are trusted by default. The TUI displays hash-verification and signature-verification status but doesn't block boot on a missing sidecar — that's deployment policy, not a mkusb concern.
- **x86_64 only** — aarch64 / riscv64 variants are a separate epic.
