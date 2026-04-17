# Publishing library crates to crates.io

Tracking issue: [#51](https://github.com/williamzujkowski/aegis-boot/issues/51). Gate: **v1.0.0-rc1**. Do not run this procedure against any pre-1.0 tag — the API is still moving, and `cargo yank` cannot retract lockfiles downstream users have already pinned.

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
- [x] `repository = "https://github.com/williamzujkowski/aegis-boot"`
- [x] `description` — one-sentence, ≤160 chars
- [x] `readme = "README.md"` — a per-crate README exists
- [x] `documentation = "https://docs.rs/<crate-name>"`
- [x] `homepage = "https://github.com/williamzujkowski/aegis-boot"`
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
