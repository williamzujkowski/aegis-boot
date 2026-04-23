#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Boot a flashed USB stick under QEMU + OVMF SecBoot enforcing, with
# USB passthrough of a host block device. Serial console → stdout.
#
# Part of the ADR 0003 persistence validation harness. See
# scripts/validation/README.md for context.
#
# Required env:
#   AEGIS_ISOS_DEV     path to the USB device (e.g. /dev/sda)
#
# Optional env:
#   TIMEOUT_S          QEMU timeout in seconds (default 180)
#   OVMF_CODE          path to OVMF code file
#                        (default /usr/share/OVMF/OVMF_CODE_4M.secboot.fd)
#   OVMF_VARS_SRC      source vars file (copied to $WORK/vars.fd)
#                        (default /usr/share/OVMF/OVMF_VARS_4M.ms.fd)
#   WORK               working directory (default /tmp/aegis-hw-test)
#
# Exits with QEMU's exit code (or 124 on timeout).

set -u
TIMEOUT_S="${TIMEOUT_S:-180}"
OVMF_CODE="${OVMF_CODE:-/usr/share/OVMF/OVMF_CODE_4M.secboot.fd}"
OVMF_VARS_SRC="${OVMF_VARS_SRC:-/usr/share/OVMF/OVMF_VARS_4M.ms.fd}"
WORK="${WORK:-/tmp/aegis-hw-test}"

if [[ -z "${AEGIS_ISOS_DEV:-}" ]]; then
    echo "AEGIS_ISOS_DEV env var required (e.g. AEGIS_ISOS_DEV=/dev/sda)" >&2
    exit 2
fi
if [[ ! -b "$AEGIS_ISOS_DEV" ]]; then
    echo "$AEGIS_ISOS_DEV is not a block device" >&2
    exit 2
fi

mkdir -p "$WORK"
cp -f "$OVMF_VARS_SRC" "$WORK/vars.fd"

echo "Booting $AEGIS_ISOS_DEV under QEMU + OVMF SecBoot enforcing" >&2
echo "  timeout=${TIMEOUT_S}s  work=$WORK" >&2
echo "  vars=$WORK/vars.fd  code=$OVMF_CODE" >&2
echo >&2

exec timeout --foreground "$TIMEOUT_S" sudo qemu-system-x86_64 \
    -nographic \
    -no-reboot \
    -machine q35,smm=on \
    -global driver=cfi.pflash01,property=secure,value=on \
    -m 2048M \
    -cpu host -enable-kvm \
    -drive "if=pflash,format=raw,unit=0,file=$OVMF_CODE,readonly=on" \
    -drive "if=pflash,format=raw,unit=1,file=$WORK/vars.fd" \
    -drive "if=none,id=usb0,file=$AEGIS_ISOS_DEV,format=raw,cache=none" \
    -device qemu-xhci,id=xhci \
    -device "usb-storage,drive=usb0,bootindex=0" \
    -serial mon:stdio \
    </dev/null
