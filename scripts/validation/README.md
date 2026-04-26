# ADR 0003 validation harness

Reusable scripts for exercising the cross-reboot-persistence paths against real USB hardware. These were first used for the 2026-04-22 addenda in `docs/validation/REAL_HARDWARE_REPORT_132.md`.

## Prerequisites

- Linux host with libvirt / QEMU + OVMF SecBoot 4M firmware (`/usr/share/OVMF/OVMF_CODE_4M.secboot.fd` and `OVMF_VARS_4M.ms.fd`)
- Passwordless sudo (for `/dev/sda` raw access + mount/umount)
- A removable USB stick the operator is willing to overwrite (the harness documents the device path; it does NOT auto-detect)
- Rust toolchain capable of building `aegis-bootctl` + `rescue-tui` at MSRV 1.95+

The harness follows the 9-step procedure in [`docs/validation/REAL_HARDWARE_REPORT_132.md`](../../docs/validation/REAL_HARDWARE_REPORT_132.md).

## What each script does

### `save_smoke.rs`

Standalone Rust binary that exercises the **exact same write protocol** as `persistence::save_durable` (see `crates/rescue-tui/src/persistence.rs:184`):

1. Write `last-choice.json.tmp` (fresh content)
2. Rename `.tmp` over `last-choice.json` (atomic on Linux + exfat.ko ≥ 5.7)
3. Open the state directory + `sync_all()` (flushes the rename to flash)

Iterates `ITERS` times and writes to the AEGIS_ISOS mount specified by `AEGIS_ISOS_MOUNT`. Use for:

- **Happy-path throughput** — sequential timing per save
- **Kill-mid-save durability** — launch in background, SIGKILL at random points, check state integrity

### `boot-ovmf.sh`

Boots the flashed stick under QEMU with OVMF SecBoot **enforcing** + USB passthrough of `/dev/sda`. Serial → stdout. Used for Phase A (clean boot) and Phase B (seeded-state restore) validation. Timeout-bounded (`TIMEOUT_S`, default 180s).

### `kill-mid-save.sh`

Harness wrapping `save_smoke`: runs N trials, each launching save_smoke in the background, SIGKILLing at a random millisecond offset in 10–500ms, then mounting the stick and checking:

- `last-choice.json` exists → must parse as valid JSON
- Stale `.tmp` files are reported but accepted (load path reads only `last-choice.json`)

Reports `PASS=N CORRUPT=M` summary.

## How to run

```bash
# 1. Build the harness binary + build aegis-boot artifacts
cd <repo-root>
rustup run 1.95.0 cargo build --release -p aegis-bootctl -p rescue-tui
rustc scripts/validation/save_smoke.rs -O -o /tmp/save_smoke
bash scripts/build-initramfs.sh

# 2. Flash the target stick (DESTRUCTIVE — overwrites the whole disk)
sudo ./target/release/aegis-boot flash --direct-install --yes --out-dir ./out /dev/sda

# 3. Copy a couple of real ISOs onto AEGIS_ISOS (iso-probe needs valid ISO9660)
#    At minimum 2 to validate cursor-position scenarios:
#    sudo mount /dev/sda2 /mnt && cp some-distro*.iso /mnt/ && sudo umount /mnt

# 4. Flip GRUB default → serial-primary so we get serial console output
sudo mount /dev/sda1 /mnt
sudo sed -i 's/^set default=0$/set default=1/' /mnt/EFI/BOOT/grub.cfg
sudo umount /mnt

# 5. Phase A — clean boot
AEGIS_ISOS_DEV=/dev/sda bash scripts/validation/boot-ovmf.sh

# 6. Phase B — seeded-state restore
#    a) Write .aegis-state/last-choice.json pointing at a specific ISO
#    b) Re-run boot-ovmf.sh, watch for:
#         rescue-tui: restored last choice idx=<N> iso=<path>
sudo mount /dev/sda2 /mnt && sudo mkdir -p /mnt/.aegis-state && \
    sudo tee /mnt/.aegis-state/last-choice.json <<'JSON'
{
  "iso_path": "/run/media/aegis-isos/my-target.iso",
  "cmdline_override": null
}
JSON
sudo umount /mnt && sudo sync
AEGIS_ISOS_DEV=/dev/sda bash scripts/validation/boot-ovmf.sh

# 7. Phase C — save under duress
RUNS=10 bash scripts/validation/kill-mid-save.sh
```

## Known limitations

- **Scripted TUI drive is fragile.** Automating rescue-tui's interactive pick → confirm flow through QEMU serial + crossterm ratatui is brittle. The kill-mid-save harness bypasses this by exercising `save_durable`'s byte-level protocol directly. The full mid-kexec physical power-pull is still a human step.
- **`SIGKILL` ≠ real power cut.** The kernel page cache above the flash isn't forced. Since `save_durable` calls `sync_all()` on the directory FD before returning, the rename is persisted before the protocol completes — but the flash barrier question is device-specific. Treat these as lower-bound durability validation.
- **Framework Laptop only here.** Per-vendor firmware quirks (Dell, ThinkPad) still need manual runs on real hardware.

## Non-destructive live-hardware procedures

Procedures for operators with access to hardware they're NOT willing to reimage. Both validate the trust chain against real firmware + real USB stack without touching the installed OS.

### iDRAC / iLO virtual-media "can the BMC see it?" test

**Goal:** confirm a BMC-emulated mass-storage boot path enumerates the aegis-boot stick as a bootable device. Does NOT actually boot the server into rescue-tui — stops at the BIOS boot-device selector.

**Prerequisites:**
- Server with iDRAC / iLO / OpenBMC remote-console access
- aegis-boot-flashed USB stick (flashed on a separate host) OR a copy of `out/aegis-boot.img` (raw disk image) to attach as virtual media
- Server's UEFI boot-menu key (F11 on Dell, F9 on HP, F12 on Supermicro)

**Steps:**

1. On a separate host, `aegis-boot flash --direct-install --yes --out-dir ./out /dev/sdX` to produce a flashed stick. Keep the `out/aegis-boot.img` around — iDRAC wants the raw image.
2. Log into iDRAC / iLO / OpenBMC web UI. Go to Virtual Console → Virtual Media → Map Removable Disk (or Map USB) → select `out/aegis-boot.img`.
3. In the iDRAC/iLO Virtual Console, trigger a "warm reboot" or "graceful power cycle" (NOT a factory-reset). Server reboots into BIOS POST.
4. Press the boot-menu key during POST (F11 / F9 / F12). The firmware's one-shot boot-device selector appears.
5. **Observe:** does a device named something like `Virtual Disk 1`, `iDRAC Virtual CD/DVD`, `USB: iDRAC Virtual Media`, or `UEFI: BMC Virtual Media, Partition 1` appear in the list? Screenshot / note the exact name.
6. **Abort boot:** use the boot menu's "cancel" or press Esc to return to normal boot order → server boots its normal OS unchanged.
7. Unmap the virtual media in iDRAC/iLO to clean up.

**What this validates:**
- BMC-emulated USB presents our GPT partition table correctly (the firmware's boot menu only lists devices whose partition table it can parse)
- The ESP on partition 1 is recognized as bootable (firmware flags it as a UEFI boot candidate)
- Our `shim + grub` don't fail BMC-virtual-USB descriptor-read quirks at POST

**What this does NOT validate:**
- kexec under the real kernel (stopped before that)
- Actually booting rescue-tui (stopped before that)
- Writing to the stick (read-only attach)

**Capture for bug filing (if it fails):** BMC vendor + version, server model, BIOS version, exact error or list of what DID appear in the boot menu.

### Dell laptop non-destructive boot test

**Goal:** full flash → boot → rescue-tui → live-kexec into Alpine (no install, all in RAM). Leaves the laptop's installed OS untouched.

**Prerequisites:**
- Dell laptop (Latitude / OptiPlex / Precision — Phoenix or Insyde firmware)
- aegis-boot-flashed USB stick with Alpine 3.20 Standard or similar live ISO added
- Laptop's boot-menu key (F12 on Dell)

**Steps:**

1. Flash the stick on a separate host: `aegis-boot flash --direct-install --yes --out-dir ./out /dev/sdX && aegis-boot add alpine-3.20-standard /dev/sdX` (or your preferred ISO slug).
2. Power off the Dell laptop. Plug in the stick.
3. Power on, press F12 repeatedly during POST to open the one-shot boot menu.
4. Select the USB device entry (often labeled `USB Storage Device` or the stick's model name).
5. **Expected:** grub menu appears → auto-selects `aegis-boot rescue` after 3s → kernel loads → you see `init: aegis-boot /init starting (PID 1)` → eventually rescue-tui's ISO list.
6. Navigate in rescue-tui to Alpine, press Enter → confirmation dialog → Enter again.
7. **Expected:** kexec banner, kernel reboot into Alpine's live environment. Alpine runs in RAM; the laptop's installed OS is untouched.
8. To exit: shutdown from Alpine (or power-cycle). Remove the stick, reboot back into the laptop's normal OS.

**What this validates:**
- Dell-branded UEFI firmware boots the stick under Secure Boot enforcing
- Real kernel loads with real drivers (Dell GPU / NIC / storage)
- kexec succeeds from rescue-tui → Alpine live
- Combined initrd loads correctly on non-Framework hardware

**What this does NOT validate:**
- The Dell is reimaged (it isn't — Alpine live is RAM-only)
- Any specific ISO kexec behavior beyond Alpine Standard

**Capture:** Dell model + year, firmware vendor (Phoenix / Insyde) + version, whether rescue-tui appeared, whether Alpine booted to a login prompt. File a `hardware: <vendor model> — success/failure` issue with the captured info, matching Gary's #328 template.

### Safety / rollback

Both procedures are non-destructive by design:
- iDRAC virtual-media attach is read-only (BMC reads from the .img; doesn't write to it)
- Dell laptop boot path loads rescue kernel + initrd into RAM, never touches the installed disk
- If anything goes wrong, physically remove the stick (laptop) or unmap virtual media (iDRAC) and power-cycle — the installed OS returns to its prior state

## References

- [ADR 0003 `LAST_BOOTED_PERSISTENCE.md`](../../docs/architecture/LAST_BOOTED_PERSISTENCE.md)
- [`REAL_HARDWARE_REPORT_132.md`](../../docs/validation/REAL_HARDWARE_REPORT_132.md)
- [`persistence.rs`](../../crates/rescue-tui/src/persistence.rs) — the save/load implementation under test
- [Gary's ASRock report #328](https://github.com/aegis-boot/aegis-boot/issues/328) — the one non-Framework data point we already have
