// SPDX-License-Identifier: MIT OR Apache-2.0

#![no_main]

use iso_parser::{IsoEnvironment, OsIsoEnvironment};
use libfuzzer_sys::fuzz_target;
use std::path::Path;

// Fuzz the path-traversal guard. validate_path MUST never panic on arbitrary
// byte input and MUST reject any path that escapes the base directory.
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

    // Invariant: if the candidate contains "..", validation MUST fail.
    if candidate.contains("..") {
        assert!(
            result.is_err(),
            "path containing '..' was not rejected: base={base:?} candidate={candidate:?}"
        );
    }
});
