# Publishing library crates to crates.io

Tracking issue: [#51](https://github.com/aegis-boot/aegis-boot/issues/51). Gate: **v1.0.0-rc1**. Do not run this procedure against any pre-1.0 tag — the API is still moving, and `cargo yank` cannot retract lockfiles downstream users have already pinned.

## What gets published

Three library crates, in dependency order:

1. **iso-parser** — no intra-workspace deps
2. **iso-probe** — depends on iso-parser
3. **kexec-loader** — no intra-workspace deps

**Intentionally NOT published:**

- `rescue-tui` — binary that only makes sense inside our initramfs; `cargo install rescue-tui` would mislead users.
- `aegis-fitness` — repo-specific health audit, no value as a dependency.
- `aegis-cli` — operator CLI, distributed via GitHub Releases + Homebrew + install.sh, not `cargo install` (we want the cosign-verified binary, not a source-build that bypasses supply-chain verification).

## Pre-publish checklist

Every library crate should already have, via its `[package]` table:

- [x] `license = "MIT OR Apache-2.0"` (workspace-inherited for most)
- [x] `repository = "https://github.com/aegis-boot/aegis-boot"`
- [x] `description` — one-sentence, ≤160 chars
- [x] `readme = "README.md"` — a per-crate README exists
- [x] `documentation = "https://docs.rs/<crate-name>"`
- [x] `homepage = "https://github.com/aegis-boot/aegis-boot"`
- [x] `keywords` — up to 5, ≤20 chars each, ASCII-lowercase/digit/hyphen
- [x] `categories` — from the canonical [crates.io category list](https://crates.io/category_slugs)

Verify all of the above cleanly with:

```bash
cargo publish --dry-run -p iso-parser
cargo publish --dry-run -p kexec-loader
# iso-probe's dry-run will fail until iso-parser is actually published
# (it depends on iso-parser as a registry dep); that failure is expected
# and resolved by the actual-publish ordering below.
```

## Internal path deps

iso-probe's dependency on iso-parser must carry **both** `path` and `version`:

```toml
iso-parser = { path = "../iso-parser", version = "1.0" }
```

The `version` is what gets baked into the published `iso-probe` package; the `path` is used for local workspace builds. Keep these aligned with every bump.

## Pre-publish soundness checks

Before tagging v1.0.0-rc1, confirm the following gates are green. They catch the classes of bug that are painful to walk back once a crate is on the registry.

### 1. `cargo publish --dry-run` per crate (#355, automated on every PR)

`.github/workflows/crates-publish-dryrun.yml` runs `cargo publish --dry-run` against `iso-parser` and `kexec-loader` on every push to main and every PR. Catches metadata regressions (missing `version =` on path deps, bad category names, oversize `description`) before they surface on publish day.

`iso-probe` isn't in this gate yet — its dry-run needs `iso-parser` to already be on the registry (documented in the CI workflow). Add it once iso-parser publishes.

### 2. `cargo-deny` license + advisory policy (#362, automated on every PR)

`.github/workflows/ci.yml`'s `cargo-deny` job runs `check advisories licenses bans sources` against `deny.toml`. The workspace accepts only the dual `MIT OR Apache-2.0` + compatible permissive licenses. Two upstream-unmaintained advisories are explicitly ignored with rationale (see `deny.toml`).

### 3. Miri UB detection on `kexec-loader` (#364, path-gated on crates/kexec-loader/**)

`.github/workflows/miri-kexec-loader.yml` runs `cargo +nightly miri test -p kexec-loader` on any PR touching the `kexec-loader` crate. Miri catches classes of undefined behavior the normal compiler doesn't see: uninitialized reads, stacked-borrows aliasing violations, use-after-free on the `OwnedFd` wrapper, `CString` lifetime bugs across the syscall boundary.

Miri does NOT catch the `libc::syscall(SYS_kexec_file_load)` call itself — miri doesn't execute real syscalls. Syscall-side validation lives in `.github/workflows/kexec-e2e.yml` (OVMF QEMU E2E) + real-hardware shakedown. Together the three gates cover:

- miri: wrapper-layer memory safety (stacked borrows, aliasing, lifetimes)
- OVMF QEMU: end-to-end kexec path under signed-chain Secure Boot
- Real hardware (#132 successor): firmware quirks bare-metal QEMU can't simulate

Run miri locally before tagging:

```bash
rustup toolchain install nightly --component miri
cargo +nightly miri test -p kexec-loader
```

Expected output: 6 tests pass, 1 ignored (needs root + would kexec the host). No UB.

### 4. Real-hardware shakedown (#132 successor)

Per the #51 epic body, v1.0.0-rc1 holds until at least one Framework, one Dell, and one ThinkPad successfully boot a direct-install stick under Secure Boot enforcing. This is the largest remaining gate. See `docs/validation/REAL_HARDWARE_REPORT_132.md` for the validation-report template + the first completed run (2026-04-21, SanDisk Cruzer under QEMU USB passthrough).

## Actual publish flow — Trusted Publishing (no long-lived token)

Crates.io's Trusted Publishing lets a specific GitHub Actions workflow mint a short-lived (~30 min) upload token via OIDC. No `CARGO_REGISTRY_TOKEN` secret is needed on the GitHub side; no long-lived token sits in `pass` or elsewhere. This is the modern supply-chain best practice (2024+) and it's what this repo ships with.

### One-time UI setup per crate (maintainer, on crates.io)

For EACH publishable crate, browse to `https://crates.io/crates/<NAME>/settings` → **Trusted Publishing** → **Add GitHub publisher**:

| Field | Value |
|---|---|
| Repository owner | `aegis-boot` |
| Repository name | `aegis-boot` |
| Workflow filename | `crates-publish.yml` |
| Environment (recommended) | `release` |

Repeat for: `aegis-wire-formats`, `iso-parser`, `iso-probe`, `kexec-loader`, `aegis-fitness`, `aegis-bootctl`. (8 workspace crates total — the v0.0.0 `aegis-boot` umbrella placeholder stays unmanaged because the root is a virtual workspace; `aegis-hwsim` publishes from its sibling repo.)

Crates.io docs: https://crates.io/docs/trusted-publishing.

### Per-release flow (after trusted publishers configured)

```bash
# 1. Bump workspace version in Cargo.toml + amend CHANGELOG.
# 2. Tag the release:
git tag -s v1.0.0-rc1 -m "v1.0.0-rc1 release candidate"
git push origin v1.0.0-rc1
```

The `v*.*.*` tag push triggers `.github/workflows/crates-publish.yml`, which:

1. Enters the `release` environment (required-reviewer gate fires — you approve in the GitHub UI).
2. Mints a short-lived crates.io token via `rust-lang/crates-io-auth-action@v1`.
3. Runs `cargo publish -p <crate> --locked` for each publishable crate in dependency order.

### Manual backfill (outage recovery)

If a crates.io outage blocks the middle of the tag-triggered run, use the workflow_dispatch at https://github.com/aegis-boot/aegis-boot/actions/workflows/crates-publish.yml → **Run workflow** → select the specific `crate` input and the tag `ref`. Same OIDC minting, same reviewer gate.

### docs.rs verification

```bash
for c in aegis-wire-formats iso-parser iso-probe kexec-loader aegis-fitness aegis-bootctl; do
  echo "checking https://docs.rs/$c"
  curl -sfSLo /dev/null "https://docs.rs/$c" && echo "  OK" || echo "  BUILD PENDING"
done
```

docs.rs typically builds within 5–15 minutes of publish.

## Post-publish

- [ ] Pin the published version into the workspace's `Cargo.toml` (swap `path = "..."` to `version = "1.0"` for downstream-visible deps) — wait, that's backwards. For *us* as the repo owners, keep `path = "..."` since we build locally. The `version` field is what matters for external users.
- [ ] Add a "see also" row to each crate's README pointing at its docs.rs page.
- [ ] Bump version in the workspace `Cargo.toml` and each `[package]` for the next development cycle (e.g. v1.1.0-dev).

## Yank policy

If a published release contains a security vulnerability:

1. Fix the bug on a branch, cut a patch release, publish the patch.
2. `cargo yank --vers <bad-version> <crate-name>` — keeps the tarball available so existing lockfiles still resolve, but prevents new resolutions from picking the yanked version.
3. File a RustSec advisory if the severity warrants it.
4. Open a Security Advisory on the GitHub repo linking back to the published CVE.

**Do not delete** — crates.io supports yank, not delete, for reproducibility.

## API stability policy (post-1.0)

- **Breaking changes** require a major bump (`2.0.0`).
- **Additive changes** (new public items, new methods on existing traits) go in a minor bump.
- **Bug fixes** go in a patch.
- **Deprecation** via `#[deprecated]` precedes removal by at least one minor release.
- Soundness fixes are not breaking; they can ship in a patch.
