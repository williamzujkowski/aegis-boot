#!/bin/bash
# distro-smoke orchestrator. See README.md.
#
# Usage:
#   ./run.sh                 # run all distros from distros.sh
#   ./run.sh opensuse        # run just one
#   AEGIS_BIN=/path ./run.sh # override mounted binary
#
# Exits 0 if every distro hit "=== DISTRO-SMOKE END ===" with no
# [FAIL] rows in `aegis-boot doctor`; non-zero on any failure.

set -eu

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$HERE/../.." && pwd)"
INSTALL_SH="$REPO_ROOT/scripts/install.sh"

# Prefer the static-musl build — that's what release.yml actually ships
# and it runs inside any distro regardless of glibc version (including
# Alpine, whose musl libc is ABI-incompatible with glibc-linked binaries).
# Fall back to the dynamic-glibc build if musl hasn't been built.
AEGIS_BIN="${AEGIS_BIN:-$REPO_ROOT/target/x86_64-unknown-linux-musl/release/aegis-boot}"
if [ ! -x "$AEGIS_BIN" ]; then
    AEGIS_BIN="$REPO_ROOT/target/release/aegis-boot"
fi
if [ ! -x "$AEGIS_BIN" ]; then
    echo "run.sh: error: release binary not found. Build first:" >&2
    echo "    cargo build --release -p aegis-cli" >&2
    echo "    # or for the release-parity build:" >&2
    echo "    cargo build --release -p aegis-cli --target x86_64-unknown-linux-musl" >&2
    exit 2
fi

# shellcheck disable=SC1091
. "$HERE/distros.sh"

RUN_ID="$(date -u +%Y%m%d-%H%M%S)"
OUTDIR="$HERE/output/$RUN_ID"
mkdir -p "$OUTDIR"

echo "run.sh: binary:  $AEGIS_BIN"
echo "run.sh: output:  $OUTDIR"
echo "run.sh: distros: $(echo "$DISTROS" | grep -c '|' || true)"
echo

FILTER="${1:-}"

run_one() {
    local name="$1" image="$2" probe_fn="$3"
    local logfile="$OUTDIR/$name.log"
    local start_ts
    start_ts="$(date -u +%s)"
    echo "[$name] image=$image ..."
    # shellcheck disable=SC2091
    if ! "$probe_fn" | docker run --rm -i \
        --name "aegis-smoke-$name-$RUN_ID" \
        -v "$AEGIS_BIN:/usr/local/bin/aegis-boot:ro" \
        -v "$INSTALL_SH:/usr/local/bin/install-sh:ro" \
        "$image" \
        /bin/sh >"$logfile" 2>&1; then
        echo "[$name] FAIL — see $logfile"
    fi
    local end_ts elapsed
    end_ts="$(date -u +%s)"
    elapsed=$((end_ts - start_ts))
    # Verify clean exit marker.
    if grep -q "=== DISTRO-SMOKE END ===" "$logfile"; then
        echo "[$name] completed in ${elapsed}s"
    else
        echo "[$name] INCOMPLETE (no END marker) in ${elapsed}s — tail:"
        tail -5 "$logfile" | sed 's/^/[   ] /'
    fi
}

while IFS='|' read -r name image probe_fn; do
    [ -z "$name" ] && continue
    [ -n "$FILTER" ] && [ "$FILTER" != "$name" ] && continue
    run_one "$name" "$image" "$probe_fn"
done <<EOF
$(echo "$DISTROS" | grep -v '^$')
EOF

# Summary: scan each log for the two signals we care about.
SUMMARY="$OUTDIR/summary.md"
{
    echo "# distro-smoke run $RUN_ID"
    echo
    echo "Binary: \`$AEGIS_BIN\`"
    echo
    echo "| distro | sgdisk found? | FAIL rows | WARN rows | score |"
    echo "|---|---|---|---|---|"
    for log in "$OUTDIR"/*.log; do
        [ -f "$log" ] || continue
        name="$(basename "$log" .log)"
        # Count FAIL/WARN in the human-format doctor output. We scope
        # to the first `=== DOCTOR (human) ===` block so repeats (from
        # both the human and --json invocations) don't double-count.
        doctor_block="$(awk '/=== DOCTOR \(human\) ===/{flag=1;next}/=== DOCTOR \(json\) ===/{flag=0}flag' "$log" 2>/dev/null)"
        fails="$(printf '%s\n' "$doctor_block" | grep -c '\[. FAIL\]' || true)"
        warns="$(printf '%s\n' "$doctor_block" | grep -c '\[. WARN\]' || true)"
        fails="${fails:-0}"
        warns="${warns:-0}"
        # Pull sgdisk row status.
        sgdisk_row="$(grep -E '\[.\s+(PASS|WARN|FAIL)\]\s+command: sgdisk' "$log" | head -1 || true)"
        if echo "$sgdisk_row" | grep -q "PASS"; then
            sgdisk="PASS"
        elif echo "$sgdisk_row" | grep -q "FAIL"; then
            sgdisk="FAIL ← false-negative if binary is in /usr/sbin"
        else
            sgdisk="?"
        fi
        score="$(grep -oE 'Health score: [0-9]+' "$log" | head -1 | awk '{print $3}' || true)"
        echo "| $name | $sgdisk | $fails | $warns | ${score:-?} |"
    done
    echo
    echo "Full logs: \`$OUTDIR/<distro>.log\`"
} >"$SUMMARY"

echo
echo "run.sh: summary at $SUMMARY"
cat "$SUMMARY"
