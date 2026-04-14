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
}

impl AppState {
    /// Build a fresh state from a discovery result.
    #[must_use]
    pub fn new(isos: Vec<DiscoveredIso>) -> Self {
        Self {
            isos,
            screen: Screen::List { selected: 0 },
        }
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
        let (message, remedy) = error_diagnostic(err);
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
        let (Screen::List { selected } | Screen::Confirm { selected }) = self.screen else {
            return None;
        };
        self.isos.get(selected)
    }
}

/// Map a [`KexecError`] to (user-facing message, optional remedy hint).
///
/// Diagnostics are deliberately specific so the TUI never shows a generic
/// "kexec failed" — see ADR 0001's commitment to "no black screens."
#[must_use]
pub fn error_diagnostic(err: &KexecError) -> (String, Option<String>) {
    match err {
        KexecError::SignatureRejected => (
            "Kernel signature verification failed (KEXEC_SIG)".to_string(),
            Some(
                "Enroll this ISO's signing key with `mokutil --import <key.der>`, reboot, then \
                 retry. Do NOT disable Secure Boot."
                    .to_string(),
            ),
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
