//! Pure rendering — given an [`AppState`], produce a frame on any
//! [`ratatui::backend::Backend`]. Tested with `TestBackend`.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};

use crate::state::{quirks_summary, AppState, Screen, SecureBootStatus};
use crate::theme::Theme;

/// Render the current frame for the given state.
///
/// Layout (#85):
///
/// ```text
/// ┌──────────────────────────────────────────────────────┐
/// │  aegis-boot v0.7.1     SB:enforcing  TPM:available   │ <- header
/// ├──────────────────────────────────────────────────────┤
/// │                                                      │
/// │              (current screen body)                   │ <- body
/// │                                                      │
/// ├──────────────────────────────────────────────────────┤
/// │  [↑↓/jk] Move  [Enter] Boot  [/] Filter  [?] Help    │ <- footer
/// └──────────────────────────────────────────────────────┘
/// ```
pub fn draw(frame: &mut Frame<'_>, state: &AppState) {
    let area = frame.area();
    let chrome = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header banner
            Constraint::Min(3),    // body
            Constraint::Length(1), // footer
        ])
        .split(area);
    let (header_area, body_area, footer_area) = (chrome[0], chrome[1], chrome[2]);

    draw_header(frame, header_area, state);
    draw_body(frame, body_area, state);
    draw_footer(frame, footer_area, state);

    // Overlays draw on top of everything.
    if let Screen::Help { .. } = &state.screen {
        draw_help_overlay(frame, area, state);
    }
    if let Screen::ConfirmQuit { .. } = &state.screen {
        draw_confirm_quit_overlay(frame, area, state);
    }
}

fn draw_body(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    // Help and ConfirmQuit overlays draw the prior screen underneath
    // for context, then layer on top.
    let effective = match &state.screen {
        Screen::Help { prior } | Screen::ConfirmQuit { prior } => prior.as_ref(),
        other => other,
    };
    match effective {
        Screen::List { selected } => draw_list(frame, area, state, *selected),
        Screen::Confirm { selected } => draw_confirm(frame, area, state, *selected),
        Screen::EditCmdline {
            selected,
            buffer,
            cursor,
        } => draw_edit_cmdline(frame, area, state, *selected, buffer, *cursor),
        Screen::Error {
            message, remedy, ..
        } => draw_error(frame, area, message, remedy.as_deref()),
        Screen::Quitting | Screen::Help { .. } | Screen::ConfirmQuit { .. } => {}
    }
}

fn draw_header(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let version = env!("CARGO_PKG_VERSION");
    let sb_color = match state.secure_boot {
        SecureBootStatus::Enforcing => state.theme.success,
        SecureBootStatus::Disabled => state.theme.error,
        SecureBootStatus::Unknown => state.theme.warning,
    };
    let header = Line::from(vec![
        Span::styled(
            format!(" aegis-boot v{version} "),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(state.secure_boot.summary(), Style::default().fg(sb_color)),
        Span::raw("  "),
        Span::styled(
            state.tpm.summary(),
            Style::default().fg(state.theme.success),
        ),
    ]);
    frame.render_widget(Paragraph::new(header), area);
}

fn draw_footer(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    // Footer hints depend on the underlying screen, not the overlay.
    let effective = match &state.screen {
        Screen::Help { prior } | Screen::ConfirmQuit { prior } => prior.as_ref(),
        other => other,
    };
    let hint = if state.filter_editing {
        " Filter: type to match  ·  [Enter] commit  ·  [Esc] clear"
    } else {
        match effective {
            Screen::List { .. } => {
                " [↑↓/jk] Move  [Enter] Boot  [/] Filter  [s] Sort  [?] Help  [q] Quit"
            }
            Screen::Confirm { .. } => {
                " [Enter] kexec  [e] Edit cmdline  [Esc/h] Back  [?] Help  [q] Quit"
            }
            Screen::EditCmdline { .. } => {
                " [Enter] Save  [Esc] Cancel  [←/→] Move  [Backspace] Delete"
            }
            Screen::Error { .. } => " Press any key to return to the list  ·  [q] Quit",
            Screen::Quitting | Screen::Help { .. } | Screen::ConfirmQuit { .. } => "",
        }
    };
    frame.render_widget(Paragraph::new(hint), area);
}

/// Single-character status glyph for a list row, encoding the worst
/// security state. Visible in monochrome themes (no color reliance).
/// (#85, k9s/dialog pattern.)
fn status_glyph(iso: &iso_probe::DiscoveredIso) -> &'static str {
    use iso_probe::{HashVerification as H, Quirk, SignatureVerification as S};
    if iso.quirks.contains(&Quirk::NotKexecBootable) {
        return "[X]"; // can't kexec at all
    }
    if matches!(iso.hash_verification, H::Mismatch { .. }) {
        return "[!]"; // tampered
    }
    if matches!(iso.signature_verification, S::Forged { .. }) {
        return "[!]"; // crypto fail
    }
    if matches!(iso.signature_verification, S::Verified { .. }) {
        return "[+]"; // signed + trusted
    }
    if matches!(iso.hash_verification, H::Verified { .. }) {
        return "[~]"; // hash ok, sig absent
    }
    "[ ]"
}

fn draw_help_overlay(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    // Center a 60x18 panel.
    let w = area.width.min(70);
    let h = area.height.min(20);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let panel = Rect::new(x, y, w, h);
    let lines = vec![
        Line::from(Span::styled(
            "Keybindings",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(" Global"),
        Line::from("   ?         this help"),
        Line::from("   q         quit (with confirmation)"),
        Line::from(""),
        Line::from(" List screen"),
        Line::from("   ↑ ↓ / j k     move selection"),
        Line::from("   g / G         first / last entry"),
        Line::from("   Enter / l     confirm selection"),
        Line::from("   /             open filter (substring match)"),
        Line::from("   s             cycle sort: name → size → distro"),
        Line::from(""),
        Line::from(" Confirm screen"),
        Line::from("   Enter         kexec into the ISO"),
        Line::from("   e             edit kernel cmdline"),
        Line::from("   Esc / h       back to list"),
        Line::from(""),
        Line::from(" Status glyphs on list rows"),
        Line::from("   [+] verified  [~] hash-only  [ ] unknown"),
        Line::from("   [!] tampered/forged  [X] not kexec-bootable"),
        Line::from(""),
        Line::from(Span::styled(
            " Esc or ? to dismiss",
            Style::default().fg(state.theme.warning),
        )),
    ];
    let block = Block::default().borders(Borders::ALL).title(" Help (#85) ");
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        panel,
    );
}

fn draw_confirm_quit_overlay(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let w = area.width.min(50);
    let h = 7;
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let panel = Rect::new(x, y, w, h);
    let lines = vec![
        Line::from(Span::styled(
            "Quit aegis-boot?",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("This will reboot the machine."),
        Line::from(""),
        Line::from(Span::styled(
            " [y/Enter] yes    [n/Esc/q] cancel",
            Style::default().fg(state.theme.warning),
        )),
    ];
    let block = Block::default().borders(Borders::ALL).title(" Confirm ");
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        panel,
    );
}

fn draw_list(frame: &mut Frame<'_>, area: Rect, state: &AppState, selected: usize) {
    if state.isos.is_empty() {
        let empty = Paragraph::new(
            "No bootable ISOs found.\n\nPress q to quit, or check that AEGIS_ISO_ROOTS\npoints at a directory containing .iso files.",
        )
        .block(Block::default().borders(Borders::ALL).title("aegis-boot"));
        frame.render_widget(empty, area);
        return;
    }

    // Tier 2 (#85) — info bar above list shows filter + sort state.
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(area);
    let (info_area, list_area) = (chunks[0], chunks[1]);

    let info_line = if state.filter_editing {
        format!(
            " /{}_   sort: {}   (Enter commits, Esc clears)",
            state.filter,
            state.sort_order.summary()
        )
    } else if !state.filter.is_empty() {
        format!(
            " filter: \"{}\"   sort: {}   (/ edit, s cycle sort)",
            state.filter,
            state.sort_order.summary()
        )
    } else {
        format!(
            " sort: {}   (/ filter, s cycle sort)",
            state.sort_order.summary()
        )
    };
    frame.render_widget(Paragraph::new(info_line), info_area);

    let view = state.visible_indices();
    if view.is_empty() {
        let msg = format!(
            "No ISOs match filter \"{}\".\nPress / to edit or Esc to clear.",
            state.filter
        );
        let p = Paragraph::new(msg).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" aegis-boot — no matches "),
        );
        frame.render_widget(p, list_area);
        return;
    }

    let items: Vec<ListItem> = view
        .iter()
        .map(|&i| {
            let iso = &state.isos[i];
            let glyph = status_glyph(iso);
            let qs = quirks_summary(iso);
            let line = if qs.is_empty() {
                format!("{glyph} {}  ({})", iso.label, iso.distribution_name())
            } else {
                format!("{glyph} {}  ({})  {qs}", iso.label, iso.distribution_name())
            };
            ListItem::new(line)
        })
        .collect();

    let title = format!(
        " aegis-boot — pick an ISO ({}/{} shown) ",
        view.len(),
        state.isos.len()
    );
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("> ");

    let mut list_state = ListState::default();
    let cursor = selected.min(view.len().saturating_sub(1));
    list_state.select(Some(cursor));
    frame.render_stateful_widget(list, list_area, &mut list_state);
}

fn draw_confirm(frame: &mut Frame<'_>, area: Rect, state: &AppState, selected: usize) {
    let Some(iso) = state.isos.get(selected) else {
        return;
    };
    let override_active = state.cmdline_overrides.contains_key(&selected);
    let effective_cmdline = state.effective_cmdline(selected);
    let cmdline_display = if effective_cmdline.is_empty() {
        "(none)".to_string()
    } else {
        effective_cmdline
    };
    let cmdline_label = if override_active {
        "Cmdline*: "
    } else {
        "Cmdline:  "
    };
    let lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled("Label:    ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(&iso.label),
        ]),
        Line::from(vec![
            Span::styled("ISO:      ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(iso.iso_path.display().to_string()),
        ]),
        Line::from(vec![
            Span::styled("Size:     ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(humanize_size(iso.size_bytes)),
        ]),
        Line::from(vec![
            Span::styled("Kernel:   ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(iso.kernel.display().to_string()),
        ]),
        Line::from(vec![
            Span::styled("Initrd:   ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(
                iso.initrd
                    .as_ref()
                    .map_or("(none)".to_string(), |p| p.display().to_string()),
            ),
        ]),
        Line::from(vec![
            Span::styled(cmdline_label, Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(cmdline_display),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Quirks:   ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(if iso.quirks.is_empty() {
                "(none known)".to_string()
            } else {
                quirks_summary(iso)
            }),
        ]),
        Line::from(vec![
            Span::styled("Checksum: ", Style::default().add_modifier(Modifier::BOLD)),
            checksum_span(&iso.hash_verification, &state.theme),
        ]),
        Line::from(vec![
            Span::styled("Signature:", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" "),
            signature_span(&iso.signature_verification, &state.theme),
        ]),
        Line::from(""),
        // Per-screen action hint kept inline to make BLOCKED state
        // unmissable; full keybind list is in the persistent footer.
        Line::from(if state.is_kexec_blocked(selected) {
            Span::styled(
                "Enter: BLOCKED — verification or quirk failure",
                Style::default().fg(state.theme.error),
            )
        } else {
            Span::raw("Enter: kexec   ·   e: edit kernel cmdline   ·   Esc: back to list")
        }),
    ];
    let title = if state.is_kexec_blocked(selected) {
        "Confirm kexec — BLOCKED"
    } else if override_active {
        "Confirm kexec (cmdline overridden)"
    } else {
        "Confirm kexec"
    };
    let para = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(title))
        .wrap(Wrap { trim: false });
    frame.render_widget(para, area);
}

fn draw_edit_cmdline(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &AppState,
    selected: usize,
    buffer: &str,
    cursor: usize,
) {
    let default_cmdline = state
        .isos
        .get(selected)
        .and_then(|i| i.cmdline.clone())
        .unwrap_or_default();
    let cursor_marker = "│"; // U+2502 BOX DRAWINGS LIGHT VERTICAL — one grapheme
    let (before, after) = buffer.split_at(cursor.min(buffer.len()));
    let rendered_buffer = format!("{before}{cursor_marker}{after}");

    let lines = vec![
        Line::from(vec![Span::styled(
            "Edit kernel command line",
            Style::default().add_modifier(Modifier::BOLD),
        )]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Default: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(if default_cmdline.is_empty() {
                "(none)".to_string()
            } else {
                default_cmdline
            }),
        ]),
        Line::from(vec![
            Span::styled("Current: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(rendered_buffer),
        ]),
        Line::from(""),
        Line::from("Enter: save · Esc: cancel · ←/→: move · Backspace: delete"),
    ];
    let para = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("Edit cmdline"))
        .wrap(Wrap { trim: false });
    frame.render_widget(para, area);
}

fn checksum_span<'a>(verification: &iso_probe::HashVerification, theme: &Theme) -> Span<'a> {
    match verification {
        iso_probe::HashVerification::Verified { .. } => {
            Span::styled("✓ verified", Style::default().fg(theme.success))
        }
        iso_probe::HashVerification::Mismatch { .. } => Span::styled(
            "✗ MISMATCH — do NOT kexec",
            Style::default()
                .fg(theme.error)
                .add_modifier(Modifier::BOLD),
        ),
        iso_probe::HashVerification::NotPresent => Span::raw("(no sibling checksum)"),
    }
}

fn signature_span<'a>(verification: &iso_probe::SignatureVerification, theme: &Theme) -> Span<'a> {
    match verification {
        iso_probe::SignatureVerification::Verified { key_id, .. } => Span::styled(
            format!("✓ verified (signer: {key_id})"),
            Style::default().fg(theme.success),
        ),
        iso_probe::SignatureVerification::KeyNotTrusted { key_id } => Span::styled(
            format!("⚠ signer not trusted ({key_id})"),
            Style::default().fg(theme.warning),
        ),
        iso_probe::SignatureVerification::Forged { .. } => Span::styled(
            "✗ FORGED — bytes don't match sig",
            Style::default()
                .fg(theme.error)
                .add_modifier(Modifier::BOLD),
        ),
        iso_probe::SignatureVerification::Error { .. } => {
            Span::styled("? sig parse error", Style::default().fg(theme.warning))
        }
        iso_probe::SignatureVerification::NotPresent => Span::raw("(no .minisig sidecar)"),
    }
}

const KB: u64 = 1024;
const MB: u64 = KB * 1024;
const GB: u64 = MB * 1024;

#[allow(clippy::cast_precision_loss)]
fn humanize_size(bytes: Option<u64>) -> String {
    let Some(b) = bytes else {
        return "(unknown)".to_string();
    };
    if b >= GB {
        format!("{:.2} GiB", b as f64 / GB as f64)
    } else if b >= MB {
        format!("{:.1} MiB", b as f64 / MB as f64)
    } else if b >= KB {
        format!("{:.0} KiB", b as f64 / KB as f64)
    } else {
        format!("{b} B")
    }
}

fn draw_error(frame: &mut Frame<'_>, area: Rect, message: &str, remedy: Option<&str>) {
    let mut lines = vec![
        Line::from(vec![Span::styled(
            "kexec failed",
            Style::default().add_modifier(Modifier::BOLD),
        )]),
        Line::from(""),
        Line::from(message.to_string()),
    ];
    if let Some(r) = remedy {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            "Remedy:",
            Style::default().add_modifier(Modifier::BOLD),
        )]));
        lines.push(Line::from(r.to_string()));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(
        "Press q to quit, any other key to return to the list.",
    ));
    let para = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("Error"))
        .wrap(Wrap { trim: false });
    frame.render_widget(para, area);
}

trait DistributionLabel {
    fn distribution_name(&self) -> &'static str;
}

impl DistributionLabel for iso_probe::DiscoveredIso {
    fn distribution_name(&self) -> &'static str {
        match self.distribution {
            iso_probe::Distribution::Arch => "Arch",
            iso_probe::Distribution::Debian => "Debian/Ubuntu",
            iso_probe::Distribution::Fedora => "Fedora",
            iso_probe::Distribution::RedHat => "RHEL/Rocky/Alma",
            iso_probe::Distribution::Alpine => "Alpine",
            iso_probe::Distribution::NixOS => "NixOS",
            iso_probe::Distribution::Windows => "Windows",
            iso_probe::Distribution::Unknown => "unknown",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iso_probe::{Distribution, Quirk};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::path::PathBuf;

    fn fake_iso(label: &str) -> iso_probe::DiscoveredIso {
        iso_probe::DiscoveredIso {
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
        }
    }

    fn render_to_string(state: &AppState) -> String {
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap_or_else(|e| panic!("terminal: {e}"));
        terminal
            .draw(|f| draw(f, state))
            .unwrap_or_else(|e| panic!("draw: {e}"));
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>()
    }

    #[test]
    fn empty_list_renders_help_text() {
        let state = AppState::new(vec![]);
        let s = render_to_string(&state);
        assert!(s.contains("No bootable ISOs"));
        assert!(s.contains("aegis-boot"));
    }

    #[test]
    fn list_renders_each_iso_label() {
        let state = AppState::new(vec![fake_iso("alpha"), fake_iso("beta")]);
        let s = render_to_string(&state);
        assert!(s.contains("alpha"));
        assert!(s.contains("beta"));
        assert!(s.contains("Debian/Ubuntu"));
    }

    #[test]
    fn list_shows_quirk_summary_for_flagged_iso() {
        let mut iso = fake_iso("warn");
        iso.quirks = vec![Quirk::UnsignedKernel];
        let state = AppState::new(vec![iso]);
        let s = render_to_string(&state);
        assert!(s.contains("unsigned-kernel"));
    }

    #[test]
    fn confirm_screen_shows_kernel_and_cmdline() {
        let mut state = AppState::new(vec![fake_iso("x")]);
        state.confirm_selection();
        let s = render_to_string(&state);
        assert!(s.contains("casper/vmlinuz"));
        assert!(s.contains("boot=casper"));
        assert!(s.contains("Confirm kexec"));
    }

    #[test]
    fn confirm_screen_shows_humanized_size() {
        let mut state = AppState::new(vec![fake_iso("x")]);
        state.confirm_selection();
        let s = render_to_string(&state);
        // fake_iso uses 1_500_000_000 bytes ≈ 1.40 GiB
        assert!(s.contains("Size:"), "missing Size: label in {s}");
        assert!(s.contains("GiB"), "missing GiB unit in {s}");
    }

    #[test]
    fn humanize_size_handles_all_units() {
        assert_eq!(humanize_size(None), "(unknown)");
        assert_eq!(humanize_size(Some(0)), "0 B");
        assert_eq!(humanize_size(Some(512)), "512 B");
        assert_eq!(humanize_size(Some(2048)), "2 KiB");
        assert_eq!(humanize_size(Some(2 * 1024 * 1024)), "2.0 MiB");
        assert_eq!(humanize_size(Some(3 * 1024 * 1024 * 1024)), "3.00 GiB");
    }

    #[test]
    fn error_screen_shows_message_and_remedy() {
        let mut state = AppState::new(vec![fake_iso("x")]);
        state.record_kexec_error(&kexec_loader::KexecError::SignatureRejected);
        let s = render_to_string(&state);
        assert!(s.contains("kexec failed"));
        assert!(s.contains("signature") || s.contains("Signature"));
        assert!(s.contains("mokutil"));
    }
}
