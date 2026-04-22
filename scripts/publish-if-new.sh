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
LIVE_VERSION="$(
    curl --silent --fail --show-error \
        --header 'User-Agent: aegis-boot publish-if-new wrapper' \
        "https://crates.io/api/v1/crates/${CRATE}" \
        2>/dev/null \
        | python3 -c 'import sys, json; d=json.load(sys.stdin); print(d.get("crate", {}).get("max_version", ""))' \
        2>/dev/null \
        || echo ""
)"

echo "publish-if-new: ${CRATE} — workspace=${WORKSPACE_VERSION}, live=${LIVE_VERSION:-<not on registry>}"

if [[ -n "$LIVE_VERSION" && "$LIVE_VERSION" == "$WORKSPACE_VERSION" ]]; then
    echo "publish-if-new: ${CRATE} v${WORKSPACE_VERSION} is already on crates.io — skipping (idempotent OK)."
    exit 0
fi

echo "publish-if-new: publishing ${CRATE} v${WORKSPACE_VERSION}..."
exec cargo publish -p "$CRATE" --locked
