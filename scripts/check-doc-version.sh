#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# check-doc-version.sh — drift-guard for aegis-boot version literals in
# user-facing docs.
#
# Single source of truth: `[workspace.package].version` in Cargo.toml.
# This script reads that once, then checks a small set of **per-file
# targeted patterns** — each pattern captures a literal that MUST be
# the current aegis-boot version. Any mismatch is a drift and fails
# the check.
#
# Why per-file patterns (not a broad repo-wide grep): a naive regex
# like `v?\d+\.\d+\.\d+` flags Rust toolchain version (1.85.0),
# Ubuntu ISO versions (24.04.2), kernel versions (6.17.0), historical
# aegis-boot references ("--version v0.12.0"), and forward-looking
# milestones ("gates v1.0.0"). All false positives.
#
# Each pattern below is anchored to text that is aegis-boot-specific
# AND is expected to always carry the CURRENT version — so a mismatch
# is always real drift.
#
# Exit codes: 0 all good, 1 drift detected, 2 tool error.
#
# Wired into CI by `.github/workflows/ci.yml`. Phase 1c of #287.

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" &>/dev/null && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT"

# ---- Parse the canonical version from [workspace.package] -------------------
CANONICAL_VERSION="$(
    awk '
        /^\[workspace\.package\]/ { f=1; next }
        f && /^\[/                 { exit }
        f && /^version = /         {
            gsub(/^version = "|"$/, "", $0)
            print
            exit
        }
    ' Cargo.toml
)"

if [[ -z "$CANONICAL_VERSION" ]]; then
    echo "check-doc-version: ERROR — could not parse [workspace.package].version from Cargo.toml" >&2
    exit 2
fi

if ! [[ "$CANONICAL_VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9._-]+)?$ ]]; then
    echo "check-doc-version: ERROR — canonical version '$CANONICAL_VERSION' doesn't match semver shape" >&2
    exit 2
fi

echo "check-doc-version: canonical version = $CANONICAL_VERSION"

# ---- Per-file targeted patterns --------------------------------------------
#
# Each row: FILE|PATTERN_WITH_@VERSION@|DESCRIPTION
# The pattern is grep -E (extended regex) with `@VERSION@` substituted
# to the canonical version at check time. The check succeeds if the
# pattern (with the canonical version) matches — i.e., the file DOES
# carry the current version at the expected location.
#
# If you add a new doc that references the aegis-boot version, add a
# row here. If a doc drops a version reference, remove its row.
#
# Add with intent: each row should be a reference that MUST match, not
# MAY match. Don't allowlist speculative patterns.
PATTERNS=(
    # NOTE: README.md's "Status" banner intentionally does NOT carry a
    # version number anymore — it's an evergreen pointer to CHANGELOG.md
    # (#384 docs-evergreen sweep). The per-release version reference
    # lives in the CHANGELOG; drift there is caught by humans reading
    # the release-notes PR. Don't re-add a README pattern here without
    # re-opening that conversation.

    # INSTALL.md --version example output
    'docs/INSTALL.md|aegis-boot --version[^\n]*aegis-boot v@VERSION@|INSTALL.md --version example'

    # CLI.md: JSON envelope examples use `"version": "X.Y.Z"` and
    # `"tool_version": "X.Y.Z"` — both are expected to carry the
    # CURRENT version because they're rendered by `aegis-boot` which
    # always emits its own CARGO_PKG_VERSION. A follow-up phase will
    # make these auto-generated; until then, drift-check them.
    'docs/CLI.md|"version": "@VERSION@"|CLI.md --version JSON envelope'
    'docs/CLI.md|"tool_version": "@VERSION@"|CLI.md tool_version JSON envelope'
    'docs/CLI.md|reports the workspace version \(currently `@VERSION@`\)|CLI.md prose'

    # NOTE: man/aegis-boot.1.in is the authored TEMPLATE, not a rendered
    # man page. It deliberately carries the literal `@VERSION@` +
    # `@DATE@` placeholders — substituted at build time by
    # crates/aegis-cli/build.rs (Phase 1b of #286/#287). The template's
    # "always has the placeholder" contract is enforced by the
    # man::tests::template_contains_version_placeholder unit test
    # in crates/aegis-cli/src/man.rs, not by this drift check.
)

drift_found=0

for row in "${PATTERNS[@]}"; do
    IFS='|' read -r file pattern_template description <<< "$row"

    if [[ ! -f "$file" ]]; then
        echo "check-doc-version: WARN — allowlisted file missing: $file ($description)" >&2
        continue
    fi

    # Substitute @VERSION@ to the canonical version.
    pattern="${pattern_template//@VERSION@/$CANONICAL_VERSION}"

    if ! grep -qE "$pattern" "$file"; then
        echo "check-doc-version: DRIFT in $file — expected $description to match current version '$CANONICAL_VERSION'" >&2
        echo "    pattern (with @VERSION@ substituted): $pattern" >&2
        drift_found=1
    fi
done

if [[ "$drift_found" -eq 1 ]]; then
    echo "check-doc-version: FAIL — doc version drift detected" >&2
    echo "" >&2
    echo "Fix: update each flagged file so the named reference matches '$CANONICAL_VERSION'." >&2
    echo "The workspace version is managed in Cargo.toml [workspace.package] (Phase 1a of #286)." >&2
    exit 1
fi

echo "check-doc-version: OK — ${#PATTERNS[@]} patterns checked, all reference $CANONICAL_VERSION"
