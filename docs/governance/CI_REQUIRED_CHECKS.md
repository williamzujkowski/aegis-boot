# CI required-checks audit

**Last audited:** 2026-04-25 (#584)
**Auditor:** autonomous-loop session

## Scope

Document the current branch-protection posture on `main` and the
implications for `paths-ignore:` / conditional CI work tracked under
epic #580 (CI feedback-loop speedup).

## Current state — `main`

```bash
$ gh api repos/aegis-boot/aegis-boot/branches/main/protection
{"message": "Branch not protected", "status": "404"}
```

**There are no branch-protection rules on `main`.** No status check is
*required* at the branch-protection layer. The Contrarian's deadlock
concern from the epic #580 vote — "adding `paths-ignore:` to required
status checks causes PRs to deadlock waiting for a SUCCESS report from
a job that intentionally never ran" — does not apply here, because no
checks are required to begin with.

This means:

- A PR can be merged even if all 38 CI jobs are red (subject to
  reviewer judgment, but not GitHub-enforced).
- Adding `paths-ignore:` filters to E2E workflows is safe — the PR's
  check-count just decreases for the matching path patterns, and no
  job hangs in a "pending forever" state.
- The "always-pass dummy" pattern from the epic plan is **not
  required** for the `paths-ignore` work. It would be required only
  if `main` later acquired branch-protection rules naming specific
  jobs.

## Implications for epic #580 Phase 1

The path-skip PRs (#XXX5 / #XXX6 in the epic plan) can proceed with a
plain `paths-ignore:` block on the 6 E2E jobs. No dummy-pass
consolidation job is needed today.

If the maintainer later adds branch-protection rules on `main` and
names any of those E2E jobs as required, the path-skip workflows will
need to be revisited. This audit must be re-run before/after any such
governance change.

## Independent finding (out of #584's scope, surfacing for the record)

The absence of branch-protection rules on `main` is itself worth
flagging — anyone with push access can land directly on `main`, force-
push, or merge a red PR. The existing CI signal is advisory, not
enforcing. If the maintainer wants the "CI is the merge gate"
guarantee, the next governance pass should:

1. Identify the minimum set of required checks (probably:
   `Test (Rust 1.88.0)`, `Test (Rust stable)`, `rescue-tui clippy`,
   `aegis-fitness audit`, plus the doc-drift checks).
2. Decide whether reviewer approval is also required.
3. Set the rules via `gh api repos/.../branches/main/protection`.

This is not blocking the epic #580 work — flagging here so the
governance gap is visible alongside the CI-speed work that depends
on knowing what the gate looks like.

## Re-audit triggers

Re-run the `gh api .../branches/main/protection` query and update this
doc when:

- Adding any new required workflow or required-check name
- Restructuring the workflow matrix (e.g. splitting the test job)
- Onboarding additional contributors with push access
- Before opening any PR that depends on the dummy-pass pattern

Refs: epic #580 Phase 1.
