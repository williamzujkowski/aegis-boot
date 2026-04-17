//! Pure rendering — given an [`AppState`], produce a frame on any
//! [`ratatui::backend::Backend`]. Tested with `TestBackend`.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, List, ListItem, ListState, Paragraph, Wrap},
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
/// │  aegis-boot v0.12.0    SB:enforcing  TPM:available   │ <- header
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
            message,
            remedy,
            return_to,
        } => draw_error(frame, area, state, *return_to, message, remedy.as_deref()),
        Screen::Verifying {
            selected,
            bytes,
            total,
            ..
        } => draw_verifying(frame, area, state, *selected, *bytes, *total),
        Screen::TrustChallenge { selected, buffer } => {
            draw_trust_challenge(frame, area, state, *selected, buffer);
        }
        Screen::Quitting | Screen::Help { .. } | Screen::ConfirmQuit { .. } => {}
    }
}

fn draw_trust_challenge(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &AppState,
    selected: usize,
    buffer: &str,
) {
    let Some(iso) = state.isos.get(selected) else {
        return;
    };
    let verdict = trust_verdict(iso);
    let lines = vec![
        Line::from(Span::styled(
            "Degraded trust — typed confirmation required",
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(state.theme.warning),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("Verdict: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(
                verdict.label(),
                Style::default().fg(verdict.color(&state.theme)),
            ),
        ]),
        Line::from(vec![
            Span::styled("ISO:     ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(iso.iso_path.display().to_string()),
        ]),
        Line::from(""),
        Line::from("This ISO lacks a verified signature in AEGIS_TRUSTED_KEYS, or has no"),
        Line::from("sidecar checksum/.minisig. Booting it is a trust decision."),
        Line::from(""),
        Line::from("To proceed, type the word below exactly then press Enter."),
        Line::from("Esc cancels."),
        Line::from(""),
        Line::from(vec![
            Span::styled("Type: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(
                "boot",
                Style::default()
                    .fg(state.theme.error)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("You:  ", Style::default().add_modifier(Modifier::BOLD)),
            // Design-review fix #2: mismatch feedback. If the operator
            // typed ≥4 chars and it's not the target token, render in
            // error colour so they see the buffer is wrong before they
            // hit Enter. Silent failure was a security-gate smell.
            Span::styled(
                buffer.to_string(),
                if buffer.len() >= 4 && buffer != "boot" {
                    Style::default()
                        .fg(state.theme.error)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                },
            ),
            Span::styled("│", Style::default().add_modifier(Modifier::SLOW_BLINK)),
        ]),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Trust challenge (#93) "),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_verifying(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &AppState,
    selected: usize,
    bytes: u64,
    total: u64,
) {
    let Some(iso) = state.isos.get(selected) else {
        return;
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Min(1),
        ])
        .split(area);
    let (label_area, path_area, gauge_area, note_area) =
        (chunks[0], chunks[1], chunks[2], chunks[3]);

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "Verifying:  ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(iso_probe::display_name(iso)),
        ])),
        label_area,
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "ISO path:   ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(iso.iso_path.display().to_string()),
        ])),
        path_area,
    );

    // Cast u64 → f64 for the ratio. Precision loss on the high bits is
    // meaningless for a progress bar — we just need a 0..=1 value.
    #[allow(clippy::cast_precision_loss)]
    let ratio = if total > 0 {
        (bytes as f64 / total as f64).clamp(0.0, 1.0)
    } else {
        0.0
    };
    // Cast f64 → u16 for percent. Ratio is pre-clamped to [0, 1],
    // so 100.0 * ratio ∈ [0, 100] — u16 cannot truncate or sign-flip.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let pct = (ratio * 100.0) as u16;
    let label = if total > 0 {
        format!(
            "{pct}%   ({} / {})",
            humanize_size(Some(bytes)),
            humanize_size(Some(total))
        )
    } else {
        "preparing…".to_string()
    };
    frame.render_widget(
        Gauge::default()
            .block(Block::default().borders(Borders::ALL).title(" SHA-256 "))
            .gauge_style(Style::default().fg(state.theme.success))
            .ratio(ratio)
            .label(label),
        gauge_area,
    );

    frame.render_widget(
        Paragraph::new(
            "Re-running hash verification against the ISO bytes on the\ndata partition. This is the same computation iso-probe ran at\ndiscovery time. Esc to cancel; the worker will finish in the\nbackground and the result will be discarded.",
        )
        .wrap(Wrap { trim: false }),
        note_area,
    );
}

fn draw_header(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let version = env!("CARGO_PKG_VERSION");
    let sb_color = match state.secure_boot {
        SecureBootStatus::Enforcing => state.theme.success,
        SecureBootStatus::Disabled => state.theme.error,
        SecureBootStatus::Unknown => state.theme.warning,
    };
    // Design-review fix #3: TPM colour must reflect TPM state, not be
    // hardcoded to green. A green "TPM:none" lies to the operator.
    let tpm_color = match state.tpm {
        crate::state::TpmStatus::Available => state.theme.success,
        crate::state::TpmStatus::Absent => state.theme.warning,
    };
    // Brand primary (steel blue) for the shield mark. Uses the aegis
    // brand colour directly — renders across every theme.
    let brand = ratatui::style::Color::Rgb(0x3B, 0x82, 0xF6);

    // Design-review fix #1: degrade the header for narrow terminals
    // instead of truncating mid-word. The tagline is the first thing
    // to go; then TPM; then SB. The shield mark + name + version
    // always survive.
    //   ≥90 cols → mark + name + version + SB + TPM + tagline
    //   ≥70 cols → mark + name + version + SB + TPM
    //   ≥50 cols → mark + name + version + SB
    //   <50 cols → mark + name + version
    let mut spans = vec![
        Span::styled(
            "◆ ",
            Style::default().fg(brand).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("aegis-boot v{version}"),
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ];
    if area.width >= 50 {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            state.secure_boot.summary(),
            Style::default().fg(sb_color),
        ));
    }
    if area.width >= 70 {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            state.tpm.summary(),
            Style::default().fg(tpm_color),
        ));
    }
    if area.width >= 90 {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            "Signed boot. Any ISO. Your keys.",
            Style::default()
                .fg(state.theme.success)
                .add_modifier(Modifier::ITALIC | Modifier::DIM),
        ));
    }
    let header = Line::from(spans);
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
                " [↑↓/jk] Move  [Enter] Boot  [/] Filter  [s] Sort  [v] Verify  [?] Help  [q] Quit"
            }
            Screen::Confirm { .. } => {
                " [Enter] kexec  [e] cmdline  [v] Verify  [Esc/h] Back  [?] Help  [q] Quit"
            }
            Screen::EditCmdline { .. } => {
                " [Enter] Save  [Esc] Cancel  [←/→] Move  [Backspace] Delete"
            }
            Screen::Error { .. } => {
                " [F10] Save evidence to AEGIS_ISOS  ·  any key = back  ·  [q] Quit"
            }
            Screen::Verifying { .. } => " Verifying in background  ·  [Esc] Cancel",
            Screen::TrustChallenge { .. } => " Type `boot` + Enter to proceed  ·  [Esc] Cancel",
            Screen::Quitting | Screen::Help { .. } | Screen::ConfirmQuit { .. } => "",
        }
    };
    frame.render_widget(Paragraph::new(hint), area);
}

/// Android Verified Boot-style coarse verdict for a single ISO. One of
/// four states drives a colored banner on the Confirm screen. (#93)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrustVerdict {
    /// Hash OR signature verified (strongest signal that sig exists).
    Green,
    /// Signature parsed but signer not in trust store (YELLOW on Android VB).
    Yellow,
    /// Bytes tampered / forged signature / not kexec-bootable.
    Red,
    /// No verification material at all.
    Gray,
}

impl TrustVerdict {
    fn label(self) -> &'static str {
        match self {
            Self::Green => "GREEN  VERIFIED",
            Self::Yellow => "YELLOW UNTRUSTED SIGNER",
            Self::Red => "RED    DO NOT BOOT",
            Self::Gray => "GRAY   NO VERIFICATION",
        }
    }

    fn reason(self) -> &'static str {
        match self {
            Self::Green => "hash + signature checked against trusted key",
            Self::Yellow => "signature parsed but key is not in AEGIS_TRUSTED_KEYS",
            Self::Red => "integrity failure — kexec will be refused",
            Self::Gray => "no sibling .sha256 or .minisig found",
        }
    }

    fn color(self, theme: &Theme) -> ratatui::style::Color {
        use ratatui::style::Color;
        match self {
            Self::Green => theme.success,
            Self::Yellow => theme.warning,
            Self::Red => theme.error,
            Self::Gray => Color::Gray,
        }
    }
}

fn trust_verdict(iso: &iso_probe::DiscoveredIso) -> TrustVerdict {
    use iso_probe::{HashVerification as H, Quirk, SignatureVerification as S};
    if iso.quirks.contains(&Quirk::NotKexecBootable)
        || matches!(iso.hash_verification, H::Mismatch { .. })
        || matches!(iso.signature_verification, S::Forged { .. })
    {
        return TrustVerdict::Red;
    }
    if matches!(iso.signature_verification, S::Verified { .. })
        || matches!(iso.hash_verification, H::Verified { .. })
    {
        return TrustVerdict::Green;
    }
    if matches!(iso.signature_verification, S::KeyNotTrusted { .. }) {
        return TrustVerdict::Yellow;
    }
    TrustVerdict::Gray
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
        Line::from("   v             verify now (re-run SHA-256 with progress)"),
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
        Line::from(" Themes (AEGIS_THEME env var)"),
        Line::from("   default · monochrome · high-contrast · okabe-ito · aegis"),
        Line::from(""),
        Line::from(" Emergency escape hatches (kernel SysRq)"),
        Line::from("   Alt+SysRq+b   reboot now"),
        Line::from("   Alt+SysRq+s   sync disks"),
        Line::from("   Alt+SysRq+e   SIGTERM all userspace"),
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

/// Render the empty-state screen when `discover()` returned zero ISOs.
///
/// Replaces the terse "No bootable ISOs found. Press q to quit, or check
/// that `AEGIS_ISO_ROOTS` points at a directory containing `.iso` files."
/// with a concrete diagnosis: the exact paths that were scanned, the
/// paths' existence state, and three specific actionable next steps
/// (mount media, copy an ISO, drop to rescue shell). (#85 Tier 2)
fn draw_empty_list(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let mut lines: Vec<Line<'_>> = Vec::new();
    lines.push(Line::from(Span::styled(
        "No bootable ISOs found.",
        Style::default().add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));

    if state.scanned_roots.is_empty() {
        lines.push(Line::from(
            "(paths scanned: none — AEGIS_ISO_ROOTS parsing returned an empty list)",
        ));
    } else {
        lines.push(Line::from(Span::styled(
            "Scanned these paths:",
            Style::default().add_modifier(Modifier::BOLD),
        )));
        for root in &state.scanned_roots {
            let exists_marker = if root.exists() { "exists" } else { "MISSING" };
            lines.push(Line::from(format!(
                "  {}  ({exists_marker})",
                root.display()
            )));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Next steps:",
        Style::default().add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(
        "  1. On the host, copy an ISO into the AEGIS_ISOS partition:",
    ));
    lines.push(Line::from("       aegis-boot add /path/to/distro.iso"));
    lines.push(Line::from(
        "     (or drag-and-drop the .iso file onto the stick via your file manager).",
    ));
    lines.push(Line::from(
        "  2. If the AEGIS_ISOS partition is on this stick but wasn't auto-mounted,",
    ));
    lines.push(Line::from(
        "     boot this stick on a host and run `aegis-boot doctor --stick /dev/sdX`.",
    ));
    lines.push(Line::from(
        "  3. Select the always-present rescue shell entry below (if enabled) to",
    ));
    lines.push(Line::from(
        "     drop to a busybox prompt and mount/inspect filesystems by hand.",
    ));
    lines.push(Line::from(""));
    lines.push(Line::from("Press q to reboot, ? for keybindings."));

    let panel = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" aegis-boot — no ISOs discovered "),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(panel, area);
}

/// Compute layout + render the optional inline error band (#85 Tier 2
/// last child) on the List screen. Returns `(info_area, list_area)`
/// for the caller to render into. Extracted from `draw_list` so the
/// main draw function stays under the workspace-wide 100-line cap.
fn split_list_chrome(frame: &mut Frame<'_>, area: Rect, state: &AppState) -> (Rect, Rect) {
    // Tier 2 (#85) — info bar above list shows filter + sort state.
    // When some ISOs on disk failed to parse, insert a one-line inline
    // error band ABOVE the info bar so it's unmissable without modal
    // interruption — memtest86+-style "one-frame warning".
    if state.skipped_iso_count > 0 {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // error band
                Constraint::Length(1), // info bar
                Constraint::Min(1),    // list
            ])
            .split(area);
        let band = Paragraph::new(Line::from(vec![
            Span::styled(
                " \u{26A0} SKIPPED ",
                Style::default()
                    .fg(state.theme.warning)
                    .add_modifier(Modifier::REVERSED | Modifier::BOLD),
            ),
            Span::raw(format!(
                "  {} ISO(s) on disk failed to parse — see journalctl -u rescue-tui (iso_parser warnings)",
                state.skipped_iso_count
            )),
        ]));
        frame.render_widget(band, chunks[0]);
        (chunks[1], chunks[2])
    } else {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(1)])
            .split(area);
        (chunks[0], chunks[1])
    }
}

fn draw_list(frame: &mut Frame<'_>, area: Rect, state: &AppState, selected: usize) {
    use crate::state::ViewEntry;
    if state.isos.is_empty() {
        draw_empty_list(frame, area, state);
        return;
    }

    let (info_area, list_area) = split_list_chrome(frame, area, state);

    // Design-review #102: filter-mode visual was too subtle (trailing
    // `_` the only indicator). Now: when editing, render a reversed-
    // style banner with "FILTER:" prefix so it's unmistakable, plus a
    // blinking caret span. Committed filter keeps the quieter style.
    if state.filter_editing {
        let styled = Line::from(vec![
            Span::styled(
                " FILTER ",
                Style::default()
                    .fg(state.theme.warning)
                    .add_modifier(Modifier::REVERSED | Modifier::BOLD),
            ),
            Span::raw("  /"),
            Span::styled(
                state.filter.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled("│", Style::default().add_modifier(Modifier::SLOW_BLINK)),
            Span::raw(format!(
                "   sort: {}   (Enter commits, Esc clears)",
                state.sort_order.summary()
            )),
        ]);
        frame.render_widget(Paragraph::new(styled), info_area);
    } else {
        let info_line = if state.filter.is_empty() {
            format!(
                " sort: {}   (/ filter, s cycle sort)",
                state.sort_order.summary()
            )
        } else {
            format!(
                " filter: \"{}\"   sort: {}   (/ edit, s cycle sort)",
                state.filter,
                state.sort_order.summary()
            )
        };
        frame.render_widget(Paragraph::new(info_line), info_area);
    }

    // Full entries view includes the rescue-shell synthetic row at
    // the end, even when the ISO list is empty (#90).
    let entries = state.visible_entries();
    let iso_entries: Vec<usize> = entries
        .iter()
        .filter_map(|e| {
            if let ViewEntry::Iso(i) = e {
                Some(*i)
            } else {
                None
            }
        })
        .collect();

    let items: Vec<ListItem> = entries
        .iter()
        .map(|e| match e {
            ViewEntry::Iso(i) => {
                let iso = &state.isos[*i];
                let glyph = status_glyph(iso);
                let qs = quirks_summary(iso);
                // Prefer pretty_name over label when present so operators see
                // the version (e.g. "Ubuntu 24.04.2 LTS" vs just "Ubuntu"). (#119)
                let display = iso_probe::display_name(iso);
                let line = if qs.is_empty() {
                    format!("{glyph} {}  ({})", display, iso.distribution_name())
                } else {
                    format!("{glyph} {}  ({})  {qs}", display, iso.distribution_name())
                };
                ListItem::new(line)
            }
            ViewEntry::RescueShell => {
                ListItem::new("[#] rescue shell (busybox)  — dropped from rescue-tui")
            }
        })
        .collect();

    let title = if iso_entries.is_empty() && state.filter.is_empty() {
        " aegis-boot — no ISOs; shell available ".to_string()
    } else if iso_entries.is_empty() {
        format!(" aegis-boot — no matches for \"{}\" ", state.filter)
    } else {
        format!(
            " aegis-boot — pick an ISO ({}/{} shown, +shell) ",
            iso_entries.len(),
            state.isos.len()
        )
    };
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("> ");

    let mut list_state = ListState::default();
    let cursor = selected.min(entries.len().saturating_sub(1));
    list_state.select(Some(cursor));
    frame.render_stateful_widget(list, list_area, &mut list_state);
}

// Intentionally over the 100-line threshold: the Confirm screen is the
// one place where verdict + digest + metadata + verification + hints all
// have to land together, and splitting it hurts more than the length.
#[allow(clippy::too_many_lines)]
fn draw_confirm(frame: &mut Frame<'_>, area: Rect, state: &AppState, selected: usize) {
    let Some(iso) = state.isos.get(selected) else {
        return;
    };
    let override_active = state.cmdline_overrides.contains_key(&selected);
    let effective_cmdline = state.effective_cmdline(selected);
    let cmdline_display = if effective_cmdline.is_empty() {
        "(none)".to_string()
    } else {
        effective_cmdline.clone()
    };
    let cmdline_label = if override_active {
        "Cmdline*: "
    } else {
        "Cmdline:  "
    };

    // Android VB-style verdict line (#93). One coarse GREEN / YELLOW /
    // RED / GRAY state derived from hash + signature + quirk results.
    let verdict = trust_verdict(iso);
    let verdict_line = Line::from(vec![
        Span::styled("Verdict:  ", Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(
            verdict.label(),
            Style::default()
                .fg(verdict.color(&state.theme))
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::raw(verdict.reason()),
    ]);

    // TPM PCR 12 measurement preview (#93). Shows exactly what bytes
    // will be extended into PCR 12 before kexec. Truncated to 16 hex
    // chars (8 bytes) by default — full 64-char hash available in log
    // stream and audit line.
    let measurement = crate::tpm::compute_measurement(&iso.iso_path, &effective_cmdline);
    let digest_hex = hex::encode(measurement);
    let digest_line = Line::from(vec![
        Span::styled("Measures: ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(format!("sha256:{}…", &digest_hex[..16])),
        Span::raw("  "),
        Span::styled("→ PCR 12", Style::default().add_modifier(Modifier::DIM)),
    ]);

    // #131: installer-vs-live warning. A distro-signed ISO has a
    // GREEN verdict; the operator might read that as "safe" and hit
    // Enter without realizing the ISO contains an installer that can
    // erase disks on this machine when the WRONG entry is picked
    // from the ISO's own boot menu. One visual warning line — no
    // extra typed challenge.
    let mut lines: Vec<Line> = vec![verdict_line];
    if iso.contains_installer {
        lines.push(Line::from(vec![
            Span::styled(
                "Warning:  ",
                Style::default()
                    .fg(state.theme.warning)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "This ISO contains an installer. If the ISO's own boot menu",
                Style::default().fg(state.theme.warning),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::raw("          "),
            Span::styled(
                "default is 'Install', DISKS ON THIS MACHINE MAY BE ERASED.",
                Style::default()
                    .fg(state.theme.warning)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
    }
    lines.extend([
        digest_line,
        Line::from(""),
        Line::from(vec![
            Span::styled("Label:    ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(iso_probe::display_name(iso)),
        ]),
        Line::from(vec![
            Span::styled("ISO:      ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(iso.iso_path.display().to_string()),
        ]),
        Line::from(vec![
            Span::styled("Size:     ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(humanize_size(iso.size_bytes)),
        ]),
        // Design-review #101: compact kernel+initrd onto one line and
        // checksum+signature onto one "Trust:" line so the verdict at
        // top stays visible on 24-row terminals with verbose quirks.
        Line::from(vec![
            Span::styled("Boot:     ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(format!(
                "{}  +  {}",
                iso.kernel.display(),
                iso.initrd
                    .as_ref()
                    .map_or("(no initrd)".to_string(), |p| p.display().to_string()),
            )),
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
            Span::styled("Trust:    ", Style::default().add_modifier(Modifier::BOLD)),
            checksum_span(&iso.hash_verification, &state.theme),
            Span::raw("   "),
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
    ]);
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
        iso_probe::HashVerification::Unreadable { .. } => Span::styled(
            // Sidecar exists but unreadable — distinct yellow verdict
            // rather than gray "not present". (#138)
            "⚠ checksum file unreadable",
            Style::default().fg(theme.warning),
        ),
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

// memtest86+ single-frame evidence: a screenshot of this panel should
// be a complete bug report — no external context needed. (#92)
#[allow(clippy::too_many_lines)]
fn draw_error(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &AppState,
    return_to: usize,
    message: &str,
    remedy: Option<&str>,
) {
    let iso = state.isos.get(return_to);
    let cmdline = state.effective_cmdline(return_to);
    let measurement_hex = iso
        .map(|i| hex::encode(crate::tpm::compute_measurement(&i.iso_path, &cmdline)))
        .unwrap_or_default();

    let mut lines = vec![
        Line::from(Span::styled(
            "kexec failed — capture this screen for bug reports",
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(state.theme.error),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "Diagnostic: ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(message.to_string()),
        ]),
    ];
    if let Some(r) = remedy {
        lines.push(Line::from(vec![
            Span::styled(
                "Remedy:     ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(r.to_string()),
        ]));
    }

    // Evidence block — only if we have an ISO context.
    if let Some(iso) = iso {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "── Evidence (memtest-style; one frame = one bug report) ──",
            Style::default().add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(vec![
            Span::styled(
                "Version:    ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("aegis-boot v{}", env!("CARGO_PKG_VERSION"))),
        ]));
        lines.push(Line::from(vec![
            Span::styled(
                "SB / TPM:   ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(
                "{}  ·  {}",
                state.secure_boot.summary(),
                state.tpm.summary()
            )),
        ]));
        lines.push(Line::from(vec![
            Span::styled(
                "ISO label:  ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(iso_probe::display_name(iso).to_string()),
        ]));
        lines.push(Line::from(vec![
            Span::styled(
                "ISO path:   ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(iso.iso_path.display().to_string()),
        ]));
        lines.push(Line::from(vec![
            Span::styled(
                "Size:       ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(humanize_size(iso.size_bytes)),
        ]));
        lines.push(Line::from(vec![
            Span::styled(
                "Distro:     ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(iso.distribution_name().to_string()),
        ]));
        lines.push(Line::from(vec![
            Span::styled(
                "Verdict:    ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(trust_verdict(iso).label().to_string()),
        ]));
        lines.push(Line::from(vec![
            Span::styled(
                "Cmdline:    ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(if cmdline.is_empty() {
                "(none)".to_string()
            } else {
                cmdline
            }),
        ]));
        lines.push(Line::from(vec![
            Span::styled(
                "Measured:   ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("sha256:{measurement_hex}")),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "F10 = save log to AEGIS_ISOS  ·  any key = back to list  ·  q = quit",
        Style::default().fg(state.theme.warning),
    )));
    let para = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" kexec error "),
        )
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
            pretty_name: None,
            distribution: Distribution::Debian,
            kernel: PathBuf::from("casper/vmlinuz"),
            initrd: Some(PathBuf::from("casper/initrd")),
            cmdline: Some("boot=casper".to_string()),
            quirks: vec![],
            hash_verification: iso_probe::HashVerification::NotPresent,
            signature_verification: iso_probe::SignatureVerification::NotPresent,
            size_bytes: Some(1_500_000_000),
            contains_installer: false,
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
    fn empty_list_shows_next_steps_with_actionable_commands() {
        // (#85 Tier 2) — empty state must tell the operator WHAT to do,
        // not just "no ISOs found". Verify the three actionable next
        // steps all render.
        let state = AppState::new(vec![]);
        let s = render_to_string(&state);
        assert!(
            s.contains("Next steps"),
            "missing 'Next steps' heading in: {s}"
        );
        assert!(
            s.contains("aegis-boot add"),
            "missing `aegis-boot add` recipe in: {s}",
        );
        assert!(
            s.contains("aegis-boot doctor"),
            "missing `aegis-boot doctor` recipe in: {s}",
        );
        assert!(
            s.contains("rescue shell"),
            "missing rescue-shell escape-hatch mention in: {s}",
        );
    }

    #[test]
    fn empty_list_surfaces_scanned_roots() {
        // Operator seeing "no ISOs" needs to know WHERE we looked. The
        // TUI should echo each scanned path with its existence state.
        let state = AppState::new(vec![]).with_scanned_roots(vec![
            std::path::PathBuf::from("/this-path-definitely-does-not-exist"),
            std::path::PathBuf::from("/tmp"),
        ]);
        let s = render_to_string(&state);
        assert!(
            s.contains("/this-path-definitely-does-not-exist"),
            "missing scanned path 1 in: {s}",
        );
        assert!(
            s.contains("MISSING"),
            "missing existence marker for non-existent path in: {s}",
        );
        assert!(s.contains("/tmp"), "missing scanned path 2 in: {s}",);
        assert!(
            s.contains("exists"),
            "missing existence marker for existing path in: {s}",
        );
    }

    #[test]
    fn empty_list_with_no_scanned_roots_explains_why() {
        // Degenerate case: AEGIS_ISO_ROOTS parsing returned empty —
        // unusual but possible if env var is literally empty. Tell
        // the operator rather than silently showing no paths.
        let state = AppState::new(vec![]);
        // Default state has scanned_roots = vec![] (not set by main.rs in tests).
        let s = render_to_string(&state);
        assert!(
            s.contains("AEGIS_ISO_ROOTS") || s.contains("paths scanned: none"),
            "empty-state with no-roots must explain: {s}",
        );
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
    fn list_inline_band_appears_when_some_isos_skipped() {
        // (#85 Tier 2 last child) — operator sees "N ISO(s) on disk
        // failed to parse" without needing to read journalctl.
        let state = AppState::new(vec![fake_iso("ok")]).with_skipped_iso_count(2);
        let s = render_to_string(&state);
        assert!(
            s.contains("SKIPPED") && s.contains("2 ISO(s) on disk failed to parse"),
            "expected inline error band in: {s}",
        );
        // The good ISO should still render alongside the band.
        assert!(s.contains("ok"));
    }

    #[test]
    fn list_no_inline_band_when_nothing_skipped() {
        // Default count is 0; band should not appear.
        let state = AppState::new(vec![fake_iso("ok")]);
        let s = render_to_string(&state);
        assert!(
            !s.contains("SKIPPED"),
            "unexpected error band in clean-state render: {s}",
        );
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
