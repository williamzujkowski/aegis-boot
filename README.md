<div align="center">

```
    ╔═══════════════╗
    ║   ▄███████▄   ║
    ║  ███████████  ║
    ║  ███┌───┐███  ║
    ║  ███│ ◆ │███  ║
    ║  ███│   │███  ║
    ║  ███└─▲─┘███  ║
    ║  ▀█████████▀  ║
    ║    ▀█████▀    ║
    ╚═══════════════╝
```

# aegis-boot

**Signed boot. Any ISO. Your keys.**

A signed UEFI Secure Boot rescue environment that lets operators pick any ISO from a USB stick's data partition and `kexec` into it — without leaving the chain of trust.

[![License](https://img.shields.io/github/license/williamzujkowski/aegis-boot)](LICENSE-APACHE)
[![Latest Release](https://img.shields.io/github/v/release/williamzujkowski/aegis-boot)](https://github.com/williamzujkowski/aegis-boot/releases/latest)
[![CI](https://img.shields.io/github/actions/workflow/status/williamzujkowski/aegis-boot/ci.yml?label=ci)](https://github.com/williamzujkowski/aegis-boot/actions)

</div>

**Status:** v0.13.0 — best-in-class push: 5 new operator subcommands (`doctor`, `recommend`, `fetch`, `attest list/show`), cosign-signed prebuilt binaries, install one-liner, Homebrew tap, attestation receipts on every flash. Real-hardware shakedown validated on Alpine + Ubuntu under Secure Boot enforcing ([#109](https://github.com/williamzujkowski/aegis-boot/issues/109)). Multi-vendor real-hardware sweep (Framework / ThinkPad / Dell) gates v1.0.0 ([#51](https://github.com/williamzujkowski/aegis-boot/issues/51)).

## What it does

1. Flash an aegis-boot image to a USB stick (`aegis-boot flash` or `dd`).
2. Drop `.iso` files onto the `AEGIS_ISOS` partition.
3. Boot the stick on any UEFI machine with Secure Boot enabled.
4. A minimal ratatui TUI lists the ISOs; the operator selects one.
5. `kexec_file_load(2)` hands off to the selected ISO's kernel.

Boot chain: `UEFI firmware → shim (MS-signed) → grub (Canonical-signed) → rescue kernel → our initramfs → rescue-tui → kexec_file_load → selected ISO's kernel`. Full rationale: [ADR 0001](./docs/adr/0001-runtime-architecture.md).

## How it differs from Ventoy / Rufus / balenaEtcher

| Tool | Boots arbitrary ISOs | Preserves Secure Boot chain | Per-ISO trust decision |
|---|---|---|---|
| **aegis-boot** | yes | **yes** — kernel-level signature check via `KEXEC_SIG` | yes — operator enrolls keys per distro |
| Ventoy | yes | weakened — one shared MOK key trusts every Ventoy-booted kernel | no |
| Rufus | one ISO at a time | depends on ISO; no orchestration | n/a |
| balenaEtcher | one ISO at a time | depends on ISO; no orchestration | n/a |

aegis-boot is the right pick when you need to boot operator-supplied ISOs **without disabling Secure Boot or trusting a global third-party MOK**. Unsigned ISO kernels are refused with a clear error and a `mokutil --import` command for the specific signing key — see [docs/UNSIGNED_KERNEL.md](./docs/UNSIGNED_KERNEL.md).

## Quickstart — operators

Install the operator CLI:

| Platform | Status |
|---|---|
| Linux x86_64 | Full support — flash + build, add ISOs, kexec, attest, doctor, compat |
| macOS (Apple Silicon + Intel) | Drive detection + `flash --image PATH` ([#229](https://github.com/williamzujkowski/aegis-boot/pull/229)). Image *building* requires Linux (mkusb.sh deps); use `aegis-boot fetch-image --url ... --sha256 ...` ([#232](https://github.com/williamzujkowski/aegis-boot/pull/232)) to pull a pre-built `.img` then pipe to `flash --image $(...)` |
| Windows | Drive enumeration via `Get-Disk` ([#230](https://github.com/williamzujkowski/aegis-boot/pull/230)). Raw-disk writing deferred — combine `aegis-boot list` with Rufus or `dd-for-Windows` for the write |

Pre-built binaries below are Linux-only today; macOS/Windows users build with `cargo install --path crates/aegis-cli` until a darwin/windows release artifact ships.

```bash
# Cosign-verified install from the latest GitHub release.
curl -sSL https://raw.githubusercontent.com/williamzujkowski/aegis-boot/main/scripts/install.sh | sh

# OR via Homebrew (Linux):
brew tap williamzujkowski/aegis-boot https://github.com/williamzujkowski/aegis-boot
brew install aegis-boot

# Or pin a version: sh install.sh --version v0.12.0
# Or skip cosign (NOT recommended): sh install.sh --no-verify
# Build from source: see BUILDING.md.
```

Each release ships a static-musl `aegis-boot-x86_64-linux` binary plus its Sigstore cosign signature + certificate; the installer checks the cert is bound to *this* repo's `release.yml` workflow before installing. See `docs/RELEASE_NOTES_FOOTER.md` for the manual `cosign verify-blob` recipe.

Then the operator flow — pick one:

### One command — `aegis-boot init` (recommended for new users)

```bash
# Empty stick → rescue-ready in under 10 minutes
sudo aegis-boot init /dev/sdc --yes
```

Composes `doctor → flash → fetch + add` for every ISO in the default `panic-room` profile (Alpine 3.20 + Ubuntu 24.04 Server + Rocky 9, ~5 GiB total). Produces one attestation manifest spanning the whole run. See [`aegis-boot init`](./docs/CLI.md#aegis-boot-init) for profiles and options.

### Step-by-step — when you want a custom ISO set

```bash
# 0. (recommended) check host + stick health before doing anything destructive
aegis-boot doctor

# 1. browse the curated catalog — known-good signed-or-MOK-needed ISOs
aegis-boot recommend
aegis-boot recommend ubuntu-24.04-live-server   # one entry's download recipe

# 2. write aegis-boot to a USB stick (3-step guided; auto-detects removable drives)
sudo aegis-boot flash             # or: sudo aegis-boot flash /dev/sdc

# 3. add ISOs to the stick (auto-detects mount; copies sidecars too)
aegis-boot add ~/Downloads/ubuntu-24.04.2-live-server-amd64.iso
aegis-boot list                   # show what's on the stick

# 4. plug the stick into a target machine, boot from it (UEFI + SB enabled)
#    The TUI discovers ISOs, shows verification status, and kexecs on Enter.
```

Operator end-to-end walkthrough: [docs/INSTALL.md](./docs/INSTALL.md). Common errors and their fixes: [docs/TROUBLESHOOTING.md](./docs/TROUBLESHOOTING.md).

## Quickstart — developers

```bash
cargo build --release
./scripts/build-initramfs.sh
./scripts/mkusb.sh                # produces out/aegis-boot.img

# Boot the simulated stick under QEMU + OVMF SecBoot
mkdir -p test-isos && cp ~/Downloads/*.iso test-isos/
./scripts/qemu-loaded-stick.sh -d ./test-isos -a usb -i
```

Full developer loop: [docs/LOCAL_TESTING.md](./docs/LOCAL_TESTING.md).

## Components

| Crate | Role |
|---|---|
| [`aegis-cli`](./crates/aegis-cli) | Operator CLI — `aegis-boot init`, `flash`, `add`, `list`, `doctor` |
| [`iso-parser`](./crates/iso-parser) | ISO media analysis — finds kernel/initrd/cmdline in distro boot configs |
| [`iso-probe`](./crates/iso-probe) | Runtime discovery + sibling `.sha256` / `.minisig` verification + installer-vs-live heuristics |
| [`kexec-loader`](./crates/kexec-loader) | Safe wrapper over `kexec_file_load(2)` with error classification |
| [`rescue-tui`](./crates/rescue-tui) | ratatui application the operator sees; hard-blocks kexec on hash/sig failure |
| [`aegis-fitness`](./crates/aegis-fitness) | Repo / build / artifact health audit (9 checks) |

## Documentation

**For operators**

- [docs/INSTALL.md](./docs/INSTALL.md) — flash → add ISOs → boot → select, end-to-end
- [docs/CLI.md](./docs/CLI.md) — `aegis-boot` CLI reference
- [docs/TROUBLESHOOTING.md](./docs/TROUBLESHOOTING.md) — common errors and fixes (errno 61, won't-boot, etc.)
- [docs/UNSIGNED_KERNEL.md](./docs/UNSIGNED_KERNEL.md) — what to do when an ISO's kernel isn't signed
- [docs/USB_LAYOUT.md](./docs/USB_LAYOUT.md) — what's on the stick (ESP + `AEGIS_ISOS`)

**For contributors**

- [CONTRIBUTING.md](./CONTRIBUTING.md) — patch workflow, conventions, PR bar
- [docs/ARCHITECTURE.md](./docs/ARCHITECTURE.md) — one-page mental model
- [BUILDING.md](./BUILDING.md) — reproducible build setup (Docker + Nix)
- [docs/LOCAL_TESTING.md](./docs/LOCAL_TESTING.md) — 8-stage local CI equivalent
- [docs/adr/](./docs/adr/) — Architecture Decision Records
- [docs/compatibility/iso-matrix.md](./docs/compatibility/iso-matrix.md) — per-distro kexec compatibility

**Security**

- [SECURITY.md](./SECURITY.md) — vulnerability reporting (use private advisory; 7-day ack SLA)
- [THREAT_MODEL.md](./THREAT_MODEL.md) — UEFI Secure Boot threat model (PK/KEK/MOK/SBAT)

**Project**

- [CHANGELOG.md](./CHANGELOG.md) | [ROADMAP.md](./ROADMAP.md) | [CODE_OF_CONDUCT.md](./CODE_OF_CONDUCT.md)

## Runtime configuration

`rescue-tui` reads these environment variables (set on the kernel cmdline or in the `/init` script):

| Variable | Default | Purpose |
|---|---|---|
| `AEGIS_ISO_ROOTS` | `/run/media:/mnt` | Colon-separated dirs to scan for `.iso` files |
| `AEGIS_THEME` | default | Theme: `default`, `monochrome` (serial/screen-reader), `high-contrast` (low-contrast framebuffers), `okabe-ito` (colorblind-safe, aliases: `cb`, `colorblind`), or `aegis` (brand). Also readable as `aegis.theme=<name>` on kernel cmdline. |
| `AEGIS_AUTO_KEXEC` | unset | Substring; first matching ISO is kexec'd without operator confirmation |
| `AEGIS_A11Y` | unset | `1` enables text-only mode (also auto-enabled when `TERM=dumb`) |
| `AEGIS_LOG_JSON` | unset | `1` switches `tracing` output to JSON for `journalctl --output=json` |
| `AEGIS_STATE_DIR` | `/var/lib/aegis-boot` | Where last-booted state is persisted |

`scripts/mkusb.sh` reads: `OUT_DIR`, `IMG`, `DISK_SIZE_MB` (default 2048), `ESP_SIZE_MB` (400), `DATA_LABEL` (`AEGIS_ISOS`), `DATA_FS` (`fat32` or `ext4`), `SHIM_SRC`, `GRUB_SRC`, `KERNEL_SRC`, `INITRD_SRC`.

## Build environment

- Rust 1.85.0 (pinned in `Dockerfile.locked`, enforced via `rust-version` in every `Cargo.toml`)
- Ubuntu 22.04 base (Docker) or a Nix flake
- No EDK II / UEFI toolchain — we use shim + signed distro kernels instead

## License

Dual-licensed under either of:

- [Apache License 2.0](./LICENSE-APACHE) (`LICENSE-APACHE`)
- [MIT License](./LICENSE-MIT) (`LICENSE-MIT`)

at your option. Contributions are accepted under the same dual license; see [CONTRIBUTING.md](./CONTRIBUTING.md#license).
