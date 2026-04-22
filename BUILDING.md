# Building Aegis-Boot

This document describes the reproducible build environment for aegis-boot under the **Option B** runtime (see [ADR 0001](./docs/adr/0001-runtime-architecture.md)).

## Overview

Aegis-boot's build outputs are:

1. Rust binaries (`rescue-tui`, plus supporting libraries from the workspace).
2. An initramfs payload containing those binaries and any helper scripts.
3. A build manifest referencing the signed distro kernel the payload rides on.

The build environment is **intentionally small**. The chosen runtime architecture offloads Secure Boot verification to shim + a signed distro kernel, so no EDK II, NASM, or UEFI toolchain is required to build aegis-boot itself.

## Layers

| Layer                    | Purpose                                                  |
| ------------------------ | -------------------------------------------------------- |
| `Dockerfile.locked`      | Pinned Ubuntu 22.04 + Rust 1.85 + `cpio` + `sbsigntool`  |
| `flake.nix`              | Nix flake for declarative dev shell                      |
| Reproducible-build CI    | Two back-to-back builds; SHA-256 of `docker save` equal  |

## Prerequisites

- Docker 24.0+ with BuildKit
- (Optional) Nix 2.18+ for `nix develop`
- Git

## Quick Start (Docker)

```bash
docker build -t aegis-boot-build -f Dockerfile.locked .
docker run --rm -v "$PWD:/build" -w /build aegis-boot-build cargo build --release
```

## Quick Start (Nix)

```bash
nix develop
cargo build --release
```

## Pinned Dependencies

| Component     | Version      | Pin Method                 |
| ------------- | ------------ | -------------------------- |
| Ubuntu base   | 22.04        | SHA-256 digest             |
| Rust          | 1.85.0       | Exact version via rustup   |
| build-essential | 12.9ubuntu3 | APT version pin           |
| git           | 2.34.1       | APT version pin            |
| cpio          | 2.13         | APT version pin            |
| sbsigntool    | 0.9.4        | APT version pin            |

## Reproducibility

CI verifies reproducibility at [`.github/workflows/reproducible-build.yml`](./.github/workflows/reproducible-build.yml). Two `docker build` passes must produce `docker save` tarballs with identical SHA-256 hashes. `SOURCE_DATE_EPOCH` is set inside the image so any downstream tooling honoring it (Rust `cargo`, `cpio --reproducible`) produces deterministic outputs.

### Running locally

```bash
docker build --no-cache -t aegis:p1 -f Dockerfile.locked . && docker save aegis:p1 | sha256sum > p1.sha256
docker build --no-cache -t aegis:p2 -f Dockerfile.locked . && docker save aegis:p2 | sha256sum > p2.sha256
diff p1.sha256 p2.sha256 && echo reproducible || echo NOT reproducible
```

## What this image does NOT contain

- **EDK II** — we do not build a UEFI application (see ADR 0001, rejection of Option A).
- **NASM / iasl / uuid-dev** — no firmware-level assembly or ACPI tooling needed.
- **Python** — removed; the build does not require any Python glue.

Removing these dependencies shrinks the image, eliminates the historical unreachable-submodule hazard ([#2](https://github.com/williamzujkowski/aegis-boot/issues/2), closed), and tightens the supply-chain surface to just what the runtime needs.

## Assembling the initramfs

Once \`rescue-tui\` is built, package it into a bootable initramfs with:

```bash
cargo build --release -p rescue-tui
./scripts/build-initramfs.sh
# → out/initramfs.cpio.gz
# → out/initramfs.cpio.gz.sha256
```

### What ends up in the initramfs

- `/usr/bin/rescue-tui` — the ratatui binary
- `/bin/busybox` (+ applet symlinks: `sh`, `mount`, `umount`, `mdev`, …)
- `/init` — PID 1 shell script that mounts `/proc`, `/sys`, `/dev`, auto-mounts block devices under `/run/media/*`, and `exec`s `rescue-tui`
- Shared-library closure of `rescue-tui` (resolved via `ldd`)

### Reproducibility

The script is deterministic by construction: sorted cpio input, every mtime flattened to `$SOURCE_DATE_EPOCH`, gzip run with `--no-name`. Two back-to-back invocations produce byte-identical `initramfs.cpio.gz`. Verified in CI (`.github/workflows/initramfs.yml`).

### Riding inside a signed distro kernel

The `initramfs.cpio.gz` is meant to be concatenated onto an existing signed distro rescue initramfs (e.g. Ubuntu's `casper/initrd`). Under Secure Boot the kernel's signature covers the initramfs it was shipped with; any code in the combined initramfs inherits that trust as long as the kernel still verifies its payload. See [ADR 0001](./docs/adr/0001-runtime-architecture.md) for the full chain-of-trust rationale.

## Signing

The rescue initramfs is not itself signed — it rides inside a signed distro kernel's initramfs build, which carries the vendor signature. See `THREAT_MODEL.md` and ADR 0001 for the full chain-of-trust rationale.

`sbsigntool` is included in the image so a downstream integrator can optionally re-sign artifacts against their own MOK key.

## Troubleshooting

**Hash mismatch** — ensure `--no-cache` on both passes, that the host time zone does not leak into mounted volumes, and that no unpinned `apt-get install` slipped back in.

**Rust toolchain drift** — `RUST_VERSION` is pinned; changing it must update the SHA-256 hash of the image in `.github/workflows/reproducible-build.yml` comments or job cache keys.
