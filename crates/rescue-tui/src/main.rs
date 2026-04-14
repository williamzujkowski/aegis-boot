//! `rescue-tui` — ratatui application shown inside the aegis-boot signed Linux
//! rescue environment. Discovers ISOs via `iso-probe`, lets the user pick one,
//! and hands off to `kexec-loader`.
//!
//! See [ADR 0001](../../../../docs/adr/0001-runtime-architecture.md) for the
//! Secure Boot rationale.

#![forbid(unsafe_code)]

mod render;
mod state;

use std::env;
use std::io;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
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
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("rescue-tui: {e}");
            ExitCode::from(1)
        }
    }
}

fn tracing_subscriber_init() {
    // Logs to stderr only if RUST_LOG is set. The TUI owns stdout.
    let _ = tracing_subscriber::fmt()
        .with_writer(io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .try_init();
}

fn parse_roots(env_var: Option<&str>) -> Vec<PathBuf> {
    match env_var {
        Some(s) if !s.is_empty() => s.split(':').map(PathBuf::from).collect(),
        _ => DEFAULT_ROOTS.iter().map(PathBuf::from).collect(),
    }
}

fn run(roots: &[PathBuf]) -> Result<(), Box<dyn std::error::Error>> {
    let isos = match iso_probe::discover(roots) {
        Ok(v) => v,
        Err(iso_probe::ProbeError::NoIsosFound) => Vec::new(),
        Err(e) => return Err(e.into()),
    };
    let mut state = AppState::new(isos);

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

        match (&state.screen, key.code) {
            (_, KeyCode::Char('q')) => state.quit(),
            (Screen::List { .. }, KeyCode::Up) => state.move_selection(-1),
            (Screen::List { .. }, KeyCode::Down) => state.move_selection(1),
            (Screen::List { .. }, KeyCode::Enter) => state.confirm_selection(),
            (Screen::Confirm { .. }, KeyCode::Esc) => state.cancel_confirmation(),
            (Screen::Confirm { selected }, KeyCode::Enter) => {
                let idx = *selected;
                attempt_kexec(state, idx);
            }
            (Screen::Error { .. }, _) => {
                state.screen = Screen::List { selected: 0 };
            }
            _ => {}
        }
    }
}

fn attempt_kexec(state: &mut AppState, idx: usize) {
    let Some(iso) = state.isos.get(idx).cloned() else {
        return;
    };
    let prepared = match iso_probe::prepare(&iso) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("iso_probe::prepare failed: {e}");
            state.record_kexec_error(&kexec_loader::KexecError::Io(io::Error::other(
                e.to_string(),
            )));
            return;
        }
    };
    let req = kexec_loader::KexecRequest {
        kernel: prepared.kernel.clone(),
        initrd: prepared.initrd.clone(),
        cmdline: prepared.cmdline.clone().unwrap_or_default(),
    };
    // Drop guard: prepared lives until kexec_file_load + reboot replace the
    // process. On error, prepared drops here and unmounts.
    match kexec_loader::load_and_exec(&req) {
        Ok(_unreachable) => unreachable!("load_and_exec returns Infallible on success"),
        Err(e) => state.record_kexec_error(&e),
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
}
