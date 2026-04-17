# Changelog

All notable changes to aegis-boot are recorded here. Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). This project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Hardware coverage loop (epics [#137](https://github.com/williamzujkowski/aegis-boot/issues/137) + [#136](https://github.com/williamzujkowski/aegis-boot/issues/136))

- **`aegis-boot compat` subcommand** ([#192](https://github.com/williamzujkowski/aegis-boot/pull/192)) — in-binary `COMPAT_DB` mirroring `docs/HARDWARE_COMPAT.md`; `aegis-boot compat [query]` fuzzy-matches vendor/model, `aegis-boot compat --json` emits a stable `schema_version=1` envelope. Seed data is verified-outcomes-only (no speculation) per the doc's policy.
- **Dedicated `hardware-report.yml` issue template** ([#193](https://github.com/williamzujkowski/aegis-boot/pull/193)) — structured GitHub form whose fields map 1:1 to `COMPAT_DB` columns. Replaces the generic bug template that `aegis-boot compat` miss-path and `HARDWARE_COMPAT.md` used to point at.
- **`doctor` machine identity row** ([#194](https://github.com/williamzujkowski/aegis-boot/pull/194)) — reads `/sys/class/dmi/id/*` (non-privileged) and prints the operator's vendor + model + firmware so filing a hardware report is copy-paste. Filters common OEM placeholders (`To Be Filled By O.E.M.`, etc.). Linux-only; verdict is Pass or Skip.
- **`doctor` compat-DB cross-check** ([#195](https://github.com/williamzujkowski/aegis-boot/pull/195)) — after the identity row, `doctor` runs the DMI string through `find_entry(COMPAT_DB)` and emits a `compat DB coverage` row. Pass when documented, Warn when not (with the report URL inlined into the detail line).
- **Guided MOK enrollment on errno 61** ([#202](https://github.com/williamzujkowski/aegis-boot/pull/202), closes the child in [#136](https://github.com/williamzujkowski/aegis-boot/issues/136)) — rescue-tui's `SignatureRejected` remedy is now three explicit steps (STEP 1/3 `sudo mokutil --import`, STEP 2/3 describing the blue-on-black "Perform MOK management" screen, STEP 3/3 with firmware boot-menu keys for the top 5 vendors). Replaces a single dense paragraph; no new screens required.
- **`compat --my-machine`** ([#206](https://github.com/williamzujkowski/aegis-boot/pull/206)) — auto-fills the lookup query from `/sys/class/dmi/id/*` for single-purpose "is MY machine documented?" flow. Symmetric with `doctor`'s compat-DB cross-check but as a dedicated subcommand. Shares `read_dmi_field` + `dmi_product_label` with doctor so both surfaces agree on the machine label. Exit codes: 0 match, 1 DB miss, 2 DMI unavailable OR mixed with explicit query.

### Scriptable surfaces

- **Uniform `--json` across every read-mostly subcommand** ([#191](https://github.com/williamzujkowski/aegis-boot/pull/191)) — `update --json` emits an eligibility envelope + host-chain (sha256 per slot) or a reason-for-ineligible; `recommend --json [slug]` emits the full catalog or a single entry. Completes the sweep alongside prior `--json` additions to `doctor`, `list`, `attest list`, `attest show`, `verify`, `fetch --dry-run`. Every surface shares the `schema_version: 1` envelope and the `doctor::json_escape` helper.
- **`aegis-boot --version --json`** ([#205](https://github.com/williamzujkowski/aegis-boot/pull/205)) — completes the --json sweep across every CLI output path including the version surface. Emits `{ schema_version, tool, version }` so scripted consumers (install one-liner assertions, Homebrew formula tests, ansible-verified installs) can parse the version without regex on the human string.
- **`aegis-boot completions bash | zsh`** ([#207](https://github.com/williamzujkowski/aegis-boot/pull/207)) — hand-rolled completion script generator for the 13-subcommand surface. Completes top-level subcommands, catalog slugs on `recommend`/`fetch` (via existing `recommend --slugs-only`), compat vendor tokens via `jq` graceful-fallback, shared flag sets on `doctor`/`list`/`attest`/`verify`/`update`, device paths on `init`/`flash`. zsh uses bashcompinit shim.

### Quality gates (epic [#138](https://github.com/williamzujkowski/aegis-boot/issues/138) — closed)

- **`iso-parser` test-mock hazards closed** ([#196](https://github.com/williamzujkowski/aegis-boot/pull/196)) — `MockIsoEnvironment::mount_iso` / `unmount` no longer `.lock().unwrap()` (would cascade-fail every test after a panicked one); use `PoisonError::into_inner` for poison recovery. `MockIsoEnvironment::metadata` no longer returns `std::fs::metadata(std::env::temp_dir())` for any known path (silently validating size/mtime assertions against `/tmp`); fails closed with `ErrorKind::Unsupported`.
- **`unwrap_used` / `expect_used` = deny on remaining three crates** ([#197](https://github.com/williamzujkowski/aegis-boot/pull/197)) — `aegis-fitness`, `iso-probe`, and `rescue-tui` had per-crate overrides at `warn` from before the workspace tightening landed. Audit found zero bare `.unwrap()`/`.expect(...)` in production code; tightening to `deny` is a pure safety-posture win. All crates now enforce the workspace default.

### Infrastructure

- **`qemu-usb-passthrough.sh` re-binds USB on exit** ([#198](https://github.com/williamzujkowski/aegis-boot/pull/198), closes [#121](https://github.com/williamzujkowski/aegis-boot/issues/121)) — after QEMU exits, `xhci_hcd` sometimes logs a reset but doesn't re-attach scsi drivers (lsusb shows the device; `/dev/sda` and `/sys/block/sda` gone until physical replug). Trap handler now resolves the device's sysfs ID before QEMU takes over, then writes it to `/sys/bus/usb/drivers/usb/{unbind,bind}` with a 300 ms settle after QEMU exits. `exec sudo` replaced with plain `sudo` so bash stays alive for the trap.

### Documentation

- **CLI.md coverage refresh** ([#199](https://github.com/williamzujkowski/aegis-boot/pull/199)) — added the missing `compat` / `update` / `verify` subcommand sections, documented `--json` across all seven supported commands in one table, refreshed the `doctor` example output to include the new machine-identity + compat DB rows.
- **Theme names + accessibility recipes** ([#200](https://github.com/williamzujkowski/aegis-boot/pull/200)) — `README.md`'s `AEGIS_THEME` row now lists all five shipped themes (default/monochrome/high-contrast/okabe-ito/aegis); `TROUBLESHOOTING.md` gets a new "Accessibility" section pairing each symptom (low-contrast / color vision / serial / screen-reader) with the appropriate theme + `AEGIS_A11Y` flag. Closes the operator-discoverability half of the Okabe-Ito item in [#93](https://github.com/williamzujkowski/aegis-boot/issues/93) (code already shipped in [#76](https://github.com/williamzujkowski/aegis-boot/issues/76)).
- **2026-04-17 content audit log** ([#201](https://github.com/williamzujkowski/aegis-boot/pull/201)) — recorded today's audit findings + PRs in `docs/content-audit.md` per the [#78](https://github.com/williamzujkowski/aegis-boot/issues/78) cadence.

### Accessibility + ergonomics

- **`Home`/`End` as layout-agnostic first/last binds** ([#204](https://github.com/williamzujkowski/aegis-boot/pull/204)) — addresses the [#93](https://github.com/williamzujkowski/aegis-boot/issues/93) P2 keybind-audit item. `g`/`G` land on weird physical positions under AZERTY and Dvorak; crossterm maps `Home`/`End` identically across every OS layout. Help overlay shows both lines.

### Bugs

- **Script safety guards** ([#138](https://github.com/williamzujkowski/aegis-boot/issues/138) children) — two long-standing silent-failure paths in the build scripts now fail fast. `scripts/build-initramfs.sh` exits on `depmod` failure (was: logged a warning and continued, producing an image whose `modules.dep` still pointed at the original `.ko.zst` paths — storage modules would silently miss at boot). Set `AEGIS_ALLOW_MISSING_DEPMOD=1` to bypass. `scripts/mkusb.sh` now validates sgdisk-derived partition start sectors are non-empty, numeric, and non-zero before using them as `dd seek=` — an empty awk result yielded `seek=0`, silently overwriting the freshly-written GPT at sector 0.
- **OVMF SB detection fallback** ([#118](https://github.com/williamzujkowski/aegis-boot/issues/118)) — `rescue-tui`'s `SecureBootStatus::detect()` now scans `/sys/firmware/efi/efivars` for any filename starting with `SecureBoot-` when the two upstream-spec paths (global-GUID and plain) miss. Handles OVMF firmware builds that publish the variable under a non-spec suffix — observed under QEMU+OVMF SecBoot shakedown where rescue-tui's header showed `SB:unknown` despite SB enforcing. Parallels the existing scan fallback in `aegis-cli doctor` (doctor.rs:371).

### Publishing prep

- **Crates.io metadata for the library trio** ([#51](https://github.com/williamzujkowski/aegis-boot/issues/51)) — `iso-parser`, `iso-probe`, `kexec-loader` now carry the full `[package]` surface (`readme`, `documentation`, `homepage`, `keywords`, `categories`) plus per-crate README files. `iso-probe`'s path dep on `iso-parser` gained the required registry `version = "0.13"`. `cargo publish --dry-run -p iso-parser` and `-p kexec-loader` come back clean; `iso-probe`'s dry-run blocks on the unpublished-registry chicken-and-egg which is expected and resolved by the real publish ordering documented in `docs/RELEASE_CRATES.md`. Gate to actual publish remains v1.0.0-rc1 (real-hardware shakedown still pending).
- **Test flake fix** — `fetch::tests::default_cache_uses_xdg_cache_home` and `default_cache_falls_back_to_home_dot_cache` now serialize on a `Mutex` to avoid the process-global env-var race. Both tests pass across parallel runs.

### CI reliability

- **apt retry loop in `Dockerfile.locked`** — GitHub Actions runners periodically can't reach `archive.ubuntu.com:80` (observed repeatedly on reproducible-build and mkusb jobs; forces a manual `gh run rerun`). `Dockerfile.locked`'s package install step now wraps `apt-get update && apt-get install` in a 3-attempt retry loop with a 15s backoff plus `Acquire::Retries=5` in apt's own config. Up to ~60s of mirror blips are now absorbed silently; genuine "package doesn't exist" errors still fail fast (they fail identically on every attempt). Does not change the reproducibility guarantee: the `reproducible-build.yml` workflow verifies the `rescue-tui` binary hash, not the Dockerfile or image digest (see the workflow header comment).

### Operator experience

- **ISO pretty-name detection** ([#119](https://github.com/williamzujkowski/aegis-boot/issues/119)) — `iso-parser` now reads `/etc/os-release` (`PRETTY_NAME`), falling back to `/lib/os-release`, `/usr/lib/os-release`, `/.disk/info` (Ubuntu/Debian convention), and `/etc/alpine-release` during ISO discovery. Populated into new `BootEntry.pretty_name` + `DiscoveredIso.pretty_name` fields (both `Option<String>`, `#[serde(default)]` for forward compat). `iso-probe::display_name()` helper returns `pretty_name` when present, falling back to `label`. rescue-tui's list view, Confirm preview, and Error pane now use it — operators see "Ubuntu 24.04.2 LTS (Noble Numbat)" or "Alpine Linux 3.20.3" instead of just the distribution family name. 11 new unit tests cover the parser (quoted/unquoted values, missing keys, multi-key files, file-priority ordering, empty-line skipping in `.disk/info`).
- **Two more `init` profiles** — `minimal` (alpine-only, ~200 MiB, fastest) and `server` (ubuntu-server + rocky + almalinux, ~6 GiB, enterprise RHEL + Ubuntu rescue triple). Operators can now pick `aegis-boot init --profile <panic-room|minimal|server>` to fit the target-environment shape. Every profile slug is enforced to be in the verified catalog at test time (`every_profile_slug_is_in_catalog`).
- **`aegis-boot init --profile panic-room`** ([#161](https://github.com/williamzujkowski/aegis-boot/issues/161)) — one-command rescue stick. Composes `doctor → flash → fetch + add` for every slug in a named profile, producing a single attestation manifest spanning the whole run. Default `panic-room` profile ships three ISOs (Alpine 3.20, Ubuntu 24.04 Server, Rocky 9) covering ~5 GiB — fits on a 16 GB stick. Extracted `try_run` variants on `doctor`, `flash`, `fetch`, and `inventory::run_add` so the composition can branch on typed `Result` instead of opaque `ExitCode`. Flash gained `--yes` to skip the typed-confirmation prompt when invoked from `init`.
- **Stale issue triage ([#52](https://github.com/williamzujkowski/aegis-boot/issues/52), [#122](https://github.com/williamzujkowski/aegis-boot/issues/122), [#127](https://github.com/williamzujkowski/aegis-boot/issues/127))** — closed three bugs/docs issues whose fixes had already shipped in v0.12.0 / v0.13.0 and were masquerading as open. Verified the fixes in source (Debian-layout marker gate, post-kexec handoff banner, CHANGELOG reproducibility caveat) before closing.

### Catalog + quality

- **Catalog curation policy** ([#154](https://github.com/williamzujkowski/aegis-boot/issues/154)) — `docs/CATALOG_POLICY.md` formalizes the 5 inclusion criteria (HTTPS canonical URL, project-published signed SHA256SUMS, operator value, stable URL, honest SB posture), accepted categories, and the PR proposal process.
- **Weekly catalog URL revalidation** ([#155](https://github.com/williamzujkowski/aegis-boot/issues/155)) — `scripts/catalog-revalidate.sh` + scheduled workflow checks every URL in the catalog via range-GET. Surfaced 25 broken URLs in the first run.
- **Catalog trimmed to 6 verified entries** ([#159](https://github.com/williamzujkowski/aegis-boot/issues/159), closes [#156](https://github.com/williamzujkowski/aegis-boot/issues/156)) — removed 12 entries with broken URLs (sourceforge rewrites, point-release rotation, wrong sig_url patterns). Keeping only entries where all three URLs verify green. Entries will be re-added per #156 as URLs are fixed.

### CI / distribution

- **Homebrew formula validated in CI** ([#157](https://github.com/williamzujkowski/aegis-boot/issues/157)) — `brew audit + style + install + test` workflow runs on Formula changes and weekly. Formula ComponentsOrder fixed (`depends_on` before `on_linux`), `uses_from_macos "coreutils"` removed (not macOS-only).

### Roadmap

- **v1.2+ category-defining epic** ([#158](https://github.com/williamzujkowski/aegis-boot/issues/158)) — 5 capabilities beyond the v1.0/v1.1 roadmap: FIDO2-backed operator identity, post-kexec verifier + TPM quote, Sigstore Rekor integration, ephemeral compute bootstrapping, automotive/coreboot rescue mode. Ranked by category-redefinition impact.

## [0.13.0] — 2026-04-16

The **best-in-class push** — what landed after v0.12.0 went out and the repo went public, working through epics #136 (operator + sysadmin reach), #137 (onboarding + discoverability), and #138 (quality gates).

### Operator surface area expansion (epic #136)

Five new `aegis-boot` subcommands. The CLI is now the single tool an operator needs from "I have a stick and an ISO" to "the target machine boots".

- **`aegis-boot doctor [--stick /dev/sdX]`** ([#141](https://github.com/williamzujkowski/aegis-boot/issues/141)) — host + stick health check. Inspects OS, prerequisite tools (`dd`, `sudo`, `sgdisk`, `lsblk`, `curl`, `sha256sum`, `gpg` — [#146](https://github.com/williamzujkowski/aegis-boot/issues/146)), Secure Boot state (mokutil + efivar fallback), removable drive enumeration, partition layout (asserts ESP + AEGIS_ISOS), AEGIS_ISOS contents (counts ISOs + sidecars). Reports a 0-100 score and a single `NEXT ACTION` line. PASS=10 / WARN=7 / FAIL=0; bands: 90+ EXCELLENT, 70+ OK, 40+ DEGRADED, <40 BROKEN.
- **`aegis-boot recommend [slug]`** ([#142](https://github.com/williamzujkowski/aegis-boot/issues/142)) — curated catalog of 13 known-good ISOs (Ubuntu LTS server + desktop, Fedora 41, Debian 12, Alpine, Arch, NixOS, SystemRescue, GParted, Memtest86+, Clonezilla, Tails, Kali). Each entry carries the project's canonical URL + the URL of the project's signed `SHA256SUMS` (no checksum pinning in our catalog — it'd rot weekly; the project's own signing key is the trust anchor). SB column makes the unsigned-needs-MOK distinction visible up front. Slug resolution is exact-or-unique-prefix.
- **`aegis-boot fetch <slug>`** ([#145](https://github.com/williamzujkowski/aegis-boot/issues/145)) — automates the recipe `recommend` prints. Downloads the ISO + signed `SHA256SUMS` + signature, runs `sha256sum -c`, runs `gpg --verify`. Four GPG verdicts: OK, UnknownKey (non-fatal — operator can review + import + retry), BadSignature (fatal), GpgMissing (fatal unless `--no-gpg`). Per-slug cache directory (`$XDG_CACHE_HOME/aegis-boot/<slug>/`); skips files already present so re-runs are cheap.
- **`aegis-boot attest list|show`** ([#147](https://github.com/williamzujkowski/aegis-boot/issues/147)) — attestation receipts. Every `flash` writes a JSON manifest at `$XDG_DATA_HOME/aegis-boot/attestations/<disk-guid>-<ts>.json` capturing tool version, timestamp, operator, host kernel + SB state, target device + model + size + GUID, image SHA-256 + size. Schema v1, additive-evolution-friendly (unknown fields tolerated by parser). Operationalizes the "prove what's on the stick" trust narrative — the differentiator vs every other USB-imaging tool.
- **Append on `aegis-boot add`** ([#148](https://github.com/williamzujkowski/aegis-boot/issues/148)) — every successful `add` appends an `IsoRecord` (filename, sha256, size, sidecars, timestamp) to the matching attestation. Lookup: mount path → owning device via `/proc/mounts` → strip partition suffix (handles `sda1`, `nvme0n1p3`, `mmcblk0p1`, `loop12p1` correctly) → disk GUID → newest matching manifest. Falls back to "most recent overall" with a warning when GUID can't be resolved.
- **`aegis-boot list` shows attestation summary** ([#149](https://github.com/williamzujkowski/aegis-boot/issues/149)) — when listing ISOs, prints a header above the table: `flashed at` + `operator` + `ISOs added since flash` + `manifest path`. Closes the attestation loop: flash → add → list all reference the same chain.

### Distribution + onboarding (epic #137)

- **GitHub Releases automation with cosign-signed binaries** ([#143](https://github.com/williamzujkowski/aegis-boot/issues/143)) — each release now ships a static-musl `aegis-boot-x86_64-linux` binary (~855 KiB), the existing `rescue-tui`, `initramfs.cpio.gz`, `sbom.cdx.json`, and an aggregated `SHA256SUMS` — every artifact accompanied by a Sigstore cosign keyless signature (`.sig` + `.pem`). The signing certificate is bound to this repo's `release.yml` workflow at the tag's ref, so verifying confirms the artifact came from *this* repo's release, not a copycat. Backfilled v0.12.0's release with all 16 cosign-signed assets.
- **`scripts/install.sh` cosign-verified one-liner** ([#144](https://github.com/williamzujkowski/aegis-boot/issues/144)) — `curl -sSL https://raw.githubusercontent.com/williamzujkowski/aegis-boot/main/scripts/install.sh | sh` downloads the latest release's binary, verifies its cosign signature, installs to `/usr/local/bin` (root) or `~/.local/bin` (non-root). POSIX-portable, truncation-safe (wrapped in `main()` called at end-of-file), fails closed if cosign is missing.
- **Homebrew tap** ([#150](https://github.com/williamzujkowski/aegis-boot/issues/150)) — `Formula/aegis-boot.rb` makes this repo a Brew tap. Operators install with `brew tap williamzujkowski/aegis-boot https://github.com/williamzujkowski/aegis-boot && brew install aegis-boot`. Linux x86_64 only today.
- **Auto-bump Brew formula on tag push** ([#151](https://github.com/williamzujkowski/aegis-boot/issues/151)) — release workflow now updates `Formula/aegis-boot.rb` automatically after each release. No more manual maintenance.
- **`docs/HARDWARE_COMPAT.md`** ([#146](https://github.com/williamzujkowski/aegis-boot/issues/146)) — community-curated table of validated machines, seeded with the v0.12.0 [#109](https://github.com/williamzujkowski/aegis-boot/issues/109) shakedown + the QEMU/OVMF reference environment.
- **`docs/RELEASE_NOTES_FOOTER.md`** — appended to every release's notes; gives operators a copy-pasteable cosign verify-blob recipe so trust is testable.

### Quality gates (epic #138)

- **`#![forbid(unsafe_code)]` on iso-parser** ([#140](https://github.com/williamzujkowski/aegis-boot/issues/140)) — kexec-loader remains the only crate with `unsafe`, which is its purpose. Tightens the trust surface for the upcoming crates.io publish ([#51](https://github.com/williamzujkowski/aegis-boot/issues/51)).

### Roadmap + governance

- 4 epics filed: [#136](https://github.com/williamzujkowski/aegis-boot/issues/136), [#137](https://github.com/williamzujkowski/aegis-boot/issues/137), [#138](https://github.com/williamzujkowski/aegis-boot/issues/138), [#139](https://github.com/williamzujkowski/aegis-boot/issues/139) (post-v1.0 fleet+trust depth).
- 2 milestones: v1.0.0 + v1.1.0.
- Cleanup: closed [#40](https://github.com/williamzujkowski/aegis-boot/issues/40) (superseded by [#123](https://github.com/williamzujkowski/aegis-boot/issues/123)), and 4 already-fixed bugs ([#113](https://github.com/williamzujkowski/aegis-boot/issues/113), [#115](https://github.com/williamzujkowski/aegis-boot/issues/115), [#116](https://github.com/williamzujkowski/aegis-boot/issues/116), [#117](https://github.com/williamzujkowski/aegis-boot/issues/117)).

### Tests

186 workspace tests, clippy clean (was 140 at v0.12.0). +46 in aegis-cli covering the new modules (catalog invariants, fetch helpers, doctor scoring, attestation roundtrip + lookup arithmetic, partition-suffix stripping incl. the `loop12` regression case).

### Repo went public

`gh repo edit --visibility public` after a clean 194-commit gitleaks scan. `https://github.com/williamzujkowski/aegis-boot` is now indexable, taggable, forkable, contributable.

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

- [#132](https://github.com/williamzujkowski/aegis-boot/issues/132) Real-HW E2E test of last-booted persistence (currently unit-tested only)
- [#123](https://github.com/williamzujkowski/aegis-boot/issues/123) Mac/Windows `aegis-boot flash` (Linux-only today)
- [#51](https://github.com/williamzujkowski/aegis-boot/issues/51) Framework / ThinkPad / Dell real-boot on the hardware itself (today: QEMU passthrough of real USB — close but not full)

## [0.11.0] — 2026-04-15

**Accessibility + design-review cleanup release.**

### Headline — text-mode accessibility (#104)

- **`AEGIS_A11Y=text` / `TERM=dumb` activates a plain-text mode.** ratatui's alternate-screen rendering is invisible to screen readers (Orca, NVDA) and braille displays (via brltty). Text mode prints a numbered menu to stdout, reads a line from stdin, and never touches raw mode / alt-screen / ANSI — usable from serial consoles, 40-col terminals, and accessibility tools out of the box.
- **Full trust-challenge + rescue-shell parity.** The text-mode Confirm flow prints the one-frame evidence block, asks y/N for GREEN verdicts or requires typing `boot` for YELLOW/GRAY degraded-trust verdicts (same gate as the TUI), hard-blocks RED.
- **`ANN:` announcements on stderr** on every menu paint and state transition. brltty / speakup can mirror to braille / speech — same pattern `dialog(1)` uses.

### Design-review follow-ups ([#101](https://github.com/williamzujkowski/aegis-boot/issues/101), [#102](https://github.com/williamzujkowski/aegis-boot/issues/102), [#103](https://github.com/williamzujkowski/aegis-boot/issues/103))

- **Compacted Confirm screen** — Kernel+Initrd merged onto one `Boot:` line; Checksum+Signature merged onto one `Trust:` line. Net −2 rows so the verdict stays above the fold on 24-row terminals.
- **Filter-mode info bar is unmistakable** — reversed-style `FILTER` label in `theme.warning`, bold filter text, `SLOW_BLINK` caret span. Previously the only cue was a trailing `_`.
- **`q` on Confirm returns to List** (not ConfirmQuit). Operators meaning "quit this screen" no longer get the reboot-the-machine prompt. ConfirmQuit still reachable from List.

### Tests

140 workspace tests (unchanged — all shipped changes are render- or branch-level without new state transitions).

### Deferred

- Text-mode process-level integration tests (filed as follow-up if requested).
- Text-mode filter / sort / verify-now (filed if real operators ask — the assistive-tech surface area is usually "pick an ISO, boot it").

## [0.10.1] — 2026-04-15

**Brand identity + design-review fixes.** Delivers [#76](https://github.com/williamzujkowski/aegis-boot/issues/76) (brand identity spec produced by the nexus-agents `ux_expert`) and the three concrete fixes from the expert's subsequent self-critique.

### Brand identity (#76)

- **`assets/brand/`** — master SVG + monochrome variant of the shield-with-keyhole logo; ASCII renders (full 10-line README hero + compact 3-line TUI); `palette.css` with oklch + hex; `BRAND.md` usage guidelines.
- **README hero block** — shield ASCII + tagline + license/release/CI badges.
- **Tagline:** *Signed boot. Any ISO. Your keys.*
- **`aegis` theme** — fifth named palette alongside default / monochrome / high-contrast / okabe-ito. Steel-blue primary (`#3B82F6`), emerald success, amber warning, vermillion error. Verified under deuteranopia/protanopia; distinct from Ubuntu/Fedora/Arch distro palettes.
- **TUI header** gains the `◆` shield mark in brand primary plus the tagline in dim italic.

### Design-review fixes (#76 self-critique)

- **Header degrades gracefully on narrow terminals.** Previously truncated mid-word ("Signed boot. Any ISO. Yo"). Now span-chain is gated on `area.width`: ≥90 = full; ≥70 drops tagline; ≥50 drops TPM; <50 keeps only mark + name + version. Shield mark always survives.
- **TrustChallenge mismatch feedback.** Typed characters `≥4` that don't equal `boot` render in error colour + bold. Silent-fail on a security gate was trainable toward muscle-memory mashing.
- **TPM status colour reflects TPM state.** Previously hardcoded to green regardless; `TPM:none` now renders amber (warning). A green "none" was a lie.

### Deferred to follow-up issues

- [#101](https://github.com/williamzujkowski/aegis-boot/issues/101) Confirm info density — verdict can scroll off 24-row terminals
- [#102](https://github.com/williamzujkowski/aegis-boot/issues/102) Filter-mode entry visual subtlety
- [#103](https://github.com/williamzujkowski/aegis-boot/issues/103) `q` on Confirm opens ConfirmQuit (should be Esc-back equivalent)
- [#104](https://github.com/williamzujkowski/aegis-boot/issues/104) `AEGIS_A11Y=text` screen-reader / braille mode

### Tests

Workspace tests 140 (+1 for the aegis theme; no test-count change from design-review fixes since they're render-only).

## [0.10.0] — 2026-04-15

**Rescue + trust challenge + evidence release.** Implements the three biggest deferred items from the UX epic parent ([#85](https://github.com/williamzujkowski/aegis-boot/issues/85)) and its trust/a11y children ([#92](https://github.com/williamzujkowski/aegis-boot/issues/92), [#93](https://github.com/williamzujkowski/aegis-boot/issues/93)).

### Headline

- **Always-present rescue-shell entry** ([#90](https://github.com/williamzujkowski/aegis-boot/issues/90)). The List screen now always ends with `[#] rescue shell (busybox)` — visible even when zero ISOs are discovered. Selecting it exits rescue-tui with sentinel code 42; `/init` recognizes the code and drops cleanly to `/bin/sh`. Previously "no ISOs found" was a dead end. Pattern: rEFInd tools row, Endless OS recovery.
- **Typed trust confirmation on degraded verdicts** ([#93](https://github.com/williamzujkowski/aegis-boot/issues/93)). Pressing Enter on a YELLOW (untrusted signer) or GRAY (no verification material) Confirm screen now opens a challenge that requires typing `boot` exactly. GREEN verdicts skip it; RED verdicts stay hard-blocked by #55. Pattern: SSH first-connect, HSTS, Gatekeeper.
- **memtest-style one-frame error screen** ([#92](https://github.com/williamzujkowski/aegis-boot/issues/92)). kexec-failure Error screen now renders a complete evidence block: version, SB/TPM state, ISO path + size + distro, trust verdict, effective cmdline, and the sha256 digest that was fed to PCR 12. One screen photograph = one complete bug report. Pattern: memtest86+.
- **F10 save-log to AEGIS_ISOS** ([#92](https://github.com/williamzujkowski/aegis-boot/issues/92)). From the Error screen, F10 serializes the evidence block to `/run/media/aegis-isos/aegis-log-<unix_ts>.txt` (or `/tmp` fallback). Operator can pull it off the stick from any other machine post-reboot. Pattern: rEFInd's refind.log on ESP.

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

**Trust UX + verify-now + a11y polish.** Synthesis from two more parallel-agent surveys: trust/attestation UX (Firefox certs, OpenSSH first-connect, GPG/minisign, Gatekeeper, TPM eventlog, Cosign, Android Verified Boot) and accessibility/field-ops (brltty, speakup, Debian-installer a11y, GRUB, systemrescue, Clonezilla, memtest86+, rEFInd log, UEFI shell). Epics filed as [#92](https://github.com/williamzujkowski/aegis-boot/issues/92) and [#93](https://github.com/williamzujkowski/aegis-boot/issues/93).

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
