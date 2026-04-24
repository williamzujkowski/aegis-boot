// SPDX-License-Identifier: MIT OR Apache-2.0

//! Windows direct-install platform adapter (#419).
//!
//! `aegis-boot flash --direct-install` for Windows hosts. Implemented
//! in phases per the #419 epic decomposition:
//!
//! - `partition` (#447) — diskpart-stdin partition-table layout
//! - `format`    (#448) — Format-Volume FAT32 + exFAT
//! - `raw_write` (#449) — `windows-rs` `CreateFileW` + `FSCTL_LOCK_VOLUME`
//! - `preflight` (#450) — elevation + `BitLocker` detection + op safety
//!
//! All submodules are `#[cfg(target_os = "windows")]`-gated in their
//! destructive side. The pure-fn builders (e.g. `partition::build_
//! diskpart_script`) compile + test on any host so the logic stays
//! reviewable from Linux.

pub(crate) mod drive_enumeration;
pub(crate) mod format;
pub(crate) mod partition;
pub(crate) mod pipeline;
pub(crate) mod preflight;
pub(crate) mod raw_write;
pub(crate) mod source_resolution;
