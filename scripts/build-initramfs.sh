#!/usr/bin/env bash
# Build a reproducible initramfs.cpio.gz that wraps rescue-tui.
#
# The resulting archive is designed to be appended (or concatenated) to a
# signed distro rescue kernel's own initramfs so that, once the kernel
# unpacks it, /usr/bin/rescue-tui runs as the boot-time UI.
#
# Reproducibility is achieved by:
#   - Sorted cpio input (stable file order)
#   - `cpio -o -H newc` (fixed on-disk layout; no timestamps baked into
#     the traversal itself beyond file mtimes)
#   - `find ... -exec touch -d @$SOURCE_DATE_EPOCH` before archiving
#     (flatten every mtime to the same deterministic value)
#   - `gzip -n --no-name` (strip filename + mtime from the gzip header)
#
# See: ADR 0001, issue #14, BUILDING.md.

set -euo pipefail

SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-1700000000}"
export SOURCE_DATE_EPOCH

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="${OUT_DIR:-$ROOT_DIR/out}"
STAGE_DIR="${STAGE_DIR:-$(mktemp -d -t aegis-initramfs-XXXXXX)}"
RESCUE_TUI_BIN="${RESCUE_TUI_BIN:-$ROOT_DIR/target/release/rescue-tui}"

cleanup() { rm -rf -- "$STAGE_DIR"; }
trap cleanup EXIT

log() { printf '[initramfs] %s\n' "$*" >&2; }

require() {
    command -v "$1" >/dev/null 2>&1 || {
        echo "missing required tool: $1" >&2
        exit 1
    }
}

require cpio
require gzip
require find
require sort
require install
require ldd
require sha256sum

if [[ ! -x "$RESCUE_TUI_BIN" ]]; then
    echo "rescue-tui binary not found or not executable: $RESCUE_TUI_BIN" >&2
    echo "build it first: cargo build --release -p rescue-tui" >&2
    exit 1
fi

mkdir -p "$OUT_DIR"

log "staging rootfs layout in $STAGE_DIR"
# POSIX-minimal directory skeleton.
install -d -m 0755 \
    "$STAGE_DIR/bin" \
    "$STAGE_DIR/sbin" \
    "$STAGE_DIR/usr/bin" \
    "$STAGE_DIR/usr/sbin" \
    "$STAGE_DIR/usr/lib" \
    "$STAGE_DIR/lib" \
    "$STAGE_DIR/lib64" \
    "$STAGE_DIR/etc" \
    "$STAGE_DIR/proc" \
    "$STAGE_DIR/sys" \
    "$STAGE_DIR/dev" \
    "$STAGE_DIR/run" \
    "$STAGE_DIR/tmp" \
    "$STAGE_DIR/mnt" \
    "$STAGE_DIR/run/media"

# --- rescue-tui --------------------------------------------------------------
install -m 0755 "$RESCUE_TUI_BIN" "$STAGE_DIR/usr/bin/rescue-tui"

# --- busybox (single static binary provides everything we need) --------------
BUSYBOX_PATH="$(command -v busybox || true)"
if [[ -z "$BUSYBOX_PATH" ]]; then
    echo "busybox not found on PATH; install busybox-static or busybox" >&2
    exit 1
fi
install -m 0755 "$BUSYBOX_PATH" "$STAGE_DIR/bin/busybox"
# Applets. Covered: mount, umount, mkdir, ls, sh, cat, mdev.
# rescue-tui doesn't call these directly — they exist for the init script
# below and for emergency shell fallback.
for applet in sh mount umount mkdir ls cat dmesg switch_root losetup \
              mdev blkid lsblk modprobe sleep echo ln readlink rmdir; do
    ln -sf /bin/busybox "$STAGE_DIR/bin/$applet"
done

# --- shared library deps of rescue-tui --------------------------------------
# busybox is typically static; rescue-tui links libc + libgcc_s + libm + libpthread.
log "copying shared library dependencies"
copy_libs() {
    local bin="$1"
    # `ldd` output: parse lines like "libc.so.6 => /lib/x86_64-linux-gnu/libc.so.6 (0x...)"
    # and plain "/lib64/ld-linux-x86-64.so.2 (0x...)" for the dynamic linker.
    # Mode 0755 because the dynamic linker is itself an ELF interpreter that
    # the kernel execve's — without the exec bit, every dynamically-linked
    # binary in the initramfs fails with "Permission denied".
    ldd "$bin" 2>/dev/null | awk '
        /=>/ { if ($3 ~ /^\//) print $3 }
        /^\t\// { print $1 }
    ' | sort -u | while IFS= read -r lib; do
        [[ -f "$lib" ]] || continue
        # Follow symlinks so we put the real file at the expected path; this
        # flattens /lib64/ld-linux-* -> /lib/x86_64-linux-gnu/ld-linux-* style
        # distro layouts into a self-contained initramfs.
        local resolved
        resolved="$(readlink -f "$lib")"
        install -D -m 0755 "$resolved" "$STAGE_DIR$lib"
    done
}
copy_libs "$STAGE_DIR/usr/bin/rescue-tui"
# If distro busybox is dynamically linked, ldd would error; ignore silently.
copy_libs "$STAGE_DIR/bin/busybox" 2>/dev/null || true

# --- PID 1 init script -------------------------------------------------------
cat > "$STAGE_DIR/init" <<'INIT_SH'
#!/bin/sh
# aegis-boot PID 1 — minimal init that sets up the rescue environment and
# hands control to /usr/bin/rescue-tui.

set -e

/bin/mount -t proc  proc  /proc
/bin/mount -t sysfs sys   /sys
/bin/mount -t devtmpfs dev /dev 2>/dev/null || /bin/mount -t tmpfs tmpfs /dev
/bin/mount -t tmpfs  run   /run

# Give the kernel a moment to enumerate USB/NVMe devices before we look.
/bin/sleep 1

# Auto-mount every block device that looks like it has a filesystem. The TUI
# will walk /run/media/* looking for .iso files.
for dev in /dev/sd* /dev/nvme*n*p* /dev/vd* /dev/mmcblk*p*; do
    [ -b "$dev" ] || continue
    name=$(/usr/bin/basename "$dev" 2>/dev/null || echo "$dev" | /bin/sed 's|.*/||')
    mp="/run/media/$name"
    /bin/mkdir -p "$mp"
    /bin/mount -o ro "$dev" "$mp" 2>/dev/null || /bin/rmdir "$mp"
done

export AEGIS_ISO_ROOTS=/run/media
export PATH=/usr/bin:/usr/sbin:/bin:/sbin
export TERM=linux

# Hand off. On clean quit, drop to a shell so the user isn't staring at
# a kernel panic.
/usr/bin/rescue-tui || {
    /bin/echo "rescue-tui exited; dropping to emergency shell"
    exec /bin/sh
}

# rescue-tui returned without error (unusual — kexec would have replaced us).
exec /bin/sh
INIT_SH
chmod 0755 "$STAGE_DIR/init"

# --- deterministic mtime flattening -----------------------------------------
log "flattening mtimes to SOURCE_DATE_EPOCH=$SOURCE_DATE_EPOCH"
find "$STAGE_DIR" -exec touch -h -d "@$SOURCE_DATE_EPOCH" {} +

# --- cpio + gzip assembly ---------------------------------------------------
OUT_CPIO="$OUT_DIR/initramfs.cpio"
OUT_GZ="$OUT_DIR/initramfs.cpio.gz"

log "creating cpio archive (newc, sorted)"
( cd "$STAGE_DIR" && find . -mindepth 1 -print0 | LC_ALL=C sort -z \
    | cpio --null --create --format=newc --quiet --reproducible \
  ) > "$OUT_CPIO"

log "compressing with deterministic gzip"
gzip --no-name --best --stdout "$OUT_CPIO" > "$OUT_GZ"
rm -f "$OUT_CPIO"

( cd "$OUT_DIR" && sha256sum initramfs.cpio.gz > initramfs.cpio.gz.sha256 )

size=$(stat -c '%s' "$OUT_GZ")
hash=$(awk '{print $1}' "$OUT_DIR/initramfs.cpio.gz.sha256")
log "wrote $OUT_GZ ($size bytes)"
log "sha256: $hash"

if [[ "$size" -gt 20971520 ]]; then
    echo "initramfs exceeds 20 MB size budget ($size bytes); investigate" >&2
    exit 1
fi
