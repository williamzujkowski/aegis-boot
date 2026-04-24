// SPDX-License-Identifier: MIT OR Apache-2.0

//! Pure state machine for the rescue TUI — no rendering, no I/O.
//!
//! Split from rendering so unit tests can cover every transition without a
//! TTY or a `TestBackend`.

use iso_probe::{DiscoveredIso, HashVerification, Quirk, SignatureVerification};
use kexec_loader::KexecError;

use crate::theme::Theme;

/// Top-level UI state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Screen {
    /// Browsing the discovered ISO list.
    List {
        /// Index of the highlighted row.
        selected: usize,
    },
    /// Confirming a kexec for the selected ISO.
    Confirm {
        /// Index into [`AppState::isos`] of the ISO under confirmation.
        selected: usize,
    },
    /// Editing the kernel command line before kexec.
    EditCmdline {
        /// Index into [`AppState::isos`] of the ISO under edit.
        selected: usize,
        /// Current buffer (starts with the ISO-declared cmdline).
        buffer: String,
        /// Byte-offset cursor position within the buffer.
        cursor: usize,
    },
    /// Showing a fatal-or-classified error after attempted kexec. The
    /// `return_to` index lets the operator return to the failed ISO row
    /// rather than the top of the list. (#85)
    Error {
        /// User-facing diagnostic copied from [`error_diagnostic`].
        message: String,
        /// User-facing remedy hint, if any.
        remedy: Option<String>,
        /// Index of the ISO whose kexec attempt produced this error, so
        /// returning to the List preserves the operator's place.
        return_to: usize,
    },
    /// Help overlay (`?` from any non-edit screen). Stores the screen
    /// to return to so dismissal restores prior state. (#85)
    Help {
        /// Boxed screen to restore on dismiss; boxed to keep the
        /// `Help` variant the same size as the others.
        prior: Box<Screen>,
    },
    /// Quit confirmation prompt. (#85 — was instant exit before.)
    ConfirmQuit {
        /// Boxed screen to restore on cancel.
        prior: Box<Screen>,
    },
    /// Typed-confirmation challenge before kexec'ing an ISO whose
    /// trust state is degraded (YELLOW untrusted signer, GRAY no
    /// verification material). Operator must type `boot` exactly to
    /// continue — prevents muscle-memory Enter on a warned state.
    /// SSH first-connect / HSTS / Gatekeeper "type the app name"
    /// pattern. (#93)
    TrustChallenge {
        /// Real ISO index being kexec'd.
        selected: usize,
        /// Current input buffer (characters typed so far).
        buffer: String,
    },
    /// Verify-now action (#89) — a worker thread streams progress
    /// while re-running hash verification against the selected ISO.
    Verifying {
        /// Real ISO index (not visible-view index) being verified.
        selected: usize,
        /// Bytes hashed so far.
        bytes: u64,
        /// Total bytes to hash (0 if unknown).
        total: u64,
        /// Populated when the worker finishes — caller then updates
        /// the ISO's verification fields in place and dismisses the
        /// screen.
        result: Option<HashVerification>,
    },
    /// User asked to quit; main loop should exit cleanly.
    Quitting,
}

/// Top-level application state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppState {
    /// All discovered ISOs.
    pub isos: Vec<DiscoveredIso>,
    /// `.iso` files on disk that iso-parser couldn't extract boot
    /// entries from (mount failure, unfamiliar layout, truncated
    /// file). Carried alongside [`Self::isos`] so the TUI can render
    /// tier-4 rows with the per-file reason — the list stays
    /// exhaustive even when parse fails. Populated from
    /// [`iso_probe::DiscoveryReport::failed`] in `main.rs`. (#457)
    pub failed_isos: Vec<iso_probe::FailedIso>,
    /// Current screen.
    pub screen: Screen,
    /// Per-ISO cmdline overrides keyed by index. When absent the
    /// ISO-declared default from `iso-probe` is used.
    pub cmdline_overrides: std::collections::HashMap<usize, String>,
    /// Active color theme (resolved from `AEGIS_THEME` env var).
    pub theme: Theme,
    /// Secure Boot enforcement status, detected once at startup.
    /// Rendered in the persistent header banner. (#85)
    pub secure_boot: SecureBootStatus,
    /// TPM availability, detected once at startup. Rendered in the
    /// persistent header banner. (#85)
    pub tpm: TpmStatus,
    /// Set when the operator picked the rescue-shell entry (#90).
    /// Causes `main.rs::run()` to exit with [`RESCUE_SHELL_EXIT_CODE`]
    /// so the initramfs `/init` script can drop to busybox.
    pub shell_requested: bool,
    /// Active substring filter on the List screen (#85 Tier 2).
    /// Empty means show all ISOs. Matches against label + path,
    /// case-insensitive.
    pub filter: String,
    /// True while the user is actively typing into the filter input
    /// (`/` opened, Enter / Esc closes).
    pub filter_editing: bool,
    /// Sort order for the List screen view. Cycled with `s`.
    pub sort_order: SortOrder,
    /// Paths that `iso_probe::discover()` was asked to scan. Surfaced
    /// in the empty-state screen so operators see where we looked
    /// (rather than "no bootable ISOs found" with no actionable
    /// detail). Populated by `main.rs` after reading `AEGIS_ISO_ROOTS`;
    /// default empty in tests. (#85 Tier 2)
    pub scanned_roots: Vec<std::path::PathBuf>,
    /// Count of `.iso` files found on disk under `scanned_roots` that
    /// DID NOT parse into a `DiscoveredIso` (malformed layout, mount
    /// failure, etc). Sourced from
    /// [`iso_probe::DiscoveryReport::failed`]'s length plus any
    /// additional files the on-disk walk saw that iso-parser never
    /// attempted. Surfaced as a yellow inline band above the List
    /// when >0 so the operator knows the list is incomplete.
    /// (#85 Tier 2 last child.) #457 will replace this banner with
    /// per-ISO tier-4 rows sourced from `DiscoveryReport::failed`.
    pub skipped_iso_count: usize,
    /// Which pane holds keyboard focus on the List screen. Defaults to
    /// [`Pane::List`]; toggled with `Tab`. (#458)
    pub pane: Pane,
    /// Vertical scroll offset for the info pane (tier-specific detail
    /// view). Increments when `↑`/`↓` are pressed while `pane` is
    /// [`Pane::Info`]. Resets to 0 when the list selection changes —
    /// per-ISO scrollback would be confusing. (#458 / #459)
    pub info_scroll: u16,
}

/// Sort order applied to the List view. Cycled with the `s` key.
/// Persisted only in-process — not saved across reboots (operators may
/// want different defaults per use). (#85 Tier 2)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortOrder {
    /// Alphabetical by label (default).
    Name,
    /// Largest ISOs first — usually the "main" install media.
    SizeDesc,
    /// Group by distribution family.
    Distro,
}

/// An entry displayed on the List screen. Includes discovered ISOs
/// (by real index) and synthetic entries like the always-present
/// rescue shell (#90).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewEntry {
    /// Real ISO at this index into [`AppState::isos`].
    Iso(usize),
    /// Synthetic "drop to rescue shell" entry (#90). Selecting it
    /// exits rescue-tui with [`RESCUE_SHELL_EXIT_CODE`] so the
    /// initramfs `/init` can recognize the operator request and
    /// `exec /bin/sh`.
    RescueShell,
}

/// Exit code rescue-tui returns when the operator picks the synthetic
/// "rescue shell" entry. `/init` switches on this value and drops to
/// busybox instead of treating the exit as an error. (#90)
pub const RESCUE_SHELL_EXIT_CODE: i32 = 42;

impl SortOrder {
    /// Cycle to the next sort order.
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::Name => Self::SizeDesc,
            Self::SizeDesc => Self::Distro,
            Self::Distro => Self::Name,
        }
    }

    /// Short label for the header / footer.
    #[must_use]
    pub fn summary(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::SizeDesc => "size↓",
            Self::Distro => "distro",
        }
    }
}

/// Which pane holds keyboard focus on the List screen. List selection
/// moves with ↑↓ when focus is `List`; the info-pane scroll offset
/// moves with ↑↓ when focus is `Info`. `Tab` toggles. (#458)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pane {
    /// Left pane — ISO list. Default focus at startup.
    #[default]
    List,
    /// Right pane — info/detail view for the selected ISO.
    Info,
}

impl Pane {
    /// Swap between `List` and `Info`. Used by the `Tab` key handler.
    #[must_use]
    pub fn toggle(self) -> Self {
        match self {
            Self::List => Self::Info,
            Self::Info => Self::List,
        }
    }
}

/// Coarse Secure Boot enforcement state, detected once at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecureBootStatus {
    /// EFI variable says SB is enforcing.
    Enforcing,
    /// EFI variable present but SB disabled / setup mode.
    Disabled,
    /// Couldn't read EFI vars (not booted via UEFI, or efivars not mounted).
    Unknown,
}

/// TPM availability for pre-kexec PCR measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TpmStatus {
    /// `/dev/tpm0` or `/dev/tpmrm0` present.
    Available,
    /// No TPM device exposed.
    Absent,
}

impl SecureBootStatus {
    /// Detect SB status from /sys/firmware/efi/efivars. Best-effort —
    /// returns Unknown on any read error so the TUI can boot anywhere.
    ///
    /// Checks, in order:
    ///   1. Global-variables-namespace GUID suffix
    ///      (`SecureBoot-8be4df61-…`) — the upstream spec name.
    ///   2. Plain `SecureBoot` — seen on some older kernels / distros
    ///      that rename on mount.
    ///   3. Directory scan for anything starting with `SecureBoot-` —
    ///      catches OVMF firmware builds that use a different-looking
    ///      suffix (observed under our QEMU `SecBoot` shakedown, #118).
    ///
    /// EFI var wire format for all three cases is identical: 4 bytes
    /// of attributes, then one byte of data (0/1). We read `bytes[4]`.
    #[must_use]
    pub fn detect() -> Self {
        // EFI var format: 4 bytes attributes, 1 byte data (0/1).
        let candidates = [
            "/sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c",
            "/sys/firmware/efi/efivars/SecureBoot",
        ];
        for path in candidates {
            if let Some(s) = Self::read_sb_bit(std::path::Path::new(path)) {
                return s;
            }
        }
        // Fallback: scan the efivars directory for any variable whose
        // filename starts with "SecureBoot-". Covers OVMF firmware
        // builds that publish the variable under a different suffix
        // than the upstream-spec one we check above. (#118)
        if let Ok(entries) = std::fs::read_dir("/sys/firmware/efi/efivars") {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_s = name.to_string_lossy();
                if name_s.starts_with("SecureBoot-") {
                    if let Some(s) = Self::read_sb_bit(&entry.path()) {
                        return s;
                    }
                }
            }
        }
        Self::Unknown
    }

    /// Read and interpret the 5-byte `[attrs][value]` layout at `path`.
    /// Returns `None` if the file can't be read or is truncated.
    /// Extracted so `detect`'s scan-fallback can reuse the same parse
    /// without duplicating the magic-offset read.
    fn read_sb_bit(path: &std::path::Path) -> Option<Self> {
        let bytes = std::fs::read(path).ok()?;
        let value = *bytes.get(4)?;
        Some(if value == 1 {
            Self::Enforcing
        } else {
            Self::Disabled
        })
    }

    /// Short user-facing label for the TUI header banner.
    #[must_use]
    pub fn summary(self) -> &'static str {
        match self {
            Self::Enforcing => "SB:enforcing",
            Self::Disabled => "SB:disabled",
            Self::Unknown => "SB:unknown",
        }
    }
}

impl TpmStatus {
    /// Detect TPM presence by checking for `/dev/tpm*` device nodes.
    #[must_use]
    pub fn detect() -> Self {
        if std::path::Path::new("/dev/tpm0").exists()
            || std::path::Path::new("/dev/tpmrm0").exists()
        {
            Self::Available
        } else {
            Self::Absent
        }
    }

    /// Short user-facing label for the TUI header banner.
    #[must_use]
    pub fn summary(self) -> &'static str {
        match self {
            Self::Available => "TPM:available",
            Self::Absent => "TPM:none",
        }
    }
}

impl AppState {
    /// Build a fresh state from a discovery result.
    #[must_use]
    pub fn new(isos: Vec<DiscoveredIso>) -> Self {
        Self {
            isos,
            failed_isos: Vec::new(),
            screen: Screen::List { selected: 0 },
            cmdline_overrides: std::collections::HashMap::new(),
            theme: Theme::default_theme(),
            secure_boot: SecureBootStatus::detect(),
            tpm: TpmStatus::detect(),
            shell_requested: false,
            filter: String::new(),
            filter_editing: false,
            sort_order: SortOrder::Name,
            scanned_roots: Vec::new(),
            skipped_iso_count: 0,
            pane: Pane::default(),
            info_scroll: 0,
        }
    }

    /// Attach the list of ISOs that iso-parser couldn't extract boot
    /// entries from — carried alongside the successful ISOs so the
    /// TUI can render tier-4 rows in the list instead of hiding them
    /// behind a count. Populated from
    /// [`iso_probe::DiscoveryReport::failed`] in `main.rs`. Tests
    /// default to an empty vec via [`Self::new`]. (#457)
    #[must_use]
    pub fn with_failed_isos(mut self, failed: Vec<iso_probe::FailedIso>) -> Self {
        self.failed_isos = failed;
        self
    }

    /// Attach the paths `discover()` was asked to scan. Called from
    /// `main.rs` before entering the event loop so the empty-state
    /// screen can tell the operator where we looked. Safe to skip in
    /// tests — field defaults to empty and renders a no-paths variant.
    /// (#85 Tier 2)
    #[must_use]
    pub fn with_scanned_roots(mut self, roots: Vec<std::path::PathBuf>) -> Self {
        self.scanned_roots = roots;
        self
    }

    /// Record how many `.iso` files on disk were silently skipped by
    /// `discover()` (malformed ISO, unsupported layout, parse failure).
    /// Populated by `main.rs` — tests default to 0 via `AppState::new()`.
    /// (#85 Tier 2 last child — inline error band)
    #[must_use]
    pub fn with_skipped_iso_count(mut self, n: usize) -> Self {
        self.skipped_iso_count = n;
        self
    }

    /// Ordered list of entries displayed on the List screen (#90).
    /// Mix of [`ViewEntry::Iso`] (one per filter-matching ISO in sort
    /// order) followed by the synthetic [`ViewEntry::RescueShell`] at
    /// the end. The rescue-shell entry is always present — even when
    /// no ISOs are discovered — so operators always have an escape
    /// hatch to a signed busybox shell.
    #[must_use]
    pub fn visible_entries(&self) -> Vec<ViewEntry> {
        let mut entries: Vec<ViewEntry> = self
            .visible_indices()
            .into_iter()
            .map(ViewEntry::Iso)
            .collect();
        entries.push(ViewEntry::RescueShell);
        entries
    }

    /// Translate a List-cursor position to the selected [`ViewEntry`].
    #[must_use]
    pub fn view_entry(&self, cursor: usize) -> Option<ViewEntry> {
        self.visible_entries().into_iter().nth(cursor)
    }

    /// Indices into [`Self::isos`] in display order — applies both the
    /// substring filter and the active sort. This is the ISO-only view
    /// (no rescue-shell entry). Use [`Self::visible_entries`] when
    /// rendering or navigating. (#85 Tier 2)
    #[must_use]
    pub fn visible_indices(&self) -> Vec<usize> {
        let needle = self.filter.to_ascii_lowercase();
        let mut indices: Vec<usize> = self
            .isos
            .iter()
            .enumerate()
            .filter(|(_, iso)| {
                if needle.is_empty() {
                    return true;
                }
                let label = iso.label.to_ascii_lowercase();
                let path = iso.iso_path.to_string_lossy().to_ascii_lowercase();
                label.contains(&needle) || path.contains(&needle)
            })
            .map(|(i, _)| i)
            .collect();
        match self.sort_order {
            SortOrder::Name => {
                indices.sort_by(|&a, &b| self.isos[a].label.cmp(&self.isos[b].label));
            }
            SortOrder::SizeDesc => {
                indices.sort_by(|&a, &b| {
                    self.isos[b]
                        .size_bytes
                        .unwrap_or(0)
                        .cmp(&self.isos[a].size_bytes.unwrap_or(0))
                });
            }
            SortOrder::Distro => {
                indices.sort_by(|&a, &b| {
                    let da = format!("{:?}", self.isos[a].distribution);
                    let db = format!("{:?}", self.isos[b].distribution);
                    da.cmp(&db)
                        .then_with(|| self.isos[a].label.cmp(&self.isos[b].label))
                });
            }
        }
        indices
    }

    /// Translate a List-cursor position to a real index into `isos`,
    /// or None if the view is empty / cursor out of range.
    #[must_use]
    pub fn real_index(&self, cursor: usize) -> Option<usize> {
        self.visible_indices().get(cursor).copied()
    }

    /// Cycle to the next sort order. (#85 Tier 2)
    pub fn cycle_sort(&mut self) {
        self.sort_order = self.sort_order.next();
        // Reset cursor so the view doesn't point past the (possibly
        // reordered) list end.
        if let Screen::List { selected } = &mut self.screen {
            *selected = 0;
        }
    }

    /// Open the filter editor. (#85 Tier 2)
    pub fn open_filter(&mut self) {
        if matches!(self.screen, Screen::List { .. }) {
            self.filter_editing = true;
        }
    }

    /// Append a character to the filter while editing.
    pub fn filter_push(&mut self, c: char) {
        if self.filter_editing {
            self.filter.push(c);
            if let Screen::List { selected } = &mut self.screen {
                *selected = 0;
            }
        }
    }

    /// Delete the last character from the filter while editing.
    pub fn filter_backspace(&mut self) {
        if self.filter_editing {
            self.filter.pop();
            if let Screen::List { selected } = &mut self.screen {
                *selected = 0;
            }
        }
    }

    /// Commit the filter (Enter): keep the query, exit editing mode.
    pub fn filter_commit(&mut self) {
        self.filter_editing = false;
    }

    /// Cancel the filter (Esc): clear the query and exit editing mode.
    pub fn filter_cancel(&mut self) {
        self.filter_editing = false;
        self.filter.clear();
        if let Screen::List { selected } = &mut self.screen {
            *selected = 0;
        }
    }

    /// Effective cmdline for the ISO at `idx` — the user's override if one
    /// exists, otherwise the ISO-declared default (or empty string).
    #[must_use]
    pub fn effective_cmdline(&self, idx: usize) -> String {
        self.cmdline_overrides
            .get(&idx)
            .cloned()
            .or_else(|| self.isos.get(idx)?.cmdline.clone())
            .unwrap_or_default()
    }

    /// Whether the ISO at `idx` carries a quirk that blocks kexec entirely
    /// (e.g. Windows installers — wrong boot protocol), OR the ISO failed
    /// integrity verification (hash mismatch / forged signature). The TUI
    /// uses this to disable the Enter binding on the Confirm screen.
    ///
    /// Hash and signature failures are enforced as hard blocks — the red
    /// `✗ MISMATCH` / `✗ FORGED` indicator on the Confirm screen would
    /// otherwise be advisory-only, which lets a physical-access attacker
    /// boot a tampered ISO by clicking through the warning. (#55)
    #[must_use]
    pub fn is_kexec_blocked(&self, idx: usize) -> bool {
        let Some(iso) = self.isos.get(idx) else {
            return false;
        };
        if iso.quirks.contains(&Quirk::NotKexecBootable) {
            return true;
        }
        if matches!(iso.hash_verification, HashVerification::Mismatch { .. }) {
            return true;
        }
        if matches!(
            iso.signature_verification,
            SignatureVerification::Forged { .. }
        ) {
            return true;
        }
        false
    }

    /// Transition confirm → edit-cmdline, seeding the buffer with whatever
    /// is currently effective for the selected ISO.
    pub fn enter_cmdline_editor(&mut self) {
        let Screen::Confirm { selected } = self.screen else {
            return;
        };
        let buffer = self.effective_cmdline(selected);
        let cursor = buffer.len();
        self.screen = Screen::EditCmdline {
            selected,
            buffer,
            cursor,
        };
    }

    /// Commit the editor's buffer as the override and return to Confirm.
    pub fn commit_cmdline_edit(&mut self) {
        let Screen::EditCmdline {
            selected, buffer, ..
        } = std::mem::replace(&mut self.screen, Screen::Quitting)
        else {
            return;
        };
        self.cmdline_overrides.insert(selected, buffer);
        self.screen = Screen::Confirm { selected };
    }

    /// Cancel the editor and return to Confirm without saving.
    pub fn cancel_cmdline_edit(&mut self) {
        if let Screen::EditCmdline { selected, .. } = self.screen {
            self.screen = Screen::Confirm { selected };
        }
    }

    /// Insert a single character at the cursor.
    pub fn cmdline_insert(&mut self, ch: char) {
        let Screen::EditCmdline { buffer, cursor, .. } = &mut self.screen else {
            return;
        };
        buffer.insert(*cursor, ch);
        *cursor += ch.len_utf8();
    }

    /// Delete one char before the cursor (backspace).
    pub fn cmdline_backspace(&mut self) {
        let Screen::EditCmdline { buffer, cursor, .. } = &mut self.screen else {
            return;
        };
        if *cursor == 0 {
            return;
        }
        let new_cursor = prev_char_boundary(buffer, *cursor);
        buffer.replace_range(new_cursor..*cursor, "");
        *cursor = new_cursor;
    }

    /// Move cursor left one char (saturating).
    pub fn cmdline_cursor_left(&mut self) {
        let Screen::EditCmdline { buffer, cursor, .. } = &mut self.screen else {
            return;
        };
        if *cursor == 0 {
            return;
        }
        *cursor = prev_char_boundary(buffer, *cursor);
    }

    /// Move cursor right one char (saturating).
    pub fn cmdline_cursor_right(&mut self) {
        let Screen::EditCmdline { buffer, cursor, .. } = &mut self.screen else {
            return;
        };
        if *cursor >= buffer.len() {
            return;
        }
        let new_cursor = (*cursor + 1..=buffer.len())
            .find(|candidate| buffer.is_char_boundary(*candidate))
            .unwrap_or(buffer.len());
        *cursor = new_cursor;
    }

    /// Jump to the first visible row (vim `g`). (#85). The visible
    /// view always has at least one row (the rescue-shell entry
    /// since #90), so the `has_view` check is trivially true — kept
    /// for defence in depth in case that invariant ever changes.
    pub fn move_to_first(&mut self) {
        let has_view = !self.visible_entries().is_empty();
        if let Screen::List { selected } = &mut self.screen {
            if has_view {
                *selected = 0;
            }
        }
    }

    /// Jump to the last visible row (vim `G`). With #90 the last row
    /// is always the rescue-shell entry.
    pub fn move_to_last(&mut self) {
        let view_len = self.visible_entries().len();
        if let Screen::List { selected } = &mut self.screen {
            if view_len > 0 {
                *selected = view_len - 1;
            }
        }
    }

    /// Advance the visible cursor up (negative) or down (positive),
    /// saturating at the visible-list bounds. Cursor indexes the full
    /// entries list (ISOs + rescue shell), not just ISOs (#85, #90).
    ///
    /// Also resets `info_scroll` to 0 whenever the cursor actually
    /// moves — per-ISO scrollback in the info pane would be
    /// confusing (#458).
    pub fn move_selection(&mut self, delta: i32) {
        let view_len = self.visible_entries().len();
        let Screen::List { selected } = &mut self.screen else {
            return;
        };
        if view_len == 0 {
            return;
        }
        let max = view_len - 1;
        let before = *selected;
        if delta < 0 {
            let step = delta.unsigned_abs() as usize;
            *selected = selected.saturating_sub(step);
        } else {
            let step = usize::try_from(delta).unwrap_or(0);
            *selected = selected.saturating_add(step).min(max);
        }
        if *selected != before {
            self.info_scroll = 0;
        }
    }

    /// Swap the keyboard-focus pane (List ↔ Info). No-op outside the
    /// List screen. (#458)
    pub fn toggle_pane(&mut self) {
        if matches!(self.screen, Screen::List { .. }) {
            self.pane = self.pane.toggle();
        }
    }

    /// Scroll the info pane up (negative) or down (positive),
    /// saturating at zero. Upper bound is enforced at render time
    /// (the pane knows its own height + content length). (#458)
    pub fn move_info_scroll(&mut self, delta: i32) {
        if delta < 0 {
            let step = u16::try_from(delta.unsigned_abs()).unwrap_or(u16::MAX);
            self.info_scroll = self.info_scroll.saturating_sub(step);
        } else {
            let step = u16::try_from(delta).unwrap_or(u16::MAX);
            self.info_scroll = self.info_scroll.saturating_add(step);
        }
    }

    /// Transition list → confirm for the highlighted ISO. The cursor
    /// on List indexes the full entries view (ISOs + rescue shell);
    /// ISO selections route to Confirm, shell selections are handled
    /// by [`Self::is_shell_selected`] (main.rs exits with
    /// [`RESCUE_SHELL_EXIT_CODE`] in that case).
    pub fn confirm_selection(&mut self) {
        if let Screen::List { selected } = self.screen {
            if let Some(ViewEntry::Iso(real)) = self.view_entry(selected) {
                self.screen = Screen::Confirm { selected: real };
            }
        }
    }

    /// Whether the List cursor currently highlights the synthetic
    /// rescue-shell entry. main.rs uses this to branch on Enter. (#90)
    #[must_use]
    pub fn is_shell_selected(&self) -> bool {
        if let Screen::List { selected } = self.screen {
            matches!(self.view_entry(selected), Some(ViewEntry::RescueShell))
        } else {
            false
        }
    }

    /// Transition confirm → list (cancel). Maps the real ISO index
    /// back to its cursor position in the current entries view (ISO
    /// entries + rescue shell, #90). Falls back to 0 if the ISO is
    /// no longer visible (e.g. filter changed).
    pub fn cancel_confirmation(&mut self) {
        if let Screen::Confirm { selected } = self.screen {
            let cursor = self
                .visible_entries()
                .iter()
                .position(|e| matches!(e, ViewEntry::Iso(i) if *i == selected))
                .unwrap_or(0);
            self.screen = Screen::List { selected: cursor };
        }
    }

    /// Record a kexec failure and transition to the Error screen.
    pub fn record_kexec_error(&mut self, err: &KexecError) {
        // If the error occurred during a Confirm flow, enrich with
        // ISO-specific context (sibling key discovery for
        // SignatureRejected).
        let (iso, return_to) = match &self.screen {
            Screen::Confirm { selected } | Screen::EditCmdline { selected, .. } => {
                (self.isos.get(*selected), *selected)
            }
            _ => (None, 0),
        };
        let (message, remedy) = error_diagnostic_with_iso(err, iso);
        self.screen = Screen::Error {
            message,
            remedy,
            return_to,
        };
    }

    /// Open the help overlay over the current screen. (#85)
    pub fn open_help(&mut self) {
        // Don't stack — if already in Help, do nothing.
        if matches!(self.screen, Screen::Help { .. }) {
            return;
        }
        let prior = std::mem::replace(&mut self.screen, Screen::Quitting);
        self.screen = Screen::Help {
            prior: Box::new(prior),
        };
    }

    /// Dismiss the help overlay and restore the prior screen.
    pub fn close_help(&mut self) {
        if let Screen::Help { prior } = std::mem::replace(&mut self.screen, Screen::Quitting) {
            self.screen = *prior;
        }
    }

    /// Open the quit-confirmation prompt over the current screen.
    /// Idempotent — re-pressing `q` in the prompt does nothing. (#85)
    pub fn request_quit(&mut self) {
        if matches!(self.screen, Screen::ConfirmQuit { .. } | Screen::Quitting) {
            return;
        }
        let prior = std::mem::replace(&mut self.screen, Screen::Quitting);
        self.screen = Screen::ConfirmQuit {
            prior: Box::new(prior),
        };
    }

    /// Dismiss the quit prompt without exiting.
    pub fn cancel_quit(&mut self) {
        if let Screen::ConfirmQuit { prior } = std::mem::replace(&mut self.screen, Screen::Quitting)
        {
            self.screen = *prior;
        }
    }

    /// Coarse trust classification for the Confirm screen's verdict
    /// line AND for deciding whether [`Self::enter_trust_challenge`]
    /// fires before a kexec. Mirrors the `TrustVerdict` enum in
    /// render.rs but kept here so state-machine tests don't need to
    /// depend on the UI layer. (#93)
    #[must_use]
    pub fn is_degraded_trust(&self, idx: usize) -> bool {
        let Some(iso) = self.isos.get(idx) else {
            return false;
        };
        // GREEN if hash OR signature is verified.
        let verified = matches!(
            iso.signature_verification,
            SignatureVerification::Verified { .. }
        ) || matches!(iso.hash_verification, HashVerification::Verified { .. });
        !verified
    }

    /// Enter the typed-confirmation challenge screen for a degraded
    /// trust state. (#93)
    pub fn enter_trust_challenge(&mut self, idx: usize) {
        self.screen = Screen::TrustChallenge {
            selected: idx,
            buffer: String::new(),
        };
    }

    /// Character entry for the trust challenge. Returns true iff the
    /// buffer now equals the expected token "boot" — caller then
    /// proceeds to kexec.
    pub fn trust_challenge_push(&mut self, c: char) -> bool {
        if let Screen::TrustChallenge { buffer, .. } = &mut self.screen {
            buffer.push(c);
            return buffer == "boot";
        }
        false
    }

    /// Backspace in the trust challenge buffer.
    pub fn trust_challenge_backspace(&mut self) {
        if let Screen::TrustChallenge { buffer, .. } = &mut self.screen {
            buffer.pop();
        }
    }

    /// Cancel the trust challenge and return to Confirm.
    pub fn trust_challenge_cancel(&mut self) {
        if let Screen::TrustChallenge { selected, .. } = self.screen {
            self.screen = Screen::Confirm { selected };
        }
    }

    /// Open the Verifying screen for the ISO at real index `idx`.
    /// Caller (main.rs) spawns the worker thread that will feed
    /// progress ticks via [`Self::verify_tick`] and completion via
    /// [`Self::verify_finish`]. (#89)
    pub fn begin_verify(&mut self, idx: usize) {
        if self.isos.get(idx).is_some() {
            self.screen = Screen::Verifying {
                selected: idx,
                bytes: 0,
                total: 0,
                result: None,
            };
        }
    }

    /// Progress update from the verify worker.
    pub fn verify_tick(&mut self, new_bytes: u64, new_total: u64) {
        if let Screen::Verifying { bytes, total, .. } = &mut self.screen {
            *bytes = new_bytes;
            *total = new_total;
        }
    }

    /// Worker finished. Update the ISO's `hash_verification` in place
    /// and transition back to the Confirm screen on the same ISO so
    /// the operator sees the refreshed verdict. (#89)
    pub fn verify_finish(&mut self, outcome: HashVerification) {
        if let Screen::Verifying { selected, .. } = self.screen {
            if let Some(iso) = self.isos.get_mut(selected) {
                iso.hash_verification = outcome;
            }
            self.screen = Screen::Confirm { selected };
        }
    }

    /// Produce a plain-text evidence snapshot of the current Error
    /// screen suitable for writing to the `AEGIS_ISOS` data partition.
    /// memtest86+-style "one frame = one bug report," serialized.
    /// (#92)
    #[must_use]
    pub fn error_evidence_text(&self) -> Option<String> {
        use std::fmt::Write as _;
        let (msg, remedy, return_to) = match &self.screen {
            Screen::Error {
                message,
                remedy,
                return_to,
            } => (message.clone(), remedy.clone(), *return_to),
            _ => return None,
        };
        let iso = self.isos.get(return_to)?;
        let cmdline = self.effective_cmdline(return_to);
        let mut body = String::new();
        body.push_str("aegis-boot kexec-failure evidence\n");
        body.push_str("=================================\n\n");
        let _ = writeln!(body, "Diagnostic: {msg}");
        if let Some(r) = remedy {
            let _ = writeln!(body, "Remedy:     {r}");
        }
        body.push('\n');
        let _ = writeln!(
            body,
            "Version:    aegis-boot v{}",
            env!("CARGO_PKG_VERSION")
        );
        let _ = writeln!(
            body,
            "SB / TPM:   {}  ·  {}",
            self.secure_boot.summary(),
            self.tpm.summary()
        );
        let _ = writeln!(body, "ISO label:  {}", iso.label);
        let _ = writeln!(body, "ISO path:   {}", iso.iso_path.display());
        let _ = writeln!(body, "Size:       {:?}", iso.size_bytes);
        let _ = writeln!(body, "Distribution: {:?}", iso.distribution);
        let _ = writeln!(body, "Quirks:     {:?}", iso.quirks);
        let _ = writeln!(body, "Hash state: {:?}", iso.hash_verification);
        let _ = writeln!(body, "Sig state:  {:?}", iso.signature_verification);
        let cmdline_display = if cmdline.is_empty() {
            "(none)"
        } else {
            &cmdline
        };
        let _ = writeln!(body, "Cmdline:    {cmdline_display}");
        Some(body)
    }

    /// Cancel an in-progress verification (Esc). Dismisses the
    /// Verifying screen without updating the iso. The worker thread
    /// continues in the background; its result is discarded.
    pub fn cancel_verify(&mut self) {
        if let Screen::Verifying { selected, .. } = self.screen {
            self.screen = Screen::Confirm { selected };
        }
    }

    /// Confirm exit — only call from `ConfirmQuit` screen. Direct exit.
    pub fn confirm_quit(&mut self) {
        self.screen = Screen::Quitting;
    }

    /// Legacy direct quit — kept for tests that exercise the terminal
    /// state but no longer wired to a key. New code should call
    /// [`Self::request_quit`] instead so the operator gets a confirmation.
    #[cfg(test)]
    pub fn quit(&mut self) {
        self.screen = Screen::Quitting;
    }

    /// Currently highlighted/selected ISO, if the screen has one.
    #[must_use]
    #[cfg(test)]
    pub fn selected_iso(&self) -> Option<&DiscoveredIso> {
        let selected = match &self.screen {
            Screen::List { selected }
            | Screen::Confirm { selected }
            | Screen::EditCmdline { selected, .. } => *selected,
            _ => return None,
        };
        self.isos.get(selected)
    }
}

/// Find the byte offset of the nearest char boundary before `cursor`.
///
/// UTF-8 chars are 1-4 bytes; step back 1 byte at a time looking for the
/// previous boundary. Walks up to 4 bytes in the worst case.
fn prev_char_boundary(buffer: &str, cursor: usize) -> usize {
    (1..=cursor.min(4))
        .find(|step| buffer.is_char_boundary(cursor - step))
        .map_or(0, |step| cursor - step)
}

/// Build the MOK enrollment remedy text as a three-step walkthrough.
/// Split from a single dense paragraph into numbered steps + a
/// firmware-boot-key hint table so operators can execute without
/// context-switching to external docs. (#136)
///
/// When the ISO has a discoverable sibling key file, step 1 becomes a
/// literal copy-paste `mokutil --import` command. When it doesn't, step 1
/// tells the operator how to place the key so the next run will name it.
fn build_mokutil_remedy(iso: Option<&DiscoveredIso>) -> String {
    let key_hint = iso.and_then(|iso| find_sibling_key(&iso.iso_path));
    let step_one = match key_hint {
        Some(ref key_path) => format!(
            "STEP 1/3 — Enroll the key (run on the host, not here):\n  \
             sudo mokutil --import {}\n  \
             (mokutil will prompt you to set a temporary password — you'll \
             need this exact password in step 2.)",
            key_path.display()
        ),
        None => "STEP 1/3 — Get the signing key, then enroll it (run on the host, not here):\n  \
                 • Find the distro's signing public key (usually a `.pub`, `.key`, or\n    \
                 `.der` file on the distro's download page).\n  \
                 • Place it alongside the ISO using any of these filenames:\n    \
                 `<iso>.pub`, `<iso>.key`, `<iso>.der` — aegis-boot will then\n    \
                 generate the exact `sudo mokutil --import <path>` command\n    \
                 for you on the next kexec attempt.\n  \
                 • After running `mokutil --import`, mokutil will prompt you to\n    \
                 set a temporary password. You'll need it in step 2."
            .to_string(),
    };
    format!(
        "{step_one}\n\
         \n\
         STEP 2/3 — Reboot and complete enrollment in MOK Manager:\n  \
         • Reboot the machine.\n  \
         • The firmware will show a blue-on-black screen titled\n    \
         \"Perform MOK management\" before the normal boot.\n  \
         • Choose \"Enroll MOK\" → \"Continue\" → \"Yes\" → enter the\n    \
         temporary password from step 1 → \"Reboot\".\n  \
         • If the machine skips straight past the blue screen, power-cycle\n    \
         within 10 seconds of the firmware splash; the MOK window is short.\n\
         \n\
         STEP 3/3 — Boot back into aegis-boot and retry:\n  \
         • Tap your firmware's boot-menu key at power-on:\n    \
         Lenovo/Dell: F12    HP: F9    ASUS: F8    MSI: F11    Apple: Option\n  \
         • Re-select the aegis-boot USB → re-select this ISO → Enter.\n  \
         • The kernel's signature now verifies against the MOK you enrolled.\n\
         \n\
         Do NOT disable Secure Boot. Do NOT enroll a global MOK (that's\n\
         Ventoy's approach and defeats the chain of trust). See\n\
         docs/UNSIGNED_KERNEL.md for the full guide (#126)."
    )
}

/// Look for a sibling key file alongside the ISO. Accepts `.pub`, `.key`,
/// and `.der` extensions — the most common formats for detached signing
/// keys published by distros.
fn find_sibling_key(iso_path: &std::path::Path) -> Option<std::path::PathBuf> {
    let base = iso_path.file_stem()?;
    let dir = iso_path.parent()?;
    for ext in ["pub", "key", "der"] {
        let candidate = dir.join(format!("{}.{ext}", base.to_string_lossy()));
        if candidate.exists() {
            return Some(candidate);
        }
    }
    // Also try full-filename extensions: ubuntu.iso.pub alongside ubuntu.iso
    let fname = iso_path.file_name()?;
    for ext in ["pub", "key", "der"] {
        let candidate = dir.join(format!("{}.{ext}", fname.to_string_lossy()));
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

/// Map a [`KexecError`] to (user-facing message, optional remedy hint).
///
/// Diagnostics are deliberately specific so the TUI never shows a generic
/// "kexec failed" — see ADR 0001's commitment to "no black screens."
///
/// For context-free error mapping. The TUI-level variant
/// [`error_diagnostic_with_iso`] augments the [`KexecError::SignatureRejected`]
/// remedy with a specific `mokutil --import` command when the ISO has a
/// sibling key file.
#[must_use]
#[cfg_attr(not(test), allow(dead_code))]
pub fn error_diagnostic(err: &KexecError) -> (String, Option<String>) {
    error_diagnostic_with_iso(err, None)
}

/// Same as [`error_diagnostic`] but enriches the remedy with ISO-specific
/// context when available. For `SignatureRejected`, if the caller passes
/// in the ISO that triggered the error, we look for sibling key files
/// (`<iso>.pub`, `<iso>.key`, `<iso>.der`) and embed the exact
/// `mokutil --import` command — removing the "which file do I enroll?"
/// guessing game from the recovery flow.
#[must_use]
pub fn error_diagnostic_with_iso(
    err: &KexecError,
    iso: Option<&DiscoveredIso>,
) -> (String, Option<String>) {
    match err {
        KexecError::SignatureRejected => (
            "Kernel signature verification failed (KEXEC_SIG)".to_string(),
            Some(build_mokutil_remedy(iso)),
        ),
        KexecError::LockdownRefused => (
            "Operation refused by kernel lockdown".to_string(),
            Some(
                "This is by design under enforcing Secure Boot. The selected kernel cannot be \
                 booted on this system without an enrolled signing key."
                    .to_string(),
            ),
        ),
        KexecError::UnsupportedImage => (
            "Kernel image format not recognized".to_string(),
            Some("The ISO's kernel is not a valid bzImage; this ISO is not supported.".to_string()),
        ),
        KexecError::Io(io) => (
            format!("System error: {io}"),
            io.raw_os_error()
                .map(|errno| format!("Underlying errno = {errno}.")),
        ),
        KexecError::InvalidPath(p) => (
            format!("Invalid path supplied to kexec: {}", p.display()),
            None,
        ),
        KexecError::Unsupported => (
            "kexec is only available on Linux".to_string(),
            Some(
                "rescue-tui is meant to run inside the signed Linux rescue initramfs.".to_string(),
            ),
        ),
    }
}

/// Render a one-line summary of an ISO's quirks for the list view.
/// Returns an empty string if there are none.
#[must_use]
pub fn quirks_summary(iso: &DiscoveredIso) -> String {
    if iso.quirks.is_empty() {
        return String::new();
    }
    let parts: Vec<&'static str> = iso
        .quirks
        .iter()
        .map(|q| match q {
            Quirk::UnsignedKernel => "unsigned-kernel",
            Quirk::BiosOnly => "bios-only",
            Quirk::RequiresWholeDeviceWrite => "needs-dd",
            Quirk::CrossDistroKexecRefused => "cross-distro-refused",
            Quirk::NotKexecBootable => "not-kexec-bootable",
        })
        .collect();
    format!("[{}]", parts.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;
    use iso_probe::Distribution;
    use std::path::PathBuf;

    fn fake_iso(label: &str) -> DiscoveredIso {
        DiscoveredIso {
            iso_path: PathBuf::from(format!("/run/media/{label}.iso")),
            label: label.to_string(),
            distribution: Distribution::Debian,
            kernel: PathBuf::from("casper/vmlinuz"),
            initrd: Some(PathBuf::from("casper/initrd")),
            cmdline: Some("boot=casper".to_string()),
            quirks: vec![],
            hash_verification: iso_probe::HashVerification::NotPresent,
            signature_verification: iso_probe::SignatureVerification::NotPresent,
            size_bytes: Some(1_500_000_000),
            contains_installer: false,
            pretty_name: None,
            sidecar: None,
        }
    }

    #[test]
    fn empty_list_blocks_confirmation_and_movement() {
        let mut s = AppState::new(vec![]);
        s.move_selection(1);
        assert_eq!(s.screen, Screen::List { selected: 0 });
        s.confirm_selection();
        assert_eq!(s.screen, Screen::List { selected: 0 });
        assert!(s.selected_iso().is_none());
    }

    #[test]
    fn movement_clamps_at_bounds() {
        // 3 ISOs + 1 rescue-shell row = 4 entries; max cursor = 3.
        let mut s = AppState::new(vec![fake_iso("a"), fake_iso("b"), fake_iso("c")]);
        s.move_selection(-5);
        assert_eq!(s.screen, Screen::List { selected: 0 });
        s.move_selection(99);
        assert_eq!(s.screen, Screen::List { selected: 3 });
    }

    #[test]
    fn confirm_then_cancel_returns_to_list_with_same_selection() {
        let mut s = AppState::new(vec![fake_iso("a"), fake_iso("b")]);
        s.move_selection(1);
        s.confirm_selection();
        assert_eq!(s.screen, Screen::Confirm { selected: 1 });
        s.cancel_confirmation();
        assert_eq!(s.screen, Screen::List { selected: 1 });
    }

    fn unwrap_remedy(remedy: Option<String>) -> String {
        remedy.unwrap_or_else(|| panic!("expected remedy to be Some"))
    }

    #[test]
    fn signature_rejected_diagnostic_includes_mokutil_hint() {
        let (msg, remedy) = error_diagnostic(&KexecError::SignatureRejected);
        assert!(msg.to_lowercase().contains("signature"));
        let r = unwrap_remedy(remedy);
        assert!(r.contains("mokutil"));
        // ADR 0001: must not *suggest* disabling SB. Allow "Do NOT disable" phrasing.
        let lc = r.to_lowercase();
        assert!(!lc.contains("disable secure boot") || lc.contains("not disable"));
    }

    #[test]
    fn signature_rejected_without_iso_gives_generic_guidance() {
        // No ISO context — remedy should tell user where to put the key
        // so future attempts can auto-suggest.
        let (_, remedy) = error_diagnostic_with_iso(&KexecError::SignatureRejected, None);
        let r = unwrap_remedy(remedy);
        assert!(r.contains("<iso>.pub") || r.contains(".pub"));
        // Remedy says "Do NOT disable Secure Boot" — literal phrase is
        // fine; the ADR 0001 commitment is no *suggestion* to disable.
        let lc = r.to_lowercase();
        assert!(!lc.contains("disable secure boot") || lc.contains("not disable"));
    }

    #[test]
    fn signature_rejected_remedy_is_three_step_walkthrough() {
        // Remedy must be structured as STEP 1/3, STEP 2/3, STEP 3/3 so
        // operators can execute it without reading a dense paragraph.
        // Also must mention MOK Manager's blue-screen cue and list
        // firmware boot-menu keys for the top vendors. (#136)
        let (_, remedy) = error_diagnostic_with_iso(&KexecError::SignatureRejected, None);
        let r = unwrap_remedy(remedy);
        assert!(r.contains("STEP 1/3"), "missing STEP 1/3 header in: {r}");
        assert!(r.contains("STEP 2/3"), "missing STEP 2/3 header in: {r}");
        assert!(r.contains("STEP 3/3"), "missing STEP 3/3 header in: {r}");
        assert!(
            r.contains("MOK management") || r.contains("MOK Manager"),
            "remedy must reference MOK Manager by name so operators know what to expect on the reboot"
        );
        let lower = r.to_ascii_lowercase();
        assert!(
            lower.contains("f12") || lower.contains("f9") || lower.contains("f11"),
            "remedy must mention firmware boot-menu keys to close the 'how do I get back?' loop"
        );
    }

    #[test]
    fn signature_rejected_with_sibling_key_embeds_exact_command() {
        use std::fs;
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let iso_path = dir.path().join("test.iso");
        fs::write(&iso_path, b"dummy").unwrap_or_else(|e| panic!("write: {e}"));
        let key_path = dir.path().join("test.pub");
        fs::write(&key_path, b"pubkey").unwrap_or_else(|e| panic!("write key: {e}"));

        let iso = DiscoveredIso {
            iso_path: iso_path.clone(),
            label: "t".into(),
            distribution: iso_probe::Distribution::Debian,
            kernel: std::path::PathBuf::from("vmlinuz"),
            initrd: None,
            cmdline: None,
            quirks: vec![],
            hash_verification: iso_probe::HashVerification::NotPresent,
            signature_verification: iso_probe::SignatureVerification::NotPresent,
            size_bytes: Some(1_500_000_000),
            contains_installer: false,
            pretty_name: None,
            sidecar: None,
        };
        let (_, remedy) = error_diagnostic_with_iso(&KexecError::SignatureRejected, Some(&iso));
        let r = unwrap_remedy(remedy);
        assert!(r.contains("mokutil --import"));
        assert!(r.contains(&key_path.display().to_string()));
    }

    #[test]
    fn find_sibling_key_tries_stem_and_full_filename() {
        use std::fs;
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        // Case 1: .pub with stem (debian.pub alongside debian.iso)
        let iso_a = dir.path().join("debian.iso");
        fs::write(&iso_a, b"x").unwrap_or_else(|e| panic!("write: {e}"));
        let key_a = dir.path().join("debian.pub");
        fs::write(&key_a, b"k").unwrap_or_else(|e| panic!("write: {e}"));
        assert_eq!(find_sibling_key(&iso_a), Some(key_a));

        // Case 2: .iso.pub full-name style (ubuntu.iso.pub alongside ubuntu.iso)
        let iso_b = dir.path().join("ubuntu.iso");
        fs::write(&iso_b, b"y").unwrap_or_else(|e| panic!("write: {e}"));
        let key_b = dir.path().join("ubuntu.iso.pub");
        fs::write(&key_b, b"k").unwrap_or_else(|e| panic!("write: {e}"));
        assert_eq!(find_sibling_key(&iso_b), Some(key_b));

        // Case 3: no key
        let iso_c = dir.path().join("orphan.iso");
        fs::write(&iso_c, b"z").unwrap_or_else(|e| panic!("write: {e}"));
        assert!(find_sibling_key(&iso_c).is_none());
    }

    #[test]
    fn lockdown_diagnostic_does_not_suggest_disabling_sb() {
        let (msg, remedy) = error_diagnostic(&KexecError::LockdownRefused);
        assert!(msg.to_lowercase().contains("lockdown"));
        let r = unwrap_remedy(remedy);
        let lc = r.to_lowercase();
        assert!(!lc.contains("disable secure boot"));
    }

    #[test]
    fn io_diagnostic_preserves_errno() {
        let io = std::io::Error::from_raw_os_error(13);
        let err = KexecError::Io(io);
        let (msg, remedy) = error_diagnostic(&err);
        assert!(msg.contains("System error"));
        assert!(unwrap_remedy(remedy).contains("13"));
    }

    #[test]
    fn record_kexec_error_transitions_to_error_screen() {
        let mut s = AppState::new(vec![fake_iso("x")]);
        s.confirm_selection();
        s.record_kexec_error(&KexecError::SignatureRejected);
        assert!(matches!(s.screen, Screen::Error { .. }));
    }

    #[test]
    fn quit_transitions_to_quitting() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.quit();
        assert_eq!(s.screen, Screen::Quitting);
    }

    // --- Tier 1 UX (#85) -----------------------------------------------

    #[test]
    fn request_quit_opens_confirm_overlay() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.request_quit();
        assert!(matches!(s.screen, Screen::ConfirmQuit { .. }));
    }

    #[test]
    fn cancel_quit_restores_prior_screen() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.confirm_selection();
        s.request_quit();
        s.cancel_quit();
        assert!(matches!(s.screen, Screen::Confirm { selected: 0 }));
    }

    #[test]
    fn confirm_quit_exits() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.request_quit();
        s.confirm_quit();
        assert_eq!(s.screen, Screen::Quitting);
    }

    #[test]
    fn open_help_overlays_prior_screen() {
        let mut s = AppState::new(vec![fake_iso("a"), fake_iso("b")]);
        s.move_selection(1);
        s.open_help();
        assert!(matches!(s.screen, Screen::Help { .. }));
        s.close_help();
        assert!(matches!(s.screen, Screen::List { selected: 1 }));
    }

    #[test]
    fn move_to_first_jumps_to_zero() {
        let mut s = AppState::new(vec![fake_iso("a"), fake_iso("b"), fake_iso("c")]);
        s.move_selection(2);
        assert_eq!(s.screen, Screen::List { selected: 2 });
        s.move_to_first();
        assert_eq!(s.screen, Screen::List { selected: 0 });
    }

    #[test]
    fn move_to_last_jumps_to_max() {
        // 3 ISOs + 1 rescue-shell row = last cursor is 3.
        let mut s = AppState::new(vec![fake_iso("a"), fake_iso("b"), fake_iso("c")]);
        s.move_to_last();
        assert_eq!(s.screen, Screen::List { selected: 3 });
    }

    #[test]
    fn record_kexec_error_preserves_failed_selection() {
        let mut s = AppState::new(vec![fake_iso("a"), fake_iso("b"), fake_iso("c")]);
        s.move_selection(1);
        s.confirm_selection();
        s.record_kexec_error(&KexecError::SignatureRejected);
        match s.screen {
            Screen::Error { return_to, .. } => assert_eq!(return_to, 1),
            other => panic!("expected Error, got {other:?}"),
        }
    }

    // --- Tier 2 UX (#85) -----------------------------------------------

    #[test]
    fn visible_indices_no_filter_returns_all_sorted_by_name() {
        let s = AppState::new(vec![
            fake_iso("zebra"),
            fake_iso("alpha"),
            fake_iso("mango"),
        ]);
        let v = s.visible_indices();
        assert_eq!(v.len(), 3);
        // SortOrder::Name is default — labels must come back A..Z.
        let labels: Vec<&str> = v.iter().map(|&i| s.isos[i].label.as_str()).collect();
        assert_eq!(labels, ["alpha", "mango", "zebra"]);
    }

    #[test]
    fn filter_substring_case_insensitive() {
        let mut s = AppState::new(vec![
            fake_iso("Ubuntu-24.04"),
            fake_iso("fedora-40"),
            fake_iso("debian-12"),
        ]);
        s.filter = "DEB".to_string();
        let labels: Vec<&str> = s
            .visible_indices()
            .iter()
            .map(|&i| s.isos[i].label.as_str())
            .collect();
        assert_eq!(labels, ["debian-12"]);
    }

    #[test]
    fn cycle_sort_visits_all_orders() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        assert_eq!(s.sort_order, SortOrder::Name);
        s.cycle_sort();
        assert_eq!(s.sort_order, SortOrder::SizeDesc);
        s.cycle_sort();
        assert_eq!(s.sort_order, SortOrder::Distro);
        s.cycle_sort();
        assert_eq!(s.sort_order, SortOrder::Name);
    }

    #[test]
    fn filter_editing_typing_pins_cursor_to_zero() {
        let mut s = AppState::new(vec![fake_iso("aaa"), fake_iso("bbb"), fake_iso("ccc")]);
        s.move_selection(2);
        s.open_filter();
        s.filter_push('b');
        // After typing, cursor should be 0 (first match: bbb).
        assert_eq!(s.screen, Screen::List { selected: 0 });
    }

    #[test]
    fn filter_cancel_clears_query_and_resets_cursor() {
        let mut s = AppState::new(vec![fake_iso("aaa"), fake_iso("bbb")]);
        s.open_filter();
        s.filter_push('z'); // matches nothing
        s.filter_cancel();
        assert!(s.filter.is_empty());
        assert!(!s.filter_editing);
        assert_eq!(s.visible_indices().len(), 2);
    }

    #[test]
    fn confirm_selection_translates_visible_cursor_to_real_index() {
        // Filter so only the second iso is visible. Cursor=0 in the
        // visible view should map to real index 1.
        let mut s = AppState::new(vec![fake_iso("ubuntu"), fake_iso("debian")]);
        s.filter = "debian".to_string();
        s.confirm_selection();
        assert_eq!(s.screen, Screen::Confirm { selected: 1 });
    }

    // --- rescue-shell entry (#90) -------------------------------------

    #[test]
    fn rescue_shell_entry_always_last_even_with_no_isos() {
        let s = AppState::new(vec![]);
        let entries = s.visible_entries();
        assert_eq!(entries, vec![ViewEntry::RescueShell]);
    }

    #[test]
    fn rescue_shell_entry_appended_after_isos() {
        let s = AppState::new(vec![fake_iso("a"), fake_iso("b")]);
        let entries = s.visible_entries();
        assert_eq!(entries.len(), 3);
        assert!(matches!(entries[2], ViewEntry::RescueShell));
    }

    #[test]
    fn rescue_shell_entry_survives_filter_with_no_iso_matches() {
        let mut s = AppState::new(vec![fake_iso("ubuntu")]);
        s.filter = "nope".to_string();
        assert_eq!(s.visible_entries(), vec![ViewEntry::RescueShell]);
    }

    #[test]
    fn is_shell_selected_true_when_cursor_on_shell_row() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.move_to_last();
        assert!(s.is_shell_selected());
    }

    #[test]
    fn is_shell_selected_false_when_cursor_on_iso_row() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.move_to_first();
        assert!(!s.is_shell_selected());
    }

    // --- typed trust challenge (#93) ----------------------------------

    #[test]
    fn enter_trust_challenge_opens_screen_with_empty_buffer() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.enter_trust_challenge(0);
        assert!(matches!(
            s.screen,
            Screen::TrustChallenge {
                selected: 0,
                ref buffer,
            } if buffer.is_empty()
        ));
    }

    #[test]
    fn trust_challenge_push_returns_true_at_boot_string() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.enter_trust_challenge(0);
        assert!(!s.trust_challenge_push('b'));
        assert!(!s.trust_challenge_push('o'));
        assert!(!s.trust_challenge_push('o'));
        assert!(s.trust_challenge_push('t'));
    }

    #[test]
    fn trust_challenge_backspace_rolls_back() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.enter_trust_challenge(0);
        s.trust_challenge_push('b');
        s.trust_challenge_push('x');
        s.trust_challenge_backspace();
        // 'b' alone is not "boot" yet.
        assert!(!matches!(
            &s.screen,
            Screen::TrustChallenge { buffer, .. } if buffer == "boot"
        ));
    }

    #[test]
    fn trust_challenge_cancel_returns_to_confirm() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.enter_trust_challenge(0);
        s.trust_challenge_cancel();
        assert!(matches!(s.screen, Screen::Confirm { selected: 0 }));
    }

    #[test]
    fn is_degraded_trust_true_when_no_verification() {
        let s = AppState::new(vec![fake_iso("a")]);
        // fake_iso() has NotPresent for both hash and sig → degraded.
        assert!(s.is_degraded_trust(0));
    }

    #[test]
    fn is_degraded_trust_false_when_hash_verified() {
        let mut iso = fake_iso("a");
        iso.hash_verification = HashVerification::Verified {
            digest: "abc".to_string(),
            source: "/x".to_string(),
        };
        let s = AppState::new(vec![iso]);
        assert!(!s.is_degraded_trust(0));
    }

    // --- one-frame error evidence (#92) -------------------------------

    #[test]
    fn error_evidence_text_populated_after_kexec_error() {
        let mut s = AppState::new(vec![fake_iso("ubuntu-24.04")]);
        s.confirm_selection();
        s.record_kexec_error(&KexecError::SignatureRejected);
        let Some(text) = s.error_evidence_text() else {
            panic!("evidence populated on Error");
        };
        assert!(text.contains("aegis-boot kexec-failure evidence"));
        assert!(text.contains("ubuntu-24.04"));
        assert!(text.contains("Diagnostic:"));
    }

    #[test]
    fn error_evidence_text_none_when_not_error_screen() {
        let s = AppState::new(vec![fake_iso("a")]);
        assert!(s.error_evidence_text().is_none());
    }

    // --- verify-now (#89) ---------------------------------------------

    #[test]
    fn begin_verify_opens_verifying_screen() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.begin_verify(0);
        assert!(matches!(
            s.screen,
            Screen::Verifying {
                selected: 0,
                bytes: 0,
                ..
            }
        ));
    }

    #[test]
    fn verify_tick_updates_progress() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.begin_verify(0);
        s.verify_tick(1024, 4096);
        match s.screen {
            Screen::Verifying { bytes, total, .. } => {
                assert_eq!(bytes, 1024);
                assert_eq!(total, 4096);
            }
            _ => panic!("expected Verifying"),
        }
    }

    #[test]
    fn verify_finish_updates_iso_and_returns_to_confirm() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.begin_verify(0);
        let outcome = HashVerification::Verified {
            digest: "abc123".to_string(),
            source: "fake".to_string(),
        };
        s.verify_finish(outcome.clone());
        assert_eq!(s.isos[0].hash_verification, outcome);
        assert!(matches!(s.screen, Screen::Confirm { selected: 0 }));
    }

    #[test]
    fn cancel_verify_returns_to_confirm_without_updating_iso() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        let original = s.isos[0].hash_verification.clone();
        s.begin_verify(0);
        s.cancel_verify();
        assert!(matches!(s.screen, Screen::Confirm { selected: 0 }));
        assert_eq!(s.isos[0].hash_verification, original);
    }

    #[test]
    fn is_kexec_blocked_false_for_clean_iso() {
        let s = AppState::new(vec![fake_iso("clean")]);
        assert!(!s.is_kexec_blocked(0));
    }

    #[test]
    fn is_kexec_blocked_true_when_quirk_present() {
        let mut iso = fake_iso("windows");
        iso.quirks = vec![Quirk::NotKexecBootable];
        let s = AppState::new(vec![iso]);
        assert!(s.is_kexec_blocked(0));
    }

    #[test]
    fn is_kexec_blocked_true_when_hash_mismatch() {
        // A tampered ISO must not be kexec'd even if the operator clicks
        // through the red ✗ MISMATCH warning. Regression for #55.
        let mut iso = fake_iso("tampered");
        iso.hash_verification = HashVerification::Mismatch {
            expected: "abc".to_string(),
            actual: "def".to_string(),
            source: "/run/media/tampered.iso.sha256".to_string(),
        };
        let s = AppState::new(vec![iso]);
        assert!(s.is_kexec_blocked(0));
    }

    #[test]
    fn is_kexec_blocked_true_when_signature_forged() {
        // A signature that fails crypto verification under a trusted key
        // must not be kexec'd. Regression for #55.
        let mut iso = fake_iso("forged");
        iso.signature_verification = SignatureVerification::Forged {
            sig_path: std::path::PathBuf::from("/run/media/forged.iso.minisig"),
        };
        let s = AppState::new(vec![iso]);
        assert!(s.is_kexec_blocked(0));
    }

    #[test]
    fn is_kexec_blocked_false_for_unknown_index() {
        let s = AppState::new(vec![fake_iso("a")]);
        assert!(!s.is_kexec_blocked(99));
    }

    #[test]
    fn enter_cmdline_editor_seeds_with_iso_default() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.confirm_selection();
        s.enter_cmdline_editor();
        let Screen::EditCmdline {
            buffer,
            cursor,
            selected,
        } = &s.screen
        else {
            panic!("expected EditCmdline screen");
        };
        assert_eq!(*selected, 0);
        assert_eq!(buffer, "boot=casper"); // from fake_iso
        assert_eq!(*cursor, buffer.len());
    }

    #[test]
    fn cmdline_insert_at_cursor() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.confirm_selection();
        s.enter_cmdline_editor();
        s.cmdline_insert(' ');
        s.cmdline_insert('q');
        s.cmdline_insert('u');
        s.cmdline_insert('i');
        s.cmdline_insert('e');
        s.cmdline_insert('t');
        let Screen::EditCmdline { buffer, .. } = &s.screen else {
            panic!("expected EditCmdline")
        };
        assert_eq!(buffer, "boot=casper quiet");
    }

    #[test]
    fn cmdline_backspace_and_cursor_navigation() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.confirm_selection();
        s.enter_cmdline_editor();
        // Buffer is "boot=casper", cursor at end (11).
        s.cmdline_backspace(); // remove 'r'
        s.cmdline_backspace(); // remove 'e'
        let Screen::EditCmdline { buffer, cursor, .. } = &s.screen else {
            panic!()
        };
        assert_eq!(buffer, "boot=casp");
        assert_eq!(*cursor, 9);
        s.cmdline_cursor_left();
        s.cmdline_cursor_left();
        s.cmdline_insert('X');
        let Screen::EditCmdline { buffer, .. } = &s.screen else {
            panic!()
        };
        // cursor=9 after backspaces → left→left moves to 7 (before 's');
        // inserting 'X' there gives "boot=ca" + "X" + "sp".
        assert_eq!(buffer, "boot=caXsp");
    }

    #[test]
    fn commit_cmdline_stores_override_and_returns_to_confirm() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.confirm_selection();
        s.enter_cmdline_editor();
        s.cmdline_insert(' ');
        s.cmdline_insert('q');
        s.commit_cmdline_edit();
        assert_eq!(s.screen, Screen::Confirm { selected: 0 });
        assert_eq!(
            s.cmdline_overrides.get(&0),
            Some(&"boot=casper q".to_string())
        );
        assert_eq!(s.effective_cmdline(0), "boot=casper q");
    }

    #[test]
    fn cancel_cmdline_edit_preserves_original() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.confirm_selection();
        s.enter_cmdline_editor();
        s.cmdline_insert('X');
        s.cancel_cmdline_edit();
        assert_eq!(s.screen, Screen::Confirm { selected: 0 });
        assert!(s.cmdline_overrides.is_empty());
        assert_eq!(s.effective_cmdline(0), "boot=casper"); // unchanged default
    }

    #[test]
    fn backspace_on_empty_buffer_is_noop() {
        let mut s = AppState::new(vec![{
            let mut i = fake_iso("a");
            i.cmdline = None; // no default
            i
        }]);
        s.confirm_selection();
        s.enter_cmdline_editor();
        s.cmdline_backspace();
        s.cmdline_cursor_left();
        let Screen::EditCmdline { buffer, cursor, .. } = &s.screen else {
            panic!()
        };
        assert_eq!(buffer, "");
        assert_eq!(*cursor, 0);
    }

    #[test]
    fn effective_cmdline_prefers_override() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        assert_eq!(s.effective_cmdline(0), "boot=casper");
        s.cmdline_overrides.insert(0, "override=yes".to_string());
        assert_eq!(s.effective_cmdline(0), "override=yes");
    }

    #[test]
    fn quirks_summary_empty_for_clean_iso() {
        let iso = fake_iso("clean");
        assert_eq!(quirks_summary(&iso), "");
    }

    #[test]
    fn quirks_summary_lists_each_quirk() {
        let mut iso = fake_iso("warn");
        iso.quirks = vec![Quirk::UnsignedKernel, Quirk::BiosOnly];
        let s = quirks_summary(&iso);
        assert!(s.contains("unsigned-kernel"));
        assert!(s.contains("bios-only"));
    }

    // ---- #458 — dual-pane focus + info-scroll -----------------------

    #[test]
    fn default_pane_is_list() {
        let s = AppState::new(vec![fake_iso("a")]);
        assert_eq!(s.pane, Pane::List);
    }

    #[test]
    fn toggle_pane_swaps_focus_on_list_screen() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        assert_eq!(s.pane, Pane::List);
        s.toggle_pane();
        assert_eq!(s.pane, Pane::Info);
        s.toggle_pane();
        assert_eq!(s.pane, Pane::List);
    }

    #[test]
    fn toggle_pane_noop_outside_list_screen() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.screen = Screen::Confirm { selected: 0 };
        s.toggle_pane();
        assert_eq!(
            s.pane,
            Pane::List,
            "pane must not change when not on List screen"
        );
    }

    #[test]
    fn move_info_scroll_increments_and_saturates_at_zero() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        assert_eq!(s.info_scroll, 0);
        s.move_info_scroll(3);
        assert_eq!(s.info_scroll, 3);
        s.move_info_scroll(-5);
        assert_eq!(s.info_scroll, 0, "saturates at zero");
    }

    #[test]
    fn move_selection_resets_info_scroll() {
        // Per-ISO scroll state would confuse operators; resetting on
        // selection change matches gitui's pattern.
        let mut s = AppState::new(vec![fake_iso("a"), fake_iso("b")]);
        s.info_scroll = 7;
        s.move_selection(1);
        assert_eq!(s.info_scroll, 0);
    }

    #[test]
    fn move_selection_noop_does_not_reset_info_scroll() {
        // When the cursor hits a saturation boundary, info_scroll
        // should not be reset — it wasn't a real navigation.
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.info_scroll = 5;
        s.move_selection(-1); // already at 0, can't go further up
        assert_eq!(s.info_scroll, 5);
    }

    #[test]
    fn pane_toggle_method_returns_opposite() {
        assert_eq!(Pane::List.toggle(), Pane::Info);
        assert_eq!(Pane::Info.toggle(), Pane::List);
    }
}
