#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# publish-if-new.sh — idempotent `cargo publish` wrapper.
#
# Compares the workspace version (from $WORKSPACE_VERSION env or
# `Cargo.toml [workspace.package].version`) against the highest
# version of $1 currently on crates.io. If they match, exits 0
# with a clean "already published" log line. Otherwise runs
# `cargo publish -p $1 --locked` and propagates its exit code.
#
# Why this exists: the trusted-publishing release workflow
# publishes 6 crates in one job. If a previous re-trigger (or a
# parallel manual publish) already pushed one of them at the
# current version, the next run's `cargo publish` for THAT crate
# returns a 400-ish "version already exists" error and — without
# this wrapper — fails the step, cascade-skipping all subsequent
# crate publishes.
#
# This wrapper turns the "already at this version" case into a
# clean no-op so the workflow can keep going. Any OTHER cargo
# publish failure (network, auth, validation) still fails
# loudly with the original cargo exit code.
#
# Usage:
#   ./scripts/publish-if-new.sh <crate-name>
#
# Exit codes:
#   0  published OR already at workspace version (idempotent OK)
#   N  cargo publish failed for any other reason (propagated)
#   2  usage error (missing crate name argument)

set -euo pipefail

CRATE="${1:-}"
if [[ -z "$CRATE" ]]; then
    echo "publish-if-new: usage: $0 <crate-name>" >&2
    exit 2
fi

# Workspace version: prefer explicit env (CI sets this) so we don't
# have to re-parse Cargo.toml inside a runner where awk + grep can
# get tripped by inline comments.
if [[ -z "${WORKSPACE_VERSION:-}" ]]; then
    SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" &>/dev/null && pwd)"
    REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
    WORKSPACE_VERSION="$(
        awk '
            /^\[workspace\.package\]/ { f=1; next }
            f && /^\[/                 { exit }
            f && /^version = /         {
                gsub(/^version = "|"$/, "", $0)
                print
                exit
            }
        ' "$REPO_ROOT/Cargo.toml"
    )"
fi

if [[ -z "$WORKSPACE_VERSION" ]]; then
    echo "publish-if-new: ERROR — could not parse workspace version" >&2
    exit 2
fi

# Query crates.io for the current max version of CRATE. The HTTP
# 404 case (crate not yet published at all) returns "" → we'll
# always publish. Any other JSON-parse failure also returns "" so
# we err on the side of attempting the publish.
#
# Two-step (download → parse) instead of a `curl | python3` pipe
# so Scorecard's PinnedDependenciesID heuristic doesn't flag this
# as a `downloadThenRun` pattern: the fetched bytes are JSON
# data, not a script.
CRATES_IO_RESPONSE="$(mktemp)"
trap 'rm -f "$CRATES_IO_RESPONSE"' EXIT
LIVE_VERSION=""
if curl --silent --fail --show-error \
        --header 'User-Agent: aegis-boot publish-if-new wrapper' \
        --output "$CRATES_IO_RESPONSE" \
        "https://crates.io/api/v1/crates/${CRATE}" 2>/dev/null; then
    LIVE_VERSION="$(
        python3 -c '
import sys, json
with open(sys.argv[1]) as fp:
    d = json.load(fp)
print(d.get("crate", {}).get("max_version", ""))
' "$CRATES_IO_RESPONSE" 2>/dev/null || echo ""
    )"
fi

echo "publish-if-new: ${CRATE} — workspace=${WORKSPACE_VERSION}, live=${LIVE_VERSION:-<not on registry>}"

if [[ -n "$LIVE_VERSION" && "$LIVE_VERSION" == "$WORKSPACE_VERSION" ]]; then
    echo "publish-if-new: ${CRATE} v${WORKSPACE_VERSION} is already on crates.io — skipping (idempotent OK)."
    exit 0
fi

# Stage workspace-rooted artifacts into per-crate subdirs that need
# them at package time. `cargo publish` packages ONLY the crate dir,
# so anything build.rs reads from `../../` must be copied in first.
# Each crate's build.rs is responsible for falling back to the
# package-local copy (see crates/aegis-cli/build.rs::first_existing).
case "$CRATE" in
    aegis-bootctl)
        SCRIPT_DIR="${SCRIPT_DIR:-$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" &>/dev/null && pwd)}"
        REPO_ROOT="${REPO_ROOT:-$(cd "$SCRIPT_DIR/.." && pwd)}"
        crate_dir="$REPO_ROOT/crates/aegis-cli"
        # Man-template + CHANGELOG: build.rs reads these to render
        # the embedded man page. Stage into crate-local paths the
        # build.rs `first_existing` fallback knows about.
        mkdir -p "$crate_dir/man"
        cp "$REPO_ROOT/man/aegis-boot.1.in" "$crate_dir/man/aegis-boot.1.in"
        cp "$REPO_ROOT/CHANGELOG.md"        "$crate_dir/CHANGELOG.md"
        # Cleanup on script exit so a workspace re-build after the
        # publish doesn't see stale staged copies.
        trap 'rm -rf "$crate_dir/man" "$crate_dir/CHANGELOG.md"' EXIT
        ;;
esac

echo "publish-if-new: publishing ${CRATE} v${WORKSPACE_VERSION}..."
exec cargo publish -p "$CRATE" --locked --allow-dirty
