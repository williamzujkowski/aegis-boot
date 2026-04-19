//! `aegis-manifest-schema-docgen` — emits the JSON Schema document
//! for [`aegis_manifest::Manifest`] to
//! `docs/reference/schemas/aegis-boot-manifest.schema.json` in the
//! parent workspace.
//!
//! Phase 4a of [#286] / [#290]. CI's
//! `aegis-manifest-schema-drift` job runs this in `--check` mode on
//! every PR. Any time a field is added, removed, or retyped on any
//! of the manifest structs, this tool regenerates a different
//! document and the drift-check fails until the committed schema
//! catches up.
//!
//! ## Why the schema is committed rather than generated-at-release
//!
//! Third-party verifiers pin against the committed schema file.
//! Generating at release time would introduce a window between a
//! shape-changing PR merge and a new release where the on-disk
//! schema and committed schema diverge silently. Making the schema
//! a gating check at PR time closes that window.
//!
//! [#286]: https://github.com/williamzujkowski/aegis-boot/issues/286
//! [#290]: https://github.com/williamzujkowski/aegis-boot/issues/290

#![forbid(unsafe_code)]

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use aegis_manifest::Manifest;
use schemars::schema_for;

/// Render the JSON Schema for [`Manifest`] as pretty-printed JSON
/// with a trailing newline. The trailing newline is important for
/// clean diffs and Unix-tool friendliness.
fn render_schema() -> Result<String, String> {
    let schema = schema_for!(Manifest);
    let mut body = serde_json::to_string_pretty(&schema)
        .map_err(|e| format!("schema-docgen: cannot serialize schema: {e}"))?;
    body.push('\n');
    Ok(body)
}

/// Walk up from `start` until a workspace Cargo.toml is found.
/// Same pattern as `constants-docgen` / `cli-docgen`.
fn find_repo_root(start: &Path) -> Result<PathBuf, String> {
    let mut cur = start;
    loop {
        let candidate = cur.join("Cargo.toml");
        if candidate.is_file() {
            if let Ok(body) = fs::read_to_string(&candidate) {
                if body.contains("[workspace]") {
                    return Ok(cur.to_path_buf());
                }
            }
        }
        match cur.parent() {
            Some(parent) => cur = parent,
            None => {
                return Err(format!(
                    "schema-docgen: could not find workspace Cargo.toml walking up from {}",
                    start.display()
                ));
            }
        }
    }
}

enum Mode {
    Write,
    Check,
}

fn parse_mode(args: &[String]) -> Result<Mode, String> {
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        [] | ["--check"] => Ok(Mode::Check),
        ["--write"] => Ok(Mode::Write),
        _ => Err(format!(
            "usage: aegis-manifest-schema-docgen [--check|--write]  (got: {args:?})"
        )),
    }
}

fn main() -> ExitCode {
    // Devtool — no security-relevant argv. Same rationale as the
    // other workspace docgens.
    // nosemgrep: rust.lang.security.args.args
    let args: Vec<String> = env::args().skip(1).collect();
    let mode = match parse_mode(&args) {
        Ok(m) => m,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::from(2);
        }
    };

    let cwd = match env::current_dir() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("schema-docgen: cannot read CWD: {e}");
            return ExitCode::from(2);
        }
    };
    let repo_root = match find_repo_root(&cwd) {
        Ok(p) => p,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::from(2);
        }
    };

    let rendered = match render_schema() {
        Ok(s) => s,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::from(2);
        }
    };

    let out_path = repo_root.join("docs/reference/schemas/aegis-boot-manifest.schema.json");

    match mode {
        Mode::Write => {
            if let Some(parent) = out_path.parent() {
                if let Err(e) = fs::create_dir_all(parent) {
                    eprintln!("schema-docgen: cannot create {}: {e}", parent.display());
                    return ExitCode::from(2);
                }
            }
            if let Err(e) = fs::write(&out_path, &rendered) {
                eprintln!("schema-docgen: cannot write {}: {e}", out_path.display());
                return ExitCode::from(2);
            }
            println!("schema-docgen: wrote {}", out_path.display());
            ExitCode::SUCCESS
        }
        Mode::Check => {
            let committed = fs::read_to_string(&out_path).unwrap_or_default();
            if committed == rendered {
                println!(
                    "schema-docgen: OK — {} matches `schema_for!(Manifest)` output",
                    out_path.display()
                );
                ExitCode::SUCCESS
            } else {
                eprintln!(
                    "schema-docgen: DRIFT — regenerated schema differs from committed copy at {}",
                    out_path.display()
                );
                eprintln!(
                    "Fix: run `cargo run -p aegis-manifest --bin aegis-manifest-schema-docgen --features schema -- --write` locally and commit the result."
                );
                ExitCode::from(1)
            }
        }
    }
}
