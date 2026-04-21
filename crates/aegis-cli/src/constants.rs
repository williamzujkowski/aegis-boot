// SPDX-License-Identifier: MIT OR Apache-2.0

//! Shared numeric constants that appear in BOTH source code AND
//! user-facing documentation.
//!
//! Phase 2 of [#286]. The v0.14.0 release cut surfaced that
//! hand-copied numbers between code and prose drift silently. This
//! module is the single home for the **numeric value** of each such
//! constant; the owning subsystem (partitioning, readback,
//! attestation, bootloader) re-exports the constant via
//! `pub(crate) use crate::constants::NAME` so call sites read
//! naturally (`direct_install::ESP_SIZE_MB`) and `grep`-archaeology
//! for a specific number lands in one file.
//!
//! ## Adding a new shared constant
//!
//! 1. Add a `pub(crate) const` here with a comment explaining the
//!    value's rationale.
//! 2. `pub(crate) use crate::constants::NAME;` in the owning module.
//! 3. Register it in [`crates/aegis-cli/src/bin/constants_docgen.rs`]
//!    so docs regenerate from it.
//! 4. Wrap the value in the target docs with HTML markers:
//!    `<!-- constants:BEGIN:NAME -->...<!-- constants:END:NAME -->`
//! 5. Run `cargo run -p aegis-cli --bin constants-docgen
//!    --features docgen -- --write` to render the markers.
//!
//! ## Not included here
//!
//! Per-subsystem *contract* versions (`attest::SCHEMA_VERSION`,
//! `direct_install_manifest::SCHEMA_VERSION`) stay in their owning
//! modules. They are independent wire-format versions, not shared
//! infrastructure values — merging them would fuse two contracts
//! that must be bumpable independently.
//!
//! [#286]: https://github.com/williamzujkowski/aegis-boot/issues/286

// NOTE: this file is `#[path]`-included by
// `crates/aegis-cli/src/bin/constants_docgen.rs` so the docgen can
// import the numeric values without going through a library target.
// Keep this file dependency-free (no `use` statements, no imports)
// so the include stays a self-contained compilation unit.

// Several of these constants are consumed by Linux-only modules
// (`direct_install`, `direct_install_manifest`) that are cfg-gated
// out on macOS/Windows. The `#[cfg_attr(not(target_os = "linux"),
// allow(dead_code))]` attribute silences the dead-code warning on
// non-Linux cross-compile targets without suppressing it on the
// primary Linux target — a genuine unused constant on Linux is still
// a signal. The docgen bin (`--features docgen`) references them on
// every target, but CI cross-checks don't build that feature.

/// ESP partition size in megabytes. Matches `scripts/mkusb.sh`'s
/// `ESP_SIZE_MB` default. 400 MB is enough for the signed chain
/// (shim ~1 MB, grub ~2 MB, kernel ~15 MB, initrd ~60 MB) plus
/// comfortable headroom for future binary growth.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) const ESP_SIZE_MB: u64 = 400;

/// Default number of bytes to read back after a write (post-flash
/// integrity check). Sized to comfortably cover the signed-chain
/// payload (shim + grub + kernel + initramfs ≈ 50 MB) with margin,
/// while keeping the readback under ~10 s on a slow USB 2.0 stick
/// (~7 MB/s).
pub(crate) const DEFAULT_READBACK_BYTES: u64 = 64 * 1024 * 1024;

/// Hard cap on attestation-manifest body size. The verifier refuses
/// to parse a manifest larger than this — bounds the JSON-parser
/// attack surface in the early-boot rescue-tui code path. 64 KiB is
/// ~100× the expected body size so legitimate future growth has room.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) const MAX_MANIFEST_BYTES: usize = 64 * 1024;

/// GRUB menu timeout in seconds. Short enough that an interactive
/// user doesn't wait, long enough that an operator who wants to
/// interrupt the default boot can do so. Matches the value baked
/// into the rendered `grub.cfg` on the ESP.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) const GRUB_TIMEOUT_SECS: u32 = 3;
