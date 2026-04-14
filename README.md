# Aegis-Boot

Reproducible UEFI Secure Boot orchestration infrastructure.

## Architecture

The runtime is a **signed Linux rescue environment + ratatui TUI + kexec** (Option B). See [ADR 0001](./docs/adr/0001-runtime-architecture.md) for the full decision record and the vote that produced it.

Boot chain: `UEFI firmware → shim → signed rescue kernel → initramfs → rescue-tui → kexec_file_load → selected ISO's kernel`.

## Components

- **[BUILDING.md](./BUILDING.md)** — Reproducible build setup (Docker + Nix)
- **[THREAT_MODEL.md](./THREAT_MODEL.md)** — UEFI Secure Boot threat model (PK/KEK/MOK/SBAT)
- **[docs/adr/](./docs/adr/)** — Architecture Decision Records
- **[docs/compatibility/iso-matrix.md](./docs/compatibility/iso-matrix.md)** — Per-distro ISO + kexec compatibility matrix
- **[Dockerfile.locked](./Dockerfile.locked)** — Pinned base image (Ubuntu 22.04, Rust 1.75.0, EDK II stable202311)
- **[flake.nix](./flake.nix)** — Nix flake for declarative dev environments
- **[crates/iso-parser](./crates/iso-parser)** — ISO media parser (on-media analysis, `std`-compatible)
- **[crates/iso-probe](./crates/iso-probe)** — Runtime ISO discovery on the live rescue environment
- **[crates/rescue-tui](./crates/rescue-tui)** — ratatui application the user sees
- **[crates/kexec-loader](./crates/kexec-loader)** — Safe wrapper over `kexec_file_load(2)`

## Status

Work in progress. Architecture decided (ADR 0001); `iso-parser` is the only crate with real functionality. `iso-probe`, `rescue-tui`, and `kexec-loader` are skeleton-only — implementation tracked from [#4](https://github.com/williamzujkowski/aegis-boot/issues/4).

## License

Dual-licensed under [Apache-2.0](./LICENSE-APACHE) or [MIT](./LICENSE-MIT) at your option.

## Security

Report vulnerabilities privately — see [SECURITY.md](./SECURITY.md).
