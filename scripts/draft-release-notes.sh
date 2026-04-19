#!/usr/bin/env bash
#
# draft-release-notes.sh — produce a first-cut CHANGELOG entry for
# the next release using `git-cliff`. The output is a DRAFT; the
# maintainer curates it into aegis-boot's prose style before
# promoting it into `CHANGELOG.md`.
#
# Phase 7 of #286. Intentionally not wired into CI — the draft is
# advisory, editorial control stays with the maintainer (per the
# acceptance criteria on #293).
#
# Usage:
#   ./scripts/draft-release-notes.sh                 # next version, unreleased
#   ./scripts/draft-release-notes.sh v0.15.0         # draft for a specific version tag
#   ./scripts/draft-release-notes.sh v0.14.0 v0.13.0 # retrospective render
#
# The tag argument drives the `## [X.Y.Z] — YYYY-MM-DD` heading the
# draft emits. Without it, the heading is `## [Unreleased]` (matching
# `CHANGELOG.md`'s working-area convention).
#
# Optional: set `GITHUB_TOKEN` in the environment to enrich bullet
# entries with PR back-links. Without a token the script runs fine
# — git-cliff falls back to subject-line parsing for PR numbers.

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" &>/dev/null && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
CONFIG="$REPO_ROOT/cliff.toml"
MIN_CLIFF_VERSION="2.6.0"

if ! [[ -f "$CONFIG" ]]; then
    echo "draft-release-notes: config not found at $CONFIG" >&2
    exit 2
fi

if ! command -v git-cliff &>/dev/null; then
    cat >&2 <<EOF
draft-release-notes: git-cliff not in PATH.

Install with one of:
  cargo install --locked git-cliff@${MIN_CLIFF_VERSION}
  cargo binstall --locked git-cliff --version ${MIN_CLIFF_VERSION}
  # or see https://git-cliff.org/docs/installation for system packages

This script is a drafting assist; it is NOT required for releases —
you can continue to author CHANGELOG entries by hand.
EOF
    exit 2
fi

NEW_TAG="${1:-}"
BASE_RANGE="${2:-}"

cd "$REPO_ROOT"

# Determine the commit range. Default: everything since the most
# recent annotated tag.
if [[ -z "$BASE_RANGE" ]]; then
    LAST_TAG="$(git describe --tags --abbrev=0 2>/dev/null || true)"
    if [[ -n "$LAST_TAG" ]]; then
        RANGE="${LAST_TAG}..HEAD"
    else
        RANGE="" # no prior tag — render the entire history
    fi
else
    # Retrospective: `v0.14.0 v0.13.0` renders between two historical tags.
    RANGE="${BASE_RANGE}..${NEW_TAG}"
fi

# Emit the draft. `--tag` drives the heading; falling through without
# a tag produces `## [Unreleased]`.
if [[ -n "$NEW_TAG" ]]; then
    # shellcheck disable=SC2086
    exec git-cliff --config "$CONFIG" --tag "$NEW_TAG" $RANGE
else
    # shellcheck disable=SC2086
    exec git-cliff --config "$CONFIG" --unreleased $RANGE
fi
