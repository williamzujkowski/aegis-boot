// Build scripts fail the build via panic on error — the workspace-
// wide `unwrap_used`/`expect_used = deny` is inappropriate here (a
// build-time panic IS the correct failure mode).
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Reads `keys/canonical-epoch.json` at build time and emits
//! `cargo:rustc-env=AEGIS_MIN_REQUIRED_EPOCH=<N>` so the crate can
//! `env!("AEGIS_MIN_REQUIRED_EPOCH")` into a compile-time `u32`
//! constant (`MIN_REQUIRED_EPOCH`).
//!
//! Also emits a `cargo:rerun-if-changed` directive so bumping the
//! epoch in-repo forces a rebuild of anything that consumes the
//! constant (the binary, the tests, any downstream crate).
//!
//! Path resolution walks up from `CARGO_MANIFEST_DIR` until it finds
//! a `keys/canonical-epoch.json` file — same discovery pattern as
//! `aegis-cli/build.rs` uses for the CHANGELOG. Works for the
//! normal in-workspace build (`keys/` at workspace root, two dirs
//! up from `crates/aegis-trust/`), for the `cargo publish`-packaged
//! tarball (we bail gracefully if no keys/ is found), and for
//! out-of-tree consumers via `cargo install --path` (same-workspace
//! discovery still works).

use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());

    // Walk up from the manifest dir looking for `keys/canonical-epoch.json`.
    // 5 levels is enough for any plausible workspace layout.
    let mut probe = manifest.clone();
    let mut found: Option<PathBuf> = None;
    for _ in 0..5 {
        let candidate = probe.join("keys").join("canonical-epoch.json");
        if candidate.is_file() {
            found = Some(candidate);
            break;
        }
        if !probe.pop() {
            break;
        }
    }

    // Locating via the workspace root is the common path; if we're being
    // built from a published crate tarball with no `keys/` adjacent,
    // fall back to emitting epoch=0 so downstream code can at least
    // compile. Consumers of the out-of-workspace build are responsible
    // for using the `AEGIS_MIN_REQUIRED_EPOCH_OVERRIDE` env var at
    // build time; the fallback-to-0 default is a deliberate "unsafe
    // default" marker that the runtime can detect and refuse.
    let epoch: u64 = match (
        std::env::var("AEGIS_MIN_REQUIRED_EPOCH_OVERRIDE"),
        found.as_ref(),
    ) {
        (Ok(v), _) => v
            .parse()
            .expect("AEGIS_MIN_REQUIRED_EPOCH_OVERRIDE must parse as u64"),
        (Err(_), Some(p)) => parse_epoch_json(p),
        (Err(_), None) => {
            // Emit a build warning so the operator sees the fallback path.
            println!(
                "cargo:warning=aegis-trust: no keys/canonical-epoch.json found in parent tree; \
                 AEGIS_MIN_REQUIRED_EPOCH defaulting to 0 (unsafe). Set \
                 AEGIS_MIN_REQUIRED_EPOCH_OVERRIDE=<N> in the environment to override."
            );
            0
        }
    };

    assert!(
        u32::try_from(epoch).is_ok(),
        "epoch {epoch} does not fit in u32"
    );
    println!("cargo:rustc-env=AEGIS_MIN_REQUIRED_EPOCH={epoch}");

    if let Some(path) = found.as_ref() {
        // Re-run the build script if the canonical-epoch file changes.
        // Absolute path because cargo's rerun-if-changed resolves
        // relative to the crate root, not the workspace root.
        println!("cargo:rerun-if-changed={}", path.display());

        // Also re-run if the historical-anchors file changes — our
        // runtime-loaded list. build.rs doesn't consume it directly,
        // but downstream code pulls it via include_str!, which cargo
        // doesn't track automatically.
        if let Some(anchors) = path.parent().map(|p| p.join("historical-anchors.json"))
            && anchors.is_file()
        {
            println!("cargo:rerun-if-changed={}", anchors.display());
        }
    }

    // The workspace root path is needed at compile time by `include_str!`
    // calls in the source (for the pubkey + historical-anchors files).
    // Emit it as an env var so the source can `concat!(env!("AEGIS_KEYS_DIR"), "/...")`.
    let keys_dir = found
        .as_ref()
        .and_then(|p| p.parent())
        .map_or_else(|| manifest.join("../../keys"), Path::to_path_buf);
    println!("cargo:rustc-env=AEGIS_KEYS_DIR={}", keys_dir.display());
}

/// Naive JSON scanner for the specific shape
/// `"epoch":\s*<integer>` — avoids pulling `serde_json` into the
/// build-script dep graph. The file is maintainer-authored from a
/// template; the shape is stable (see `keys/canonical-epoch.json`).
/// Rejects anything ambiguous (multiple `epoch` keys, missing value).
fn parse_epoch_json(path: &Path) -> u64 {
    let body = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let mut matches: Vec<u64> = Vec::new();
    for line in body.lines() {
        let trimmed = line.trim();
        // Match lines like `"epoch": 1,` or `"epoch":1`
        if let Some(rest) = trimmed.strip_prefix("\"epoch\"") {
            let after_colon = rest
                .trim_start()
                .strip_prefix(':')
                .unwrap_or_else(|| panic!("malformed `epoch` line in {}: {line}", path.display()))
                .trim()
                .trim_end_matches(',')
                .trim();
            let n: u64 = after_colon.parse().unwrap_or_else(|e| {
                panic!("parse epoch value in {} ({line}): {e}", path.display())
            });
            matches.push(n);
        }
    }
    match matches.as_slice() {
        [e] => *e,
        [] => panic!("{} has no `\"epoch\":` line", path.display()),
        more => panic!(
            "{} has {} `\"epoch\":` lines; expected exactly 1",
            path.display(),
            more.len()
        ),
    }
}
