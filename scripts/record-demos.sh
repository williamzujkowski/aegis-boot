#!/usr/bin/env bash
# Record the three asciinema demos referenced in #348.
#
# This script is INTERACTIVE — it drives `asciinema rec` against three
# canned aegis-boot flows. Outputs go to docs/demos/casts/ as `.cast`
# files; render-demos.sh converts them to inline-renderable SVG.
#
# Maintainer alignment (#348, 2026-04-25):
#   - Three flows: quickstart, init (3-distro), QEMU+OVMF rescue-tui boot.
#   - Inline-render via SVG (svg-term-cli or agg) for native GitHub
#     markdown — no external host (asciinema.org) dependency.
#   - Below the "What it does" section in README.md.
#
# Prereqs:
#   - asciinema (apt: asciinema, brew: asciinema)
#   - For QEMU demo: qemu-system-x86, ovmf, a built aegis-boot.img
#   - Raw write demos require a sacrificial USB stick; loop-device path
#     is preferred for reproducibility (the Linux-host CI uses loop too).
#
# Usage:
#   tools/record-demos.sh quickstart          # cast 1
#   tools/record-demos.sh init                # cast 2
#   tools/record-demos.sh qemu-boot           # cast 3
#   tools/record-demos.sh all                 # all three sequentially

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CASTS_DIR="$REPO_ROOT/docs/demos/casts"
mkdir -p "$CASTS_DIR"

require() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "error: '$1' not on PATH" >&2
        exit 1
    fi
}

require asciinema

record_quickstart() {
    local target="${AEGIS_DEMO_TARGET:-/dev/loop99}"
    cat <<EOF
record-demos: cast 1 of 3 — aegis-boot quickstart

This demo records:
  sudo aegis-boot quickstart $target
which composes flash + add for a single-distro panic-room stick (Alpine
3.20 default). Operator-facing happy path; sub-10-minute by design.

You will be prompted by asciinema to start the recording. The script
will run quickstart against $target. Press Ctrl-D or 'exit' to stop
recording when the flow finishes.
EOF
    asciinema rec \
        --title "aegis-boot quickstart — single-distro panic-room (Alpine 3.20)" \
        --command "sudo $REPO_ROOT/target/release/aegis-boot quickstart $target" \
        "$CASTS_DIR/01-quickstart.cast"
}

record_init() {
    local target="${AEGIS_DEMO_TARGET:-/dev/loop99}"
    cat <<EOF
record-demos: cast 2 of 3 — aegis-boot init (3-distro)

This demo records:
  sudo aegis-boot init $target --yes
which composes flash + fetch + add for the 3-distro panic-room profile
(Alpine + Ubuntu Server + Rocky). Single attestation manifest covers
the whole run.
EOF
    asciinema rec \
        --title "aegis-boot init — panic-room 3-distro stick" \
        --command "sudo $REPO_ROOT/target/release/aegis-boot init $target --yes" \
        "$CASTS_DIR/02-init.cast"
}

record_qemu_boot() {
    cat <<EOF
record-demos: cast 3 of 3 — QEMU+OVMF rescue-tui boot

This demo records the rescue-tui experience:
  ./scripts/qemu-loaded-stick.sh -d ./test-isos -a usb -i
launches QEMU+OVMF with SecureBoot enforcing, boots the aegis-boot.img,
and lands the operator on the ratatui ISO list. This is what an
operator sees on a real machine.

Press Ctrl-A then x to exit QEMU when the demo flow is done; that's
the cleanest way to stop the recording without leaving the VM running.
EOF
    asciinema rec \
        --title "aegis-boot rescue-tui — booted under QEMU + OVMF SecureBoot" \
        --command "$REPO_ROOT/scripts/qemu-loaded-stick.sh -d $REPO_ROOT/test-isos -a usb -i" \
        "$CASTS_DIR/03-qemu-boot.cast"
}

case "${1:-}" in
    quickstart) record_quickstart ;;
    init) record_init ;;
    qemu-boot) record_qemu_boot ;;
    all) record_quickstart; record_init; record_qemu_boot ;;
    *)
        cat <<EOF >&2
usage: $0 <quickstart|init|qemu-boot|all>

Set AEGIS_DEMO_TARGET to override the default loop device for
quickstart / init (default: /dev/loop99). Recordings land in
docs/demos/casts/. Render to SVG via tools/render-demos.sh.
EOF
        exit 2
        ;;
esac

echo
echo "Recording done. Cast(s) in $CASTS_DIR/"
echo "Next: $REPO_ROOT/scripts/render-demos.sh"
