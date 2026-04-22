# aegis-boot → aegis-boot/ org migration plan

**Status:** APPROVED (consensus vote 2026-04-22, 80% supermajority). Maintainer-executed checklist; no bot automation.
**Vote record:** rev 1 rejected at 60% (Gemini surfaced OIDC discontinuity + crates.io squatter race); rev 2 (PR #389) added §1 crates.io pre-registration + §6.3 OIDC identity bridge; rev 2 passed at 80%. Architect ✓88%, Security ✓95% (flipped from rev-1 REJECT), DevEx ✓87%, PM ✓88%, AI/ML abstained (95%), Contrarian ✗95% (operational nitpicks not security-blocking). Verification hashes in PR #389.
**Scope:** Move `github.com/williamzujkowski/aegis-boot` and `github.com/williamzujkowski/aegis-hwsim` under a new GitHub Organization at `github.com/aegis-boot`.
**Maintainer:** William Zujkowski (solo). Personal/individual GitHub account; does NOT register a business entity. If Apple Developer Program enrollment becomes necessary (see #369), the **Individual** enrollment tier applies.
**Org name verified available:** 2026-04-22 — re-verified (`github.com/aegis-boot` → HTTP 404).
**Plan legend:** [UI] = browser click-through • [CLI] = terminal command • [LOCAL] = local repo edit.

## Naming-conflict research (re-run 2026-04-22)

| Registry | Name | Status | Notes |
|---|---|---|---|
| GitHub org | `aegis-boot` | **AVAILABLE** | HTTP 404 confirmed |
| GitHub org | `aegisboot` | AVAILABLE | backup if hyphenated form is unexpectedly taken |
| GitHub user | `Aegis` | exists (unrelated) | different user, case-insensitive distinct from `aegis-boot` |
| GitHub org | `Aegisub` | exists (unrelated) | subtitle editor, different domain |
| GitHub org | `aegis-aead` | exists (unrelated) | AEAD cipher library, different domain |
| GitHub org | `aegiswp` | exists (unrelated) | WordPress plugin, different domain |
| crates.io | `aegis-boot` | **AVAILABLE** | |
| crates.io | `aegis-cli` | **AVAILABLE** | |
| crates.io | `aegis-wire-formats` | **AVAILABLE** | |
| crates.io | `aegis-fitness` | **AVAILABLE** | |
| crates.io | `aegis-hwsim` | **AVAILABLE** | |
| crates.io | `iso-parser` | **AVAILABLE** | |
| crates.io | `iso-probe` | **AVAILABLE** | |
| crates.io | `kexec-loader` | **AVAILABLE** | |
| Docker Hub | `aegis-boot` | AVAILABLE | not actively used by this project |
| npm | `aegis-boot` | AVAILABLE | not actively used (no JS surface) |
| npm | `aegis-cli` | **TAKEN** (unrelated) | "editorconfig for AI agents"; no collision — we don't publish JS |

**Conclusion:** no blocking naming conflicts across any registry we target. The GitHub org slug + all 8 crates.io names the project currently uses are free to claim post-transfer.

---

## 0. Don't-skip preamble

- This is a **manual** plan. Execute each step in order; do not run ahead.
- Every step is a checkbox. Check it off in a working copy of this doc as you go.
- If any step fails in a way the plan doesn't cover, **stop** and diagnose before proceeding — GitHub repo transfers have a 1-hour cooldown (see §7 Rollback) and rushing causes pain.
- Keep a scratch note of all URLs, webhook IDs, and secret names you touch — you will need them in §5.

---

## 1. Pre-move checklist (before clicking create-org)

- [ ] **[UI]** Re-verify org name is still free: open `https://github.com/aegis-boot` in a browser — expect HTTP 404. If it resolves, **STOP** and pick a new name.
- [ ] **[CLI]** Check current repos are in a known-good state:
  ```bash
  cd /home/william/git/aegis-boot && git status && git log -1
  cd /home/william/git/aegis-hwsim && git status && git log -1
  ```
  Both should show `nothing to commit, working tree clean` and a recent commit on `main`.
- [ ] **[UI]** Back up branch protection rules for reference. For each repo open:
  - `https://github.com/williamzujkowski/aegis-boot/settings/branches`
  - `https://github.com/williamzujkowski/aegis-hwsim/settings/branches`

  Screenshot or copy the rule set for `main` (required status check names, required reviewers, force-push rules). These **are** carried during transfer but verifying post-move requires knowing what should be there.
- [ ] **[UI]** Back up repo-level secrets list (names only, not values):
  - `https://github.com/williamzujkowski/aegis-boot/settings/secrets/actions`
  - `https://github.com/williamzujkowski/aegis-hwsim/settings/secrets/actions`

  Secrets **do not** transfer. You will re-add these in §5.
- [ ] **[UI]** Back up Environments (if any) at `/settings/environments` for each repo. Note environment names + protection rules.
- [ ] **[UI]** Back up webhooks list (names + URLs only, not secrets) at `/settings/hooks` for each repo. Webhooks usually transfer but sigstore/cosign OIDC needs special verification (§6).
- [ ] **[CLI, pre-transfer] Reserve all crates.io names with v0.0.0 placeholder crates.** Run under the current `williamzujkowski` GitHub-linked cargo account:
  ```bash
  # For each of: iso-parser, iso-probe, kexec-loader, aegis-fitness,
  # aegis-cli, aegis-wire-formats, aegis-boot, aegis-hwsim.
  cargo new --lib /tmp/crate-placeholder-aegis-boot
  cd /tmp/crate-placeholder-aegis-boot
  # Edit Cargo.toml: name = "<crate>", version = "0.0.0", description = "placeholder — real crate ships under aegis-boot/aegis-boot (see GitHub)."
  cargo publish --token <token>
  ```
  **Why this is pre-transfer work, not post-**: the moment the `williamzujkowski/aegis-boot` → `aegis-boot/aegis-boot` transfer completes, the 8 unclaimed crate names are visibly "discovered" via the repo's public source tree. Squatters troll crates.io for name-grab opportunities on newly-prominent repos; the gap between "repo moves" and "we claim the names under the new org" is a race window we can close to zero by claiming under the old account first, then transferring ownership to the org's cargo account post-transfer. Cost: 8 × `cargo publish` + 8 × `cargo owner --add` (~10 minutes total). Benefit: eliminates the squatter race entirely.
- [ ] **[LOCAL, post-transfer plan only]** After §4 completes, transfer crate ownership: `cargo owner --add aegis-boot <crate-name>` for each placeholder. Actual trusted-publishing (GitHub Actions OIDC under the new org) ships with the first real release post-transfer.

---

## 2. Creating the org

- [ ] **[UI]** Navigate to `https://github.com/organizations/new`.
- [ ] **[UI]** Pick the **Free** plan (public repos are free unlimited Actions; private repos get 2000 minutes/month — irrelevant since both target repos stay public).
- [ ] **[UI]** Org name: `aegis-boot` (exact, lowercase, hyphen).
- [ ] **[UI]** Contact email: use your personal email (the one already on your GitHub account).
- [ ] **[UI]** Ownership: **"My personal account"** (NOT "A business or institution") — this is the correct choice for a solo maintainer on an individual account; it avoids any "business entity" registration question and keeps billing/tax handling simple (no D-U-N-S required).
- [ ] **[UI]** Skip the "Invite members" step (solo maintainer).
- [ ] **[UI]** Complete the "How will you use this org?" survey (answers don't matter for plan or cost).
- [ ] **[UI]** Confirm landing page shows `https://github.com/aegis-boot` with you as the sole Owner.

---

## 3. Initial org settings — configure immediately, before any repo transfer

All paths below are relative to `https://github.com/organizations/aegis-boot/settings/`.

- [ ] **[UI] Billing & plans → Spending limits → Actions: $0** ← **mandatory per maintainer decision.** macOS runners are 10x cost multiplier; a missed workflow loop on macOS can burn $100s in hours. Setting this to $0 means a billing-threshold hit **stops** workflows instead of charging the card. Navigate: `/billing/spending_limit` → Actions → set to `0` USD → Save.
- [ ] **[UI] Member privileges → Base permissions: Read** (`/member_privileges`). Default is Write — change to Read so future contributors cannot push without explicit grant.
- [ ] **[UI] Member privileges → Repository forking: Allow members to fork** (same page). Needed so external contributors can fork and PR.
- [ ] **[UI] Moderation → Code review limits: enable** (`/interaction_limits`). Solo maintainer today, but enabling now prevents drive-by merges later.
- [ ] **[UI] Code security → Global settings → Secret scanning + push protection: ENABLE** (`/security_analysis`). Free for public repos. Push protection blocks secrets at `git push` time rather than after leak.
- [ ] **[UI] Code security → Global settings → Dependabot alerts + security updates: ENABLE** (same page).
- [ ] **[UI] Repository defaults → Default branch: `main`** (`/repository-defaults`). Matches existing convention.
- [ ] **[UI] Create the org profile repo:** org-profile README reads from `aegis-boot/.github` repo at path `profile/README.md`.
  - Navigate `https://github.com/organizations/aegis-boot/repositories/new`.
  - Repo name: `.github` (exact, with leading dot).
  - Visibility: Public.
  - Initialize with a README (placeholder; you'll replace it).
  - After creation, add `profile/README.md` via the web UI with a one-paragraph org description.

---

## 4. Moving repos

**Order matters:** transfer `aegis-boot` first (the anchor repo), then `aegis-hwsim`. If `aegis-boot` transfer fails, you want to know before you touch the second repo.

### 4.1 Transfer `williamzujkowski/aegis-boot` → `aegis-boot/aegis-boot`

- [ ] **[UI]** Open `https://github.com/williamzujkowski/aegis-boot/settings` → scroll to **Danger Zone** → **Transfer ownership**.
- [ ] **[UI]** New owner: `aegis-boot`. Repo name: `aegis-boot` (unchanged). Confirm by typing `williamzujkowski/aegis-boot` in the confirm box.
- [ ] **[UI]** Click **I understand, transfer this repository** — expect ~10-30s spinner, then redirect to `https://github.com/aegis-boot/aegis-boot`.
- [ ] **[UI]** Verify `https://github.com/williamzujkowski/aegis-boot` redirects (HTTP 301) to the new URL. GitHub honors this redirect for ≥1 year (indefinite if no new repo claims the old slug).
- [ ] **[LOCAL]** Update local git remote:
  ```bash
  cd /home/william/git/aegis-boot
  git remote set-url origin git@github.com:aegis-boot/aegis-boot.git
  git remote -v  # verify both fetch + push now point to aegis-boot/aegis-boot
  git fetch origin
  ```
- [ ] **[UI]** Verify clone URLs on the repo landing page match `aegis-boot/aegis-boot`.
- [ ] **[UI]** Re-run the most recent workflow on `main` via `https://github.com/aegis-boot/aegis-boot/actions` → pick latest CI run → **Re-run all jobs**. Confirm it passes under the new org path.
- [ ] **[UI]** Verify GitHub Pages settings at `/settings/pages` (if Pages is in use) — the CNAME/custom-domain field transfers but confirm the build source branch is still set.
- [ ] **[UI]** Verify branch protection on `main` at `/settings/branches` matches the pre-move screenshot from §1.

### 4.2 Transfer `williamzujkowski/aegis-hwsim` → `aegis-boot/aegis-hwsim`

- [ ] **[UI]** Same steps as §4.1, substituting `aegis-hwsim` for `aegis-boot` in the repo path. New owner: `aegis-boot`. Repo name: `aegis-hwsim` (unchanged).
- [ ] **[LOCAL]** Update remote:
  ```bash
  cd /home/william/git/aegis-hwsim
  git remote set-url origin git@github.com:aegis-boot/aegis-hwsim.git
  git fetch origin
  ```
- [ ] **[UI]** Re-run latest CI on `main` and confirm pass.

---

## 5. Post-move fixes

### 5.1 Settings that DON'T transfer and need re-doing

- [ ] **[UI]** Re-add repo-level secrets (compared against the §1 backup list). Navigate `https://github.com/aegis-boot/aegis-boot/settings/secrets/actions` and add each.
  - **Consider promoting to org secrets**: anything used by both repos (cosign keys are keyless, but crates.io tokens for trusted publishing may be reusable). Org secrets: `https://github.com/organizations/aegis-boot/settings/secrets/actions`.
- [ ] **[UI]** Re-create Environments (release, etc.) at `/settings/environments` if any. Protection rules don't transfer.
- [ ] **[UI]** Verify webhooks at `/settings/hooks`. Most transfer; sigstore/cosign OIDC config is separate and handled in §6.
- [ ] **[UI]** Actions minute counters reset on transfer — expected, no action needed.

### 5.2 Downstream hardcoded references — LOCAL edits, open as follow-up PRs

#### 5.2.0 One-shot sweep script (recommended first step)

After the transfer completes, run the automated sweep script instead of hand-editing every file:

```bash
cd /home/william/git/aegis-boot

# 1. Dry-run — inspect what would change.
./scripts/org-migration-sweep.sh

# 2. When the dry-run output looks right, write in place.
./scripts/org-migration-sweep.sh --write

# 3. Review the diff.
git diff

# 4. Verify nothing remains (CI-runnable; exits 1 on any remaining legacy URL).
./scripts/org-migration-sweep.sh --check

# 5. Commit + push + open PR.
git checkout -b chore/365-post-transfer-url-sweep
git add -u
git commit -m "chore(org): sweep legacy williamzujkowski/aegis-* URLs to aegis-boot/"
git push -u origin chore/365-post-transfer-url-sweep
```

The sweep covers `.md`, `.rs`, `.toml`, `.yml`, `.yaml`, `.sh`, `.rb` files and deliberately **excludes** `CHANGELOG.md` (historical entries keep their original URLs — those releases were signed under the legacy cosign identity and forensic back-references to those releases remain correct).

As of 2026-04-22, the sweep reports **169 matches across 56 files** that would be rewritten. The hand-edit tables in §5.2.1 + §5.2.2 below are the fallback manual path if the sweep is ever insufficient (new file types, special-cased contexts). Under normal operation the sweep is complete.

#### 5.2.1 Critical files — cosign + operator install paths

The sweep handles these in one pass; this table is the eyes-on list for post-sweep review — these are the paths where a bad substitution breaks operator-facing verification.

- [ ] **[LOCAL]** `Cargo.toml` line 26: `repository = "https://github.com/williamzujkowski/aegis-boot"` → `https://github.com/aegis-boot/aegis-boot`.
- [ ] **[LOCAL]** `scripts/install.sh`:
  - Line 9 (usage comment): `raw.githubusercontent.com/williamzujkowski/aegis-boot/main/scripts/install.sh` → `raw.githubusercontent.com/aegis-boot/aegis-boot/main/scripts/install.sh`.
  - Line 26: `REPO="williamzujkowski/aegis-boot"` → `REPO="aegis-boot/aegis-boot"`.
  - Line 29: `COSIGN_IDENTITY_REGEXP='^https://github\.com/williamzujkowski/aegis-boot/\.github/workflows/release\.yml@refs/tags/v.+$'` → swap org segment to `aegis-boot`. See §6 for exact replacement string.
- [ ] **[LOCAL]** `Formula/aegis-boot.rb` lines 4, 11, 25, 69, 70, 73, 91, 96 — sweep all `williamzujkowski/aegis-boot` → `aegis-boot/aegis-boot`. Line 96 is the cosign identity regexp (see §6).
- [ ] **[LOCAL]** `Formula/README.md` lines 8, 22 — same sweep.
- [ ] **[LOCAL]** `.github/workflows/release.yml` line 183 (commented URL, but verify no uncommented copies).
- [ ] **[LOCAL]** `docs/RELEASE_NOTES_FOOTER.md` — the cosign verify-blob regexp (see §6).
- [ ] **[LOCAL]** `README.md` — badges (CI, crates.io, License) typically reference the repo slug. Grep for `williamzujkowski/aegis-boot` and replace.
- [ ] **[LOCAL]** Full sweep:
  ```bash
  cd /home/william/git/aegis-boot
  grep -rn "williamzujkowski/aegis-boot" . --include="*.md" --include="*.rs" --include="*.toml" --include="*.yml" --include="*.yaml" --include="*.sh" --include="*.rb" --exclude-dir=target --exclude-dir=.git
  ```
  Every match is a candidate replacement. Review each — the CHANGELOG.md **should not** be rewritten (history is history), but README, docs, scripts, Formula, and workflows should all be updated.
- [ ] **[LOCAL]** Same full-sweep in `/home/william/git/aegis-hwsim`.

### 5.3 Homebrew tap

- [ ] **[LOCAL]** `Formula/aegis-boot.rb` tap URL: the tap install command in line 4 becomes `brew tap aegis-boot/aegis-boot https://github.com/aegis-boot/aegis-boot` (after the tap-name standardization follows the new org). Verify this matches operator docs.
- [ ] **[UI]** If you maintain a separate tap repo (e.g. `homebrew-aegis-boot`), transfer that too via the same Settings → Transfer flow.

---

## 6. Sigstore / cosign OIDC — critical for release.yml

**Why this matters:** `release.yml` uses cosign keyless signing. The signing certificate binds the artifact to the **GitHub Actions workflow identity**, which includes the org path. Old release artifacts (signed under `williamzujkowski/aegis-boot`) stay valid forever — signatures are hash-bound, not location-bound. **New** releases will sign under `aegis-boot/aegis-boot`, and verification instructions must be updated.

### 6.1 Old → new identity strings

**OLD** (`docs/RELEASE_NOTES_FOOTER.md` + `scripts/install.sh:29` + `Formula/aegis-boot.rb:96`):
```
^https://github\.com/williamzujkowski/aegis-boot/\.github/workflows/release\.yml@refs/tags/v.+$
```

**NEW** (for releases cut after the transfer):
```
^https://github\.com/aegis-boot/aegis-boot/\.github/workflows/release\.yml@refs/tags/v.+$
```

### 6.2 Verification-doc transition note

Release notes MUST cover BOTH identities until old releases fall out of the supported window. Add to `docs/RELEASE_NOTES_FOOTER.md` after the cosign verify-blob block:

> **Note on identity:** releases tagged before `vX.Y.Z` (first release after org transfer, date TBD) were signed under the legacy `williamzujkowski/aegis-boot` identity. Those signatures remain valid; to verify those artifacts, substitute `williamzujkowski` for `aegis-boot` in the `--certificate-identity-regexp` flag above.

- [ ] **[LOCAL]** Update `docs/RELEASE_NOTES_FOOTER.md` cosign block to use the NEW identity as primary, with the legacy note appended.
- [ ] **[LOCAL]** Update `scripts/install.sh` line 29 to the NEW identity. Add a fallback verification block in the script that tries both identities when the release tag predates the transfer — OR bump a `SCRIPT_VERSION` and require users to use the correct install.sh matching their release.
- [ ] **[LOCAL]** Update `Formula/aegis-boot.rb` line 96 to the NEW identity.
- [ ] **[LOCAL]** Update `README.md` wherever it documents verification — same NEW identity, same legacy note.
- [ ] **[UI]** On the FIRST release cut after transfer, manually run `cosign verify-blob` end-to-end against a release artifact to confirm the new identity validates. Before this is confirmed, do not merge the verification-doc updates to `main`.

### 6.3 OIDC identity bridge (pre-transfer signed delegation)

**Why this exists.** Cosign keyless signatures bind to the full GitHub Actions workflow identity string — the `sub` claim in the Fulcio cert. When the repo moves, the `sub` changes. Documentation alone ("use the new identity regex for releases after date X") is fine for humans running `cosign verify-blob` by hand, but an automated admission controller or pinning system has no way to chain from the old identity to the new without a signed authorization from the previous identity. A social-engineered `aegis-boot/` identity would be indistinguishable from the legitimate new one to such a verifier.

**The fix — a signed successor-identity manifest in the PRE-transfer repo.**

- [ ] **[LOCAL, pre-transfer]** Create `.github/identity-transition.json` at the root of the CURRENT `williamzujkowski/aegis-boot` repo with the following shape:
  ```json
  {
    "schema_version": 1,
    "statement": "Authorization of successor cosign identity on repository transfer",
    "predecessor": {
      "github_owner": "williamzujkowski",
      "repo": "aegis-boot",
      "cosign_identity_regexp": "^https://github\\.com/williamzujkowski/aegis-boot/\\.github/workflows/release\\.yml@refs/tags/v.+$"
    },
    "successor": {
      "github_org": "aegis-boot",
      "repo": "aegis-boot",
      "cosign_identity_regexp": "^https://github\\.com/aegis-boot/aegis-boot/\\.github/workflows/release\\.yml@refs/tags/v.+$"
    },
    "transition_date": "2026-XX-XX",
    "rationale": "Repository moved to dedicated GitHub org; see docs/governance/ORG_MIGRATION_PLAN.md"
  }
  ```
- [ ] **[CLI, pre-transfer]** Sign it via `cosign sign-blob` using the current workflow identity, producing `.github/identity-transition.json.sig` + `.github/identity-transition.json.pem`:
  ```bash
  # Runs via a one-off GitHub Actions workflow dispatch against main —
  # the cert is bound to williamzujkowski/aegis-boot at sign time.
  cosign sign-blob --yes \
    --output-signature .github/identity-transition.json.sig \
    --output-certificate .github/identity-transition.json.pem \
    .github/identity-transition.json
  ```
- [ ] **[LOCAL, pre-transfer]** Commit all three files (`.json`, `.sig`, `.pem`) to `main` BEFORE the §4 transfer. GitHub preserves them through the transfer; automated verifiers pulling the new repo can read them to chain the trust from old → new identity.
- [ ] **[POST-TRANSFER verification]** On the first release cut under the new org, verify an automated consumer can walk: "trust `williamzujkowski/aegis-boot` legacy identity (baked into my verifier) → read `.github/identity-transition.json` → verify its sig against the baked-in legacy identity → trust the NEW `aegis-boot/aegis-boot` identity per the manifest → verify the current release under the new identity." A sample walker script ships under `scripts/verify-identity-chain.sh` (Phase 2 follow-up of this plan).

**Trade-off acknowledged:** the `successor.cosign_identity_regexp` is a *prediction* of what the Fulcio `sub` will look like after transfer — not an observation. This is fine because the Actions workflow identity format is deterministic (`https://github.com/{org}/{repo}/.github/workflows/{workflow}.yml@refs/tags/{tag}`); the only variable is the org slug, which we control. A post-transfer step verifies the prediction matched reality.

**When this matters:** TODAY, aegis-boot's only `cosign verify-blob` callers are `install.sh` (which can be updated in place via §5) and operator-hand invocations (human reads updated docs). Automated admission controllers (Gatekeeper, Kyverno, sigstore-policy-controller) are not load-bearing consumers today. The identity bridge is bonus defense-in-depth for *future* automation — building the bridge while we still control both endpoints is cheaper than retrofitting after the `williamzujkowski` account stops signing aegis-boot releases. Low-cost, high-option-value work; cheap to ship now.

---

## 7. Rollback plan

If §4 transfer breaks something and you need to revert:

- [ ] **[UI]** Note: GitHub enforces a **1-hour cooldown** before you can transfer the same repo again. Use that hour to diagnose — do not panic-transfer back.
- [ ] **[UI]** Reverse transfer: `https://github.com/aegis-boot/aegis-boot/settings` → Danger Zone → Transfer ownership → new owner `williamzujkowski`.
- [ ] **[LOCAL]** Revert `git remote set-url origin git@github.com:williamzujkowski/aegis-boot.git` in every local clone.
- [ ] **[LOCAL]** Revert any `Cargo.toml` / `scripts/install.sh` / `Formula/aegis-boot.rb` / docs edits from §5.2 that were already merged. Keep the branch `docs/365-org-migration-plan` around as the post-mortem source.
- [ ] **[UI]** GitHub auto-redirect remains in place from `williamzujkowski/aegis-boot` ↔ `aegis-boot/aegis-boot` for ≥1 year; clones using the old URL keep working regardless.
- [ ] **[UI]** If the `aegis-boot` org becomes unusable, you can delete it at `https://github.com/organizations/aegis-boot/settings/profile` → bottom → **Delete this organization**. Do this only after confirming no repos remain under it.

---

## 8. Branch strategy (no change recommended)

Solo maintainer + all repos public + existing `main` + protected-status-checks pattern is working. **Do not introduce gitflow.** Keep:

- Single long-lived branch: `main`
- Feature branches: `feat/<issue>-*`, `fix/<issue>-*`, `docs/<issue>-*` per `.claude/rules/git.md` (upstream from nexus-agents — matches current convention in this repo).
- Branch protection on `main`: require status checks (`ci`, `reproducible-build`, `linkcheck`, etc. — copy from §1 backup), require PR review (1 approval, which solo-maintainer bypasses via admin), disallow force push.
- Org-level rule: consider adding an **org ruleset** at `https://github.com/organizations/aegis-boot/settings/rules` that enforces the same branch protection across both repos. This is optional but reduces config drift when you add the next repo.

---

## 9. Acceptance — what "done" looks like

- [ ] `https://github.com/aegis-boot/aegis-boot` resolves; `https://github.com/williamzujkowski/aegis-boot` redirects to it.
- [ ] `https://github.com/aegis-boot/aegis-hwsim` resolves; old URL redirects.
- [ ] Latest CI run on `main` passes under new org path for both repos.
- [ ] Actions spending cap shows `$0` at `https://github.com/organizations/aegis-boot/settings/billing/spending_limit`.
- [ ] Secret scanning + push protection enabled at `https://github.com/organizations/aegis-boot/settings/security_analysis`.
- [ ] `grep -rn williamzujkowski/aegis-boot /home/william/git/aegis-boot --exclude-dir=target --exclude-dir=.git --exclude=CHANGELOG.md` returns **zero** matches outside CHANGELOG.md.
- [ ] First post-transfer release cut, signed with NEW cosign identity, verified end-to-end against updated docs.
- [ ] `brew tap aegis-boot/aegis-boot && brew install aegis-boot` works from a fresh macOS host.
- [ ] `curl -sSL https://raw.githubusercontent.com/aegis-boot/aegis-boot/main/scripts/install.sh | sh` works from a fresh Linux host.

---

## Appendix A — full file-edit checklist (consolidated)

Files that MUST be edited (from §5.2 grep) — use this as a single pass:

| Path | Line(s) | Change |
|---|---|---|
| `Cargo.toml` | 26 | repo URL → aegis-boot org |
| `scripts/install.sh` | 9, 26, 29 | usage comment, REPO var, COSIGN_IDENTITY_REGEXP |
| `Formula/aegis-boot.rb` | 4, 11, 25, 69, 70, 73, 91, 96 | tap URL, homepage, release URL, issue links, clone URL, docs URL, identity regexp |
| `Formula/README.md` | 8, 22 | tap URL, issue links |
| `docs/RELEASE_NOTES_FOOTER.md` | cosign block | identity regexp + legacy note |
| `README.md` | badges + any embedded URLs | grep pass |
| `.github/workflows/release.yml` | 183 (comment) | verify no other matches |

Files that MUST NOT be rewritten (history):

- `CHANGELOG.md` — historical release entries
- `.git/` — the git history itself

---

_Doc owner: maintainer. Plan version: 1. Last reviewed: 2026-04-20._
