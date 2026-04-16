# Roadmap

A forward-looking sketch. The CHANGELOG tells what already shipped; this file says where we're going. Everything here is subject to change — file an issue if you think the priorities are wrong.

## Now (toward v1.0.0)

- **#51** Multi-vendor real-hardware shakedown — Framework / ThinkPad / Dell direct boot. Required for v1.0.0; QEMU USB-passthrough is not enough. Needs physical access; can't run from CI.
- **#132** Last-booted real-hardware E2E test — automate the manual shakedown that closed [#109](https://github.com/williamzujkowski/aegis-boot/issues/109).
- **#123** `aegis-boot flash` on macOS / Windows — currently Linux-only; cross-platform support is a v1.0 expectation.
- **`scripts/release.sh`** — manual asset upload is fine for v0.x; for v1.0.0 we want one command.
- **Reproducibility extension** — currently only `rescue-tui` is verified reproducible under SOURCE_DATE_EPOCH. Stretch: include `initramfs.cpio.gz` once we can pin the busybox version.

## Recently shipped

- **v0.12.0** — `aegis-boot` operator CLI (flash/list/add) ([#124](https://github.com/williamzujkowski/aegis-boot/issues/124), [#125](https://github.com/williamzujkowski/aegis-boot/issues/125)); installer-vs-live warning on Confirm screen ([#131](https://github.com/williamzujkowski/aegis-boot/issues/131)); post-kexec handoff banner ([#127](https://github.com/williamzujkowski/aegis-boot/issues/127)); unsigned-kernel operator guidance ([#126](https://github.com/williamzujkowski/aegis-boot/issues/126)); real-hardware shakedown closing the v1.0 boot-chain gate ([#109](https://github.com/williamzujkowski/aegis-boot/issues/109)).
- **v0.10.1** — repo cleanup ([#77](https://github.com/williamzujkowski/aegis-boot/issues/77)), branding ([#76](https://github.com/williamzujkowski/aegis-boot/issues/76)), docs accuracy audit cadence ([#78](https://github.com/williamzujkowski/aegis-boot/issues/78)).

## Later (post-1.0)

- **Architecture variants** — aarch64 build, riscv64 exploration. Separate epics, deferred until x86_64 is solid on real hardware.
- **Remote attestation** — beyond TPM PCR 12 measurement (which we already do), wire up a referenceable verifier. Probably a small companion crate.
- **Network boot / PXE** — explicitly out of scope for v1.0; reconsider if real users ask.
- **Custom signing chain** — let operators substitute their own shim → grub → kernel for environments that don't trust Microsoft's CA.

## Non-goals (probably forever)

- A full UEFI application (rejected in [ADR 0001](./docs/adr/0001-runtime-architecture.md), Option A)
- Linking `libtss2-esys` (we shell out to `tpm2_pcrextend`; see [`crates/rescue-tui/src/tpm.rs`](./crates/rescue-tui/src/tpm.rs))
- Native Windows ISO `kexec` (different boot protocol; `Quirk::NotKexecBootable` blocks it explicitly)
- A web UI / management console — this is a boot tool, not a server

## How items get on this roadmap

Open an issue. If three or more "yes" answers from [CONTRIBUTING.md's bar](./CONTRIBUTING.md), it lands here. The order within a section is rough — sequencing usually emerges from dependencies, not voting.
