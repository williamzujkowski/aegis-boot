# Changelog

All notable changes to aegis-boot are recorded here. Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). This project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Cross-platform test fixes + Windows cache-dir fallback (#502)

Three tests were previously Linux-biased; #502 makes them run cleanly on Windows and broadens the #501 full-suite gate to all 593 aegis-bootctl tests:

- `bug_report::tests::read_stick_logs_parses_valid_and_skips_malformed` — filename had `:` in it (ISO-8601 timestamp), which NTFS forbids. Renamed to dashes; the in-file JSON still carries the colon form.
- `doctor::tests::check_command_present_with_pkg_finds_existing_binary` — probed for `ls` in PATH; now uses `cmd` on Windows, `ls` elsewhere.
- `fetch_image::default_cache_path` tests — depended on POSIX `XDG_CACHE_HOME` / `HOME`. The function now also respects `LOCALAPPDATA` + `USERPROFILE` fallbacks (Windows-standard cache conventions), which fixes the tests AND makes the real tool behave correctly on Windows end-user installs.

### Windows-2022 CI gate (#420 stub)

New `windows-cargo-check.yml` workflow runs `cargo check -p aegis-bootctl --all-targets` + `cargo test -p aegis-bootctl --locked` on a `windows-2022` GitHub-hosted runner. Catches MSVC-vs-GNU drift the cross-compile-from-Linux gate misses + exercises the 70+ new aegis-cli tests (pipeline, source_resolution, drive_enumeration, flash_dispatcher) on a real Windows host. Clippy runs with `continue-on-error` until the pre-existing Windows-gated-code backlog clears. First piece of the full #420 multi-OS matrix.

### Windows `--direct-install` CLI dispatcher (#497 piece 4 — closes #497 + parent #483)

`aegis-boot flash --direct-install` now works on Windows, completing the [epic #419](https://github.com/aegis-boot/aegis-boot/issues/419) Windows direct-install adapter. The dispatcher lives in `windows_direct_install::flash_dispatcher` and composes: drive-arg parsing → source resolution (from `--out-dir` with env overrides) → pipeline::run (preflight → partition → format ESP + AEGIS_ISOS → stage_esp).

- Drive identifier accepts `1`, `PhysicalDrive1`, or `\\.\PhysicalDrive1` — no quoting gymnastics.
- If no drive arg is given, the dispatcher lists flashable candidates and exits with a remediation hint ("re-run with an explicit drive argument"). No interactive prompt — WinRM / remote SSH invocations often have a closed stdin, and a silent prompt-hang is worse than a clear message.
- `--yes` is required on the first operator-facing invocation (destructive-action guard). Without it, the dispatcher surfaces the candidate list and exits 2.
- Receipt-formatted per-stage timing report on success matches the shape of Linux's `flash_direct_install` ending output.
- Pure core `run_direct_install_using(explicit_dev, out_dir, enumerate_fn, runner)` takes injected enumerator + `PhaseRunner`, so the full happy-path + 5 error-path branches are Linux-testable without PowerShell (15 new unit tests).

macOS gets a clearer refusal: the cfg-gated error message now points at #418 (macOS adapter tracker) instead of the generic "Linux-only" text.

### Windows drive enumeration (#497 piece 3)

New `windows_direct_install::drive_enumeration` module wraps `Get-Disk | ConvertTo-Json` so the CLI can list flashable physical drives, filtered to safe candidates (not disk 0, not `IsBoot`, not `IsSystem`, not read-only, ≥1 GiB). Pure-fn JSON parser + filter are unit-tested on Linux via canned output; the subprocess wrapper is Windows-gated. 14 tests cover the Win11-VM canned output shape, BOM-prefix tolerance, missing-optional-field tolerance, every filter predicate, stable ordering, human-readable size formatting, and `BusType` integer-enum mapping.

### Windows signed-chain source resolver (#497 piece 2)

New `windows_direct_install::source_resolution` module resolves the six ESP chain files from an operator-controlled `out_dir` with per-file env var overrides (`AEGIS_SHIM_SRC`, `AEGIS_GRUB_SRC`, `AEGIS_MM_SRC`, `AEGIS_GRUB_CFG`, `AEGIS_KERNEL_SRC`, `AEGIS_INITRD_SRC`). Default filenames match the names `scripts/mkusb.sh` writes into `out/` so a developer who built the chain on Linux can run direct-install on Windows against the same directory without renaming. Missing-file errors collect every missing file (not fail-fast) so an operator who staged 4 of 6 sees all remaining names in one shot. 10 unit tests on the pure-fn surface (host-agnostic — no Windows needed).

### Windows direct-install pipeline composer (#483)

New `windows_direct_install::pipeline` module composes the four Phase-module stages of [epic #419](https://github.com/aegis-boot/aegis-boot/issues/419) — preflight (elevation + BitLocker) → partition (diskpart) → format ESP + `AEGIS_ISOS` → `stage_esp` — into a single `run(runner, plan)` entrypoint with per-stage timing receipts and abort-on-first-failure cascade.

The phase dispatch is routed through a `PhaseRunner` trait so the composition logic is unit-testable on any host: 13 tests cover the happy path + every stage's abort behavior + receipt math, all executable via `cargo test --locked` on Linux. The default `WindowsPhaseRunner` wires each method straight through to the already-validated phase modules.

Deliberately out of scope for #483 (tracked separately): drive enumeration, signed-chain source path resolution, attestation manifest writing, and `aegis-boot flash --direct-install` CLI dispatch on Windows. Those need their own design passes for the non-Linux host; this PR lands just the composer so the dispatch work has a stable target.

### Windows direct-install raw-write wiring (#484, Phase 3 of #419)

`windows_direct_install::raw_write::{write_bytes_to_physical_drive, stage_esp}` now call the real Win32 APIs, replacing the `"not yet wired"` stubs from [#449](https://github.com/aegis-boot/aegis-boot/issues/449).

- `write_bytes_to_physical_drive` opens `\\.\PhysicalDriveN` with `GENERIC_READ | GENERIC_WRITE`, `FILE_SHARE_NONE`, `FILE_FLAG_NO_BUFFERING | FILE_FLAG_WRITE_THROUGH`, queries the on-disk sector size via `IOCTL_DISK_GET_DRIVE_GEOMETRY_EX`, issues `FSCTL_LOCK_VOLUME` before the write, loops `WriteFile` on a 4 MiB page-aligned buffer (sector-aligned via `std::alloc` with 4 KiB layout), and finishes with `FSCTL_DISMOUNT_VOLUME` so Windows re-reads the partition table.
- `stage_esp` enumerates volumes via `FindFirstVolumeW` + `IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS` to find the one backed by the target physical drive, then uses buffered FAT32 FS-driver I/O to copy each of the 6 signed-chain files under `\\?\Volume{GUID}\EFI\BOOT\…`.
- RAII wrappers (`OwnedHandle`, `AlignedBuffer`) ensure `CloseHandle` + `dealloc` fire on every exit path, so a write-loop error doesn't leak an exclusive handle (which would leave the volume locked until the process exits).
- Each `unsafe` block carries its own narrow `#[allow(unsafe_code)]` with a documented safety invariant; the workspace-level `unsafe_code = "deny"` catches any unannotated slip.
- New `Win32_Security` feature on the `windows = "0.58"` dep, required by `CreateFileW`'s `SECURITY_ATTRIBUTES` parameter even when passed as `None`.
- Integration test `raw_write_roundtrip_on_scratch_disk` writes a 12 KiB pattern at offset 64 KiB and reads it back; opt-in via `AEGIS_BOOT_RAW_WRITE_TEST_DRIVE=<n>` env var so accidental execution can't destroy data. Validated on Win11 (QEMU SATA scratch disk) against `\\.\PhysicalDrive1`.

Unblocks [#483](https://github.com/aegis-boot/aegis-boot/issues/483) (the CLI integration phase that composes partition + format + raw_write + preflight under `aegis-boot flash --direct-install`).

## [0.17.0] — 2026-04-24

Headline themes: the rescue-tui UX overhaul from [epic #455](https://github.com/aegis-boot/aegis-boot/issues/455) (dual-pane layout, 6-tier trust model, every ISO on the stick is now visible with a descriptive verdict), ADR 0003 cross-reboot persistence shipped end-to-end, MSRV bumped to Rust 1.88, first-class NixOS install path via flake, and the entire major-dep-bump backlog from #411 cleared (indicatif, toml, schemars, sha2).

### rescue-tui UX overhaul — dual-pane + all-ISOs-visible (#455 epic, 7 PRs)

Triggered by [#454](https://github.com/aegis-boot/aegis-boot/issues/454) (a tester reported drag-dropped ISOs were "not obvious" in the TUI). The new UI surfaces every `.iso` file on `AEGIS_ISOS` with a tier verdict instead of hiding un-verifiable ones behind a count banner.

**Trust-tier model** — 6 named tiers replace the 4-color coarse verdict:

| Tier | Verdict             | Bootable | Meaning                                    |
| ---- | ------------------- | -------- | ------------------------------------------ |
| 1    | OperatorAttested    | yes      | Hash or sig verified vs trusted source     |
| 2    | BareUnverified      | yes\*    | No sidecar — typed-confirm required        |
| 3    | KeyNotTrusted       | yes\*    | Sig parses, signer untrusted               |
| 4    | ParseFailed         | **no**   | iso-parser couldn't extract kernel         |
| 5    | SecureBootBlocked   | **no**   | Kernel rejected by platform keyring        |
| 6    | HashMismatch        | **no**   | ISO bytes don't match declared hash        |

Design principle: *Secure Boot stays strict; operator attestation relaxes gracefully.* An ISO that fails signature verification is never bootable; an ISO that lacks operator attestation is bootable with friction proportional to the missing signal.

**Dual-pane layout** — gitui-style: 40% ISO list on the left, 60% info pane on the right. `Tab` toggles focus. Info pane shows per-tier metadata (file, size, sha256, signer, kernel, initrd, cmdline, distro, quirks) plus a pre-wrapped `Reason:` block for tier 4/5/6. Long error strings pre-wrapped via `textwrap` to work around ratatui/ratatui#2342 (Paragraph wrap+scroll accounting bug). Focus border brightens on the active pane, dims on the inactive pane. Context-sensitive footer legend filters by current screen + pane + filter-editing state.

**Programmatic docs** — new `tiers-docgen` binary in rescue-tui renders the tier table and keybinding reference as Markdown from the canonical in-code sources (`TrustVerdict` enum + `KEYBINDINGS` registry). Marker-pair rewriting in `docs/HOW_IT_WORKS.md`, `docs/TOUR.md`, and `crates/rescue-tui/README.md`. CI's new `tiers-drift` job enforces drift-freedom (same pattern as `constants-drift` / `cli-drift` / `manifest-schema-drift`).

**ANSI preview** — dev-only `tui-screenshots` binary renders 10 curated fixtures (empty list, all 6 tiers, focus states, filter editing, help overlay, confirm screen, trust challenge) to ANSI-escaped stdout. `cat docs/screenshots/rescue-tui-preview.ansi` in a color terminal for visual review without a build+boot cycle.

**API changes** — `iso_probe::discover()` now returns `DiscoveryReport { isos: Vec<DiscoveredIso>, failed: Vec<FailedIso> }`. `ProbeError::NoIsosFound` is now reserved for "zero `.iso` files found on disk" — a stick full of broken ISOs returns `Ok(report)` with populated `failed` so rescue-tui can render tier-4 rows. Paired iso-parser API `scan_directory_with_failures` adds the `ScanReport` shape; legacy `scan_directory` stays as a thin wrapper for backwards compat.

PRs landed: [#463](https://github.com/aegis-boot/aegis-boot/pull/463) (design doc), [#464](https://github.com/aegis-boot/aegis-boot/pull/464) (DiscoveryReport API), [#471](https://github.com/aegis-boot/aegis-boot/pull/471) (6 tiers), [#472](https://github.com/aegis-boot/aegis-boot/pull/472) (dual-pane scaffold), [#473](https://github.com/aegis-boot/aegis-boot/pull/473) (info-pane content + tier-4 rows), [#474](https://github.com/aegis-boot/aegis-boot/pull/474) (keybinding registry), [#475](https://github.com/aegis-boot/aegis-boot/pull/475) (render coverage suite — 19 tests), [#476](https://github.com/aegis-boot/aegis-boot/pull/476) (tiers-docgen + CI drift check), [#477](https://github.com/aegis-boot/aegis-boot/pull/477) (ANSI preview tool).

### `aegis-boot add --scan` — retroactive sidecar generation (#479)

Complement to the epic: operators who drag-and-dropped ISOs onto `AEGIS_ISOS` from their host OS can now upgrade those tier-2 (BareUnverified) entries to tier-1 (OperatorAttested) with one command:

```bash
sudo aegis-boot add --scan /dev/sda2
sudo aegis-boot add --scan /mnt/aegis-isos
sudo aegis-boot add --scan                  # auto-detect AEGIS_ISOS
```

Walks the mount via `scan_isos` (#274 Phase 6a), classifies each `.iso` by sidecar state, streams sha256, writes coreutils-compatible `<hex>  <filename>\n` sidecars atomically (tempfile-in-same-dir + persist). Never overwrites an existing sidecar — a hash mismatch is surfaced as a **tamper signal** instead of auto-corrected. Direct-write-first, `sudo cp` fallback only on `PermissionDenied` / `ReadOnlyFilesystem` so unit tests and already-root flows stay prompt-free. Per-ISO attestation entries. Minisig generation is out of scope (would require the operator's private signing key). New `pub fn iso_probe::compute_iso_sha256(path)` exposes the streaming hasher.

PR: [#480](https://github.com/aegis-boot/aegis-boot/pull/480). 18 new tests, 512 total in aegis-bootctl.

### ⚠️ Breaking change for external schema consumers

### ⚠️ Breaking change for external schema consumers

`aegis-wire-formats`'s schemars dep bumped 0.8 → 1.2 ([#414](https://github.com/aegis-boot/aegis-boot/pull/414)). The 13 JSON Schema files under `docs/reference/schemas/` now declare `"$schema": "https://json-schema.org/draft/2020-12/schema"` (previously `draft-07`). Draft-07 validators will need to accept both drafts or upgrade. Library consumers using `aegis-wire-formats` without the `schema` feature are unaffected; schemars stays `optional = true`.

### Cross-reboot last-booted persistence — ADR 0003 + #375 all phases

Closed the `/run/aegis-boot` tmpfs scope mismatch caught during #132 validation. `rescue-tui::persistence` now writes a stripped `last-choice.json` to `AEGIS_ISOS/.aegis-state/` using atomic rename-over + directory fsync, so cursor position survives full reboots. Within-session `cmdline_override` (failed-kexec retry) remains tmpfs-only per the two-tier design.

- ADR 0003 [`LAST_BOOTED_PERSISTENCE.md`](./docs/architecture/LAST_BOOTED_PERSISTENCE.md) accepted at 83.3% supermajority.
- Phase 1 implementation ([#402](https://github.com/aegis-boot/aegis-boot/pull/402)).
- Phase 2/3 reboot-simulation round-trip test + hardware-procedure doc ([#403](https://github.com/aegis-boot/aegis-boot/pull/403)).
- `reboot_simulation_round_trip` + `within_session_load_prefers_tmpfs_with_cmdline_override` tests lock the behavior.
- Hardware leg (physical Framework/Dell/ThinkPad flash → boot → pick → power-cycle → boot → verify cursor) documented in `docs/validation/REAL_HARDWARE_REPORT_132.md`; execution tracked under the multi-vendor gate.

### NixOS install path — first-class flake package (#406)

`flake.nix` now exposes `packages.aegis-bootctl` (via `rustPlatform.buildRustPackage`), `apps.default`, and `nixosModules.aegis-boot`. Runtime deps (sgdisk, mkfs.fat, mkfs.exfat, mcopy, curl, gnupg, coreutils) are baked into `$PATH` via `makeWrapper` so there's nothing extra to install:

```bash
nix run github:aegis-boot/aegis-boot -- flash /dev/sdX --yes
nix profile install github:aegis-boot/aegis-boot
```

Plus a `nixosModules.aegis-boot` for declarative system-flake import. CI's nix-smoke job now also builds the derivation on every PR ([#407](https://github.com/aegis-boot/aegis-boot/pull/407)).

### MSRV bump — Rust 1.85 → 1.88 (#258 cleared)

Driven by `time 0.3.47` in the ratatui 0.30 tree, not a language-feature need. Rust 1.88 is ~10 months old; the bump is accepting an upstream-forced move. Touches 21 files (workspace Cargo.toml, 11 CI workflows, Dockerfile.locked, BUILDING.md, README.md, ARCHITECTURE.md, INSTALL.md, flake.nix channel pin nixos-25.05 → nixos-25.11, scripts/check-doc-version.sh, iso-parser/fuzz/Cargo.toml). Closes the low-severity lru `IterMut` dependabot advisory ([#408](https://github.com/aegis-boot/aegis-boot/pull/408)).

### ratatui 0.29 → 0.30 (#258)

Backend trait `<B as Backend>::Error` lost its default `'static` bound. Added `where <B as Backend>::Error: 'static` to `rescue-tui::event_loop` — the only generic Backend call site. No visual behavior change.

### Major-version dep bump backlog cleared — #411 all 4 items

- **`indicatif` 0.17 → 0.18** ([#412](https://github.com/aegis-boot/aegis-boot/pull/412)) — progress bar on `aegis-boot flash`, API stable for our usage.
- **`toml` 0.8 → 1.1** ([#413](https://github.com/aegis-boot/aegis-boot/pull/413)) — parser/writer/datetime crate split; iso-probe sidecar API unaffected.
- **`schemars` 0.8 → 1.2** ([#414](https://github.com/aegis-boot/aegis-boot/pull/414)) — see breaking note above.
- **`sha2` 0.10 → 0.11** ([#415](https://github.com/aegis-boot/aegis-boot/pull/415)) — trait package restructure (crypto-common 0.1 → 0.2, digest 0.10 → 0.11); `Digest::new/update/finalize` identical. SHA-256 is byte-stable by algorithm spec, so on-disk hashes are unaffected.

### Dep refresh (compat-level)

- `tokio` 1.51 → 1.52 (dev-dep, iso-parser).
- `crossterm` 0.28 → 0.29 on rescue-tui's direct pin — ratatui 0.30 already pulls 0.29 transitively, de-duplicates.
- `python311` → `python312` in flake devShell (nixos-25.11 alignment).

### Tooling + CI

- **SAST sweep cleared 18 `unsafe-usage` findings** ([#405](https://github.com/aegis-boot/aegis-boot/pull/405)) — edition-2024 `std::env::set_var` in `ENV_MUTEX`-guarded `#[cfg(test)]` blocks, now documented with SAFETY + `nosemgrep` annotations.
- **Rust stable `collapsible_if` lint fix** — 3 sites in `iso-parser/src/lib.rs` using let-chains (stable since 1.88).
- **Actions hygiene:** `cachix/install-nix-action` v27 → v31; `sign-identity-transition.yml` checkout@v4 → v5 for consistency.
- **Node 20 deprecation** tracked in [#409](https://github.com/aegis-boot/aegis-boot/issues/409) — GitHub auto-forces Node 24 on 2026-06-02. No action needed until then; plan to opt-in test early May 2026 via `FORCE_JAVASCRIPT_ACTIONS_TO_NODE24=true`.

### #181 Phase 2b — in-place update executor + CLI wiring shipped end-to-end

`aegis-boot update <device> --apply --experimental-apply` now **actually rotates the signed chain** on an eligible stick (was planner-only dry-print before). Verified end-to-end on real USB hardware: flashed stick → injected drift → rotated → confirmed restored bits + `.bak` preservation + AEGIS_ISOS byte-for-byte unchanged.

- **[#425](https://github.com/aegis-boot/aegis-boot/pull/425)** — mtools `mren` + `mdel` argv builders. 4 unit tests cover argv shape + `--` injection so paths starting with `-` can't be misread as flags.
- **[#426](https://github.com/aegis-boot/aegis-boot/pull/426)** — `RotationSources` + `execute_rotation` + `execute_rollback`. State machine: mcopy src → `.new`, mark Staged; mren original → `.bak`; mren `.new` → original; mark Rotated. Rollback walker executes `rollback_plan` actions best-effort.
- **[#429](https://github.com/aegis-boot/aegis-boot/pull/429)** — direct-install now also writes the host-side attestation manifest so `aegis-boot update`'s eligibility check can find direct-install-flashed sticks (was blocking Phase 2b end-to-end).
- **[#431](https://github.com/aegis-boot/aegis-boot/pull/431)** — `resolve_host_chain` reports the hash of the *combined* initrd (distro + aegis initramfs) instead of the distro initrd alone. Closes the false-positive-CHANGED-on-`/initrd.img` diff (#430) that would have caused the executor to destroy the combined form.
- **[#434](https://github.com/aegis-boot/aegis-boot/pull/434)** — CLI wiring (`apply_rotation`) materializes host-side sources into tempfiles (combined initrd via `combine_initrd`, grub.cfg via `render_grub_cfg`), hands them to `execute_rotation`. Fixed an `mtools`-path-prefix bug found during real-hardware testing (executor was passing bare `/vmlinuz` where mtools wants `::/vmlinuz`; mcopy was silently falling back to the host filesystem).
- **[#437](https://github.com/aegis-boot/aegis-boot/pull/437)** — post-rotate sha256 readback verification. After `mren <esp_path>.new → <esp_path>` completes, re-hash the rotated file and assert it matches the planner's `fresh_sha256`. Closes a silent-corruption gap (mtools / exFAT / flash hiccup could have left bytes that don't match what was signed).
- **[#438](https://github.com/aegis-boot/aegis-boot/pull/438)** — grub.cfg diff no longer UNKNOWN. `resolve_rendered_grub_cfg` materializes grub.cfg via `render_grub_cfg` into a tempfile + hashes it, same pattern #431 used for initrd. All 6 canonical ESP slots now resolve on Linux; rotation executor can rotate grub.cfg if operator customizes it.
- **[#439](https://github.com/aegis-boot/aegis-boot/pull/439)** — `aegis-boot update --rollback` CLI verb. Restores `.bak` files from a prior `--apply` run via the existing `execute_rollback` state machine. Independent from `--experimental-apply` gating (rollback is a recovery verb, not a new write). Real-hardware verified: apply → rollback → verify round-trip works; no-.bak case prints operator-friendly guidance.

All Linux-only per `cfg(target_os = "linux")` — `direct_install` is Linux-specific; cross-platform rotation ships under #367 Phase D (see Win11 findings below).

### New command — `aegis-boot verify --stick` ([#432](https://github.com/aegis-boot/aegis-boot/issues/432) / [#436](https://github.com/aegis-boot/aegis-boot/pull/436))

Ships the Option-B integrity check from the #430 design discussion: "has my stick silently diverged from the bytes we signed into its attestation manifest?" Complements `update`'s freshness semantic ("does my stick need new bits?").

For each `EspFileEntry` in the host-side attestation manifest, reads the stick's copy via `mtype`, compares to the recorded hash, reports per-file verdict: `OK` / `DRIFT` / `UNREADABLE`.

Exit codes: 0 all match, 1 any drift or unreadable, 2 resolver error. JSON mode (`--json`) emits structured output with summary + per-file verdicts. Covers grub.cfg (which update's diff previously couldn't reach before #438).

### #181 Phase 4 — operator UX polish

- **[#441](https://github.com/aegis-boot/aegis-boot/pull/441)** — attestation manifest auto-refreshes after `update --apply`. On success, reads the host-side manifest, updates `esp_files[]` entries for rotated slots with their new sha256 + size, bumps `sequence`, normalizes `tool_version`, writes back to both the host-side attestations dir AND mcopies over `::/aegis-boot-manifest.json` on the ESP. Without this, `verify --stick` would always show drift after every rotation — closed the last consistency gap between update + verify.
- **[#442](https://github.com/aegis-boot/aegis-boot/pull/442)** — `aegis-boot doctor --stick` surfaces `manifest sequence` + `tool_version` as a new stick-check row. Operators can see "has this stick been updated and with what version" at a glance.

### Real-hardware Phase-2b validation

Session on 2026-04-22/23 against the local SanDisk Cruzer 29.8 GB + Framework Laptop:
- Corrupted first 16 bytes of `/vmlinuz` on the stick
- `aegis-boot update /dev/sda` correctly reported `CHANGED /vmlinuz`
- `aegis-boot update /dev/sda --apply --experimental-apply` → `Rotation complete: 1 file(s) rotated in place`
- Post-rotation sha256: `/vmlinuz` = correct `39cc3d97…`, `/vmlinuz.BAK` = drifted `bc65565d…`
- Re-run of update: `Summary: 0 would change, 4 unchanged, 2 inconclusive`
- AEGIS_ISOS partition bytes identical before and after

### Windows 11 cross-platform prototyping ([#419](https://github.com/aegis-boot/aegis-boot/issues/419))

Spun up a libvirt Win11 VM with WinRM + scratch qcow2 and prototyped the Windows direct-install path against `\\.\PhysicalDrive1`. Key findings filed on #419:

1. **Windows `Initialize-Disk -PartitionStyle GPT` auto-inserts a Microsoft Reserved Partition.** Linux's `sgdisk` doesn't. For byte-parity between Linux- and Windows-flashed sticks, use `diskpart` (explicit partition layout, no implicit MSR) rather than `Initialize-Disk` + `New-Partition`.
2. **`New-Partition` / `Format-Volume` (PowerShell cmdlets) work end-to-end** — ESP type GUID recognized, exFAT native on Win11, no drive-letter auto-assign for ESP. Modern-PS path is a viable Phase-2 option for formatting.
3. **Raw write to `\\.\PhysicalDrive1` via `[System.IO.FileStream]` works** — wrote 4096 bytes + readback-verified under exclusive-share lock. Maps cleanly to `windows-rs` `CreateFileW` + `WriteFile` for the Rust implementation.
4. **Still unexercised:** UAC split-token elevation detection, BitLocker lock failure semantics, `FILE_FLAG_NO_BUFFERING` direct-I/O (`[System.IO.FileStream]` defaults to buffered — production Rust path wants `CreateFileW` direct).

Together these findings scope #419 into clean phases (diskpart-for-partition, Format-Volume-for-fs, windows-rs-for-raw-write, elevation+BitLocker detection). Implementation is a follow-up session.

### Real-hardware validation — ADR 0003 load + save paths

Validated end-to-end on real USB hardware (SanDisk Cruzer 29.8 GB / Framework Laptop / kernel 6.14.0-37 / OVMF SecBoot enforcing) with QEMU USB passthrough of `/dev/sda`.

- **Load path** — [#423](https://github.com/aegis-boot/aegis-boot/pull/423): seeded `.aegis-state/last-choice.json` on real exFAT, booted under SecBoot enforcing, observed `rescue-tui: restored last choice idx=0 iso=...` confirming the stripped cross-reboot form survives VM restart + is applied on startup.
- **Save path under duress** — [#423](https://github.com/aegis-boot/aegis-boot/pull/423) addendum #2: 10/10 kill-mid-save runs passed with 0 JSON corruption across random kill timings in 10–500ms. Baseline throughput: 7.35 ms per save (write `.tmp` + rename over + dir fsync). Stale `.tmp` leftovers are benign per atomic-rename semantics.
- **Harness shipped** — [#424](https://github.com/aegis-boot/aegis-boot/pull/424): `scripts/validation/` directory with `save_smoke.rs`, `boot-ovmf.sh`, `kill-mid-save.sh`, and a runbook README so other maintainers / future sessions can reproduce on their own hardware.

Residual gap for multi-vendor closure: mid-kexec physical power-pull on Framework / Dell / ThinkPad. SIGKILL ≠ real power cut; the flash-barrier question is device-specific.

### SysRq help-overlay: full REISUB sequence ([#422](https://github.com/aegis-boot/aegis-boot/pull/422))

The rescue-tui `?` help overlay listed three SysRq bindings (b/s/e). Expanded to the full REISUB sequence (R/E/I/S/U/B) so operators hitting the keys in wrong order don't defeat the "safe forced reboot" property. Panel grew 70×20 → 80×32 to fit without wrap-clipping. `kernel.sysrq=1` was already enabled by `scripts/build-initramfs.sh` so the keys work at runtime.

### Close-outs

- Epic **#51** (crates.io publishing) closed — 6 crates live (`iso-parser`, `iso-probe`, `kexec-loader`, `aegis-wire-formats`, `aegis-fitness`, `aegis-bootctl`) at v0.16.0 via Trusted Publishing (OIDC, no long-lived tokens).
- Successor **#375** closed — all software-side acceptance met; hardware leg rolls up into multi-vendor gate.
- Tech-debt **#258** closed via #408.
- Tech-debt **#404** (SAST semgrep) closed via #405.
- Tech-debt **#411** closed via #412/#413/#414/#415.

## [0.16.0] — 2026-04-22

First release under the dedicated **`aegis-boot` GitHub org**. Direct-install pipeline shipped end-to-end with signed attestation manifests, operator UX gained three one-command workflows (`quickstart`, `init`, catalog-slug `add`), and the supply chain grew a second signed trust anchor (minisign, ADR 0002 ACCEPTED) to sit alongside the cosign-keyless release signing. macOS arm64 release binaries ship per-tag via a new Homebrew bottle and a `release.yml` that builds the darwin target natively. The org move closes the `aegis-boot/aegis-*` naming story, pre-registers all library crates on crates.io with team-based ownership, and replaces the long-lived `CARGO_REGISTRY_TOKEN` with Trusted Publishing (short-lived OIDC tokens per-release).

### Direct-install pipeline lands — v1.0.0 blocker #274 clears (epic [#274](https://github.com/aegis-boot/aegis-boot/issues/274))

`aegis-boot flash --direct-install` replaces the legacy `mkusb.sh + dd` path with a Rust-native pipeline: zap + GPT partition via `sgdisk`, ESP `mkfs.fat`, AEGIS_ISOS `mkfs.exfat` (default) / `fat32` / `ext4`, render grub.cfg, concat initrd, stage the signed chain via `mmd + mcopy`, and — new in this release — write a signed attestation manifest to `::/aegis-boot-manifest.json` on the freshly-staged ESP. Byte-parity against `mkusb.sh` is enforced in CI (`.github/workflows/direct-install-e2e.yml`): each of the 6 canonical ESP files is sha256-compared and the workflow fails on any divergence.

- **Phase 3 — wire `--direct-install`** ([#350](https://github.com/aegis-boot/aegis-boot/pull/350)). First end-to-end run behind the flag; pre-flight deps gate + per-stage wall-clock timers landed in companion PRs ([#353](https://github.com/aegis-boot/aegis-boot/pull/353), [#354](https://github.com/aegis-boot/aegis-boot/pull/354)).
- **Phase 3b — signed attestation manifest write** ([#386](https://github.com/aegis-boot/aegis-boot/pull/386), closes [#349](https://github.com/aegis-boot/aegis-boot/issues/349)). Stage 7 of the flash pipeline reads device identity (sgdisk + blkid), computes per-file ESP sha256s, builds a `schema_version=1` `Manifest` (`aegis_wire_formats::Manifest`), serializes, and optionally minisign-signs it if `AEGIS_BOOT_SIGNING_KEY` is set. Unsigned operator-default per ADR 0002 §6.3; the maintainer-signed path ships the full cosign-verifiable claim.
- **Phase 6 — subfolder support** ([#351](https://github.com/aegis-boot/aegis-boot/pull/351), [#381](https://github.com/aegis-boot/aegis-boot/pull/381), [#382](https://github.com/aegis-boot/aegis-boot/pull/382), [#383](https://github.com/aegis-boot/aegis-boot/pull/383)). `aegis-boot list` recurses AEGIS_ISOS; `add --folder NAME` places an ISO under a named subfolder; `doctor --stick` prints per-ISO trust-state rows; rescue-tui surfaces the subfolder prefix in the boot menu.

### Operator UX — three one-command paths (closes [#352](https://github.com/aegis-boot/aegis-boot/issues/352))

The 5-command `doctor → init → fetch → add → boot` walkthrough still works, but two new capstones cover the common cases:

- **`aegis-boot quickstart <device>`** ([#358](https://github.com/aegis-boot/aegis-boot/pull/358)) — sub-10-minute empty-stick-to-booted-rescue with Alpine 3.20 Standard. Thin wrapper over `init --profile minimal --yes --direct-install`.
- **`aegis-boot init`** — one-command rescue stick with the `panic-room` profile (Alpine + Ubuntu + Rocky) or other curated profiles.
- **`aegis-boot add <catalog-slug>`** ([#356](https://github.com/aegis-boot/aegis-boot/pull/356)) — operators no longer have to `fetch` then `add`; one verb both fetches (cosign-verifying) and adds to the stick.

rescue-tui's empty-state pointer ([#357](https://github.com/aegis-boot/aegis-boot/pull/357)) surfaces the catalog-slug shortcut directly in the menu when the stick has no ISOs.

### In-place update groundwork — #181 Phase 1 + Phase 2a

`aegis-boot update <device>` goes from not-a-command to two real phases:

- **Phase 1 — eligibility preflight** ([#380](https://github.com/aegis-boot/aegis-boot/pull/380)). Read-only: validates GPT + ESP + AEGIS_ISOS label + attestation-GUID match, prints the per-file sha256 diff between stick and a fresh flash, reports ELIGIBLE / INELIGIBLE with a specific reason. Zero writes.
- **Phase 2a — rotation planner + `--experimental-apply` gate** ([#388](https://github.com/aegis-boot/aegis-boot/pull/388)). Pure-function planner + rollback analyzer in `update_apply.rs`. The CLI flag `--apply --experimental-apply` dry-prints the plan the Phase 2b executor would run. Still no writes — FAT32 has no journal; Phase 2b will ship the executor with bounded rollback on per-file verify failure.

### macOS arm64 ships — #365 Phase A1 + A3

First cross-platform release binary lands. Native aarch64-apple-darwin build via GitHub Actions macos-14 runner ([#371](https://github.com/aegis-boot/aegis-boot/pull/371)). Homebrew bottle at `aegis-boot/aegis-boot` tap ([#372](https://github.com/aegis-boot/aegis-boot/pull/372)) sidesteps Gatekeeper entirely. Binary is ad-hoc codesigned but not notarized — notarization is demand-signal-gated via [#369](https://github.com/aegis-boot/aegis-boot/issues/369).

### GitHub org migration — williamzujkowski/aegis-* → aegis-boot/ (closes [#365](https://github.com/aegis-boot/aegis-boot/issues/365) migration phase)

Both repositories transferred under a dedicated `aegis-boot` GitHub organization. Old URLs 301-redirect (GitHub preserves for ≥1 year), but all 179 in-repo references + 129 CHANGELOG issue links swept to the new org path ([#394](https://github.com/aegis-boot/aegis-boot/pull/394)). The cosign keyless identity used by `release.yml` changes from `williamzujkowski/aegis-boot/...` to `aegis-boot/aegis-boot/...` — old-identity release signatures remain valid (hash-bound, not location-bound), and the rev-3 ADR 0002 identity-transition bridge (`.github/identity-transition.json` signed under the legacy identity pre-transfer) lets automated verifiers chain cryptographically.

### Crates.io — 8 placeholders + team ownership + Trusted Publishing (ADR 0002)

All 8 workspace + sibling crate names pre-registered as v0.0.0 placeholders to close the post-transfer squatter race: `aegis-boot`, `aegis-bootctl`, `aegis-wire-formats`, `aegis-fitness`, `aegis-hwsim`, `iso-parser`, `iso-probe`, `kexec-loader`. Each is co-owned by `williamzujkowski` + `github:aegis-boot:aegis-boot-admins`, so anyone in the admins team can publish.

**Trusted Publishing** ([#395](https://github.com/aegis-boot/aegis-boot/pull/395)) replaces the long-lived `CARGO_REGISTRY_TOKEN`. The new `.github/workflows/crates-publish.yml` is tag-triggered, gated on the `release` GitHub environment (required-reviewer + `v*`-tag deployment policy), and uses `rust-lang/crates-io-auth-action@v1` to mint a ~30-minute OIDC token per release. No registry secret sits in GitHub org secrets, `pass`, or anywhere else.

**Package rename**: `crates/aegis-cli/` publishes as `aegis-bootctl` on crates.io because the `aegis-cli` name was claimed by an unrelated Aegis Authenticator TOTP tool (v1.3.95). Directory path stays `crates/aegis-cli/` to preserve git history; the binary name is `aegis-boot` (via `[[bin]] name = "aegis-boot"`, independent of package name), so operators see zero change. Tracked in [#392](https://github.com/aegis-boot/aegis-boot/issues/392), landed via [#393](https://github.com/aegis-boot/aegis-boot/pull/393).

### ADRs accepted

- **0002 — Key Management** ([`docs/architecture/KEY_MANAGEMENT.md`](./docs/architecture/KEY_MANAGEMENT.md)). rev 3, **ACCEPTED at 83.3% supermajority** on [#366](https://github.com/aegis-boot/aegis-boot/issues/366). Ships minisign (Ed25519) as the operator-facing trust anchor for runtime-emitted signatures (attestation manifests + future #367 bundle manifests); keeps cosign keyless for release-artifact signing. One active key + historical-anchors list for forever-valid old-manifest verification. Monotonic **Key Epoch counter** with a binary-embedded `MIN_REQUIRED_EPOCH` floor closes the post-compromise rollback window on fresh installs. Quarterly rotation rehearsals keep the runbook exercised.
- **0003 — Cross-reboot last-booted persistence** ([`docs/architecture/LAST_BOOTED_PERSISTENCE.md`](./docs/architecture/LAST_BOOTED_PERSISTENCE.md)) ([#379](https://github.com/aegis-boot/aegis-boot/pull/379)). PROPOSED. Closes the #132 spec-mismatch: rescue-tui's cursor should persist across reboots, but the shipped module writes to tmpfs. ADR scopes a two-stage `tmpfs → AEGIS_ISOS` migration.

### Bug-report bundler — `aegis-boot bug-report` (closes [#342](https://github.com/aegis-boot/aegis-boot/issues/342))

Three-phase incremental ship of an operator-facing bug-reporting workflow:

- **Phase 1** ([#344](https://github.com/aegis-boot/aegis-boot/pull/344)) — workstation bundler subcommand collects a redacted host-state archive.
- **Phase 2** ([#345](https://github.com/aegis-boot/aegis-boot/pull/345)) — rescue-tui writes Tier-A failure microreports onto the stick when a kexec fails.
- **Phase 3a** ([#346](https://github.com/aegis-boot/aegis-boot/pull/346)) — `bug-report --include-stick` pulls those microreports back; exFAT filename fix.

### CI + supply chain

- **SPDX per-file license headers** ([#361](https://github.com/aegis-boot/aegis-boot/pull/361)). Every Rust + shell + YAML source file carries a machine-readable license tag.
- **cargo-deny license allowlist** ([#362](https://github.com/aegis-boot/aegis-boot/pull/362)). CI fails any PR that introduces a transitive dep under a license not on the allowlist.
- **NOTICE file** ([#363](https://github.com/aegis-boot/aegis-boot/pull/363)) — upstream-component attribution for the signed Secure Boot chain (shim, grub, distro kernel).
- **miri UB detection on `kexec-loader`** ([#378](https://github.com/aegis-boot/aegis-boot/pull/378), closes [#364](https://github.com/aegis-boot/aegis-boot/issues/364)). Path-gated `.github/workflows/miri-kexec-loader.yml` runs `cargo +nightly miri test -p kexec-loader` whenever anything under `crates/kexec-loader/` changes. Covers stacked-borrows aliasing + lifetime bugs the normal compiler doesn't catch.
- **crates.io publish-readiness dry-run workflow** ([#355](https://github.com/aegis-boot/aegis-boot/pull/355)). `cargo publish --dry-run` per library crate on every PR — catches metadata regressions (missing `version =` on path deps, bad category names, oversize `description`) before publish day.

### Documentation + governance

- **Doc-evergreen sweep** ([#384](https://github.com/aegis-boot/aegis-boot/pull/384)) — 14 files refreshed, `<!-- constants:BEGIN:NAME --><!-- constants:END:NAME -->` markers source live values from `crates/aegis-cli/src/constants.rs`, stale issue references re-pointed, dead flag references dropped.
- **OPSEC scrub** ([#390](https://github.com/aegis-boot/aegis-boot/pull/390)) — public-facing docs no longer reference maintainer employment; technical constraints (no D-U-N-S, Individual Apple Developer enrollment tier) preserved.
- **Quickstart README promotion** ([#359](https://github.com/aegis-boot/aegis-boot/pull/359)) — `aegis-boot quickstart` now the headlined fastest install.

### Fixed

- **Initramfs exfat modprobe** ([#373](https://github.com/aegis-boot/aegis-boot/pull/373), closes [#132](https://github.com/aegis-boot/aegis-boot/issues/132) external-user report) — rescue kernel now loads `exfat` + `nls_cp437` + `nls_iso8859-1` at init, so sticks formatted with the `exfatprogs` default (#243) actually mount on boot.
- **`init --direct-install` arg forwarding** ([#376](https://github.com/aegis-boot/aegis-boot/pull/376)) — `quickstart` passes `--direct-install` through to `init` cleanly; previously the flag was silently dropped.

### Not yet shipped (deferred to next milestones)

- **Windows native release binary** — Phase B of [#365](https://github.com/aegis-boot/aegis-boot/issues/365). Drive enumeration + `Get-Disk` logic landed in earlier releases; raw-disk writing + code signing are the gates. Winget manifest scaffold ships in this release ([#370](https://github.com/aegis-boot/aegis-boot/pull/370)) but the real publish is Phase B.
- **#181 Phase 2b (destructive executor)** — rotation planner ships; the executor that actually writes to the ESP is Phase 2b, gated on OVMF E2E validation of the full backup → stage → verify → rotate → rollback cycle.
- **#367 Phase D (cross-platform bundle trust anchor)** — unblocked by ADR 0002 acceptance; implementation tracks post-v0.16.0.
- **Real-hardware multi-vendor shakedown** — [#51](https://github.com/aegis-boot/aegis-boot/issues/51) v1.0.0 gate. QEMU + OVMF passes on every release; Framework / ThinkPad / Dell direct-boot reports are the v1.0.0 threshold.

## [0.15.0] — 2026-04-20

Doc-automation milestone release. Closes epic [#286](https://github.com/aegis-boot/aegis-boot/issues/286) (7-phase auto-generation + drift-checks for every user-facing doc) and the operator-UX sweep umbrella [#310](https://github.com/aegis-boot/aegis-boot/issues/310). 12 committed JSON Schemas for every `aegis-boot --json` surface. First community hardware-compat submission surfaced four bugs, all fixed. New local cross-distro test harness. CI grew from 17 → 22 drift-checks.

### Doc-automation + evergreen-numbers strategy (closes epic [#286](https://github.com/aegis-boot/aegis-boot/issues/286))

The v0.14.0 release incident — four docs stuck at v0.13.0 after a one-line version bump — motivated a full doc-automation sweep. Seven phases shipped; every user-facing doc that mirrors a code value is now generated, drift-checked, or both. CI grew from 17 to **22 checks** (+doc-version-drift, +lychee link-check, +doc constants drift, +CLI subcommand drift, +manifest JSON schema drift). Every auto-generated doc is gated on every PR.

- **Phase 1 — workspace version single-source + build.rs-templated man page** (#287 closed by [#294](https://github.com/aegis-boot/aegis-boot/pull/294) + [#295](https://github.com/aegis-boot/aegis-boot/pull/295)). One line in `Cargo.toml [workspace.package]` now drives every crate's version + the man page's `.TH` header + the CI drift-check's allowlist. The v0.14.0 class of bug (4 docs at old version) is structurally preventable.
- **Phase 2 — shared constants registry** (#288 closed by [#299](https://github.com/aegis-boot/aegis-boot/pull/299)). `ESP_SIZE_MB`, `DEFAULT_READBACK_BYTES`, `MAX_MANIFEST_BYTES`, `GRUB_TIMEOUT_SECS` now single-sourced in `crates/aegis-cli/src/constants.rs`; docs reference them via HTML-marker injection with a CI `--check` drift guard. Adding a new shared constant is a one-line edit.
- **Phase 3 — CLI reference auto-generation** (#289 closed by [#300](https://github.com/aegis-boot/aegis-boot/pull/300) + [#301](https://github.com/aegis-boot/aegis-boot/pull/301)). New `cli-docgen` tool validates every subcommand in the dispatch table has a section in `docs/CLI.md` and a `.TP` entry in the man page; then renders `docs/reference/CLI_SYNOPSIS.md` from the live `aegis-boot <sub> --help` output for every subcommand. First run of the checker found 4 real doc gaps (fetch-image, completions, man, tour).
- **Phase 4 — JSON schema registry via schemars** (#290 closed by 10 PRs, [#303](https://github.com/aegis-boot/aegis-boot/pull/303)..[#319](https://github.com/aegis-boot/aegis-boot/pull/319)). Every one of the 8 `--json` surfaces in `aegis-boot` now emits via a typed serde envelope from the new `aegis-wire-formats` crate (renamed from `aegis-manifest` after stabilization via [#323](https://github.com/aegis-boot/aegis-boot/pull/323)); 12 committed JSON Schemas under `docs/reference/schemas/` let third-party verifiers pin against wire contracts with CI drift-gating. Patterns established: untagged enums for mutually-exclusive shapes (attest-list, recommend, compat), internally-tagged enums for variant-typed rows (verify verdict, update eligibility), flatten + enum for shared-prefix-plus-variant-suffix envelopes.
- **Phase 5 — lychee markdown link-checker** (#291 closed by [#298](https://github.com/aegis-boot/aegis-boot/pull/298)). Scheduled (weekly) + PR-triggered lychee run catches link rot before it reaches operators. First run surfaced 4 real broken links fixed in-PR.
- **Phase 6 — rustdoc-as-API-landing-page** (#292 closed by [#297](https://github.com/aegis-boot/aegis-boot/pull/297)). Library-crate READMEs (iso-parser, iso-probe, kexec-loader) are now included via `#[doc = include_str!("../README.md")]` at lib.rs root — docs.rs renders the README as the crate landing page, zero drift between README prose and rustdoc's front door. Phase 6's first run surfaced a drifted API snippet in iso-parser's README, fixed inline and tracked via [#296](https://github.com/aegis-boot/aegis-boot/issues/296).
- **Phase 7 — git-cliff CHANGELOG draft-assist** (#293 closed by [#302](https://github.com/aegis-boot/aegis-boot/pull/302)). `scripts/draft-release-notes.sh` wraps `git-cliff` with tuned commit-category mapping + `(#NNN)` squash-merge stripping. Advisory-only — maintainer still curates. This section's first draft came from the new script.

### Operator-UX sweep (closes umbrella [#310](https://github.com/aegis-boot/aegis-boot/issues/310))

A periodic UX / operator-experience assessment surfaced three findings, each ratified via `consensus_vote` with `higher_order` strategy (80% approve) and shipped in the agreed sequence `#313 → #312 → #311`:

- **`install.sh` pre-flights mkusb.sh dependencies** (#313 closed by [#314](https://github.com/aegis-boot/aegis-boot/pull/314)). `aegis-boot doctor` now checks `mcopy` / `mkfs.vfat` / `mkfs.exfat` / `sgdisk` / `dd` / `lsblk` / `curl` / `sha256sum` / `gpg` / `cosign`. After installing the binary, `install.sh` runs `doctor` and surfaces any missing deps with a multi-distro install one-liner (apt-get / dnf / pacman). Closes the class of bug that [#282](https://github.com/aegis-boot/aegis-boot/issues/282) fell into — first-time operator on a minimal distro hits a late opaque failure from `mkusb.sh`.
- **rescue-tui empty-state footer names the rescue-shell keybinding** (#312 closed by [#321](https://github.com/aegis-boot/aegis-boot/pull/321)). Empty-state previously told operators to "select the rescue shell entry below (if enabled)" — but `draw_empty_list` replaces the list entirely so there was no "below." Now: bolded footer reading `Press Enter for rescue shell · q to reboot · ? for keybindings.`
- **`aegis-boot fetch` shows download progress** (#311 closed by [#322](https://github.com/aegis-boot/aegis-boot/pull/322)). Uses curl's `--progress-bar` on the ISO download by default when stdout is a TTY. `--no-progress` and `--progress` flags override the auto-detect for scripted usage / non-TTY CI logs. 5-minute silent stretches are gone.

### Cross-cutting cleanup

- **JSON escaping standardized** (#306 closed by [#320](https://github.com/aegis-boot/aegis-boot/pull/320)). The hand-rolled `doctor::json_escape` helper and its 4 unit tests retired; every `--json` emitter in aegis-cli now routes through `serde_json`. Generic pre-dispatch error envelope `aegis_wire_formats::CliError` covers the 5 remaining hand-rolled error-path sites in list / verify / attest.

### External user reports (openSUSE Tumbleweed + ASRock Z690M-ITX/ax)

First community hardware-compat submission ([#328](https://github.com/aegis-boot/aegis-boot/issues/328), @garyoakidoki — ASRock Z690M-ITX/ax + AMI BIOS 20.01 + Secure Boot enforcing, full flash → boot → kexec chain verified). The single report surfaced four distinct bugs, all fixed and merged:

- **`doctor`'s `sgdisk`/`lsblk`/etc. false-FAIL when `$PATH` drops `/usr/sbin`** ([#328](https://github.com/aegis-boot/aegis-boot/issues/328) closed by [#331](https://github.com/aegis-boot/aegis-boot/pull/331)). On openSUSE (and some other distros) `sudo` and install.sh's subprocess env don't inherit `/usr/sbin`, so the root-only utilities `doctor` needs read as "not found in PATH" despite being installed. `which()` now falls back to canonical sbin directories (`/usr/sbin`, `/sbin`, `/usr/local/sbin`) when `$PATH` lookup misses. Verified across opensuse / ubuntu / alpine / fedora / arch via the new `tools/distro-smoke` harness — sgdisk PASS on all 5 distros.
- **`install.sh` refused when `cosign` was present but outside `$PATH`** ([#328](https://github.com/aegis-boot/aegis-boot/issues/328) closed by [#331](https://github.com/aegis-boot/aegis-boot/pull/331)). New `--cosign PATH` flag for explicit override, plus auto-probe of the install.sh script directory and the `--prefix` directory when `command -v cosign` misses. The verify-blob invocation now uses the resolved `$cosign_bin`, so operators running an alternate cosign binary get what they asked for.
- **`doctor` found cosign but `fetch-image` didn't** ([#332](https://github.com/aegis-boot/aegis-boot/issues/332) closed by [#336](https://github.com/aegis-boot/aegis-boot/pull/336)). Two different probes: `doctor` used `which("cosign")` (file-existence on PATH), `fetch-image` shelled out to `cosign --version` and checked the exit code (which returns false for reasons beyond "binary missing" — broken install, fussy cosign version, transparency-log timeout). New shared `src/cmd_path.rs` module owns the single probe so every aegis-boot surface asking "is this command available?" agrees. Behavior change: when cosign is present but `--version` exits non-zero, `fetch-image` now attempts verify-blob and surfaces the real cosign error instead of a confusing "not on PATH" message.
- **`doctor` remedy text used single-distro hints + wrong package names** ([#333](https://github.com/aegis-boot/aegis-boot/issues/333) closed by [#337](https://github.com/aegis-boot/aegis-boot/pull/337)). Core-utility remedies hinted only Debian/Ubuntu and defaulted `pkg == cmd`, producing advice like "install `lsblk` via apt-get install lsblk" (lsblk is in `util-linux`, not a package named `lsblk`). New `PkgNames` struct for binaries whose package names diverge across distro families (`sgdisk` → `gdisk` on apt/dnf, `gptfdisk` on pacman; `gpg` → `gnupg` on apt/pacman, `gnupg2` on dnf). Same-name-across-distros fixes: `dd`/`sha256sum` → `coreutils`, `lsblk` → `util-linux`. `check_host_commands()` extracted from `try_run()` in the same change.
- **`fetch-image` cosign-install URL was stale** ([#329](https://github.com/aegis-boot/aegis-boot/issues/329) closed by [#330](https://github.com/aegis-boot/aegis-boot/pull/330)). `https://docs.sigstore.dev/cosign/installation/` → `https://docs.sigstore.dev/cosign/system_config/installation/`. The old URL 301-redirects; the new one returns 200. Three call-sites updated (fetch_image warning, doctor NEXT ACTION, install.sh error).

### Local cross-distro test harness (new)

- **`tools/distro-smoke/`** ([#334](https://github.com/aegis-boot/aegis-boot/pull/334)). Docker-based matrix that runs `aegis-boot doctor` + `install.sh --help` across openSUSE / Ubuntu / Alpine / Fedora / Arch using the static-musl release binary. Catches the class of install-flow bugs that unit tests can't. One-shot orchestrator (`run.sh`) writes `output/<run-id>/<distro>.log` + `summary.md`; MANIFEST.md enumerates every artifact with exact cleanup commands. Containers all run `--rm` so no stray state. Motivated by #328; surfaced #333 on first run. Not wired into CI yet — operator-facing harness, promote-to-CI path documented in the README.

### Doc accuracy forward-rolls

- **INSTALL.md + `fetch-image` help** ([#326](https://github.com/aegis-boot/aegis-boot/pull/326)). `v0.12.0` → `v0.14.1` in the TUI screenshot; hardcoded version in `fetch-image`'s `print_help()` → `env!("CARGO_PKG_VERSION")`.
- **Scaffolding comments + Alpine validation phrasing** ([#327](https://github.com/aegis-boot/aegis-boot/pull/327)). `plan.rs` + `readback.rs` module-level dead-code-allow comments no longer claim "no callers wired up" (flash is now the first caller for both); README status line distinguishes Ubuntu's successful kexec from Alpine's designed unsigned-kernel rejection under KEXEC_SIG.

## [0.14.1] — 2026-04-19

### Bug fixes (release-workflow)

- **`release.yml` hits GitHub's 2 GiB asset upload cap + ships a much smaller .img** — v0.14.0's publish step failed at the final asset upload with `HTTP 422 Validation Failed` on `aegis-boot.img`. Root cause: `mkusb.sh`'s default `DISK_SIZE_MB=2048` produces a file exactly at GitHub's 2 GiB release-asset cap (2,147,483,648 bytes). The build + cosign-sign steps succeeded, so the release gained orphaned `aegis-boot.img.sig` + `aegis-boot.img.sha256` assets whose target image wasn't uploaded. Fix: set `DISK_SIZE_MB=512` in `release.yml`. The .img only needs to carry the ESP's signed chain (~56 MB actual payload, 400 MB partition for headroom) + an empty AEGIS_ISOS data partition; operators add ISOs post-flash, and `flash` auto-expands AEGIS_ISOS to fill any-size stick (#242). The previous 2048 MB default shipped ~1644 MB of empty data partition that operators never saw — 512 MB is ~4× smaller, ~4× faster to fetch, same UX.

### Documentation

- **Doc version refs rolled to 0.14.1** — `README.md`, `docs/INSTALL.md`, `docs/CLI.md`, `man/aegis-boot.1`. Motivates the doc-automation tracking issue ([#286](https://github.com/aegis-boot/aegis-boot/issues/286)) for workspace-level version single-source + CI drift-check.

### Release workflow

- v0.14.0 is left in place on GitHub as a partial release (all non-`.img` assets valid + signed); operators should use v0.14.1.

## [0.14.0] — 2026-04-19

### Bug fixes (operator-reported)

- **`flash` on an installed binary no longer fails with a cryptic repo-root error** (closes [#282](https://github.com/aegis-boot/aegis-boot/issues/282)) — reported by an external user who ran `sudo ./aegis-boot flash /dev/sda` on v0.13.0 and hit `flash failed: cannot find aegis-boot repo root (no Cargo.toml)`. Root cause: `build_image_via_mkusb` required the repo tree for `scripts/mkusb.sh`, so every operator who installs via a release tarball (not `git clone`) hit it on first use. New `FlashError::NoImageSource` variant with `FLASH_NO_IMAGE_SOURCE` stable code renders three numbered alternatives, each interpolating the exact device path: (1) pass `--image /path/to/aegis-boot.img`, (2) `aegis-boot fetch-image && flash --image` (now usable because this release ships the `.img` asset below), (3) clone + `cargo install --path crates/aegis-cli`. 4 new unit tests cover classification + rendered output + device-path interpolation + the /dev/sdX fallback placeholder.

### Direct-install flash (epic [#274](https://github.com/aegis-boot/aegis-boot/issues/274)) — Phases 2a, 2b, 2c, 3a, 3b

All five phases land behind `#[allow(dead_code)]` — the flash command still goes through `mkusb.sh + dd` in v0.14.0. A later release wires the `--direct-install` flag and runs the new path end-to-end under OVMF SecBoot (Phase 3c/d/e of the epic).

- **Phase 2a: partition + format foundation** (PR1 of #274) — new `crates/aegis-cli/src/direct_install.rs` ships Rust-native helpers: `partition_stick` (sgdisk zap + fresh GPT + ESP 400 MB + AEGIS_ISOS rest-of-disk), `format_esp` (mkfs.fat FAT32 labeled AEGIS_ESP), `format_data_partition` (mkfs.exfat labeled AEGIS_ISOS). Constants (`ESP_SIZE_MB`, `AEGIS_ISOS_LABEL`, `ESP_TYPE_CODE`, `DATA_TYPE_CODE`) are drift-tested against `scripts/mkusb.sh`'s defaults. 10 unit tests cover the pure `build_partition_argv` argv-builder. Backed by nexus-agents consensus vote (higher_order, 80% approve).
- **Phase 2b: ESP staging + grub.cfg + combine_initrd** (PR2 of #274) — adds `build_mcopy_argv` / `build_mmd_argv` (with `--` delimiter hardening against argv-injection on `-`-prefix paths and `-D o`/`-D s` for idempotent replay), `EspStagingSources` struct bundling the 6 signed-chain paths, `render_grub_cfg` + `build_grub_cfg_body` (3-menuentry rescue-tui menu: tty0-primary / serial-primary / verbose), `combine_initrd` (concat distro_initrd || aegis_initrd matching `mkusb.sh:114-115`), and `stage_esp` (mmd the EFI skeleton + 6 mcopy writes in the fixed `mkusb.sh:186-191` order). 13 new drift tests pin all 6 `::/` destination paths, grub.cfg content invariants (vmlinuz + initrd.img + serial-primary + verbose + `aegis.verbose=1`), and concat ordering.
- **Phase 2c: signed attestation manifest** (PR3 of #274, closes [#277](https://github.com/aegis-boot/aegis-boot/issues/277)) — produces `::/aegis-boot-manifest.json` + `::/aegis-boot-manifest.json.minisig` on the ESP. Schema version 1 (locked via #277 consensus): closed-set file list with FAT32-case-insensitive comparison; `manifest_sequence` (u64 monotonic) defends against rollback without relying on a secure RTC; `disk_guid` + `partition_count` + per-partition `type_guid`/`partuuid`/`fs_uuid` replaces a brittle `partition_table_sha256` (GPT backup header LBA moves with disk size); 64 KiB hard cap on body size bounds early-boot JSON parser exposure; `expected_pcrs: []` until aegis-hwsim E6 locks the TPM PCR shape. New pure-Rust `minisign = "0.9"` dep (no sigtool shellout, ed25519-based). 22 unit tests cover schema pin, canonical JSON stability, size-cap rejection on write + read, rollback rejection, forward-incompat schema rejection, sign → verify round-trip, tampered-body + wrong-key rejection, case-insensitive set cover.
- **Phase 3a: hash + size helpers** (PR4 of #274) — `sha256_file` (streaming 64 KiB-chunk hasher, heap-allocated to keep stack frames small), `file_size`, and `compute_esp_file_hashes` taking a Phase 2b `EspStagingSources` and returning `[EspFileEntry; 6]` in the fixed canonical order. Both grub.cfg destinations share digest+size — they reference the same local source. 8 unit tests: RFC-known `sha256("")` + `sha256("abc")` digests, `>64 KiB` chunk-boundary path, missing-file propagation, round-trip through `build_manifest` → `parse_and_validate`.
- **Phase 3b: GPT + blkid device-identity readers** (PR5 of #274) — `build_sgdisk_p_argv` / `build_sgdisk_info_argv` / `build_blkid_tag_argv` (narrow `-o value -s <KEY>` form); `parse_disk_guid_from_sgdisk_p`, `parse_partition_count_from_sgdisk_p` (detects rogue 3rd partition — verifier uses this against `device.partition_count: 2`), `parse_first_last_lba_from_sgdisk_info`; `read_device_identity` composing 6 read-only subprocess calls. 11 new unit tests with realistic `sgdisk -p` / `--info=1` sample outputs + partial / non-numeric guards.

### Release integrity

- **`release.yml` now publishes signed `aegis-boot.img`** (PR2 of [#235](https://github.com/aegis-boot/aegis-boot/issues/235)) — extends the release workflow to produce the signed-chain disk image alongside the existing binary artifacts. `mkusb.sh` runs after the CLI build and emits `out/aegis-boot.img` + `out/aegis-boot.img.sha256`; the image is added to the aggregate `SHA256SUMS`, signed by the existing cosign `sign-blob` loop (producing `aegis-boot.img.sig` + `aegis-boot.img.pem` bound to the release workflow's OIDC identity), and uploaded as four additional assets. Operators can now `aegis-boot fetch-image --url .../aegis-boot.img` and the existing auto-verification (shipped in #267) activates — no flag-flipping required. Image contents default to empty `AEGIS_ISOS` (operator uses `aegis-boot fetch` + `add` to populate). CI job timeout bumped 25 → 30 min to accommodate the mkusb.sh run.
- **`aegis-boot fetch-image` cosign keyless auto-verify** (PR1 of #235) — adds a second integrity layer on top of the existing `--sha256` check. After download + sha256 verify, `fetch-image` automatically downloads `<URL>.sig` + `<URL>.pem` and runs `cosign verify-blob` against a hardcoded keyless identity bound to aegis-boot's own `release.yml` workflow. Graceful-degrades on 404 (older releases / forks) to WARNING + sha256-only; fail-closed on verification mismatch (deletes the image + sidecars). New `--no-cosign` flag for air-gapped operators. Optional `cosign` row in `aegis-boot doctor` (WARN not FAIL when absent).
- **`aegis-boot fetch-image` auto-resolves to the latest release** (PR3 of #235) — operators now run `aegis-boot fetch-image` with zero arguments and get the latest release image, cosign-verified. New `--version TAG` flag pins to a specific release; `--url URL` still works for arbitrary sources. `release_download_url_for_tag()` helper refuses shell-meta / path-traversal in the tag argument (`[A-Za-z0-9._-]{1,32}` only).

### Security hardening

- **Refuse group/world-writable `AEGIS_TRUSTED_KEYS` entries** (CWE-732 defense-in-depth) — `iso-probe::minisign::load_trusted_keys` refuses to load a `.pub` file (and skips an entire directory) when the inode's mode bits include `0o022` (group-write or world-write). Prevents an architectural weakness if the env var ever points at a multi-user shared location. Currently safe in the single-user initramfs but forecloses the foot-gun on operator hosts. 6 unit tests cover owner-only modes, group/world-writable rejection, missing-file fail-closed.

### CI hardening

- **Pin semgrep image to 1.160.0** (partially closes [#253](https://github.com/aegis-boot/aegis-boot/issues/253)) — `returntocorp/semgrep:latest` floats. If upstream setuptools 81+ removes `pkg_resources` before semgrep drops its transitive import, the SAST container would fail to start and the gate would turn into a hard failure or a silent no-op. Pinned to 1.160.0 (2026-04-16 release). Refresh the pin when semgrep ships a build that drops `pkg_resources`.

### Bug fixes

- **Close auto-expand automount race** (closes [#272](https://github.com/aegis-boot/aegis-boot/issues/272)) — on Linux desktops running udisks2 / gvfs (GNOME / KDE / XFCE), `flash --image <smaller-img>` would run sgdisk to expand partition 2, then fail the subsequent `mkfs.exfat` step with `Device or resource busy` because the desktop auto-mounted the freshly-resized partition. Fix: after `partprobe` and before `mkfs.exfat`, run `udevadm settle` + `findmnt -n <dev>` + lazy `umount -l`. No-op on initramfs / CI. Real-hardware validated: 30 GB Cruzer, 2 GB image → AEGIS_ISOS spans the full stick.

### v1.1 usability epic [#241](https://github.com/aegis-boot/aegis-boot/issues/241) — Ventoy parity without dropping signed-chain

- **`aegis-boot flash` auto-expands AEGIS_ISOS to fill the stick** (closes [#242](https://github.com/aegis-boot/aegis-boot/issues/242)) — rescoped from a standalone `aegis-boot expand` subcommand to a flash-time behaviour per nexus-agents consensus vote (Architect 0.88, Security 0.90). After dd + sync + readback-verify, `flash` runs `sgdisk -e` to move the backup GPT to end-of-disk, recreates partition 2 spanning all remaining space with type 0700 (Microsoft Basic Data), `partprobe`s, then `mkfs.exfat`'s the new larger partition. Operators flashing a fresh 32 GB stick now see a ~30 GB AEGIS_ISOS instead of ~1.6 GB — no separate command, no mental model, no way to accidentally nuke existing ISOs (the reformat runs on a known-empty partition). New `--no-expand` flag opts out for the rare case an operator wants the small mkusb-default partition. The expand step is surfaced in `flash --dry-run` output as step 5 of 6. Trust impact: none — partition 1 (ESP, signed chain) untouched; partition 2 is unsigned operator content by design. Linux-only (sgdisk + partprobe + mkfs.exfat are Linux tools); macOS silently skips the expand step. Soft-fails: if the sgdisk/mkfs chain fails, the stick still boots, operator sees a warning naming the failure. 3 new unit tests: `partition2_path` handles both SCSI-style (`/dev/sda` → `/dev/sda2`) and NVMe/mmcblk-style (`/dev/nvme0n1` → `/dev/nvme0n1p2`) device names; the flash `Plan` exposes the expand step with "AEGIS_ISOS" and "exFAT" in the rendered output.

- **`aegis-boot flash` indicatif progress UI** (PR3 of [#244](https://github.com/aegis-boot/aegis-boot/issues/244)) — replaces the silent 2-minute dd stretch with a live progress bar showing bytes-written / total, current bytes/sec, and ETA. Implementation: a background reader thread drains dd's `status=progress` stderr through a small pure-function parser (`parse_dd_progress_line`) and forwards byte counts to an indicatif `ProgressBar`. `sudo -v` runs first to refresh credentials so dd's piped stderr doesn't swallow a password prompt. Linux-only (macOS dd doesn't emit `status=progress`); macOS + `--no-progress` both fall back to the original silent `.status()` path. New argv flag `--no-progress` opts out for CI / dumb terminals / structured-logging pipelines. New `run_dd` dispatcher picks the right runner by platform and flag. New \`indicatif = "0.17"\` runtime dep (\`default-features = false\`, ~50 KB). 5 new unit tests cover the parser: canonical GNU dd format, u64-range values (32 GB+ sticks), whitespace/CR tolerance, non-progress-line rejection (`N+M records in/out`, empty, error lines, noise), non-numeric prefix rejection.

- **`aegis-boot add` structured FAT32-ceiling error** (PR5 of [#247](https://github.com/aegis-boot/aegis-boot/issues/247)) — converts `add`'s FAT32 4 GiB per-file ceiling refusal from an ad-hoc 11-line `eprintln!` block into a `UserFacing`-rendered error. New `AddError::Fat32CeilingExceeded { detail, flash_target }` carries the pre-formatted detail string (filename + humanized size + fs type) and the canonical device path that both reflash recipes get interpolated into. Output now renders:
  ```
  error[ADD_FAT32_CEILING]: ISO exceeds FAT32 4 GiB per-file ceiling
    what happened: Win11_25H2.iso is 7.9 GiB — exceeds FAT32's 4 GiB per-file ceiling. ...
    try one of:
      1. Reflash with the new exfat default (preserves cross-OS r/w on Linux + macOS + Windows): `sudo aegis-boot flash /dev/sdc`
      2. Reflash with ext4 for a Linux-only stick: `DATA_FS=ext4 sudo aegis-boot flash /dev/sdc`
  ```
  Copy-paste readiness is the value add — the old block required the operator to re-type or edit the device path. Stable `ADD_FAT32_CEILING` code for tooling that greps stderr. Fallback to `/dev/sdX` placeholder preserved when the mount wasn't backed by a resolvable block device (bind mount / operator-supplied path). `unmount_temp` cleanup on the error path unchanged. 3 new unit tests cover the rendered-block shape (header + detail + both options with concrete device), the `/dev/sdX` placeholder fallback, and the `Display` impl.

- **`aegis-boot update` structured Ineligible error + `UserFacing::suggestions()`** (PR4 of [#247](https://github.com/aegis-boot/aegis-boot/issues/247)) — extends the `UserFacing` trait with an optional `suggestions() -> Vec<String>` for the multi-option case and converts `update`'s NOT-ELIGIBLE branch to use it. Rationale: `update` already shipped a two-option "your options: 1. re-flash... 2. run `aegis-boot init /dev/sdX`..." block that the original single-line `suggestion()` couldn't carry, so the naive "just convert to `UserFacing`" approach would regress operator UX. `Vec<String>` (owned) not `&[&str]` so implementors can embed dynamic strings like the operator's device path in option 2; allocation happens only on the error path and is bounded by the number of alternatives. `render_string` now checks `suggestions()` first and renders a numbered `try one of:` list; `suggestion()` falls through as the single-line advice form when `suggestions()` is empty. Empty vector treated the same as absent (default impl returns `Vec::new()`). New `UpdateError::Ineligible { reason, device }` variant in `update.rs` carries both the per-reason sentence from `check_eligibility` and the device path that gets interpolated into option 2 (`aegis-boot init /dev/sdc`) so operators can copy-paste without substitution. Output now renders with a stable `UPDATE_INELIGIBLE` code in the header so tooling can grep. JSON mode (`--json`) unchanged — the structured envelope already carried the reason. 5 new unit tests (3 on the trait: numbered-list emission, precedence over `suggestion()`, single-line fallback when `suggestions()` empty; 2 on `UpdateError`: rendered-block shape with device interpolation, `Display` impl includes the reason).

- **`aegis-boot flash` structured failure messages** (PR3 of [#247](https://github.com/aegis-boot/aegis-boot/issues/247)) — flash errors now render in the `cause / what happened / try / see / code` format from the epic's spec. New `FlashError` enum with 5 variants (`ImageBuild`, `DdFailed`, `ReadbackMismatch`, `ShortReadback`, `Other`), each implementing `UserFacing` with variant-specific suggestion text and a `docs_url` into `docs/TROUBLESHOOTING.md`. Classification happens at the top-level boundary via `FlashError::classify(msg)` — keeps the internal `flash()` on `Result<(), String>` so we don't need to touch every `.map_err` site (that can land in a later PR if/when value demands it). Errors now surface with stable codes like `FLASH_DD_FAILED` that tooling can grep. The `#![allow(dead_code)]` from `userfacing.rs`'s foundation PR is dropped; `render_string` is now live and `render` carries a per-item allow tied to its future `Display`-integration caller. Example output for a dd failure:
  ```
  error[FLASH_DD_FAILED]: write to stick failed (dd)
    what happened: dd exited with exit status: 1
    try: The write to the device failed. Unplug, replug, and retry. ...
    see: https://github.com/aegis-boot/aegis-boot/blob/main/docs/TROUBLESHOOTING.md#dd-exited-...
  ```
  7 new unit tests cover `FlashError::classify` pattern-matching (mkusb/dd/readback-mismatch/short-readback/unknown fallback to Other) + the rendered-output shape (all 4 sections present + stable error code) + the invariant that `ReadbackMismatch` and `ShortReadback` share their suggestion + docs URL (same operator action applies).

- **`aegis-boot flash` post-write readback verification** (PR2 of [#244](https://github.com/aegis-boot/aegis-boot/issues/244)) — closes the silent-write-failure window. Cheap or failing USB sticks sometimes accept a `dd` happily, return success, and hold zeros in the boot sector — the next boot then fails with a Secure Boot violation that's impossible to diagnose from the rescue UI. Reading back the first 64 MB and re-checking the sha256 catches that **before the operator pulls the stick**. Implementation: `precompute_image_prefix_hash(img_path)` runs before `dd` while we still have local-only file I/O (so readback failures surface as a clean "stick is bad" error, not "couldn't even read source for comparison"); after `dd` + `sync` + `partprobe`, `readback_verify_device(dd_target, expected_hex)` shells out to `sudo dd if=<dev> bs=1M count=64 status=none` to read back the prefix and compares the sha256 in-process via the existing `readback::sha256_of_first_bytes`. On success: `✓ readback verified — first 64 MB on stick matches the source image`. On mismatch: clean error message naming the failure mode (silent write failure, often counterfeit/failing flash chip) and the actionable next step (try a different stick or USB port). Soft-fails when the source image hash can't be precomputed (e.g. truncated source) — operator gets a warning and the dd still runs, but verification is explicitly SKIPPED rather than silently passed. 2 new unit tests cover the precompute path (image ≥64 MiB → 64-char lowercase hex; image <64 MiB → "truncated"/"shorter" error). Indicatif progress UI deferred to PR3 (touches the dd subprocess; meaningful refactor).

- **AEGIS_ISOS defaults to exFAT** (closes [#243](https://github.com/aegis-boot/aegis-boot/issues/243)) — `mkusb.sh`'s default `DATA_FS` is now `exfat` instead of `fat32`. Lifts the FAT32 4 GB per-file ceiling so Win11 (~7.9 GB), Rocky 9 DVD (~10 GB), Ubuntu LTS Desktop (~5.8 GB) now drop straight onto the stick — no `DATA_FS=ext4` reflash needed. exFAT is natively read/write on Linux 5.7+, macOS, and Windows. The initramfs build now ships the `kernel/fs/exfat/exfat` module (`CONFIG_EXFAT_FS`) and the runtime mount-fallback loop tries `exfat:rw` before `ext4:rw` and the legacy `vfat` paths. `aegis-boot list /dev/sdX` and `add` mount calls now try `exfat → ext4 → vfat` in order, recovering from any of the three default filesystems automatically. The legacy `DATA_FS=fat32` opt-in remains for max-compatibility builds, with the FAT32-ceiling check still firing on those sticks (now pointing operators at exfat **and** ext4 reflash recipes). New apt-prereq: `exfatprogs` (in Ubuntu main since 22.04, Debian 11+, Fedora 33+); added to mkusb/ovmf-secboot CI workflows + dev-test.sh + LOCAL_TESTING.md prereq lists. Trust impact: none — boot decisions still consume the sha256-attested manifest, and the existing GRAY-verdict + typed-confirmation gate already covers the "drag-and-drop unattested ISOs" ingress vector.
- **`aegis-boot flash --dry-run`** (PR2 of [#247](https://github.com/aegis-boot/aegis-boot/issues/247)) — first caller of the foundational `Plan` + `Operation` types from PR1. Operators can now preview what `flash` would do *before* the destructive write — no USB cycle burned. The new `build_flash_plan(&drive, prebuilt_image)` helper in `flash.rs` produces a 5-step typed `Plan` (precheck source / precheck removable+USB / write to block device / readback verify / write attestation receipt). Under `--dry-run`, the plan is rendered to stdout and the runner exits before `confirm_destructive` and `flash()`. Non-dry-run path is unchanged — same drive selection, same typed-confirmation prompt, same `dd` + attestation. Help text and CLI argv parser extended; 5 new unit tests cover the plan shape (5-op pipeline structure, `--image` vs mkusb branching, drive-size in `WriteToBlockDevice`, `DEFAULT_READBACK_BYTES` in `ReadbackVerify`, intent string contains the device path). `UserFacing` error wrapping deferred to PR3 to keep this change contained.
- **Per-ISO sidecar metadata** (closes [#246](https://github.com/aegis-boot/aegis-boot/issues/246)) — operator-curated `<iso>.aegis.toml` files now travel alongside ISOs and surface in the rescue-TUI menu and `aegis-boot list` table. Schema (all fields optional): `display_name`, `description`, `version`, `category`, `last_verified_at`, `last_verified_on`, `notes`. New `iso-probe::sidecar` module (TOML parser + writer) with `IsoSidecar` struct + `load_sidecar`/`write_sidecar`/`sidecar_path_for` helpers. `DiscoveredIso.sidecar: Option<IsoSidecar>` field populated during scan; `iso_probe::display_name` resolution order is now `sidecar.display_name → pretty_name → label`, so the existing 4 rescue-TUI render call-sites pick up curated names with zero render-side changes. New `iso_probe::display_description(iso)` helper for the menu's optional second-line subtitle. `aegis-boot add` gains `--description TEXT --version VER --category CAT` flags that write the sidecar at copy time (routed through the existing sudo-mount staging path so AEGIS_ISOS permissions are respected). `aegis-boot list` renders the curated name as the primary label with the bare filename in parens on a continuation row when both differ; `aegis-boot list --json` adds stable `display_name` + `description` fields (JSON `null` when no sidecar). Sidecars are **not signed** — boot decisions still consume the sha256-attested manifest; tampering with a sidecar can change display strings but cannot affect what boots. New `toml = "0.8"` dependency on iso-probe (`default-features = false`, features = `["parse", "display"]`). 25 new unit tests cover round-trip, malformed-sidecar fallback, `parse_add_args` flag forms, and scan-attach behaviour.
- **`init` wizard helpers — pure logic for serial-confirmation safety gate** (PR1 of [#245](https://github.com/aegis-boot/aegis-boot/issues/245)) — foundational unit-tested helpers for the wrong-device-dd safety gate. New `crates/aegis-cli/src/init_wizard.rs` ships dep-free pure functions: `parse_lsblk_removable_usb` (parses `lsblk -J -b -o NAME,SIZE,MODEL,SERIAL,RM,TRAN` and filters to `rm=true && tran=usb`), `serial_token` (extracts the last 4 alphanumeric chars of a hardware serial), `serial_matches` (typo-strict normalized match), `format_drive_menu` (numbered render with model + size + serial), `parse_menu_selection` (1-indexed → 0-indexed with bounds check), `is_target_mounted` (parses `findmnt -J` output), and the `trust_narrative_paragraph` shown once during `init` (mirrors `docs/HOW_IT_WORKS.md`). 30 new unit tests cover the lsblk filter, serial-token edge cases, match strictness, menu rendering, selection bounds, and mounted-detection. No callers wired up — the interactive prompt that consumes these helpers lands in PR2.
- **`aegis-boot init` interactive wizard wired up** (PR2 of [#245](https://github.com/aegis-boot/aegis-boot/issues/245)) — first caller of the foundational `init_wizard` helpers from PR1 ([#254](https://github.com/aegis-boot/aegis-boot/pull/254), merged). When `init` runs without an explicit device AND without `--yes`, the wizard now drives the operator through the full safety gate: enumerate removable USB drives via `lsblk -J -b -o NAME,SIZE,MODEL,SERIAL,RM,TRAN`, render a numbered menu (single-drive case offers a `[Y/n]` shortcut), refuse-on-mounted via `findmnt -J <device>` (override with new `--force` flag), display the chosen device's serial and ask the operator to type the last 4 chars to confirm, print the trust-narrative paragraph from `init_wizard::trust_narrative_paragraph`, and finally hand the resolved `/dev/sdX` to the existing `flash_step` with `--yes` (the wizard already did the human-confirmation work — flash should not re-prompt). New `Parsed.force` field + `--force` argv flag. Help text gains an "INTERACTIVE MODE" section. The `#![allow(dead_code)]` from `init_wizard.rs`'s foundation PR is dropped now that all helpers have callers. The `FindmntEntry` struct is collapsed to `serde_json::Value` since we only inspect array length, not per-entry fields. `Parsed` gets `#[allow(clippy::struct_excessive_bools)]` (5 independent argv bool flags; bitflags would obscure the 1:1 mapping).
- **End-user explainer documentation + `aegis-boot tour`** (closes [#248](https://github.com/aegis-boot/aegis-boot/issues/248)) — three new operator-onboarding surfaces. `docs/HOW_IT_WORKS.md` is the 5-minute conceptual walkthrough: what aegis-boot does, why every other multi-ISO USB tool requires disabling Secure Boot or trusting an unsigned bootloader, and the firmware → shim → grub → kernel → rescue-tui → kexec trust chain in 30 seconds. `docs/TOUR.md` is the first-time procedural walkthrough — `doctor → init → fetch → add → boot`, ~10 minutes hands-on. New `aegis-boot tour` CLI subcommand prints a 30-second in-terminal walkthrough showing the 4-command path + pointers to both docs and `--help` for each subcommand. Wired through `print_help`'s top-level summary, the man page subcommand list, and bash/zsh completions. Audience: a Linux-curious sysadmin who's read about Secure Boot but never set it up — explicitly the audience that aegis-boot needs to grow into beyond operators who already know what shim/grub/MOK mean.
- **Post-write readback verification helpers** (PR1 of [#244](https://github.com/aegis-boot/aegis-boot/issues/244)) — foundational `crates/aegis-cli/src/readback.rs` module for the "step 4 of 4: read back + verify" surface in the new flash flow. `sha256_of_first_bytes(reader, n_bytes)` streams up to N bytes through Sha256, returns the hex digest plus actual bytes consumed. `verify_readback(path, n_bytes, expected_sha256_hex)` opens a path, runs the streamer, and returns a typed `ReadbackError` (`Io` / `InvalidExpectedFormat` / `ShortRead` / `Mismatch`). `is_valid_sha256_hex` guards against malformed expected values pre-comparison. `DEFAULT_READBACK_BYTES = 64 MiB` — sized to cover the ~50 MB signed-chain payload (shim + grub + kernel + initramfs) with margin while staying under 10s on slow USB. No callers wired up in this PR; the `flash` integration lands alongside the `indicatif`-based progress UI in PR2. Closes the silent-write-failure window: cheap sticks sometimes accept a `dd` happily, return success, and hold zeros in the boot sector — readback catches that before the operator pulls the stick. 14 new unit tests cover happy path, mismatch, short-read, malformed-expected, missing file, chunk-boundary, and the format guard. Adds `sha2 = "0.10"` and `hex = "0.4"` to aegis-cli (both already transitive deps via iso-probe).

### CI hardening

- **Bump Node-20-based GitHub Actions to Node-24-compatible v5** (closes [#252](https://github.com/aegis-boot/aegis-boot/issues/252)) — `actions/checkout@v4 → @v5` and `actions/upload-artifact@v4 → @v5` across all 11 workflow files (`ci.yml`, `mkusb.yml`, `kexec-e2e.yml`, `fuzz.yml`, `reproducible-build.yml`, `initramfs.yml`, `catalog-revalidate.yml`, `ovmf-secboot.yml`, `integration.yml`, `qemu-smoke.yml`, `brew-test.yml`, `release.yml`). Closes the deprecation banner runners surface on every job ("Node.js 20 will be removed from the runner on September 16th, 2026"). v5 is backward-compatible with the explicit `retention-days` and `if-no-files-found` settings the upload steps use. Out of scope: `Swatinem/rust-cache@v2`, `EmbarkStudios/cargo-deny-action@v2`, `cachix/install-nix-action@v27`, `sigstore/cosign-installer@v3` — non-`actions/*` and not flagged by the Node-20 banner; tracked separately if their upstreams ship Node-24 majors.

### Cross-platform reach (epic [#136](https://github.com/aegis-boot/aegis-boot/issues/136) child issues)

- **macOS drive enumeration + `flash --image`** ([#229](https://github.com/aegis-boot/aegis-boot/pull/229), closes [#228](https://github.com/aegis-boot/aegis-boot/issues/228)) — first slice of cross-platform flash. `detect/` extracted into platform dispatch via `cfg(target_os)`: Linux sysfs unchanged, new macOS module parses `diskutil list -plist external physical | plutil -convert json` (no plist crate dep). New `flash --image PATH` flag skips `mkusb.sh` and writes a pre-built image directly — works on every platform, required on macOS where mkusb.sh's losetup/sbsign/sgdisk dependency chain is Linux-only. macOS path: `diskutil unmountDisk` before dd, `/dev/diskN → /dev/rdiskN` rewrite for ~10x throughput on the raw node, macOS-style `bs=4m` + `conv=sync` (no `oflag=direct`/`status=progress`). CI gains a new `aegis-cli` job that runs `cargo test -p aegis-cli` + a `cargo check --target x86_64-apple-darwin` cross-compile gate against committed `tests/fixtures/diskutil/*.json` parser fixtures.
- **Windows drive enumeration via `Get-Disk`** ([#230](https://github.com/aegis-boot/aegis-boot/pull/230)) — third-platform unlock following the macOS pattern. `detect/windows.rs` parses `Get-Disk | ConvertTo-Json -Depth 2`, filtering to `BusType == "USB"` AND `IsBoot == false` AND `IsSystem == false` (never offer the operator's boot device as a flash target — rare but possible when machines actually boot from USB). Returns `\\.\PhysicalDriveN`. Raw-disk writing on Windows is a follow-up; this PR delivers enumeration so `aegis-boot list` on Windows can at least show the operator which USB they're looking at. CI extends the `aegis-cli` job with `cargo check --target x86_64-pc-windows-gnu`.
- **`aegis-boot fetch-image` subcommand** ([#232](https://github.com/aegis-boot/aegis-boot/pull/232), closes [#231](https://github.com/aegis-boot/aegis-boot/issues/231)) — pairs with `flash --image`. `aegis-boot fetch-image --url URL --sha256 HEX` downloads + sha256-verifies a pre-built `aegis-boot.img`, prints the verified path to stdout so it composes via `$(...)`: `img=$(aegis-boot fetch-image --url ... --sha256 ...) && aegis-boot flash --image "$img" /dev/sdX`. Security defaults: HTTPS-only (refuses http/file/ftp/javascript:), control-char rejection in URLs (NUL/CR/LF), 64-hex sha256 validated at parse time, mismatch deletes the cached file (no silent trust on re-run), WARNING surfaced when `--sha256` omitted with the computed hash printed for pinning. Subprocess use: shells out to `curl` + `sha256sum` (existing host deps). Cosign signature verification deferred until release.yml publishes `.sig` + `.pem` alongside `.img`.

### Hardware coverage loop (epics [#137](https://github.com/aegis-boot/aegis-boot/issues/137) + [#136](https://github.com/aegis-boot/aegis-boot/issues/136))

- **`aegis-boot compat` subcommand** ([#192](https://github.com/aegis-boot/aegis-boot/pull/192)) — in-binary `COMPAT_DB` mirroring `docs/HARDWARE_COMPAT.md`; `aegis-boot compat [query]` fuzzy-matches vendor/model, `aegis-boot compat --json` emits a stable `schema_version=1` envelope. Seed data is verified-outcomes-only (no speculation) per the doc's policy.
- **Dedicated `hardware-report.yml` issue template** ([#193](https://github.com/aegis-boot/aegis-boot/pull/193)) — structured GitHub form whose fields map 1:1 to `COMPAT_DB` columns. Replaces the generic bug template that `aegis-boot compat` miss-path and `HARDWARE_COMPAT.md` used to point at.
- **`doctor` machine identity row** ([#194](https://github.com/aegis-boot/aegis-boot/pull/194)) — reads `/sys/class/dmi/id/*` (non-privileged) and prints the operator's vendor + model + firmware so filing a hardware report is copy-paste. Filters common OEM placeholders (`To Be Filled By O.E.M.`, etc.). Linux-only; verdict is Pass or Skip.
- **`doctor` compat-DB cross-check** ([#195](https://github.com/aegis-boot/aegis-boot/pull/195)) — after the identity row, `doctor` runs the DMI string through `find_entry(COMPAT_DB)` and emits a `compat DB coverage` row. Pass when documented, Warn when not (with the report URL inlined into the detail line).
- **Guided MOK enrollment on errno 61** ([#202](https://github.com/aegis-boot/aegis-boot/pull/202), closes the child in [#136](https://github.com/aegis-boot/aegis-boot/issues/136)) — rescue-tui's `SignatureRejected` remedy is now three explicit steps (STEP 1/3 `sudo mokutil --import`, STEP 2/3 describing the blue-on-black "Perform MOK management" screen, STEP 3/3 with firmware boot-menu keys for the top 5 vendors). Replaces a single dense paragraph; no new screens required.
- **`compat --my-machine`** ([#206](https://github.com/aegis-boot/aegis-boot/pull/206)) — auto-fills the lookup query from `/sys/class/dmi/id/*` for single-purpose "is MY machine documented?" flow. Symmetric with `doctor`'s compat-DB cross-check but as a dedicated subcommand. Shares `read_dmi_field` + `dmi_product_label` with doctor so both surfaces agree on the machine label. Exit codes: 0 match, 1 DB miss, 2 DMI unavailable OR mixed with explicit query.

### Scriptable surfaces

- **Uniform `--json` across every read-mostly subcommand** ([#191](https://github.com/aegis-boot/aegis-boot/pull/191)) — `update --json` emits an eligibility envelope + host-chain (sha256 per slot) or a reason-for-ineligible; `recommend --json [slug]` emits the full catalog or a single entry. Completes the sweep alongside prior `--json` additions to `doctor`, `list`, `attest list`, `attest show`, `verify`, `fetch --dry-run`. Every surface shares the `schema_version: 1` envelope and the `doctor::json_escape` helper.
- **`aegis-boot --version --json`** ([#205](https://github.com/aegis-boot/aegis-boot/pull/205)) — completes the --json sweep across every CLI output path including the version surface. Emits `{ schema_version, tool, version }` so scripted consumers (install one-liner assertions, Homebrew formula tests, ansible-verified installs) can parse the version without regex on the human string.
- **`aegis-boot completions bash | zsh`** ([#207](https://github.com/aegis-boot/aegis-boot/pull/207)) — hand-rolled completion script generator for the 13-subcommand surface. Completes top-level subcommands, catalog slugs on `recommend`/`fetch` (via existing `recommend --slugs-only`), compat vendor tokens via `jq` graceful-fallback, shared flag sets on `doctor`/`list`/`attest`/`verify`/`update`, device paths on `init`/`flash`. zsh uses bashcompinit shim.

### v1.1 usability epic [#241](https://github.com/aegis-boot/aegis-boot/issues/241) — Ventoy parity without dropping signed-chain

- **`Plan` + `UserFacing` trait scaffolding** (closes part of [#247](https://github.com/aegis-boot/aegis-boot/issues/247)) — foundational PR1 of the universal `--dry-run` + structured-error rollout. New `crates/aegis-cli/src/plan.rs` carries a typed `Operation` enum (signature verify, block-device write, readback verify, attestation persist, partition-table modify, fs resize, mount/unmount, file copy, manifest update) and a `Plan` struct that orders them with intent narration; `Display` produces the per-step dry-run output format from #247's spec. New `crates/aegis-cli/src/userfacing.rs` ships the `UserFacing` trait (`summary`/`detail`/`suggestion`/`docs_url`/`code`) plus `render` + `render_string` plain-text renderers. No callers wired up — per-command rollout (`flash`, `update`, `add`, `init`, `expand`) lands in follow-ups so each adopter ships independently. Dep-free; switching the renderer to `miette` later is a one-file change. `Operation` is `#[non_exhaustive]` to keep variant additions semver-minor. 15 new unit tests cover trait dispatch, optional-field rendering, sha256 truncation, and ETA emission.

### Quality gates (epic [#138](https://github.com/aegis-boot/aegis-boot/issues/138) — closed)

- **`iso-parser` test-mock hazards closed** ([#196](https://github.com/aegis-boot/aegis-boot/pull/196)) — `MockIsoEnvironment::mount_iso` / `unmount` no longer `.lock().unwrap()` (would cascade-fail every test after a panicked one); use `PoisonError::into_inner` for poison recovery. `MockIsoEnvironment::metadata` no longer returns `std::fs::metadata(std::env::temp_dir())` for any known path (silently validating size/mtime assertions against `/tmp`); fails closed with `ErrorKind::Unsupported`.
- **`unwrap_used` / `expect_used` = deny on remaining three crates** ([#197](https://github.com/aegis-boot/aegis-boot/pull/197)) — `aegis-fitness`, `iso-probe`, and `rescue-tui` had per-crate overrides at `warn` from before the workspace tightening landed. Audit found zero bare `.unwrap()`/`.expect(...)` in production code; tightening to `deny` is a pure safety-posture win. All crates now enforce the workspace default.

### Infrastructure

- **`qemu-usb-passthrough.sh` re-binds USB on exit** ([#198](https://github.com/aegis-boot/aegis-boot/pull/198), closes [#121](https://github.com/aegis-boot/aegis-boot/issues/121)) — after QEMU exits, `xhci_hcd` sometimes logs a reset but doesn't re-attach scsi drivers (lsusb shows the device; `/dev/sda` and `/sys/block/sda` gone until physical replug). Trap handler now resolves the device's sysfs ID before QEMU takes over, then writes it to `/sys/bus/usb/drivers/usb/{unbind,bind}` with a 300 ms settle after QEMU exits. `exec sudo` replaced with plain `sudo` so bash stays alive for the trap.

### Documentation

- **CLI.md coverage refresh** ([#199](https://github.com/aegis-boot/aegis-boot/pull/199)) — added the missing `compat` / `update` / `verify` subcommand sections, documented `--json` across all seven supported commands in one table, refreshed the `doctor` example output to include the new machine-identity + compat DB rows.
- **Theme names + accessibility recipes** ([#200](https://github.com/aegis-boot/aegis-boot/pull/200)) — `README.md`'s `AEGIS_THEME` row now lists all five shipped themes (default/monochrome/high-contrast/okabe-ito/aegis); `TROUBLESHOOTING.md` gets a new "Accessibility" section pairing each symptom (low-contrast / color vision / serial / screen-reader) with the appropriate theme + `AEGIS_A11Y` flag. Closes the operator-discoverability half of the Okabe-Ito item in [#93](https://github.com/aegis-boot/aegis-boot/issues/93) (code already shipped in [#76](https://github.com/aegis-boot/aegis-boot/issues/76)).
- **2026-04-17 content audit log** ([#201](https://github.com/aegis-boot/aegis-boot/pull/201)) — recorded today's audit findings + PRs in `docs/content-audit.md` per the [#78](https://github.com/aegis-boot/aegis-boot/issues/78) cadence.

### Accessibility + ergonomics

- **`Home`/`End` as layout-agnostic first/last binds** ([#204](https://github.com/aegis-boot/aegis-boot/pull/204)) — addresses the [#93](https://github.com/aegis-boot/aegis-boot/issues/93) P2 keybind-audit item. `g`/`G` land on weird physical positions under AZERTY and Dvorak; crossterm maps `Home`/`End` identically across every OS layout. Help overlay shows both lines.

### Distribution + discoverability

- **Static completion files refresh** ([#209](https://github.com/aegis-boot/aegis-boot/pull/209)) — `completions/aegis-boot.bash` and `completions/_aegis-boot` (shipped by `scripts/install.sh`) gained the four subcommands added today (`update`, `verify`, `compat`, `completions`) + `--json` flags on every supported surface + `--my-machine` for compat + `--dry-run` for fetch. Preserves the existing hand-crafted `_init_completion`/`_arguments`-with-descriptions sophistication; no architectural churn.
- **`aegis-boot(1)` man page** ([#210](https://github.com/aegis-boot/aegis-boot/pull/210)) — hand-crafted roff at `man/aegis-boot.1` covering every subcommand + all `AEGIS_*` env vars + exit-code semantics + SEE ALSO pointers to `rescue-tui(1)`, `kexec_file_load(2)`, `mokutil(1)`, `sgdisk(8)`. `scripts/install.sh` installs it (root: `/usr/local/share/man/man1/`; non-root: `~/.local/share/man/man1/`) and runs `mandb -q` to refresh the index.
- **`aegis-boot man` subcommand** ([#211](https://github.com/aegis-boot/aegis-boot/pull/211)) — embeds `man/aegis-boot.1` into the binary via `include_str!` so operators can install the man page without GitHub round-trips: `aegis-boot man | sudo tee /usr/local/share/man/man1/aegis-boot.1`. Completes the self-contained-discoverability trio alongside `aegis-boot completions bash|zsh` (#207) and built-in `--help`. Four regression tests including a drift-guard that asserts every subcommand name appears as a `.B` marker in the embedded page.
- **Homebrew Formula installs completions + man page** ([#212](https://github.com/aegis-boot/aegis-boot/pull/212)) — `generate_completions_from_executable` + `Utils.safe_popen_read(bin/"aegis-boot", "man")`. `chmod 0555` after `bin.install` since GitHub release downloads come without the exec bit. Version-gated via `--help` probe so the Formula stays clean against v0.13.0 (pre-completions/man) and activates fully on v0.14.0+.

### Windows / UDF ISO support (real-hardware testing surfaced #214-#223)

Triggered by a Win11 25H2 ISO dropped into `test-isos/` during interactive testing. The scanner had two silent-failure paths and an unchecked filesystem constraint that all blocked Windows (and large Linux) ISOs before this arc.

- **UDF mount + Windows layout detection** ([#214](https://github.com/aegis-boot/aegis-boot/pull/214)) — iso-parser's `mount_iso` forced `-t iso9660`. Windows install ISOs are UDF-primary with a ~50 KB iso9660 fallback containing only a readme.txt, so the scanner would silently see an empty filesystem. Changed to `-t udf,iso9660` (UDF first, iso9660 fallback for pure-Linux media). Added `try_windows_layout` that looks for `/bootmgr`, `/sources/boot.wim`, or `/efi/microsoft/boot/` and emits a synthesized `BootEntry` with `Distribution::Windows` + the existing `NotKexecBootable` quirk — rescue-tui's `[X] not kexec-bootable` glyph + kexec-refusal code now fires end-to-end instead of ISOs being skipped with a misleading `NoBootEntries`. Verified end-to-end against Win11 25H2, Alpine 3.20.3, Ubuntu 24.04.2 Server.
- **Mount-empty diagnostic** ([#215](https://github.com/aegis-boot/aegis-boot/pull/215)) — when the initial mount attempt returns `status=success` but the mount_point is empty (busybox loop-mode no-op, or filesystem-type mismatch), iso-parser used to return `Ok(empty mount_point)` and callers reported `NoBootEntries`. Now re-verifies the mount_point has entries and emits `MountFailed("mount claimed success but <path> is empty — filesystem type likely not auto-detected")` with the original stderr — aligns with epic [#138](https://github.com/aegis-boot/aegis-boot/issues/138)'s "no silent failures" charter.
- **FAT32 4 GiB preflight** ([#216](https://github.com/aegis-boot/aegis-boot/pull/216)) — `aegis-boot add` now reads `/proc/mounts` to detect the `AEGIS_ISOS` partition's filesystem type and refuses 4+ GiB ISOs on `vfat` with a specific "reflash with `DATA_FS=ext4`" remediation. Triggered by the Win11 ISO (7.9 GiB) but also affects Rocky 9 DVD (~10 GiB), Windows 10 installer (~5.5 GiB), and Ubuntu Desktop (flirting with the 4 GiB ceiling). Runs before the free-space check so operators see the filesystem-specific error rather than a generic "no space" mid-copy.
- **USB_LAYOUT.md + TROUBLESHOOTING.md coverage** ([#217](https://github.com/aegis-boot/aegis-boot/pull/217)) — expanded the FAT32 fit-table with Windows 10/11, Rocky 9 DVD, Ubuntu Server rows; added a `#iso-too-large-for-fat32` anchor to TROUBLESHOOTING so operators pasting the preflight error text into a search box land directly on the fix; added `#windows-installer-iso-doesnt-boot` explaining the architectural constraint (Windows uses `bootmgr.efi` + NT loader, not a kexec-compatible Linux kernel) and pointing at `dd`/Rufus for actual Windows installation.
- **Named-disk preflight** ([#219](https://github.com/aegis-boot/aegis-boot/pull/219)) — the generic `/dev/sdX` placeholder in the FAT32 error forced operators into a risky `lsblk` lookup mid-rescue-flow. The preflight now reads `/proc/mounts` to derive the specific device (e.g., `/dev/sda2` → parent `/dev/sda`, `/dev/nvme0n1p2` → `/dev/nvme0n1`), so the `DATA_FS=ext4 sudo aegis-boot flash <disk>` line is copy-pasteable. Handles sata/virtio/xen/hd (`sdXN` style) and nvme/mmcblk/loop (`pN` suffix style) naming conventions.
- **Helper consolidation** ([#220](https://github.com/aegis-boot/aegis-boot/pull/220)) — the Win11 arc added inventory-side copies of `/proc/mounts` parsing and partition-suffix stripping that already existed in `attest.rs`. New `crates/aegis-cli/src/mounts.rs` module consolidates three helpers (`device_for_mount`, `filesystem_type`, `parent_disk`) with stricter disambiguation — the old sata-style "two alpha chars before trailing digits" heuristic mis-stripped `/dev/mmcblk0` into `/dev/mmcblk`. Explicit prefix allowlist (`sd|vd|hd|xvd`) now for sata-style; nvme/mmcblk/loop require the `p<N>` separator.
- **`aegis-boot compat --submit`** ([#222](https://github.com/aegis-boot/aegis-boot/pull/222)) — closes the last friction point in the hardware-coverage loop. Auto-gathers DMI (via the shared `doctor::read_dmi_field` / `dmi_product_label` / `dmi_bios_label` helpers) and emits a GitHub issue-form URL with `vendor` / `model` / `firmware` / `aegis-version` query params pre-filled. Operators click once instead of manually copying fields from `doctor`'s output. Includes a minimal 12-line RFC 3986 percent-encoder that preserves unreserved ASCII and escapes everything else (including multibyte UTF-8).
- **`doctor` surfaces `compat --submit`** ([#223](https://github.com/aegis-boot/aegis-boot/pull/223)) — the compat-DB-miss WARN row now says `run \`aegis-boot compat --submit\` to draft a report` instead of emitting the raw ~80-char GitHub URL. Terminal-friendly (copy-pasteable; URLs aren't clickable in serial/tmux/minimal ttys) and the subcommand does strictly more work than the URL alone (DMI auto-fill).

Real-hardware verified: `aegis-boot add Win11_25H2_English_x64_v2.iso /media/william/AEGIS_ISOS` refuses with exit 1, names `/dev/sda` directly in the ext4-reflash recipe; Alpine 209 MiB proceeds normally on the same vfat stick; `aegis-boot compat --submit` produces a correctly-encoded pre-filled URL from live DMI on a Framework Laptop 12th Gen.

### Bugs

- **Script safety guards** ([#138](https://github.com/aegis-boot/aegis-boot/issues/138) children) — two long-standing silent-failure paths in the build scripts now fail fast. `scripts/build-initramfs.sh` exits on `depmod` failure (was: logged a warning and continued, producing an image whose `modules.dep` still pointed at the original `.ko.zst` paths — storage modules would silently miss at boot). Set `AEGIS_ALLOW_MISSING_DEPMOD=1` to bypass. `scripts/mkusb.sh` now validates sgdisk-derived partition start sectors are non-empty, numeric, and non-zero before using them as `dd seek=` — an empty awk result yielded `seek=0`, silently overwriting the freshly-written GPT at sector 0.
- **OVMF SB detection fallback** ([#118](https://github.com/aegis-boot/aegis-boot/issues/118)) — `rescue-tui`'s `SecureBootStatus::detect()` now scans `/sys/firmware/efi/efivars` for any filename starting with `SecureBoot-` when the two upstream-spec paths (global-GUID and plain) miss. Handles OVMF firmware builds that publish the variable under a non-spec suffix — observed under QEMU+OVMF SecBoot shakedown where rescue-tui's header showed `SB:unknown` despite SB enforcing. Parallels the existing scan fallback in `aegis-cli doctor` (doctor.rs:371).

### Publishing prep

- **Crates.io metadata for the library trio** ([#51](https://github.com/aegis-boot/aegis-boot/issues/51)) — `iso-parser`, `iso-probe`, `kexec-loader` now carry the full `[package]` surface (`readme`, `documentation`, `homepage`, `keywords`, `categories`) plus per-crate README files. `iso-probe`'s path dep on `iso-parser` gained the required registry `version = "0.13"`. `cargo publish --dry-run -p iso-parser` and `-p kexec-loader` come back clean; `iso-probe`'s dry-run blocks on the unpublished-registry chicken-and-egg which is expected and resolved by the real publish ordering documented in `docs/RELEASE_CRATES.md`. Gate to actual publish remains v1.0.0-rc1 (real-hardware shakedown still pending).
- **Test flake fix** — `fetch::tests::default_cache_uses_xdg_cache_home` and `default_cache_falls_back_to_home_dot_cache` now serialize on a `Mutex` to avoid the process-global env-var race. Both tests pass across parallel runs.

### CI reliability

- **apt retry loop in `Dockerfile.locked`** — GitHub Actions runners periodically can't reach `archive.ubuntu.com:80` (observed repeatedly on reproducible-build and mkusb jobs; forces a manual `gh run rerun`). `Dockerfile.locked`'s package install step now wraps `apt-get update && apt-get install` in a 3-attempt retry loop with a 15s backoff plus `Acquire::Retries=5` in apt's own config. Up to ~60s of mirror blips are now absorbed silently; genuine "package doesn't exist" errors still fail fast (they fail identically on every attempt). Does not change the reproducibility guarantee: the `reproducible-build.yml` workflow verifies the `rescue-tui` binary hash, not the Dockerfile or image digest (see the workflow header comment).

### Operator experience

- **ISO pretty-name detection** ([#119](https://github.com/aegis-boot/aegis-boot/issues/119)) — `iso-parser` now reads `/etc/os-release` (`PRETTY_NAME`), falling back to `/lib/os-release`, `/usr/lib/os-release`, `/.disk/info` (Ubuntu/Debian convention), and `/etc/alpine-release` during ISO discovery. Populated into new `BootEntry.pretty_name` + `DiscoveredIso.pretty_name` fields (both `Option<String>`, `#[serde(default)]` for forward compat). `iso-probe::display_name()` helper returns `pretty_name` when present, falling back to `label`. rescue-tui's list view, Confirm preview, and Error pane now use it — operators see "Ubuntu 24.04.2 LTS (Noble Numbat)" or "Alpine Linux 3.20.3" instead of just the distribution family name. 11 new unit tests cover the parser (quoted/unquoted values, missing keys, multi-key files, file-priority ordering, empty-line skipping in `.disk/info`).
- **Two more `init` profiles** — `minimal` (alpine-only, ~200 MiB, fastest) and `server` (ubuntu-server + rocky + almalinux, ~6 GiB, enterprise RHEL + Ubuntu rescue triple). Operators can now pick `aegis-boot init --profile <panic-room|minimal|server>` to fit the target-environment shape. Every profile slug is enforced to be in the verified catalog at test time (`every_profile_slug_is_in_catalog`).
- **`aegis-boot init --profile panic-room`** ([#161](https://github.com/aegis-boot/aegis-boot/issues/161)) — one-command rescue stick. Composes `doctor → flash → fetch + add` for every slug in a named profile, producing a single attestation manifest spanning the whole run. Default `panic-room` profile ships three ISOs (Alpine 3.20, Ubuntu 24.04 Server, Rocky 9) covering ~5 GiB — fits on a 16 GB stick. Extracted `try_run` variants on `doctor`, `flash`, `fetch`, and `inventory::run_add` so the composition can branch on typed `Result` instead of opaque `ExitCode`. Flash gained `--yes` to skip the typed-confirmation prompt when invoked from `init`.
- **Stale issue triage ([#52](https://github.com/aegis-boot/aegis-boot/issues/52), [#122](https://github.com/aegis-boot/aegis-boot/issues/122), [#127](https://github.com/aegis-boot/aegis-boot/issues/127))** — closed three bugs/docs issues whose fixes had already shipped in v0.12.0 / v0.13.0 and were masquerading as open. Verified the fixes in source (Debian-layout marker gate, post-kexec handoff banner, CHANGELOG reproducibility caveat) before closing.

### Catalog + quality

- **Catalog curation policy** ([#154](https://github.com/aegis-boot/aegis-boot/issues/154)) — `docs/CATALOG_POLICY.md` formalizes the 5 inclusion criteria (HTTPS canonical URL, project-published signed SHA256SUMS, operator value, stable URL, honest SB posture), accepted categories, and the PR proposal process.
- **Weekly catalog URL revalidation** ([#155](https://github.com/aegis-boot/aegis-boot/issues/155)) — `scripts/catalog-revalidate.sh` + scheduled workflow checks every URL in the catalog via range-GET. Surfaced 25 broken URLs in the first run.
- **Catalog trimmed to 6 verified entries** ([#159](https://github.com/aegis-boot/aegis-boot/issues/159), closes [#156](https://github.com/aegis-boot/aegis-boot/issues/156)) — removed 12 entries with broken URLs (sourceforge rewrites, point-release rotation, wrong sig_url patterns). Keeping only entries where all three URLs verify green. Entries will be re-added per #156 as URLs are fixed.

### CI / distribution

- **Homebrew formula validated in CI** ([#157](https://github.com/aegis-boot/aegis-boot/issues/157)) — `brew audit + style + install + test` workflow runs on Formula changes and weekly. Formula ComponentsOrder fixed (`depends_on` before `on_linux`), `uses_from_macos "coreutils"` removed (not macOS-only).

### Roadmap

- **v1.2+ category-defining epic** ([#158](https://github.com/aegis-boot/aegis-boot/issues/158)) — 5 capabilities beyond the v1.0/v1.1 roadmap: FIDO2-backed operator identity, post-kexec verifier + TPM quote, Sigstore Rekor integration, ephemeral compute bootstrapping, automotive/coreboot rescue mode. Ranked by category-redefinition impact.

## [0.13.0] — 2026-04-16

The **best-in-class push** — what landed after v0.12.0 went out and the repo went public, working through epics #136 (operator + sysadmin reach), #137 (onboarding + discoverability), and #138 (quality gates).

### Operator surface area expansion (epic #136)

Five new `aegis-boot` subcommands. The CLI is now the single tool an operator needs from "I have a stick and an ISO" to "the target machine boots".

- **`aegis-boot doctor [--stick /dev/sdX]`** ([#141](https://github.com/aegis-boot/aegis-boot/issues/141)) — host + stick health check. Inspects OS, prerequisite tools (`dd`, `sudo`, `sgdisk`, `lsblk`, `curl`, `sha256sum`, `gpg` — [#146](https://github.com/aegis-boot/aegis-boot/issues/146)), Secure Boot state (mokutil + efivar fallback), removable drive enumeration, partition layout (asserts ESP + AEGIS_ISOS), AEGIS_ISOS contents (counts ISOs + sidecars). Reports a 0-100 score and a single `NEXT ACTION` line. PASS=10 / WARN=7 / FAIL=0; bands: 90+ EXCELLENT, 70+ OK, 40+ DEGRADED, <40 BROKEN.
- **`aegis-boot recommend [slug]`** ([#142](https://github.com/aegis-boot/aegis-boot/issues/142)) — curated catalog of 13 known-good ISOs (Ubuntu LTS server + desktop, Fedora 41, Debian 12, Alpine, Arch, NixOS, SystemRescue, GParted, Memtest86+, Clonezilla, Tails, Kali). Each entry carries the project's canonical URL + the URL of the project's signed `SHA256SUMS` (no checksum pinning in our catalog — it'd rot weekly; the project's own signing key is the trust anchor). SB column makes the unsigned-needs-MOK distinction visible up front. Slug resolution is exact-or-unique-prefix.
- **`aegis-boot fetch <slug>`** ([#145](https://github.com/aegis-boot/aegis-boot/issues/145)) — automates the recipe `recommend` prints. Downloads the ISO + signed `SHA256SUMS` + signature, runs `sha256sum -c`, runs `gpg --verify`. Four GPG verdicts: OK, UnknownKey (non-fatal — operator can review + import + retry), BadSignature (fatal), GpgMissing (fatal unless `--no-gpg`). Per-slug cache directory (`$XDG_CACHE_HOME/aegis-boot/<slug>/`); skips files already present so re-runs are cheap.
- **`aegis-boot attest list|show`** ([#147](https://github.com/aegis-boot/aegis-boot/issues/147)) — attestation receipts. Every `flash` writes a JSON manifest at `$XDG_DATA_HOME/aegis-boot/attestations/<disk-guid>-<ts>.json` capturing tool version, timestamp, operator, host kernel + SB state, target device + model + size + GUID, image SHA-256 + size. Schema v1, additive-evolution-friendly (unknown fields tolerated by parser). Operationalizes the "prove what's on the stick" trust narrative — the differentiator vs every other USB-imaging tool.
- **Append on `aegis-boot add`** ([#148](https://github.com/aegis-boot/aegis-boot/issues/148)) — every successful `add` appends an `IsoRecord` (filename, sha256, size, sidecars, timestamp) to the matching attestation. Lookup: mount path → owning device via `/proc/mounts` → strip partition suffix (handles `sda1`, `nvme0n1p3`, `mmcblk0p1`, `loop12p1` correctly) → disk GUID → newest matching manifest. Falls back to "most recent overall" with a warning when GUID can't be resolved.
- **`aegis-boot list` shows attestation summary** ([#149](https://github.com/aegis-boot/aegis-boot/issues/149)) — when listing ISOs, prints a header above the table: `flashed at` + `operator` + `ISOs added since flash` + `manifest path`. Closes the attestation loop: flash → add → list all reference the same chain.

### Distribution + onboarding (epic #137)

- **GitHub Releases automation with cosign-signed binaries** ([#143](https://github.com/aegis-boot/aegis-boot/issues/143)) — each release now ships a static-musl `aegis-boot-x86_64-linux` binary (~855 KiB), the existing `rescue-tui`, `initramfs.cpio.gz`, `sbom.cdx.json`, and an aggregated `SHA256SUMS` — every artifact accompanied by a Sigstore cosign keyless signature (`.sig` + `.pem`). The signing certificate is bound to this repo's `release.yml` workflow at the tag's ref, so verifying confirms the artifact came from *this* repo's release, not a copycat. Backfilled v0.12.0's release with all 16 cosign-signed assets.
- **`scripts/install.sh` cosign-verified one-liner** ([#144](https://github.com/aegis-boot/aegis-boot/issues/144)) — `curl -sSL https://raw.githubusercontent.com/aegis-boot/aegis-boot/main/scripts/install.sh | sh` downloads the latest release's binary, verifies its cosign signature, installs to `/usr/local/bin` (root) or `~/.local/bin` (non-root). POSIX-portable, truncation-safe (wrapped in `main()` called at end-of-file), fails closed if cosign is missing.
- **Homebrew tap** ([#150](https://github.com/aegis-boot/aegis-boot/issues/150)) — `Formula/aegis-boot.rb` makes this repo a Brew tap. Operators install with `brew tap aegis-boot/aegis-boot https://github.com/aegis-boot/aegis-boot && brew install aegis-boot`. Linux x86_64 only today.
- **Auto-bump Brew formula on tag push** ([#151](https://github.com/aegis-boot/aegis-boot/issues/151)) — release workflow now updates `Formula/aegis-boot.rb` automatically after each release. No more manual maintenance.
- **`docs/HARDWARE_COMPAT.md`** ([#146](https://github.com/aegis-boot/aegis-boot/issues/146)) — community-curated table of validated machines, seeded with the v0.12.0 [#109](https://github.com/aegis-boot/aegis-boot/issues/109) shakedown + the QEMU/OVMF reference environment.
- **`docs/RELEASE_NOTES_FOOTER.md`** — appended to every release's notes; gives operators a copy-pasteable cosign verify-blob recipe so trust is testable.

### Quality gates (epic #138)

- **`#![forbid(unsafe_code)]` on iso-parser** ([#140](https://github.com/aegis-boot/aegis-boot/issues/140)) — kexec-loader remains the only crate with `unsafe`, which is its purpose. Tightens the trust surface for the upcoming crates.io publish ([#51](https://github.com/aegis-boot/aegis-boot/issues/51)).

### Roadmap + governance

- 4 epics filed: [#136](https://github.com/aegis-boot/aegis-boot/issues/136), [#137](https://github.com/aegis-boot/aegis-boot/issues/137), [#138](https://github.com/aegis-boot/aegis-boot/issues/138), [#139](https://github.com/aegis-boot/aegis-boot/issues/139) (post-v1.0 fleet+trust depth).
- 2 milestones: v1.0.0 + v1.1.0.
- Cleanup: closed [#40](https://github.com/aegis-boot/aegis-boot/issues/40) (superseded by [#123](https://github.com/aegis-boot/aegis-boot/issues/123)), and 4 already-fixed bugs ([#113](https://github.com/aegis-boot/aegis-boot/issues/113), [#115](https://github.com/aegis-boot/aegis-boot/issues/115), [#116](https://github.com/aegis-boot/aegis-boot/issues/116), [#117](https://github.com/aegis-boot/aegis-boot/issues/117)).

### Tests

186 workspace tests, clippy clean (was 140 at v0.12.0). +46 in aegis-cli covering the new modules (catalog invariants, fetch helpers, doctor scoring, attestation roundtrip + lookup arithmetic, partition-suffix stripping incl. the `loop12` regression case).

### Repo went public

`gh repo edit --visibility public` after a clean 194-commit gitleaks scan. `https://github.com/aegis-boot/aegis-boot` is now indexable, taggable, forkable, contributable.

## [0.12.0] — 2026-04-16

**The operator-journey release.** Transforms aegis-boot from a developer tool to an operator tool. Synthesis of two full shakedowns on real USB hardware (Alpine refusal + Ubuntu success) and a `nexus-agents ux_expert` alignment review that surfaced #131 as a v1.0 blocker.

### Headline — aegis-boot CLI (#124, #125)

New binary `aegis-boot` (crate `aegis-cli`) with three subcommands:

- **`aegis-boot flash [device]`** — 3-step guided USB writer. Auto-detects removable drives (sysfs-based, shows model + capacity + partition count), typed `flash` confirmation, builds the image + dd's with progress, syncs, partprobes.
- **`aegis-boot list [device]`** — auto-finds the mounted `AEGIS_ISOS` partition (via `/proc/mounts`) or accepts `/dev/sdX` / path argument. Prints every `.iso` with its sidecar verification status (`✓ sha256`, `✓ minisig`).
- **`aegis-boot add <iso> [device]`** — copies an ISO onto the stick with free-space check + automatic sidecar detection (`.sha256`, `.SHA256SUMS`, `.minisig`). Warns when no sidecars found.

Verified end-to-end against the live shakedown stick: instantly finds both Alpine and Ubuntu ISOs.

### Operator-journey UX polish

- **Post-kexec handoff banner** (#127) — clear-screen + 9-line "Booting ... screen may go blank briefly" before `kexec_file_load`. No more silent black screen between TUI and new kernel.
- **Installer-vs-live warning strip** (#131, **v1.0 blocker fix**) — yellow 2-line warning on Confirm when the ISO filename matches one of 23 installer-bearing patterns (`live-server`, `netinst`, Fedora DVD, Anaconda, Windows, etc.). Prevents false confidence from GREEN verdicts on ISOs that can erase the host's disk.
- **Unsigned kernel guidance** (#126) — `docs/UNSIGNED_KERNEL.md` documents the full operator choice tree (distro-signed ISO vs MOK enrollment vs what NOT to do). Error screen's no-key remedy rewritten from one vague sentence to a concrete two-path guide. Relaxed kexec mode investigated and `not shipping` decision documented — `kexec_load(2)` is blocked by the same SB lockdown that blocks `kexec_file_load`.

### Real-hardware validation (#109 closed)

First real-USB shakedown on a SanDisk Cruzer 32GB via QEMU USB-passthrough with OVMF Secure Boot enforcing:

- **Alpine 3.20.3** (unsigned kernel) → `errno 61 (ENODATA)` — correct refusal per KEXEC_SIG policy
- **Ubuntu 24.04.2 LTS** (Canonical-signed) → `kexec_core: Starting new kernel` — **successful handoff**

6 bugs found + fixed during shakedown: #112 console order, #113 secondary mount noise, #115 tty0 alt-screen, #116 Alpine misclassification (2 rounds), #117 double-scan, #122 Debian false-match.

### Tests

140 workspace tests, clippy clean. No net-new tests for this release — CLI surface is interactive; render changes are visual; both categories validated manually in shakedown.

### Known v1.0.0 gaps (from ux_expert alignment review)

- [#132](https://github.com/aegis-boot/aegis-boot/issues/132) Real-HW E2E test of last-booted persistence (currently unit-tested only)
- [#123](https://github.com/aegis-boot/aegis-boot/issues/123) Mac/Windows `aegis-boot flash` (Linux-only today)
- [#51](https://github.com/aegis-boot/aegis-boot/issues/51) Framework / ThinkPad / Dell real-boot on the hardware itself (today: QEMU passthrough of real USB — close but not full)

## [0.11.0] — 2026-04-15

**Accessibility + design-review cleanup release.**

### Headline — text-mode accessibility (#104)

- **`AEGIS_A11Y=text` / `TERM=dumb` activates a plain-text mode.** ratatui's alternate-screen rendering is invisible to screen readers (Orca, NVDA) and braille displays (via brltty). Text mode prints a numbered menu to stdout, reads a line from stdin, and never touches raw mode / alt-screen / ANSI — usable from serial consoles, 40-col terminals, and accessibility tools out of the box.
- **Full trust-challenge + rescue-shell parity.** The text-mode Confirm flow prints the one-frame evidence block, asks y/N for GREEN verdicts or requires typing `boot` for YELLOW/GRAY degraded-trust verdicts (same gate as the TUI), hard-blocks RED.
- **`ANN:` announcements on stderr** on every menu paint and state transition. brltty / speakup can mirror to braille / speech — same pattern `dialog(1)` uses.

### Design-review follow-ups ([#101](https://github.com/aegis-boot/aegis-boot/issues/101), [#102](https://github.com/aegis-boot/aegis-boot/issues/102), [#103](https://github.com/aegis-boot/aegis-boot/issues/103))

- **Compacted Confirm screen** — Kernel+Initrd merged onto one `Boot:` line; Checksum+Signature merged onto one `Trust:` line. Net −2 rows so the verdict stays above the fold on 24-row terminals.
- **Filter-mode info bar is unmistakable** — reversed-style `FILTER` label in `theme.warning`, bold filter text, `SLOW_BLINK` caret span. Previously the only cue was a trailing `_`.
- **`q` on Confirm returns to List** (not ConfirmQuit). Operators meaning "quit this screen" no longer get the reboot-the-machine prompt. ConfirmQuit still reachable from List.

### Tests

140 workspace tests (unchanged — all shipped changes are render- or branch-level without new state transitions).

### Deferred

- Text-mode process-level integration tests (filed as follow-up if requested).
- Text-mode filter / sort / verify-now (filed if real operators ask — the assistive-tech surface area is usually "pick an ISO, boot it").

## [0.10.1] — 2026-04-15

**Brand identity + design-review fixes.** Delivers [#76](https://github.com/aegis-boot/aegis-boot/issues/76) (brand identity spec produced by the nexus-agents `ux_expert`) and the three concrete fixes from the expert's subsequent self-critique.

### Brand identity (#76)

- **`assets/brand/`** — master SVG + monochrome variant of the shield-with-keyhole logo; ASCII renders (full 10-line README hero + compact 3-line TUI); `palette.css` with oklch + hex; `BRAND.md` usage guidelines.
- **README hero block** — shield ASCII + tagline + license/release/CI badges.
- **Tagline:** *Signed boot. Any ISO. Your keys.*
- **`aegis` theme** — fifth named palette alongside default / monochrome / high-contrast / okabe-ito. Steel-blue primary (`#3B82F6`), emerald success, amber warning, vermilion error. Verified under deuteranopia/protanopia; distinct from Ubuntu/Fedora/Arch distro palettes.
- **TUI header** gains the `◆` shield mark in brand primary plus the tagline in dim italic.

### Design-review fixes (#76 self-critique)

- **Header degrades gracefully on narrow terminals.** Previously truncated mid-word ("Signed boot. Any ISO. Yo"). Now span-chain is gated on `area.width`: ≥90 = full; ≥70 drops tagline; ≥50 drops TPM; <50 keeps only mark + name + version. Shield mark always survives.
- **TrustChallenge mismatch feedback.** Typed characters `≥4` that don't equal `boot` render in error colour + bold. Silent-fail on a security gate was trainable toward muscle-memory mashing.
- **TPM status colour reflects TPM state.** Previously hardcoded to green regardless; `TPM:none` now renders amber (warning). A green "none" was a lie.

### Deferred to follow-up issues

- [#101](https://github.com/aegis-boot/aegis-boot/issues/101) Confirm info density — verdict can scroll off 24-row terminals
- [#102](https://github.com/aegis-boot/aegis-boot/issues/102) Filter-mode entry visual subtlety
- [#103](https://github.com/aegis-boot/aegis-boot/issues/103) `q` on Confirm opens ConfirmQuit (should be Esc-back equivalent)
- [#104](https://github.com/aegis-boot/aegis-boot/issues/104) `AEGIS_A11Y=text` screen-reader / braille mode

### Tests

Workspace tests 140 (+1 for the aegis theme; no test-count change from design-review fixes since they're render-only).

## [0.10.0] — 2026-04-15

**Rescue + trust challenge + evidence release.** Implements the three biggest deferred items from the UX epic parent ([#85](https://github.com/aegis-boot/aegis-boot/issues/85)) and its trust/a11y children ([#92](https://github.com/aegis-boot/aegis-boot/issues/92), [#93](https://github.com/aegis-boot/aegis-boot/issues/93)).

### Headline

- **Always-present rescue-shell entry** ([#90](https://github.com/aegis-boot/aegis-boot/issues/90)). The List screen now always ends with `[#] rescue shell (busybox)` — visible even when zero ISOs are discovered. Selecting it exits rescue-tui with sentinel code 42; `/init` recognizes the code and drops cleanly to `/bin/sh`. Previously "no ISOs found" was a dead end. Pattern: rEFInd tools row, Endless OS recovery.
- **Typed trust confirmation on degraded verdicts** ([#93](https://github.com/aegis-boot/aegis-boot/issues/93)). Pressing Enter on a YELLOW (untrusted signer) or GRAY (no verification material) Confirm screen now opens a challenge that requires typing `boot` exactly. GREEN verdicts skip it; RED verdicts stay hard-blocked by #55. Pattern: SSH first-connect, HSTS, Gatekeeper.
- **memtest-style one-frame error screen** ([#92](https://github.com/aegis-boot/aegis-boot/issues/92)). kexec-failure Error screen now renders a complete evidence block: version, SB/TPM state, ISO path + size + distro, trust verdict, effective cmdline, and the sha256 digest that was fed to PCR 12. One screen photograph = one complete bug report. Pattern: memtest86+.
- **F10 save-log to AEGIS_ISOS** ([#92](https://github.com/aegis-boot/aegis-boot/issues/92)). From the Error screen, F10 serializes the evidence block to `/run/media/aegis-isos/aegis-log-<unix_ts>.txt` (or `/tmp` fallback). Operator can pull it off the stick from any other machine post-reboot. Pattern: rEFInd's refind.log on ESP.

### Breaking

- The `AEGIS_ISOS` partition is now written to on Error-screen F10. If the partition is mounted read-only or full, the save fails silently with a logged warning. No behavior change unless the operator uses F10.
- `Screen::Error` variant gains `return_to: usize` (landed earlier in v0.8.0; flagged here because it's now also used by the evidence panel).
- rescue-tui's `main()` return type changed from `Result<(), _>` to `Result<u8, _>` so run() can propagate the shell-drop sentinel code. External callers of the binary should not be affected — it's an internal refactor.

### Tests

- v0.9.0: 126
- v0.10.0: 139 (+13: rescue-shell entries 5, trust-challenge 5, evidence 2, +1 misc)

### Deferred

- #92 brltty + speakup in initramfs, TERM=dumb fallback, --selftest mode
- #93 signer key fingerprint display (blocked on minisign-verify API)
- #91 distro grouping / submenus (low priority — `s` sort covers it)

### Verified

- `cargo clippy --workspace --all-targets -- -D warnings` clean
- `cargo test --workspace` 139 / 139 green
- `qemu-loaded-stick.sh` boot still works end-to-end with Alpine 3.20

## [0.9.0] — 2026-04-15

**Trust UX + verify-now + a11y polish.** Synthesis from two more parallel-agent surveys: trust/attestation UX (Firefox certs, OpenSSH first-connect, GPG/minisign, Gatekeeper, TPM eventlog, Cosign, Android Verified Boot) and accessibility/field-ops (brltty, speakup, Debian-installer a11y, GRUB, systemrescue, Clonezilla, memtest86+, rEFInd log, UEFI shell). Epics filed as [#92](https://github.com/aegis-boot/aegis-boot/issues/92) and [#93](https://github.com/aegis-boot/aegis-boot/issues/93).

### Trust UX on the Confirm screen (#93)

- **Android VB-style coloured verdict line** at the top of Confirm. One of GREEN / YELLOW / RED / GRAY with a one-sentence reason. Colored AND text-labeled so monochrome themes still read.
- **Measured-bytes digest preview** — shows the exact `sha256(iso_path || 0x00 || cmdline)` that will be extended into PCR 12, truncated to 16 hex chars.
- **Eventlog-style audit line** to the tracing stream before kexec with iso_path, cmdline, and full 64-char measurement hex.

### Verify-now action (#89)

- **`v` on List / Confirm re-runs SHA-256 against the selected ISO** with a live progress bar. Worker thread + `mpsc`, cancellable via Esc. Pattern: Ventoy F4.

### Accessibility polish (#92 partial)

- **Okabe-Ito colorblind-safe theme** — fourth named palette. Aliases `okabe-ito` / `colorblind` / `cb`. Deuteranopia- and protanopia-safe.
- **SysRq emergency cheatsheet** in the `?` help overlay. Alt+SysRq+b/s/e documented.
- **Theme list in help overlay** — four themes + the `AEGIS_THEME` env var.

### Tests

- v0.8.0: 121
- v0.9.0: 126 (+5)

### Deferred

- #92 brltty + speakup in initramfs, dual-sink log capture to ESP, memtest-style one-frame error screen, TERM=dumb fallback
- #93 signer key fingerprint display, typed confirmation on degraded trust
- #90 always-present rescue-shell entry
- #91 distro grouping / submenus

## [0.8.0] — 2026-04-15

**UX overhaul release** ([#85](https://github.com/aegis-boot/aegis-boot/issues/85)). Synthesis of a parallel-agent survey of best-in-class boot pickers (Ventoy, rEFInd, systemd-boot, GRUB2, Apple Option-key, Lenovo F12) and TUI applications (lazygit, ranger, fzf, k9s, helix, dialog). The rescue-tui is now substantially more discoverable, navigable, and trustworthy at a glance.

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

**Documentation accuracy patch** ([#78](https://github.com/aegis-boot/aegis-boot/issues/78)). No code changes.

- README.md: full rewrite. Removed false "skeleton-only" status for rescue-tui / iso-probe / kexec-loader (those crates now hold ~4000 LOC and 108 tests across 7 releases). Removed wrong "Rust 1.75.0" claim (workspace pin is 1.85.0). Removed wrong "EDK II stable202311" claim (the Dockerfile and BUILDING.md both explicitly state EDK II is not used). Added quickstart, current component matrix, doc index.
- CHANGELOG: v0.5.0 section's "byte-reproducible bootable disk image" claim corrected — only `rescue-tui` is verified reproducible; the disk image embeds host-installed shim/grub/kernel. v0.7.0 headline reframed from "Real-hardware-ready" to "Storage-module-complete" since real hardware has not been validated.
- docs/LOCAL_TESTING.md: documented the v0.7.0 `--attach {virtio,sata,usb}` flag with examples and a capability table.
- docs/USB_LAYOUT.md: added a section listing the storage modules shipped in the initramfs as of v0.7.0 and the QEMU-only validation status.
- crates/iso-parser/Cargo.toml: bumped to 0.7.1 (was stuck at 0.1.0 — drift from the rest of the workspace) and switched to workspace `edition` / `rust-version` inheritance.
- New: `docs/content-audit.md` records each documentation accuracy audit so we can re-audit on a cadence.

## [0.7.0] — 2026-04-15

**Storage-module-complete release.** Adds the kernel modules real hardware needs (AHCI, NVMe, USB-storage, UAS) so rescue-tui can in principle see a USB stick or internal disk on a physical machine. **Real-hardware boot has not yet been validated** — that's gated on a Framework / ThinkPad / Dell shakedown ([#51](https://github.com/aegis-boot/aegis-boot/issues/51)) and gates v1.0.0. v0.6.x fixed the QEMU+virtio path; v0.7.0 is the foundation for the next step.

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

Tracks progress of the [v0.3.0 epic (#29)](https://github.com/aegis-boot/aegis-boot/issues/29). Raises the security floor (real cryptographic authentication) and the UX floor (last-choice persistence, explicit Windows-not-bootable diagnostic).

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

Tracks progress of the [v0.2.0 epic (#24)](https://github.com/aegis-boot/aegis-boot/issues/24). Closes must-haves for:

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
- **`iso_probe::lookup_quirks()`** returns an empty list for every distribution. Real population tracked in [#6](https://github.com/aegis-boot/aegis-boot/issues/6). Callers must not treat empty as "safe."
- **kexec handoff** is unit-tested via errno classification but not yet end-to-end exercised with a signed target ISO.
