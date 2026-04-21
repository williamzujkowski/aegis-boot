#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# Boot a REAL USB stick (already dd'd with mkusb.sh output) under
# QEMU+OVMF Secure Boot via USB host passthrough. Safer than
# rebooting the host for first-hardware shakedown (#109).
#
# Usage:
#   scripts/qemu-usb-passthrough.sh 0781:5530         # vendor:product
#   scripts/qemu-usb-passthrough.sh 0781:5530 -i      # GTK display
#   scripts/qemu-usb-passthrough.sh 0781:5530 --dry-run
#
# Prereqs:
#   * Stick is dd'd with out/aegis-boot.img (see mkusb.sh).
#   * Host is NOT using the stick — no auto-mount active.
#   * sudo (QEMU needs root to take over a USB device, or the user
#     needs to be in the appropriate udev group with rules set).
#   * OVMF Secure Boot firmware installed (same as qemu-try.sh).
#
# The stick is taken over by QEMU for the VM's lifetime. When the VM
# exits the host reclaims it (udev re-detects).

set -euo pipefail

VENDOR_PRODUCT="${1:-}"
INTERACTIVE=0
DRY_RUN=0
shift || true
while (( $# > 0 )); do
    case "$1" in
        -i|--interactive) INTERACTIVE=1; shift ;;
        --dry-run)        DRY_RUN=1; shift ;;
        -h|--help)
            grep '^# ' "$0" | sed 's/^# \?//' | head -25
            exit 0
            ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
done

if [[ -z "$VENDOR_PRODUCT" ]]; then
    echo "usage: $0 <vendor:product> [-i] [--dry-run]" >&2
    echo "   e.g. $0 0781:5530" >&2
    echo "find with: lsusb" >&2
    exit 2
fi

if ! [[ "$VENDOR_PRODUCT" =~ ^[0-9a-fA-F]{4}:[0-9a-fA-F]{4}$ ]]; then
    echo "bad vendor:product format (expected xxxx:xxxx hex): $VENDOR_PRODUCT" >&2
    exit 2
fi

VENDOR="0x${VENDOR_PRODUCT%%:*}"
PRODUCT="0x${VENDOR_PRODUCT##*:}"

log()  { printf '[usb-passthrough] %s\n' "$*" >&2; }
warn() { printf '[usb-passthrough][WARN] %s\n' "$*" >&2; }

# Confirm the device is visible and not busy.
if ! lsusb -d "$VENDOR_PRODUCT" >/dev/null 2>&1; then
    echo "device $VENDOR_PRODUCT not found in lsusb output" >&2
    exit 1
fi

log "target USB device: $(lsusb -d "$VENDOR_PRODUCT")"

# Warn loudly if the stick appears to be mounted on the host — QEMU
# will take it away mid-I/O and the filesystem might be flagged dirty.
if mount | grep -qE "^/dev/sd[a-z][0-9]+.*\s+/"; then
    # Check each sdX against the vendor:product via /sys/block/.
    for dev in /sys/block/sd*; do
        [[ -e "$dev/device" ]] || continue
        # Walk up to USB parent and check vid:pid.
        usb_dev=$(readlink -f "$dev/device" 2>/dev/null | grep -oP 'usb[0-9]+/[^/]+' | tail -1 || true)
        [[ -n "$usb_dev" ]] || continue
        vid_file="/sys/bus/usb/devices/${usb_dev##*/}/idVendor"
        pid_file="/sys/bus/usb/devices/${usb_dev##*/}/idProduct"
        if [[ -r "$vid_file" && -r "$pid_file" ]]; then
            vid=$(cat "$vid_file")
            pid=$(cat "$pid_file")
            if [[ "$vid:$pid" == "${VENDOR_PRODUCT,,}" ]]; then
                dev_name=$(basename "$dev")
                if mount | grep -qE "^/dev/${dev_name}[0-9]+"; then
                    warn "stick appears to be mounted on host as /dev/${dev_name}*"
                    warn "unmount BEFORE running this script, or the VM may see a dirty FS"
                fi
            fi
        fi
    done
fi

OVMF_CODE="${OVMF_CODE:-/usr/share/OVMF/OVMF_CODE_4M.secboot.fd}"
OVMF_VARS_SRC="${OVMF_VARS_SRC:-/usr/share/OVMF/OVMF_VARS_4M.ms.fd}"
[[ -r "$OVMF_CODE" ]] || { echo "missing $OVMF_CODE" >&2; exit 1; }
[[ -r "$OVMF_VARS_SRC" ]] || { echo "missing $OVMF_VARS_SRC" >&2; exit 1; }

WORK=$(mktemp -d --tmpdir aegis-usb-passthrough-XXXXXX)
cleanup() { rm -rf -- "$WORK"; }
trap cleanup EXIT
cp "$OVMF_VARS_SRC" "$WORK/vars.fd"
chmod 0644 "$WORK/vars.fd"

display_args=(-nographic -serial mon:stdio)
(( INTERACTIVE )) && display_args=(-display gtk -serial stdio)

# USB host passthrough via qemu-xhci. Vendor:Product match is stable
# across replugs and less brittle than hostbus/hostaddr.
QEMU_ARGS=(
    qemu-system-x86_64
    "${display_args[@]}"
    -machine q35,smm=on
    -global driver=cfi.pflash01,property=secure,value=on
    -m 2048M
    -drive "if=pflash,format=raw,unit=0,file=$OVMF_CODE,readonly=on"
    -drive "if=pflash,format=raw,unit=1,file=$WORK/vars.fd"
    -device qemu-xhci,id=xhci
    -device "usb-host,bus=xhci.0,vendorid=$VENDOR,productid=$PRODUCT"
    -boot menu=on
)

if (( DRY_RUN )); then
    log "DRY RUN — would execute:"
    printf '  sudo '
    printf '%q ' "${QEMU_ARGS[@]}"; echo
    exit 0
fi

# Resolve the host sysfs path for the target device (e.g. "1-3") so
# we can force a re-bind after QEMU exits. xhci_hcd sometimes logs a
# reset but doesn't rebind scsi, leaving /sys/block without an sdX
# node even though lsusb still shows the device (#121). Capturing
# the path BEFORE QEMU takes over avoids having to re-walk sysfs
# when the device is in an intermediate state.
resolve_usb_sysfs_path() {
    for devdir in /sys/bus/usb/devices/*-*; do
        [[ -r "$devdir/idVendor" && -r "$devdir/idProduct" ]] || continue
        local vid pid
        vid=$(cat "$devdir/idVendor")
        pid=$(cat "$devdir/idProduct")
        if [[ "$vid:$pid" == "${VENDOR_PRODUCT,,}" ]]; then
            basename "$devdir"
            return 0
        fi
    done
    return 1
}

USB_SYSFS_ID=""
if ! USB_SYSFS_ID=$(resolve_usb_sysfs_path); then
    warn "could not resolve sysfs path for $VENDOR_PRODUCT; post-exit re-bind will be skipped"
fi

rebind_usb() {
    # Issue #121 workaround. Unbind → bind kicks a fresh enumeration
    # so xhci_hcd re-attaches scsi drivers and /dev/sd* reappears.
    # Errors are non-fatal — the operator can always physically
    # replug — so we log and continue regardless.
    [[ -n "$USB_SYSFS_ID" ]] || return 0
    local unbind="/sys/bus/usb/drivers/usb/unbind"
    local bind="/sys/bus/usb/drivers/usb/bind"
    if [[ ! -w "$unbind" || ! -w "$bind" ]]; then
        # Root-only sysfs path. Use sudo but keep the failure soft.
        log "re-binding $USB_SYSFS_ID via sudo (restores /dev/sdX — #121)"
        if ! echo "$USB_SYSFS_ID" | sudo tee "$unbind" >/dev/null 2>&1; then
            warn "unbind $USB_SYSFS_ID failed; skipping re-bind"
            return 0
        fi
        # Short settle before re-bind — some controllers race otherwise.
        sleep 0.3
        if ! echo "$USB_SYSFS_ID" | sudo tee "$bind" >/dev/null 2>&1; then
            warn "bind $USB_SYSFS_ID failed; physical replug may be required"
        fi
    else
        log "re-binding $USB_SYSFS_ID (restores /dev/sdX — #121)"
        echo "$USB_SYSFS_ID" > "$unbind" || warn "unbind failed"
        sleep 0.3
        echo "$USB_SYSFS_ID" > "$bind" || warn "bind failed"
    fi
}

# Extend the existing EXIT trap so rebind runs even when QEMU is
# killed via signal (Ctrl-C, SIGTERM, etc.) — not just clean exits.
trap 'rebind_usb; cleanup' EXIT

log "booting VM with passthrough; QEMU needs root to claim the device"
log "  * VM will boot OVMF Secure Boot firmware"
log "  * operator picks the USB entry from UEFI menu (hit F12 or esc at OVMF splash)"
log "  * Ctrl-A X exits QEMU; stick returns to host on exit"
# Note: not using `exec sudo ...` here. The trap handler needs to run
# after QEMU exits to issue the sysfs rebind (#121). `exec` replaces
# bash, preventing the trap from firing.
sudo "${QEMU_ARGS[@]}"
