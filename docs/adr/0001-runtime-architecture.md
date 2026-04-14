# ADR 0001: Runtime Architecture — Signed Linux Rescue + ratatui + kexec

**Status:** Accepted
**Date:** 2026-04-14
**Deciders:** 5-agent consensus vote (higher_order, supermajority) on [issue #4](https://github.com/williamzujkowski/aegis-boot/issues/4)
**Result:** Option B, 4–1 (80%)

## Context

Aegis-boot's product goal: drop any bootable ISO into a drive, boot the machine, show a TUI listing discovered ISOs, select one, boot it — under UEFI Secure Boot guarantees.

At the point of this decision the repo has `iso-parser` (ingestion) and a Secure Boot threat model, but the runtime (TUI + boot-time integration) is unspecified. Three architectures were evaluated:

- **A — Custom UEFI app** (Rust + `uefi-rs`, TUI pre-OS, chainload via `LoadImage`/`StartImage`)
- **B — Signed Linux rescue + ratatui + kexec** (small signed kernel + initramfs, kexec into selected ISO's kernel)
- **C — GRUB menu generator** (`iso-parser` emits `grub.cfg`; shim→GRUB→loopback ISO)

## Decision

**Adopt Option B.**

## Rationale

Four of five voters (security, systems/firmware, product, architecture, devil's-advocate roles) converged on B; security specialist dissented for A.

Primary drivers:

1. **Operational Secure Boot burden is outsourced.** shim + signed distro kernel inherits Fedora/Debian/SUSE's revocation response, SBAT bumps, and Microsoft CA rotation (2026 "Windows UEFI CA 2023" cutover). Option A would make us our own shim, responsible for every dbx update.
2. **"Real TUI" is only met by ratatui.** GOP framebuffer text (A) or `EFI_SIMPLE_TEXT_OUTPUT` is a `println` loop; GRUB's menu (C) fails the product goal on day zero.
3. **Driver ecosystem is non-trivial.** xHCI USB HID, ISO9660 + El Torito + UDF hybrid parsing (Rock Ridge, Joliet), NVMe, GOP quirks across AMI/Insyde/Phoenix firmware. The kernel has already solved this; reimplementing in `uefi-rs` is a perpetual cost for a small team.
4. **Testability.** `cargo test` + `ratatui::TestBackend` + existing `iso-parser` (std-compatible) drops in unchanged. Option A requires a `no_std` rewrite of the IO layer.
5. **Graceful degradation.** If `kexec` fails on exotic hardware, the user still has a working rescue shell with networking, `dmesg`, `lsblk`. A and C leave a black screen.

## Consequences

### Crate layout

- `crates/iso-parser/` — existing, std-compatible ISO parser (kept)
- `crates/iso-probe/` — loopback + GPT/ISO9660 discovery on the live rescue environment; wraps `iso-parser` for the runtime caller
- `crates/rescue-tui/` — ratatui application; the TUI the user sees
- `crates/kexec-loader/` — thin wrapper over `kexec_file_load(2)`; must refuse `kexec_load` (lockdown blocks it anyway)

### Boot chain

```
UEFI firmware
  → shim (vendor-signed, Microsoft UEFI CA)
    → signed Linux rescue kernel (Canonical / Debian / Fedora CA)
      → initramfs (contains rescue-tui + iso-probe + kexec-loader)
        → rescue-tui (user selects ISO)
          → kexec-loader → kexec_file_load → selected ISO's kernel + initrd
```

### Required kernel config (rescue kernel)

- `CONFIG_KEXEC_FILE=y`
- `CONFIG_KEXEC_SIG=y`
- `CONFIG_KEXEC_BZIMAGE_VERIFY_SIG=y`
- `CONFIG_LOCKDOWN_LSM=y` with `lockdown=integrity`

Explicitly **not** used: `kexec_load(2)` (classic syscall — blocked under lockdown, weaker signature story).

### Risks we accept and must mitigate

| Risk | Mitigation |
|---|---|
| `kexec_file_load` + `KEXEC_SIG` lockdown-bypass CVEs (e.g. CVE-2022-21505 class) | Pin minimum rescue kernel version; track kernel security advisories; document update cadence. |
| Unsigned / self-built ISO kernels fail signature check | Honest UX: present a clear "this ISO's kernel is not signed by a trusted CA; enroll its key via `mokutil` to boot it" message. Never suggest disabling SB. |
| Cross-distro kexec refusal (RHEL-family may reject non-RHEL-CA kernels even with `KEXEC_SIG` satisfied) | Maintain a per-distro compatibility matrix (tracking issue forthcoming). |
| Hybrid ISOs assuming `dd`-to-block-device or BIOS isolinux-only | Per-distro quirks table in `iso-probe`; fail gracefully with a specific diagnostic, not a black screen. |
| Image size 80–150 MB vs ~2 MB UEFI app | Acceptable tradeoff; documented in README. |
| Initramfs supply chain | Reproducible build pipeline (issue #2) must cover initramfs content, not just binaries. |

### Preserved dissenting view (Option A)

The security voter argued that chainloading via `LoadImage`/`StartImage` sidesteps SBAT/MOK/kexec entirely, minimizes TCB to a single signed PE, and eliminates the GRUB/shim/kernel CVE lineage (BootHole family: CVE-2020-10713, CVE-2024-45774..45783; shim CVE-2023-40547; kernel lockdown CVE-2022-21505).

This dissent shapes Option B's implementation:

- `kexec-loader` is deliberately the smallest crate — its CVE blast radius is our largest residual TCB concern.
- Future revisit trigger: if upstream kexec-under-SB accumulates more than one critical CVE per year for two consecutive years, revisit Option A.

## Alternatives considered

### Option A — Custom UEFI app (rejected)

- Rejected primarily because (a) the stated "TUI" requirement degrades to framebuffer text, (b) driver re-implementation cost (USB HID, ISO9660/UDF) dominates project bandwidth for a 2-person team, (c) we become our own shim with full signing/revocation responsibility.
- Preserved as the long-term revisit option if kexec-under-SB becomes untenable.

### Option C — GRUB menu generator (unanimously rejected)

- GRUB's stock menu is explicitly **not** a TUI — fails product goal on day zero.
- GRUB under Secure Boot is the single largest CVE surface in the ecosystem (BootHole lineage).
- `grub.cfg` loopback coverage is distro-specific and silently broken for many ISOs (Windows, VMware ESXi, memdisk-dependent live images).

## References

- Issue [#4](https://github.com/williamzujkowski/aegis-boot/issues/4) — architecture decision
- Issue [#2](https://github.com/williamzujkowski/aegis-boot/issues/2) — reproducible build (must cover rescue kernel + initramfs + signing)
- `THREAT_MODEL.md` — pending update to reflect the chosen chain
