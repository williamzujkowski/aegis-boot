# aegis-manifest

Signed attestation manifest format for [aegis-boot](https://github.com/williamzujkowski/aegis-boot) USB sticks. Defines the on-disk `::/aegis-boot-manifest.json` shape the flash-time attestation writes and that runtime verifiers (rescue-tui, `aegis-boot doctor --stick`, aegis-hwsim E6 attestation-roundtrip) read back.

Part of the [aegis-boot](https://github.com/williamzujkowski/aegis-boot) rescue environment — a signed-chain UEFI Secure Boot stick that boots any ISO.

## Scope

This crate ships:

- **Serde types** for the manifest envelope (`Manifest`, `Device`, `EspPartition`, `DataPartition`, `EspFileEntry`, `PcrEntry`).
- **Schema version constant** pinning the wire-format version at 1 (locked by [#277](https://github.com/williamzujkowski/aegis-boot/issues/277)).
- **Optional JSON Schema generation** behind the `schema` feature — enables `#[derive(JsonSchema)]` on every public type and compiles the `aegis-manifest-schema-docgen` binary that writes `aegis-boot-manifest.schema.json` consumers can validate against.

Deliberately **not** shipped here:

- Writer / signer / filesystem I/O code — that logic is tightly coupled to the `direct_install` flow on Linux and lives in the `aegis-cli` crate.
- `minisign` signature verification — callers receive the manifest body + a detached signature and verify out-of-band.

## Feature flags

- `schema` (off by default) — pulls `schemars` in, adds `JsonSchema` derives, enables the `aegis-manifest-schema-docgen` binary used by the parent workspace's CI drift-check.

## Platform support

Pure Rust, no platform-specific code. Works anywhere serde + serde_json work.

## Status

**Pre-1.0**. Schema version is locked at 1; API may still gain minor conveniences before a crates.io publish. Consume via the [aegis-boot workspace](https://github.com/williamzujkowski/aegis-boot).

## License

Licensed under either of [Apache-2.0](../../LICENSE-APACHE) or [MIT](../../LICENSE-MIT) at your option.
