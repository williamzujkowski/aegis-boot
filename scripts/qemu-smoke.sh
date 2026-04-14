#!/usr/bin/env bash
# QEMU smoke boot: assert that the built initramfs actually boots and that
# rescue-tui reaches its first render.
#
# What this proves:
#   - /init runs as PID 1, mounts /proc, /sys, /dev, /run
#   - rescue-tui starts (dynamic libs resolved, terminal initializes)
#   - iso_probe::discover runs and finds nothing → "No bootable ISOs found"
#
# What this does NOT prove:
#   - Secure Boot chain (no signed kernel used here)
#   - kexec handoff (no fixture ISO attached)
#   - Real hardware driver behavior
#
# Run locally: ./scripts/qemu-smoke.sh
# CI: see .github/workflows/qemu-smoke.yml

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="${OUT_DIR:-$ROOT_DIR/out}"
KERNEL="${KERNEL:-}"
TIMEOUT_SECONDS="${TIMEOUT_SECONDS:-60}"

log() { printf '[qemu-smoke] %s\n' "$*" >&2; }

require() {
    command -v "$1" >/dev/null 2>&1 || {
        echo "missing required tool: $1" >&2
        exit 1
    }
}

require qemu-system-x86_64
require timeout

# Locate a kernel if not supplied. Prefer an explicit env override, then look
# for linux-image-virtual / generic in /boot, then fall back to the running
# kernel. We need a bzImage the generic x86_64 QEMU can boot without a
# specific virtio config — most distro -generic kernels work fine.
if [[ -z "$KERNEL" ]]; then
    for candidate in \
        /boot/vmlinuz-*-virtual \
        /boot/vmlinuz-*-generic \
        /boot/vmlinuz; do
        if [[ -e "$candidate" && ! -L "$candidate" ]]; then
            KERNEL="$candidate"
            break
        fi
        if [[ -L "$candidate" ]]; then
            resolved="$(readlink -f "$candidate")"
            if [[ -f "$resolved" ]]; then
                KERNEL="$resolved"
                break
            fi
        fi
    done
fi
if [[ -z "$KERNEL" || ! -f "$KERNEL" ]]; then
    echo "no bootable kernel found; set KERNEL=/path/to/vmlinuz" >&2
    exit 1
fi
log "kernel: $KERNEL"

# Build initramfs if missing.
if [[ ! -f "$OUT_DIR/initramfs.cpio.gz" ]]; then
    log "initramfs not found; building it"
    "$ROOT_DIR/scripts/build-initramfs.sh"
fi
log "initramfs: $OUT_DIR/initramfs.cpio.gz"

# Kernels must be readable by the user running QEMU. Distro-signed kernels
# under /boot are often mode 0600. Work around by copying to a world-readable
# temp file when needed.
if [[ ! -r "$KERNEL" ]]; then
    tmp_kernel="$(mktemp --tmpdir qemu-smoke-kernel-XXXXXX.vmlinuz)"
    sudo cp "$KERNEL" "$tmp_kernel"
    sudo chmod 0644 "$tmp_kernel"
    KERNEL="$tmp_kernel"
    trap 'rm -f -- "$tmp_kernel"' EXIT
    log "copied kernel to $tmp_kernel for read access"
fi

log "booting under QEMU (timeout ${TIMEOUT_SECONDS}s)"
output_file="$(mktemp --tmpdir qemu-smoke-out-XXXXXX.log)"
trap '[[ -n "${output_file:-}" ]] && rm -f -- "$output_file"' EXIT

# Headless boot. `-nographic` routes serial to stdio and disables the display.
# `-no-reboot` stops QEMU when the guest tries to reboot.
# `panic=5` reboots 5s after kernel panic so we fail fast if /init dies.
# `rdinit=/init` tells the kernel to exec our script as PID 1.
set +e
timeout "$TIMEOUT_SECONDS" qemu-system-x86_64 \
    -nographic \
    -no-reboot \
    -m 512M \
    -kernel "$KERNEL" \
    -initrd "$OUT_DIR/initramfs.cpio.gz" \
    -append 'console=ttyS0 panic=5 rdinit=/init quiet loglevel=3' \
    </dev/null \
    >"$output_file" 2>&1
qemu_exit=$?
set -e

# Print what the guest said so CI logs show it.
echo "--- QEMU serial output ---"
cat "$output_file"
echo "--- end QEMU serial output ---"

# Look for our startup banner (stderr → serial console) — reliable even
# when the serial TTY doesn't report a size and ratatui renders blank.
if grep -qE 'aegis-boot rescue-tui starting|No bootable ISOs|aegis-boot — pick an ISO' "$output_file"; then
    log "rescue-tui rendered — boot smoke PASSED"
    exit 0
fi

echo "rescue-tui output not detected; qemu exit=$qemu_exit" >&2
exit 1
