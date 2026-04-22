# iso-probe

Runtime ISO discovery for a live rescue environment. Given a set of root paths (typically the AEGIS_ISOS partition mounted at `/run/media/aegis-isos`), finds every `.iso`, loop-mounts it once, extracts per-ISO boot metadata via [iso-parser](https://crates.io/crates/iso-parser), and returns metadata records suitable for display in a TUI.

Part of the [aegis-boot](https://github.com/aegis-boot/aegis-boot) rescue environment — a signed-chain UEFI Secure Boot stick that boots any ISO.

## Two-phase API

1. **`discover`** — scan roots, mount each ISO once, extract kernel/initrd/cmdline paths, unmount. Returns `DiscoveredIso` records — metadata only, no live mounts. Safe to display as a picker.
2. **`prepare`** — given a user-selected `DiscoveredIso`, re-mount the ISO and return a `PreparedIso` whose absolute paths can be fed to [`kexec-loader::load_and_exec`](https://crates.io/crates/kexec-loader). Mount persists until `PreparedIso` is dropped, or until `kexec` replaces the process on the success path.

## Design

- **Forbid unsafe.** Mounting + loopback + path manipulation only. No raw syscalls.
- **Sidecar verification.** If `<iso>.sha256` or `<iso>.minisig` is present next to the ISO, verifies before reporting. Uses [`minisign-verify`](https://crates.io/crates/minisign-verify) for Ed25519 signatures and [`sha2`](https://crates.io/crates/sha2) for digests.
- **Sync-only API.** Callers drive async elsewhere if they want; `pollster` is available for sync-over-async bridging.

## Usage

```text
// Illustrative shape only. Types and paths are consumer-specific;
// the real API is documented in the `discover` and `prepare` items
// below.
use iso_probe::{discover, prepare};

let discovered = discover(&["/run/media/aegis-isos"])?;
for iso in &discovered {
    println!("{} ({})", iso.label, iso.verification.display_summary());
}

// Operator picks one:
let prepared = prepare(&discovered[0])?;
kexec_loader::load_and_exec(&prepared.kernel, &prepared.initrd, &prepared.cmdline)?;
```

See the [API docs](https://docs.rs/iso-probe) for the full surface.

## Status

**Pre-1.0.** API is settling through real-hardware validation on the parent project's test fleet. Publishing to crates.io at 1.0. Until then, consume via the [aegis-boot workspace](https://github.com/aegis-boot/aegis-boot).

## License

Licensed under either of [Apache-2.0](../../LICENSE-APACHE) or [MIT](../../LICENSE-MIT) at your option.
