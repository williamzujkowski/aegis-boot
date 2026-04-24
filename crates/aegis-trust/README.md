# aegis-trust

Runtime trust anchor for aegis-boot — the Rust library that implements [ADR 0002](../../docs/architecture/KEY_MANAGEMENT.md)'s epoch-aware minisign verification model.

## What it is

A small, focused crate (~400 LOC + tests) that answers two questions at runtime:

1. **Is this signed body trustworthy?** — `TrustAnchor::verify_with_epoch(body, sig, epoch)` runs the rotation-aware check: epoch is at or above the binary-embedded floor, at or above the locally-seen floor, and the signature verifies under the epoch's pubkey.
2. **What's the current epoch floor?** — `TrustAnchor::current_floor()` returns `max(MIN_REQUIRED_EPOCH, seen_epoch)`, the value the binary refuses to accept below.

## Who uses it

- **#417** — runtime signed-chain downloader; verifies the bundle manifest before flashing.
- **#349 successor** — attestation manifest verify path; migrates off the bare `minisign::PublicKey` handle.
- **`aegis-boot doctor`** — surfaces binary + local + remote epoch values and flags drift.

## What it reads at build time

- `keys/canonical-epoch.json` — the workspace-root file committed alongside `keys/maintainer-epoch-1.pub`. `build.rs` reads the `epoch` field and emits it as `AEGIS_MIN_REQUIRED_EPOCH`, which the crate exposes as `pub const MIN_REQUIRED_EPOCH: u32`.

## What it reads at runtime

- `keys/historical-anchors.json` — embedded via `include_str!` at build time so the binary ships the full epoch history without file-system dependencies.
- `keys/maintainer-epoch-<N>.pub` — each epoch's pubkey is embedded the same way.
- `$XDG_STATE_HOME/aegis-boot/trust/seen-epoch` — read-and-written at verify time; monotonic u32.

## What it does NOT do

- Key generation (maintainer tooling; see `docs/architecture/KEY_MANAGEMENT.md`).
- Network fetches to validate freshness against a canonical source (tracked as a follow-up once a public trust-anchor-status endpoint exists).
- Sign anything — only verify.
