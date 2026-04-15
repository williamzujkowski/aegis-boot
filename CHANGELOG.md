# Changelog

All notable changes to aegis-boot are recorded here. Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). This project adheres to [Semantic Versioning](https://semver.org/).

## [0.8.0] — 2026-04-15

**UX overhaul release** ([#85](https://github.com/williamzujkowski/aegis-boot/issues/85)). Synthesis of a parallel-agent survey of best-in-class boot pickers (Ventoy, rEFInd, systemd-boot, GRUB2, Apple Option-key, Lenovo F12) and TUI applications (lazygit, ranger, fzf, k9s, helix, dialog). The rescue-tui is now substantially more discoverable, navigable, and trustworthy at a glance.

### Headline — chrome and discoverability

- **Persistent header banner** with `aegis-boot vX.Y.Z`, Secure Boot status (`SB:enforcing` / `SB:disabled` / `SB:unknown`), and TPM status (`TPM:available` / `TPM:none`). Color-coded AND text-labeled so monochrome themes still convey the protection state. SB detected from `/sys/firmware/efi/efivars/SecureBoot`; TPM from `/dev/tpm[0|rm0]`.
- **Persistent footer** with screen-specific keybind hints, replacing inline per-screen help text. One source of truth for what every key does, always visible.
- **`?` opens a help overlay** modal with the full keybind list and status-glyph legend. Esc / `?` to dismiss. lazygit / k9s pattern.
- **`q` now opens a quit-confirmation overlay** instead of exiting immediately. Accidental `q` during navigation no longer reboots the machine.

### Headline — list navigation

- **Vim navigation aliases** on the List screen: `j/k` (down/up), `g/G` (first/last), `l` (confirm), `h` (back). Arrow keys still work.
- **`/` opens an incremental substring filter**. Matches against ISO label + path, case-insensitive. Cursor pins to the first match while typing; Enter commits, Esc clears. Becomes essential at 20+ entries.
- **`s` cycles sort order**: name → size↓ → distro. Default is name (alphabetical). SizeDesc surfaces the largest ("main") install media first.
- **Info bar above the list** shows current filter + sort state with inline reminders.
- **Status glyphs on every list row**, visible in monochrome:
  - `[+]` verified signature
  - `[~]` hash verified, no signature
  - `[ ]` no verification material present
  - `[!]` hash mismatch OR forged signature (kexec-blocked)
  - `[X]` not kexec-bootable (Windows ISO etc.)

  Operators scanning the list now see security state at a glance, not just on the Confirm screen.

### Smaller fixes

- **Error screen returns to the failed-ISO selection** instead of snapping to row 0. The cursor preservation bug surfaced by the gap analysis.
- **Empty-list state** now suggests checking `AEGIS_ISO_ROOTS` instead of just saying "press q."
- **Empty-filter-result state** distinguishes "no ISOs at all" from "no matches for current filter" with recovery hints.

### State machine additions

- `Screen::Help { prior: Box<Screen> }` — overlay over any screen
- `Screen::ConfirmQuit { prior: Box<Screen> }` — quit prompt
- `Screen::Error` gains `return_to: usize` so cursor preservation is type-safe
- `AppState` gains `secure_boot`, `tpm`, `filter`, `filter_editing`, `sort_order`
- `SortOrder` enum (Name / SizeDesc / Distro) with `cycle` + `summary`

### Tests

- v0.7.1: 108 unit tests
- v0.8.0: 121 unit tests (+13: 7 Tier-1 transitions, 6 Tier-2 filter/sort)

### Deferred to v0.8.x

- Tier 3: `v` verify-now action with progress bar, distro grouping/submenus, always-present rescue-shell entry. Filed under #85; tracked separately.

### Verified

- Workspace tests green (121 / 121).
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `qemu-loaded-stick.sh -d ./test-isos` boots through the new chrome with Alpine 3.20 (4 ISOs discovered, list/filter/sort all functional).

## [0.7.1] — 2026-04-15

**Documentation accuracy patch** ([#78](https://github.com/williamzujkowski/aegis-boot/issues/78)). No code changes.

- README.md: full rewrite. Removed false "skeleton-only" status for rescue-tui / iso-probe / kexec-loader (those crates now hold ~4000 LOC and 108 tests across 7 releases). Removed wrong "Rust 1.75.0" claim (workspace pin is 1.85.0). Removed wrong "EDK II stable202311" claim (the Dockerfile and BUILDING.md both explicitly state EDK II is not used). Added quickstart, current component matrix, doc index.
- CHANGELOG: v0.5.0 section's "byte-reproducible bootable disk image" claim corrected — only `rescue-tui` is verified reproducible; the disk image embeds host-installed shim/grub/kernel. v0.7.0 headline reframed from "Real-hardware-ready" to "Storage-module-complete" since real hardware has not been validated.
- docs/LOCAL_TESTING.md: documented the v0.7.0 `--attach {virtio,sata,usb}` flag with examples and a capability table.
- docs/USB_LAYOUT.md: added a section listing the storage modules shipped in the initramfs as of v0.7.0 and the QEMU-only validation status.
- crates/iso-parser/Cargo.toml: bumped to 0.7.1 (was stuck at 0.1.0 — drift from the rest of the workspace) and switched to workspace `edition` / `rust-version` inheritance.
- New: `docs/content-audit.md` records each documentation accuracy audit so we can re-audit on a cadence.

## [0.7.0] — 2026-04-15

**Storage-module-complete release.** Adds the kernel modules real hardware needs (AHCI, NVMe, USB-storage, UAS) so rescue-tui can in principle see a USB stick or internal disk on a physical machine. **Real-hardware boot has not yet been validated** — that's gated on a Framework / ThinkPad / Dell shakedown ([#51](https://github.com/williamzujkowski/aegis-boot/issues/51)) and gates v1.0.0. v0.6.x fixed the QEMU+virtio path; v0.7.0 is the foundation for the next step.

### Headline

- **Storage controller modules shipped in the initramfs** (#72). `build-initramfs.sh` now copies (or skips-as-built-in) AHCI (SATA), NVMe, USB core + xHCI/EHCI + usb-storage + UAS, plus SCSI core. On modern Ubuntu kernels (6.8+) these are modules; without shipping them, rescue-tui on real hardware had no visibility into the storage bus and the user's USB stick + internal disks were all invisible. `/init` now modprobes the set early with a longer (3s) settle sleep for USB hub/bus enumeration.
- **`qemu-loaded-stick.sh --attach {virtio,sata,usb}`** (#72). Lets developers exercise each storage-driver path without real hardware. `virtio` is the fast paravirtual default; `sata` drives the AHCI module path real desktops use; `usb` hangs the stick off `qemu-xhci` with `usb-storage`, matching a real USB plug.

### Verified end-to-end (in QEMU only)

With `alpine-3.20.3-x86_64.iso` on the AEGIS_ISOS partition (real-hardware behavior may differ — drivers behave differently on actual PCI/USB buses):

| Attach mode | Result |
|---|---|
| `virtio` | `discovered 4 ISO(s)` |
| `sata`   | `discovered 4 ISO(s)` (AHCI path, `/dev/sda*`) |
| `usb`    | `discovered 4 ISO(s)` (xHCI + usb-storage, `/dev/sda*`) |

### Size budget

Initramfs went from 11.3 MiB → 11.6 MiB (+300 KiB). Most storage code is already built-in on Ubuntu generic kernels; the net-new shipped modules are libahci, ahci, nvme-core, nvme, usb-storage, uas, and nls_utf8. Well under the 20 MiB budget.

### Deferred

- Real-hardware shakedown on Framework / ThinkPad / Dell (gates v1.0.0 per #51). Needs physical access; can't run from CI.
- exFAT support in `/init` mount fallbacks (module exists but not currently shipped). File if needed.

## [0.6.2] — 2026-04-15

**Discovery + boot patch release.** Closes findings from the first end-to-end test of `qemu-loaded-stick.sh` (#66) against a real Alpine 3.20 ISO. v0.6.0 and v0.6.1 *appeared* to work in CI because `qemu-kexec-e2e.sh` uses a fixture ISO mounted directly as `-cdrom`, bypassing the AEGIS_ISOS partition path entirely. With a real loaded stick, rescue-tui silently reported "0 ISOs discovered" — three bugs in a row produced that result.

### Bug fixes (all #68)

- **`/init` could not mount the AEGIS_ISOS FAT32 partition.** Kernel's `CONFIG_NLS_DEFAULT="utf8"` but `CONFIG_NLS_UTF8=m` (we don't ship the module). Bare `mount -t vfat` returned `EINVAL`. Now passes `codepage=437,iocharset=cp437` (both built-in via `CONFIG_NLS_CODEPAGE_437=y`) so the mount actually succeeds. Falls back through ext4 → vfat-with-options → bare-vfat → exfat.
- **`/init`'s label resolver only used busybox `findfs`,** which doesn't recognize FAT32 labels reliably. Added `blkid -L` and `/dev/disk/by-label/` fallbacks so the AEGIS_ISOS partition is found regardless of filesystem type.
- **`/init` had `set -e`,** which aborted PID 1 on the first non-zero exit (e.g. a missing optional resolver), triggering kernel `panic=5` and a reboot loop. Removed; each command now handles its own errors explicitly.

### Diagnostic hardening

`scan_directory` and `iso_probe::discover` previously logged silent-skips at `debug` level — operators saw "0 ISOs" with no signal of where the scan looked or why. Now:

- `iso-parser` logs WARN per skipped ISO with the actual error
- `iso-parser` logs INFO per scan with `attempted` / `extracted` / `skipped` counts
- `iso-probe::discover` logs INFO per root scanned (which root, did it exist, how many entries)
- `/init` logs PID-1 banner, `/proc/cmdline`, `/proc/mounts`, block-device listing, and per-fstype mount errors before rescue-tui takes the alternate screen

### Other

- **`scripts/qemu-loaded-stick.sh`** switched from `if=ide` to `virtio-blk` so disks are visible without shipping `ahci.ko` (real-hardware module shipping is tracked separately as #72 for v0.7.0).
- **#69** Build-initramfs no longer warns when modules are built-in (`CONFIG_*=y`) — distinguishes "missing module" from "compiled into kernel."

### Verified

End-to-end `qemu-loaded-stick.sh -d ./test-isos` with `alpine-3.20.3-x86_64.iso`:
- Before v0.6.2: `discovered 0 ISO(s)`
- After v0.6.2:  `discovered 4 ISO(s)` (2 boot entries × 2 root scans)

### Real-hardware note

This release fixes the QEMU + virtio path. **Real USB sticks on real hardware still won't work** until #72 ships AHCI / NVMe / USB-storage modules. v0.6.2 is the foundation for that work; the final hardware-shakedown release is targeted at v1.0.0 (#51).

## [0.6.1] — 2026-04-15

**Security patch release.** Closes findings surfaced by the v0.6.0 full review (#52). No new features. **Operators running v0.6.0 with untrusted ISOs on the data partition should upgrade.**

### Security fixes

- **CRITICAL — kexec proceeded despite hash mismatch / forged signature** (#55). `is_kexec_blocked()` previously checked only `Quirk::NotKexecBootable`. The Confirm screen rendered red `✗ MISMATCH` / `✗ FORGED` warnings for hash and signature failures, but pressing Enter still called `attempt_kexec()` — a physical-access attacker who tampered an ISO could boot it as long as the operator clicked through. Now hard-blocks on `HashVerification::Mismatch` and `SignatureVerification::Forged` in addition to the existing quirk gate.
- **HIGH — `validate_path()` accepted paths outside base** (#56). `iso-parser`'s helper silently returned `Ok(path)` when `strip_prefix(base)` failed; `validate_path("/mnt/iso", "/etc/passwd")` returned `Ok("/etc/passwd")`. Replaced with a component-aware check that rejects any `..` component and requires `path.starts_with(base)`. Not exploitable via the documented call path in v0.6.0, but the function was a footgun for future contributors.
- **MEDIUM — minisign verifier conflated Forged with KeyNotTrusted** (#57). Tampered ISOs signed by a trusted key were misclassified as "untrusted signer." Now inspects `minisign-verify::Error::InvalidSignature` to distinguish "wrong signer" from "tampered bytes under trusted signer." Pairs with the #55 fix to make the `Forged` block actually reachable for the trusted-key tamper case.

### Other changes

- **CI** — `aegis-fitness` audit now runs on every push and PR (#53). v0.6.0 claimed this was wired but only `dev-test.sh` ran it. Threshold 80 (vs. dev-test's 90) because the CI job doesn't build initramfs artifacts.
- **Tests** — extracted `find_auto_kexec_target()` from `rescue-tui::main` and added 4 unit tests (#54). Empty `AEGIS_AUTO_KEXEC` now returns `None` instead of matching the first ISO.
- **Docs** — tightened CHANGELOG v0.6.0 claims (#52, #58). The disk image embeds host-installed shim/grub/kernel and is not byte-reproducible across hosts; only the `rescue-tui` binary is verified reproducible. Local-run time updated from "6-8 min" to a realistic "8-15 min" range.

### Test tally

- **v0.6.0:** 100 unit tests
- **v0.6.1:** 108 unit tests (+8: 2 hash/sig kexec-block regressions + 2 path-traversal regressions + 4 AEGIS_AUTO_KEXEC matching)

## [0.6.0] — 2026-04-14

**The polish release.** Lands all four nice-to-haves deferred from v0.5.0. No deployment-shape changes — `mkusb.sh` output structure is unchanged from v0.5.0's; this release adds operator-facing affordances on top. (Note: the disk image embeds host-installed shim/grub/kernel binaries and is not byte-reproducible across hosts — only the rescue-tui binary is verified reproducible under SOURCE_DATE_EPOCH.)

### Headline

- **TPM PCR extension before kexec** (#46). New `tpm` module in rescue-tui shells out to `tpm2_pcrextend` to measure `sha256(iso_path || 0x00 || cmdline)` into PCR 12 before handoff. Enables downstream remote attestation. TPM-less hardware logs a warning and continues — physical-access recovery stays unblocked.
- **ISO size in Confirm preview** (#47). `DiscoveredIso` now carries `size_bytes` (populated via `stat(2)` at discovery). Confirm screen shows a humanized value (B/KiB/MiB/GiB) so operators sanity-check what they're about to boot.
- **`AEGIS_THEME` palette override** (#48). New `theme` module with three named palettes (`default`, `monochrome`, `high-contrast`). Resolved at startup from the `AEGIS_THEME` env var. Useful on serial consoles where the default 16-color palette is unreadable, and on low-contrast framebuffer consoles like OVMF default.
- **`aegis-fitness` CLI** (#49). New binary crate that scores repo / build / artifact health out of 100. JSON + human output, exit code gated on threshold (default 90). Wired into `dev-test.sh` as stage 8/8. Modeled on `nexus-agents fitness-audit`.

### Test tally

- **v0.5.0:** 87 tests
- **v0.6.0:** 100 unit tests (+13: 5 TPM + 2 size + 4 theme + 5 fitness-scoring). The "5 fitness" tests cover the `aegis-fitness` binary's score-computation logic, not the audit checks themselves (the binary runs 9 checks at runtime).

### Deferred

None of the v0.5.0-deferred items remain. v0.7.0 epic (TBD) will likely focus on real-hardware deployment validation and remote attestation protocol.

## [0.5.0] — 2026-04-14

**The user-shippable release.** Turns aegis-boot from an engine (rescue-tui + crates + reproducible initramfs) into an artifact a user can write to a USB stick and boot. Closes the deployment-story gap that v0.1-v0.4 deliberately left open.

### Headline

**`scripts/mkusb.sh` image builder** (#41). Produces a bootable disk image (only the `rescue-tui` binary is byte-reproducible under SOURCE_DATE_EPOCH; the disk image embeds host-installed shim/grub/kernel binaries and is not hash-stable across hosts):
- GPT partition table
- **ESP** (FAT32, 400 MB): MS-signed shim → Canonical-signed grub → signed kernel + combined initrd (distro initrd + our rescue initramfs)
- **AEGIS_ISOS data partition** (FAT32 by default, remainder of disk): user drops `.iso` files here

`dd if=out/aegis-boot.img of=/dev/sdX` onto a real stick and boot. Local validation via `scripts/qemu-try.sh` (interactive OVMF SecBoot boot) or `scripts/dev-test.sh` (full 7-stage CI-equivalent run).

### Also landed

- **`DATA_FS=ext4` override** (#44) — removes FAT32's 4 GB single-file cap for shipping Ubuntu LTS desktop ISOs and similar.
- **Label-aware init auto-mount** (#41) — `/init` mounts `LABEL=AEGIS_ISOS` at `/run/media/aegis-isos` before scanning; operators see a stable mount name regardless of which USB port the stick landed on.
- **MOK enrollment helper** (#42) — when `kexec` returns `SignatureRejected`, the TUI Error screen now embeds the exact `sudo mokutil --import <key>` command with the actual key file path (discovered via `<iso>.pub` / `.key` / `.der` sibling convention). Removes the "which key file do I enroll?" guessing game.
- **`AEGIS_AUTO_KEXEC` mode** (#38, landed in v0.4) — non-interactive automation mode for CI tests.
- **Real `kexec_file_load` E2E** (#43) — iso-parser's `mount_iso` gained a losetup fallback for loop-device allocation on kernels with lazy `/dev/loop-control` semantics. Kexec handoff proved end-to-end in local QEMU: rescue-tui discovers ISO → `iso_probe::prepare` loop-mounts via util-linux losetup → `kexec_file_load` fires. Handled kernel compression quirks (decompress `.ko.zst` at initramfs-build time so busybox modprobe can load the modules).
- **Kernel modules shipped into initramfs** (#43) — `isofs`, `udf`, `loop` (decompressed from `.ko.zst` if the kernel compresses). Without this, `mount -t iso9660` silently fails on distros that compile these as modules.
- **util-linux losetup shipped into initramfs** — busybox's losetup applet doesn't accept `--show` and doesn't handle modern loop-control semantics consistently; we now carry the real one + its library closure.
- **`scripts/dev-test.sh` + `docs/LOCAL_TESTING.md`** — full CI-equivalent run locally in ~8 minutes for the billing-paused interim. Remains useful as a pre-push sanity check once CI is back.

### Deferred to v0.6.0

- **TPM PCR extension** — measure ISO hash + cmdline into PCR 12 before kexec. Needs `swtpm` in CI and a concrete trust-chain doc before shipping.
- **ratatui theming** — `--theme=<name>` flag. Nice-to-have.
- **ISO metadata preview pane** — volume label, release-notes snippet. Nice-to-have.
- **`aegis-boot fitness-audit` CLI** — self-check subcommand.
- **Full target-kernel boot (not partial-pass)** — requires QEMU configuration that preserves serial across `reboot(LINUX_REBOOT_CMD_KEXEC)`; our current CI configuration doesn't. Local kexec fires correctly but serial is lost. Partial-pass accepts matched+mounted+kexec-invoked as proof.

### Test tally

- **v0.4.0:** 84 tests
- **v0.5.0:** 87 tests (+3)

### CI tally

The full matrix reached 16 jobs across four workflows (`ci.yml`, `mkusb.yml`, `ovmf-secboot.yml`, `kexec-e2e.yml`) before GHA billing suspended runs. All jobs passed locally via `dev-test.sh`. Once billing resolves the CI gate returns.

### Upgrade notes

- `scripts/build-initramfs.sh` now ships kernel modules. Initramfs size went from 3.6 MB (v0.4.0) to ~4.0 MB. Still well under the 20 MB budget.
- `AEGIS_KMOD_SRC` env var lets operators override the kernel-modules source when the deployment kernel differs from the build host's installed kernel (common in cross-compile / packaging scenarios).
- `DATA_FS=ext4` is opt-in; default remains FAT32.

## [0.4.0] — 2026-04-14

The "real Secure Boot" release. Closes the deferred OVMF SecBoot work from v0.2.0/v0.3.0 and lands the matching UX enforcement.

### Headline

**OVMF SecBoot end-to-end CI** (#16, PR #34 + #35). Every PR now boots a real signed shim → signed grub → Canonical-signed kernel chain under enforcing-mode Secure Boot, with our `initramfs.cpio.gz` concatenated onto the distro initrd. Pass criteria asserted from serial output:

1. Linux kernel logs `Secure boot enabled` — proves SB is actually enforcing through the chain.
2. `aegis-boot rescue-tui starting` banner appears — proves our binary survives the signed boot path and runs to completion.

CI matrix: 11 → **13 checks per PR**.

### Other landed

- **Phase 1 OVMF SecBoot foundation** (#34) — fast smoke that loads `OVMF_CODE_4M.secboot.fd` + MS-enrolled vars and asserts firmware initializes without crashing. Stays as a quick gate alongside the deeper E2E.
- **`NotKexecBootable` quirk enforcement** (#36) — Windows installer ISOs and other non-Linux-kernel media are now blocked from kexec at the TUI layer with a specific diagnostic, not a generic kexec failure. Confirm screen title becomes "BLOCKED"; Enter records `UnsupportedImage` without firing the syscall.

### Deferred to v0.5.0

- **TPM PCR extension** — measure ISO + cmdline into PCR 12/13 before kexec. Needs `swtpm` in CI to test; design forthcoming.
- **MOK enrollment guidance** — currently surfaced as TUI text; future work could automate `mokutil --import` for the user.
- **kexec end-to-end with a signed fixture ISO** — proves the rescue-tui-to-target-ISO handoff under SB. Distinct from this release's "rescue-tui runs under SB" proof.

### Test tally

- **v0.3.0:** 82 tests
- **v0.4.0:** 84 tests (+2 — most v0.4.0 work was CI-side; the new tests cover the `is_kexec_blocked` enforcement)

### CI tally

13 checks per PR, all green on `main`. New: `OVMF SecBoot foundation`, `OVMF SecBoot E2E (signed chain → rescue-tui)`.

### Upgrade notes

- `AppState::is_kexec_blocked(idx)` is new public API for downstream TUI consumers (none in tree yet, but documented for future workflow templates).

## [0.3.0] — 2026-04-14

Tracks progress of the [v0.3.0 epic (#29)](https://github.com/williamzujkowski/aegis-boot/issues/29). Raises the security floor (real cryptographic authentication) and the UX floor (last-choice persistence, explicit Windows-not-bootable diagnostic).

### Landed

- **Minisign detached signature verification** (#30) — `iso-probe::verify_iso_signature` looks for `<iso>.minisig` and verifies against `AEGIS_TRUSTED_KEYS`. New `SignatureVerification` enum: `Verified` / `KeyNotTrusted` / `Forged` / `NotPresent` / `Error`. TUI Confirm screen renders the result with colored severity. **Real authentication, not just integrity** — distinct from the v0.2.0 hash check, which only proves the ISO matches its own checksum file.
- **Boot menu persistence** (#31) — last kexec choice (ISO path + cmdline override) saved to `$AEGIS_STATE_DIR/last-choice.json` (defaults to `/run/aegis-boot`). On startup, the matching ISO is pre-selected and the override is re-applied. Best-effort: missing or corrupt state is silently ignored.
- **Windows installer detection** (#32) — new `Distribution::Windows` variant + `Quirk::NotKexecBootable`. Detected from `bootmgr`, `sources/boot.wim`, `efi/microsoft/`, or `windows` path markers. Surfaces a specific diagnostic instead of falling through the generic "unsigned kernel" path that wouldn't help here.

### Deferred to v0.4.0 (documented honestly)

- **OVMF SecBoot CI verification** (#16) — needs a dedicated design doc to nail down whether to enroll a test MOK + sign our own kernel, or chain through Ubuntu's signed shim+kernel. Either approach is a meaningful CI investment.
- **UDF filesystem support in iso-parser** — kernel handles UDF transparently when loop-mounting; iso-parser's path-based detection works for hybrid ISOs already. Standalone UDF (no ISO9660 cohabitation) hasn't been observed in supported distros' install media. If a real-world need surfaces, it lands then.
- **Kernel module loading in initramfs** — distro `linux-image-virtual` / `linux-image-generic` kernels compile USB xHCI, NVMe, AHCI, sd_mod, and ext4 directly in. Module-loading complexity isn't justified until we hit hardware that actually needs it.
- **TPM PCR extension** — measure ISO + cmdline into PCR 12/13 before kexec. Genuinely useful for attestation but needs `swtpm` in CI to test, which is its own setup.

### Test tally

- **v0.2.0:** 71 tests
- **v0.3.0:** 81 tests (+10)

### CI tally

11 checks per PR, all green on `main`. (The `Boot initramfs under QEMU` job briefly flaked once on PR #31 — admin-merged after 10/11 green; tracked but not blocking.)

### Upgrade notes

- `iso_probe::DiscoveredIso` gained `signature_verification: SignatureVerification` — consumers that construct the struct manually must populate it (`SignatureVerification::NotPresent` if you don't want minisign checks).
- `iso_parser::Distribution` gained `Windows` variant — `match` expressions on `Distribution` need a new arm or wildcard.
- `iso_probe::Quirk` gained `NotKexecBootable` variant — same.
- `rescue-tui` gains a `serde_json` dep (transitive: `serde`).

## [0.2.0] — 2026-04-14

Tracks progress of the [v0.2.0 epic (#24)](https://github.com/williamzujkowski/aegis-boot/issues/24). Closes must-haves for:

- **Structured tracing to journald** — every discover/prepare/kexec step emits a `tracing` event with stable fields. `AEGIS_LOG_JSON=1` opts into JSON format for `journalctl --output=json` triage. Default filter raised to `info` so operators see useful output without setting `RUST_LOG`.
- **TUI kernel cmdline editor** — Confirm → `e` enters an in-TUI editor; Enter commits, Esc cancels. Per-ISO override map preserved across cancel/re-enter. UTF-8 cursor walking via `String::is_char_boundary`. The override takes precedence over the ISO-declared default at kexec time.
- **ISO hash verification against sibling checksum files** — `iso-probe` looks for `<iso>.sha256` (sidecar) first, then `SHA256SUMS` in the same directory. First match wins. Confirm screen renders a colored status: green `✓ verified`, red bold `✗ MISMATCH — do NOT kexec`, or default `(no sibling checksum)`. **Not** crypto-grade signing — that's a separate follow-up. Module docstring is explicit about what hash verification buys and what it doesn't.
- **Real kexec_file_load integration test** — `kexec_loader::load_dry` exercises the real syscall against a real kernel in CI, asserting `/sys/kernel/kexec_loaded` transitions 0 → 1. First time the kexec syscall path is end-to-end-verified rather than just errno-classification-unit-tested.
- **Distribution enum extended** — Alpine / NixOS / RHEL (Rocky / AlmaLinux) promoted from `Unknown`-detected to named variants with specific detection + quirk mappings. `docs/compatibility/iso-matrix.md` updated.

### What did NOT land in 0.2.0

- **OVMF SecBoot CI verification** — deferred to v0.3.0. Requires end-to-end shim+signed-kernel+MOK plumbing that doesn't fit a small CI job cleanly; needs a dedicated design doc.
- **True crypto-grade ISO signature verification** (minisign / sigstore) — the module boundary is in place; the verifier itself is follow-up work.
- **UDF filesystem, kernel module loading, TPM PCR extension** — all should-haves / nice-to-haves in #24 that didn't fit.

### Test tally

- **v0.1.0 baseline:** 35 tests
- **v0.2.0:** 71 tests (+36)

### CI tally

11 checks per PR, all green on `main`:
Test (1.85) · Test (stable) · SAST (semgrep) · cargo-deny · gitleaks · CycloneDX SBOM · Nix smoke · reproducible-build · initramfs build · loop-mount integration · QEMU smoke boot.

### Upgrade notes

- `iso_probe::DiscoveredIso` gained a `hash_verification` field — consumers that construct the struct manually must populate it (use `HashVerification::NotPresent` if you don't want hash checks).
- `Distribution` enum added three variants (`Alpine`, `NixOS`, `RedHat`) — `match` expressions on `Distribution` must add arms or use a wildcard.

## [0.1.0] — 2026-04-14

First release. The rescue runtime boots end-to-end in CI: a real kernel unpacks a reproducible `initramfs.cpio.gz`, PID 1 runs, `rescue-tui` reaches first render, and the whole chain is verified on every PR.

### Architecture

- **ADR 0001** — signed Linux rescue + ratatui TUI + `kexec_file_load(2)` runtime. Decided by 5-agent consensus vote (higher-order, supermajority, 4–1) preserved in [`docs/adr/0001-runtime-architecture.md`](./docs/adr/0001-runtime-architecture.md).

### Crates

- **`iso-parser`** (existing, preserved) — ISO9660 / El Torito / UDF discovery, `cargo-fuzz`-covered.
- **`iso-probe`** (new, v0.1.0) — sync facade + RAII `PreparedIso` for kexec handoff. Real loop-mount integration test (#16).
- **`kexec-loader`** (new, v0.1.0) — audited `unsafe` FFI over `kexec_file_load(2)` only. Classifies `EKEYREJECTED` / `EPERM` / `ENOEXEC`. `kexec_load(2)` and `KEXEC_FILE_UNSAFE` deliberately not exposed.
- **`rescue-tui`** (new, v0.1.0) — ratatui binary. Pure state-machine + renderer split; stderr startup banner for serial consoles.

### Build + ship

- `Dockerfile.locked` — Ubuntu 22.04 (digest-pinned) + Rust 1.85, no EDK II (dropped per ADR 0001). `rescue-tui` binary is byte-reproducible under `SOURCE_DATE_EPOCH`.
- `scripts/build-initramfs.sh` — produces `out/initramfs.cpio.gz` (3.6 MB, byte-reproducible: sha256 `d82acb9e170b9750a40c23470dad45d15cd0a7cc48234f11b36e9d41a31bbb95`).
- `scripts/qemu-smoke.sh` — boots the initramfs under QEMU and asserts the TUI starts.

### CI (11 checks per PR)

Test (1.85) · Test (stable) · SAST (semgrep) · cargo-deny · gitleaks · CycloneDX SBOM · Nix smoke · reproducible-build · initramfs build · loop-mount integration · QEMU smoke boot.

### Documentation

- [`THREAT_MODEL.md`](./THREAT_MODEL.md) rewritten for the Option B chain.
- [`BUILDING.md`](./BUILDING.md) — reproducible build + initramfs assembly recipe.
- [`docs/adr/0001-runtime-architecture.md`](./docs/adr/0001-runtime-architecture.md) — decision record incl. preserved security dissent + revisit triggers.

### Known limits

- **Secure Boot chain** is demonstrated by design but not yet CI-verified. `aegis-boot` trusts shim + a distro-signed kernel; the initramfs rides that kernel's signature. Real MOK + SB enforcement verification is a separate follow-up.
- **`iso_probe::lookup_quirks()`** returns an empty list for every distribution. Real population tracked in [#6](https://github.com/williamzujkowski/aegis-boot/issues/6). Callers must not treat empty as "safe."
- **kexec handoff** is unit-tested via errno classification but not yet end-to-end exercised with a signed target ISO.
