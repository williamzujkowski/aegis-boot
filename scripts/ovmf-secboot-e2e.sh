#!/usr/bin/env bash
# OVMF SecBoot end-to-end test (Phase 2 of issue #16).
#
# Boots a real signed shim → signed grub → signed Canonical kernel chain
# under OVMF SecBoot enforcing, with our `initramfs.cpio.gz` concatenated
# onto the distro initrd. Asserts:
#   1. Linux kernel logs "Secure boot enabled".
#   2. rescue-tui's startup banner appears.
#
# Together this proves the deployment story end-to-end: a signed distro
# kernel can carry our rescue payload through an enforcing SB chain and
# our binary runs.
#
# What this does NOT prove:
#   - Real-world MOK enrollment for unsigned ISO kernels (deployment task).
#   - kexec handoff from rescue-tui (still pending #29 follow-up).

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="${OUT_DIR:-$ROOT_DIR/out}"
TIMEOUT_SECONDS="${TIMEOUT_SECONDS:-120}"

OVMF_CODE="${OVMF_CODE:-/usr/share/OVMF/OVMF_CODE_4M.secboot.fd}"
OVMF_VARS_SRC="${OVMF_VARS_SRC:-/usr/share/OVMF/OVMF_VARS_4M.ms.fd}"
SHIM_SRC="${SHIM_SRC:-/usr/lib/shim/shimx64.efi.signed}"
GRUB_SRC="${GRUB_SRC:-/usr/lib/grub/x86_64-efi-signed/grubx64.efi.signed}"

log() { printf '[ovmf-e2e] %s\n' "$*" >&2; }

require() {
    command -v "$1" >/dev/null 2>&1 || {
        echo "missing required tool: $1" >&2
        exit 1
    }
}

require qemu-system-x86_64
require mkfs.vfat
require mcopy
require mmd
require timeout

for f in "$OVMF_CODE" "$OVMF_VARS_SRC" "$SHIM_SRC" "$GRUB_SRC"; do
    [[ -r "$f" ]] || {
        echo "missing or unreadable: $f" >&2
        exit 1
    }
done

# Find a readable signed kernel + initrd.
KERNEL=""
INITRD=""
for k in /boot/vmlinuz-*-generic /boot/vmlinuz-*-virtual; do
    [[ -e "$k" ]] || continue
    if [[ -r "$k" ]]; then
        KERNEL="$k"
        ver=$(basename "$k" | sed 's/^vmlinuz-//')
        candidate="/boot/initrd.img-${ver}"
        if [[ -r "$candidate" ]]; then
            INITRD="$candidate"
        fi
        break
    fi
done
[[ -n "$KERNEL" ]] || {
    echo "no readable /boot/vmlinuz-*-{generic,virtual} found" >&2
    exit 1
}
[[ -n "$INITRD" ]] || {
    echo "no matching initrd for $KERNEL" >&2
    exit 1
}
log "kernel: $KERNEL"
log "initrd: $INITRD"

# Build initramfs if missing.
if [[ ! -f "$OUT_DIR/initramfs.cpio.gz" ]]; then
    log "building rescue initramfs"
    "$ROOT_DIR/scripts/build-initramfs.sh"
fi

# Concatenate Ubuntu's initrd + ours. The kernel unpacks all cpio segments
# in order; ours runs LAST so /init wins over Ubuntu's.
WORK="$(mktemp -d --tmpdir aegis-secboot-e2e-XXXXXX)"
trap 'rm -rf -- "$WORK"' EXIT
cat "$INITRD" "$OUT_DIR/initramfs.cpio.gz" > "$WORK/combined.img"
log "combined initrd: $(stat -c '%s' "$WORK/combined.img") bytes"

# Build minimal grub.cfg.
cat > "$WORK/grub.cfg" <<'EOF'
set timeout=0
menuentry "aegis-boot e2e" {
    linux /vmlinuz console=ttyS0 panic=5 loglevel=4 quiet
    initrd /initrd.img
}
EOF

# Build the FAT32 ESP image.
ESP_SIZE_MB=200
ESP_IMG="$WORK/esp.img"
dd if=/dev/zero of="$ESP_IMG" bs=1M count="$ESP_SIZE_MB" status=none
mkfs.vfat -F 32 -n AEGIS_ESP "$ESP_IMG" >/dev/null

mmd -i "$ESP_IMG" ::/EFI ::/EFI/BOOT
mcopy -i "$ESP_IMG" "$SHIM_SRC" ::/EFI/BOOT/BOOTX64.EFI
mcopy -i "$ESP_IMG" "$GRUB_SRC" ::/EFI/BOOT/grubx64.efi
mcopy -i "$ESP_IMG" "$WORK/grub.cfg" ::/EFI/BOOT/grub.cfg
mcopy -i "$ESP_IMG" "$KERNEL" ::/vmlinuz
mcopy -i "$ESP_IMG" "$WORK/combined.img" ::/initrd.img

# Prepare writable OVMF vars copy.
cp "$OVMF_VARS_SRC" "$WORK/vars.fd"
chmod 0644 "$WORK/vars.fd"

OUTPUT="$WORK/serial.log"
log "booting QEMU under OVMF SecBoot (timeout ${TIMEOUT_SECONDS}s)"
set +e
timeout "$TIMEOUT_SECONDS" qemu-system-x86_64 \
    -nographic \
    -no-reboot \
    -m 1024M \
    -drive "if=pflash,format=raw,unit=0,file=$OVMF_CODE,readonly=on" \
    -drive "if=pflash,format=raw,unit=1,file=$WORK/vars.fd" \
    -drive "if=virtio,format=raw,file=$ESP_IMG" \
    -serial mon:stdio \
    </dev/null \
    >"$OUTPUT" 2>&1
qemu_exit=$?
set -e

echo "--- QEMU serial output (last 80 lines) ---"
tail -80 "$OUTPUT"
echo "--- end QEMU serial output ---"

PASS=1
if grep -qiE 'secure boot enabled|secureboot.*enabled' "$OUTPUT"; then
    log "kernel reported Secure Boot enabled"
else
    log "MISS: kernel didn't log 'secure boot enabled'"
    PASS=0
fi

if grep -q 'aegis-boot rescue-tui starting' "$OUTPUT"; then
    log "rescue-tui startup banner observed"
else
    log "MISS: no 'aegis-boot rescue-tui starting' banner"
    PASS=0
fi

if [[ "$PASS" -eq 1 ]]; then
    log "SecBoot E2E: PASS (signed chain → SB enforcing → rescue-tui ran)"
    exit 0
fi

log "SecBoot E2E: FAIL (qemu_exit=$qemu_exit; see serial log above)"
exit 1
