# Architecture overview

One-page mental model for contributors. For decision rationale see [ADR 0001](./adr/0001-runtime-architecture.md). For threat boundaries see [THREAT_MODEL.md](../THREAT_MODEL.md).

## Boot chain

```
┌─────────────────────────────────────────────────────────────────┐
│ UEFI firmware                                                   │
│   verifies /EFI/BOOT/BOOTX64.EFI (shim) against db/dbx          │
└─────────────────────────────────────────────────────────────────┘
                                │
                                ▼
┌─────────────────────────────────────────────────────────────────┐
│ shim (Microsoft-signed)                                         │
│   verifies grubx64.efi via vendor cert                          │
└─────────────────────────────────────────────────────────────────┘
                                │
                                ▼
┌─────────────────────────────────────────────────────────────────┐
│ grub (Canonical-signed)                                         │
│   verifies /vmlinuz via shim's keyring                          │
└─────────────────────────────────────────────────────────────────┘
                                │
                                ▼
┌─────────────────────────────────────────────────────────────────┐
│ Linux rescue kernel (Canonical-signed)                          │
│   loads concatenated initramfs (distro + ours)                  │
└─────────────────────────────────────────────────────────────────┘
                                │
                                ▼
┌─────────────────────────────────────────────────────────────────┐
│ /init (PID 1, busybox sh)                                       │
│   mounts /proc /sys /dev, modprobes storage modules,            │
│   auto-mounts AEGIS_ISOS under /run/media, exec's rescue-tui    │
└─────────────────────────────────────────────────────────────────┘
                                │
                                ▼
┌─────────────────────────────────────────────────────────────────┐
│ rescue-tui (ratatui)                                            │
│   discovers .iso files via iso-probe                            │
│   shows verification status (sha256 / minisig sidecars)         │
│   warns on installer images                                     │
└─────────────────────────────────────────────────────────────────┘
                                │
                                ▼
┌─────────────────────────────────────────────────────────────────┐
│ kexec_file_load(2)  ← KERNEL ENFORCES KEXEC_SIG SIGNATURE       │
│   selected ISO's kernel takes over                              │
│   ENODATA (errno 61) if unsigned and SB enforcing               │
└─────────────────────────────────────────────────────────────────┘
```

The trust boundary is the dashed line between "what the firmware verified" and "what the operator picked." The kernel enforces signature checks on the selected kernel via `KEXEC_SIG`; aegis-boot does **not** bypass that enforcement. Unsigned kernels surface as `errno 61` and require explicit MOK enrollment by the operator (not a global trust decision).

## Crate dependencies

```
aegis-cli ────┐                              (operator workstation)
              │                              binary: aegis-boot
              ▼
         (shells out to mkusb.sh + dd; reads /sys/block/sd*)


rescue-tui ──┬──► iso-probe ──► iso-parser   (on the stick, in initramfs)
             │                                binary: rescue-tui
             ├──► kexec-loader
             │
             └──► (TPM PCR 12 measurement via tpm2_pcrextend shell-out)
```

| Crate | Lives | Used at | Role |
|---|---|---|---|
| `aegis-cli` | workstation | flash time | Operator CLI: `flash`, `list`, `add` |
| `rescue-tui` | initramfs | boot time | TUI loop, screens, key bindings, kexec dispatch |
| `iso-probe` | initramfs | boot time | Filesystem walk, sidecar verification, installer heuristic |
| `iso-parser` | initramfs | boot time | Parses isolinux/grub/EFI configs out of an ISO to find kernel + initrd + cmdline |
| `kexec-loader` | initramfs | boot time | Safe wrapper over `kexec_file_load(2)` syscall with error classification |
| `aegis-fitness` | dev | CI / pre-release | Repo / build / artifact health audit (9 checks) |

`unsafe` is forbidden workspace-wide except in `kexec-loader`, where the syscall lives behind a tightly scoped function.

## What's on the stick

```
┌─────────────────────────────────────────────────────────┐
│  GPT partition table                                    │
├─────────────────────────────────────────────────────────┤
│  Part 1 — ESP (FAT32, label AEGIS_ESP, ~400 MB)         │  ← signed boot chain
│    /EFI/BOOT/BOOTX64.EFI   (MS-signed shim)             │     immutable in normal use
│    /EFI/BOOT/grubx64.efi   (Canonical-signed grub)      │
│    /EFI/BOOT/grub.cfg                                   │
│    /vmlinuz                (Canonical-signed kernel)    │
│    /initrd.img             (distro initrd + our /init)  │
├─────────────────────────────────────────────────────────┤
│  Part 2 — Data (FAT32 or ext4, label AEGIS_ISOS)        │  ← operator content
│    ubuntu-24.04.2-live-server-amd64.iso                 │     replaceable without
│    ubuntu-24.04.2-live-server-amd64.iso.sha256          │     reflashing
│    fedora-workstation-41-x86_64.iso                     │
│    alpine-3.20.3-x86_64.iso                             │
│    alpine-3.20.3-x86_64.iso.pub  ← MOK key, optional    │
└─────────────────────────────────────────────────────────┘
```

Splitting the two means: the operator can swap ISO sets without touching the signed boot chain, and the boot chain's immutability simplifies the threat model.

## Build pipeline

```
   source ──► cargo build (Rust 1.85, pinned)
            └─► rescue-tui binary

   rescue-tui + busybox + /init script + ldd-resolved libs
            └─► scripts/build-initramfs.sh
                  └─► out/initramfs.cpio.gz (deterministic, SOURCE_DATE_EPOCH)

   shim-signed + grub-efi-amd64-signed + linux-image + our initramfs
            └─► scripts/mkusb.sh
                  └─► out/aegis-boot.img (GPT + ESP + AEGIS_ISOS)

   out/aegis-boot.img
            └─► dd to /dev/sdX  (via aegis-boot flash, or by hand)
```

CI verifies:
- Reproducibility: two `docker build` passes produce byte-identical `docker save` SHAs
- Initramfs determinism: two `build-initramfs.sh` runs produce byte-identical `initramfs.cpio.gz`
- OVMF SecBoot E2E: simulated SB-enforcing boot reaches rescue-tui
- kexec E2E: rescue-tui successfully kexecs into a target kernel

## Where to look in the code

| You're working on | Start here |
|---|---|
| The TUI | [`crates/rescue-tui/src/main.rs`](../crates/rescue-tui/src/main.rs) (event loop), [`state.rs`](../crates/rescue-tui/src/state.rs) (state machine), [`render.rs`](../crates/rescue-tui/src/render.rs) (screens) |
| The CLI | [`crates/aegis-cli/src/main.rs`](../crates/aegis-cli/src/main.rs) (dispatch), [`flash.rs`](../crates/aegis-cli/src/flash.rs), [`inventory.rs`](../crates/aegis-cli/src/inventory.rs) |
| ISO discovery | [`crates/iso-probe/src/lib.rs`](../crates/iso-probe/src/lib.rs) |
| ISO content parsing | [`crates/iso-parser/src/lib.rs`](../crates/iso-parser/src/lib.rs) |
| The kexec syscall | [`crates/kexec-loader/src/lib.rs`](../crates/kexec-loader/src/lib.rs) (the only `unsafe` in the workspace) |
| Stick assembly | [`scripts/mkusb.sh`](../scripts/mkusb.sh) |
| Initramfs | [`scripts/build-initramfs.sh`](../scripts/build-initramfs.sh), `/init` is appended inline |

## Glossary

- **PK / KEK / db / dbx** — UEFI Secure Boot key hierarchy. Platform Key signs Key Exchange Keys, which sign the `db` allowlist and `dbx` blocklist of allowed/revoked binaries.
- **MOK** — Machine Owner Key. Operator-controlled trust anchor enrolled via shim + `mokutil`. Lets you authorize binaries the platform CA didn't sign.
- **SBAT** — Secure Boot Advanced Targeting. Component-level revocation carried inside shim and grub.
- **shim** — Microsoft-signed bootloader shim that bridges the firmware → distro bootloader. Carries the distro CA and the MOK keyring.
- **`kexec_file_load(2)`** — The signature-aware kexec syscall. Required when SB is enforcing; the older `kexec_load(2)` is blocked by kernel lockdown.
- **`KEXEC_SIG`** — Kernel config that requires the loaded kernel to be signed by a key the platform trusts.
- **GREEN / YELLOW / GRAY verdict** — rescue-tui's verification verdict on an ISO: GREEN = sha256 + minisig both verify; YELLOW = one verifies; GRAY = no sidecars present (boot allowed but with friction).

---

For deeper context:
- Why this architecture and not EDK II or dracut? → [ADR 0001](./adr/0001-runtime-architecture.md)
- Threat model (PK / KEK / MOK / SBAT details, attacker capabilities, non-goals) → [THREAT_MODEL.md](../THREAT_MODEL.md)
- The full boot-chain narrative with verification at each step → [USB_LAYOUT.md § Chain of trust recap](./USB_LAYOUT.md#chain-of-trust-recap)
