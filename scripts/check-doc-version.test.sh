#!/usr/bin/env bash
#
# check-doc-version.test.sh — smoke-test that `check-doc-version.sh`
# (a) passes on the current green tree, and (b) correctly FAILS when
# a synthetic drift is introduced.
#
# Not wired into CI automatically — this tests the tester. Run it
# after modifying scripts/check-doc-version.sh or adding a new
# file to its allowlist.

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" &>/dev/null && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT"

echo "check-doc-version.test: GREEN path — current tree should pass"
if "$SCRIPT_DIR/check-doc-version.sh" > /dev/null; then
    echo "  PASS — checker reports OK on current tree"
else
    echo "  FAIL — checker reports drift on current tree (fix that first before testing the tester)" >&2
    exit 1
fi

echo "check-doc-version.test: RED path — synthetic drift must be caught"
BACKUP="$(mktemp)"
cp README.md "$BACKUP"
# Append a fake Status line with a drifted version. Sed the existing
# line rather than append so the test simulates the actual failure
# mode (drift of an existing reference, not addition of a new one).
sed -i 's|\*\*Status:\*\* v[0-9.]*|**Status:** v99.99.99|' README.md
if "$SCRIPT_DIR/check-doc-version.sh" > /dev/null 2>&1; then
    # Checker passed on a drifted tree — that's a bug in the checker.
    echo "  FAIL — checker did NOT catch the synthetic drift to v99.99.99" >&2
    cp "$BACKUP" README.md
    rm "$BACKUP"
    exit 1
fi
cp "$BACKUP" README.md
rm "$BACKUP"
echo "  PASS — checker caught the synthetic drift and exited non-zero"

echo "check-doc-version.test: all tests passed"
