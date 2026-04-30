// SPDX-License-Identifier: MIT OR Apache-2.0

//! Aegis-boot catalog — public API façade.
//!
//! Phase 1 of [#701](https://github.com/aegis-boot/aegis-boot/issues/701):
//! the data + keyring previously embedded here have moved to
//! [`aegis-catalog-data`]. This crate is now a thin re-export shim so
//! consumers (`aegis-cli`, `aegis-fetch`, `rescue-tui`) keep compiling
//! against `aegis_catalog::*` without changes.
//!
//! Phase 2 will extract `aegis-catalog-data` to its own git repo
//! (`aegis-boot/aegis-catalog-data`) so the catalog can ship
//! independent of the aegis-boot release cycle. The shim layer here
//! is what makes that move non-breaking for consumers.

#![forbid(unsafe_code)]

pub use aegis_catalog_data::*;
