//! Pure state machine for the rescue TUI — no rendering, no I/O.
//!
//! Split from rendering so unit tests can cover every transition without a
//! TTY or a `TestBackend`.

use iso_probe::{DiscoveredIso, Quirk};
use kexec_loader::KexecError;

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
    /// Showing a fatal-or-classified error after attempted kexec.
    Error {
        /// User-facing diagnostic copied from [`error_diagnostic`].
        message: String,
        /// User-facing remedy hint, if any.
        remedy: Option<String>,
    },
    /// User asked to quit; main loop should exit cleanly.
    Quitting,
}

/// Top-level application state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppState {
    /// All discovered ISOs.
    pub isos: Vec<DiscoveredIso>,
    /// Current screen.
    pub screen: Screen,
    /// Per-ISO cmdline overrides keyed by index. When absent the
    /// ISO-declared default from `iso-probe` is used.
    pub cmdline_overrides: std::collections::HashMap<usize, String>,
}

impl AppState {
    /// Build a fresh state from a discovery result.
    #[must_use]
    pub fn new(isos: Vec<DiscoveredIso>) -> Self {
        Self {
            isos,
            screen: Screen::List { selected: 0 },
            cmdline_overrides: std::collections::HashMap::new(),
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
    /// (e.g. Windows installers — wrong boot protocol). The TUI uses this
    /// to disable the Enter binding on the Confirm screen.
    #[must_use]
    pub fn is_kexec_blocked(&self, idx: usize) -> bool {
        self.isos
            .get(idx)
            .is_some_and(|iso| iso.quirks.contains(&Quirk::NotKexecBootable))
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
        let Screen::EditCmdline {
            buffer, cursor, ..
        } = &mut self.screen
        else {
            return;
        };
        buffer.insert(*cursor, ch);
        *cursor += ch.len_utf8();
    }

    /// Delete one char before the cursor (backspace).
    pub fn cmdline_backspace(&mut self) {
        let Screen::EditCmdline {
            buffer, cursor, ..
        } = &mut self.screen
        else {
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
        let Screen::EditCmdline {
            buffer, cursor, ..
        } = &mut self.screen
        else {
            return;
        };
        if *cursor == 0 {
            return;
        }
        *cursor = prev_char_boundary(buffer, *cursor);
    }

    /// Move cursor right one char (saturating).
    pub fn cmdline_cursor_right(&mut self) {
        let Screen::EditCmdline {
            buffer, cursor, ..
        } = &mut self.screen
        else {
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

    /// Advance the highlighted row up (negative) or down (positive), saturating
    /// at list bounds.
    pub fn move_selection(&mut self, delta: i32) {
        let Screen::List { selected } = &mut self.screen else {
            return;
        };
        if self.isos.is_empty() {
            return;
        }
        let max = self.isos.len() - 1;
        if delta < 0 {
            let step = delta.unsigned_abs() as usize;
            *selected = selected.saturating_sub(step);
        } else {
            let step = usize::try_from(delta).unwrap_or(0);
            *selected = selected.saturating_add(step).min(max);
        }
    }

    /// Transition list → confirm for the highlighted ISO.
    pub fn confirm_selection(&mut self) {
        if let Screen::List { selected } = self.screen {
            if !self.isos.is_empty() {
                self.screen = Screen::Confirm { selected };
            }
        }
    }

    /// Transition confirm → list (cancel).
    pub fn cancel_confirmation(&mut self) {
        if let Screen::Confirm { selected } = self.screen {
            self.screen = Screen::List { selected };
        }
    }

    /// Record a kexec failure and transition to the Error screen.
    pub fn record_kexec_error(&mut self, err: &KexecError) {
        // If the error occurred during a Confirm flow, enrich with
        // ISO-specific context (sibling key discovery for
        // SignatureRejected).
        let iso = match &self.screen {
            Screen::Confirm { selected } | Screen::EditCmdline { selected, .. } => {
                self.isos.get(*selected)
            }
            _ => None,
        };
        let (message, remedy) = error_diagnostic_with_iso(err, iso);
        self.screen = Screen::Error { message, remedy };
    }

    /// Request a clean exit.
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

/// Build the MOK enrollment remedy text, naming the specific key file if
/// one is discoverable alongside the ISO. The operator should be able to
/// copy-paste the resulting command verbatim.
fn build_mokutil_remedy(iso: Option<&DiscoveredIso>) -> String {
    let key_hint = iso.and_then(|iso| find_sibling_key(&iso.iso_path));
    match key_hint {
        Some(key_path) => format!(
            "Enroll this ISO's signing key:\n  \
             sudo mokutil --import {}\n\
             Reboot and complete enrollment via MOK Manager (set a temporary \
             password at --import; MOK Manager will prompt for it on the \
             next boot). Then retry. Do NOT disable Secure Boot.",
            key_path.display()
        ),
        None => "This ISO's signing key isn't in the platform or MOK keyring. \
                 Place the public key alongside the ISO (as `<iso>.pub`, \
                 `<iso>.key`, or `<iso>.der`) and aegis-boot will suggest \
                 the exact `mokutil --import` command on the next attempt. \
                 Do NOT disable Secure Boot."
            .to_string(),
    }
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
            Some("rescue-tui is meant to run inside the signed Linux rescue initramfs.".to_string()),
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
        let mut s = AppState::new(vec![fake_iso("a"), fake_iso("b"), fake_iso("c")]);
        s.move_selection(-5);
        assert_eq!(s.screen, Screen::List { selected: 0 });
        s.move_selection(99);
        assert_eq!(s.screen, Screen::List { selected: 2 });
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
        };
        let (_, remedy) =
            error_diagnostic_with_iso(&KexecError::SignatureRejected, Some(&iso));
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
    fn is_kexec_blocked_false_for_unknown_index() {
        let s = AppState::new(vec![fake_iso("a")]);
        assert!(!s.is_kexec_blocked(99));
    }

    #[test]
    fn enter_cmdline_editor_seeds_with_iso_default() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.confirm_selection();
        s.enter_cmdline_editor();
        let Screen::EditCmdline { buffer, cursor, selected } = &s.screen else {
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
        assert_eq!(s.cmdline_overrides.get(&0), Some(&"boot=casper q".to_string()));
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
}
