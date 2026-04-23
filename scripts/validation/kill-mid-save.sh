#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Kill-mid-save durability test for ADR 0003's save_durable protocol.
# Launches save_smoke with many iterations, SIGKILLs at a random
# offset, then verifies the final-file invariants:
#
#   - If `last-choice.json` exists → MUST parse as valid JSON
#   - Stale `last-choice.json.tmp` files are accepted (load path reads
#     only `last-choice.json`)
#
# See scripts/validation/README.md for setup + caveats.
#
# Required env:
#   AEGIS_ISOS_DEV     path to the USB device (e.g. /dev/sda)
#
# Optional env:
#   RUNS               number of kill trials (default 10)
#   MOUNT              mount point for AEGIS_ISOS (default /mnt/aegis-isos)
#   SMOKE              path to save_smoke binary (default /tmp/aegis-hw-test/save_smoke)
#   MIN_DELAY_MS       min kill delay (default 10)
#   MAX_DELAY_MS       max kill delay (default 500)
#   ITERS_PER_RUN      save iterations per trial (default 10000)
#
# Exits 0 on PASS=RUNS + CORRUPT=0, else 1.

set -uo pipefail

RUNS="${RUNS:-10}"
MOUNT="${MOUNT:-/mnt/aegis-isos}"
SMOKE="${SMOKE:-/tmp/aegis-hw-test/save_smoke}"
MIN_DELAY_MS="${MIN_DELAY_MS:-10}"
MAX_DELAY_MS="${MAX_DELAY_MS:-500}"
ITERS="${ITERS_PER_RUN:-10000}"

if [[ -z "${AEGIS_ISOS_DEV:-}" ]]; then
    echo "AEGIS_ISOS_DEV env var required" >&2
    exit 2
fi
if [[ ! -x "$SMOKE" ]]; then
    echo "save_smoke not found at $SMOKE — build via 'rustc scripts/validation/save_smoke.rs -O -o $SMOKE'" >&2
    exit 2
fi

sudo mkdir -p "$MOUNT"
PASS=0
CORRUPT=0
LEFTOVER_TMP=0

for run in $(seq 1 "$RUNS"); do
    sudo mount "$AEGIS_ISOS_DEV"2 "$MOUNT"
    sudo AEGIS_ISOS_MOUNT="$MOUNT" ITERS="$ITERS" "$SMOKE" \
        >/tmp/smoke-run-$run.out 2>&1 &
    SPID=$!

    # Random delay in [MIN_DELAY_MS, MAX_DELAY_MS)
    range=$((MAX_DELAY_MS - MIN_DELAY_MS))
    delay_ms=$((RANDOM % range + MIN_DELAY_MS))
    # sleep accepts fractional seconds; pad to 3 digits
    sleep "0.$(printf "%03d" "$delay_ms")"

    sudo kill -9 "$SPID" 2>/dev/null
    wait "$SPID" 2>/dev/null
    sudo sync

    state_dir="$MOUNT/.aegis-state"
    final="$state_dir/last-choice.json"
    tmp="$state_dir/last-choice.json.tmp"

    status="PASS"
    note=""

    if sudo test -f "$final"; then
        if sudo cat "$final" | python3 -c 'import json,sys; json.load(sys.stdin)' 2>/dev/null; then
            status="PASS"
            note="(final-present, parses)"
            PASS=$((PASS + 1))
        else
            status="CORRUPT"
            note="(final-present, BAD JSON)"
            CORRUPT=$((CORRUPT + 1))
        fi
    else
        # No final file — kill happened before first rename, accept
        PASS=$((PASS + 1))
        note="(no final; kill before first rename)"
    fi

    if sudo test -f "$tmp"; then
        LEFTOVER_TMP=$((LEFTOVER_TMP + 1))
        note="$note  [leftover .tmp]"
    fi

    echo "run $(printf '%2d' "$run") kill-at=$(printf '%3dms' "$delay_ms")  $status $note"

    sudo umount "$MOUNT" || sudo umount -l "$MOUNT"
done

echo "---"
echo "PASS=$PASS  CORRUPT=$CORRUPT  (of $RUNS runs)  leftover-tmp=$LEFTOVER_TMP"

if [[ "$CORRUPT" -eq 0 && "$PASS" -eq "$RUNS" ]]; then
    exit 0
else
    exit 1
fi
