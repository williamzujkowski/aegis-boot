# Homebrew formula

This directory makes the aegis-boot repo a [Homebrew tap](https://docs.brew.sh/Taps): a third-party formula source that operators can install from.

## Install

```bash
brew tap williamzujkowski/aegis-boot https://github.com/williamzujkowski/aegis-boot
brew install aegis-boot
```

After that, `brew upgrade aegis-boot` picks up new formula versions when they're committed here.

## What gets installed

- `bin/aegis-boot` — the operator CLI (Linux x86_64 binary downloaded from GitHub Releases)
- Runtime deps via Brew: `curl`, `gnupg`, `gptfdisk`, `coreutils` (only auto-installed on macOS — Linux ships these in the base system)

## Platforms

- **Linux x86_64** + **macOS arm64 (Apple Silicon)**: supported today via the cosign-signed release binaries.
- **Linux aarch64, macOS x86_64, Windows**: not yet supported — the formula errors with a pointer to [#365](https://github.com/williamzujkowski/aegis-boot/issues/365) (cross-platform flash epic). For now, build from source: `cargo install --path crates/aegis-cli`.

## Verifying the binary cosign signature

The Brew install pulls the same binary the manual `curl | sh` installer does, so the same Sigstore cosign verification recipe applies — see [docs/RELEASE_NOTES_FOOTER.md](../docs/RELEASE_NOTES_FOOTER.md).

## Updating the formula

When a new release is tagged, this file needs:

1. Bump `version "X.Y.Z"`
2. Update the URL to the new tag
3. Update `sha256 "..."` to the new binary's hash (from the release's `SHA256SUMS` asset)
4. Possibly extend the `test do` block as new subcommands ship

A future PR (tracked under [epic #365](https://github.com/williamzujkowski/aegis-boot/issues/365)) will add a release-time CI step that bumps the formula automatically alongside the GitHub release.
