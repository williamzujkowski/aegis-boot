// SPDX-License-Identifier: MIT OR Apache-2.0

//! Canonical keybinding registry — the single source of truth for
//! the rescue-tui footer legend, the help overlay, and (via #462
//! `tiers-docgen`) the published keybinding reference.
//!
//! Adding a new keybinding:
//! 1. Add a row to [`KEYBINDINGS`].
//! 2. Implement the dispatch in `main.rs` (event handling is
//!    match-based; unifying match + registry is tracked separately).
//! 3. `tiers-docgen` picks up the new row automatically.
//!
//! The registry is read by [`draw_footer`](crate::render) which
//! filters by `(screen_kind, pane, filter_editing)` so the footer
//! legend always matches what's actually available in the current
//! context.
//!
//! ## Doc-friendly shape
//!
//! Every entry pairs a short `label` (footer-fit) with a long-form
//! `description` (help overlay + docs). Both fields are `'static`
//! string slices so the whole registry compiles down to
//! `.rodata` — no allocation at render time.

use crate::state::{Pane, Screen};

/// Simplified screen tag used for keybinding filtering. Mirrors
/// [`Screen`]'s variants but drops the data payloads since bindings
/// depend only on *which* screen, not its current contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ScreenKind {
    /// List of ISOs (dual-pane).
    List,
    /// Confirm-and-boot screen.
    Confirm,
    /// Edit kernel cmdline overlay.
    EditCmdline,
    /// Error screen after a kexec or verify failure.
    Error,
    /// Verify-now progress screen.
    Verifying,
    /// Typed-confirmation trust challenge for tier 2/3 ISOs.
    TrustChallenge,
    /// Help overlay.
    Help,
    /// Quit-confirmation overlay.
    ConfirmQuit,
    /// Quitting (terminal state; no bindings).
    Quitting,
    /// "Cannot boot" toast over a tier-4 row. (#546)
    BlockedToast,
    /// Consent screen for elevated-risk boot paths (#347).
    Consent,
    /// Confirm-before-delete prompt for an ISO on the data partition.
    ConfirmDelete,
}

impl ScreenKind {
    /// Derive the kind from the full [`Screen`] variant (ignoring the
    /// underlying payload).
    pub(crate) fn from_screen(s: &Screen) -> Self {
        match s {
            Screen::List { .. } => Self::List,
            Screen::Confirm { .. } => Self::Confirm,
            Screen::EditCmdline { .. } => Self::EditCmdline,
            Screen::Error { .. } => Self::Error,
            Screen::Verifying { .. } => Self::Verifying,
            Screen::TrustChallenge { .. } => Self::TrustChallenge,
            Screen::Help { .. } => Self::Help,
            Screen::ConfirmQuit { .. } => Self::ConfirmQuit,
            Screen::Quitting => Self::Quitting,
            Screen::BlockedToast { .. } => Self::BlockedToast,
            Screen::Consent { .. } => Self::Consent,
            Screen::ConfirmDelete { .. } => Self::ConfirmDelete,
        }
    }
}

/// A single keybinding entry. Lives in the static [`KEYBINDINGS`]
/// slice — all fields are `'static`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Keybinding {
    /// Human-readable key glyph — what the user sees in the footer.
    /// Examples: `"↑↓"`, `"Tab"`, `"Enter"`, `"/"`, `"q"`.
    pub(crate) key: &'static str,
    /// Short label (one or two words). Rendered in the footer legend.
    pub(crate) label: &'static str,
    /// Long-form description. Rendered in the help overlay + docs.
    pub(crate) description: &'static str,
    /// Screens this binding applies to. Empty slice = all screens.
    pub(crate) screens: &'static [ScreenKind],
    /// If `Some`, binding only matters when focus is on this pane.
    /// `None` = pane-agnostic (works regardless of focus).
    pub(crate) pane: Option<Pane>,
    /// If `true`, binding only applies while the filter input is open.
    /// Default `false` (applies when filter is not being edited).
    pub(crate) while_filter_editing: bool,
}

/// Canonical keybinding table. `tiers-docgen` (#462) renders this as
/// the published reference; [`filter_for`] is what the footer reads
/// at render time.
pub(crate) const KEYBINDINGS: &[Keybinding] = &[
    // ---- Universal bindings (all screens) --------------------------
    Keybinding {
        key: "?",
        label: "Help",
        description: "Show the help overlay with all keybindings",
        screens: &[],
        pane: None,
        while_filter_editing: false,
    },
    Keybinding {
        key: "q",
        label: "Quit",
        description: "Quit rescue-tui (returns control to the boot menu)",
        screens: &[],
        pane: None,
        while_filter_editing: false,
    },
    // ---- List screen -----------------------------------------------
    Keybinding {
        key: "↑↓/jk",
        label: "Move",
        description: "Move the list cursor (pane=List) or scroll info pane (pane=Info)",
        screens: &[ScreenKind::List],
        pane: None,
        while_filter_editing: false,
    },
    Keybinding {
        key: "Tab",
        label: "Focus",
        description: "Toggle focus between the ISO list and the info pane",
        screens: &[ScreenKind::List],
        pane: None,
        while_filter_editing: false,
    },
    Keybinding {
        key: "Enter",
        label: "Boot",
        description: "Confirm the selected ISO (only valid in the list pane)",
        screens: &[ScreenKind::List],
        pane: Some(Pane::List),
        while_filter_editing: false,
    },
    Keybinding {
        key: "/",
        label: "Filter",
        description: "Open the substring filter — typed chars match label + path",
        screens: &[ScreenKind::List],
        pane: None,
        while_filter_editing: false,
    },
    Keybinding {
        key: "s",
        label: "Sort",
        description: "Cycle sort order: name → size → distro → name",
        screens: &[ScreenKind::List],
        pane: None,
        while_filter_editing: false,
    },
    Keybinding {
        key: "v",
        label: "Verify",
        description: "Re-compute sha256 of the selected ISO in a background thread",
        screens: &[ScreenKind::List, ScreenKind::Confirm],
        pane: None,
        while_filter_editing: false,
    },
    Keybinding {
        key: "D",
        label: "Delete",
        description: "Delete the highlighted ISO + sidecar from the data partition (confirm prompt)",
        screens: &[ScreenKind::List],
        pane: Some(Pane::List),
        while_filter_editing: false,
    },
    // ---- ConfirmDelete prompt --------------------------------------
    Keybinding {
        key: "y",
        label: "Delete",
        description: "Confirm — unlink the ISO and its `.aegis.toml` sidecar",
        screens: &[ScreenKind::ConfirmDelete],
        pane: None,
        while_filter_editing: false,
    },
    Keybinding {
        key: "n/Esc",
        label: "Cancel",
        description: "Cancel — return to the list without deleting",
        screens: &[ScreenKind::ConfirmDelete],
        pane: None,
        while_filter_editing: false,
    },
    // ---- Filter-editing overlay ------------------------------------
    Keybinding {
        key: "Enter",
        label: "Commit",
        description: "Commit the current filter and close the input",
        screens: &[ScreenKind::List],
        pane: None,
        while_filter_editing: true,
    },
    Keybinding {
        key: "Esc",
        label: "Clear",
        description: "Close the filter input and clear the current filter",
        screens: &[ScreenKind::List],
        pane: None,
        while_filter_editing: true,
    },
    // ---- Confirm screen --------------------------------------------
    Keybinding {
        key: "Enter",
        label: "kexec",
        description: "Kexec into the selected ISO (may trigger a trust challenge)",
        screens: &[ScreenKind::Confirm],
        pane: None,
        while_filter_editing: false,
    },
    Keybinding {
        key: "e",
        label: "cmdline",
        description: "Edit the kernel command line before boot",
        screens: &[ScreenKind::Confirm],
        pane: None,
        while_filter_editing: false,
    },
    Keybinding {
        key: "Esc/h",
        label: "Back",
        description: "Return to the list without booting",
        screens: &[ScreenKind::Confirm, ScreenKind::Error],
        pane: None,
        while_filter_editing: false,
    },
    // ---- EditCmdline overlay ---------------------------------------
    Keybinding {
        key: "Enter",
        label: "Save",
        description: "Save the edited kernel command line and return to Confirm",
        screens: &[ScreenKind::EditCmdline],
        pane: None,
        while_filter_editing: false,
    },
    Keybinding {
        key: "Esc",
        label: "Cancel",
        description: "Discard edits and return to Confirm",
        screens: &[
            ScreenKind::EditCmdline,
            ScreenKind::Verifying,
            ScreenKind::TrustChallenge,
        ],
        pane: None,
        while_filter_editing: false,
    },
    // ---- Error screen ----------------------------------------------
    Keybinding {
        key: "F10",
        label: "Save evidence",
        description: "Write a failure-log bundle to AEGIS_ISOS for post-mortem analysis",
        screens: &[ScreenKind::Error],
        pane: None,
        while_filter_editing: false,
    },
    // ---- TrustChallenge --------------------------------------------
    Keybinding {
        key: "boot+Enter",
        label: "Proceed",
        description: "Type the word 'boot' and press Enter to proceed past the trust challenge",
        screens: &[ScreenKind::TrustChallenge],
        pane: None,
        while_filter_editing: false,
    },
];

/// Return all bindings that apply in the given context, preserving
/// the registry's declared order (which is also the order rendered
/// in the footer and docs).
pub(crate) fn filter_for(
    screen: ScreenKind,
    pane: Pane,
    filter_editing: bool,
) -> impl Iterator<Item = &'static Keybinding> {
    KEYBINDINGS.iter().filter(move |k| {
        if k.while_filter_editing != filter_editing {
            return false;
        }
        if !k.screens.is_empty() && !k.screens.contains(&screen) {
            return false;
        }
        if let Some(required) = k.pane
            && required != pane
        {
            return false;
        }
        true
    })
}

/// Render the filtered bindings as a single-line footer legend —
/// `[key] label · [key] label · …`. Used by
/// [`crate::render::draw_footer`].
///
/// Terminal states ([`ScreenKind::Quitting`], [`ScreenKind::Help`],
/// [`ScreenKind::ConfirmQuit`]) produce an empty footer — the prior
/// screen's bindings are surfaced by the overlay routing in
/// `draw_footer` (it reads `prior`, not the overlay screen itself).
pub(crate) fn footer_line(screen: ScreenKind, pane: Pane, filter_editing: bool) -> String {
    if matches!(
        screen,
        ScreenKind::Quitting | ScreenKind::Help | ScreenKind::ConfirmQuit
    ) {
        return String::new();
    }
    let mut parts: Vec<String> = filter_for(screen, pane, filter_editing)
        .map(|k| format!("[{}] {}", k.key, k.label))
        .collect();
    if parts.is_empty() {
        return String::new();
    }
    parts.insert(0, String::new()); // leading space for breathing room
    parts.join("  ")
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn every_keybinding_has_non_empty_fields() {
        for k in KEYBINDINGS {
            assert!(!k.key.is_empty(), "empty key: {k:?}");
            assert!(!k.label.is_empty(), "empty label: {k:?}");
            assert!(!k.description.is_empty(), "empty description: {k:?}");
        }
    }

    #[test]
    fn filter_for_list_pane_shows_boot() {
        let bindings: Vec<&Keybinding> = filter_for(ScreenKind::List, Pane::List, false).collect();
        assert!(
            bindings.iter().any(|k| k.label == "Boot"),
            "Boot binding missing when list pane focused"
        );
        // Tab/Move are pane-agnostic so they show regardless.
        assert!(bindings.iter().any(|k| k.label == "Focus"));
        assert!(bindings.iter().any(|k| k.label == "Move"));
    }

    #[test]
    fn filter_for_info_pane_hides_boot() {
        // Enter = Boot only applies when list pane holds focus.
        let bindings: Vec<&Keybinding> = filter_for(ScreenKind::List, Pane::Info, false).collect();
        assert!(
            !bindings.iter().any(|k| k.label == "Boot"),
            "Boot binding must be hidden when info pane holds focus"
        );
        // But Move + Focus are still available.
        assert!(bindings.iter().any(|k| k.label == "Move"));
        assert!(bindings.iter().any(|k| k.label == "Focus"));
    }

    #[test]
    fn filter_for_filter_editing_shows_commit_clear() {
        let bindings: Vec<&Keybinding> = filter_for(ScreenKind::List, Pane::List, true).collect();
        assert!(bindings.iter().any(|k| k.label == "Commit"));
        assert!(bindings.iter().any(|k| k.label == "Clear"));
        // Normal navigation bindings hidden — they don't apply here.
        assert!(!bindings.iter().any(|k| k.label == "Move"));
    }

    #[test]
    fn filter_for_confirm_screen_shows_kexec() {
        let bindings: Vec<&Keybinding> =
            filter_for(ScreenKind::Confirm, Pane::List, false).collect();
        assert!(bindings.iter().any(|k| k.label == "kexec"));
        assert!(bindings.iter().any(|k| k.label == "cmdline"));
        assert!(bindings.iter().any(|k| k.label == "Back"));
        // List-only bindings hidden.
        assert!(!bindings.iter().any(|k| k.label == "Filter"));
    }

    #[test]
    fn footer_line_not_empty_for_list_screen() {
        let line = footer_line(ScreenKind::List, Pane::List, false);
        assert!(line.contains("[Tab]"));
        assert!(line.contains("[?]"));
        assert!(line.contains("[q]"));
    }

    #[test]
    fn footer_line_empty_on_quitting() {
        assert_eq!(
            footer_line(ScreenKind::Quitting, Pane::List, false),
            "",
            "Quitting state has no applicable bindings"
        );
    }

    #[test]
    fn screen_kind_maps_every_screen_variant() {
        // Exhaustiveness: every Screen variant must produce a
        // distinct ScreenKind so the filter is well-defined.
        let kinds = [
            ScreenKind::from_screen(&Screen::List { selected: 0 }),
            ScreenKind::from_screen(&Screen::Confirm { selected: 0 }),
            ScreenKind::from_screen(&Screen::EditCmdline {
                selected: 0,
                buffer: String::new(),
                cursor: 0,
            }),
            ScreenKind::from_screen(&Screen::Error {
                message: String::new(),
                remedy: None,
                return_to: 0,
            }),
            ScreenKind::from_screen(&Screen::Verifying {
                selected: 0,
                bytes: 0,
                total: 0,
                result: None,
            }),
            ScreenKind::from_screen(&Screen::TrustChallenge {
                selected: 0,
                buffer: String::new(),
            }),
            ScreenKind::from_screen(&Screen::Quitting),
            ScreenKind::from_screen(&Screen::ConfirmDelete { selected: 0 }),
        ];
        // Sanity: 8 inputs → 8 distinct kinds (Help / ConfirmQuit also
        // exist but require a `prior` Screen we'd need to construct).
        let distinct: std::collections::HashSet<ScreenKind> = kinds.iter().copied().collect();
        assert_eq!(distinct.len(), 8);
    }
}
