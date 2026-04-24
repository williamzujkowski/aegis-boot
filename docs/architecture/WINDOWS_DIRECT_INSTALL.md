# Windows direct-install architecture

**Epic:** [#419](https://github.com/aegis-boot/aegis-boot/issues/419) (closed 2026-04-24)
**Status:** Shipped. `aegis-boot flash --direct-install <drive>` is supported on Windows 10+ hosts.
**Authors:** Maintainer + Claude Opus 4.7

This document captures the design rationale behind the Windows direct-install adapter. It exists so a future contributor touching `crates/aegis-cli/src/windows_direct_install/` knows which decisions are load-bearing and which were convenience choices they can revisit. Reading the code alone tells you *what*; this tells you *why*.

For operator-facing usage, see [`docs/CLI.md § aegis-boot flash`](../CLI.md#aegis-boot-flash).

## 1. Scope

The adapter produces an `aegis-boot`-formatted USB stick on Windows hosts with identical on-disk layout to the Linux path (`AEGIS_ESP` FAT32 partition with the signed chain + `AEGIS_ISOS` exFAT data partition). An operator on Windows can:

```
aegis-boot flash --direct-install 1 --out-dir ./out --yes
```

…and get the same result a Linux operator gets from `sudo aegis-boot flash /dev/sdc --direct-install`.

The Windows path intentionally reuses Linux-compatible labels + filesystem types so a stick flashed on one OS is bit-identical (modulo filesystem serial numbers) to one flashed on the other — operators can hand a stick between maintainers without reflashing.

## 2. Module layout

```
crates/aegis-cli/src/windows_direct_install/
├── mod.rs                  — module index
├── partition.rs            — #447 Phase 1: diskpart harness
├── format.rs               — #448 Phase 2: Format-Volume wrapper
├── raw_write.rs            — #449/#484 Phase 3: windows-rs CreateFileW + FSCTL + WriteFile
├── preflight.rs            — #450 Phase 4: elevation + BitLocker detection
├── pipeline.rs             — #483/#496 composer (PhaseRunner trait + abort-cascade)
├── source_resolution.rs    — #497 piece 2: resolve 6 chain files from out_dir + env overrides
├── drive_enumeration.rs    — #497 piece 3: Get-Disk JSON parser + filter
└── flash_dispatcher.rs     — #497 piece 4: CLI dispatch (parse drive arg → compose → run)
```

The split into four *Phase* modules (`partition`, `format`, `raw_write`, `preflight`) and three *integration* modules (`pipeline`, `source_resolution`, `drive_enumeration`, `flash_dispatcher`) mirrors the #419 epic decomposition. Each Phase module has a single narrow responsibility + is separately unit-tested on Linux via pure-fn builders before the Windows subprocess wrappers are invoked.

## 3. Key design decisions

### 3.1 `windows-rs`, not `.NET FileStream`

Win11 prototyping on 2026-04-23 validated `[System.IO.FileStream]` with direct-I/O flags works end-to-end. We chose `windows-rs` anyway because:

- **Deterministic behavior.** Managed `FileStream` hides the flush-at-end stall; when `FILE_FLAG_NO_BUFFERING` is set, Windows defers writes until `Close()` and the flush can take tens of seconds on a slow USB stick. Operators see a progress bar pinned at 99% then a long silence. The `windows-rs` path `WriteFile`s in sector-aligned chunks with deterministic pacing.
- **No PowerShell-per-call.** `FileStream` spawns a PowerShell interpreter per invocation. On a 2 GiB image that's 1000s of process spawns. `windows-rs` is a direct FFI binding.
- **Bounded dep.** The `windows` crate with exactly 5 features (`Win32_Foundation`, `Win32_Security`, `Win32_Storage_FileSystem`, `Win32_System_IO`, `Win32_System_Ioctl`) is a bounded, audited surface vs. pulling in PowerShell Core (hundreds of megabytes).

### 3.2 `unsafe_code = "deny"` with narrow `#[allow]`

The workspace enforces `unsafe_code = "deny"`. Each syscall site in `raw_write.rs` opts out with a narrow `#[allow(unsafe_code)]` annotation + a `// nosemgrep:` tag + a documented safety invariant. Same pattern as `kexec-loader`'s Linux-side `libc::syscall` calls.

This is deliberate: `forbid(unsafe_code)` would refuse any opt-out, which means we can't have raw-disk Win32 calls at all. `deny` lets us opt in narrowly, per function, with reviewer-visible justification.

### 3.3 `PhaseRunner` trait for composer testability

`pipeline::run(runner: &dyn PhaseRunner, plan: &DirectInstallPlan)` dispatches all 5 phase calls through a trait. The production `WindowsPhaseRunner` wires each method to its real subprocess / syscall implementation. Tests supply a mock.

This lets 13 unit tests exercise the composer's abort-cascade + per-stage timing logic without needing a Windows VM. The composer logic — "fail at stage N, skip N+1..M, but record timings for stages that ran" — is the most bug-prone piece of the whole adapter. Testing it on Linux without a Windows host was a specific requirement.

### 3.4 `DirectInstallPlan` as a frozen input

The pipeline takes a fully-populated `DirectInstallPlan { physical_drive, sources }`. It doesn't enumerate drives, doesn't resolve source paths, doesn't prompt. That lets:

- `flash_dispatcher.rs` own all the operator-interactive parts (parse drive arg, handle missing-drive case, print candidate list).
- Tests cover every abort branch without needing env var / file-system setup.
- Future callers (e.g. an MSI installer) can compose their own `DirectInstallPlan` from UI state rather than environment variables.

### 3.5 Source resolution: env var overrides per-file, not a single `--chain-dir`

`source_resolution::build_staging_sources(out_dir)` looks for 6 files under `out_dir` with fixed default names (`shimx64.efi.signed` etc.) matching `scripts/mkusb.sh`'s output. Each file can be individually overridden via a per-file env var (`AEGIS_SHIM_SRC`, `AEGIS_GRUB_SRC`, etc.).

Alternative: a single `AEGIS_CHAIN_DIR` pointing at any dir with any names. Rejected because:

- Per-file overrides let an operator pull the shim from `C:\Windows\System32\...` (a Microsoft-signed path) while the kernel + initramfs come from the aegis-boot out-dir. Single-dir forces copy-everything-first.
- Missing-file errors can name the specific file by both its default name AND the env var that would have rescued it. A single dir override would give "missing file X" with no clear recovery path.
- The `mkusb.sh` default filenames (lowercase, `.signed` suffix) are already in operator muscle memory. Preserving them meant the developer who built the chain on Linux runs direct-install on Windows against the same directory without renaming anything.

### 3.6 No interactive drive-selection prompt

`flash_dispatcher::run_direct_install` does NOT prompt for a drive choice. If `explicit_dev` is omitted, it prints the candidate list and exits with a re-run hint:

```
aegis-boot flash --direct-install: no drive specified. Candidates:
  PhysicalDrive1  SanDisk Cruzer            8.0 GiB  [RAW]
Re-run with an explicit drive argument: `aegis-boot flash --direct-install <N>`
```

Reason: the common invocation path is WinRM / SSH-from-Linux (e.g. a CI harness, a tech in a rescue session using `ssh admin@windows-box`). Those sessions often have closed stdin. An interactive prompt silently hangs. A clear "re-run with arg" message surfaces the problem immediately.

### 3.7 `--yes` is mandatory on Windows direct-install

Linux direct-install has a typed-confirmation (`flash` literal) step. Windows direct-install skips that (destructive on Windows can't be undone by "I made a typo, CTRL-C"; diskpart's clean is immediate) and instead requires `--yes` on the command line.

This matches the operator expectation: if you typed `aegis-boot flash --direct-install 1 --yes`, you meant it. If you omitted `--yes`, you see "destructive action will destroy ALL partitions on PhysicalDrive1 — re-run with --yes" and exit 2.

### 3.8 `#[repr(C, align(8))]` on the readback buffer

`raw_write::sys::volume_backs_physical_drive` queries `IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS` and reads back a `VOLUME_DISK_EXTENTS` struct via pointer reinterpret. The first implementation used `[u8; 256]` — which has 1-byte alignment while `VOLUME_DISK_EXTENTS` needs 8-byte alignment. Technically UB on the reinterpret (practically fine on x86-64 but the contract is undefined).

The fix (during #509 clippy-strict promotion): wrap in a `#[repr(C, align(8))]` struct:

```rust
#[repr(C, align(8))]
struct AlignedExtentBuf([u8; 256]);
```

Alternative would've been `[u64; 32]` for inherent 8-alignment — rejected because it forced u8 ↔ u64 casts at every access point.

### 3.9 CI coverage — native + cross-compile

The `windows-cargo-check.yml` workflow runs on a `windows-2022` GitHub runner:

- `cargo check -p aegis-bootctl --all-targets` (MSVC toolchain) — catches windows-rs API drift the GNU cross-compile from Linux misses.
- `cargo test -p aegis-bootctl --locked` — runs all 599 tests on real Windows.
- `cargo clippy -p aegis-bootctl --all-targets -- -D warnings` — strict after #509 cleared the 27-finding backlog.

The existing `aegis-cli` Linux job ALSO does `cargo check -p aegis-bootctl --target x86_64-pc-windows-gnu --all-targets` as a faster pre-gate that catches type errors before the Windows runner starts.

## 4. What's NOT in the adapter

- **No raw-disk write against a real physical drive in CI.** Would need a VHD round-trip harness. Tracked as the remaining work on #420.
- **No attestation manifest writing.** The Linux flash path writes a signed JSON manifest onto the ESP; the Windows equivalent needs signing-key lifecycle for non-Linux hosts, which isn't designed yet.
- **No `--dry-run` support.** The typed `Plan` shape used for Linux `--dry-run` is Linux-device-path-shaped today. A Windows-compatible Plan variant is a follow-up.
- **No interactive drive prompt.** Intentional (see §3.6).
- **No automatic signed-chain download.** Tracked in [#417](https://github.com/aegis-boot/aegis-boot/issues/417) — the Rufus `DownloadSignedFile` pattern.

## 5. Validation summary

Validated on a Win11 VM (QEMU SATA 2 GiB scratch disk as `\\.\PhysicalDrive1`) during #484 wiring:

```
test windows_direct_install::raw_write::tests::raw_write_roundtrip_on_scratch_disk ... ok
```

Unit test coverage across the adapter:

| Module                | Tests |
| --------------------- | ----- |
| `raw_write`           | 18    |
| `pipeline`            | 13    |
| `source_resolution`   | 10    |
| `drive_enumeration`   | 14    |
| `flash_dispatcher`    | 15    |
| **Total (new)**       | **70** |

All 70 tests run on Linux via dependency injection. The one Windows-only integration test fires on the windows-2022 runner via `windows-cargo-check.yml`.

## 6. Trail of PRs

The adapter shipped over ~24 hours in 2026-04 across these PRs:

| PR | Issue | Content |
| -- | ----- | ------- |
| #451 | #447 | Phase 1 — diskpart harness |
| #452 | #448 | Phase 2 — Format-Volume wrapper |
| #453 | #450 | Phase 4 — elevation + BitLocker detection |
| #482 | #449 | Phase 3 scaffold — windows-rs dep + pure-fn core |
| #493 | #484 | Phase 3 wiring — CreateFileW + FSCTL + WriteFile |
| #496 | #483 (partial) | Pipeline composer + PhaseRunner trait |
| #498 | #497.2 | Source resolver |
| #499 | #497.3 | Drive enumeration |
| #500 | #497 / #483 / #419 | CLI dispatcher (closes epic) |
| #501 | #420 (stub) | Windows CI gate |
| #509 | — | Strict clippy + alignment-cast UB fix |

Read chronologically those PRs tell the full story of how the adapter was designed, validated, and hardened. This document is the summary of their collective decision record.
