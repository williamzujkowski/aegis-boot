// SPDX-License-Identifier: MIT OR Apache-2.0

#![no_main]

use iso_parser::{IsoEnvironment, OsIsoEnvironment};
use libfuzzer_sys::fuzz_target;
use std::path::{Component, Path};

// Fuzz the path-traversal guard. validate_path MUST never panic on arbitrary
// byte input and MUST reject any path that escapes the base directory via a
// genuine `..` PATH COMPONENT (between `/` boundaries), not an arbitrary
// substring.
//
// Why component-based, not substring-based: a filename like `foo..bar` or
// `..\x03|.` is a single legitimate component name. ISOs in the wild can
// legally contain such filenames, and validate_path must extract them
// without interpreting the embedded dots as a traversal. The earlier
// substring-based invariant flagged these as false traversals, which
// surfaced as a nightly-fuzz panic on 2026-04-19 through 2026-04-23.
// Fixing the invariant is the right move because validate_path's own
// implementation already uses `Path::components()` correctly.
fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else {
        return;
    };

    // Split input into base and candidate around first NUL, so both halves are
    // exercised. Without a separator, use a fixed base.
    let (base, candidate) = match s.find('\0') {
        Some(i) => (&s[..i], &s[i + 1..]),
        None => ("/safe", s),
    };

    if base.is_empty() || candidate.is_empty() {
        return;
    }

    let env = OsIsoEnvironment::new();
    let result = env.validate_path(Path::new(base), Path::new(candidate));

    // Invariant: if the candidate has a `..` PATH COMPONENT (ParentDir),
    // validation MUST fail. Substring matches like `foo..bar` or
    // `..\x03|.` are legitimate filenames, not traversal attempts.
    let has_parent_component = Path::new(candidate)
        .components()
        .any(|c| matches!(c, Component::ParentDir));
    if has_parent_component {
        assert!(
            result.is_err(),
            "path with a `..` parent component was not rejected: base={base:?} candidate={candidate:?}"
        );
    }
});
