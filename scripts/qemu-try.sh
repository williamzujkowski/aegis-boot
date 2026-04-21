#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# Boot a mkusb-produced image under QEMU+OVMF SecBoot for developer testing.
#
# Usage:
#   scripts/mkusb.sh               # builds out/aegis-boot.img
#   scripts/qemu-try.sh            # boots it under QEMU with OVMF SecBoot
#
# Optional: drop ISOs into the AEGIS_ISOS data partition before booting so
# the TUI has something to list. See docs/USB_LAYOUT.md.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="${OUT_DIR:-$ROOT_DIR/out}"
IMG="${IMG:-$OUT_DIR/aegis-boot.img}"

OVMF_CODE="${OVMF_CODE:-/usr/share/OVMF/OVMF_CODE_4M.secboot.fd}"
OVMF_VARS_SRC="${OVMF_VARS_SRC:-/usr/share/OVMF/OVMF_VARS_4M.ms.fd}"

log() { printf '[qemu-try] %s\n' "$*" >&2; }

[[ -f "$IMG" ]] || {
    echo "image not found: $IMG" >&2
    echo "build first: scripts/mkusb.sh" >&2
    exit 1
}

WORK="$(mktemp -d --tmpdir aegis-qemu-try-XXXXXX)"
trap 'rm -rf -- "$WORK"' EXIT
cp "$OVMF_VARS_SRC" "$WORK/vars.fd"
chmod 0644 "$WORK/vars.fd"

log "booting $IMG under OVMF SecBoot"
log "  (Ctrl-A X to exit QEMU; timeouts NOT applied — interactive session)"

exec qemu-system-x86_64 \
    -nographic \
    -machine q35,smm=on \
    -global driver=cfi.pflash01,property=secure,value=on \
    -m 2048M \
    -drive "if=pflash,format=raw,unit=0,file=$OVMF_CODE,readonly=on" \
    -drive "if=pflash,format=raw,unit=1,file=$WORK/vars.fd" \
    -drive "if=ide,format=raw,file=$IMG" \
    -boot order=c \
    -serial mon:stdio \
    "$@"
