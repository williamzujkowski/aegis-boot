//! Pure rendering — given an [`AppState`], produce a frame on any
//! [`ratatui::backend::Backend`]. Tested with `TestBackend`.

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};

use crate::state::{AppState, Screen, quirks_summary};

/// Render the current frame for the given state.
pub fn draw(frame: &mut Frame<'_>, state: &AppState) {
    let area = frame.area();
    match &state.screen {
        Screen::List { selected } => draw_list(frame, area, state, *selected),
        Screen::Confirm { selected } => draw_confirm(frame, area, state, *selected),
        Screen::Error { message, remedy } => draw_error(frame, area, message, remedy.as_deref()),
        Screen::Quitting => {}
    }
}

fn draw_list(frame: &mut Frame<'_>, area: Rect, state: &AppState, selected: usize) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(2)])
        .split(area);

    if state.isos.is_empty() {
        let empty = Paragraph::new("No bootable ISOs found.\nPress q to quit.")
            .block(Block::default().borders(Borders::ALL).title("aegis-boot"));
        frame.render_widget(empty, chunks[0]);
        return;
    }

    let items: Vec<ListItem> = state
        .isos
        .iter()
        .map(|iso| {
            let qs = quirks_summary(iso);
            let line = if qs.is_empty() {
                format!("{}  ({})", iso.label, iso.distribution_name())
            } else {
                format!("{}  ({})  {}", iso.label, iso.distribution_name(), qs)
            };
            ListItem::new(line)
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("aegis-boot — pick an ISO"),
        )
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("> ");

    let mut list_state = ListState::default();
    list_state.select(Some(selected));
    frame.render_stateful_widget(list, chunks[0], &mut list_state);

    let help = Paragraph::new("↑/↓: navigate · Enter: select · q: quit");
    frame.render_widget(help, chunks[1]);
}

fn draw_confirm(frame: &mut Frame<'_>, area: Rect, state: &AppState, selected: usize) {
    let Some(iso) = state.isos.get(selected) else {
        return;
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
            Span::styled("Cmdline:  ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(iso.cmdline.clone().unwrap_or_else(|| "(none)".to_string())),
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
        Line::from(""),
        Line::from("Press Enter to kexec into this ISO, Esc to cancel."),
    ];
    let para = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Confirm kexec"),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(para, area);
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
    lines.push(Line::from("Press q to quit, any other key to return to the list."));
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
            iso_probe::Distribution::Unknown => "unknown",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iso_probe::{Distribution, Quirk};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
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
        }
    }

    fn render_to_string(state: &AppState) -> String {
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap_or_else(|e| panic!("terminal: {e}"));
        terminal.draw(|f| draw(f, state)).unwrap_or_else(|e| panic!("draw: {e}"));
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
    fn error_screen_shows_message_and_remedy() {
        let mut state = AppState::new(vec![fake_iso("x")]);
        state.record_kexec_error(&kexec_loader::KexecError::SignatureRejected);
        let s = render_to_string(&state);
        assert!(s.contains("kexec failed"));
        assert!(s.contains("signature") || s.contains("Signature"));
        assert!(s.contains("mokutil"));
    }
}
