# Contributing to aegis-boot

Thanks for your interest. This is a small project with a sharp focus — a signed UEFI Secure Boot rescue environment that `kexec`s into operator-selected ISOs. Patches that move us toward that goal are welcome.

## Quickstart

```bash
git clone git@github.com:williamzujkowski/aegis-boot.git
cd aegis-boot
cargo test --workspace               # 140 tests as of v0.12.0
./scripts/dev-test.sh                # full 8-stage local CI
```

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

## Security issues

**Do not file public issues for vulnerabilities.** See [SECURITY.md](./SECURITY.md) for the private reporting path.

## Code of conduct

This project follows the [Contributor Covenant 2.1](./CODE_OF_CONDUCT.md). Be kind, be specific, assume good intent.

## License

By contributing, you agree your contributions are dual-licensed under [Apache-2.0](./LICENSE-APACHE) OR [MIT](./LICENSE-MIT) at the user's option, matching the rest of the project.
