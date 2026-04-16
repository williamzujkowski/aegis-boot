# Operator install guide

End-to-end: get aegis-boot onto a USB stick, drop ISOs onto it, boot a target machine, and pick an ISO to kexec into. Reading time: ~5 minutes. Hands-on time: ~10 minutes plus dd.

This guide assumes Linux on the operator workstation (where you write the stick). macOS / Windows flashing is tracked in [#123](https://github.com/williamzujkowski/aegis-boot/issues/123).

## Before you start

Have on hand:
- A USB stick (8 GB or larger recommended; aegis-boot itself fits in 2 GB but you want headroom for ISOs)
- One or more `.iso` files you want to be able to boot
- A target machine with UEFI Secure Boot **enforcing** (the whole point of aegis-boot — if you've disabled SB, you're paying for protections you're not getting)
- `sudo` on your workstation (we shell out to `dd`, `mount`, `umount`)

## Step 1 — write aegis-boot to the stick

```bash
sudo aegis-boot flash
```

What happens:
1. The CLI scans `/sys/block/sd*` for removable drives and shows you what it found:
   ```
   Detected removable drives:
     [1] /dev/sdc  SanDisk Cruzer Blade  29.8 GB  (1 partitions)
   ```
2. It asks you to confirm. If only one removable drive is present, `[Y/n]`. If more than one, `[1-N]`.
3. It asks for a typed `flash` confirmation (not `y/n`) — this is intentional friction because `dd` is destructive:
   ```
   ALL DATA ON /dev/sdc (SanDisk Cruzer Blade, 29.8 GB) WILL BE ERASED.
   Type 'flash' to confirm: flash
   ```
4. It builds the image (`scripts/mkusb.sh`), `dd`s it to the stick with `oflag=direct conv=fsync status=progress`, syncs, and runs `partprobe`.

Want to point at a specific drive instead of auto-detect? `sudo aegis-boot flash /dev/sdc`.

If `aegis-boot flash` says "No removable USB drives detected", the kernel doesn't see the stick as removable. Replug it; check `lsblk -o NAME,TRAN,RM`; the stick should show `RM=1` and `TRAN=usb`.

## Step 2 — add ISOs to the stick

The stick now has two partitions: `AEGIS_ESP` (signed boot chain, leave it alone) and `AEGIS_ISOS` (where you drop ISOs). The CLI handles the mount/copy/unmount cycle for you:

```bash
aegis-boot add ~/Downloads/ubuntu-24.04.2-live-server-amd64.iso
```

What happens:
1. The CLI reads `/proc/mounts` to find a currently-mounted `AEGIS_ISOS`. If none, it mounts the stick's partition 2 to a temporary directory.
2. It checks free space (refuses if `iso_size + 10 MiB headroom` doesn't fit).
3. It copies the ISO and any sibling sidecars: `<iso>.sha256`, `<iso>.SHA256SUMS`, `<iso>.minisig`. These let `rescue-tui` show a GREEN verification verdict instead of GRAY.
4. It unmounts the temporary mount (if it created one).

Verify:
```bash
aegis-boot list
```

```
ISOs on /tmp/aegis-cli-12345-0:

  [✓ sha256] [  minisig]    1.6 GiB  ubuntu-24.04.2-live-server-amd64.iso
  [  sha256] [  minisig]    198 MiB  alpine-3.20.3-x86_64.iso

2 ISO(s) total. Legend:
  ✓ sha256   sibling <iso>.sha256 present
  ✓ minisig  sibling <iso>.minisig present
  (missing sidecars mean the ISO will show GRAY verdict in rescue-tui)
```

Sidecars are optional but strongly recommended — see [TROUBLESHOOTING.md § "Why is my ISO GRAY?"](./TROUBLESHOOTING.md#why-is-my-iso-gray-instead-of-green).

## Step 3 — boot from the stick

1. Plug the stick into the target machine.
2. Open the firmware boot menu (commonly `F12`, `F11`, `Esc`, or `Del` at POST — varies by vendor).
3. Pick the USB entry. It will show as "USB" or the stick's vendor name.
4. shim verifies grub, grub verifies the kernel, the kernel runs `/init`, and `rescue-tui` starts.

If the firmware refuses to show the USB entry, your boot mode might be CSM/Legacy instead of UEFI — see [TROUBLESHOOTING.md § "Stick won't appear in boot menu"](./TROUBLESHOOTING.md#stick-wont-appear-in-the-firmware-boot-menu).

## Step 4 — pick an ISO

`rescue-tui` shows your ISOs with verification status:

```
aegis-boot v0.12.0    SB:enforcing  TPM:available

  ▸ ubuntu-24.04.2-live-server-amd64.iso        1.6 GiB  [✓ sha256]
    alpine-3.20.3-x86_64.iso                     198 MiB  [no verify]

[↑↓/jk] Move  [Enter] Boot  [/] Filter  [?] Help
```

Navigate with arrows (or `j/k`), press `Enter` to advance to the Confirm screen.

The Confirm screen shows the verdict (GREEN / YELLOW / GRAY), the discovered kernel + initrd paths, and any installer warnings. **If the ISO is an installer image** (Ubuntu live-server, Fedora netinst, Windows, etc.), the screen explicitly warns:

> Warning: This ISO contains an installer. If the ISO's own boot menu default is 'Install', DISKS ON THIS MACHINE MAY BE ERASED.

Press `Enter` again to commit. The TUI prints a handoff banner ("Booting … screen may go blank briefly") and invokes `kexec_file_load(2)`. The selected kernel takes over.

## Common outcomes

- ✅ **Signed kernel boots cleanly.** You see `kexec_core: Starting new kernel` followed by the ISO's own boot.
- ⚠️ **Unsigned kernel refused (`errno 61 ENODATA`).** The Error screen shows you the `mokutil --import` command. See [UNSIGNED_KERNEL.md](./UNSIGNED_KERNEL.md).
- ❌ **Cross-distro kexec quirk.** Some kernels refuse to kexec other-vendor kernels. See [docs/compatibility/iso-matrix.md](./compatibility/iso-matrix.md) for the per-distro table.

## Updating the stick later

The signed boot chain (ESP) doesn't change between releases for the same `mkusb.sh` output — only the ISO set on `AEGIS_ISOS` changes day-to-day. So:

- **Add or remove ISOs:** `aegis-boot add` / `rm` from a host mount. No reflash.
- **Update aegis-boot itself** (new release): rerun `sudo aegis-boot flash`. This rewrites the whole stick, including erasing your ISO set — back up `AEGIS_ISOS` first if you care about its contents.

## Where to go next

- [docs/CLI.md](./CLI.md) — full `aegis-boot` CLI reference
- [docs/TROUBLESHOOTING.md](./TROUBLESHOOTING.md) — common errors and fixes
- [docs/UNSIGNED_KERNEL.md](./UNSIGNED_KERNEL.md) — booting Alpine / Arch / NixOS via MOK enrollment
- [docs/USB_LAYOUT.md](./USB_LAYOUT.md) — what's actually on the stick (ESP + AEGIS_ISOS scheme)
