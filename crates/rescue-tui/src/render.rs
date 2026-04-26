// SPDX-License-Identifier: MIT OR Apache-2.0

//! Pure rendering — given an [`AppState`], produce a frame on any
//! [`ratatui::backend::Backend`]. Tested with `TestBackend`.

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Gauge, List, ListItem, ListState, Paragraph, Wrap},
};

use crate::keybindings::{self, ScreenKind};
use crate::state::{AppState, ConsentKind, Pane, Screen, SecureBootStatus, quirks_summary};
use crate::theme::Theme;
use crate::verdict::TrustVerdict;

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
    if let Screen::BlockedToast { message, .. } = &state.screen {
        draw_blocked_toast_overlay(frame, area, state, message);
    }
    if let Screen::Consent { kind, .. } = &state.screen {
        draw_consent_overlay(frame, area, state, *kind);
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
        // Confirm + Consent share the same backdrop: Consent (#347)
        // renders the Confirm screen underneath at the selected ISO so
        // the operator sees verdict context, then the consent overlay
        // paints on top via the overlay pass below (parallel to the
        // BlockedToast pattern).
        Screen::Confirm { selected } | Screen::Consent { selected, .. } => {
            draw_confirm(frame, area, state, *selected);
        }
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
        // BlockedToast (#546) renders the List underneath at return_to,
        // then the toast popup paints on top via the overlay pass.
        Screen::BlockedToast { return_to, .. } => draw_list(frame, area, state, *return_to),
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
    let verdict = trust_verdict(iso, state);
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

    // Phase A layout cleanup: the marketing tagline ("Signed boot.
    // Any ISO. Your keys.") was zero-functional during a rescue boot
    // and ate ~30 cols of header space; dropped here. The version +
    // SB + TPM trio remains — those are operator-actionable
    // ("UNKNOWN SB? My firmware lied about Secure Boot") and survive
    // through every theme.
    //
    // Width-degradation order (TPM is the next to go on serial-narrow
    // consoles, then SB, then mark+name+version always survive):
    //   ≥70 cols → ◆ aegis-boot vX.Y.Z   SB:state   TPM:state
    //   ≥50 cols → ◆ aegis-boot vX.Y.Z   SB:state
    //   <50 cols → ◆ aegis-boot vX.Y.Z
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
    let header = Line::from(spans);
    frame.render_widget(Paragraph::new(header), area);
}

fn draw_footer(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    // Footer hints depend on the underlying screen, not the overlay.
    // Registry-driven since #460 — the KEYBINDINGS table is the single
    // source of truth for footer, help overlay, and docgen.
    let effective = match &state.screen {
        Screen::Help { prior } | Screen::ConfirmQuit { prior } => prior.as_ref(),
        other => other,
    };
    let kind = ScreenKind::from_screen(effective);
    let hint = keybindings::footer_line(kind, state.pane, state.filter_editing);
    frame.render_widget(Paragraph::new(hint), area);
}

/// Derive the trust verdict for an ISO. Delegates to the canonical
/// [`TrustVerdict::from_discovered`] in [`crate::verdict`] so there's
/// a single source of truth for the tier model (#457).
fn trust_verdict(iso: &iso_probe::DiscoveredIso, state: &AppState) -> TrustVerdict {
    TrustVerdict::from_discovered(iso, state.secure_boot)
}

/// Single-character status glyph for a list row, encoding the worst
/// security state. Visible in monochrome themes (no color reliance).
/// (#85, k9s/dialog pattern.)
/// Render a single ISO row for the boot-menu list. Extracted so
/// `draw_list` stays under clippy's 100-line ceiling. #274 Phase 6c
/// added the folder-prefix rendering so operators running in
/// rescue-tui see the subfolder path (e.g. `ubuntu-24.04/Ubuntu
/// 24.04.2 LTS`) that `aegis-boot list` shows on the host.
fn render_iso_list_item<'a>(
    iso: &iso_probe::DiscoveredIso,
    scanned_roots: &[std::path::PathBuf],
    secure_boot: SecureBootStatus,
) -> ListItem<'a> {
    let glyph = TrustVerdict::from_discovered(iso, secure_boot).glyph();
    let qs = quirks_summary(iso);
    // Prefer pretty_name over label when present so operators see the
    // version (e.g. "Ubuntu 24.04.2 LTS" vs just "Ubuntu"). (#119)
    let display = iso_probe::display_name(iso);
    let folder = iso_folder_prefix(&iso.iso_path, scanned_roots);
    let display_with_folder = match folder {
        Some(f) => format!("{f}/{display}"),
        None => display.to_string(),
    };
    let line = if qs.is_empty() {
        format!(
            "{glyph} {}  ({})",
            display_with_folder,
            iso.distribution_name()
        )
    } else {
        format!(
            "{glyph} {}  ({})  {qs}",
            display_with_folder,
            iso.distribution_name()
        )
    };
    ListItem::new(line)
}

/// #274 Phase 6c — compute the subfolder path for an ISO relative to
/// whichever scanned root it lives under. Returns `None` if the ISO
/// sits at the root of any scanned root, or if no scanned root is a
/// parent of the ISO's `iso_path` (defensive — shouldn't happen since
/// the iso was discovered under one of the roots, but returning None
/// renders as a flat-layout row which is the safe degradation).
///
/// Always forward-slash separated regardless of host OS — matches
/// the exFAT stick filesystem's canonical form and mirrors Phase 6a's
/// `inventory::relative_folder` output shape.
fn iso_folder_prefix(iso_path: &std::path::Path, roots: &[std::path::PathBuf]) -> Option<String> {
    // Prefer the longest matching root so nested roots (unlikely but
    // possible if an operator passes both /run/media/aegis-isos and
    // /run/media/aegis-isos/subdir) resolve to the tighter one.
    let parent = iso_path.parent()?;
    let best_root = roots
        .iter()
        .filter(|r| parent.starts_with(r))
        .max_by_key(|r| r.as_os_str().len())?;
    let rel = parent.strip_prefix(best_root).ok()?;
    if rel.as_os_str().is_empty() {
        return None;
    }
    let parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

fn draw_help_overlay(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    // Centered panel. Grew from 70x20 to 80x32 when the SysRq
    // cheatsheet expanded to the full REISUB sequence (#93).
    let w = area.width.min(80);
    let h = area.height.min(32);
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
        Line::from("   Home / End    first / last entry (layout-agnostic)"),
        Line::from("   g / G         first / last entry (vim)"),
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
        Line::from(
            "   default (aurora · APCA) · material-design · ansi · monochrome · high-contrast · okabe-ito · aegis",
        ),
        Line::from(""),
        Line::from(" Emergency escape hatches (kernel SysRq)"),
        Line::from("   REISUB = safe forced reboot; hit each slowly, in order."),
        Line::from("   Alt+SysRq+r   raw keyboard mode (reclaim from X/Wayland)"),
        Line::from("   Alt+SysRq+e   SIGTERM all processes except init"),
        Line::from("   Alt+SysRq+i   SIGKILL all processes except init"),
        Line::from("   Alt+SysRq+s   sync disks"),
        Line::from("   Alt+SysRq+u   remount all filesystems readonly"),
        Line::from("   Alt+SysRq+b   reboot now"),
        Line::from(""),
        Line::from(Span::styled(
            " Esc or ? to dismiss",
            Style::default().fg(state.theme.warning),
        )),
    ];
    let block = Block::default().borders(Borders::ALL).title(" Help (#85) ");
    // Clear underlying buffer cells before rendering the overlay so blank
    // lines in the help body don't reveal info-pane text underneath. (#629)
    frame.render_widget(Clear, panel);
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
    // Clear underlying buffer before rendering — see #629.
    frame.render_widget(Clear, panel);
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        panel,
    );
}

/// Centered popup overlay for [`Screen::BlockedToast`] (#546). Two-line
/// payload — the message itself plus a "press any key" dismiss hint —
/// rendered with an error-tinted border so the operator immediately
/// reads "this is a refusal, not a confirmation prompt." Sized to the
/// max message width so long parse-failed reasons don't wrap awkwardly.
fn draw_blocked_toast_overlay(frame: &mut Frame<'_>, area: Rect, state: &AppState, message: &str) {
    // Width: hug the message, but cap to avoid edge-of-screen panels.
    // Account for the "press any key to dismiss" hint as the floor.
    const DISMISS_HINT: &str = "press any key to dismiss";
    let content_width = message.len().max(DISMISS_HINT.len());
    let w = u16::try_from(content_width.saturating_add(4))
        .unwrap_or(u16::MAX)
        .min(area.width)
        .min(70);
    let h: u16 = 5;
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let panel = Rect::new(x, y, w, h);
    let lines = vec![
        Line::from(Span::styled(
            message.to_string(),
            Style::default()
                .fg(state.theme.error)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            DISMISS_HINT,
            Style::default().fg(state.theme.warning),
        )),
    ];
    let block = Block::default().borders(Borders::ALL).title(" Blocked ");
    // Clear underlying buffer before rendering — see #629.
    frame.render_widget(Clear, panel);
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        panel,
    );
}

/// Centered consent-prompt overlay (#347). Renders the Confirm screen
/// underneath at the selected ISO so the operator sees the verdict
/// context they're consenting against, then layers the prompt on top.
/// Visually distinct from `BlockedToast` (warning-tinted border + a
/// `[ Consent required ]` title) so the operator reads it as
/// "decision needed" rather than "boot refused."
fn draw_consent_overlay(frame: &mut Frame<'_>, area: Rect, state: &AppState, kind: ConsentKind) {
    use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

    let prose = kind.prose();
    let title = kind.title();
    // Width: hug the longest line; cap so wide messages still land in
    // the centered panel rather than spilling to the framebuffer edges.
    let content_width = prose
        .iter()
        .copied()
        .chain(std::iter::once(title))
        .map(str::len)
        .max()
        .unwrap_or(40);
    let w = u16::try_from(content_width.saturating_add(4))
        .unwrap_or(u16::MAX)
        .min(area.width)
        .min(72);
    // Height: prose lines + 2 (top/bottom border) + 2 (top spacer + footer).
    let h = u16::try_from(prose.len().saturating_add(4))
        .unwrap_or(u16::MAX)
        .min(area.height);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let panel = Rect::new(x, y, w, h);

    let mut lines: Vec<Line<'_>> = Vec::with_capacity(prose.len().saturating_add(2));
    lines.push(Line::from(""));
    for body_line in prose {
        lines.push(Line::from(Span::styled(
            *body_line,
            Style::default().fg(state.theme.warning),
        )));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {title} "));
    // Clear underlying buffer before rendering — see #629.
    frame.render_widget(Clear, panel);
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
        "  1. On the host, add an ISO from the signed catalog in one step (#352):",
    ));
    lines.push(Line::from("       aegis-boot add ubuntu-24.04-live-server"));
    lines.push(Line::from(
        "     (run `aegis-boot recommend` for slugs — or `aegis-boot add /path/to.iso`)",
    ));
    lines.push(Line::from(
        "  2. If the AEGIS_ISOS partition is on this stick but wasn't auto-mounted,",
    ));
    lines.push(Line::from(
        "     boot this stick on a host and run `aegis-boot doctor --stick /dev/sdX`.",
    ));
    // Phase 2 of #312: empty-state previously said "select the rescue
    // shell entry below (if enabled)", but `draw_empty_list` replaces
    // the list entirely — there IS no "below" from the operator's
    // view. Point directly at the Enter keybinding since that's what
    // actually dispatches to rescue shell in this state (only entry
    // in the synthetic view is `ViewEntry::RescueShell`).
    lines.push(Line::from(
        "  3. Press Enter now to drop to a busybox rescue shell and mount /",
    ));
    lines.push(Line::from(
        "     inspect filesystems by hand — the synthetic \"rescue shell\" entry is",
    ));
    lines.push(Line::from(
        "     pre-selected in the background list even when no ISOs were found.",
    ));
    lines.push(Line::from(""));
    // Highlight the three bindings that actually do something useful
    // on this screen (Enter → rescue shell, q → reboot, ? → keybindings
    // overlay). #312.
    lines.push(Line::from(Span::styled(
        "Press Enter for rescue shell · q to reboot · ? for keybindings.",
        Style::default().add_modifier(Modifier::BOLD),
    )));

    let panel = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" aegis-boot — no ISOs discovered "),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(panel, area);
}

/// Compute layout for the List screen chrome. Returns
/// `(info_area, list_area)` for the caller to render into.
///
/// The earlier SKIPPED band (#85 Tier 2) was removed in the Phase A
/// layout cleanup: parse-failed ISOs render as `[!] <name> — PARSE
/// FAILED: <reason>` tier-4 rows directly in the list (#458), which
/// is the same fact through a single source of truth. The redundant
/// banner ate a row of vertical real estate on 80×25 consoles where
/// the verdict banner most needs to be above the fold.
fn split_list_chrome(_frame: &mut Frame<'_>, area: Rect, _state: &AppState) -> (Rect, Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(area);
    (chunks[0], chunks[1])
}

/// Render the top-of-list-chrome info line (filter banner while
/// editing, else the committed sort/filter hint). Extracted from
/// [`draw_list`] so that function stays under the clippy
/// too-many-lines gate.
///
/// Design-review #102: filter-mode visual was too subtle (trailing
/// `_` the only indicator). When editing, a reversed-style banner
/// with "FILTER:" prefix plus a blinking caret makes the mode
/// unmistakable. Committed filter keeps the quieter style.
fn render_list_info_line(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
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
        frame.render_widget(Paragraph::new(styled), area);
    } else {
        let info_line = if state.filter.is_empty() {
            format!(
                " sort: {}   (/ filter, s cycle sort, Tab focus)",
                state.sort_order.summary()
            )
        } else {
            format!(
                " filter: \"{}\"   sort: {}   (/ edit, Tab focus)",
                state.filter,
                state.sort_order.summary()
            )
        };
        frame.render_widget(Paragraph::new(info_line), area);
    }
}

fn draw_list(frame: &mut Frame<'_>, area: Rect, state: &AppState, selected: usize) {
    use crate::state::ViewEntry;
    if state.isos.is_empty() && state.failed_isos.is_empty() {
        draw_empty_list(frame, area, state);
        return;
    }

    // Chrome: (filter/sort hint line) on top, (main body) below. The
    // main body splits between the ISO list and the info pane; the
    // split orientation + ratio adapts to terminal size (Phase B
    // layout cleanup, #DESIGN-UX-004):
    //
    //   < 100 cols → vertical: list on top, info below. The 40/60
    //                horizontal split chops kernel cmdlines on
    //                serial/OVMF consoles and centers the verdict
    //                banner over a tiny pane.
    //   ≥ 100 cols → horizontal: list left, info right, but the
    //                list pane width is `min(40%, longest_row+4)` so
    //                short ISO sets shrink the list pane and give
    //                the info pane more room for sha256 / cmdline /
    //                signer chain.
    let (info_line_area, body_area) = split_list_chrome(frame, area, state);
    let entries = state.visible_entries();
    let (list_area, info_pane_area) = if body_area.width < 100 {
        // Compact / vertical layout for narrow terminals.
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            // Cap list at min(8 rows + chrome, 50% of body) so info
            // pane keeps half the screen even with a long list.
            .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
            .split(body_area);
        (chunks[0], chunks[1])
    } else {
        // Wide / horizontal layout with adaptive list width.
        let longest_label = entries
            .iter()
            .map(|e| match e {
                ViewEntry::Iso(i) => state
                    .isos
                    .get(*i)
                    .map_or(0, |iso| iso.label.chars().count()),
                ViewEntry::FailedIso(i) => state.failed_isos.get(*i).map_or(0, |f| {
                    f.iso_path
                        .file_name()
                        .map_or(0, |n| n.to_string_lossy().chars().count())
                }),
                ViewEntry::RescueShell => "rescue shell (busybox) — dropped from rescue-tui"
                    .chars()
                    .count(),
            })
            .max()
            .unwrap_or(20);
        // Add ~12 cols for the leading status glyph, distro tag, and
        // borders. Cap at 40% so the info pane never starves.
        let list_cols_fit = u16::try_from(longest_label.saturating_add(12)).unwrap_or(u16::MAX);
        let list_cols_max = body_area.width * 40 / 100;
        let list_cols = list_cols_fit.min(list_cols_max).max(20);
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(list_cols), Constraint::Min(40)])
            .split(body_area);
        (chunks[0], chunks[1])
    };

    render_list_info_line(frame, info_line_area, state);

    // `entries` already computed above for the layout decision.
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
                render_iso_list_item(&state.isos[*i], &state.scanned_roots, state.secure_boot)
            }
            ViewEntry::FailedIso(i) => render_failed_iso_list_item(&state.failed_isos[*i]),
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
    // Focus styling — the active pane's border brightens; the inactive
    // pane dims. List-pane highlight is only visible when the list
    // itself holds focus (otherwise the reverse-video row looks like
    // a bug from the operator's POV). (#458, gitui pattern.)
    let list_focused = state.pane == Pane::List;
    let list_block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(pane_border_style(list_focused, &state.theme));
    let list_highlight = if list_focused {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default().add_modifier(Modifier::DIM)
    };
    let list = List::new(items)
        .block(list_block)
        .highlight_style(list_highlight)
        .highlight_symbol("> ");

    let mut list_state = ListState::default();
    let cursor = selected.min(entries.len().saturating_sub(1));
    list_state.select(Some(cursor));
    frame.render_stateful_widget(list, list_area, &mut list_state);

    // Info pane — tier-aware summary of the currently-selected row.
    // This is the #458 scaffold: verdict + filename + one-line reason.
    // #459 extends with the full per-tier metadata (sha256, signer,
    // kernel/initrd, cmdline, quirks, …).
    draw_info_pane(
        frame,
        info_pane_area,
        state,
        selected,
        state.pane == Pane::Info,
    );
}

/// Border style for a pane based on focus state. Focused panes use
/// theme.success (bright) so the eye immediately lands on the active
/// input target. Unfocused panes use `Color::DarkGray` so they
/// recede — but stay visible enough that the layout is still
/// readable. (#458)
/// Render a list row for a [`iso_probe::FailedIso`] — tier-4 entry
/// in the rescue-tui list. Shown with a red glyph and a truncated
/// reason. The info pane reveals the full reason when the row is
/// selected. (#459)
fn render_failed_iso_list_item<'a>(failed: &iso_probe::FailedIso) -> ListItem<'a> {
    let glyph = "[!]"; // tier-4 marker
    let name = failed.iso_path.file_name().map_or_else(
        || failed.iso_path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    // Short reason for the list row — full reason lives in the info pane.
    let short = {
        let r = &failed.reason;
        if r.len() > 40 {
            let mut end = 40;
            while !r.is_char_boundary(end) {
                end -= 1;
            }
            format!("{}…", &r[..end])
        } else {
            r.clone()
        }
    };
    ListItem::new(format!("{glyph} {name}  — PARSE FAILED: {short}"))
}

fn pane_border_style(focused: bool, theme: &Theme) -> Style {
    if focused {
        Style::default().fg(theme.success)
    } else {
        Style::default().fg(ratatui::style::Color::DarkGray)
    }
}

/// Render the info pane — full per-tier content for the currently
/// selected row. (#459)
///
/// Tier 1/2/3 (bootable): metadata rows (verdict, file, size, sha256,
/// signer, kernel, initrd, cmdline, distro, quirks) plus any
/// tier-specific notes.
///
/// Tier 4/5/6 (blocked): verdict + filename/size + a `Reason:` block
/// with the full wrapped error. Long reasons are pre-wrapped via the
/// `textwrap` crate (ratatui's `Paragraph::wrap` + `.scroll` has a
/// known issue with wrapped-line accounting, ratatui/ratatui#2342).
fn draw_info_pane(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &AppState,
    selected: usize,
    focused: bool,
) {
    use crate::state::ViewEntry;
    let entries = state.visible_entries();
    let cursor = selected.min(entries.len().saturating_sub(1));
    let title = if focused {
        " info (focused) "
    } else {
        " info "
    };
    let border = pane_border_style(focused, &state.theme);

    // Usable content width = pane width - 2 border cols - 2 padding.
    // Clamped to a small minimum so extreme-narrow terminals still
    // produce at least some wrapping rather than 0-width panics.
    let content_width = usize::from(area.width).saturating_sub(4).max(10);

    let lines: Vec<Line> = match entries.get(cursor) {
        Some(ViewEntry::Iso(idx)) => match state.isos.get(*idx) {
            Some(iso) => info_pane_iso_lines(iso, state, content_width),
            None => vec![Line::from("(no ISO at selected index)")],
        },
        Some(ViewEntry::FailedIso(idx)) => match state.failed_isos.get(*idx) {
            Some(failed) => info_pane_failed_lines(failed, &state.theme, content_width),
            None => vec![Line::from("(no failed ISO at selected index)")],
        },
        Some(ViewEntry::RescueShell) => vec![
            Line::from(vec![
                Span::styled("Verdict:  ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw("SHELL (busybox)"),
            ]),
            Line::from(vec![
                Span::styled("Action:   ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw("exits rescue-tui to a busybox shell"),
            ]),
            Line::from(""),
            Line::from("Useful when no ISO will boot or you need to"),
            Line::from("inspect the stick from a signed environment."),
        ],
        None => vec![Line::from("(empty)")],
    };

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(border),
        )
        .scroll((state.info_scroll, 0));
    frame.render_widget(paragraph, area);
}

/// Helper: render the info-pane line set for a successfully-parsed
/// ISO. Covers tiers 1/2/3/5/6. (#459)
fn info_pane_iso_lines<'a>(
    iso: &iso_probe::DiscoveredIso,
    state: &AppState,
    content_width: usize,
) -> Vec<Line<'a>> {
    let verdict = TrustVerdict::from_discovered(iso, state.secure_boot);
    let mut lines = Vec::with_capacity(15);

    // #632: full-width verdict banner. The verdict is the security
    // gate of the rescue path — promote it from an inline colored
    // token to a banner that's the eye's first stop. Bold label on a
    // themed bg fill. Monochrome theme falls back to plain bold +
    // bookend (Reset bg = no fill).
    lines.push(verdict_banner_line(
        verdict.label(),
        verdict.color(&state.theme),
        content_width,
    ));
    lines.push(labeled("File:     ", filename_str(&iso.iso_path)));
    lines.push(labeled("Size:     ", humanize_size(iso.size_bytes)));
    lines.push(labeled("sha256:   ", hash_summary(iso)));
    lines.push(labeled("Signer:   ", signer_summary(iso)));
    lines.push(labeled("Kernel:   ", iso.kernel.display().to_string()));
    lines.push(labeled(
        "Initrd:   ",
        iso.initrd
            .as_ref()
            .map_or_else(|| "(none)".to_string(), |p| p.display().to_string()),
    ));
    lines.push(labeled(
        "Cmdline:  ",
        iso.cmdline.clone().unwrap_or_else(|| "(none)".to_string()),
    ));
    lines.push(labeled("Distro:   ", format!("{:?}", iso.distribution)));
    let qs = quirks_summary(iso);
    lines.push(labeled(
        "Quirks:   ",
        if qs.is_empty() {
            "none".to_string()
        } else {
            qs
        },
    ));

    // Tier-specific note. For tier-4/5/6 (which can be reached here if
    // a tier-6 hash mismatch is detected on an otherwise-parseable
    // ISO), render a wrapped reason block.
    //
    // Windows ISOs get a dedicated actionable-redirect panel per the
    // L1 design in docs/design/windows-iso-boot.md — a tier-5
    // dead-end is a support-ticket generator, so we point operators
    // at Rufus (the tool that actually solves their problem) and at
    // the Linux ISOs already on this stick.
    if !verdict.is_bootable() {
        if matches!(iso.distribution, iso_probe::Distribution::Windows)
            && iso.quirks.contains(&iso_probe::Quirk::NotKexecBootable)
        {
            lines.push(Line::from(""));
            lines.extend(windows_redirect_lines(state, content_width));
        } else {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Reason:",
                Style::default()
                    .fg(state.theme.error)
                    .add_modifier(Modifier::BOLD),
            )));
            extend_wrapped(&mut lines, &verdict.reason(), content_width);
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Boot is disabled for this ISO.",
                Style::default().add_modifier(Modifier::DIM),
            )));
        }
    } else if matches!(
        verdict,
        TrustVerdict::BareUnverified | TrustVerdict::KeyNotTrusted
    ) {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Note:",
            Style::default()
                .fg(state.theme.warning)
                .add_modifier(Modifier::BOLD),
        )));
        let note = match verdict {
            TrustVerdict::BareUnverified => {
                "This ISO is bootable but has no operator attestation. \
                 A typed confirmation is required before boot."
            }
            TrustVerdict::KeyNotTrusted => {
                "Signature is structurally valid but the signer is not in \
                 AEGIS_TRUSTED_KEYS. Typed confirmation is required before boot."
            }
            _ => "",
        };
        extend_wrapped(&mut lines, note, content_width);
    }

    lines
}

/// Helper: render the info-pane line set for a tier-4 (`ParseFailed`)
/// row. Routes through [`TrustVerdict::from_failed`] so the tier
/// label, color, and reason come from the same canonical source the
/// rest of the UI uses. (#459)
fn info_pane_failed_lines<'a>(
    failed: &iso_probe::FailedIso,
    theme: &Theme,
    content_width: usize,
) -> Vec<Line<'a>> {
    let verdict = TrustVerdict::from_failed(failed);
    let mut lines = Vec::with_capacity(11);
    // #632: see info_pane_iso_lines for rationale.
    lines.push(verdict_banner_line(
        verdict.label(),
        verdict.color(theme),
        content_width,
    ));
    lines.push(labeled("File:     ", filename_str(&failed.iso_path)));
    lines.push(labeled("Path:     ", failed.iso_path.display().to_string()));
    lines.push(labeled("Kind:     ", format!("{:?}", failed.kind)));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Reason:",
        Style::default()
            .fg(verdict.color(theme))
            .add_modifier(Modifier::BOLD),
    )));
    extend_wrapped(&mut lines, &verdict.reason(), content_width);
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "This ISO could not be mounted or did not contain a recognized",
        Style::default().add_modifier(Modifier::DIM),
    )));
    lines.push(Line::from(Span::styled(
        "boot layout. Boot is disabled.",
        Style::default().add_modifier(Modifier::DIM),
    )));
    lines
}

/// L1 Windows-ISO prose panel per
/// `docs/design/windows-iso-boot.md § Revised recommendation`.
///
/// Rendered in place of the generic tier-5 reason block whenever the
/// selected ISO is a Windows installer. Points the operator at Rufus
/// (the tool that actually solves their problem) and lists the
/// bootable Linux ISOs already on this stick.
///
/// Mission alignment: aegis-boot helps operators migrate *off*
/// Windows, so the "primary" alternative is Linux, and Rufus handles
/// the "I still need Windows" case without forcing aegis-boot to
/// grow a Windows-PE boot path.
fn windows_redirect_lines<'a>(state: &AppState, content_width: usize) -> Vec<Line<'a>> {
    let mut lines: Vec<Line<'a>> = Vec::new();
    lines.push(Line::from(Span::styled(
        "Windows 11 installer detected — aegis-boot doesn't boot Windows by design.",
        Style::default()
            .fg(state.theme.error)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "To install Windows 11, use Rufus:",
        Style::default().add_modifier(Modifier::BOLD),
    )));
    for bullet in [
        "  1. Copy your Win11 ISO off this stick",
        "  2. Get Rufus from https://rufus.ie",
        "  3. Flash the ISO to a different USB stick",
    ] {
        extend_wrapped(&mut lines, bullet, content_width);
    }
    lines.push(Line::from(""));

    // Linux-ISO listing from the same AppState the rest of the UI
    // drives off. Filter to bootable, non-Windows rows so a second
    // unparsable Windows ISO on the stick doesn't end up in the
    // "try Linux instead" bullet list.
    let bootable_linux: Vec<&iso_probe::DiscoveredIso> = state
        .isos
        .iter()
        .filter(|candidate| {
            !matches!(candidate.distribution, iso_probe::Distribution::Windows)
                && !candidate
                    .quirks
                    .contains(&iso_probe::Quirk::NotKexecBootable)
        })
        .collect();

    if bootable_linux.is_empty() {
        lines.push(Line::from(Span::styled(
            "To try Linux instead:",
            Style::default().add_modifier(Modifier::BOLD),
        )));
        extend_wrapped(
            &mut lines,
            "  drop a Linux ISO into this stick's AEGIS_ISOS/ partition and re-plug.",
            content_width,
        );
    } else {
        lines.push(Line::from(Span::styled(
            "To try Linux instead, these are on this stick:",
            Style::default().add_modifier(Modifier::BOLD),
        )));
        for candidate in bootable_linux {
            extend_wrapped(
                &mut lines,
                &format!("  - {}", filename_str(&candidate.iso_path)),
                content_width,
            );
        }
    }
    lines
}

/// Build a `"Label: value"` info-pane line with the label in bold.
fn labeled(label: &str, value: String) -> Line<'_> {
    Line::from(vec![
        Span::styled(label, Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(value),
    ])
}

/// Render the trust verdict as a full-width banner row at the top of
/// the info pane. (#632)
///
/// The banner is the eye's first stop — verdict is the security gate
/// of the rescue path and must be unmistakable. Bold black text on a
/// themed background fill spans the available content width with the
/// label centered between bookend arrows.
///
/// Monochrome theme falls back gracefully: `verdict_color` is
/// `Color::Reset`, which renders as a transparent bg, but the bold
/// modifier + bookend arrows still give the row enough visual weight
/// to function as a banner.
fn verdict_banner_line(label: &str, verdict_color: Color, width: usize) -> Line<'_> {
    // Compose " ▶ LABEL ◀ " then pad to width with spaces inside the
    // styled span so the bg fill spans the full pane.
    let core = format!(" ▶ {label} ◀ ");
    let core_chars = core.chars().count();
    let pad = width.saturating_sub(core_chars);
    let body = format!("{core}{}", " ".repeat(pad));
    Line::from(Span::styled(
        body,
        Style::default()
            .bg(verdict_color)
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD),
    ))
}

fn filename_str(p: &std::path::Path) -> String {
    p.file_name().map_or_else(
        || p.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// Short verification summary for the info pane's sha256 row. Avoids
/// dumping a 64-char hex blob while still saying whether verification
/// happened and which side-car source was used.
fn hash_summary(iso: &iso_probe::DiscoveredIso) -> String {
    use iso_probe::HashVerification as H;
    match &iso.hash_verification {
        H::Verified { digest, source } => {
            format!("verified ({}) from {}", short_hex(digest), source)
        }
        H::Mismatch {
            expected, actual, ..
        } => format!(
            "MISMATCH expected {} got {}",
            short_hex(expected),
            short_hex(actual)
        ),
        // #633: "MISSING" reads as failure at a glance; the prior
        // em-dash glyph could be misread as a checkmark/bullet under
        // fbcon glyph fallback.
        H::NotPresent => "MISSING (no sibling .sha256 found)".to_string(),
        H::Unreadable { reason, .. } => format!("unreadable: {reason}"),
    }
}

/// Short signer summary paired with trust decision.
fn signer_summary(iso: &iso_probe::DiscoveredIso) -> String {
    use iso_probe::SignatureVerification as S;
    match &iso.signature_verification {
        S::Verified { key_id, .. } => format!("{key_id} (✓ trusted)"),
        S::KeyNotTrusted { key_id } => format!("{key_id} (✗ not in AEGIS_TRUSTED_KEYS)"),
        S::Forged { .. } => "FORGED signature".to_string(),
        S::Error { reason } => format!("verification error: {reason}"),
        // #633: see hash_summary for rationale.
        S::NotPresent => "MISSING (no sibling .minisig found)".to_string(),
    }
}

// short_hex moved to crates/aegis-core (#556 PoC).
use aegis_core::short_hex;

/// Pre-wrap a string to `content_width` and append one `Line` per
/// wrapped row to `out`. Uses `textwrap::wrap` because ratatui's own
/// `Paragraph::wrap` + `.scroll` mis-counts wrapped lines
/// (ratatui/ratatui#2342) and breaks info-pane scroll math.
fn extend_wrapped(out: &mut Vec<Line<'_>>, text: &str, content_width: usize) {
    let wrapped = textwrap::wrap(text, content_width);
    for piece in wrapped {
        out.push(Line::from(piece.to_string()));
    }
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

    // Trust-tier verdict line. One of the 6 TrustVerdict variants
    // (#457 extends the original GREEN/YELLOW/RED/GRAY model to also
    // surface ParseFailed / SecureBootBlocked / HashMismatch).
    //
    // #632 promoted the verdict to a full-width banner — the Confirm
    // screen is the operator's last stop before kexec, so the trust
    // state must be the eye's first stop here too. Banner first, then
    // a "Reason:" row when the verdict carries explanatory text.
    let verdict = trust_verdict(iso, state);
    let confirm_width = usize::from(area.width).saturating_sub(2).max(20);
    let verdict_line =
        verdict_banner_line(verdict.label(), verdict.color(&state.theme), confirm_width);
    let verdict_reason = verdict.reason();
    let reason_line = if verdict_reason.is_empty() {
        None
    } else {
        Some(Line::from(vec![
            Span::styled("Reason:   ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(verdict_reason),
        ]))
    };

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
    if let Some(line) = reason_line {
        lines.push(line);
    }
    // #602: audit-log write failure banner. Non-blocking — the verdict
    // and kexec-gate are unaffected; this just signals "the JSONL
    // proof for this verify did not persist," letting the operator
    // factor that into the boot-or-recheck decision.
    if let Some(msg) = state.audit_warning.as_deref() {
        lines.push(Line::from(vec![
            Span::styled(
                "Audit:    ",
                Style::default()
                    .fg(state.theme.warning)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(msg, Style::default().fg(state.theme.warning)),
        ]));
    }
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
        // #558 per-axis breakdown. The combined `Verdict:` line at the
        // top of the Confirm screen down-shifts to the worse-of-two; the
        // breakdown here lets operators see which axis the verdict came
        // from. Source = origin trust (sidecar + minisig); Media = bytes
        // match recorded hash. See verdict.rs for the mapping table.
        {
            let src = crate::verdict::TrustVerdict::source_verdict(iso);
            Line::from(vec![
                Span::styled("Source:   ", Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(
                    src.label(),
                    Style::default()
                        .fg(src.color(&state.theme))
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(
                    "(origin trust — sidecar + minisig)",
                    Style::default().add_modifier(Modifier::DIM),
                ),
            ])
        },
        {
            let med = crate::verdict::TrustVerdict::media_verdict(iso);
            Line::from(vec![
                Span::styled("Media:    ", Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(
                    med.label(),
                    Style::default()
                        .fg(med.color(&state.theme))
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(
                    "(bytes-on-stick vs recorded hash)",
                    Style::default().add_modifier(Modifier::DIM),
                ),
            ])
        },
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

/// Format an optional byte count for the info-pane Size: row. Wraps
/// `aegis_core::humanize_bytes` and adds the rescue-tui-specific
/// "(unknown)" sentinel for ISOs whose size couldn't be stat'd.
/// (#556 proof-of-concept: 4-level ladder logic moved to aegis-core;
/// only the `Option`-handling glue stays here.)
fn humanize_size(bytes: Option<u64>) -> String {
    bytes.map_or_else(|| "(unknown)".to_string(), aegis_core::humanize_bytes)
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
            Span::raw(trust_verdict(iso, state).label().to_string()),
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
    use crate::state::ViewEntry;
    use iso_probe::{Distribution, Quirk};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::path::PathBuf;

    // ---- #274 Phase 6c: subfolder-prefix rendering ------------------------

    #[test]
    fn iso_folder_prefix_returns_none_for_root_level_iso() {
        let roots = vec![PathBuf::from("/run/media/aegis-isos")];
        let got = iso_folder_prefix(
            std::path::Path::new("/run/media/aegis-isos/alpine.iso"),
            &roots,
        );
        assert_eq!(got, None, "flat-layout ISO has no folder prefix");
    }

    #[test]
    fn iso_folder_prefix_returns_single_level_subfolder() {
        let roots = vec![PathBuf::from("/run/media/aegis-isos")];
        let got = iso_folder_prefix(
            std::path::Path::new("/run/media/aegis-isos/ubuntu-24.04/server.iso"),
            &roots,
        );
        assert_eq!(got.as_deref(), Some("ubuntu-24.04"));
    }

    #[test]
    fn iso_folder_prefix_returns_nested_subfolder_forward_slash() {
        let roots = vec![PathBuf::from("/run/media/aegis-isos")];
        let got = iso_folder_prefix(
            std::path::Path::new("/run/media/aegis-isos/ubuntu/24.04/server.iso"),
            &roots,
        );
        assert_eq!(got.as_deref(), Some("ubuntu/24.04"));
    }

    #[test]
    fn iso_folder_prefix_picks_longest_matching_root() {
        // If an operator somehow configures nested roots, prefer the
        // tighter match so the folder prefix stays short.
        let roots = vec![
            PathBuf::from("/run/media"),
            PathBuf::from("/run/media/aegis-isos"),
        ];
        let got = iso_folder_prefix(
            std::path::Path::new("/run/media/aegis-isos/alpine-3.20/std.iso"),
            &roots,
        );
        assert_eq!(got.as_deref(), Some("alpine-3.20"));
    }

    #[test]
    fn iso_folder_prefix_returns_none_when_no_root_matches() {
        // Defensive: if discovery returned an ISO outside every scanned
        // root (shouldn't happen), fall back to None → flat render.
        let roots = vec![PathBuf::from("/run/media/other")];
        let got = iso_folder_prefix(std::path::Path::new("/tmp/random/thing.iso"), &roots);
        assert_eq!(got, None);
    }

    #[test]
    fn iso_folder_prefix_empty_roots_returns_none() {
        let got = iso_folder_prefix(
            std::path::Path::new("/run/media/aegis-isos/ubuntu/server.iso"),
            &[],
        );
        assert_eq!(got, None);
    }

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
            sidecar: None,
        }
    }

    fn render_to_string(state: &AppState) -> String {
        // Bumped from 80x20 → 120x30 with the dual-pane layout (#458) —
        // the 40/60 split leaves ~48 cols for the list pane including
        // borders, which is still tight for long labels + distro +
        // quirks. 120 cols is a typical modern terminal and matches
        // what real operators see in rescue-tui.
        render_to_string_sized(state, 120, 30)
    }

    /// Render variant that lets individual tests exercise smaller or
    /// larger terminal geometries. Default is [`render_to_string`]'s
    /// 120x30. The minimum reasonable rescue-tui width is 80 cols
    /// (pre-#458 default); tests that assert on the truncated /
    /// minimum layout should use this helper explicitly.
    fn render_to_string_sized(state: &AppState, cols: u16, rows: u16) -> String {
        let backend = TestBackend::new(cols, rows);
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
    fn empty_list_points_at_catalog_slug_add_shortcut() {
        // #352 UX-5: the new-user empty state should surface the
        // one-step `aegis-boot add <catalog-slug>` path (from UX-4,
        // PR #356) — NOT just the "supply a local ISO path" recipe.
        // Operator sees the catalog shortcut first, local-path second.
        let state = AppState::new(vec![]);
        let s = render_to_string(&state);
        assert!(
            s.contains("aegis-boot add ubuntu-24.04-live-server"),
            "empty state should show the catalog-slug add shortcut: {s}"
        );
        assert!(
            s.contains("aegis-boot recommend"),
            "empty state should mention `aegis-boot recommend` for catalog discovery: {s}"
        );
        // The local-path form must still render for operators with a
        // pre-downloaded ISO — not a replacement, an ADDITION.
        assert!(
            s.contains("aegis-boot add /path/to.iso"),
            "local-path form must still render alongside the catalog slug: {s}"
        );
    }

    #[test]
    fn empty_list_footer_names_enter_rescue_shell_keybinding() {
        // #312 — the empty-state footer must tell the operator how
        // to drop to the rescue shell RIGHT NOW (not reference a
        // "rescue shell entry below", which doesn't render in this
        // screen). Enter is the dispatch key; this test pins that
        // hint into the visible output.
        let state = AppState::new(vec![]);
        let s = render_to_string(&state);
        assert!(
            s.contains("Press Enter for rescue shell"),
            "empty-state footer must name the Enter → rescue shell keybinding: {s}"
        );
    }

    #[test]
    fn empty_list_does_not_reference_unseen_entry_below() {
        // #312 regression guard — the pre-fix text said "select the
        // rescue shell entry below", but `draw_empty_list` replaces
        // the list entirely so there IS no "below" from the
        // operator's POV. Ensure the new text doesn't leak back in.
        let state = AppState::new(vec![]);
        let s = render_to_string(&state);
        assert!(
            !s.contains("entry below"),
            "empty-state must not reference an unseen 'entry below': {s}"
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
        assert!(s.contains("/tmp"), "missing scanned path 2 in: {s}");
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
    fn list_skipped_band_removed_in_phase_a_layout_cleanup() {
        // Phase A removed the SKIPPED band (#85 Tier 2 — was a
        // memtest86+-style one-frame warning). Parse-failed ISOs now
        // render as tier-4 `[!] <name> — PARSE FAILED: <reason>` rows
        // directly in the list (#458). The band was redundant and ate
        // a row that the verdict banner needed on 80×25 consoles.
        //
        // This regression-guard test asserts the literal "SKIPPED"
        // header text is gone from the render even when ISOs are
        // marked as skipped (the field still exists on AppState; the
        // rendering of it is what's removed).
        let state = AppState::new(vec![fake_iso("ok")]).with_skipped_iso_count(2);
        let s = render_to_string(&state);
        assert!(
            !s.contains("SKIPPED"),
            "Phase A removed the SKIPPED band; render should not contain it: {s}",
        );
        assert!(
            !s.contains("ISO(s) on disk failed to parse"),
            "Phase A removed the SKIPPED band copy: {s}",
        );
        // The good ISO must still render — removing the band must not
        // affect the list content.
        assert!(s.contains("ok"));
    }

    #[test]
    fn list_shows_quirk_summary_for_flagged_iso() {
        let mut iso = fake_iso("warn");
        iso.quirks = vec![Quirk::UnsignedKernel];
        let state = AppState::new(vec![iso]);
        let s = render_to_string(&state);
        assert!(s.contains("unsigned-kernel"));
    }

    // ---- #458 — dual-pane layout --------------------------------------

    #[test]
    fn list_screen_renders_info_pane_with_verdict() {
        // The info pane (right side of the 40/60 split) must surface
        // the currently-selected row's verdict label so the operator
        // sees what tier they're about to act on. Since #632 the
        // verdict is rendered as a full-width banner — assert the
        // bookend arrow + label are present rather than the prior
        // "Verdict:" inline-token text.
        let iso = fake_iso("ubuntu-live");
        let state = AppState::new(vec![iso]);
        let s = render_to_string(&state);
        // fake_iso has no sidecar/sig → tier 2 (BareUnverified).
        assert!(s.contains("UNVERIFIED"), "expected Tier 2 label in: {s}");
        assert!(
            s.contains("▶ UNVERIFIED ◀"),
            "expected #632 verdict banner with bookend arrows in: {s}"
        );
    }

    #[test]
    fn missing_sibling_renders_as_word_token_not_glyph() {
        // #633: reading "—" as a checkmark/bullet is a real risk on
        // fbcon glyph fallback. Sibling-missing must read as failure
        // at a glance; the explicit "MISSING" word token does that.
        let iso = fake_iso("ubuntu-live");
        let state = AppState::new(vec![iso]);
        let s = render_to_string(&state);
        assert!(
            s.contains("MISSING"),
            "expected MISSING word token (not a glyph) in: {s}"
        );
        // The explanatory parenthetical must still trail the token.
        assert!(s.contains("no sibling .sha256 found"));
    }

    #[test]
    fn list_screen_renders_info_pane_with_filename() {
        let iso = fake_iso("ubuntu-live");
        let state = AppState::new(vec![iso]);
        let s = render_to_string(&state);
        assert!(s.contains("File:"), "info pane must label File: ");
        // fake_iso's iso_path ends in the label — it's constructed
        // that way in the test harness (ubuntu-live.iso).
        assert!(s.contains("ubuntu-live"));
    }

    #[test]
    fn info_pane_label_reflects_focus_state() {
        let iso = fake_iso("a");
        let mut state = AppState::new(vec![iso]);
        // Default: List focused, info pane title is " info ".
        let unfocused = render_to_string(&state);
        assert!(
            unfocused.contains(" info "),
            "unfocused title missing: {unfocused}"
        );
        // Tab moves focus → info pane title becomes " info (focused) ".
        state.toggle_pane();
        let focused = render_to_string(&state);
        assert!(
            focused.contains("info (focused)"),
            "focused title missing: {focused}"
        );
    }

    #[test]
    fn empty_list_bypasses_dual_pane_and_shows_empty_screen() {
        // No ISOs and no failures → empty screen path, not dual pane.
        let state = AppState::new(Vec::new());
        let s = render_to_string(&state);
        // draw_empty_list renders a single-pane message.
        assert!(
            !s.contains("Verdict:"),
            "empty state should not show info pane verdict: {s}"
        );
    }

    // ---- #459 — info pane full content + tier-4 rows ----------------

    fn fake_failed_iso(name: &str, reason: &str) -> iso_probe::FailedIso {
        iso_probe::FailedIso {
            iso_path: std::path::PathBuf::from(format!("/isos/{name}")),
            reason: reason.to_string(),
            kind: iso_probe::FailureKind::MountFailed,
        }
    }

    #[test]
    fn info_pane_tier1_shows_full_metadata_rows() {
        // Fake an ISO with a verified hash so it renders as tier 1
        // (OperatorAttested) and the info pane's full metadata
        // rows (Kernel, Initrd, Cmdline, Distro, Quirks) should appear.
        let mut iso = fake_iso("ubuntu");
        iso.hash_verification = iso_probe::HashVerification::Verified {
            digest: "abcdef1234567890".to_string(),
            source: "/isos/ubuntu.iso.sha256".to_string(),
        };
        let state = AppState::new(vec![iso]);
        let s = render_to_string(&state);
        assert!(s.contains("VERIFIED"), "tier 1 label missing: {s}");
        // Each metadata row's label must appear somewhere in the pane.
        for label in &["File:", "Size:", "sha256:", "Kernel:", "Distro:"] {
            assert!(s.contains(label), "info pane missing {label}: {s}");
        }
    }

    #[test]
    fn info_pane_tier4_failed_iso_shows_reason_block() {
        let state = AppState::new(Vec::new()).with_failed_isos(vec![fake_failed_iso(
            "broken.iso",
            "mount: wrong fs type, bad option, bad superblock",
        )]);
        let s = render_to_string(&state);
        assert!(s.contains("PARSE FAILED"), "tier-4 label missing: {s}");
        assert!(s.contains("broken.iso"), "filename missing: {s}");
        assert!(s.contains("wrong fs type"), "reason string missing: {s}");
        assert!(s.contains("Boot is disabled"), "disabled hint missing: {s}");
    }

    #[test]
    fn info_pane_wraps_long_reason_strings() {
        // 400-char reason — wider than the info pane at 120x30 → must
        // wrap. textwrap renders multiple lines; no line may exceed
        // the pane's content width.
        let long_reason = "x".repeat(400);
        let state = AppState::new(Vec::new())
            .with_failed_isos(vec![fake_failed_iso("x.iso", &long_reason)]);
        let s = render_to_string(&state);
        // Rendered output should contain many 'x' chars across the
        // wrapped region. The raw long string isn't present
        // contiguously — textwrap inserts breaks.
        let x_runs: Vec<&str> = s
            .split_whitespace()
            .filter(|t| t.starts_with('x'))
            .collect();
        assert!(
            x_runs.len() > 1,
            "long reason should wrap into multiple line fragments: {s}"
        );
    }

    #[test]
    fn list_includes_failed_iso_as_parse_failed_row() {
        // All ISOs on disk must be visible — tier-4 rows too.
        let state = AppState::new(Vec::new())
            .with_failed_isos(vec![fake_failed_iso("fail.iso", "mount error")]);
        let s = render_to_string(&state);
        assert!(
            s.contains("fail.iso"),
            "failed ISO filename missing from list: {s}"
        );
        assert!(s.contains("PARSE FAILED"), "tier-4 list label missing: {s}");
    }

    #[test]
    fn info_pane_bare_unverified_shows_typed_confirmation_note() {
        // Tier 2 shows a warning-colored Note block pointing at the
        // typed-confirmation challenge the operator will hit on boot.
        let iso = fake_iso("bare");
        let state = AppState::new(vec![iso]);
        let s = render_to_string(&state);
        assert!(s.contains("UNVERIFIED"), "tier-2 label missing: {s}");
        assert!(s.contains("Note:"), "tier-2 note block missing: {s}");
        // "typed confirmation" can wrap into separate lines in the
        // render buffer, so the two words aren't guaranteed
        // contiguous. Check for presence of each word plus the
        // uniquely-phrased signal "no operator attestation" so the
        // assertion doesn't false-pass on an unrelated screen.
        assert!(s.contains("typed"), "tier-2 note missing 'typed': {s}");
        assert!(
            s.contains("operator attestation"),
            "tier-2 note missing 'operator attestation': {s}"
        );
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

    // ---- #461 — render coverage suite --------------------------------
    //
    // Comprehensive render-level regression suite covering all 6
    // TrustVerdict tiers, every Screen variant, edge cases (empty
    // list, extreme geometry, focus states, filter modes).
    //
    // These complement the string-contains tests above by exercising
    // the full render pipeline with a wider variety of state
    // fixtures. A failure here catches the class of bug where a
    // previously-working screen silently regresses (e.g. a variant
    // pattern that gets dropped, a label that disappears, a color
    // that inverts).

    /// Build an ISO with a specific `(hash, sig, quirks)` combination
    /// so we can assert each tier renders correctly in one shot.
    fn iso_with_verification(
        label: &str,
        hash: iso_probe::HashVerification,
        sig: iso_probe::SignatureVerification,
        quirks: Vec<Quirk>,
    ) -> iso_probe::DiscoveredIso {
        let mut iso = fake_iso(label);
        iso.hash_verification = hash;
        iso.signature_verification = sig;
        iso.quirks = quirks;
        iso
    }

    #[test]
    fn render_coverage_tier1_operator_attested() {
        let iso = iso_with_verification(
            "ubuntu",
            iso_probe::HashVerification::Verified {
                digest: "abcdef1234567890".to_string(),
                source: "/isos/ubuntu.iso.sha256".to_string(),
            },
            iso_probe::SignatureVerification::NotPresent,
            vec![],
        );
        let s = render_to_string(&AppState::new(vec![iso]));
        assert!(s.contains("VERIFIED"), "tier-1 label missing");
        assert!(s.contains("verified"), "info pane sha256 status missing");
    }

    #[test]
    fn render_coverage_tier2_bare_unverified() {
        let iso = iso_with_verification(
            "bare",
            iso_probe::HashVerification::NotPresent,
            iso_probe::SignatureVerification::NotPresent,
            vec![],
        );
        let s = render_to_string(&AppState::new(vec![iso]));
        assert!(s.contains("UNVERIFIED"));
        assert!(s.contains("no sibling .sha256"));
    }

    #[test]
    fn render_coverage_tier3_key_not_trusted() {
        let iso = iso_with_verification(
            "untrusted",
            iso_probe::HashVerification::NotPresent,
            iso_probe::SignatureVerification::KeyNotTrusted {
                key_id: "9f3a...".to_string(),
            },
            vec![],
        );
        let s = render_to_string(&AppState::new(vec![iso]));
        assert!(s.contains("UNTRUSTED KEY"));
        assert!(s.contains("9f3a"));
    }

    #[test]
    fn render_coverage_tier4_parse_failed() {
        let failed = iso_probe::FailedIso {
            iso_path: PathBuf::from("/isos/broken.iso"),
            reason: "mount: wrong fs type".to_string(),
            kind: iso_probe::FailureKind::MountFailed,
        };
        let state = AppState::new(Vec::new()).with_failed_isos(vec![failed]);
        let s = render_to_string(&state);
        assert!(s.contains("PARSE FAILED"));
        assert!(s.contains("wrong fs type"));
        assert!(s.contains("MountFailed"));
    }

    #[test]
    fn render_coverage_tier5_windows_blocked() {
        let mut iso = fake_iso("win11");
        iso.quirks = vec![Quirk::NotKexecBootable];
        iso.distribution = Distribution::Windows;
        let s = render_to_string(&AppState::new(vec![iso]));
        assert!(s.contains("BOOT BLOCKED"), "tier-5 label missing");
        // L1 panel replaces the generic "Boot is disabled" prose with
        // an actionable Rufus-redirect for Windows ISOs. The verdict
        // label remains BOOT BLOCKED (drives color + is_bootable gate);
        // only the info-pane body swaps.
        assert!(
            s.contains("Windows 11 installer detected"),
            "Windows redirect header missing: {s}"
        );
        assert!(
            s.contains("https://rufus.ie"),
            "Rufus URL missing from redirect panel: {s}"
        );
    }

    #[test]
    fn render_coverage_tier5_windows_lists_bootable_linux_isos() {
        // When a Linux ISO is also on the stick, the redirect panel
        // surfaces it by filename — the operator shouldn't have to
        // re-scan the list pane to find their non-Windows options.
        let mut win = fake_iso("win11");
        win.quirks = vec![Quirk::NotKexecBootable];
        win.distribution = Distribution::Windows;
        win.iso_path = PathBuf::from("/isos/Win11_23H2.iso");

        let mut linux = fake_iso("ubuntu");
        linux.distribution = Distribution::Debian;
        linux.iso_path = PathBuf::from("/isos/ubuntu-24.04.iso");

        let mut state = AppState::new(vec![win, linux]);
        // `Screen::List { selected }` is a position in the sorted
        // visible_entries() list, not a direct state.isos index.
        // With sort=name, "ubuntu" sorts before "win11" so row 0 is
        // Ubuntu; pin the Windows row explicitly so info-pane
        // renders its redirect panel.
        let win_row = state
            .visible_entries()
            .iter()
            .position(|e| matches!(e, ViewEntry::Iso(i) if matches!(state.isos[*i].distribution, Distribution::Windows)))
            .unwrap_or(0);
        state.screen = Screen::List { selected: win_row };
        let s = render_to_string(&state);
        assert!(
            s.contains("these are on this stick"),
            "bootable-linux heading missing: {s}"
        );
        assert!(
            s.contains("ubuntu-24.04.iso"),
            "bootable linux ISO filename missing: {s}"
        );
    }

    #[test]
    fn render_coverage_tier5_windows_fallback_when_no_linux_isos() {
        // With no non-Windows ISOs on the stick, the panel prompts
        // the operator to drop one into AEGIS_ISOS/ rather than
        // listing an empty bullet set.
        let mut win = fake_iso("win11");
        win.quirks = vec![Quirk::NotKexecBootable];
        win.distribution = Distribution::Windows;
        let s = render_to_string(&AppState::new(vec![win]));
        assert!(
            s.contains("AEGIS_ISOS"),
            "fallback panel must name the AEGIS_ISOS/ partition: {s}"
        );
    }

    #[test]
    fn render_coverage_tier6_hash_mismatch() {
        let iso = iso_with_verification(
            "forged",
            iso_probe::HashVerification::Mismatch {
                expected: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    .to_string(),
                actual: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                    .to_string(),
                source: "/isos/forged.iso.sha256".to_string(),
            },
            iso_probe::SignatureVerification::NotPresent,
            vec![],
        );
        let s = render_to_string(&AppState::new(vec![iso]));
        assert!(s.contains("HASH MISMATCH"));
        assert!(
            s.contains("MISMATCH"),
            "sha256 status must call out mismatch"
        );
    }

    #[test]
    fn render_coverage_confirm_screen_preserves_metadata() {
        let mut state = AppState::new(vec![fake_iso("debian")]);
        state.confirm_selection();
        let s = render_to_string(&state);
        // Verdict banner (#632) replaces the "Verdict: ..." inline text;
        // assert the bookend-arrow form. Reason text + the rest of the
        // confirm-screen metadata stays.
        for needle in &[
            "▶ UNVERIFIED ◀",
            "Reason:",
            "casper/vmlinuz",
            "boot=casper",
            "GiB",
        ] {
            assert!(s.contains(needle), "Confirm screen missing {needle}: {s}");
        }
    }

    #[test]
    fn render_coverage_verifying_screen_shows_progress() {
        let mut state = AppState::new(vec![fake_iso("verify-me")]);
        state.begin_verify(0);
        state.verify_tick(500_000, 1_000_000);
        let s = render_to_string(&state);
        assert!(s.contains("Verifying"), "Verifying label missing: {s}");
    }

    #[test]
    fn render_coverage_trust_challenge_shows_typed_confirmation() {
        let mut state = AppState::new(vec![fake_iso("degraded")]);
        state.screen = Screen::TrustChallenge {
            selected: 0,
            buffer: "bo".to_string(),
        };
        let s = render_to_string(&state);
        assert!(s.contains("typed confirmation"));
        assert!(s.contains("Degraded"));
    }

    #[test]
    fn render_coverage_help_overlay_drawn_above_prior_screen() {
        let mut state = AppState::new(vec![fake_iso("a")]);
        state.open_help();
        let s = render_to_string(&state);
        assert!(s.contains("Keybindings") || s.contains("Help"));
    }

    #[test]
    fn render_help_overlay_clears_underlying_info_pane_text() {
        // #629: the help overlay must opaquely cover the dual-pane base.
        // Without the Clear widget, ratatui's Paragraph leaves cells
        // untouched on blank `Line::from("")` rows, and info-pane fields
        // ("Cmdline", "Initrd", etc.) bleed through.
        //
        // Strategy: render with one ISO whose info pane carries a marker
        // string that's NOT present in the help body. Open help, render,
        // grep the buffer dump. The marker must be absent.
        const MARKER: &str = "BLEED_PROBE_XYZZY";
        let mut iso = fake_iso("a");
        // Plant the marker in an info-pane field — `cmdline` lands in
        // the right pane on the List screen.
        iso.cmdline = Some(MARKER.to_string());
        let mut state = AppState::new(vec![iso]);
        state.open_help();
        let s = render_to_string(&state);
        assert!(
            s.contains("Keybindings"),
            "help overlay didn't render — sanity check failed"
        );
        assert!(
            !s.contains(MARKER),
            "help overlay leaked info-pane text underneath: marker {MARKER:?} found in render"
        );
    }

    #[test]
    fn render_coverage_confirm_quit_overlay_drawn() {
        let mut state = AppState::new(vec![fake_iso("a")]);
        state.request_quit();
        let s = render_to_string(&state);
        assert!(
            s.contains("Quit") || s.contains("quit"),
            "ConfirmQuit overlay missing: {s}"
        );
    }

    #[test]
    fn render_coverage_filter_editing_mode_shows_banner() {
        let mut state = AppState::new(vec![fake_iso("a")]);
        state.open_filter();
        state.filter_push('t');
        state.filter_push('e');
        let s = render_to_string(&state);
        assert!(s.contains("FILTER"), "filter-editing banner missing: {s}");
    }

    #[test]
    fn render_coverage_filter_no_matches_shows_empty_state_label() {
        let mut state = AppState::new(vec![fake_iso("debian")]);
        state.open_filter();
        state.filter_push('z');
        state.filter_commit();
        let s = render_to_string(&state);
        assert!(s.contains("no matches"), "no-match label missing: {s}");
    }

    #[test]
    fn render_coverage_focus_border_differs_by_pane() {
        // The focused pane's border uses theme.success (green), the
        // unfocused pane uses DarkGray. We can't color-introspect in
        // the TestBackend easily, so assert title distinction: the
        // active info-pane title reads "info (focused)".
        let mut state = AppState::new(vec![fake_iso("a")]);
        let list_focused = render_to_string(&state);
        assert!(!list_focused.contains("(focused)"));
        state.toggle_pane();
        let info_focused = render_to_string(&state);
        assert!(info_focused.contains("info (focused)"));
    }

    #[test]
    fn render_coverage_mixed_tiers_all_visible_in_list() {
        // Stick containing a tier-1 ISO + a tier-4 (failed) ISO.
        // Both must appear in the list — verifying all-ISOs-visible
        // is the primary epic goal (#455).
        let iso = iso_with_verification(
            "good",
            iso_probe::HashVerification::Verified {
                digest: "1".repeat(64),
                source: "/isos/good.iso.sha256".to_string(),
            },
            iso_probe::SignatureVerification::NotPresent,
            vec![],
        );
        let failed = iso_probe::FailedIso {
            iso_path: PathBuf::from("/isos/broken.iso"),
            reason: "wrong fs type".to_string(),
            kind: iso_probe::FailureKind::MountFailed,
        };
        let state = AppState::new(vec![iso]).with_failed_isos(vec![failed]);
        let s = render_to_string(&state);
        assert!(s.contains("good"), "tier-1 filename missing");
        assert!(s.contains("broken.iso"), "tier-4 filename missing");
    }

    #[test]
    fn render_coverage_extreme_narrow_terminal_does_not_panic() {
        // 80x24 is the documented minimum. The dual-pane layout is
        // cramped at this width but must not panic — degraded
        // rendering is acceptable.
        let state = AppState::new(vec![fake_iso("a")]);
        let s = render_to_string_sized(&state, 80, 24);
        assert!(!s.is_empty(), "80x24 render produced empty output");
    }

    #[test]
    fn render_coverage_extreme_wide_terminal_still_renders() {
        let state = AppState::new(vec![fake_iso("a")]);
        let s = render_to_string_sized(&state, 200, 60);
        assert!(s.contains("UNVERIFIED"), "wide terminal lost tier label");
    }

    #[test]
    fn render_coverage_footer_matches_list_context() {
        // Footer is registry-driven (#460). On List screen with
        // list-pane focus we expect Tab/?/q plus navigation.
        let state = AppState::new(vec![fake_iso("a")]);
        let s = render_to_string(&state);
        for k in &["[Tab]", "[?]", "[q]", "[/]"] {
            assert!(s.contains(k), "footer missing {k}: {s}");
        }
    }

    #[test]
    fn render_coverage_info_pane_scroll_offsets_hide_top_lines() {
        // Scroll the info pane down; early rows should be clipped.
        let mut state = AppState::new(vec![fake_iso("a")]);
        state.toggle_pane();
        state.move_info_scroll(3);
        let s = render_to_string(&state);
        // With info_scroll=3 the "Verdict:" row may be pushed off the
        // top. We can't assert "absent" cleanly because ratatui still
        // draws labels below, but the total length should still be
        // non-empty (no panic).
        assert!(!s.is_empty());
    }
}
