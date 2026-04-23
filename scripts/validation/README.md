# ADR 0003 validation harness

Reusable scripts for exercising the cross-reboot-persistence paths against real USB hardware. These were first used for the 2026-04-22 addenda in `docs/validation/REAL_HARDWARE_REPORT_132.md`.

## Prerequisites

- Linux host with libvirt / QEMU + OVMF SecBoot 4M firmware (`/usr/share/OVMF/OVMF_CODE_4M.secboot.fd` and `OVMF_VARS_4M.ms.fd`)
- Passwordless sudo (for `/dev/sda` raw access + mount/umount)
- A removable USB stick the operator is willing to overwrite (the harness documents the device path; it does NOT auto-detect)
- Rust toolchain capable of building `aegis-bootctl` + `rescue-tui` at MSRV 1.88+

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
rustup run 1.88.0 cargo build --release -p aegis-bootctl -p rescue-tui
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

## References

- [ADR 0003 `LAST_BOOTED_PERSISTENCE.md`](../../docs/architecture/LAST_BOOTED_PERSISTENCE.md)
- [`REAL_HARDWARE_REPORT_132.md`](../../docs/validation/REAL_HARDWARE_REPORT_132.md)
- [`persistence.rs`](../../crates/rescue-tui/src/persistence.rs) — the save/load implementation under test
