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

## Actual publish flow

From a clean checkout of a tagged release commit (`v1.0.0-rc1` or later):

```bash
# 1. Confirm crates.io account has 2FA enabled
cargo login                     # if not already done
cargo owner --list iso-parser   # prove we're authenticated

# 2. Publish in dependency order; each publish blocks until the
#    registry index updates so the next step can resolve it.
cargo publish -p iso-parser
cargo publish -p iso-probe
cargo publish -p kexec-loader

# 3. Verify docs.rs built each one (may take 5-15 min)
for c in iso-parser iso-probe kexec-loader; do
  echo "checking https://docs.rs/$c"
  curl -sfSLo /dev/null "https://docs.rs/$c" && echo "  OK"
done
```

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
