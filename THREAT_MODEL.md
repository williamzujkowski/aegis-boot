# Aegis-Boot Threat Model

**Version:** 2.0
**Scope:** Option B runtime (signed Linux rescue + ratatui TUI + kexec) per [ADR 0001](./docs/adr/0001-runtime-architecture.md)
**Last reviewed:** 2026-04-16 (re-confirmed for v0.12.0 release; no model changes — the operator CLI sits below the trust boundary and cannot influence kernel verification)

This document replaces the v1.0 threat model, which assumed a systemd-boot + custom-signed-orchestrator chain. That assumption no longer matches the chosen runtime and has been removed rather than patched.

---

## 1. Glossary

- **PK / KEK / db / dbx** — UEFI Secure Boot key hierarchy: Platform Key signs KEK signs the `db` allowlist and `dbx` blocklist.
- **MOK (Machine Owner Key)** — Platform-agnostic user key, enrolled via `shim` and `mokutil`, used to authorize kernels/modules not signed by Microsoft or the distro CA.
- **SBAT (Secure Boot Advanced Targeting)** — Component-level revocation mechanism carried inside `shim` and GRUB; lets vendors burn old vulnerable versions without issuing a fresh `dbx` hash.
- **shim** — First-stage bootloader signed by Microsoft UEFI CA, delegates to a second-stage bootloader or kernel signed by the distro CA.
- **KEXEC_SIG** — Kernel config gating `kexec_file_load(2)` on platform-keyring signature verification of the kexec target.
- **Lockdown** — LSM mode (`integrity` or `confidentiality`) that restricts kernel interfaces when Secure Boot is enforced.
- **Initramfs** — Compressed cpio archive unpacked by the kernel into a tmpfs root at boot.

## 2. System Under Test

Aegis-boot is a rescue environment that boots under UEFI Secure Boot, discovers ISO images on attached media, and boots the user's selection via `kexec_file_load(2)`.

### Components aegis-boot owns

| Component        | Source                                 | Location in runtime |
|------------------|----------------------------------------|---------------------|
| `iso-parser`     | `crates/iso-parser/` (std, loopback)   | initramfs           |
| `iso-probe`      | `crates/iso-probe/`                    | initramfs           |
| `rescue-tui`     | `crates/rescue-tui/` (ratatui binary)  | initramfs, PID 1 or spawned by init |
| `kexec-loader`   | `crates/kexec-loader/`                 | linked into rescue-tui |

### Components aegis-boot depends on (outsourced trust)

| Component        | Owner                                  | Why trusted       |
|------------------|----------------------------------------|-------------------|
| UEFI firmware    | Platform vendor                        | Platform root of trust |
| `shim`           | Microsoft UEFI CA → vendor             | Microsoft-signed; SBAT-versioned |
| Rescue kernel    | Upstream distro (Debian/Fedora/…)      | Signed by distro CA, CA in `shim` keyring |
| Kernel `KEXEC_SIG` | Linux kernel maintainers             | Verifies kexec target signature against platform + MOK keyrings |

## 3. Boot Chain of Trust

```
┌──────────────────┐
│  UEFI firmware   │   validates PE signature of next stage
└────────┬─────────┘
         ▼
┌──────────────────┐
│  shim (vendor)   │   reviewed by shim review board
└────────┬─────────┘   checks SBAT, delegates to signed kernel
         ▼
┌──────────────────────────────┐
│  signed Linux rescue kernel  │   distro-signed; mounts initramfs
└────────────┬─────────────────┘
             ▼
┌──────────────────────────┐
│  initramfs               │
│  (rescue-tui + helpers)  │   NOT separately signed; rides kernel chain
└────────────┬─────────────┘
             ▼
┌──────────────────────┐
│  rescue-tui          │   runs in Linux userspace, post-ExitBootServices
└────────────┬─────────┘
             ▼
┌───────────────────────────────────┐
│  kexec_file_load(selected ISO)    │   kernel verifies target signature
└────────────┬──────────────────────┘   via KEXEC_SIG + platform keyring
             ▼
┌─────────────────────┐
│  target ISO kernel  │   boots per its own distro's Secure Boot policy
└─────────────────────┘
```

### Critical invariants

1. `rescue-tui` **never runs before `ExitBootServices`**. All pre-OS trust is in the hands of firmware + shim + kernel.
2. `kexec-loader` **only ever calls `kexec_file_load(2)`** — never `kexec_load(2)`, which bypasses `KEXEC_SIG`.
3. `rescue-tui` **never proposes disabling Secure Boot**. On signature rejection, the UX presents a `mokutil` enrollment path.

## 4. Assets

| ID  | Asset                                     | Criticality | Owner            |
|-----|-------------------------------------------|-------------|------------------|
| A1  | UEFI platform keyring (db/dbx)            | Critical    | Platform vendor  |
| A2  | `shim` binary (SBAT table, vendor cert)   | Critical    | Vendor/shim review |
| A3  | Signed rescue kernel + initramfs          | Critical    | Distro           |
| A4  | `rescue-tui` binary inside initramfs      | High        | Project          |
| A5  | `kexec-loader` code path (`unsafe` FFI)   | High        | Project          |
| A6  | User-supplied ISO content on attached media | Medium    | User             |
| A7  | MOK keyring                               | High        | User (enrolled once) |
| A8  | EFI System Partition                      | Medium      | Platform         |
| A9  | Kernel command line for the target ISO    | Medium      | User (via TUI)   |

## 5. Trust Tiers

| Tier | Inputs                                                    | Treatment |
|------|-----------------------------------------------------------|-----------|
| T1 — Authoritative | Firmware, shim, signed kernel, kernel keyring verdict | Full trust — sole authority for go/no-go on kexec |
| T2 — Semi-trusted | Project-owned Rust code in the initramfs                | Trusted for UX/flow; cannot override T1 |
| T3 — Untrusted   | User-supplied ISO content, ISO-embedded boot configs    | Data only — never executed pre-verification |
| T4 — Hostile     | Malformed ISOs, maliciously crafted loop-mount payloads  | Must fail closed; no crash, no out-of-bounds access |

## 6. Threat Actors

| ID  | Actor                        | Capability                                 |
|-----|------------------------------|--------------------------------------------|
| TA1 | Unprivileged local user      | Insert USB media, select ISOs              |
| TA2 | Privileged local user (root post-boot of target) | Full OS after kexec succeeds |
| TA3 | Malicious ISO author         | Crafted ISO / kernel image / cmdline       |
| TA4 | Supply-chain attacker        | Compromise aegis-boot release artifacts    |
| TA5 | Physical evil-maid           | Media swap, firmware tamper between boots  |
| TA6 | Nation-state APT             | Firmware implants, signing cert theft, BootHole-class exploits |

## 7. STRIDE — Option B Runtime

| ID  | Category | Threat | Asset | Mitigation |
|-----|----------|--------|-------|------------|
| S1  | Spoofing | Unsigned ISO kernel presented as trusted | A6 → A3 | `KEXEC_SIG` rejects; `kexec-loader` returns `SignatureRejected`; TUI surfaces MOK enrollment path |
| S2  | Spoofing | Malicious `shim` replacement on ESP | A2, A8 | Firmware `db` validation; SBAT on legitimate shim denies downgrade |
| T1  | Tampering | Modify `rescue-tui` binary in initramfs | A4 | Initramfs is measured into TPM (IMA-appraisal, future work); kernel+initramfs ride a distro-signed chain |
| T2  | Tampering | Inject kernel cmdline via TUI to weaken target ISO | A9 | `rescue-tui` constrains cmdline to ISO-declared defaults + explicit user override; audit-logged |
| T3  | Tampering | Loop-mount race — swap ISO contents between probe and kexec | A6 | Open fd held for the entire probe→kexec window; `kexec_file_load` operates on fd, not path (no TOCTOU) |
| R1  | Repudiation | No record of which ISO was kexec'd | A4 | `rescue-tui` emits structured `tracing` events captured by `journalctl` in the rescue environment |
| I1  | Information Disclosure | `rescue-tui` error paths leak host info into TUI | A4 | Errors classified and redacted; only actionable diagnostics displayed |
| I2  | Information Disclosure | Cold-boot memory dump of rescue environment | A3 | Out of scope — user responsibility; rescue env holds no secrets beyond ISO metadata |
| D1  | Denial of Service | Malformed ISO hangs `iso-probe` | A6 | `iso-probe` has per-file timeout; bounded memory; fuzz coverage on parser |
| D2  | Denial of Service | `rescue-tui` crashes, leaves user at kernel panic | A4 | Panics caught; fallback to login shell on a reserved VT; documented recovery |
| E1  | EoP | Buffer bug in `kexec-loader` FFI escapes sandbox | A5 | `unsafe_code = forbid` crate-wide except 4 audited blocks with SAFETY comments; `kexec_load(2)` not exposed; `KEXEC_FILE_UNSAFE` flag not set |
| E2  | EoP | Lockdown bypass lets user-mode code load unsigned kernel | A3 → A1 | Out of our scope (kernel CVE); tracked via upstream security advisories; trigger for revisiting Option A if rate > 1 critical/year × 2 years |

## 8. ISO Handling Attack Surface

Every ISO the user selects passes through this chain. Each step is a failure-closed boundary.

1. **`iso-probe` discovery** — loop-mount in read-only mode, walk ISO9660/UDF, extract kernel + initrd + cmdline. Invalid layouts are skipped, not retried; malformed structures raise classified errors. Fuzzed via `cargo-fuzz` (see #3 / `crates/iso-parser/fuzz/`).
2. **`rescue-tui` preflight** — render quirk warnings from `iso-probe`'s compatibility matrix (see #6). Block `kexec` attempts that match known-broken signatures.
3. **`kexec-loader`** — open fd for the extracted kernel, pass to `kexec_file_load` with cmdline + optional initrd. Error classification table:
   - `EKEYREJECTED` → `SignatureRejected` → TUI: "Enroll this ISO's signing key via `mokutil` or use a distro-signed ISO"
   - `EPERM` → `LockdownRefused` → TUI: "Secure Boot policy refuses this operation"
   - `ENOEXEC` → `UnsupportedImage` → TUI: "This kernel image format is not supported"
   - other errno → `Io(raw)` with preserved errno for triage
4. **`reboot(LINUX_REBOOT_CMD_KEXEC)`** — does not return on success. If it returns, the path is classified as `Io` and reported; the rescue TUI remains usable.

## 9. Supply Chain

| Risk | Mitigation |
|------|------------|
| Compromise of aegis-boot binaries in distribution | Release artifacts signed; reproducible-build CI (`rescue-tui` sha256 equal across two passes) with `SOURCE_DATE_EPOCH` — see [BUILDING.md](./BUILDING.md) |
| Malicious Rust dependency | `cargo-deny` in CI (advisories + licenses + bans); minimum dep surface for `kexec-loader` (only `libc`, `thiserror`, `tracing`) |
| Malicious SDK/build image | `Dockerfile.locked` pins `ubuntu:22.04` by SHA-256 digest and Rust by version |
| `shim` / kernel compromise | Out of scope — we inherit distro + shim review board trust. ADR 0001 explicitly accepts this as the Option B tradeoff |

## 10. Assumptions

| A   | Assumption | Justification |
|-----|------------|---------------|
| Ax1 | UEFI Secure Boot is enforcing | Verified via `bootctl status` at build integration time; users deploy only in enforcing mode |
| Ax2 | The rescue kernel is signed by a CA in the platform or MOK keyring | Project ships tested integrations for Debian/Fedora/Ubuntu/Alpine/Arch/NixOS (per #6 compatibility matrix) |
| Ax3 | `KEXEC_SIG` is enabled and `kexec_load(2)` is disabled by lockdown | Required minimum kernel config — documented in README / BUILDING.md |
| Ax4 | Users will not disable Secure Boot to make unsigned ISOs boot | UX never prompts to; `mokutil` enrollment is the supported remedy |

## 11. Out of Scope

- Cold-boot memory attacks against the rescue environment.
- DMA attacks via Thunderbolt (platform configuration).
- Side-channel attacks against the platform (Spectre-class).
- TPM remote attestation (tracked separately — future ADR).
- Post-kexec behavior of the target ISO (outside our trust boundary).

## 12. Revisit Triggers

Per ADR 0001, this threat model is partly a bet on the kexec-under-SB story remaining stable. Revisit if:

- More than one critical CVE affecting `kexec_file_load` + `KEXEC_SIG` is published per year for two consecutive years.
- `shim` review board changes revocation cadence such that distro-signed kernels stop being reliably available across major distros.
- SBAT schema changes invalidate our trust inheritance assumption.

In any of these cases, ADR 0001's preserved Option A dissent becomes the starting point for re-evaluation.

## 13. Review Cadence

| Review | Frequency | Trigger |
|--------|-----------|---------|
| Full threat model refresh | Annually (next: 2027-04-14) | Calendar |
| STRIDE delta review | Per-PR if runtime trust boundary changes | Reviewer checklist |
| Assumption validation | Per release | Release checklist |
| Revisit-trigger audit | Quarterly | Calendar + CVE feed |
