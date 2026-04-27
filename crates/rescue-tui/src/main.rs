// SPDX-License-Identifier: MIT OR Apache-2.0

//! `rescue-tui` — ratatui application shown inside the aegis-boot signed Linux
//! rescue environment. Discovers ISOs via `iso-probe`, lets the user pick one,
//! and hands off to `kexec-loader`.
//!
//! See [ADR 0001](../../../../docs/adr/0001-runtime-architecture.md) for the
//! Secure Boot rationale.

#![forbid(unsafe_code)]

// All modules now live in lib.rs so sibling binaries like
// tiers-docgen (#462) can share them. main.rs is a thin driver.
use rescue_tui::{failure_log, network, persistence, render, state, theme, tier_b_log, tpm};

use std::collections::HashSet;
use std::env;
use std::io;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::Duration;

use crossterm::cursor::MoveTo;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::state::{AppState, Screen};

/// Default search roots when `AEGIS_ISO_ROOTS` is not set.
const DEFAULT_ROOTS: &[&str] = &["/run/media", "/mnt"];

fn main() -> ExitCode {
    tracing_subscriber_init();
    let roots = parse_roots(env::var("AEGIS_ISO_ROOTS").ok().as_deref());
    match run(&roots) {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("rescue-tui: {e}");
            ExitCode::from(1)
        }
    }
}

fn tracing_subscriber_init() {
    // systemd-journald captures stderr from services it runs, so stderr is
    // the right destination even for "structured" output — the journal
    // handles it. `AEGIS_LOG_JSON=1` switches to a machine-readable format
    // suited to `journalctl --output=json`; the default stays human-readable.
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new("rescue_tui=info,iso_probe=info,kexec_loader=info")
    });
    if std::env::var("AEGIS_LOG_JSON").is_ok() {
        let _ = tracing_subscriber::fmt()
            .with_writer(io::stderr)
            .with_env_filter(filter)
            .json()
            .try_init();
    } else {
        let _ = tracing_subscriber::fmt()
            .with_writer(io::stderr)
            .with_env_filter(filter)
            .try_init();
    }
}

/// Persist a Tier B parse-failure log (#347 Phase 3b). Best-effort —
/// writes to `/run/media/aegis-isos` first (the live `AEGIS_ISOS`
/// mount), falls back to `/tmp/aegis-tier-b-log` if that's not
/// writable. Never blocks rescue-tui startup; tracing carries the
/// diagnostic if every base fails. Lifted out of `run()` so that fn
/// stays under clippy's 100-line cap.
fn persist_tier_b_log(failed: &[iso_probe::FailedIso]) {
    if failed.is_empty() {
        return;
    }
    for base in ["/run/media/aegis-isos", "/tmp/aegis-tier-b-log"] {
        let dir = std::path::Path::new(base);
        match tier_b_log::write_failure_log(failed, dir) {
            Ok(Some(path)) => {
                tracing::info!(
                    path = %path.display(),
                    count = failed.len(),
                    "tier-b: parse-failure log written"
                );
                return;
            }
            Ok(None) => return, // empty list, guarded above; defensive
            Err(e) => {
                tracing::debug!(
                    base = base,
                    error = %e,
                    "tier-b: write attempt failed; trying next base"
                );
            }
        }
    }
}

/// Count the number of `.iso` files present under any of `roots`.
/// Depth-limited walk (max 3 levels) since `AEGIS_ISOS` is a flat
/// layout in practice; goes 3 deep to catch operators who nested one
/// or two levels. Matches the depth bound in `iso_probe::find_iso_size`.
///
/// Used for the #85 Tier 2 inline error band: `discover()` silently
/// drops per-ISO parse failures; we need an independent count to
/// spot "N ISOs on disk, M discovered, N-M skipped" and surface that
/// to the operator without requiring them to read journalctl.
fn count_iso_files_on_disk(roots: &[PathBuf]) -> usize {
    const MAX_DEPTH: u32 = 3;
    // Dedupe by canonical absolute path so overlapping roots
    // (e.g. AEGIS_ISO_ROOTS=/run/media/aegis-isos:/run/media — the
    // /init default) don't count the same .iso file twice. iso-probe's
    // discover() already dedupes its results; without the same dedup
    // here, the inline "N ISO(s) failed to parse" band overcounts
    // (#623).
    fn walk(dir: &std::path::Path, depth: u32, seen: &mut HashSet<PathBuf>) {
        if depth == 0 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_file()
                && path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("iso"))
            {
                let key = std::fs::canonicalize(&path).unwrap_or(path);
                seen.insert(key);
            } else if ft.is_dir() {
                walk(&path, depth - 1, seen);
            }
        }
    }
    let mut seen: HashSet<PathBuf> = HashSet::new();
    for root in roots {
        if root.exists() {
            walk(root, MAX_DEPTH, &mut seen);
        }
    }
    seen.len()
}

fn parse_roots(env_var: Option<&str>) -> Vec<PathBuf> {
    match env_var {
        Some(s) if !s.is_empty() => s.split(':').map(PathBuf::from).collect(),
        _ => DEFAULT_ROOTS.iter().map(PathBuf::from).collect(),
    }
}

fn run(roots: &[PathBuf]) -> Result<u8, Box<dyn std::error::Error>> {
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        roots = ?roots,
        "rescue-tui starting"
    );
    // Count .iso files on disk BEFORE calling discover(). iso-probe
    // silently drops ISOs it can't parse (malformed layout, unsupported
    // distro, loopback mount failure after PR #170 surfaces a warn).
    // Tracing logs the details; the TUI inline band shows the count so
    // operators can act without reading journalctl. (#85 Tier 2)
    let on_disk_iso_count = count_iso_files_on_disk(roots);

    let (isos, failed) = match iso_probe::discover(roots) {
        Ok(report) => (report.isos, report.failed),
        Err(iso_probe::ProbeError::NoIsosFound) => {
            tracing::info!("no ISOs discovered under any root");
            (Vec::new(), Vec::new())
        }
        Err(e) => {
            tracing::error!(error = %e, "ISO discovery failed");
            return Err(e.into());
        }
    };
    // Log structured failures at DEBUG. The inline SKIPPED banner is
    // kept for now (sourced from failed.len() plus any on-disk files
    // iso-parser never even attempted to mount); #458 replaces it with
    // per-ISO tier-4 rows rendered from AppState::failed_isos.
    for failure in &failed {
        tracing::debug!(
            iso = %failure.iso_path.display(),
            reason = %failure.reason,
            kind = ?failure.kind,
            "iso-probe: failed ISO (surfaces as tier-4 row once #458 lands)"
        );
    }
    let counted_but_not_attempted = on_disk_iso_count.saturating_sub(isos.len() + failed.len());
    let skipped = failed.len() + counted_but_not_attempted;
    persist_tier_b_log(&failed);
    // Startup banner to stderr — mirrored via tracing::info! so structured
    // consumers (journald, CI smoke greps) see the same signal as humans
    // reading the serial console directly.
    eprintln!(
        "aegis-boot rescue-tui starting: discovered {} ISO(s){}",
        isos.len(),
        if skipped > 0 {
            format!(" ({skipped} skipped — see logs)")
        } else {
            String::new()
        }
    );
    tracing::info!(
        discovered = isos.len(),
        on_disk = on_disk_iso_count,
        skipped,
        "ISO discovery complete"
    );
    for iso in &isos {
        tracing::debug!(
            label = %iso.label,
            path = %iso.iso_path.display(),
            distribution = ?iso.distribution,
            quirks = ?iso.quirks,
            "discovered ISO"
        );
    }
    let mut state = AppState::new(isos)
        .with_failed_isos(failed)
        .with_scanned_roots(roots.to_vec())
        .with_skipped_iso_count(skipped);
    if let Ok(name) = env::var("AEGIS_THEME") {
        state.theme = theme::Theme::from_name(&name);
        tracing::info!(theme = %name, "rescue-tui: theme override applied");
    }
    apply_persisted_choice(&mut state);

    // Non-interactive automation mode. When AEGIS_AUTO_KEXEC is set to a
    // substring match against ISO paths, rescue-tui skips the TUI entirely
    // and kexecs the first matching ISO. Exit codes:
    //   0  — should be unreachable; load_and_exec replaces the process
    //   2  — no ISO matched the substring
    //   3  — kexec failed (classified error in the log)
    // This is intentionally not a full scripting interface — just enough to
    // support CI end-to-end testing without a TTY. Real operator automation
    // should live outside the TUI binary.
    if let Ok(needle) = env::var("AEGIS_AUTO_KEXEC") {
        return run_auto_kexec(&state, &needle).map(|()| 0u8);
    }

    // #104: text-mode fallback for screen readers / braille /
    // 40-col serial / TERM=dumb. Triggered by AEGIS_A11Y=text or
    // TERM=dumb. Renders a numbered menu to stdout and reads a
    // line from stdin — no alt-screen, no cursor positioning, no
    // escape sequences that would confuse assistive tech.
    let text_mode_requested = env::var("AEGIS_A11Y").is_ok_and(|v| v.eq_ignore_ascii_case("text"))
        || env::var("TERM").is_ok_and(|v| v == "dumb");
    if text_mode_requested {
        return run_text_mode(&mut state);
    }

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    // Linux VT (tty0) does not honor xterm CSI ?1049h alternate
    // screen — rescue-tui would otherwise draw over the kernel +
    // /init dmesg scroll. Explicitly clear + home the cursor so
    // the first frame is drawn on a blank canvas. (#115)
    execute!(
        stdout,
        EnterAlternateScreen,
        Clear(ClearType::All),
        MoveTo(0, 0)
    )?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let loop_result = event_loop(&mut terminal, &mut state);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    loop_result?;

    // #90: if the operator selected the rescue-shell entry, signal
    // that to /init via the exit code. Otherwise ordinary quit.
    if state.shell_requested {
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        Ok(crate::state::RESCUE_SHELL_EXIT_CODE as u8)
    } else {
        Ok(0)
    }
}

// Event loop is intentionally over the 100-line clippy threshold: the
// screen-specific key handling lives here in one place, and splitting
// it hurts readability more than the length.
#[allow(clippy::too_many_lines)]
fn event_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    state: &mut AppState,
) -> Result<(), Box<dyn std::error::Error>>
where
    // Required by ratatui 0.30: Backend::Error is no longer 'static by
    // default, but we box it into a trait-object error that needs to be.
    <B as ratatui::backend::Backend>::Error: 'static,
{
    // Active verify-now worker (#89). None when no verification is in
    // flight; Some(rx) while the worker thread is streaming progress.
    let mut active_verify: Option<Receiver<VerifyMsg>> = None;
    // Active DHCP worker (#655 Phase 1B). Same ownership shape as
    // verify: Some(rx) while udhcpc runs, None otherwise.
    let mut active_dhcp: Option<Receiver<NetworkMsg>> = None;

    loop {
        terminal.draw(|f| render::draw(f, state))?;

        if state.screen == Screen::Quitting {
            return Ok(());
        }

        // Drain DHCP worker messages.
        if let Some(rx) = active_dhcp.as_ref() {
            loop {
                match rx.try_recv() {
                    Ok(NetworkMsg::Progress { status }) => {
                        state.network_progress(status);
                    }
                    Ok(NetworkMsg::Done { iface, result }) => {
                        state.network_finish_dhcp(iface, result);
                        active_dhcp = None;
                        break;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        active_dhcp = None;
                        break;
                    }
                }
            }
        }

        // Drain any pending verify-now progress / completion.
        if let Some(rx) = active_verify.as_ref() {
            loop {
                match rx.try_recv() {
                    Ok(VerifyMsg::Progress { bytes, total }) => {
                        state.verify_tick(bytes, total);
                    }
                    Ok(VerifyMsg::Done(Ok(outcome))) => {
                        // #548 UX T4B: append an audit-log line for the
                        // operator-initiated verify, regardless of
                        // verdict. Best-effort — the operator already
                        // sees the verdict in the TUI; the JSONL line
                        // is for post-mortem / "I checked this ISO
                        // before boot" evidence.
                        if let Some(iso_path) =
                            state.iso_being_verified().map(|i| i.iso_path.clone())
                        {
                            match save_verify_audit_log(&iso_path, &outcome) {
                                Ok(path) => {
                                    tracing::info!(
                                        path = %path.display(),
                                        "verify-now: audit-log line written"
                                    );
                                    // #602: a successful write supersedes any
                                    // prior failure banner — the audit trail
                                    // is intact again, so dismiss the warning.
                                    state.clear_audit_warning();
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        error = %e,
                                        "verify-now: audit-log write failed (continuing)"
                                    );
                                    // #602: surface the failure inline on the
                                    // Confirm screen so the operator sees
                                    // "verdict shown but not persisted" before
                                    // deciding whether to kexec. The kexec
                                    // gate itself is unaffected.
                                    state.set_audit_warning(format!(
                                        "audit log write failed ({e}) — verdict shown but not persisted"
                                    ));
                                }
                            }
                        }
                        state.verify_finish(outcome);
                        active_verify = None;
                        break;
                    }
                    Ok(VerifyMsg::Done(Err(e))) => {
                        tracing::warn!(error = %e, "verify-now: worker failed");
                        state.cancel_verify();
                        active_verify = None;
                        break;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        active_verify = None;
                        break;
                    }
                }
            }
        }

        // Shorter poll timeout while verifying so the progress bar
        // animates smoothly; 250ms is fine otherwise.
        let poll = if active_verify.is_some() {
            Duration::from_millis(50)
        } else {
            Duration::from_millis(250)
        };
        if !event::poll(poll)? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        // Verifying screen swallows all keys: Esc cancels, others no-op.
        if matches!(state.screen, Screen::Verifying { .. }) {
            if key.code == KeyCode::Esc {
                state.cancel_verify();
                active_verify = None;
            }
            continue;
        }

        // ConfirmQuit modal swallows everything: y/Enter confirms exit,
        // n/Esc cancels back to prior screen.
        if matches!(state.screen, Screen::ConfirmQuit { .. }) {
            match key.code {
                KeyCode::Char('y' | 'Y') | KeyCode::Enter => state.confirm_quit(),
                KeyCode::Char('n' | 'N' | 'q') | KeyCode::Esc => {
                    state.cancel_quit();
                }
                _ => {}
            }
            continue;
        }

        // Help overlay swallows everything: ?/Esc/q dismisses.
        if matches!(state.screen, Screen::Help { .. }) {
            match key.code {
                KeyCode::Char('?' | 'q') | KeyCode::Esc => state.close_help(),
                _ => {}
            }
            continue;
        }

        // BlockedToast (#546) — any key dismisses back to List. Lives
        // here alongside Help/ConfirmQuit so the toast doesn't fall
        // through to the main match's per-screen bindings.
        if matches!(state.screen, Screen::BlockedToast { .. }) {
            state.dismiss_blocked_toast();
            continue;
        }

        let in_editor = matches!(state.screen, Screen::EditCmdline { .. });
        let in_filter_input = state.filter_editing;

        // Filter editor consumes all keys while active (#85 Tier 2).
        if in_filter_input {
            match key.code {
                KeyCode::Enter => state.filter_commit(),
                KeyCode::Esc => state.filter_cancel(),
                KeyCode::Backspace => state.filter_backspace(),
                KeyCode::Char(c) => state.filter_push(c),
                _ => {}
            }
            continue;
        }

        // Global keys (not in cmdline editor / filter input):
        //   q  → on Confirm, back to List (design-review #103 — the
        //        reboot-the-machine prompt was overloaded with
        //        "quit this screen"); elsewhere, open the quit prompt.
        //   ?  → help overlay
        if !in_editor {
            match key.code {
                KeyCode::Char('q') => {
                    if matches!(state.screen, Screen::Confirm { .. }) {
                        state.cancel_confirmation();
                    } else if matches!(state.screen, Screen::Network { .. }) {
                        // Network overlay treats 'q' as Close (parallel
                        // to Confirm). Without this, 'q' would always
                        // open the global quit prompt — surprising for
                        // an overlay-style screen. (#655 Phase 1B)
                        state.cancel_network();
                    } else {
                        state.request_quit();
                    }
                    continue;
                }
                KeyCode::Char('?') => {
                    state.open_help();
                    continue;
                }
                _ => {}
            }
        }

        // Ctrl+A / Ctrl+E in the cmdline editor are readline-style
        // line-start / line-end jumps. Intercept BEFORE the main match so
        // the catch-all KeyCode::Char(c) → cmdline_insert(c) doesn't
        // swallow them as literal 'a' / 'e'. (#544)
        if matches!(state.screen, Screen::EditCmdline { .. })
            && key.modifiers.contains(KeyModifiers::CONTROL)
        {
            match key.code {
                KeyCode::Char('a') => {
                    state.cmdline_cursor_home();
                    continue;
                }
                KeyCode::Char('e') => {
                    state.cmdline_cursor_end();
                    continue;
                }
                _ => {}
            }
        }

        // Shift+↑ / Shift+↓ on the List screen scrolls the info pane while
        // keeping list-pane focus, mirroring tig/gitui/lazygit. Without
        // this chord the operator has to Tab away from the list, scroll,
        // then Tab back — losing their selection cursor. Intercept BEFORE
        // the main match so the unmodified KeyCode::Up/Down arms route to
        // the focused pane (existing behavior). (#545)
        if matches!(state.screen, Screen::List { .. })
            && key.modifiers.contains(KeyModifiers::SHIFT)
        {
            match key.code {
                KeyCode::Up => {
                    state.move_info_scroll(-1);
                    continue;
                }
                KeyCode::Down => {
                    state.move_info_scroll(1);
                    continue;
                }
                _ => {}
            }
        }

        match (&state.screen, key.code) {
            // Tab toggles focus between the list pane and the info pane
            // on the List screen only. Shift+Tab is treated the same
            // way — we only have two panes so there's no forward/back
            // distinction to preserve. (#458)
            (Screen::List { .. }, KeyCode::Tab | KeyCode::BackTab) => state.toggle_pane(),
            // Vim navigation aliases for arrow keys (#85). Routed to
            // whichever pane currently holds focus (#458).
            (Screen::List { .. }, KeyCode::Up | KeyCode::Char('k')) => {
                if state.pane == state::Pane::Info {
                    state.move_info_scroll(-1);
                } else {
                    state.move_selection(-1);
                }
            }
            (Screen::List { .. }, KeyCode::Down | KeyCode::Char('j')) => {
                if state.pane == state::Pane::Info {
                    state.move_info_scroll(1);
                } else {
                    state.move_selection(1);
                }
            }
            // `g`/`G` are the vim aliases; `Home`/`End` are the layout-
            // agnostic equivalents — crossterm maps them identically
            // under AZERTY, Dvorak, and any other layout, whereas the
            // letter keys follow the OS's logical remapping. (#93)
            (Screen::List { .. }, KeyCode::Char('g') | KeyCode::Home) => state.move_to_first(),
            (Screen::List { .. }, KeyCode::Char('G') | KeyCode::End) => state.move_to_last(),
            (Screen::List { .. }, KeyCode::Enter | KeyCode::Char('l')) => {
                if state.is_shell_selected() {
                    // #90: rescue shell picked. Signal the shell-drop
                    // exit path; run() will propagate the sentinel
                    // code so /init can exec /bin/sh.
                    tracing::info!(
                        "operator selected rescue shell — exiting with code {}",
                        crate::state::RESCUE_SHELL_EXIT_CODE
                    );
                    state.shell_requested = true;
                    // Direct exit — no confirmation prompt needed;
                    // the operator already picked the shell
                    // explicitly.
                    state.confirm_quit();
                } else {
                    state.confirm_selection();
                }
            }
            (Screen::List { .. }, KeyCode::Char('/')) => state.open_filter(),
            (Screen::List { .. }, KeyCode::Char('s')) => state.cycle_sort(),
            (Screen::List { selected }, KeyCode::Char('v')) => {
                if let Some(real_idx) = state.real_index(*selected)
                    && let Some(iso) = state.isos.get(real_idx)
                {
                    let rx = spawn_verify_worker(iso.iso_path.clone());
                    state.begin_verify(real_idx);
                    active_verify = Some(rx);
                }
            }
            // `D` opens the ConfirmDelete prompt for the highlighted
            // ISO row. Limited to pane=List so muscle-memory keypresses
            // while reading the Info pane don't trigger a destructive
            // prompt. `enter_delete` itself self-guards against
            // FailedIso / RescueShell rows (returns None) so this arm
            // is safe to fire even on a non-deletable cursor.
            (Screen::List { selected }, KeyCode::Char('D')) if state.pane == state::Pane::List => {
                let _ = state.enter_delete(*selected);
            }
            // ConfirmDelete handlers. `y/Y` performs the unlink and
            // updates state on success / surfaces an Error screen on
            // failure. `n/N/Esc` cancels back to List, cursor preserved.
            (Screen::ConfirmDelete { selected }, KeyCode::Char('y' | 'Y')) => {
                let cursor = *selected;
                let target = state
                    .real_index(cursor)
                    .and_then(|i| state.isos.get(i))
                    .map(|iso| iso.iso_path.clone());
                if let Some(path) = target {
                    match perform_iso_delete(&path) {
                        Ok(()) => {
                            tracing::info!(
                                iso = %path.display(),
                                "rescue-tui: ISO + sidecar removed via D-prompt confirm"
                            );
                            state.delete_completed();
                        }
                        Err(e) => {
                            tracing::warn!(
                                iso = %path.display(),
                                error = %e,
                                "rescue-tui: ISO delete failed"
                            );
                            state.record_delete_error(&e);
                        }
                    }
                } else {
                    // Cursor went stale (filter shifted etc.) — bail
                    // back to List rather than exploding.
                    state.cancel_delete();
                }
            }
            (Screen::ConfirmDelete { .. }, KeyCode::Char('n' | 'N') | KeyCode::Esc) => {
                state.cancel_delete();
            }

            // ---- Network overlay (#655 Phase 1B) ----------------
            // `n` from List or Confirm opens it. `n` is currently NOT
            // bound on either screen; if a future feature claims it,
            // either remap or check key.modifiers here.
            (Screen::List { .. } | Screen::Confirm { .. }, KeyCode::Char('n')) => {
                state.enter_network(network::enumerate_interfaces());
            }
            (Screen::Network { .. }, KeyCode::Up | KeyCode::Char('k')) => {
                state.network_move_selection(-1);
            }
            (Screen::Network { .. }, KeyCode::Down | KeyCode::Char('j')) => {
                state.network_move_selection(1);
            }
            (Screen::Network { .. }, KeyCode::Char('r')) => {
                state.network_refresh(network::enumerate_interfaces());
            }
            (Screen::Network { .. }, KeyCode::Enter) => {
                if let Some(iface) = state.network_begin_dhcp() {
                    let rx = spawn_dhcp_worker(iface);
                    active_dhcp = Some(rx);
                }
            }
            (Screen::Network { .. }, KeyCode::Esc | KeyCode::Char('q')) => {
                state.cancel_network();
            }
            (Screen::Confirm { selected }, KeyCode::Char('v')) => {
                let real_idx = *selected;
                if let Some(iso) = state.isos.get(real_idx) {
                    let rx = spawn_verify_worker(iso.iso_path.clone());
                    state.begin_verify(real_idx);
                    active_verify = Some(rx);
                }
            }

            (Screen::Confirm { .. }, KeyCode::Esc | KeyCode::Char('h')) => {
                state.cancel_confirmation();
            }
            (Screen::Confirm { .. }, KeyCode::Char('e')) => state.enter_cmdline_editor(),
            (Screen::Confirm { selected }, KeyCode::Enter) => {
                let idx = *selected;
                if state.is_kexec_blocked(idx) {
                    tracing::warn!(
                        idx,
                        "rescue-tui: refused kexec — ISO is kexec-blocked (quirk or verification failure)"
                    );
                    state.record_kexec_error(&kexec_loader::KexecError::UnsupportedImage);
                } else if let Some(kind) = state.consent_required_for(idx) {
                    // #347: elevated-risk path requires per-session
                    // consent before kexec proceeds. Once granted, the
                    // session_consent flag short-circuits this branch
                    // and subsequent boots flow through normally.
                    state.enter_consent(kind, idx);
                } else if state.is_degraded_trust(idx) {
                    // #93: YELLOW/GRAY verdict → require typing "boot".
                    state.enter_trust_challenge(idx);
                } else {
                    attempt_kexec(state, idx);
                }
            }

            // #347 consent-screen handlers. 'y' grants for the session;
            // Esc cancels back to Confirm. The ConsentKind drives what
            // happens after grant: install-warning consent flows into
            // the normal kexec dispatch (re-entering the Confirm Enter
            // path with session_consent set short-circuits the consent
            // gate); tier-4 force-boot would attempt a kexec that fails
            // with a clearer error than the current silent BlockedToast.
            (Screen::Consent { .. }, KeyCode::Char('y' | 'Y')) => {
                if let Some(idx) = state.grant_consent() {
                    // After grant: re-evaluate the gate chain for `idx`.
                    // The consent flag is now sticky for the session, so
                    // the consent gate won't fire again; falls through
                    // to trust-challenge or kexec as appropriate.
                    if state.is_degraded_trust(idx) {
                        state.enter_trust_challenge(idx);
                    } else {
                        attempt_kexec(state, idx);
                    }
                }
            }
            (Screen::Consent { .. }, KeyCode::Esc) => {
                state.cancel_consent();
            }

            // Trust challenge (#93): Enter only fires kexec if the
            // buffer equals "boot" — otherwise ignored. Backspace /
            // characters edit the buffer; Esc cancels.
            (Screen::TrustChallenge { selected, buffer }, KeyCode::Enter) => {
                let idx = *selected;
                if buffer == "boot" {
                    attempt_kexec(state, idx);
                }
            }
            (Screen::TrustChallenge { .. }, KeyCode::Esc) => state.trust_challenge_cancel(),
            (Screen::TrustChallenge { .. }, KeyCode::Backspace) => {
                state.trust_challenge_backspace();
            }
            (Screen::TrustChallenge { .. }, KeyCode::Char(c)) => {
                state.trust_challenge_push(c);
            }

            (Screen::EditCmdline { .. }, KeyCode::Enter) => state.commit_cmdline_edit(),
            (Screen::EditCmdline { .. }, KeyCode::Esc) => state.cancel_cmdline_edit(),
            (Screen::EditCmdline { .. }, KeyCode::Left) => state.cmdline_cursor_left(),
            (Screen::EditCmdline { .. }, KeyCode::Right) => state.cmdline_cursor_right(),
            (Screen::EditCmdline { .. }, KeyCode::Home) => state.cmdline_cursor_home(),
            (Screen::EditCmdline { .. }, KeyCode::End) => state.cmdline_cursor_end(),
            (Screen::EditCmdline { .. }, KeyCode::Backspace) => state.cmdline_backspace(),
            (Screen::EditCmdline { .. }, KeyCode::Char(c)) => state.cmdline_insert(c),

            // F10 on Error → tee the one-frame evidence to
            // /run/media/aegis-isos/aegis-log-<ts>.txt so the
            // operator can pull it off the stick from any machine
            // after reboot. rEFInd log-on-ESP pattern. (#92)
            (Screen::Error { .. }, KeyCode::F(10)) => {
                if let Some(text) = state.error_evidence_text() {
                    match save_error_log(&text) {
                        Ok(path) => tracing::info!(
                            path = %path.display(),
                            "operator saved error evidence via F10"
                        ),
                        Err(e) => tracing::warn!(
                            error = %e,
                            "F10 save-log failed (continuing)"
                        ),
                    }
                    // Anonymous Tier-A microreport for later
                    // inclusion in `aegis-boot bug-report`. Best-
                    // effort, non-blocking, no consent needed (the
                    // Tier-A envelope carries no PII by
                    // construction). #342 Phase 2.
                    failure_log::record_failure(&text, "kexec_failure", "rescue_tui");
                }
            }

            // Error → return to List, preserving the ISO that failed
            // so the operator doesn't have to re-navigate. (#85)
            (Screen::Error { return_to, .. }, _) => {
                let idx = *return_to;
                state.screen = Screen::List { selected: idx };
            }
            _ => {}
        }
    }
}

/// Text-mode fallback (#104). Plain-text numbered menu + stdin. No
/// ratatui alt-screen, no ANSI colour (except on stderr which is fine
/// for tracing — terminals usually ignore it with `TERM=dumb`). Every
/// screen transition also emits an `ANN:` line to stderr so
/// brltty/speakup can mirror it to braille/speech.
///
/// Intentionally a single large state-machine function — the
/// accessibility contract pairs each `ANN:` stderr line with the
/// next stdin prompt, which is awkward to split across helpers
/// without ending up with tighter coupling than the original prose.
/// Opt out of the too-many-lines lint here, not refactor cosmetics.
#[allow(clippy::too_many_lines)]
fn run_text_mode(state: &mut AppState) -> Result<u8, Box<dyn std::error::Error>> {
    use std::io::Write;
    let mut out = std::io::stdout().lock();

    loop {
        let entries = state.visible_entries();

        writeln!(out)?;
        writeln!(out, "aegis-boot — pick an entry (text-mode)")?;
        writeln!(out, "Signed boot. Any ISO. Your keys.")?;
        writeln!(
            out,
            "SB: {}    TPM: {}",
            state.secure_boot.summary(),
            state.tpm.summary()
        )?;
        writeln!(out)?;

        for (i, entry) in entries.iter().enumerate() {
            match entry {
                crate::state::ViewEntry::Iso(idx) => {
                    let iso = &state.isos[*idx];
                    let glyph = match trust_verdict_for_text(iso) {
                        "green" => "[+]",
                        "yellow" => "[~]",
                        "red" => "[!]",
                        _ => "[ ]",
                    };
                    writeln!(
                        out,
                        "  {:>2}. {} {}  ({})",
                        i + 1,
                        glyph,
                        iso.label,
                        iso.iso_path.display()
                    )?;
                }
                crate::state::ViewEntry::FailedIso(idx) => {
                    // Tier 4 — iso-parser couldn't extract boot entries.
                    // Surfaced in the text-mode menu so the operator
                    // sees the file exists and why it's not bootable.
                    // Not selectable (index yields a RefusedBoot branch
                    // in the dispatch match below). (#459)
                    if let Some(f) = state.failed_isos.get(*idx) {
                        writeln!(
                            out,
                            "  {:>2}. [!] {} — PARSE FAILED: {}",
                            i + 1,
                            f.iso_path.display(),
                            f.reason
                        )?;
                    }
                }
                crate::state::ViewEntry::RescueShell => {
                    writeln!(out, "  {:>2}. [#] rescue shell (busybox)", i + 1)?;
                }
            }
        }

        writeln!(out)?;
        writeln!(
            out,
            "Legend: [+] verified  [~] hash only  [ ] no trust  [!] tampered  [#] shell"
        )?;
        writeln!(out, "Enter number to select, 'q' + Enter to quit.")?;
        write!(out, "> ")?;
        out.flush()?;

        tracing::info!(
            event = "a11y_menu_shown",
            entries = entries.len(),
            "ANN: menu shown, {} entries",
            entries.len()
        );

        let mut line = String::new();
        if std::io::stdin().read_line(&mut line)? == 0 {
            // EOF → treat as quit.
            return Ok(0);
        }
        let input = line.trim();
        if input.eq_ignore_ascii_case("q") || input.eq_ignore_ascii_case("quit") {
            return Ok(0);
        }
        let Ok(n) = input.parse::<usize>() else {
            writeln!(out, "error: not a number — try again or 'q' to quit")?;
            continue;
        };
        if n == 0 || n > entries.len() {
            writeln!(out, "error: out of range (1..={})", entries.len())?;
            continue;
        }
        let Some(chosen) = entries.get(n - 1).copied() else {
            writeln!(out, "error: index {n} no longer valid")?;
            continue;
        };
        match chosen {
            crate::state::ViewEntry::RescueShell => {
                tracing::info!("ANN: operator selected rescue shell (text mode)");
                state.shell_requested = true;
                #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
                return Ok(crate::state::RESCUE_SHELL_EXIT_CODE as u8);
            }
            crate::state::ViewEntry::Iso(idx) => {
                run_text_confirm(state, &mut out, idx)?;
                // After confirm/verdict/shell return, loop repaints.
                // If kexec succeeded it replaced the process.
            }
            crate::state::ViewEntry::FailedIso(idx) => {
                // Tier-4 rows aren't bootable by design. Text mode
                // surfaces the reason and loops so the operator can
                // pick a different entry. (#459)
                if let Some(f) = state.failed_isos.get(idx) {
                    writeln!(
                        out,
                        "error: entry {n} is a parse-failed ISO and cannot boot: {}",
                        f.reason
                    )?;
                }
            }
        }
    }
}

/// Single-screen Confirm flow in text mode. Prints the one-frame
/// evidence block, asks yes/no (or `boot` typed-confirmation for
/// degraded trust), then dispatches to `attempt_kexec` or returns.
fn run_text_confirm(
    state: &mut AppState,
    out: &mut impl std::io::Write,
    idx: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some(iso) = state.isos.get(idx) else {
        return Ok(());
    };
    let cmdline = state.effective_cmdline(idx);
    let measurement = hex::encode(crate::tpm::compute_measurement(&iso.iso_path, &cmdline));
    let verdict = trust_verdict_for_text(iso);

    writeln!(out)?;
    writeln!(out, "── Confirm kexec ──────────────────────────────")?;
    writeln!(out, "Verdict:    {}", verdict.to_uppercase())?;
    writeln!(out, "Label:      {}", iso.label)?;
    writeln!(out, "ISO:        {}", iso.iso_path.display())?;
    writeln!(
        out,
        "Cmdline:    {}",
        if cmdline.is_empty() {
            "(none)"
        } else {
            &cmdline
        }
    )?;
    writeln!(out, "Measures:   sha256:{} → PCR 12", &measurement[..32])?;
    writeln!(out)?;

    tracing::info!(
        event = "a11y_confirm_shown",
        iso = %iso.iso_path.display(),
        verdict = verdict,
        "ANN: confirm {} verdict {}",
        iso.label,
        verdict
    );

    if state.is_kexec_blocked(idx) {
        writeln!(
            out,
            "BLOCKED: verification or quirk failure — kexec refused."
        )?;
        writeln!(out, "Press Enter to return to the list.")?;
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        drop(line);
        return Ok(());
    }

    let prompt = if state.is_degraded_trust(idx) {
        writeln!(
            out,
            "Degraded trust — type 'boot' (exactly) then Enter to proceed, or 'no' to cancel."
        )?;
        "boot"
    } else {
        writeln!(out, "Proceed with kexec? [y/N]")?;
        "y"
    };
    write!(out, "> ")?;
    out.flush()?;

    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    let input = line.trim().to_ascii_lowercase();
    if input == prompt || (prompt == "y" && (input == "yes" || input == "y")) {
        attempt_kexec(state, idx);
        // Only reached if kexec failed — surface the error.
        if let crate::state::Screen::Error {
            message, remedy, ..
        } = &state.screen
        {
            writeln!(out)?;
            writeln!(out, "kexec failed: {message}")?;
            if let Some(r) = remedy {
                writeln!(out, "Remedy: {r}")?;
            }
            // Return the screen to List for the next loop iteration.
            state.screen = crate::state::Screen::List { selected: 0 };
        }
    }
    Ok(())
}

fn trust_verdict_for_text(iso: &iso_probe::DiscoveredIso) -> &'static str {
    use iso_probe::{HashVerification as H, Quirk, SignatureVerification as S};
    if iso.quirks.contains(&Quirk::NotKexecBootable)
        || matches!(iso.hash_verification, H::Mismatch { .. })
        || matches!(iso.signature_verification, S::Forged { .. })
    {
        return "red";
    }
    if matches!(iso.signature_verification, S::Verified { .. })
        || matches!(iso.hash_verification, H::Verified { .. })
    {
        return "green";
    }
    if matches!(iso.signature_verification, S::KeyNotTrusted { .. }) {
        return "yellow";
    }
    "gray"
}

/// Convert a Unix epoch (seconds, UTC) to `(year, month, day)` using
/// Howard Hinnant's "civil from days" algorithm. Battle-tested,
/// branch-only-on-sign, no leap-year edge-case bugs. We carry the
/// math inline rather than pull in `chrono` / `time` because rescue-tui
/// is a tiny initramfs binary and the only place we need calendar
/// arithmetic is the daily-rotated verify-audit-log filename. (#548)
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::similar_names
)]
fn epoch_to_civil_date(epoch_secs: u64) -> (i32, u32, u32) {
    let z = (epoch_secs / 86_400) as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    // (z - era * 146_097) is non-negative for any post-1970 epoch; the cast
    // sign-loss lint fires on the type widening but the value is bounded.
    let day_of_era = (z - era * 146_097) as u64;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_phase = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_phase + 2) / 5 + 1;
    let month = if month_phase < 10 {
        month_phase + 3
    } else {
        month_phase - 9
    };
    let year_adjusted = if month <= 2 { year + 1 } else { year };
    (year_adjusted as i32, month as u32, day as u32)
}

/// Append a one-line JSONL record to the verify-audit log when a
/// verify-now action completes. Symmetric counterpart to F10 evidence
/// save (#92) on the Error screen — the success-path equivalent. (#548)
///
/// Location: `/run/media/aegis-isos/verify-log/<YYYY-MM-DD>.jsonl`
/// (per-day rotation), falling back to `/tmp/aegis-verify-log/...` if
/// the `AEGIS_ISOS` partition isn't mounted writable. Best-effort —
/// returns the path on success, an error string on failure (caller
/// just logs and continues; the operator already saw the verdict in
/// the TUI).
///
/// Network-overlay worker → UI messages (#655 Phase 1B). Mirrors the
/// `VerifyMsg` shape: a stream of `Progress` updates, then exactly one
/// `Done` with the terminal result. Worker is fire-and-forget; UI
/// drops the Receiver to abandon.
#[derive(Debug)]
enum NetworkMsg {
    /// Best-effort progress hint (e.g. "trying lease 1/5"). The
    /// associated iface is already in the [`crate::state::NetworkOp::Pending`]
    /// state, so we don't redundantly carry it here.
    Progress { status: String },
    /// Terminal result. `Ok(lease)` carries the post-DHCP iface state;
    /// `Err(message)` carries a human-readable failure cause.
    Done {
        iface: String,
        result: Result<network::NetworkLease, String>,
    },
}

/// Spawn a thread that runs `udhcpc -i <iface> -n -q -t 5 -T 2` and
/// emits Progress + Done messages on the returned Receiver. The
/// worker is short-lived (DHCP times out in ~10s with these flags)
/// and exits when `udhcpc` does.
///
/// Args used:
///   `-i <iface>`  bind to interface
///   `-n`          fail (don't fork) if no lease
///   `-q`          quit after lease/fail (one-shot)
///   `-t 5`        try DHCP DISCOVER 5 times before giving up
///   `-T 2`        2-second timeout per attempt
fn spawn_dhcp_worker(iface: String) -> Receiver<NetworkMsg> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(NetworkMsg::Progress {
            status: "starting udhcpc...".to_string(),
        });
        // udhcpc may live at /bin/udhcpc (the busybox symlink we ship
        // in Phase 1A) OR at /sbin/udhcpc on dev-host distros. Probe
        // both before falling back to PATH.
        let mut cmd = if std::path::Path::new("/bin/udhcpc").exists() {
            std::process::Command::new("/bin/udhcpc")
        } else if std::path::Path::new("/sbin/udhcpc").exists() {
            std::process::Command::new("/sbin/udhcpc")
        } else {
            std::process::Command::new("udhcpc")
        };
        let output = cmd
            .args(["-i", &iface, "-n", "-q", "-t", "5", "-T", "2"])
            .output();
        let result: Result<network::NetworkLease, String> = match output {
            Ok(o) if o.status.success() => network::read_lease(&iface),
            Ok(o) => Err(format!(
                "udhcpc exited {}: {}",
                o.status,
                String::from_utf8_lossy(&o.stderr).trim()
            )),
            Err(e) => Err(format!("spawn udhcpc: {e}")),
        };
        let _ = tx.send(NetworkMsg::Done { iface, result });
    });
    rx
}

/// Unlink an ISO file and its `<iso>.aegis.toml` sidecar. Used by the
/// `D` keybinding's confirm flow.
///
/// Returns Ok on success. Errors out at the first filesystem failure;
/// in particular a missing sidecar is OK (returns Ok), but a sidecar
/// that exists yet won't unlink (read-only mount, perms) is surfaced
/// to the operator since an orphaned sidecar would mislead the next
/// `iso-probe` walk into showing a tier-4 row for a non-existent ISO.
fn perform_iso_delete(iso_path: &std::path::Path) -> Result<(), String> {
    std::fs::remove_file(iso_path).map_err(|e| format!("unlink ISO: {e}"))?;
    let sidecar = iso_probe::sidecar_path_for(iso_path);
    if sidecar.exists() {
        std::fs::remove_file(&sidecar).map_err(|e| format!("unlink sidecar: {e}"))?;
    }
    Ok(())
}

/// JSON record shape:
/// `{"timestamp_epoch": 1700000000, "iso_path": "...", "outcome": {...}}`
/// where `outcome` is the serialized [`iso_probe::HashVerification`].
fn save_verify_audit_log(
    iso_path: &std::path::Path,
    outcome: &iso_probe::HashVerification,
) -> Result<PathBuf, String> {
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs();
    let (year, month, day) = epoch_to_civil_date(ts_epoch);
    let filename = format!("{year:04}-{month:02}-{day:02}.jsonl");
    let record = serde_json::json!({
        "timestamp_epoch": ts_epoch,
        "iso_path": iso_path.display().to_string(),
        "outcome": outcome,
    });
    let line = format!("{record}\n");
    for base in ["/run/media/aegis-isos", "/tmp/aegis-verify-log"] {
        let dir = std::path::Path::new(base).join("verify-log");
        if std::fs::create_dir_all(&dir).is_err() {
            continue;
        }
        let out = dir.join(&filename);
        let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&out)
        else {
            continue;
        };
        if f.write_all(line.as_bytes()).is_err() {
            continue;
        }
        return Ok(out);
    }
    Err("no writable target (tried /run/media/aegis-isos, /tmp/aegis-verify-log)".into())
}

/// Write an error-screen evidence snapshot to the `AEGIS_ISOS` data
/// partition so the operator can retrieve it after reboot. Best-effort
/// — returns the path on success, an error message on failure.
/// Location: first writable directory in `[/run/media/aegis-isos,
/// /tmp]` with a timestamped filename. (#92)
fn save_error_log(text: &str) -> Result<PathBuf, String> {
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs();
    let filename = format!("aegis-log-{ts}.txt");
    for dir in ["/run/media/aegis-isos", "/tmp"] {
        let dir_path = std::path::Path::new(dir);
        if !dir_path.is_dir() {
            continue;
        }
        let out = dir_path.join(&filename);
        let Ok(mut f) = std::fs::File::create(&out) else {
            continue;
        };
        if f.write_all(text.as_bytes()).is_err() {
            continue;
        }
        return Ok(out);
    }
    Err("no writable target (tried /run/media/aegis-isos, /tmp)".into())
}

/// Message from the verify-now worker thread back to the event loop.
/// The worker is fire-and-forget after cancel — the channel is dropped
/// on the UI side and the thread exits when sends fail. (#89)
#[derive(Debug)]
enum VerifyMsg {
    /// Incremental progress update.
    Progress { bytes: u64, total: u64 },
    /// Worker finished. Contains either the [`iso_probe::HashVerification`]
    /// outcome (success path) or an I/O error.
    Done(Result<iso_probe::HashVerification, String>),
}

/// Kick off a verify-now worker thread for the ISO at the given path.
/// Returns the receiver end; caller installs it into `active_verify` so
/// the event loop can poll. The thread runs sha256 with periodic
/// progress ticks. (#89)
fn spawn_verify_worker(iso_path: PathBuf) -> Receiver<VerifyMsg> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let tx_progress = tx.clone();
        let result = iso_probe::verify_iso_hash_with_progress(&iso_path, |bytes, total| {
            // Send is best-effort; drop errors when the UI side
            // has cancelled and released its Receiver.
            let _ = tx_progress.send(VerifyMsg::Progress { bytes, total });
        });
        let payload = result.map_err(|e| e.to_string());
        let _ = tx.send(VerifyMsg::Done(payload));
    });
    rx
}

fn find_auto_kexec_target(isos: &[iso_probe::DiscoveredIso], needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    isos.iter()
        .position(|iso| iso.iso_path.to_string_lossy().contains(needle))
}

/// Non-interactive kexec path for automation. Matches the first ISO whose
/// path contains `needle` (substring match on the absolute path), then calls
/// `attempt_kexec`. Returns a meaningful exit code so CI can assert.
fn run_auto_kexec(state: &AppState, needle: &str) -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!(needle, "AEGIS_AUTO_KEXEC mode");
    let Some(idx) = find_auto_kexec_target(&state.isos, needle) else {
        tracing::error!(needle, "AEGIS_AUTO_KEXEC: no ISO path matched substring");
        return Err(format!("AEGIS_AUTO_KEXEC: no match for '{needle}'").into());
    };
    let Some(iso) = state.isos.get(idx).cloned() else {
        return Err("AEGIS_AUTO_KEXEC: index out of range".into());
    };
    tracing::info!(
        iso = %iso.iso_path.display(),
        idx,
        "AEGIS_AUTO_KEXEC: matched ISO, invoking kexec"
    );
    // Mirror attempt_kexec but without the state mutation — we're not
    // coming back from load_and_exec on success.
    let prepared = iso_probe::prepare(&iso)?;
    let cmdline = state
        .cmdline_overrides
        .get(&idx)
        .cloned()
        .or_else(|| prepared.cmdline.clone())
        .unwrap_or_default();
    let req = kexec_loader::KexecRequest {
        kernel: prepared.kernel.clone(),
        initrd: prepared.initrd.clone(),
        cmdline,
    };
    print_handoff_banner(&iso.label, &req);
    kexec_loader::load_and_exec(&req)
        .map(|_infallible| ())
        .map_err(|e| format!("kexec failed: {e}").into())
}

/// Apply any saved last-choice to the freshly-built [`AppState`]: pre-select
/// the matching ISO in the List and seed its cmdline override if one was
/// saved. Missing / corrupt / stale state is ignored.
fn apply_persisted_choice(state: &mut AppState) {
    // Opportunistic one-shot migration: if a previous tick wrote
    // last-choice to tmpfs because AEGIS_ISOS wasn't mounted yet,
    // drain it onto the data partition now that we're past the
    // initramfs mount stage. Best-effort — any failure leaves
    // state on tmpfs where load() will still find it.
    let _ = persistence::migrate_tmpfs_to_aegis_isos();

    let dir = persistence::default_state_dir();
    let Some(choice) = persistence::load(&dir) else {
        return;
    };
    let Some(idx) = state
        .isos
        .iter()
        .position(|iso| iso.iso_path == choice.iso_path)
    else {
        tracing::debug!(
            iso = %choice.iso_path.display(),
            "rescue-tui: persisted last-choice ISO not present in current discovery"
        );
        return;
    };
    tracing::info!(
        idx,
        iso = %choice.iso_path.display(),
        "rescue-tui: restored last choice"
    );
    state.screen = Screen::List { selected: idx };
    if let Some(override_) = choice.cmdline_override {
        state.cmdline_overrides.insert(idx, override_);
    }
}

fn save_last_choice(state: &AppState, idx: usize) {
    let Some(iso) = state.isos.get(idx) else {
        return;
    };
    let choice = persistence::LastChoice {
        iso_path: iso.iso_path.clone(),
        cmdline_override: state.cmdline_overrides.get(&idx).cloned(),
    };

    // Two writes per ADR 0003 §2:
    //   1. tmpfs (session-local, full fidelity incl. cmdline_override)
    //      — used by failed-kexec retry within the same boot.
    //   2. AEGIS_ISOS (cross-reboot, cmdline_override stripped) — used
    //      by the next boot to pre-position the cursor.
    //
    // Both are best-effort; either failure logs at debug and moves on.
    let dir = persistence::default_state_dir();
    if let Err(e) = persistence::save(&dir, &choice) {
        tracing::debug!(error = %e, "rescue-tui: last-choice tmpfs save failed (best-effort)");
    }
    if let Err(e) = persistence::save_durable(&choice) {
        tracing::debug!(error = %e, "rescue-tui: last-choice AEGIS_ISOS save failed (best-effort)");
    }
}

fn attempt_kexec(state: &mut AppState, idx: usize) {
    let Some(iso) = state.isos.get(idx).cloned() else {
        tracing::warn!(idx, "attempt_kexec called with out-of-range index");
        return;
    };
    tracing::info!(
        label = %iso.label,
        iso_path = %iso.iso_path.display(),
        "user confirmed kexec"
    );
    save_last_choice(state, idx);
    let prepared = match iso_probe::prepare(&iso) {
        Ok(p) => {
            tracing::info!(
                mount = %p.mount_point().display(),
                kernel = %p.kernel.display(),
                "prepared ISO for kexec"
            );
            p
        }
        Err(e) => {
            tracing::warn!(error = %e, "iso_probe::prepare failed");
            state.record_kexec_error(&kexec_loader::KexecError::Io(io::Error::other(
                e.to_string(),
            )));
            return;
        }
    };
    // User override takes precedence over the ISO-declared default; fall
    // back to whatever iso-probe extracted from the ISO's own boot config.
    let cmdline = state
        .cmdline_overrides
        .get(&idx)
        .cloned()
        .or_else(|| prepared.cmdline.clone())
        .unwrap_or_default();
    let req = kexec_loader::KexecRequest {
        kernel: prepared.kernel.clone(),
        initrd: prepared.initrd.clone(),
        cmdline,
    };
    // TPM PCR measurement: extend sha256(iso_path || 0x00 || cmdline)
    // into PCR 12 before kexec. Failure is logged but doesn't block —
    // rescue-tui may run on TPM-less hardware during physical-access
    // recovery. Full eventlog-style audit line emitted here so
    // downstream attestation can cross-reference the chosen boot with
    // the observed PCR change. (#93)
    let measurement = tpm::compute_measurement(&iso.iso_path, &req.cmdline);
    let measurement_hex = hex::encode(measurement);
    tracing::info!(
        event_type = "kexec_pre_measurement",
        pcr = tpm::DEFAULT_PCR,
        iso_path = %iso.iso_path.display(),
        cmdline = %req.cmdline,
        measurement = %measurement_hex,
        "audit: computed pre-kexec PCR 12 measurement (eventlog form)"
    );
    match tpm::extend_pcr(tpm::DEFAULT_PCR, &measurement) {
        Ok(hex) => tracing::info!(
            pcr = tpm::DEFAULT_PCR,
            measurement = %hex,
            "TPM: extended PCR with pre-kexec measurement"
        ),
        Err(e) => tracing::warn!(
            error = %e,
            "TPM: skipping pre-kexec measurement"
        ),
    }

    tracing::info!(
        kernel = %req.kernel.display(),
        initrd = ?req.initrd.as_ref().map(|p| p.display().to_string()),
        cmdline = %req.cmdline,
        "invoking kexec_file_load"
    );

    // #127: post-kexec handoff banner. Print to stderr (which the
    // terminal still has when the TUI exits alt-screen) so the
    // operator isn't staring at a black screen wondering if the
    // kexec fired. Survives in the framebuffer until the new
    // kernel's own output replaces it.
    print_handoff_banner(&iso.label, &req);

    // Drop guard: prepared lives until kexec_file_load + reboot replace the
    // process. On error, prepared drops here and unmounts.
    match kexec_loader::load_and_exec(&req) {
        Ok(_unreachable) => unreachable!("load_and_exec returns Infallible on success"),
        Err(e) => {
            tracing::error!(error = %e, "kexec failed");
            state.record_kexec_error(&e);
        }
    }
}

/// Clear-screen + banner printed right before `kexec_file_load` so the
/// operator sees "booting ..." instead of a blank screen. (#127)
fn print_handoff_banner(label: &str, req: &kexec_loader::KexecRequest) {
    // ANSI: clear screen + home cursor. Works on tty0, ttyS0, and xterm.
    eprint!("\x1b[2J\x1b[H");
    eprintln!("aegis-boot: invoking kexec...");
    eprintln!();
    eprintln!("  Booting: {label}");
    eprintln!("  Kernel:  {}", req.kernel.display());
    if let Some(ref initrd) = req.initrd {
        eprintln!("  Initrd:  {}", initrd.display());
    }
    eprintln!();
    eprintln!("The screen may go blank briefly while the new kernel loads.");
    eprintln!("If boot stalls, a classified error will appear here; otherwise");
    eprintln!("expect the ISO's own boot output within ~10 seconds.");
    eprintln!();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_roots_defaults_when_unset() {
        let r = parse_roots(None);
        assert_eq!(r, vec![PathBuf::from("/run/media"), PathBuf::from("/mnt")]);
    }

    #[test]
    fn parse_roots_defaults_when_empty_string() {
        let r = parse_roots(Some(""));
        assert_eq!(r, vec![PathBuf::from("/run/media"), PathBuf::from("/mnt")]);
    }

    #[test]
    fn parse_roots_splits_on_colon() {
        let r = parse_roots(Some("/a:/b:/c"));
        assert_eq!(
            r,
            vec![
                PathBuf::from("/a"),
                PathBuf::from("/b"),
                PathBuf::from("/c"),
            ]
        );
    }

    // ---- count_iso_files_on_disk (#623 dedup overlapping roots) ------

    #[test]
    fn count_iso_files_on_disk_dedupes_overlapping_roots() {
        // Reproduces #623: /init exports
        // AEGIS_ISO_ROOTS=/run/media/aegis-isos:/run/media — the
        // second root is a parent of the first, so a naive sum of
        // walks would count each ISO twice.
        let tmp = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let inner = tmp.path().join("aegis-isos");
        std::fs::create_dir(&inner).unwrap_or_else(|e| panic!("create_dir: {e}"));
        for name in ["alpine.iso", "ubuntu.iso", "win11.iso"] {
            std::fs::write(inner.join(name), b"x").unwrap_or_else(|e| panic!("write {name}: {e}"));
        }

        let roots = vec![inner.clone(), tmp.path().to_path_buf()];
        // 3 unique ISOs, even though the recursive walks visit each
        // file from both the inner and outer roots.
        assert_eq!(count_iso_files_on_disk(&roots), 3);
    }

    #[test]
    fn count_iso_files_on_disk_skips_missing_roots() {
        let tmp = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        std::fs::write(tmp.path().join("foo.iso"), b"x").unwrap_or_else(|e| panic!("write: {e}"));
        let roots = vec![
            tmp.path().to_path_buf(),
            PathBuf::from("/nonexistent/aegis-test-root"),
        ];
        assert_eq!(count_iso_files_on_disk(&roots), 1);
    }

    // ---- epoch_to_civil_date (#548 verify-audit-log) -----------------

    #[test]
    fn epoch_to_civil_date_unix_epoch() {
        // 1970-01-01 00:00:00 UTC.
        assert_eq!(epoch_to_civil_date(0), (1970, 1, 1));
    }

    #[test]
    fn epoch_to_civil_date_known_dates() {
        // Sanity-check several known epoch boundaries to pin Hinnant's
        // algorithm against any future refactor that introduces an
        // off-by-one in the leap-year math.
        // 2000-01-01 00:00:00 = 946_684_800
        assert_eq!(epoch_to_civil_date(946_684_800), (2000, 1, 1));
        // 2000-02-29 00:00:00 = 951_782_400 (a Y2k leap day)
        assert_eq!(epoch_to_civil_date(951_782_400), (2000, 2, 29));
        // 2024-02-29 00:00:00 = 1_709_164_800 (recent leap day)
        assert_eq!(epoch_to_civil_date(1_709_164_800), (2024, 2, 29));
        // 2026-04-25 00:00:00 = 1_777_075_200 (today, around when this PR
        // was authored — pins the audit-log filename format under CI's
        // synthetic SOURCE_DATE_EPOCH if anyone wires that through).
        assert_eq!(epoch_to_civil_date(1_777_075_200), (2026, 4, 25));
    }

    #[test]
    fn epoch_to_civil_date_intra_day_returns_same_ymd() {
        // Any timestamp within the same UTC day must yield the same YMD —
        // the audit-log file rotates per-day, not per-second.
        let d_2026_04_25 = epoch_to_civil_date(1_777_075_200); // midnight
        let d_2026_04_25_noon = epoch_to_civil_date(1_777_075_200 + 12 * 3600);
        let d_2026_04_25_2359 = epoch_to_civil_date(1_777_075_200 + 86_399);
        assert_eq!(d_2026_04_25, d_2026_04_25_noon);
        assert_eq!(d_2026_04_25, d_2026_04_25_2359);
    }

    #[test]
    fn epoch_to_civil_date_rolls_over_at_utc_midnight() {
        let last_sec_of_day = epoch_to_civil_date(1_777_075_200 + 86_399);
        let first_sec_of_next = epoch_to_civil_date(1_777_075_200 + 86_400);
        assert_eq!(last_sec_of_day, (2026, 4, 25));
        assert_eq!(first_sec_of_next, (2026, 4, 26));
    }

    fn fake_iso_at(path: &str) -> iso_probe::DiscoveredIso {
        iso_probe::DiscoveredIso {
            iso_path: PathBuf::from(path),
            label: path.to_string(),
            distribution: iso_probe::Distribution::Unknown,
            kernel: PathBuf::from("vmlinuz"),
            initrd: None,
            cmdline: None,
            quirks: vec![],
            hash_verification: iso_probe::HashVerification::NotPresent,
            signature_verification: iso_probe::SignatureVerification::NotPresent,
            size_bytes: None,
            contains_installer: false,
            pretty_name: None,
            sidecar: None,
        }
    }

    #[test]
    fn find_auto_kexec_target_returns_none_when_no_match() {
        let isos = vec![fake_iso_at("/run/media/ubuntu.iso")];
        assert!(find_auto_kexec_target(&isos, "fedora").is_none());
    }

    #[test]
    fn find_auto_kexec_target_returns_first_substring_match() {
        let isos = vec![
            fake_iso_at("/run/media/ubuntu-24.04.iso"),
            fake_iso_at("/run/media/fedora-39.iso"),
            fake_iso_at("/run/media/fedora-40.iso"),
        ];
        assert_eq!(find_auto_kexec_target(&isos, "fedora"), Some(1));
    }

    #[test]
    fn find_auto_kexec_target_returns_none_for_empty_needle() {
        // Empty needle would match every path via String::contains; reject
        // explicitly so AEGIS_AUTO_KEXEC="" doesn't silently boot the first
        // ISO it finds.
        let isos = vec![fake_iso_at("/run/media/anything.iso")];
        assert!(find_auto_kexec_target(&isos, "").is_none());
    }

    #[test]
    fn find_auto_kexec_target_handles_empty_iso_list() {
        assert!(find_auto_kexec_target(&[], "anything").is_none());
    }
}
