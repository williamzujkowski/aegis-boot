#!/usr/bin/env bash
# Build a fresh aegis-boot stick image, copy real ISOs onto its
# AEGIS_ISOS data partition, then boot the result under QEMU+OVMF
# Secure Boot — the closest no-real-USB simulation of the operator
# experience.
#
# Usage:
#   scripts/qemu-loaded-stick.sh                       # ./test-isos
#   scripts/qemu-loaded-stick.sh -d ~/Downloads/isos
#   scripts/qemu-loaded-stick.sh -d ./test-isos -i     # interactive GTK
#   scripts/qemu-loaded-stick.sh -d ./test-isos -k     # keep image
#   scripts/qemu-loaded-stick.sh -d ./test-isos --dry-run
#
# Prereqs (same as dev-test.sh): qemu-system-x86, ovmf,
# shim-signed, grub-efi-amd64-signed, linux-image-generic, mtools,
# dosfstools, exfatprogs, gdisk, busybox-static, cpio, xorriso, util-linux,
# AND sudo for the AEGIS_ISOS loop-mount step.
#
# Closes #66.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ISO_DIR="$ROOT_DIR/test-isos"
INTERACTIVE=0
KEEP=0
DRY_RUN=0
SIZE_MB=""
ATTACH="virtio"  # virtio | sata | usb

usage() {
    cat <<USAGE
Usage: $0 [options]

  -d, --iso-dir DIR   directory of .iso files to load (default: ./test-isos)
  -s, --size MB       disk image size in MiB
                      (default: max(2048, ceil(1.5 * sum_of_iso_sizes)))
  -a, --attach MODE   how to attach the stick to the VM:
                        virtio  paravirtual virtio-blk (default, fastest,
                                needs no storage modules in initramfs)
                        sata    AHCI SATA — exercises ahci.ko path that
                                real desktops and laptops use
                        usb     usb-storage on xHCI — closest match for a
                                real USB stick plugged into a host
  -i, --interactive   QEMU GTK display instead of -nographic serial
  -k, --keep          keep the built image after exit (default: cleanup)
      --dry-run       print the QEMU invocation, do not run
  -h, --help          this message
USAGE
}

while (( $# > 0 )); do
    case "$1" in
        -d|--iso-dir)     ISO_DIR="$2"; shift 2 ;;
        -s|--size)        SIZE_MB="$2"; shift 2 ;;
        -a|--attach)      ATTACH="$2"; shift 2 ;;
        -i|--interactive) INTERACTIVE=1; shift ;;
        -k|--keep)        KEEP=1; shift ;;
        --dry-run)        DRY_RUN=1; shift ;;
        -h|--help)        usage; exit 0 ;;
        *) echo "unknown option: $1" >&2; usage; exit 2 ;;
    esac
done

log()  { printf '[loaded-stick] %s\n' "$*" >&2; }
warn() { printf '[loaded-stick][WARN] %s\n' "$*" >&2; }

require() {
    command -v "$1" >/dev/null 2>&1 || {
        echo "missing required tool: $1" >&2
        exit 1
    }
}

require qemu-system-x86_64
require sgdisk
require losetup
require sudo

# --- ISO discovery ---------------------------------------------------------
[[ -d "$ISO_DIR" ]] || {
    echo "ISO directory not found: $ISO_DIR" >&2
    echo "create it and drop .iso files in, or pass -d <dir>" >&2
    exit 1
}

mapfile -t ISOS < <(find "$ISO_DIR" -maxdepth 1 -type f -name '*.iso' | sort)
if (( ${#ISOS[@]} == 0 )); then
    warn "no .iso files in $ISO_DIR — rescue-tui will boot an empty list"
    warn "this is still useful for proving the boot chain"
fi
log "found ${#ISOS[@]} ISO file(s) in $ISO_DIR"
for iso in "${ISOS[@]}"; do
    log "  - $(basename "$iso") ($(stat -c '%s' "$iso") bytes)"
done

# --- Sizing ----------------------------------------------------------------
if [[ -z "$SIZE_MB" ]]; then
    total_iso_bytes=0
    for iso in "${ISOS[@]}"; do
        total_iso_bytes=$(( total_iso_bytes + $(stat -c '%s' "$iso") ))
    done
    # 1.5x ISO bytes for headroom + ESP overhead, min 2048 MB.
    needed_mb=$(( total_iso_bytes * 3 / 2 / 1024 / 1024 + 600 ))
    SIZE_MB=$(( needed_mb > 2048 ? needed_mb : 2048 ))
fi
log "stick size: ${SIZE_MB} MiB"

if (( DRY_RUN )); then
    log "DRY RUN — would build $SIZE_MB MiB image, copy ${#ISOS[@]} ISO(s),"
    log "         then boot under QEMU+OVMF SecBoot. Exiting before mkusb."
    exit 0
fi

# --- Build the base image via mkusb.sh -------------------------------------
log "building base image with scripts/mkusb.sh (sudo may prompt)"
DISK_SIZE_MB="$SIZE_MB" "$ROOT_DIR/scripts/mkusb.sh"
IMG="$ROOT_DIR/out/aegis-boot.img"
[[ -f "$IMG" ]] || { echo "mkusb.sh did not produce $IMG" >&2; exit 1; }

# --- Copy ISOs onto the AEGIS_ISOS partition -------------------------------
if (( ${#ISOS[@]} > 0 )); then
    log "loop-mounting AEGIS_ISOS partition to copy ISOs"
    LOOP=$(sudo losetup --find --show --partscan "$IMG")
    cleanup_loop() {
        sudo umount "$MNT" 2>/dev/null || true
        sudo losetup -d "$LOOP" 2>/dev/null || true
        rmdir "$MNT" 2>/dev/null || true
    }
    trap cleanup_loop EXIT
    MNT=$(mktemp -d --tmpdir aegis-loaded-stick-mnt-XXXXXX)

    # Partition 2 is AEGIS_ISOS. losetup --partscan exposes it as ${LOOP}p2.
    DATA_PART="${LOOP}p2"
    [[ -b "$DATA_PART" ]] || {
        echo "expected partition device $DATA_PART not present after partscan" >&2
        exit 1
    }
    sudo mount "$DATA_PART" "$MNT"
    log "mounted $DATA_PART at $MNT"
    for iso in "${ISOS[@]}"; do
        log "  copying $(basename "$iso") ..."
        sudo cp "$iso" "$MNT/"
        # Copy sibling sha256/minisig if present so verification UI works.
        for sidecar in "${iso}.sha256" "${iso}.SHA256SUMS" "${iso}.minisig"; do
            [[ -f "$sidecar" ]] && sudo cp "$sidecar" "$MNT/"
        done
    done
    sudo sync
    cleanup_loop
    trap - EXIT
    log "AEGIS_ISOS load complete"
fi

# --- Build the QEMU command -----------------------------------------------
OVMF_CODE="${OVMF_CODE:-/usr/share/OVMF/OVMF_CODE_4M.secboot.fd}"
OVMF_VARS_SRC="${OVMF_VARS_SRC:-/usr/share/OVMF/OVMF_VARS_4M.ms.fd}"
[[ -r "$OVMF_CODE" ]] || { echo "missing $OVMF_CODE" >&2; exit 1; }
[[ -r "$OVMF_VARS_SRC" ]] || { echo "missing $OVMF_VARS_SRC" >&2; exit 1; }

WORK=$(mktemp -d --tmpdir aegis-loaded-stick-XXXXXX)
final_cleanup() {
    rm -rf -- "$WORK"
    if (( KEEP == 0 )) && [[ -f "$IMG" ]]; then
        log "removing $IMG (pass -k to keep)"
        rm -f "$IMG"
    fi
}
trap final_cleanup EXIT
cp "$OVMF_VARS_SRC" "$WORK/vars.fd"
chmod 0644 "$WORK/vars.fd"

display_args=(-nographic -serial mon:stdio)
(( INTERACTIVE )) && display_args=(-display gtk -serial stdio)

case "$ATTACH" in
    virtio)
        attach_args=(
            -drive "if=none,id=stick,format=raw,file=$IMG"
            -device "virtio-blk-pci,drive=stick"
        )
        ;;
    sata|ahci)
        attach_args=(
            -device ahci,id=ahci
            -drive "if=none,id=stick,format=raw,file=$IMG"
            -device "ide-hd,bus=ahci.0,drive=stick"
        )
        ;;
    usb)
        attach_args=(
            -device qemu-xhci,id=xhci
            -drive "if=none,id=stick,format=raw,file=$IMG"
            -device "usb-storage,bus=xhci.0,drive=stick"
        )
        ;;
    *)
        echo "unknown --attach mode: $ATTACH (expected virtio|sata|usb)" >&2
        exit 2
        ;;
esac
log "attach mode: $ATTACH"

QEMU_ARGS=(
    qemu-system-x86_64
    "${display_args[@]}"
    -machine q35,smm=on
    -global driver=cfi.pflash01,property=secure,value=on
    -m 2048M
    -drive "if=pflash,format=raw,unit=0,file=$OVMF_CODE,readonly=on"
    -drive "if=pflash,format=raw,unit=1,file=$WORK/vars.fd"
    "${attach_args[@]}"
    -boot order=c
)

log "booting loaded stick (${#ISOS[@]} ISO(s)) — Ctrl-A X to exit"
exec "${QEMU_ARGS[@]}"
