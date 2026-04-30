#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
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

# QEMU_SMOKE_TEST_MODE (optional) — when set to one of the closed-set
# dispatcher slugs, append `aegis.test=<NAME>` to the kernel cmdline so
# /init's grep-and-export hook (PR #680) flips rescue-tui into the
# named scripted mode. Default success grep also switches to the
# test-mode start landmark instead of the rescue-tui banner.
#
# Used by tools/local-ci.sh test-mode <NAME> to smoke each test mode
# locally without a full aegis-hwsim run.
QEMU_SMOKE_TEST_MODE="${QEMU_SMOKE_TEST_MODE:-}"
EXTRA_CMDLINE=""
SUCCESS_GREP='aegis-boot rescue-tui starting|No bootable ISOs|aegis-boot — pick an ISO'
case "$QEMU_SMOKE_TEST_MODE" in
    "")
        ;;
    kexec-unsigned|mok-enroll|manifest-roundtrip)
        EXTRA_CMDLINE=" aegis.test=${QEMU_SMOKE_TEST_MODE}"
        # Grep for the per-mode start landmark (substring-stable per
        # docs/rescue-tui-serial-format.md). All three modes emit
        # `aegis-boot-test: <NAME> starting` (kexec-unsigned) or a
        # close variant (`MOK enrollment walkthrough starting`,
        # `manifest-roundtrip starting`); the broad regex below
        # covers all three.
        SUCCESS_GREP="aegis-boot-test: ${QEMU_SMOKE_TEST_MODE}|MOK enrollment walkthrough starting|manifest-roundtrip starting"
        log "test-mode smoke: ${QEMU_SMOKE_TEST_MODE} (cmdline gets aegis.test=${QEMU_SMOKE_TEST_MODE})"
        ;;
    *)
        echo "qemu-smoke: ERROR — unknown QEMU_SMOKE_TEST_MODE '$QEMU_SMOKE_TEST_MODE'" >&2
        echo "  valid: kexec-unsigned, mok-enroll, manifest-roundtrip" >&2
        exit 2
        ;;
esac

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
    -append "console=ttyS0 panic=5 rdinit=/init quiet loglevel=3${EXTRA_CMDLINE}" \
    </dev/null \
    >"$output_file" 2>&1
qemu_exit=$?
set -e

# Print what the guest said so CI logs show it.
echo "--- QEMU serial output ---"
cat "$output_file"
echo "--- end QEMU serial output ---"

# Look for the success landmark. Default: rescue-tui banner. Test-mode:
# the per-mode start landmark.
if grep -qE "$SUCCESS_GREP" "$output_file"; then
    if [[ -n "$QEMU_SMOKE_TEST_MODE" ]]; then
        log "test-mode '${QEMU_SMOKE_TEST_MODE}' dispatched — start landmark detected"
    else
        log "rescue-tui rendered — boot smoke PASSED"
    fi
    exit 0
fi

if [[ -n "$QEMU_SMOKE_TEST_MODE" ]]; then
    echo "test-mode '${QEMU_SMOKE_TEST_MODE}' did not fire; qemu exit=$qemu_exit" >&2
    echo "  expected substring: $SUCCESS_GREP" >&2
else
    echo "rescue-tui output not detected; qemu exit=$qemu_exit" >&2
fi
exit 1
