# USB image layout

This document describes the on-disk layout of an aegis-boot USB stick built by [`scripts/mkusb.sh`](../scripts/mkusb.sh), and explains how users drop ISO files onto the data partition.

## At a glance

```
┌─────────────────────────────────────────────────────────┐
│  GPT partition table                                    │
├─────────────────────────────────────────────────────────┤
│  Part 1 — ESP (FAT32, label AEGIS_ESP, ~400 MB)         │
│    /EFI/BOOT/BOOTX64.EFI   ← MS-signed shim             │
│    /EFI/BOOT/grubx64.efi   ← Canonical-signed grub      │
│    /EFI/BOOT/grub.cfg                                   │
│    /EFI/ubuntu/grub.cfg    (same; signed grub looks here) │
│    /vmlinuz                ← Canonical-signed kernel    │
│    /initrd.img             ← distro initrd + aegis initramfs │
├─────────────────────────────────────────────────────────┤
│  Part 2 — Data (FAT32, label AEGIS_ISOS, remainder)     │
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

## File size limit on FAT32

FAT32 caps individual files at **4 GB minus 1 byte**. Typical distro ISOs:

| ISO | Size | Fits on FAT32? |
|---|---|---|
| Ubuntu LTS desktop | ~5.8 GB | ❌ exceeds |
| Fedora Workstation | ~2.3 GB | ✅ |
| Debian netinst / live standard | ~1 GB | ✅ |
| Arch | ~1.2 GB | ✅ |
| Alpine standard | ~200 MB | ✅ |
| NixOS minimal | ~900 MB | ✅ |

If you need to ship the full Ubuntu LTS desktop image: rebuild with:

```bash
DATA_FS=ext4 ./scripts/mkusb.sh
```

Trade-off: ext4 isn't natively writable from macOS or Windows, so the "drop files on the host" workflow requires Linux (or FUSE drivers like `ext4fuse` on macOS, `Ext2Fsd`/`Paragon` on Windows). Pick based on where you're dropping ISOs onto the stick.

The data-partition GUID changes with the filesystem: FAT32 gets `0700` (Microsoft Basic Data, cross-OS friendly) and ext4 gets `8300` (Linux filesystem). Both mount fine from Linux; the type only matters for auto-mount behavior on other OSes.

## Building

```bash
# Needs: shim-signed, grub-efi-amd64-signed, linux-image-virtual or -generic,
# mtools, dosfstools, gdisk. All in the standard Ubuntu/Debian repos.
sudo apt-get install -y \
    shim-signed grub-efi-amd64-signed linux-image-virtual \
    mtools dosfstools gdisk

# Kernel reads require root (kernels are mode 0600 by default on modern Ubuntu).
sudo chmod 0644 /boot/vmlinuz-*-virtual /boot/initrd.img-*-virtual

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

For rescue-tui to see the AEGIS_ISOS partition (or any internal disk the operator might want to scan), the initramfs has to include the kernel modules for the storage controller. Most modern Ubuntu generic kernels (6.8+) compile these as modules, not built-in. As of v0.7.0 (#72), `build-initramfs.sh` ships:

- `libahci`, `ahci` — SATA AHCI controllers (most desktops + older laptops)
- `nvme-core`, `nvme` — NVMe SSDs (most modern laptops)
- `usb-storage`, `uas` — USB mass storage (the deployment path)
- `nls_utf8` — vfat fallback charset (the default `iocharset=utf8` is otherwise a missing module)

`scsi_mod`, `sd_mod`, `usbcore`, `xhci-hcd/pci`, `ehci-hcd/pci`, and `loop`/`isofs`/`udf` are typically built-in on Ubuntu kernels and skipped at build time with an INFO log. `/init` modprobes the full set early; built-in modules return successfully without doing anything.

Real-hardware boot has been validated only in QEMU simulation as of v0.7.0 (`scripts/qemu-loaded-stick.sh --attach {virtio,sata,usb}`). A Framework / ThinkPad / Dell shakedown is tracked in [#51](https://github.com/williamzujkowski/aegis-boot/issues/51).

## Chain of trust recap

See [THREAT_MODEL.md](../THREAT_MODEL.md) for the full model. In short:

- UEFI firmware validates `/EFI/BOOT/BOOTX64.EFI` (shim) against `db`/`dbx`.
- shim validates `grubx64.efi` via its built-in vendor cert.
- grub validates `/vmlinuz` via shim's keyring.
- Kernel unpacks the combined initrd (distro + ours) and runs our `/init`.
- `rescue-tui` loads, user picks an ISO, `kexec_file_load` validates the target kernel against the platform keyring (enforced by `KEXEC_SIG` when SB is on).
- Unsigned ISO kernels surface `SignatureRejected` — the TUI tells the user to enroll the key via `mokutil`, never to disable SB.

## Limitations

- **Single-file 4 GB FAT32 cap** on the default data partition (see above).
- **User responsibility**: ISOs placed on the data partition are trusted by default. The TUI displays hash-verification and signature-verification status but doesn't block boot on a missing sidecar — that's deployment policy, not a mkusb concern.
- **x86_64 only** — aarch64 / riscv64 variants are a separate epic.
