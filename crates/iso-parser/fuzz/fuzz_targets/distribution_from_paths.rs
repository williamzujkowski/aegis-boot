// SPDX-License-Identifier: MIT OR Apache-2.0

#![no_main]

use iso_parser::Distribution;
use libfuzzer_sys::fuzz_target;
use std::path::Path;

// Distribution::from_paths must be total over arbitrary byte-string paths —
// it should never panic on any input (including non-UTF-8 via OsStr).
fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else {
        return;
    };
    let _ = Distribution::from_paths(Path::new(s));
});
