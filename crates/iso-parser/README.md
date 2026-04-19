# iso-parser

Boot entry discovery from ISO installation media. Scans directories for `.iso` files, detects distribution layouts (Arch, Debian/Ubuntu, Fedora, RHEL family, Alpine, NixOS, Mint), and extracts the kernel + initrd paths a bootloader needs to hand off via `kexec_file_load(2)`.

Part of the [aegis-boot](https://github.com/williamzujkowski/aegis-boot) rescue environment — a signed-chain UEFI Secure Boot stick that boots any ISO.

## Design

- **No unsafe.** `#![forbid(unsafe_code)]` at the crate level. This crate parses untrusted ISO content; it has no business calling raw syscalls.
- **Stateless.** No global state, no filesystem mounts, no network. Caller supplies paths; parser returns structured data.
- **No_std capable** via the `std` feature flag (on by default).

## Supported layouts

| Distribution           | Detection marker                                              |
| ---------------------- | ------------------------------------------------------------- |
| Arch Linux             | `/arch/boot/x86_64/vmlinuz-linux`                             |
| Debian / Ubuntu        | `/install/`, `/casper/`, `/.disk/`, `/pool/`, or `/dists/`    |
| Fedora                 | `/images/pxeboot/`                                            |
| RHEL / Rocky / Alma    | Same as Fedora; distinct lockdown policy recorded separately  |
| Alpine                 | `/boot/vmlinuz-lts` + `/boot/initramfs-lts`                   |
| NixOS                  | `/boot/bzImage`                                               |
| Linux Mint             | Debian-family layout in `/casper/`                            |

## Usage

```text
// Illustrative shape only. Caller supplies an `IsoEnvironment` —
// trait for mount/unmount + metadata. The production impl shells
// out to mount(8); tests use `MockIsoEnvironment` for per-test
// in-memory fixtures. See the in-repo integration tests
// (crates/iso-parser/src/lib.rs::tests) for concrete call sites.
use iso_parser::IsoParser;
use std::path::Path;

let env: MyIsoEnvironment = /* ... */;
let parser = IsoParser::new(env);
let entries = parser
    .scan_directory(Path::new("/mnt/iso"))
    .await?;
for entry in entries {
    println!("kernel: {}", entry.kernel.display());
    println!("initrd: {}", entry.initrd.display());
}
```

The `text` fence keeps this illustrative — the concrete types `MyIsoEnvironment` + the `?`/`await` plumbing are consumer-specific.

See the [API docs](https://docs.rs/iso-parser) for the full surface.

## Status

**Pre-1.0.** API is settling through real-hardware validation on the parent project's test fleet (Framework / ThinkPad / Dell). Publishing to crates.io at 1.0. Until then, consume via the [aegis-boot workspace](https://github.com/williamzujkowski/aegis-boot).

## License

Licensed under either of [Apache-2.0](../../LICENSE-APACHE) or [MIT](../../LICENSE-MIT) at your option.
