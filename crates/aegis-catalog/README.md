# aegis-catalog

Curated ISO catalog + URL resolvers for aegis-boot. The same `CATALOG`
slice is used by the host `aegis-boot recommend` CLI and the
in-rescue `Catalog` screen (#655 Phase 2) to drive ISO browse + fetch.

This crate intentionally has **no runtime dependencies** beyond `std`.
Resolver implementations shell out to `curl` for HTTPS GETs (used by
`aegis-boot recommend --refresh` on the host); the rescue-tui doesn't
call resolvers, so the curl shell-out path is dead-code from its
perspective.

The catalog policy lives in [`docs/CATALOG_POLICY.md`](../../docs/CATALOG_POLICY.md);
this crate is just the data + types + resolver framework.
