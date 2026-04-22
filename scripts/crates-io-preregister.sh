#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# crates-io-preregister.sh — claim all 8 aegis-boot workspace crate
# names as v0.0.0 placeholder crates BEFORE the GitHub org transfer
# so a squatter can't race between "repo moves to aegis-boot/" and
# "we claim the name under the new org's cargo account."
#
# See docs/governance/ORG_MIGRATION_PLAN.md §1 for rationale and
# post-transfer `cargo owner --add aegis-boot` step.
#
# Usage:
#   # Option A — token in `pass` (recommended — stays out of shell history):
#   pass show aegis-boot/crates-io-token | cargo login
#
#   # Option B — interactive:
#   cargo login   # prompts for the token
#
#   # Then run the script:
#   ./scripts/crates-io-preregister.sh
#
# Runs from YOUR (personal) cargo account. All 8 placeholders get
# owner = you; after the GitHub transfer, you'll run
# `cargo owner --add aegis-boot <crate>` to add the org as co-owner
# (the org needs a linked cargo account first — that's a post-transfer
# UI step on crates.io).
#
# Idempotent: re-running after a partial success succeeds for the
# unpublished crates and no-ops for already-published (cargo publish
# errors out on existing names; we catch that and continue).
#
# Exit codes:
#   0  all 8 crates published (or already existed)
#   1  one or more publishes failed for reasons other than "already exists"

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" &>/dev/null && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# The 8 crate names we ship, plus aegis-hwsim.
# Order doesn't matter — each placeholder is self-contained.
#
# Note on `aegis-bootctl` vs the repo's `crates/aegis-cli/`: the
# operator CLI in `crates/aegis-cli/` ships with package name
# `aegis-cli` on disk today, but `aegis-cli` on crates.io is taken
# by an unrelated "Aegis Authenticator" TOTP tool (v1.3.95). The
# in-workspace package name will be renamed to `aegis-bootctl` in a
# follow-up PR; this placeholder reserves the new name now. The
# binary itself is `aegis-boot` regardless of the package name.
CRATES=(
    aegis-boot
    aegis-bootctl
    aegis-wire-formats
    aegis-fitness
    aegis-hwsim
    iso-parser
    iso-probe
    kexec-loader
)

# Work under a pristine /tmp dir so we don't pollute the repo with
# 8 throwaway Cargo projects.
WORK_DIR="$(mktemp -d -t aegis-preregister-XXXXXX)"
trap 'rm -rf "$WORK_DIR"' EXIT
cd "$WORK_DIR"

failed=0
already_existed=0
published=0

for crate in "${CRATES[@]}"; do
    printf '\n=== Preregistering %s v0.0.0 ===\n' "$crate"
    cargo new --lib "$crate" >/dev/null
    cd "$crate"

    # Minimal valid v0.0.0 crate. Description is required for publish.
    cat > Cargo.toml <<CARGO
[package]
name = "$crate"
version = "0.0.0"
edition = "2021"
description = "Placeholder — canonical home will be https://github.com/aegis-boot/aegis-boot."
license = "MIT OR Apache-2.0"
repository = "https://github.com/aegis-boot/aegis-boot"
readme = "README.md"

[lib]
CARGO

    cat > README.md <<README
# $crate

Placeholder v0.0.0 — the canonical home for this crate will be
\`https://github.com/aegis-boot/aegis-boot\` once the GitHub org
transfer (see \`docs/governance/ORG_MIGRATION_PLAN.md\`) completes.

A real release will ship under a future non-0.0.0 version. If you
depend on this placeholder, pin to a later version once the real
crate lands; the 0.0.0 placeholder is here purely to reserve the
name and prevent squatting during the repo transition.
README

    # Attempt publish. Capture the failure reason so we can
    # distinguish "already exists" (idempotent OK) from "broke."
    if cargo publish --allow-dirty 2>&1 | tee /tmp/publish.log; then
        published=$((published + 1))
        printf '  → published.\n'
    else
        if grep -qi "already exists\|already published\|invalid version\|conflict" /tmp/publish.log; then
            already_existed=$((already_existed + 1))
            printf '  → already claimed (idempotent OK).\n'
        else
            failed=$((failed + 1))
            printf '  → FAILED with unexpected error. See /tmp/publish.log above.\n' >&2
        fi
    fi
    cd ..
done

printf '\n--- Summary ---\n'
printf '  Published now:  %d\n' "$published"
printf '  Already claimed: %d\n' "$already_existed"
printf '  Failed:          %d\n' "$failed"
printf '  Total:           %d\n' "${#CRATES[@]}"

if [[ "$failed" -gt 0 ]]; then
    printf '\nOne or more unexpected failures. Review the cargo publish output above,\n' >&2
    printf 'fix the issue, and re-run — the script is idempotent on the already-claimed names.\n' >&2
    exit 1
fi

printf '\nNext step (POST-TRANSFER): after the GitHub repo moves to\n'
printf 'aegis-boot/aegis-boot, the org needs a linked crates.io account. Once that\n'
printf 'is set up (UI step on crates.io), run:\n\n'
for crate in "${CRATES[@]}"; do
    printf '  cargo owner --add aegis-boot %s\n' "$crate"
done
printf '\nto transfer co-ownership of each placeholder to the org.\n'
