//! `aegis-manifest-schema-docgen` — emits JSON Schema documents for
//! every public wire-format type in `aegis-manifest` into
//! `docs/reference/schemas/` in the parent workspace.
//!
//! Targets today:
//!
//! * [`aegis_manifest::Manifest`] →
//!   `docs/reference/schemas/aegis-boot-manifest.schema.json`
//!   (Phase 4a of [#286])
//! * [`aegis_manifest::Attestation`] →
//!   `docs/reference/schemas/aegis-attestation.schema.json`
//!   (Phase 4c-1 of [#286])
//! * [`aegis_manifest::Version`] →
//!   `docs/reference/schemas/aegis-boot-version.schema.json`
//!   (Phase 4b-1 of [#286])
//! * [`aegis_manifest::ListReport`] →
//!   `docs/reference/schemas/aegis-boot-list.schema.json`
//!   (Phase 4b-2 of [#286])
//! * [`aegis_manifest::AttestListReport`] →
//!   `docs/reference/schemas/aegis-boot-attest-list.schema.json`
//!   (Phase 4b-3 of [#286])
//!
//! CI's `manifest-schema-drift` job runs this in `--check` mode on
//! every PR. Any time a field is added, removed, or retyped on any
//! of the covered structs, this tool regenerates a different
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

use aegis_manifest::{AttestListReport, Attestation, ListReport, Manifest, Version};
use schemars::schema_for;

/// A single schema target: logical name, relative output path, and
/// a closure that renders the JSON Schema body.
struct Target {
    name: &'static str,
    relative_path: &'static str,
    render: fn() -> Result<String, String>,
}

fn targets() -> Vec<Target> {
    vec![
        Target {
            name: "Manifest",
            relative_path: "docs/reference/schemas/aegis-boot-manifest.schema.json",
            render: render_manifest_schema,
        },
        Target {
            name: "Attestation",
            relative_path: "docs/reference/schemas/aegis-attestation.schema.json",
            render: render_attestation_schema,
        },
        Target {
            name: "Version",
            relative_path: "docs/reference/schemas/aegis-boot-version.schema.json",
            render: render_version_schema,
        },
        Target {
            name: "ListReport",
            relative_path: "docs/reference/schemas/aegis-boot-list.schema.json",
            render: render_list_schema,
        },
        Target {
            name: "AttestListReport",
            relative_path: "docs/reference/schemas/aegis-boot-attest-list.schema.json",
            render: render_attest_list_schema,
        },
    ]
}

fn render_manifest_schema() -> Result<String, String> {
    render_pretty(&schema_for!(Manifest))
}

fn render_attestation_schema() -> Result<String, String> {
    render_pretty(&schema_for!(Attestation))
}

fn render_version_schema() -> Result<String, String> {
    render_pretty(&schema_for!(Version))
}

fn render_list_schema() -> Result<String, String> {
    render_pretty(&schema_for!(ListReport))
}

fn render_attest_list_schema() -> Result<String, String> {
    render_pretty(&schema_for!(AttestListReport))
}

/// Serialize a JSON Schema as pretty-printed JSON with a trailing
/// newline. The trailing newline is important for clean diffs and
/// Unix-tool friendliness.
fn render_pretty<T: serde::Serialize>(schema: &T) -> Result<String, String> {
    let mut body = serde_json::to_string_pretty(schema)
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

    let mut drift_targets: Vec<&'static str> = Vec::new();
    let mut wrote = 0usize;

    for target in targets() {
        let rendered = match (target.render)() {
            Ok(s) => s,
            Err(msg) => {
                eprintln!("{msg}");
                return ExitCode::from(2);
            }
        };
        let out_path = repo_root.join(target.relative_path);

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
                println!(
                    "schema-docgen: wrote {} ({} schema)",
                    out_path.display(),
                    target.name
                );
                wrote += 1;
            }
            Mode::Check => {
                let committed = fs::read_to_string(&out_path).unwrap_or_default();
                if committed == rendered {
                    println!(
                        "schema-docgen: OK — {} matches `schema_for!({})` output",
                        out_path.display(),
                        target.name
                    );
                } else {
                    eprintln!(
                        "schema-docgen: DRIFT — regenerated {} schema differs from committed copy at {}",
                        target.name,
                        out_path.display()
                    );
                    drift_targets.push(target.name);
                }
            }
        }
    }

    match mode {
        Mode::Write => {
            println!("schema-docgen: wrote {wrote} schema file(s)");
            ExitCode::SUCCESS
        }
        Mode::Check => {
            if drift_targets.is_empty() {
                ExitCode::SUCCESS
            } else {
                eprintln!();
                eprintln!(
                    "schema-docgen: FAIL — {} schema(s) diverge: {}",
                    drift_targets.len(),
                    drift_targets.join(", ")
                );
                eprintln!(
                    "Fix: run `cargo run -p aegis-manifest --bin aegis-manifest-schema-docgen --features schema -- --write` locally and commit the result."
                );
                ExitCode::from(1)
            }
        }
    }
}
