<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

# aegis-core

Shared utility helpers for the aegis-boot workspace. Intended as the DRY
home for cross-cutting primitives that don't belong in any one
specialized crate (`iso-parser`, `iso-probe`, `kexec-loader`, etc.) —
particularly the tiny formatting + string helpers that were previously
copy-pasted across `aegis-cli` and `rescue-tui`.

## Status

**Proof-of-concept (#556).** Established with two extracted helpers
(`short_hex`, `humanize_bytes`) so the workspace has a clean place to
land future shared utilities without resurrecting the duplication.
The maintainer's stated direction (#556 alignment, 2026-04-25):

> "Was intended to provide DRY centralization for key utilities to
> handle core tasks and potentially make the future ecosystem more
> modular and consistent."

This crate satisfies that intent for the immediately-duplicated
utilities. Future shared work — most plausibly a future netboot
sister project or operator-tooling that wants the same helpers — can
add to this crate rather than re-deriving each helper.

## Scope

Tiny pure helpers only. **Does not** belong here:
- I/O of any kind (file reads, network calls, subprocess spawning)
- Domain types from `iso-parser` / `iso-probe` / `aegis-wire-formats`
  — those have their own homes
- Anything that needs `tokio` / `serde` / `tracing` — keep dependencies
  zero so this crate is cheap to depend on from anywhere
- Anything that's only used by one crate — that's not "shared"

## Helpers

| Helper             | Purpose                                                        |
| ------------------ | -------------------------------------------------------------- |
| `short_hex(s)`     | Truncate a hex digest to 12 chars + `…` (UTF-8-safe at boundary)|
| `humanize_bytes(b)`| Format a `u64` byte count as `B` / `KiB` / `MiB` / `GiB`        |

## License

Dual-licensed MIT OR Apache-2.0 (matches the workspace).
