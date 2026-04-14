# iso-parser fuzzing

Fuzz targets for security-critical code paths in the ISO parser.

## Targets

| Target | Exercises | Invariant |
|---|---|---|
| `validate_path` | `IsoEnvironment::validate_path` | Paths containing `..` MUST be rejected; no input causes a panic. |
| `distribution_from_paths` | `Distribution::from_paths` | Total function: no input causes a panic. |

## Running locally

```bash
cargo install cargo-fuzz
cd crates/iso-parser
cargo +nightly fuzz run validate_path -- -max_total_time=60
cargo +nightly fuzz run distribution_from_paths -- -max_total_time=60
```

## Corpus

`fuzz/corpus/<target>/` contains hand-crafted seeds. Intentionally minimal —
libFuzzer grows the corpus through coverage-guided mutation. Adding large
real-world inputs (e.g., full ISOs) is **explicitly rejected** per the project
fuzzing policy: it bloats CI bandwidth without improving coverage density.

## CI

Nightly workflow (`.github/workflows/fuzz.yml`) runs each target for 10 minutes
on a schedule. Crashes fail the job and are uploaded as artifacts for triage.
Per-PR fuzzing is intentionally not done — see ADR in PR description.
