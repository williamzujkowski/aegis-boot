//! `rescue-tui` — ratatui application shown inside the aegis-boot signed Linux
//! rescue environment. Lets the user pick a discovered ISO and hands off to
//! `kexec-loader`.
//!
//! See [ADR 0001](../../../../docs/adr/0001-runtime-architecture.md).

#![forbid(unsafe_code)]

fn main() {
    // TODO(#4): initialize tracing, run iso_probe::discover, render ratatui,
    // read user selection, invoke kexec_loader::load_and_exec.
    eprintln!("rescue-tui: skeleton only. See ADR 0001.");
    std::process::exit(1);
}
