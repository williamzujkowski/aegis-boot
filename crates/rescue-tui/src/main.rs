//! `rescue-tui` — ratatui application shown inside the aegis-boot signed Linux
//! rescue environment. Discovers ISOs via `iso-probe`, lets the user pick one,
//! and hands off to `kexec-loader`.
//!
//! See [ADR 0001](../../../../docs/adr/0001-runtime-architecture.md) for the
//! Secure Boot rationale.

#![forbid(unsafe_code)]

mod persistence;
mod render;
mod state;
mod theme;
mod tpm;

use std::env;
use std::io;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::state::{AppState, Screen};

/// Default search roots when `AEGIS_ISO_ROOTS` is not set.
const DEFAULT_ROOTS: &[&str] = &["/run/media", "/mnt"];

fn main() -> ExitCode {
    tracing_subscriber_init();
    let roots = parse_roots(env::var("AEGIS_ISO_ROOTS").ok().as_deref());
    match run(&roots) {
        Ok(()) => ExitCode::SUCCESS,
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

fn parse_roots(env_var: Option<&str>) -> Vec<PathBuf> {
    match env_var {
        Some(s) if !s.is_empty() => s.split(':').map(PathBuf::from).collect(),
        _ => DEFAULT_ROOTS.iter().map(PathBuf::from).collect(),
    }
}

fn run(roots: &[PathBuf]) -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        roots = ?roots,
        "rescue-tui starting"
    );
    let isos = match iso_probe::discover(roots) {
        Ok(v) => v,
        Err(iso_probe::ProbeError::NoIsosFound) => {
            tracing::info!("no ISOs discovered under any root");
            Vec::new()
        }
        Err(e) => {
            tracing::error!(error = %e, "ISO discovery failed");
            return Err(e.into());
        }
    };
    // Startup banner to stderr — mirrored via tracing::info! so structured
    // consumers (journald, CI smoke greps) see the same signal as humans
    // reading the serial console directly.
    eprintln!(
        "aegis-boot rescue-tui starting: discovered {} ISO(s)",
        isos.len()
    );
    tracing::info!(discovered = isos.len(), "ISO discovery complete");
    for iso in &isos {
        tracing::debug!(
            label = %iso.label,
            path = %iso.iso_path.display(),
            distribution = ?iso.distribution,
            quirks = ?iso.quirks,
            "discovered ISO"
        );
    }
    let mut state = AppState::new(isos);
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
        return run_auto_kexec(&state, &needle);
    }

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = event_loop(&mut terminal, &mut state);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn event_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    state: &mut AppState,
) -> Result<(), Box<dyn std::error::Error>> {
    loop {
        terminal.draw(|f| render::draw(f, state))?;

        if state.screen == Screen::Quitting {
            return Ok(());
        }

        if !event::poll(Duration::from_millis(250))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
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
        //   q  → quit confirmation prompt (was instant exit before #85)
        //   ?  → help overlay
        if !in_editor {
            match key.code {
                KeyCode::Char('q') => {
                    state.request_quit();
                    continue;
                }
                KeyCode::Char('?') => {
                    state.open_help();
                    continue;
                }
                _ => {}
            }
        }

        match (&state.screen, key.code) {
            // Vim navigation aliases for arrow keys (#85).
            (Screen::List { .. }, KeyCode::Up | KeyCode::Char('k')) => state.move_selection(-1),
            (Screen::List { .. }, KeyCode::Down | KeyCode::Char('j')) => state.move_selection(1),
            (Screen::List { .. }, KeyCode::Char('g')) => state.move_to_first(),
            (Screen::List { .. }, KeyCode::Char('G')) => state.move_to_last(),
            (Screen::List { .. }, KeyCode::Enter | KeyCode::Char('l')) => {
                state.confirm_selection();
            }
            (Screen::List { .. }, KeyCode::Char('/')) => state.open_filter(),
            (Screen::List { .. }, KeyCode::Char('s')) => state.cycle_sort(),

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
                } else {
                    attempt_kexec(state, idx);
                }
            }

            (Screen::EditCmdline { .. }, KeyCode::Enter) => state.commit_cmdline_edit(),
            (Screen::EditCmdline { .. }, KeyCode::Esc) => state.cancel_cmdline_edit(),
            (Screen::EditCmdline { .. }, KeyCode::Left) => state.cmdline_cursor_left(),
            (Screen::EditCmdline { .. }, KeyCode::Right) => state.cmdline_cursor_right(),
            (Screen::EditCmdline { .. }, KeyCode::Backspace) => state.cmdline_backspace(),
            (Screen::EditCmdline { .. }, KeyCode::Char(c)) => state.cmdline_insert(c),

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

/// Find the first ISO whose path contains `needle` (substring match on the
/// absolute path). Pure helper for `AEGIS_AUTO_KEXEC`; extracted for unit
/// testing without spinning up QEMU. (#54)
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
    kexec_loader::load_and_exec(&req)
        .map(|_infallible| ())
        .map_err(|e| format!("kexec failed: {e}").into())
}

/// Apply any saved last-choice to the freshly-built [`AppState`]: pre-select
/// the matching ISO in the List and seed its cmdline override if one was
/// saved. Missing / corrupt / stale state is ignored.
fn apply_persisted_choice(state: &mut AppState) {
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
    let dir = persistence::default_state_dir();
    if let Err(e) = persistence::save(&dir, &choice) {
        tracing::debug!(error = %e, "rescue-tui: last-choice save failed (best-effort)");
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
