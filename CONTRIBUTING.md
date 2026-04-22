# Contributing to aegis-boot

Thanks for your interest. This is a small project with a sharp focus — a signed UEFI Secure Boot rescue environment that `kexec`s into operator-selected ISOs. Patches that move us toward that goal are welcome.

## Quickstart

```bash
git clone git@github.com:williamzujkowski/aegis-boot.git
cd aegis-boot
cargo test --workspace               # run every unit + integration test
./scripts/dev-test.sh                # full 8-stage local CI
```

The exact test count drifts every release — `cargo test --workspace 2>&1 | grep 'test result:'` prints the current totals. CI ([.github/workflows/ci.yml](./.github/workflows/ci.yml)) is the authoritative merge gate; see [§CI gates](#ci-gates-your-pr-must-pass) below for the full list.

Prereqs are listed at the top of [`scripts/dev-test.sh`](./scripts/dev-test.sh) and in [`docs/LOCAL_TESTING.md`](./docs/LOCAL_TESTING.md).

The operator-facing CLI lives in [`crates/aegis-cli`](./crates/aegis-cli) (binary `aegis-boot`). When working on the operator surface, exercise it directly: `cargo run -p aegis-cli -- flash --help`. Don't add operator-facing flags without updating [`docs/CLI.md`](./docs/CLI.md).

## Workflow

1. **Open an issue first** for anything bigger than a typo — alignment beats rework.
2. **Branch off `main`**: `feat/<issue>-short-description`, `fix/<issue>-...`, `docs/<topic>`, `chore/<topic>`.
3. **Conventional commits** (validated in PR review; no commitlint hook yet):
   ```
   feat(rescue-tui): add high-contrast theme
   fix(security): block kexec on hash mismatch (#55)
   docs: tighten v0.7.0 CHANGELOG
   ```
   Types: `feat`, `fix`, `refactor`, `docs`, `test`, `chore`, `perf`, `build`, `ci`.
4. **One concern per PR.** Don't bundle a security fix with a refactor.
5. **Tests required for behavior changes.** TDD encouraged — write the failing test first, then make it pass.
6. **Run `dev-test.sh` locally before pushing.** GHA CI is the merge gate but local validation catches problems faster.
7. **PR body should explain the *why*.** The diff explains the what.

## What we look for in a PR

- Tests cover happy path + at least one edge case
- No `unwrap()` / `expect()` outside tests (lints enforce this — won't compile)
- No `unsafe` outside `kexec-loader` (workspace lint forbids)
- Doc strings on new public items (`missing_docs = warn`)
- CHANGELOG updated under the relevant unreleased section if the change is user-visible

## What ships in releases

We follow semver pre-1.0 loosely:

- **patch (`0.x.y`)** — bug fixes, doc fixes, dependency bumps without API change
- **minor (`0.x.0`)** — new features, additive API changes, anything that warrants a release-notes section
- **major (`x.0.0`)** — breaking API changes; v1.0.0 is gated on real-hardware validation ([#51](https://github.com/williamzujkowski/aegis-boot/issues/51))

Each release gets a CHANGELOG section, a tag, and a GitHub release. Build artifacts are uploaded by hand for now (CI release workflow paused; tracked in [#51](https://github.com/williamzujkowski/aegis-boot/issues/51)).

### Drafting release notes

At release-cut time, run the git-cliff drafting assist to produce a first-cut changelog entry from conventional-commit subjects:

```bash
# Draft "what's in [Unreleased]" against last tag → HEAD:
./scripts/draft-release-notes.sh

# Draft a specific new-version heading:
./scripts/draft-release-notes.sh v0.15.0
```

The output is advisory, not authoritative — promote it into `CHANGELOG.md` by (1) re-wording bullets into aegis-boot's prose style (commit subjects say "what"; the CHANGELOG needs the "why" + user-visible impact), (2) dropping scaffolding PRs, and (3) promoting critical bug fixes out of their section into the lead. See the existing versioned entries in `CHANGELOG.md` for the target tone.

The draft script needs `git-cliff` locally (`cargo install --locked git-cliff@2.6.1`). It is intentionally not wired into CI — editorial control stays with the maintainer. Phase 7 of [#286](https://github.com/williamzujkowski/aegis-boot/issues/286).

## CI gates your PR must pass

Every PR runs the following — each is also runnable locally. Running them before push saves a CI round-trip.

| Gate | Local command | Source of truth |
| --- | --- | --- |
| Workspace tests (stable + pinned MSRV) | `cargo test --workspace --locked` | `.github/workflows/ci.yml` |
| Clippy `-D warnings` | `cargo clippy --workspace --all-targets -- -D warnings` | same |
| `cargo fmt --check` | `cargo fmt --check` | same |
| macOS + Windows cross-compile check | `cargo check -p aegis-cli --target x86_64-apple-darwin --all-targets` | same |
| cargo-deny: advisories + licenses + bans + sources | `cargo deny check` | [`deny.toml`](./deny.toml) |
| `cargo publish --dry-run` on publishable crates | `cargo publish --dry-run -p iso-parser -p kexec-loader --locked` | `.github/workflows/crates-publish-dryrun.yml` |
| Constants drift | `cargo run -p aegis-cli --bin constants-docgen --features docgen -- --check` | [`crates/aegis-cli/src/constants.rs`](./crates/aegis-cli/src/constants.rs) |
| CLI drift (subcommand + synopsis) | `cargo run -p aegis-cli --bin cli-docgen --features docgen -- --check` | `crates/aegis-cli/src/bin/cli_docgen.rs` |
| JSON schema drift | `cargo run -p aegis-wire-formats --bin aegis-wire-formats-schema-docgen --features schema -- --check` | `docs/reference/schemas/*.schema.json` |
| Workspace version drift | CI job, no local gate | `.github/workflows/ci.yml` |
| Semgrep Rust SAST | GitHub-only | `.github/workflows/ci.yml` (job `sast`) |
| gitleaks secret scan | GitHub-only | `.gitleaks.toml` |
| Miri UB detection (kexec-loader) | `cargo +nightly miri test -p kexec-loader` | `.github/workflows/miri-kexec-loader.yml` |
| Real-hardware / OVMF boot smoke | GitHub-only | `.github/workflows/direct-install-e2e.yml`, `ovmf-secboot.yml` |

`./scripts/dev-test.sh` bundles most of these into a single "run-before-push" command.

**First PR?** `gh issue list --label "good first issue"` surfaces issues curated for newcomers. Or propose your own fix via a new issue first (step 1 above).

## Extending the CLI

Adding a new subcommand touches **four** places; the CI `cli-drift` gate will reject partial wiring:

1. `crates/aegis-cli/src/<subcommand>.rs` — implementation
2. `crates/aegis-cli/src/main.rs` — dispatch table + `print_help()` entry
3. `crates/aegis-cli/src/bin/cli_docgen.rs` — `SUBCOMMANDS` registry
4. `docs/CLI.md` + `man/aegis-boot.1.in` — prose companion + man section

After editing, regenerate the synopsis (picked up by the CI drift-check):

```bash
cargo build --release -p aegis-cli --bin aegis-boot
cargo run -q -p aegis-cli --bin cli-docgen --features docgen -- --write
```

## Security issues

**Do not file public issues for vulnerabilities.** See [SECURITY.md](./SECURITY.md) for the private reporting path.

## Code of conduct

This project follows the [Contributor Covenant 2.1](./CODE_OF_CONDUCT.md). Be kind, be specific, assume good intent.

## License

By contributing, you agree your contributions are dual-licensed under [Apache-2.0](./LICENSE-APACHE) OR [MIT](./LICENSE-MIT) at the user's option, matching the rest of the project.
