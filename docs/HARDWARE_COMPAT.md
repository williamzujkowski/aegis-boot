# Hardware compatibility

A community-curated table of machines aegis-boot has been seen to boot on (or fail on) under enforcing UEFI Secure Boot. The point: when an operator asks "will this work on my laptop?", the answer is concrete instead of a shrug.

If you successfully boot — or fail to boot — aegis-boot on a machine not listed here, please **submit a report** (see [How to report](#how-to-report) below). One report from one operator is more useful than zero from many.

> **Looking up a specific machine?** `aegis-boot compat [query]` queries the same data from the CLI (e.g. `aegis-boot compat thinkpad`). `--json` output is stable (schema_version=1) for scripting.

## How to read this

| Column | Meaning |
|---|---|
| **Machine** | Vendor + model + (year if relevant). |
| **Firmware** | UEFI vendor / version. Fast-boot status if known. |
| **SB state** | Secure Boot mode at boot time. **enforcing** = the goal; **audit/setup/disabled** = aegis-boot still boots but the trust claim is weakened. |
| **Boot key** | Firmware boot-menu key (varies by vendor: F12, F11, Esc, Del). |
| **Stick → boot** | Did aegis-boot's signed chain reach rescue-tui? |
| **kexec** | Did the operator-selected ISO actually kexec? |
| **Quirks** | Anything an operator should know before booting this machine. |
| **Reported by** | GitHub handle (or anonymous). |

A row in this table represents **one** operator's outcome. Multiple rows for the same machine are fine — the more data, the better.

---

## Validated platforms

### Real hardware

| Machine | Firmware | SB state | Boot key | Stick → boot | kexec | Quirks | Reported by | Date |
|---|---|---|---|---|---|---|---|---|
| Generic SanDisk Cruzer Blade 32GB on x86_64 host | OVMF 4M (Debian package, MS-enrolled vars) | enforcing | n/a (USB-passthrough → QEMU) | ✅ | ✅ Ubuntu 24.04.2 boots; ✅ Alpine 3.20.3 correctly refused with `errno 61` | None observed in shakedown (#109) | @williamzujkowski | 2026-04-16 |

The shakedown was performed via QEMU USB-passthrough on a real SanDisk Cruzer 32 GB stick written by `aegis-boot flash`. Direct boot on physical Framework / ThinkPad / Dell hardware is the next gate ([#51](https://github.com/williamzujkowski/aegis-boot/issues/51)) and would expand this table considerably.

### QEMU / virtualized

QEMU + OVMF + the MS-enrolled VARs file is the reference test environment. It boots cleanly on every CI run; consider it the floor of what aegis-boot supports.

| Environment | OVMF version | SB state | Stick → boot | kexec | Notes |
|---|---|---|---|---|---|
| QEMU q35 + OVMF (Ubuntu 22.04 packaged `OVMF_CODE_4M.secboot.fd` + `OVMF_VARS_4M.ms.fd`) | 2024.02-2 | enforcing | ✅ | ✅ | The reference test environment used by all CI workflows. |

---

## How to report

### What we want to capture

For a successful boot:
- Machine vendor + model + (year)
- Firmware vendor + version (often visible in BIOS settings; on Linux: `sudo dmidecode -s bios-version` and `sudo dmidecode -s bios-vendor`)
- Secure Boot state (`mokutil --sb-state` from a Linux ISO booted on the machine, or read from BIOS)
- Boot-menu key
- Which ISO you booted (vendor + version)
- Output of `aegis-boot --version` from the workstation that wrote the stick

For a failure:
- Same as above
- The exact failure mode (won't appear in firmware boot menu? grub starts but kernel doesn't? rescue-tui starts but kexec fails?)
- Relevant `dmesg` output if the kernel reached you (the [TROUBLESHOOTING.md](./TROUBLESHOOTING.md) guide has notes on capturing this)

### Where to submit

Open a GitHub issue with the **`hardware-report`** label using the bug template at [`.github/ISSUE_TEMPLATE/bug.yml`](../.github/ISSUE_TEMPLATE/bug.yml) — the same fields apply. We'll periodically curate accepted reports into this document.

Alternatively, open a PR adding a row to the table directly. We'll accept PRs from any contributor who's actually booted aegis-boot on the machine in question.

### What we don't want

- Speculation ("I think a ThinkPad X1 should work because aegis-boot uses standard EFI…"). The whole point of this table is verified outcomes — please only report machines you've personally booted or failed to boot.
- Reports under disabled Secure Boot. aegis-boot still works, but the trust claim that justifies its existence is undermined; SB-disabled outcomes don't help operators who specifically want SB-preserving boot.

## Future: automated reports

A planned feature ([epic #137](https://github.com/williamzujkowski/aegis-boot/issues/137)) is `aegis-boot doctor --report` — opt-in anonymous telemetry that posts the host's firmware vendor / model / SB state / outcome. The data shape will be public and operators will see exactly what's submitted before consenting per-run. No data is collected today.

## See also

- [INSTALL.md](./INSTALL.md) — operator install walkthrough
- [TROUBLESHOOTING.md](./TROUBLESHOOTING.md) — common failure modes by category
- [UNSIGNED_KERNEL.md](./UNSIGNED_KERNEL.md) — what to do when an ISO's kernel isn't signed
- [#51](https://github.com/williamzujkowski/aegis-boot/issues/51) — the multi-vendor real-hardware shakedown gate for v1.0.0
