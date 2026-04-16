#!/usr/bin/env bash
# Build a bootable aegis-boot USB image.
#
# Output: a raw disk image the user can either:
#   - `dd if=aegis-boot.img of=/dev/sdX bs=4M oflag=direct` onto a real stick
#   - Boot directly under QEMU with scripts/qemu-try.sh
#
# Layout:
#   GPT disk
#     Part 1 (ESP, FAT32, ~300 MB)
#       /EFI/BOOT/BOOTX64.EFI       Microsoft-signed shim
#       /EFI/BOOT/grubx64.efi       Canonical-signed grub
#       /EFI/BOOT/grub.cfg          minimal menu, chainloads our kernel
#       /EFI/ubuntu/grub.cfg        same (Canonical grub looks here)
#       /vmlinuz                    Canonical-signed kernel
#       /initrd.img                 our aegis-boot initramfs (concat with
#                                   distro initrd for driver coverage)
#     Part 2 (AEGIS_ISOS, FAT32 or ext4, remainder of disk)
#       User drops .iso files here; rescue-tui discovers them.
#
# Requires: the same signed packages we use for OVMF SecBoot E2E CI:
#   shim-signed, grub-efi-amd64-signed, linux-image-generic or -virtual,
#   mtools, dosfstools, gdisk, cpio, gzip.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="${OUT_DIR:-$ROOT_DIR/out}"
IMG="${IMG:-$OUT_DIR/aegis-boot.img}"

# Sizing. ESP is generous enough to hold shim+grub+kernel+initrd with room
# to spare. Data partition takes the rest of the disk.
DISK_SIZE_MB="${DISK_SIZE_MB:-2048}"     # default 2 GB test image
ESP_SIZE_MB="${ESP_SIZE_MB:-400}"
DATA_LABEL="${DATA_LABEL:-AEGIS_ISOS}"
DATA_FS="${DATA_FS:-fat32}"              # fat32 or ext4; ext4 removes 4GB/file cap

# Input binary locations (overridable for cross-builds / packaging).
SHIM_SRC="${SHIM_SRC:-/usr/lib/shim/shimx64.efi.signed}"
GRUB_SRC="${GRUB_SRC:-/usr/lib/grub/x86_64-efi-signed/grubx64.efi.signed}"
KERNEL_SRC="${KERNEL_SRC:-}"   # auto-detected below if not set
INITRD_SRC="${INITRD_SRC:-}"

log() { printf '[mkusb] %s\n' "$*" >&2; }

require() {
    command -v "$1" >/dev/null 2>&1 || {
        echo "missing required tool: $1" >&2
        exit 1
    }
}

require mkfs.vfat
require mcopy
require mmd
require sgdisk
require dd
require stat

# Locate a readable signed kernel + initrd.
if [[ -z "$KERNEL_SRC" ]]; then
    for k in /boot/vmlinuz-*-virtual /boot/vmlinuz-*-generic; do
        [[ -e "$k" && -r "$k" ]] || continue
        KERNEL_SRC="$k"
        ver=$(basename "$k" | sed 's/^vmlinuz-//')
        candidate="/boot/initrd.img-${ver}"
        [[ -r "$candidate" ]] && INITRD_SRC="$candidate"
        break
    done
fi
[[ -n "$KERNEL_SRC" && -r "$KERNEL_SRC" ]] || {
    echo "no readable signed kernel found; set KERNEL_SRC=/path/to/vmlinuz" >&2
    echo "(kernels under /boot are often mode 0600 and need sudo to read)" >&2
    exit 1
}
[[ -n "$INITRD_SRC" && -r "$INITRD_SRC" ]] || {
    echo "no readable distro initrd matching $KERNEL_SRC" >&2
    exit 1
}

for f in "$SHIM_SRC" "$GRUB_SRC"; do
    [[ -r "$f" ]] || {
        echo "missing signed bootloader: $f" >&2
        echo "install: sudo apt-get install shim-signed grub-efi-amd64-signed" >&2
        exit 1
    }
done

# Build our initramfs if it doesn't already exist.
if [[ ! -f "$OUT_DIR/initramfs.cpio.gz" ]]; then
    log "building aegis-boot initramfs"
    "$ROOT_DIR/scripts/build-initramfs.sh"
fi
AEGIS_INITRD="$OUT_DIR/initramfs.cpio.gz"

# Combined initrd: distro initrd + our initramfs. The kernel unpacks both
# cpio segments; our /init loads last and wins. This gives us full distro
# driver coverage + our rescue userland.
WORK="$(mktemp -d --tmpdir aegis-mkusb-XXXXXX)"
trap 'rm -rf -- "$WORK"' EXIT

log "concatenating distro initrd + aegis-boot initramfs"
cat "$INITRD_SRC" "$AEGIS_INITRD" > "$WORK/combined-initrd.img"
log "  distro : $(stat -c '%s' "$INITRD_SRC") bytes"
log "  aegis  : $(stat -c '%s' "$AEGIS_INITRD") bytes"
log "  combined: $(stat -c '%s' "$WORK/combined-initrd.img") bytes"

# grub.cfg — serial console redirect for operator visibility on
# real hardware with a serial port, plus sane defaults for normal boots.
#
# `insmod serial` is required before `serial` / `terminal_*` because
# the signed Canonical grub keeps serial as a loadable module rather
# than built-in. Without insmod, grub prints
#   error: can't find command `serial'.
#   error: terminal `serial' isn't found.
# and falls back to whatever EFI gave it — which on headless QEMU is
# nothing, so the boot menu never reaches a working console. (#109)
cat > "$WORK/grub.cfg" <<'EOF'
insmod serial
serial --unit=0 --speed=115200
terminal_input serial console
terminal_output serial console
set timeout=3

# Normal boot — concise kernel logs.
# console= order MATTERS: last one wins as /dev/console for userspace.
# We want tty0 (local monitor) as the default rescue-tui target on
# real-hardware boots; kernel still echoes to all console= targets
# so a serial operator gets dmesg + can edit grub to flip the order.
# (#112)
menuentry "aegis-boot rescue" {
    linux /vmlinuz console=ttyS0,115200 console=tty0 panic=5 loglevel=4
    initrd /initrd.img
}

# Serial-primary variant — for operators using a serial console or a
# KVM IP console with no local monitor. rescue-tui's alt-screen
# renders on ttyS0.
menuentry "aegis-boot rescue (serial-primary)" {
    linux /vmlinuz console=tty0 console=ttyS0,115200 panic=5 loglevel=4
    initrd /initrd.img
}

# Verbose boot (#109 shakedown) — loglevel=7, earlyprintk, and
# AEGIS_BOOT_VERBOSE=1 causes /init to pause 30s after diagnostics so
# the operator can read the pre-rescue-tui state on screen. Also tees
# the /init log to /run/media/aegis-isos/aegis-boot-<ts>.log.
menuentry "aegis-boot rescue (verbose — first-boot debug)" {
    linux /vmlinuz console=ttyS0,115200 console=tty0 panic=30 loglevel=7 earlyprintk=efi ignore_loglevel aegis.verbose=1
    initrd /initrd.img
}
EOF

# ---- Build ESP partition image ---------------------------------------------
ESP_IMG="$WORK/esp.part"
dd if=/dev/zero of="$ESP_IMG" bs=1M count="$ESP_SIZE_MB" status=none
mkfs.vfat -F 32 -n AEGIS_ESP "$ESP_IMG" >/dev/null

mmd -i "$ESP_IMG" ::/EFI ::/EFI/BOOT ::/EFI/ubuntu
mcopy -i "$ESP_IMG" "$SHIM_SRC"   ::/EFI/BOOT/BOOTX64.EFI
mcopy -i "$ESP_IMG" "$GRUB_SRC"   ::/EFI/BOOT/grubx64.efi
mcopy -i "$ESP_IMG" "$WORK/grub.cfg" ::/EFI/ubuntu/grub.cfg
mcopy -i "$ESP_IMG" "$WORK/grub.cfg" ::/EFI/BOOT/grub.cfg
mcopy -i "$ESP_IMG" "$KERNEL_SRC" ::/vmlinuz
mcopy -i "$ESP_IMG" "$WORK/combined-initrd.img" ::/initrd.img

# ---- Build data partition image (empty FAT32 for user ISOs) ----------------
DATA_SIZE_MB=$((DISK_SIZE_MB - ESP_SIZE_MB - 4))  # -4 MB for GPT + alignment
if (( DATA_SIZE_MB < 32 )); then
    echo "DISK_SIZE_MB ($DISK_SIZE_MB) too small; need at least ESP + 32 MB" >&2
    exit 1
fi
DATA_IMG="$WORK/data.part"
dd if=/dev/zero of="$DATA_IMG" bs=1M count="$DATA_SIZE_MB" status=none
# DATA_FS picks the filesystem for the user's ISO-drop partition.
#   fat32 (default) — cross-OS write-friendly; max 4 GB per file
#   ext4            — no per-file limit; writable from Linux only
case "$DATA_FS" in
    fat32)
        mkfs.vfat -F 32 -n "$DATA_LABEL" "$DATA_IMG" >/dev/null
        ;;
    ext4)
        require mkfs.ext4
        # -F: force (we're writing to a regular file, not a block device)
        # -L: volume label (findfs LABEL=AEGIS_ISOS needs it)
        # -E nodiscard: don't try TRIM on a regular file
        # -O ^has_journal: skip journal on removable media — cleaner dd
        #   output, slightly better wear, initramfs mounts read-only anyway
        mkfs.ext4 -F -L "$DATA_LABEL" -E nodiscard -O ^has_journal \
            "$DATA_IMG" >/dev/null 2>&1
        ;;
    *)
        echo "DATA_FS must be 'fat32' or 'ext4', got: $DATA_FS" >&2
        exit 1
        ;;
esac
log "data partition: $DATA_FS, ${DATA_SIZE_MB} MB, label $DATA_LABEL"

# ---- Assemble the GPT disk -------------------------------------------------
log "assembling GPT disk: $IMG (${DISK_SIZE_MB} MB)"
mkdir -p "$OUT_DIR"
dd if=/dev/zero of="$IMG" bs=1M count="$DISK_SIZE_MB" status=none

# Data partition GUID depends on filesystem: 0700 Microsoft Basic Data
# (what Windows/macOS expect to see for FAT32), 8300 Linux filesystem
# (appropriate for ext4). Both are equally mountable from Linux; the
# type code mostly matters for cross-OS automount behavior.
case "$DATA_FS" in
    fat32) DATA_TYPE="0700" ;;
    ext4)  DATA_TYPE="8300" ;;
esac

sgdisk -o "$IMG" >/dev/null
sgdisk \
    -n 1:2048:+${ESP_SIZE_MB}M -t 1:ef00      -c 1:"EFI System" \
    -n 2:0:0                   -t 2:"$DATA_TYPE" -c 2:"$DATA_LABEL" \
    "$IMG" >/dev/null

# Splice partitions into the disk image. sgdisk reports offsets; derive
# them from the partition table we just wrote.
ESP_START=$(sgdisk -i 1 "$IMG" | awk '/First sector:/ {print $3}')
DATA_START=$(sgdisk -i 2 "$IMG" | awk '/First sector:/ {print $3}')
log "  ESP  @ sector $ESP_START"
log "  data @ sector $DATA_START"

dd if="$ESP_IMG"  of="$IMG" bs=512 seek="$ESP_START"  conv=notrunc status=none
dd if="$DATA_IMG" of="$IMG" bs=512 seek="$DATA_START" conv=notrunc status=none

sha256sum "$IMG" > "$IMG.sha256"

size=$(stat -c '%s' "$IMG")
hash=$(awk '{print $1}' "$IMG.sha256")
log "wrote $IMG ($size bytes)"
log "sha256: $hash"
log ""
log "next steps:"
log "  1. Drop .iso files onto the AEGIS_ISOS partition:"
log "       sudo losetup -fP $IMG"
log "       # find the loop device, then mount the 2nd partition"
log "       sudo mount /dev/loopXp2 /mnt/aegis-isos"
log "       sudo cp ubuntu.iso /mnt/aegis-isos/"
log "       sudo umount /mnt/aegis-isos && sudo losetup -d /dev/loopX"
log "  2. Boot under QEMU to test: scripts/qemu-try.sh (TODO)"
log "  3. dd onto a real USB stick:"
log "       sudo dd if=$IMG of=/dev/sdX bs=4M oflag=direct status=progress"
