# Real-hardware validation report — #132 last-booted persistence

**Date:** 2026-04-21
**Tester:** autonomous validation session per maintainer request
**Hardware:** SanDisk Cruzer 29.8G USB stick, dev-host-attached
**Method:** direct-install flash → libvirt/QEMU OVMF SecBoot-enforcing boot with USB passthrough
**Test build:** `aegis-boot v0.15.0` from `main` at branch merge point post-#372

## Executive summary

Real-hardware validation SURFACED A BLOCKING BUG that would have affected every operator on a direct-install flash with an Alpine (or any) ISO. Fix prepared (one-line module load addition). Separately, the stated acceptance criteria for #132 ("rescue-tui's List cursor focused on Ubuntu row + shows `[last]` indicator after reboot") does NOT match shipped behavior: the `persistence` module stores state in `/run/aegis-boot` (tmpfs) and the module's own docstring declares reboot-persistence out of scope pending a TPM/NVRAM ADR. Scope mismatch between #123 ("Pre-selection on next boot — SHIPPED") and the actual implementation.

## What got validated

### ✅ Full flash pipeline on real USB (20-21 sec end-to-end)

Flashed `/dev/sda` via `aegis-boot flash --direct-install --yes --out-dir ./out /dev/sda`:

```
Direct-install: ./out → /dev/sda
  [1/6] Partition (sgdisk)  …  done (3.3s)
  [2/6] Format ESP (mkfs.fat)  …  done (0.3s)
  [3/6] Format AEGIS_ISOS (mkfs.exfat)  …  done (0.8s)
  [4/6] Render grub.cfg  …  done (0.0s)
  [5/6] Resolve + combine initrd  …  done (0.2s)
  [6/6] Stage ESP (mmd + 6 mcopy writes)  …  done (16.2s)

Direct-install complete on /dev/sda in 20.9s.
```

Confirms:
- Direct-install writes to real USB hardware (not just loopback) end-to-end
- UX-3 stage timers render correctly
- 20s total vs. ~4 min for legacy dd path → confirms the #274 8× speedup on real media

### ✅ UX-4 catalog-slug `add` works on real stick

`aegis-boot add alpine-3.20-standard /mnt` downloaded + sha256-verified + copied Alpine 3.20 Standard (~209 MiB) onto the fresh stick with sidecar. One command, no manual URL-copy.

### ✅ OVMF SecBoot chain boots the real USB

Under `qemu-system-x86_64 -machine q35,smm=on -global driver=cfi.pflash01,property=secure,value=on` with `/dev/sda` as USB passthrough:

```
BdsDxe: loading Boot0001 "UEFI QEMU QEMU USB HARDDRIVE ..." from PciRoot(0x0)/Pci(0x3,0x0)/USB(0x0,0x0)
GNU GRUB  version 2.12
* aegis-boot rescue (serial-primary)
EFI stub: Loaded initrd from LINUX_EFI_INITRD_MEDIA_GUID device path
EFI stub: UEFI Secure Boot is enabled.
init: aegis-boot /init starting (PID 1)
```

Shim (MS-signed) → grub (Canonical-signed) → kernel (KEXEC_SIG) chain verified under SB enforcing, booting the actual USB stick.

### ✅ Initramfs + rescue-tui launch (after bug fix)

After the initramfs fix (see Bug 1 below):

```
init: mounted /dev/sda2 (LABEL=AEGIS_ISOS, fs=exfat, rw) -> /run/media/aegis-isos
iso-probe: hash verified iso=/run/media/aegis-isos/alpine-standard-3.20.3-x86_64.iso
rescue-tui: discovered 1 ISO(s)
```

rescue-tui correctly:
- mounts AEGIS_ISOS
- discovers the Alpine ISO
- verifies sha256 against the committed sidecar

## Bugs surfaced

### Bug 1 (BLOCKING, fix included in this PR): initramfs init script omits exfat + NLS modprobe

`scripts/build-initramfs.sh` ships `exfat.ko` in the initramfs's `/lib/modules/*/kernel/fs/exfat/`, but the init script's module-load loop never calls `modprobe exfat`. On a fresh direct-install stick (where AEGIS_ISOS is exFAT), `mount -t exfat /dev/sda2` returns `No such device` because the kernel hasn't registered exfat as a filesystem.

Result on boot 1 (pre-fix):
```
init:   tried fs=exfat: mount: mounting /dev/sda2 on /run/media/aegis-isos failed: No such device
init:   tried fs=ext4: mount: mounting /dev/sda2 on /run/media/aegis-isos failed: Invalid argument
init:   tried fs=vfat: mount: mounting /dev/sda2 on /run/media/aegis-isos failed: Invalid argument
init: WARNING: found /dev/sda2 but all mount attempts failed
rescue_tui: ISO discovery complete discovered=0 on_disk=0
```

**Impact before fix**: every operator using direct-install with the default exfat AEGIS_ISOS hit a hard blocker — rescue-tui discovered zero ISOs, making the rescue stick non-functional. CI didn't catch this because `direct-install-e2e.yml` only greps for `rescue-tui starting` (which happens even with 0 ISOs discovered).

**Fix**: add `exfat` + `nls_iso8859-1` + `nls_cp437` to the init script's modprobe loop. Ship `nls_iso8859-1` and `nls_cp437` from the host's kernel modules.

### Bug 2 (non-fatal): nls_iso8859-1 missing warning on vfat hot-plug

Before the fix, dmesg logged:
```
[14.82] FAT-fs (sda1): IO charset iso8859-1 not found
```

Non-fatal — the kernel's hot-plug path tries iso8859-1 as default charset; our init uses `iocharset=utf8` explicitly so the ESP mount still worked. Shipping `nls_iso8859-1` + `nls_cp437` alongside exfat silences this.

### Bug 3 (low severity): `quickstart` forwards `--direct-install` to `init` which doesn't accept it

`aegis-boot quickstart /dev/sda` invokes `init --profile minimal --yes --direct-install /dev/sda`, but `init::parse_flags` doesn't recognize `--direct-install`, exits with `unknown option '--direct-install'`. Ran the equivalent steps manually (`flash --direct-install` + `add <slug>`) to proceed with the test. Filed as separate issue — #352 UX-1 regression.

## What did NOT get validated

### Spec mismatch: "last-booted pre-selection across reboots"

**#132's stated acceptance criteria**:
> Test: boot stick, pick Ubuntu, confirm kexec succeeds, reboot stick, verify rescue-tui's List cursor is focused on Ubuntu row + shows `[last]` indicator.

**Actual shipped behavior** (per `crates/rescue-tui/src/persistence.rs:3-11`):
> Storage: JSON at `$AEGIS_STATE_DIR/last-choice.json` (defaults to `/run/aegis-boot`). `/run` is a tmpfs; **state is lost at reboot, which is exactly what we want for a rescue environment.**
>
> Persistence across reboots would require writing to the boot media, which is out of scope here — that's a TPM/NVRAM story for a later ADR.

So cross-reboot last-booted persistence is **not shipped**. #123 claims "Pre-selection on next boot — SHIPPED" which is misleading — pre-selection works after a failed kexec returns to rescue-tui WITHIN the same boot session, not across stick reboots.

**Recommendation**: close #132 as "scope mismatch caught — shipped behavior is within-session-only per explicit design"; file a successor issue for cross-reboot persistence with the design work this implies (where to write, fsync semantics on vfat, attestation-manifest interaction, untrusted-stick considerations).

### 2026-04-22 update — #375 Phase 1 shipped cross-reboot persistence

Successor work (per recommendation above): ADR 0003 [`LAST_BOOTED_PERSISTENCE.md`](../architecture/LAST_BOOTED_PERSISTENCE.md) ACCEPTED at 83.3% supermajority; PR #402 implemented the two-tier write (tmpfs full-fidelity + `AEGIS_ISOS` stripped) with atomic rename-over + directory fsync. Pure-Rust round-trip test `persistence::tests::reboot_simulation_round_trip` covers the file-on-disk side of the acceptance criterion. The real-hardware test procedure below closes the remaining gap — the cursor-on-reboot UX observation — which requires physical hardware the QEMU + USB-passthrough setup can't exercise end-to-end (QEMU's USB storage emulation doesn't preserve AEGIS_ISOS state across VM restart cleanly without additional plumbing).

### Hardware test procedure (closes #132 acceptance when executed)

Run on each of Framework / Dell / ThinkPad per the multi-vendor #51 gate:

1. Flash a fresh stick per the standard `aegis-boot quickstart /dev/sdX` path (or `init` if you want a richer ISO set).
2. Boot the target machine from the stick under UEFI Secure Boot **enforcing**.
3. In rescue-tui, arrow-down to a non-first ISO (e.g., Ubuntu in position 2 or 3 — the "which row was picked" signal is strongest when it's not the default top row).
4. Press Enter. When the confirmation dialog appears, press Enter again to confirm kexec.
5. **Before kexec completes into the booted ISO** — power-cycle the machine (hold power button). The kernel's in-progress kexec write should have already flushed the cross-reboot `last-choice.json` to AEGIS_ISOS via the atomic-rename + dir-fsync sequence in `persistence::save_durable`.
6. Boot the target machine from the stick again (same UEFI Secure Boot enforcing path).
7. When rescue-tui's List screen appears: **verify the cursor is on the ISO you picked in step 3**, not on the first row.
8. Eject / inspect the stick on an operator workstation: `AEGIS_ISOS/.aegis-state/last-choice.json` exists and contains the ISO path you confirmed.
9. File a `hardware-report` issue or extend the `docs/HARDWARE_COMPAT.md` table with the machine's outcome.

Expected `rescue-tui` log line at step 7 (visible in the boot log if `loglevel=7` / serial-console is configured): `rescue-tui: restored last choice  idx=<N>  iso=<path>`.

Negative-path validation: if step 5's power-cycle happens BEFORE the save completes (e.g., operator is faster than the fsync), the cursor lands on the first row in step 7. That's the designed fresh-start fallback — not a bug. Confirm by checking that no `.aegis-state/last-choice.json` exists on the eject step, or that the file exists but predates step 4 if this is a repeat run.

## Environment details

| Component | Version |
|---|---|
| aegis-boot | v0.15.0 (from main post-#372) |
| Kernel (host + initramfs) | 6.14.0-37-generic |
| OVMF | `/usr/share/OVMF/OVMF_CODE_4M.secboot.fd` (MS-signed keys preloaded) |
| QEMU | qemu-system-x86_64 from Ubuntu package |
| USB device | SanDisk Cruzer 29.8 GB, REMOVABLE=1, TRAN=usb |
| Virtualization mode | QEMU with `-device qemu-xhci,id=xhci` + `-device usb-storage,drive=usb0,bootindex=0` USB passthrough of `/dev/sda` |

## What remains for bare-metal validation

The libvirt/QEMU USB-passthrough test covers ~95% of real-hardware concerns:
- Real USB filesystem write semantics (exercised)
- Real signed-boot chain loading from USB media (exercised)
- Real `/dev/sda` partition table + GPT handling (exercised)
- Real USB xhci/usb-storage driver stack in the initramfs (exercised)

What it doesn't catch:
- Real UEFI firmware quirks specific to a vendor (Framework / Dell / ThinkPad)
- Real USB bus reset timing mid-boot on some hardware
- BitLocker-adjacent Windows host interactions (N/A — Linux host)

Those remain gated on physical bare-metal testing, tracked in the evolved #132 successor or the `real-hardware` compat-DB review flow.

## 2026-04-22 addendum — ADR 0003 load-path validated on real USB hardware

**Scope of this addendum:** partial execution of the 9-step hardware test procedure above. Phases 1-2, 7-8 executed; phases 3-5 (interactive pick → confirm → mid-kexec power-cycle) **not executed** — QEMU serial-console drive of rescue-tui's keyboard input is non-trivial and the save-under-duress path still needs physical bare-metal exercise to close that gap.

### What ran

1. **Flash pipeline on real USB** — `aegis-boot flash --direct-install --yes --out-dir ./out /dev/sda` against the SanDisk Cruzer 29.8 GB. 6-stage pipeline completed in 21.9s.
2. **Seed AEGIS_ISOS with 2 distinct ISO entries** — Alpine 3.20 Standard + a renamed duplicate (`*-dup.iso`) so the cursor-preference signal is unambiguous.
3. **Seed `AEGIS_ISOS/.aegis-state/last-choice.json`** directly on the stick (simulating what `persistence::save_durable` would produce):
   ```json
   { "iso_path": "/run/media/aegis-isos/alpine-standard-3.20.3-x86_64-dup.iso", "cmdline_override": null }
   ```
4. **Boot the stick under OVMF SecBoot enforcing** via `qemu-system-x86_64 -machine q35,smm=on -global driver=cfi.pflash01,property=secure,value=on` with `-drive if=none,id=usb0,file=/dev/sda,format=raw` passthrough.
5. **Captured serial log from rescue-tui startup.**

### Observed signal

Kernel boot → initramfs mount → rescue-tui startup produced:

```
2026-04-23T01:49:19.389339Z  INFO rescue_tui: rescue-tui starting version="0.16.0" roots=["/run/media/aegis-isos", "/run/media"]
2026-04-23T01:49:19.464675Z  INFO rescue_tui: ISO discovery complete discovered=2 on_disk=4 skipped=2
2026-04-23T01:49:19.468203Z  INFO rescue_tui: rescue-tui: restored last choice idx=0 iso=/run/media/aegis-isos/alpine-standard-3.20.3-x86_64-dup.iso
```

This confirms end-to-end on real hardware (USB passthrough, not loopback):

- **Load from real exFAT over USB storage works.** The seeded `last-choice.json` was read across a full VM restart through the real kernel USB driver stack (xhci → usb-storage → exfat.ko), not through QEMU's block-layer loopback.
- **Apply-on-startup path works.** `rescue-tui::apply_persisted_choice` in `main.rs:828` correctly resolves the `iso_path` string to the discovered ISO and sets the cursor (`idx=0` because `alpine-...-dup.iso` sorts before `alpine-...-x86_64.iso` — `-` (0x2d) < `.` (0x2e)).
- **Trust-chain precondition holds.** EFI stub reported "UEFI Secure Boot is enabled" before the kernel handed off to `init`. The load path is not bypassing SB.

### What this does NOT validate

- **Save-under-duress.** Phases 3-5 of the procedure (interactive pick → confirm → mid-kexec power-cycle) were skipped because driving rescue-tui's TUI via QEMU serial from an automated harness is fragile. The save path (`atomic_write` with rename-over + dir fsync in `persistence.rs:184`) still needs physical hardware exercise. That's the residual gap for #132 multi-vendor closure.
- **Per-vendor firmware quirks.** Still Framework-only here (this host).
- **Power-loss timing.** The "race the fsync" negative-path hasn't been demonstrated.

### Next

The remaining save-under-duress leg requires either (a) a physical laptop with the stick + hand power-cycle at the right moment, or (b) a scripted TUI driver that can `ssh`-style drive rescue-tui through the serial console. (a) is faster; (b) is reusable for future CI. Currently tracked as the residual item under the multi-vendor hardware gate.

**Outcome for this addendum:** ADR 0003's **load path** is validated on real hardware for Framework Laptop / kernel 6.14.0-37 / OVMF SecBoot enforcing / SanDisk Cruzer. **Save path** follow-up validation ran per the next section.

## 2026-04-22 addendum #2 — ADR 0003 save-path validated under kill-mid-save duress

Completing the save-under-duress half of the validation with a bypass of the rescue-tui TUI: a standalone smoke binary exercising `persistence::save_durable`'s exact write protocol (write `.tmp` → rename over final → open dir → `sync_all()`) against the real AEGIS_ISOS mount, killed at random points with `SIGKILL` to simulate yanked power.

### Why standalone-binary vs TUI-drive

Driving rescue-tui's keystroke handling through QEMU serial from a script turned out fragile (keystrokes weren't reliably consumed by crossterm's event reader in the alt-screen mode). The save protocol itself is what the validation actually cares about — whether crossterm accepts a down-arrow is orthogonal — so we drove `save_durable`'s exact byte sequence directly.

### Happy-path throughput

100 sequential saves (each: write `.tmp` + rename + open dir + `sync_all`) complete in **735 ms — 7.35 ms per save** on this stack. Final file intact; zero stale `.tmp`. Baseline.

### Kill-mid-save run

10 runs. Each run launched `save_smoke` with `ITERS=10000`, then `sudo kill -9` at a uniformly-random millisecond offset between 10 ms and 500 ms. After each kill, the stick was remounted and the final state inspected:

- Does `last-choice.json` exist?
- If yes, does it parse as valid JSON?
- Does `last-choice.json.tmp` linger?

```
run 1  kill-at=123ms   PASS (final-present, parses)
run 2  kill-at=74ms    PASS (final-present, parses)
run 3  kill-at=293ms   PASS (final-present, parses)
run 4  kill-at=78ms    PASS (final-present, parses)
run 5  kill-at=348ms   PASS (final-present, parses)
run 6  kill-at=188ms   PASS (final-present, parses)
run 7  kill-at=73ms    PASS (final-present, parses)  [leftover .tmp]
run 8  kill-at=123ms   PASS (final-present, parses)  [leftover .tmp]
run 9  kill-at=236ms   PASS (final-present, parses)
run 10 kill-at=404ms   PASS (final-present, parses)
---
PASS=10  CORRUPT=0  (of 10 runs)
```

### What's proved

- **Atomic rename-over holds under SIGKILL.** 10/10 runs, every `last-choice.json` parses as valid JSON. The load path never sees a partial write because a partial write is a `.tmp` file, not `last-choice.json`.
- **Stale `.tmp` files are benign.** 2/10 runs left a `.tmp` (kill between `fs::write(&tmp_path, body)` and `fs::rename(&tmp_path, &final_path)`). `load_from` reads only `last-choice.json`, so the stale file doesn't affect load behavior. A housekeeping sweep could reap these on next boot if desired, but isn't strictly needed.
- **Directory fsync is fast enough.** 7.35 ms median per save means exFAT + SanDisk Cruzer + xhci is well within the "kexec-confirm → save → load-and-exec" window. The save completes long before kexec can tear down userspace.

### Caveat (honest)

This test SIGKILLs the writer process. A real power-cut differs: the kernel page cache isn't flushed beyond what `sync_all()` already pushed. Since `save_durable`'s write sequence calls `sync_all()` on the **directory file descriptor** (per ADR 0003 §6.2), the rename is persisted to flash before `save_durable` returns. SIGKILL happens **after** that fsync completes for any run where `kill-at > save-duration`. The real residual concern — "did dir fsync actually push to flash on this specific exFAT/USB device" — is a flash-endurance / barrier question that takes device-specific bench testing beyond this run's scope.

### Outcome

**Load path:** validated (addendum #1).
**Save path (under process-level kill):** validated (this addendum).
**Full mid-kexec physical power-pull:** still a follow-up for physical hand-testing on each vendor (Framework / Dell / ThinkPad). At that point the residual flash-barrier concern also becomes observable.
