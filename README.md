# aegis-boot

A signed UEFI Secure Boot rescue environment that lets operators pick any ISO from a USB stick's data partition and `kexec` into it — without leaving the chain of trust.

**Status:** v0.7.0 — feature-complete under QEMU simulation; real-hardware shakedown gates v1.0.0 ([#51](https://github.com/williamzujkowski/aegis-boot/issues/51)).

## What it does

1. Flash `out/aegis-boot.img` (produced by `scripts/mkusb.sh`) to a USB stick.
2. Drop `.iso` files onto the `AEGIS_ISOS` partition.
3. Boot the stick on any UEFI machine with Secure Boot enabled.
4. A minimal ratatui TUI lists the ISOs; the operator selects one.
5. `kexec_file_load(2)` hands off to the selected ISO's kernel.

Boot chain: `UEFI firmware → shim (MS-signed) → grub (Canonical-signed) → rescue kernel → our initramfs → rescue-tui → kexec_file_load → selected ISO's kernel`. Full rationale: [ADR 0001](./docs/adr/0001-runtime-architecture.md).

## Quickstart (simulation)

```bash
# 1. build
cargo build --release -p rescue-tui
./scripts/build-initramfs.sh
./scripts/mkusb.sh

# 2. drop some ISOs into a dir
mkdir -p test-isos && cp ~/Downloads/*.iso test-isos/

# 3. boot the simulated stick under QEMU+OVMF SecBoot
./scripts/qemu-loaded-stick.sh -d ./test-isos -a usb -i
```

See [docs/LOCAL_TESTING.md](./docs/LOCAL_TESTING.md) for the full developer loop.

## Components

| Crate | Role |
|---|---|
| [`iso-parser`](./crates/iso-parser) | ISO media analysis — finds kernel/initrd/cmdline in distro boot configs |
| [`iso-probe`](./crates/iso-probe) | Runtime discovery + sibling `.sha256` / `.minisig` verification |
| [`kexec-loader`](./crates/kexec-loader) | Safe wrapper over `kexec_file_load(2)` with error classification |
| [`rescue-tui`](./crates/rescue-tui) | ratatui application the operator sees; hard-blocks kexec on hash/sig failure |
| [`aegis-fitness`](./crates/aegis-fitness) | Repo / build / artifact health audit (9 checks) |

## Documentation

- [BUILDING.md](./BUILDING.md) — reproducible build setup (Docker + Nix)
- [docs/LOCAL_TESTING.md](./docs/LOCAL_TESTING.md) — 8-stage local CI equivalent
- [docs/USB_LAYOUT.md](./docs/USB_LAYOUT.md) — GPT + ESP + `AEGIS_ISOS` partition scheme
- [docs/adr/](./docs/adr/) — Architecture Decision Records
- [docs/compatibility/iso-matrix.md](./docs/compatibility/iso-matrix.md) — per-distro kexec compatibility
- [SECURITY.md](./SECURITY.md) — vulnerability reporting
- [THREAT_MODEL.md](./THREAT_MODEL.md) — UEFI Secure Boot threat model (PK/KEK/MOK/SBAT)
- [CHANGELOG.md](./CHANGELOG.md)

## Build environment

- Rust 1.85.0 (pinned in `Dockerfile.locked`, enforced via `rust-version` in every `Cargo.toml`)
- Ubuntu 22.04 base (Docker) or a Nix flake
- No EDK II / UEFI toolchain — we use shim + signed distro kernels instead

## License

Dual-licensed: [Apache-2.0](./LICENSE-APACHE) OR [MIT](./LICENSE-MIT), at your option.
