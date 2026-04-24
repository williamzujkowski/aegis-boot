// SPDX-License-Identifier: MIT OR Apache-2.0

//! macOS platform adapter for `aegis-boot flash --direct-install`.
//!
//! Counterpart to [`crate::windows_direct_install`]. Phase 1 ships
//! the pure-function layout builders; subprocess wrappers +
//! `flash_dispatcher` wiring land in follow-up PRs under [#418].
//!
//! ## Why a separate module tree
//!
//! Both platforms produce the same on-disk layout (two partitions:
//! the ESP plus `AEGIS_ISOS`), but the host tooling differs enough
//! to be worth isolating:
//!
//!   * **Windows** — `diskpart` consumes a scripted multi-line stdin.
//!     Pure-fn side builds the script as a single `String`.
//!   * **macOS** — `diskutil partitionDisk` takes the whole layout as
//!     a single argv call. Pure-fn side builds a `Vec<String>` of
//!     positional args instead.
//!
//! Sharing a common abstraction would obscure more than it saved —
//! one more indirection between a call site and the shell incantation
//! it models. Mirror the Windows module structure instead.
//!
//! [#418]: https://github.com/aegis-boot/aegis-boot/issues/418

pub(crate) mod esp_stage;
pub(crate) mod partition;
pub(crate) mod preflight;
