// SPDX-License-Identifier: MIT OR Apache-2.0

//! `tui-screenshots` — dev tool that renders rescue-tui fixtures to
//! ANSI-escaped stdout so an operator (or reviewer) can visually
//! inspect the new dual-pane UI without building + booting an aegis-
//! boot stick.
//!
//! Produces one "screenshot" per interesting scenario (empty list,
//! mixed tiers, tier-4 selected, tier-5 selected, focus states, etc.)
//! separated by a human-readable banner. The output is terminal-ready:
//! `cargo run -p rescue-tui --bin tui-screenshots | less -R`, or pipe
//! through `aha` / `ansi2html` to get colored HTML for non-terminal
//! review.
//!
//! This is a dev aid only — not installed, not part of the boot chain.

#![forbid(unsafe_code)]

use std::io::Write;
use std::path::PathBuf;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};

use rescue_tui::render::draw;
use rescue_tui::state::{AppState, Pane, Screen};

type DynError = Box<dyn std::error::Error>;

fn main() -> Result<(), DynError> {
    let scenarios: Vec<(&str, &str, Box<dyn Fn() -> AppState>)> = vec![
        (
            "01-empty-list",
            "Empty stick — no ISOs, no failed parses. Rescue-shell entry still available.",
            Box::new(empty_state),
        ),
        (
            "02-mixed-tiers-list-focused",
            "Six ISOs spanning tiers 1/2/3/4/5/6. List pane focused (default).",
            Box::new(|| mixed_tier_state(Pane::List, 0)),
        ),
        (
            "03-mixed-tiers-info-focused",
            "Same list, info pane focused via Tab. Notice the border swap + list dim.",
            Box::new(|| mixed_tier_state(Pane::Info, 0)),
        ),
        (
            "04-tier4-parse-failed-selected",
            "Tier 4 (ParseFailed) row selected. Info pane shows the sanitized reason and disables boot.",
            Box::new(|| mixed_tier_state(Pane::List, 3)),
        ),
        (
            "05-tier5-secure-boot-blocked",
            "Tier 5 (SecureBootBlocked) — Windows ISO selected. Info pane names the reason.",
            Box::new(|| mixed_tier_state(Pane::List, 4)),
        ),
        (
            "06-tier6-hash-mismatch",
            "Tier 6 (HashMismatch) — tamper signal. Info pane shows expected vs actual digests.",
            Box::new(|| mixed_tier_state(Pane::List, 5)),
        ),
        (
            "07-filter-editing",
            "Filter input active. Typed 'ubuntu' to narrow the list.",
            Box::new(filter_editing_state),
        ),
        (
            "08-help-overlay",
            "Help overlay (?) — registry-driven keybinding reference.",
            Box::new(help_state),
        ),
        (
            "09-confirm-screen",
            "Confirm screen — one-frame evidence for the selected ISO.",
            Box::new(confirm_state),
        ),
        (
            "10-trust-challenge",
            "Typed-confirmation challenge for tier-2/3 ISOs.",
            Box::new(trust_challenge_state),
        ),
    ];

    let mut stdout = std::io::stdout().lock();
    for (slug, caption, builder) in &scenarios {
        let state = builder();
        writeln!(stdout, "{}", banner(slug, caption))?;
        let rendered = render_ansi(&state, 120, 30);
        writeln!(stdout, "{rendered}")?;
    }
    Ok(())
}

fn banner(slug: &str, caption: &str) -> String {
    let line = "=".repeat(120);
    format!("\n\x1b[1;33m{line}\x1b[0m\n\x1b[1m[{slug}]\x1b[0m {caption}\n\x1b[1;33m{line}\x1b[0m")
}

/// Render a single `AppState` via `TestBackend` and convert the
/// resulting buffer to ANSI-escaped text. Output is terminal-ready:
/// each cell's style becomes the corresponding escape sequence, and
/// every line is reset before the newline.
fn render_ansi(state: &AppState, cols: u16, rows: u16) -> String {
    let backend = TestBackend::new(cols, rows);
    let mut term = Terminal::new(backend).expect("terminal");
    term.draw(|f| draw(f, state)).expect("draw");
    buffer_to_ansi(term.backend().buffer(), cols)
}

fn buffer_to_ansi(buf: &Buffer, cols: u16) -> String {
    let mut out = String::new();
    for (i, cell) in buf.content.iter().enumerate() {
        if i > 0 && (i as u16).is_multiple_of(cols) {
            out.push_str("\x1b[0m\n");
        }
        push_style(&mut out, cell.fg, cell.bg, cell.modifier);
        out.push_str(cell.symbol());
    }
    out.push_str("\x1b[0m");
    out
}

fn push_style(out: &mut String, fg: Color, bg: Color, modifier: Modifier) {
    out.push_str("\x1b[0m");
    if modifier.contains(Modifier::BOLD) {
        out.push_str("\x1b[1m");
    }
    if modifier.contains(Modifier::DIM) {
        out.push_str("\x1b[2m");
    }
    if modifier.contains(Modifier::REVERSED) {
        out.push_str("\x1b[7m");
    }
    out.push_str(&fg_code(fg));
    out.push_str(&bg_code(bg));
}

fn fg_code(c: Color) -> String {
    match c {
        Color::Reset => String::new(),
        Color::Black => "\x1b[30m".to_string(),
        Color::Red => "\x1b[31m".to_string(),
        Color::Green => "\x1b[32m".to_string(),
        Color::Yellow => "\x1b[33m".to_string(),
        Color::Blue => "\x1b[34m".to_string(),
        Color::Magenta => "\x1b[35m".to_string(),
        Color::Cyan => "\x1b[36m".to_string(),
        Color::Gray => "\x1b[37m".to_string(),
        Color::DarkGray => "\x1b[90m".to_string(),
        Color::LightRed => "\x1b[91m".to_string(),
        Color::LightGreen => "\x1b[92m".to_string(),
        Color::LightYellow => "\x1b[93m".to_string(),
        Color::LightBlue => "\x1b[94m".to_string(),
        Color::LightMagenta => "\x1b[95m".to_string(),
        Color::LightCyan => "\x1b[96m".to_string(),
        Color::White => "\x1b[97m".to_string(),
        Color::Rgb(r, g, b) => format!("\x1b[38;2;{r};{g};{b}m"),
        Color::Indexed(i) => format!("\x1b[38;5;{i}m"),
    }
}

fn bg_code(c: Color) -> String {
    match c {
        Color::Reset => String::new(),
        Color::Rgb(r, g, b) => format!("\x1b[48;2;{r};{g};{b}m"),
        Color::Indexed(i) => format!("\x1b[48;5;{i}m"),
        _ => String::new(),
    }
}

// ---- Fixtures --------------------------------------------------------

fn empty_state() -> AppState {
    AppState::new(Vec::new())
}

fn mixed_tier_state(pane: Pane, selected: usize) -> AppState {
    let isos = vec![
        fake_tier1(),
        fake_tier2(),
        fake_tier3(),
        // Tier 4 comes from failed_isos, not isos. Selected index 3
        // lands on it via the visible_entries ordering.
        fake_tier5_windows(),
        fake_tier6_mismatch(),
    ];
    let failed = vec![fake_tier4_failed()];
    let mut state = AppState::new(isos).with_failed_isos(failed);
    state.pane = pane;
    state.screen = Screen::List { selected };
    state
}

fn filter_editing_state() -> AppState {
    let mut state = mixed_tier_state(Pane::List, 0);
    state.filter_editing = true;
    state.filter = "ubuntu".to_string();
    state
}

fn help_state() -> AppState {
    let inner = mixed_tier_state(Pane::List, 0);
    let mut state = inner.clone();
    state.screen = Screen::Help {
        prior: Box::new(inner.screen),
    };
    state
}

fn confirm_state() -> AppState {
    let mut state = mixed_tier_state(Pane::List, 0);
    state.screen = Screen::Confirm { selected: 0 };
    state
}

fn trust_challenge_state() -> AppState {
    let mut state = mixed_tier_state(Pane::List, 1);
    state.screen = Screen::TrustChallenge {
        selected: 1,
        buffer: "boo".to_string(),
    };
    state
}

// ---- ISO fixtures ---------------------------------------------------

fn base_iso(name: &str) -> iso_probe::DiscoveredIso {
    iso_probe::DiscoveredIso {
        iso_path: PathBuf::from(format!("/run/media/aegis-isos/{name}.iso")),
        label: name.to_string(),
        pretty_name: None,
        distribution: iso_probe::Distribution::Debian,
        kernel: PathBuf::from("casper/vmlinuz"),
        initrd: Some(PathBuf::from("casper/initrd")),
        cmdline: Some("boot=casper quiet splash".to_string()),
        quirks: vec![],
        hash_verification: iso_probe::HashVerification::NotPresent,
        signature_verification: iso_probe::SignatureVerification::NotPresent,
        size_bytes: Some(2_470_000_000),
        contains_installer: false,
        sidecar: None,
    }
}

fn fake_tier1() -> iso_probe::DiscoveredIso {
    let mut iso = base_iso("ubuntu-24.04-live-server");
    iso.pretty_name = Some("Ubuntu 24.04.2 LTS (Noble Numbat)".to_string());
    iso.hash_verification = iso_probe::HashVerification::Verified {
        digest: "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2".to_string(),
        source: "/run/media/aegis-isos/ubuntu-24.04-live-server.iso.sha256".to_string(),
    };
    iso.signature_verification = iso_probe::SignatureVerification::Verified {
        key_id: "aegis-catalog-2026".to_string(),
        sig_path: PathBuf::from("/run/media/aegis-isos/ubuntu-24.04-live-server.iso.minisig"),
    };
    iso
}

fn fake_tier2() -> iso_probe::DiscoveredIso {
    let mut iso = base_iso("alpine-standard-3.20");
    iso.distribution = iso_probe::Distribution::Alpine;
    iso.quirks = vec![iso_probe::Quirk::UnsignedKernel];
    iso.size_bytes = Some(200_000_000);
    iso
}

fn fake_tier3() -> iso_probe::DiscoveredIso {
    let mut iso = base_iso("archlinux-2026.04.01-x86_64");
    iso.distribution = iso_probe::Distribution::Arch;
    iso.size_bytes = Some(980_000_000);
    iso.signature_verification = iso_probe::SignatureVerification::KeyNotTrusted {
        key_id: "7fabc9d2e1fa4ac2".to_string(),
    };
    iso.quirks = vec![iso_probe::Quirk::UnsignedKernel];
    iso
}

fn fake_tier4_failed() -> iso_probe::FailedIso {
    iso_probe::FailedIso {
        iso_path: PathBuf::from("/run/media/aegis-isos/my-custom-build.iso"),
        reason:
            "mount failed: /dev/loop7: wrong fs type, bad option, bad superblock on /dev/loop7, \
             missing codepage or helper program, or other error"
                .to_string(),
        kind: iso_probe::FailureKind::MountFailed,
    }
}

fn fake_tier5_windows() -> iso_probe::DiscoveredIso {
    let mut iso = base_iso("Win11_25H2_EnglishInternational_x64");
    iso.distribution = iso_probe::Distribution::Windows;
    iso.kernel = PathBuf::from("bootmgr");
    iso.initrd = None;
    iso.cmdline = None;
    iso.quirks = vec![iso_probe::Quirk::NotKexecBootable];
    iso.size_bytes = Some(5_200_000_000);
    iso
}

fn fake_tier6_mismatch() -> iso_probe::DiscoveredIso {
    let mut iso = base_iso("debian-12.5.0-amd64-netinst");
    iso.hash_verification = iso_probe::HashVerification::Mismatch {
        expected: "deadbeefcafef00dbadc0ffee0ddf00dabad1deabaddcafef00dbeefcafef00d1".to_string(),
        actual: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string(),
        source: "/run/media/aegis-isos/debian-12.5.0-amd64-netinst.iso.sha256".to_string(),
    };
    iso.size_bytes = Some(380_000_000);
    iso
}
