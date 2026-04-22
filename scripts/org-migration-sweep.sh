#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# org-migration-sweep.sh — rewrite every williamzujkowski/aegis-* URL
# in this repo to the new aegis-boot/ org path. Runs AFTER the
# GitHub repo transfer completes (see docs/governance/ORG_MIGRATION_PLAN.md).
#
# Idempotent: running twice produces no changes on the second pass.
#
# DRY-RUN by default; pass --write to actually modify files.
# Always runs through git, so anything it breaks is `git diff`-visible
# before you commit. Never touches CHANGELOG.md (historical releases
# keep their original URLs — those signatures validate under the
# legacy cosign identity).
#
# Usage:
#   ./scripts/org-migration-sweep.sh          # dry-run — show what would change
#   ./scripts/org-migration-sweep.sh --write  # actually rewrite files
#   ./scripts/org-migration-sweep.sh --check  # exit 1 if any legacy URL remains
#                                             # (CI use: verify post-transfer sweep landed)
#
# Exit codes:
#   0  success (dry-run: no changes shown = nothing to do; write: files rewritten)
#   1  --check mode found remaining legacy URLs
#   2  usage error

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" &>/dev/null && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT"

OLD_ORG="williamzujkowski"
NEW_ORG="aegis-boot"

mode="dry-run"
case "${1:-}" in
    --write) mode="write" ;;
    --check) mode="check" ;;
    --help|-h)
        sed -n '4,28p' "${BASH_SOURCE[0]}" | sed 's/^# //; s/^#//'
        exit 0
        ;;
    "") ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
esac

# The sweep covers two legacy paths. Both get rewritten to the same
# new org; aegis-hwsim stays a sibling repo under the new org.
OLD_PATTERNS=(
    "${OLD_ORG}/aegis-boot"
    "${OLD_ORG}/aegis-hwsim"
)
# Matching NEW paths (one-to-one index).
NEW_PATTERNS=(
    "${NEW_ORG}/aegis-boot"
    "${NEW_ORG}/aegis-hwsim"
)

# File types to sweep. Excludes target/ (build artifacts), .git/ (history),
# and CHANGELOG.md (historical release entries stay as-is — those releases
# were signed under the legacy cosign identity and the URLs in their
# entries remain correct for forensics).
FIND_ARGS=(
    -type f
    \(
        -name '*.md' -o
        -name '*.rs' -o
        -name '*.toml' -o
        -name '*.yml' -o
        -name '*.yaml' -o
        -name '*.sh' -o
        -name '*.rb'
    \)
    ! -path './target/*'
    ! -path './.git/*'
    ! -name 'CHANGELOG.md'
)

# Collect files with any match in a single pass so we can report
# counts + iterate cleanly.
mapfile -t matched_files < <(
    find . "${FIND_ARGS[@]}" -print0 \
        | xargs -0 grep -l -E "${OLD_ORG}/aegis-(boot|hwsim)" 2>/dev/null \
        || true
)

if [[ ${#matched_files[@]} -eq 0 ]]; then
    case "$mode" in
        dry-run|write)
            echo "org-migration-sweep: no legacy URLs found — nothing to do."
            ;;
        check)
            echo "org-migration-sweep: --check PASS — no legacy URLs remain."
            ;;
    esac
    exit 0
fi

# --check mode: report + exit 1 so CI can fail.
if [[ "$mode" == "check" ]]; then
    echo "org-migration-sweep: --check FAIL — ${#matched_files[@]} file(s) still reference legacy org path:" >&2
    for f in "${matched_files[@]}"; do
        count=$(grep -cE "${OLD_ORG}/aegis-(boot|hwsim)" "$f" || true)
        printf '  %s  (%d match(es))\n' "$f" "$count" >&2
    done
    echo "" >&2
    echo "Run ./scripts/org-migration-sweep.sh --write to rewrite, then review + commit." >&2
    exit 1
fi

# dry-run + write share the diff-generation logic; write mode applies
# in place, dry-run only prints what the change WOULD be.
total_matches=0
for f in "${matched_files[@]}"; do
    before="$(grep -cE "${OLD_ORG}/aegis-(boot|hwsim)" "$f" || true)"
    total_matches=$((total_matches + before))
    printf '  %s  (%d match(es))\n' "$f" "$before"

    if [[ "$mode" == "write" ]]; then
        # Apply each old→new pair. sed -i is POSIX-gray; the -e form
        # works on both GNU and BSD sed though BSD's -i needs an empty
        # string. Keep it GNU-only here; the script's target is the
        # Linux maintainer workstation per ORG_MIGRATION_PLAN.md.
        for i in "${!OLD_PATTERNS[@]}"; do
            sed -i "s|${OLD_PATTERNS[$i]}|${NEW_PATTERNS[$i]}|g" "$f"
        done
    fi
done

echo
case "$mode" in
    dry-run)
        echo "org-migration-sweep: dry-run summary — ${total_matches} match(es) across ${#matched_files[@]} file(s)."
        echo "Re-run with --write to actually rewrite, then review + commit."
        ;;
    write)
        echo "org-migration-sweep: write summary — rewrote ${total_matches} match(es) across ${#matched_files[@]} file(s)."
        echo
        echo "Review with: git diff"
        echo "Verify sweep complete: ./scripts/org-migration-sweep.sh --check"
        echo "When happy: git add -u && git commit -m 'chore(org): sweep legacy williamzujkowski/aegis-* URLs to aegis-boot/'"
        ;;
esac
