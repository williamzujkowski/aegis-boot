# Windows ISO boot — design research

**Status:** Research / Proposal
**Author:** Maintainer + Claude Opus 4.7
**Related issue:** (to be filed — `feat(epic): boot Windows 11 ISOs via aegis-boot`)

## Problem

aegis-boot's rescue-tui today refuses to boot Windows 11 installer ISOs. This is deliberate — iso-probe tags `Distribution::Windows` with `Quirk::NotKexecBootable`, and rescue-tui renders those as tier-5 "BOOT BLOCKED".

The reason is architectural: rescue-tui's boot-handoff path is **Linux kexec** — it extracts a Linux kernel + initrd from the ISO and invokes the `kexec_file_load(2)` syscall to replace the running kernel. Windows' boot loader is a PE32 UEFI executable (`bootmgfw.efi`), not a Linux kernel; mainline `kexec_file_load` refuses to load it.

Operators who need to boot a Win11 installer from an aegis-boot stick today must:

1. Have a second USB stick flashed with the Windows ISO separately (e.g. via Rufus), or
2. Re-flash the aegis-boot stick with Rufus (destroying the aegis-boot chain), or
3. Use an external DVD or network-boot rig.

This defeats the operator-flow goal of "one signed stick, any ISO on it, boot what's needed."

## Scope

This document surveys the technical approaches for enabling Windows 11 installer boot from rescue-tui, scores them against aegis-boot's existing design constraints, and recommends an implementation direction.

**Non-goals:**

- Booting an *installed* Windows OS from the stick (that's dual-boot, not rescue-tui's remit).
- Booting arbitrary UEFI applications other than Windows installers (general `bootmgr` chainload is out of scope).
- Replacing the Linux rescue environment (Linux distros still kexec; only Windows takes the new path).

## Existing scaffolding

| Component | What it already does | What's missing |
| --------- | -------------------- | -------------- |
| `iso-parser::try_windows_layout` | Detects Windows ISOs via `bootmgr` / `sources/boot.wim` / `efi/boot/bootx64.efi` markers, emits a `BootEntry` labelled "Windows (not kexec-bootable)" | Nothing — the detection is solid. |
| `iso-probe::Quirk::NotKexecBootable` | Quirk assigned to `Distribution::Windows` | New quirk variant `RequiresUefiChainload`, or refactor to a `BootAction` enum. |
| `rescue-tui` tier-5 renderer | Shows "BOOT BLOCKED — Boot is disabled" for `NotKexecBootable` | New action path for Windows: extract boot tree, stage, reboot-via-`BootNext`. |
| `kexec-loader` crate | Loads Linux `bzImage` via `SYS_kexec_file_load` | Cannot load PE32 images without custom kernel patches. |

## Candidate approaches

### Option A — `safeboot-loader` (Trammell Hudson / osresearch)

[trmm.net/chainload](https://trmm.net/chainload/) / [github.com/osresearch/safeboot-loader](https://github.com/osresearch/safeboot-loader).

**How it works:**

1. A custom Linux kernel patchset extends `kexec_load()` to accept PE32 executables.
2. On kexec invocation, a trampoline preserves the UEFI state (CR3, GDT, LDT, IDT) captured during the original aegis-boot boot.
3. Linux hooks `ExitBootServices()` to prevent its first call so the trampoline can re-enter UEFI.
4. The trampoline invokes `gBS->LoadImage()` + `gBS->StartImage()` on `bootmgfw.efi`.
5. Windows bootmgr picks up where Linux left off — same UEFI firmware state, same Secure Boot variables.

**Pros:**

- Matches aegis-boot's existing kexec handoff model most closely — rescue-tui does "load kernel, kexec" and that's it.
- UEFI state preservation means Windows sees the real firmware, not a synthesized environment. Secure Boot chain is intact.
- Proven by Hudson's production work on remote-attested BitLocker unlock.

**Cons (severe):**

- **Custom kernel patchset.** We ship a pinned Debian kernel today; maintaining a forked kernel is a continuous tax. Every Ubuntu/Debian kernel bump requires rebasing + re-testing the patchset.
- `memmap=exactmap` to isolate Linux to a 1 GiB region — changes aegis-boot's rescue-env memory model.
- UEFI hook code is x86-64 and UEFI-spec specific; ARM support would need a second port.
- Not in mainline. No upstream momentum. Hudson's fork is the only maintainer.

### Option B — `wimboot` (iPXE) via kexec

[ipxe.org/wimboot](https://ipxe.org/wimboot).

**How it works:**

1. `wimboot` is a hybrid UEFI/BIOS bootloader that takes a `.wim` file + BCD hive + `bootmgr` and constructs a RAM-resident environment for Windows PE.
2. Designed for iPXE HTTP delivery: iPXE `chain`s into wimboot, wimboot pulls `boot.wim` over HTTP, hands control to Windows PE.
3. Works with Secure Boot; signed by Microsoft UEFI CA via shim.

**Pros:**

- Ships as a bootloader, not a kernel patch. Much lighter footprint than Option A.
- Microsoft UEFI CA signature on shim-loaded wimboot — Secure Boot passes.
- Well-understood; iPXE is widely deployed.

**Cons:**

- Still requires a way to chain *into* wimboot from the running Linux kernel. Stock `kexec_file_load` won't take a PE32 binary — same problem as Option A, just with a different PE32 target.
- Loads the WIM into RAM. Win11 `boot.wim` is ~700 MB; `install.wim` is 4-5 GB. 16+ GiB RAM minimum.
- Primarily USB-over-iPXE story, not a USB-only story. Patching it to read from a mounted FAT32 is doable but non-trivial.

### Option C — Ventoy-style virtual disk

[ventoy.net](https://www.ventoy.net/).

**How it works:**

1. Ventoy ships its own UEFI bootloader (`ventoy.efi`) that loads at UEFI boot time — replaces aegis-boot's boot chain entirely.
2. Ventoy presents a TUI to pick an ISO.
3. On selection, a Ventoy kernel module (`vtoy`) intercepts filesystem I/O and presents the ISO as if it were a whole virtual disk to the guest OS.
4. For Windows specifically, Ventoy patches boot.wim at runtime to inject the vtoy driver so Windows sees the ISO as a CD-ROM.

**Pros:**

- Proven: Ventoy supports Win11 installer booting today.
- Single-stick, no repartitioning.

**Cons:**

- **Replaces aegis-boot's signed chain.** Ventoy owns the boot flow; aegis-boot's rescue-tui + Secure Boot chain-of-trust story is lost.
- **Partial closed source.** Ventoy's license is "GPLv3+ (partial)" — some components are redistributed binary blobs without source. This has been [controversial in security-sensitive deployments](https://forums.ventoy.net/) (exact threat-model implications left as an exercise for auditors, but it fails aegis-boot's "every byte auditable" bar).
- The `vtoy` kernel module is a Windows-side injection. Forks of Windows kernel surface → harder to audit.

Not a serious candidate for aegis-boot's threat model.

### Option D — UEFI `BootNext` + staged FAT32 partition (recommended)

**How it works:**

1. Aegis-boot stick is partitioned at flash time with a **third** partition: `WIN_STAGE` (FAT32, ~1.5 GiB, unused at flash time). Layout becomes:

   | # | Label | FS | Size | Purpose |
   |---|-------|----|------|---------|
   | 1 | `AEGIS_ESP` | FAT32 | 400 MiB | aegis-boot signed chain |
   | 2 | `AEGIS_ISOS` | exFAT | bulk | ISO storage |
   | 3 | `WIN_STAGE` | FAT32 | 1.5 GiB | pre-allocated Windows-install staging |

2. Operator picks a Win11 ISO in rescue-tui.
3. New `windows-bootprep` crate:
   a. Loop-mounts the ISO read-only from `AEGIS_ISOS`.
   b. Formats `WIN_STAGE` fresh.
   c. Copies the ISO contents (`efi/`, `sources/`, `bootmgr*`, etc. — ~700 MiB for Win11 minimal) to `WIN_STAGE`.
4. rescue-tui invokes `efibootmgr --disk /dev/sdX --part 3 --create --label "Windows 11 Setup (one-shot)" --loader '\EFI\BOOT\BOOTX64.EFI'`.
5. rescue-tui sets `BootNext` to the new entry (one-shot) via `efibootmgr --bootnext <id>`.
6. rescue-tui calls `reboot`.
7. UEFI firmware reads `BootNext`, launches `\EFI\BOOT\BOOTX64.EFI` from `WIN_STAGE`. Windows setup runs.
8. `BootNext` is one-shot; subsequent boots fall back to the default `AEGIS_ESP` entry. Aegis-boot is untouched.

**Pros:**

- **No kernel patches.** Uses stock `kexec_file_load` for Linux distros (existing path) and stock `efibootmgr` + `reboot` for Windows (new path). Both are mainline upstream.
- **Aegis-boot chain intact.** `AEGIS_ESP` untouched; signed boot chain still verified on next boot.
- **Secure Boot clean.** Windows' own signed `bootmgfw.efi` runs under the same UEFI keyring that already trusts Microsoft UEFI CA.
- **One-shot.** `BootNext` fires exactly once; operator can't accidentally re-enter Windows setup on the next reboot.
- **Auditable.** Every step is standard Linux userspace + standard UEFI API calls. No custom bootloaders, no vendor-specific patches, no closed-source blobs.

**Cons:**

- **Stick-layout change.** Every flash after this ships with a 1.5 GiB `WIN_STAGE` partition that sits empty for operators who never install Windows. Budget-wise: ~5% of a 32 GiB stick. Acceptable.
- **Free space on existing sticks is stranded.** Sticks flashed before this change don't have `WIN_STAGE`. We'd either:
  - Tell those operators to reflash (the "correct" answer — flashes are cheap).
  - Add a `aegis-boot grow-layout` subcommand that non-destructively shrinks `AEGIS_ISOS` and adds `WIN_STAGE` (doable via `sgdisk` + `mkfs.vfat`, but `AEGIS_ISOS` must be empty or small enough to move — and exFAT resize is limited).
- **`efibootmgr` dependency.** Adds `efibootmgr` to the rescue-env initramfs (~50 KiB). Already a commonly-shipped userspace tool.
- **UEFI firmware support.** `BootNext` is in the UEFI 2.0 spec (2006+). Every x86-64 UEFI firmware we care about supports it.
- **Install-to-stick vs install-to-disk boundary is subtle.** The Win11 installer, once running, will ask where to install. It's capable of installing *onto* the USB stick itself and destroying aegis-boot if the operator picks the wrong disk. UX mitigation: rescue-tui shows a confirm screen naming "your aegis-boot stick is on disk N; do not select it as the install target."
- **Not minimum-RAM friendly.** Win11 installer itself wants 4 GiB; aegis-boot's rescue env wants ~1 GiB. Target 6 GiB total.

## Recommendation (updated 2026-04-24 after maintainer review + consensus vote)

### Original recommendation — Option D — was REJECTED on size grounds

Maintainer push-back on the initial Option D recommendation:

> "1.5 GiB is 3x the previous size just for Windows support. I'd rather help folks move off of Windows 11 to Linux."

The stick-layout change (+1.5 GiB) is disproportionate for a feature that conflicts with the project's philosophical mission. The signed-chain preservation argument isn't enough by itself.

### Alternatives surveyed + consensus vote (2026-04-24)

Two lighter options, plus three marginal variants:

- **L1 — "Helpful refusal + Linux redirect":** rescue-tui tier-5 panel for Win11 ISOs gets actionable prose naming three alternatives (try the Linux ISOs already on the stick, use L2's CLI for a standalone Windows stick, fall back to Rufus). ~30 LOC. Zero stick growth. Zero signed-chain impact.
- **L2 — "Second-stick CLI":** `aegis-boot flash --windows-target <iso> <stick>` wipes a different stick and makes it a standard Rufus-style Win11 installer, reusing the `windows_direct_install::{partition,format,raw_write}` pipeline from [#419](https://github.com/aegis-boot/aegis-boot/issues/419). Zero aegis-boot stick growth; operator needs 2 sticks. ~150 LOC (mostly reuse).
- **L3 — "Grow AEGIS_ESP to 1 GiB, chainload in-place":** +600 MiB growth (vs +1.5 GiB for Option D). Mixes Windows + aegis-boot bits on the same ESP — attestation complexity grows. ~400 LOC.
- **L4 — "Detect-only, no boot path":** Leave the BOOT BLOCKED status as-is. <5 LOC.
- **L5 — "PE32 kexec via Hudson's `safeboot-loader`":** Maintain a forked kernel with PE32 kexec patches. No stick-layout change. Ongoing ~1 dev-week per kernel-version tax.

### Vote result

Ran 6-agent `consensus_vote` with `strategy: higher_order` on the alternatives. Result: **83 % approval of the overall proposal; L1 + L2 as the unanimous top-2 combination.**

| Role | #1 pick | Reasoning summary |
| --- | --- | --- |
| Software Architect | L1 | Only option honoring every constraint; L2 as the escape hatch L1 points at. |
| Security Engineer | L1 | Zero new attack surface; L3 co-mingling rejected as bootkit-persistence risk; L5 rejected as supply-chain audit liability. |
| Developer Experience | L2 | Operators need a real success path; pair with L1 for the UI. |
| AI/ML Engineer | L1 | 30 LOC minimal audit surface, mission-aligned. |
| Product Manager | L1 | Tier-5 dead-end is a support-ticket generator; actionable prose is proper UX. |
| Contrarian (rejected the proposal) | L2 | Flags legit concern: operators *do* need Windows PE for non-`fwupd` OEM firmware updates + BitLocker recovery in rescue scenarios. |

### Revised recommendation: **L1 + L2, shipped together**

- **L1 ships first** as a single-PR rescue-tui prose change — snapshot-testable, zero deps, trivially revertible.
- **L2 ships behind L1** as a standalone `flash --windows-target` subcommand reusing the #419 pipeline. The CLI becomes the "[2] use a second stick" alternative L1's prose panel points at.

Combined cost: ~180 LOC. Zero aegis-boot stick size change. Zero signed-chain perturbation. Aligned with the maintainer's "help operators move off Windows" mission: the default behavior nudges toward Linux, the escape hatch exists for operators who genuinely need it.

### Contrarian's concern — recorded, not adopted

The contrarian role flagged that rescue-tool users legitimately need Windows PE for:

1. **Vendor firmware updates outside `fwupd`'s coverage** (some OEM BIOS updaters are Windows-only).
2. **BitLocker-protected volume recovery** when the Linux rescue path can't unseal the TPM-sealed key.

The L1 + L2 combination addresses (1) and (2) by letting operators flash a second stick as needed. The maintainer's position — that aegis-boot should not be the direct Windows boot path — is preserved.

### Superseded: "Cost is a one-time partition-layout change..."

The Option D implementation sketch below is retained for historical context. The L1 + L2 implementation sketch is in the epic tracking issue ([see References](#references)).

## Implementation sketch for L1 + L2 (recommended)

### L1 — rescue-tui prose panel (~30 LOC, 1 PR)

- [ ] iso-probe quirks: rename `Quirk::NotKexecBootable` → `Quirk::RequiresAlternatePath` (or add the new variant) so rescue-tui can distinguish "this ISO type just can't be booted" from "boot this some other way". Windows keeps the quirk but gets a different rendering.
- [ ] rescue-tui tier-5 render: for `Distribution::Windows`, replace "BOOT BLOCKED" prose with an actionable 3-bullet panel:
  1. "aegis-boot won't boot Windows ISOs — that's by design. Here are your options:"
  2. "Try Linux: [list of Linux ISOs found on this stick, or a catalog-slug hint if none]"
  3. "Make a dedicated Windows installer stick: `aegis-boot flash --windows-target {iso} {new-stick}`"
  4. "Or: use Rufus (`aegis-boot` is not a Rufus replacement)"
- [ ] Snapshot test in `rescue-tui` asserting the panel renders for a canned Win11 iso-probe fixture.
- [ ] Mention in `docs/HOW_IT_WORKS.md § Supported ISOs`.

### L2 — `aegis-boot flash --windows-target` (~150 LOC, 1-2 PRs)

- [ ] New CLI arg `--windows-target <stick>` on `aegis-boot flash`, mutually exclusive with `--direct-install` (aegis-boot flow) and `--image` (pre-built image flow).
- [ ] New module `crates/aegis-cli/src/windows_target.rs` that:
  - Accepts a Windows ISO path + a target-stick device path.
  - Refuses disk 0 and the `aegis-boot`-formatted stick (detects `AEGIS_ESP` label on the target).
  - Calls `windows_direct_install::partition::partition_via_diskpart` (Windows host) OR `sgdisk` (Linux host — we need a Linux-host equivalent since #419's diskpart is Windows-only).
  - Formats partition 1 as FAT32 (big — 4 GiB+ to fit `install.wim`).
  - Copies the Windows ISO contents (loop-mount + `cp -r`) to the FAT32 partition.
  - Writes an attestation manifest noting the stick is a "Windows installer (not aegis-boot)".
- [ ] Linux-host Windows-target path needs `sgdisk` + `mkfs.vfat` + `rsync`/`cp`. The `windows_direct_install::partition::partition_via_diskpart` logic ports cleanly — same layout, different tools.
- [ ] Unit tests on the pure-fn argument parsing + same-stick-detection refusal.
- [ ] Integration test: loop-mount a Win11 installer fixture + a scratch image file as the "target stick"; assert FAT32 layout matches what Rufus produces byte-for-byte on the boot-relevant files.
- [ ] Documentation: new section in `docs/CLI.md § aegis-boot flash` covering `--windows-target`, pointing back at the rescue-tui panel for discoverability.

### Out of scope for L1 + L2

- Booting Windows ISOs from the aegis-boot stick itself (that was Option D — rejected).
- Windows-on-ARM target (x86_64 only).
- Windows-installer-flashing from a non-Linux host (Windows operator can use Rufus; macOS operator can use `dd` + the ISO contents, covered in docs).

## Historical: Option D implementation sketch (not pursued)

The 5-phase Option D sketch below is retained for context. If a future reviewer revisits the decision — for example, because L1 + L2 doesn't cover a rescue scenario we've failed to anticipate — this section is the starting point.

### Phase 1 — Stick layout + flash

- [ ] `scripts/mkusb.sh` partition table gains a 1.5 GiB `WIN_STAGE` FAT32 partition, label `WIN_STAGE`.
- [ ] `windows_direct_install::partition::build_diskpart_script` gains the same third partition for Windows-flashed sticks.
- [ ] `crates/aegis-wire-formats`: attestation manifest schema gains an optional `win_stage_partition` slot (UUID + size) so the audit log records whether the stick is Windows-capable.
- [ ] Rollout: minor version bump; existing sticks continue to work (rescue-tui detects absence of `WIN_STAGE` and hides the Windows option).

### Phase 2 — `windows-bootprep` crate

- [ ] New crate `crates/windows-bootprep/` with `prepare_windows_stage(iso_path, stage_part_device) -> Result<()>` as the public surface.
- [ ] Pure-fn inputs + unit tests: ISO path, staging device, quirks-aware file whitelist (don't copy anything beyond the boot tree to keep stage small).
- [ ] `#[cfg(target_os = "linux")]` side: subprocess wrappers for `mkfs.vfat`, `mount -o loop,ro`, `cp -r`, `umount`, `sync`.

### Phase 3 — `efibootmgr` wiring

- [ ] New helper module in `crates/kexec-loader/` or a sibling `crates/uefi-boot/` — `set_bootnext(device, partition, loader_path) -> Result<()>`.
- [ ] Subprocess-invokes `efibootmgr --create --bootnext ...`.
- [ ] Unit tests with an injected `efibootmgr` stub.
- [ ] Add `efibootmgr` to the rescue-env initramfs dependency list.

### Phase 4 — rescue-tui action path

- [ ] New `Quirk::RequiresUefiChainload` (or refactor `Quirk` → `BootAction` enum).
- [ ] `iso-probe::lookup_quirks(Distribution::Windows)` returns `RequiresUefiChainload` instead of `NotKexecBootable`.
- [ ] rescue-tui tier-5 becomes tier-7 "Windows installer (UEFI chainload)" — renders as a distinct verdict with its own icon.
- [ ] Selection handler: confirm screen ("This will reboot into Windows setup. aegis-boot returns on the next reboot. The install target must NOT be `/dev/sdX` (your aegis-boot stick)."), then invoke `windows-bootprep` + `set_bootnext` + `reboot`.

### Phase 5 — integration tests

- [ ] QEMU E2E: OVMF + a pre-made Win11 installer ISO + synthetic aegis-boot stick; assert that `reboot` from rescue-tui triggers Windows setup's first screen (match the "Windows Setup" WIM loading banner).
- [ ] Real-hardware smoke on the maintainer's Win11 VM — flash stick via `aegis-boot flash --direct-install`, add Win11 ISO, reboot into VM BIOS, select stick, confirm aegis-boot menu → pick Win11 ISO → reboot → confirm Win11 setup renders.
- [ ] CI gate: `windows-install-e2e.yml` runs the QEMU pass on PRs touching `windows-bootprep` or `rescue-tui`'s action dispatch.

## Out of scope (explicit non-goals)

- Booting *installed* Windows (dual-boot Win10/Win11 off the stick's target-disk). That's a different UX — aegis-boot is a rescue/install environment, not a boot manager.
- Booting Windows Server or Hyper-V Server installers — same mechanism works, but validation needs separate test media.
- Windows on ARM. Possible but unvalidated.
- Generic UEFI application chainload (arbitrary `.efi` files). Scoped to Windows installers with an iso-parser-recognized layout.

## Open questions

1. **Can `WIN_STAGE` share the ESP?** The existing `AEGIS_ESP` is 400 MiB FAT32. Could we just drop the Windows boot tree into `AEGIS_ESP/EFI/Microsoft/Boot/` next to aegis-boot's `/EFI/BOOT/` tree? Tradeoffs: simpler layout, but expands the ESP past the 400 MiB limit, and mixing aegis-boot + Windows boot trees complicates the attestation manifest. Recommended: separate partition.
2. **Resize existing `AEGIS_ISOS` in-place?** If an operator has a stick already in the field, can `aegis-boot upgrade-layout` shrink `AEGIS_ISOS` + add `WIN_STAGE`? exFAT resize is limited; only viable if `AEGIS_ISOS` is empty. Probably cheaper operationally to tell operators to reflash.
3. **Should `WIN_STAGE` persist between operator invocations?** After a Win11 install completes, rescue-tui's next-boot could optionally clear `WIN_STAGE` (sync it back to zero) so the stick doesn't retain Windows installer bytes unnecessarily. Useful for shared-stick scenarios.
4. **Multi-installer support.** If the operator has multiple Windows ISOs on `AEGIS_ISOS`, the second pick overwrites `WIN_STAGE`. Fine for the 99% use case but surfaces an ordering constraint.

## References

- iPXE wimboot reference: https://ipxe.org/wimboot
- Trammell Hudson's safeboot-loader: https://github.com/osresearch/safeboot-loader
- Hudson's chainload writeup: https://trmm.net/chainload/
- OSFC 2022 talk "Linux as a UEFI bootloader and kexec'ing Windows": https://www.osfc.io/2022/talks/linux-as-a-uefi-bootloader-and-kexecing-windows/
- Ventoy (for background, not recommended): https://www.ventoy.net/
- UEFI `BootNext` spec (UEFI 2.0 §3.1.3): https://uefi.org/specifications
- Microsoft "Install Windows from a USB Flash Drive": https://learn.microsoft.com/en-us/windows-hardware/manufacture/desktop/install-windows-from-a-usb-flash-drive
