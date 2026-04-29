// SPDX-License-Identifier: MIT OR Apache-2.0

//! Library facade for the rescue-tui crate.
//!
//! Most of rescue-tui's logic lives in the `rescue-tui` binary
//! (`src/main.rs`); this lib.rs exists to expose a small set of
//! modules to sibling binaries (notably `tiers-docgen` — #462) so
//! generated docs can derive from the same `TrustVerdict` enum and
//! `KEYBINDINGS` registry the TUI itself uses. Having a lib also lets
//! integration tests exercise render paths from outside `main.rs`.
//!
//! ## What's public here
//!
//! - [`verdict`] — the 6-tier [`TrustVerdict`](verdict::TrustVerdict).
//! - [`keybindings`] — the canonical [`KEYBINDINGS`](keybindings::KEYBINDINGS) registry.
//! - [`state`] — [`AppState`](state::AppState) and supporting types.
//! - [`theme`] — color palettes.
//! - [`docgen`] — render the tier + keybinding tables as Markdown
//!   (used by the `tiers-docgen` binary).
//!
//! The event-loop / IO-heavy modules (`failure_log`, `persistence`,
//! `render`, `tpm`) are `pub` here so the `rescue-tui` binary in
//! `main.rs` can reach them, but they're not meant as external API —
//! they assume a running rescue-tui context.

#![forbid(unsafe_code)]

pub mod docgen;
pub mod failure_log;
pub mod keybindings;
pub mod network;
pub mod persistence;
pub mod render;
pub mod state;
pub mod test_mode;
pub mod theme;
pub mod tier_b_log;
pub mod tpm;
pub mod verdict;
