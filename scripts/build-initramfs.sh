#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
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

# --- tpm2_pcrextend (optional — PCR attestation before kexec) ----------------
# If present on the build host, ship it so rescue-tui's TPM measurement path
# can extend PCR 12 before handoff. Without this, the measurement is skipped
# with a logged warning — fine for TPM-less hardware but removes the
# attestation story.
TPM2_PCREXTEND="$(command -v tpm2_pcrextend || true)"
if [[ -n "$TPM2_PCREXTEND" && -f "$TPM2_PCREXTEND" ]]; then
    install -m 0755 "$TPM2_PCREXTEND" "$STAGE_DIR/usr/bin/tpm2_pcrextend"
    log "shipping tpm2_pcrextend for TPM PCR attestation"
else
    log "tpm2_pcrextend not on PATH — TPM measurement will be skipped at runtime"
fi

# --- util-linux losetup (proper loop-device handling) ------------------------
# Busybox's losetup applet doesn't accept `--show` and its behavior for
# loop-device allocation on modern kernels (loop-control, on-demand node
# creation) is inconsistent. Ship util-linux's real losetup if available;
# iso-parser prefers it automatically when present.
UTIL_LOSETUP="$(command -v losetup || true)"
if [[ -n "$UTIL_LOSETUP" && -f "$UTIL_LOSETUP" ]]; then
    # Find the actual binary, not a busybox symlink.
    resolved=$(readlink -f "$UTIL_LOSETUP")
    if ! [[ "$resolved" =~ busybox ]]; then
        install -m 0755 "$resolved" "$STAGE_DIR/sbin/losetup.util-linux"
        copy_libs_placeholder="$STAGE_DIR/sbin/losetup.util-linux"
    fi
fi

# --- Kernel modules (isofs, loop, udf) ---------------------------------------
# Modern Ubuntu distro kernels ship iso9660 support as a MODULE, not
# built-in. Without loading it, `mount -t iso9660 /dev/loop0 /mnt` fails
# even though the loop device exists. Ship the module tree so /init can
# modprobe isofs before attempting ISO mounts.
#
# If AEGIS_KMOD_SRC is set, copy modules from there. Otherwise, copy from
# the currently-running kernel's /lib/modules/$(uname -r)/. When the
# target kernel in the deployment doesn't match the build host's kernel,
# operators must override AEGIS_KMOD_SRC — we warn loudly.
KMOD_SRC="${AEGIS_KMOD_SRC:-}"
if [[ -z "$KMOD_SRC" ]]; then
    # Prefer a kernel whose version matches /boot/vmlinuz-* — that's the
    # kernel the operator actually installed for deployment/testing,
    # not the build host's running kernel. This matters on CI runners
    # where the host kernel (e.g. azure) differs from the installed
    # -generic kernel.
    for vmlinuz in /boot/vmlinuz-*; do
        [[ -e "$vmlinuz" && ! -L "$vmlinuz" ]] || continue
        ver=$(basename "$vmlinuz" | sed 's/^vmlinuz-//')
        candidate="/lib/modules/$ver"
        if [[ -d "$candidate/kernel/fs" ]]; then
            KMOD_SRC="$candidate"
            break
        fi
    done
fi
# Fallback: the running kernel's modules (may be wrong if deployment
# uses a different kernel).
if [[ -z "$KMOD_SRC" ]]; then
    for candidate in /lib/modules/*/kernel/fs; do
        [[ -d "$candidate" ]] || continue
        KMOD_SRC="${candidate%/kernel/fs}"
    done
fi
if [[ -n "$KMOD_SRC" && -d "$KMOD_SRC" ]]; then
    KVER=$(basename "$KMOD_SRC")
    log "shipping kernel modules from $KMOD_SRC (kernel $KVER)"
    MOD_DEST="$STAGE_DIR/lib/modules/$KVER"
    install -d "$MOD_DEST/kernel/fs/isofs"
    install -d "$MOD_DEST/kernel/fs/udf"
    install -d "$MOD_DEST/kernel/drivers/block"
    # Each module may be .ko or .ko.zst depending on compression. Ship
    # whatever the source kernel has.
    # Each module may be .ko, .ko.zst, .ko.xz, or .ko.gz depending on the
    # kernel's CONFIG_MODULE_COMPRESS_* setting. Busybox's modprobe applet
    # handles .ko.gz natively but NOT .ko.zst — Ubuntu's stock kernel
    # compiles as zstd. Decompress on the fly at build time so the shipped
    # module is always plain .ko (works with every known module loader).
    copy_module() {
        local rel_path="$1" dest_dir="$2"
        local src_dir="$KMOD_SRC/$(dirname "$rel_path" | sed 's|^\./||')"
        local base
        base="$(basename "$rel_path")"
        for ext in ko ko.zst ko.xz ko.gz; do
            local src="$src_dir/$base.$ext"
            [[ -f "$src" ]] || continue
            local dest="$dest_dir/$base.ko"
            mkdir -p "$(dirname "$dest")"
            case "$ext" in
                ko)     install -m 0644 "$src" "$dest" ;;
                ko.zst) zstd -d -q -c "$src" > "$dest" && chmod 0644 "$dest" ;;
                ko.xz)  xz -d -c "$src" > "$dest" && chmod 0644 "$dest" ;;
                ko.gz)  gzip -d -c "$src" > "$dest" && chmod 0644 "$dest" ;;
            esac
            return 0
        done
        return 1
    }
    # Distinguish "shipped as a module but we couldn't find it" (real
    # warning) from "compiled into the kernel image" (no .ko exists, no
    # action needed). Reads CONFIG_* from /boot/config-$KVER. Kernels
    # 6.14+ ship loop as built-in (CONFIG_BLK_DEV_LOOP=y), so the
    # previous "WARNING: loop module not found" was a false alarm. (#69)
    KCONFIG="/boot/config-$KVER"
    is_builtin() {
        [[ -r "$KCONFIG" ]] && grep -q "^${1}=y$" "$KCONFIG"
    }
    try_module() {
        local rel="$1" dest="$2" name="$3" kconfig_key="$4"
        if copy_module "$rel" "$dest"; then
            return 0
        fi
        if is_builtin "$kconfig_key"; then
            log "$name is built-in to kernel $KVER (no module to ship)"
        else
            log "WARNING: $name module not found (CONFIG_$kconfig_key not set?)"
        fi
    }
    try_module "kernel/fs/isofs/isofs" "$MOD_DEST/kernel/fs/isofs" \
        "isofs" "CONFIG_ISO9660_FS"
    try_module "kernel/fs/udf/udf" "$MOD_DEST/kernel/fs/udf" \
        "udf" "CONFIG_UDF_FS"
    # exfat (Linux 5.7+, mainlined in fs/exfat). Now the default for
    # AEGIS_ISOS as of #243; without this module the rescue-tui's
    # exfat-mount fallback in the AEGIS_ISOS auto-mount loop would
    # silently fail on every device manufactured after that change.
    try_module "kernel/fs/exfat/exfat" "$MOD_DEST/kernel/fs/exfat" \
        "exfat" "CONFIG_EXFAT_FS"
    try_module "kernel/drivers/block/loop" "$MOD_DEST/kernel/drivers/block" \
        "loop" "CONFIG_BLK_DEV_LOOP"

    # --- storage controller modules (#72) ------------------------------
    # Without these, /dev/sd* / /dev/nvme* never appear on real hardware
    # because Ubuntu generic kernels compile most storage drivers as
    # modules. Modules are copied BY RELATIVE PATH so copy_module's
    # src_dir resolution works regardless of where the module actually
    # lives under /lib/modules/<ver>. Each call is best-effort — any
    # missing module logs a warning but doesn't fail the build.

    # SCSI core — prerequisite for sd_mod and usb-storage.
    try_module "kernel/drivers/scsi/scsi_mod" \
        "$MOD_DEST/kernel/drivers/scsi" \
        "scsi_mod" "CONFIG_SCSI"
    try_module "kernel/drivers/scsi/sd_mod" \
        "$MOD_DEST/kernel/drivers/scsi" \
        "sd_mod" "CONFIG_BLK_DEV_SD"

    # SATA / AHCI — most modern desktops and laptops.
    try_module "kernel/drivers/ata/libata" \
        "$MOD_DEST/kernel/drivers/ata" \
        "libata" "CONFIG_ATA"
    try_module "kernel/drivers/ata/libahci" \
        "$MOD_DEST/kernel/drivers/ata" \
        "libahci" "CONFIG_SATA_AHCI"
    try_module "kernel/drivers/ata/ahci" \
        "$MOD_DEST/kernel/drivers/ata" \
        "ahci" "CONFIG_SATA_AHCI"

    # NVMe — modern laptops and workstations.
    try_module "kernel/drivers/nvme/host/nvme-core" \
        "$MOD_DEST/kernel/drivers/nvme/host" \
        "nvme-core" "CONFIG_NVME_CORE"
    try_module "kernel/drivers/nvme/host/nvme" \
        "$MOD_DEST/kernel/drivers/nvme/host" \
        "nvme" "CONFIG_BLK_DEV_NVME"

    # USB core + host controllers — THE deployment path (USB stick).
    try_module "kernel/drivers/usb/core/usbcore" \
        "$MOD_DEST/kernel/drivers/usb/core" \
        "usbcore" "CONFIG_USB"
    try_module "kernel/drivers/usb/common/usb-common" \
        "$MOD_DEST/kernel/drivers/usb/common" \
        "usb-common" "CONFIG_USB_COMMON"
    try_module "kernel/drivers/usb/host/xhci-hcd" \
        "$MOD_DEST/kernel/drivers/usb/host" \
        "xhci-hcd" "CONFIG_USB_XHCI_HCD"
    try_module "kernel/drivers/usb/host/xhci-pci" \
        "$MOD_DEST/kernel/drivers/usb/host" \
        "xhci-pci" "CONFIG_USB_XHCI_PCI"
    try_module "kernel/drivers/usb/host/ehci-hcd" \
        "$MOD_DEST/kernel/drivers/usb/host" \
        "ehci-hcd" "CONFIG_USB_EHCI_HCD"
    try_module "kernel/drivers/usb/host/ehci-pci" \
        "$MOD_DEST/kernel/drivers/usb/host" \
        "ehci-pci" "CONFIG_USB_EHCI_PCI"

    # USB mass storage — both classic (usb-storage) and UAS (USB 3.x).
    try_module "kernel/drivers/usb/storage/usb-storage" \
        "$MOD_DEST/kernel/drivers/usb/storage" \
        "usb-storage" "CONFIG_USB_STORAGE"
    try_module "kernel/drivers/usb/storage/uas" \
        "$MOD_DEST/kernel/drivers/usb/storage" \
        "uas" "CONFIG_USB_UAS"

    # --- vfat NLS fallback (#68, #109) ----------------------------------
    # CONFIG_NLS_DEFAULT="utf8" on Ubuntu but NLS_UTF8 is a module, and
    # the kernel's vfat default `iocharset=iso8859-1` needs the
    # `nls_iso8859-1` module which Ubuntu also keeps loadable. We ship
    # `nls_utf8` and /init mounts vfat with `iocharset=utf8` explicitly.
    # (Earlier comments referenced cp437 as an iocharset — wrong; cp437
    # is a codepage. iocharset must be one of utf8/iso8859-*/koi8/etc.)
    try_module "kernel/fs/nls/nls_utf8" \
        "$MOD_DEST/kernel/fs/nls" \
        "nls_utf8" "CONFIG_NLS_UTF8"
    # Also ship nls_cp437 + nls_iso8859-1 so the kernel's hot-plug
    # automount path doesn't log "IO charset iso8859-1 not found"
    # on real hardware. Our /init uses iocharset=utf8 explicitly,
    # but udev hot-plug mounts that fire before /init gets to the
    # vfat partition are the source of the dmesg noise.
    try_module "kernel/fs/nls/nls_cp437" \
        "$MOD_DEST/kernel/fs/nls" \
        "nls_cp437" "CONFIG_NLS_CODEPAGE_437"
    try_module "kernel/fs/nls/nls_iso8859-1" \
        "$MOD_DEST/kernel/fs/nls" \
        "nls_iso8859-1" "CONFIG_NLS_ISO8859_1"
    # Regenerate modules.dep so it references our decompressed .ko paths
    # (source kernel's modules.dep points at .ko.zst). depmod -b rebuilds
    # into the staged tree; no runtime kernel match needed.
    #
    # Fail hard on depmod failure (#138): a silent warning here was
    # producing silent boot-time failures (storage modules missing at
    # the busybox modprobe call in /init because modules.dep still
    # points at the original .ko.zst paths). If depmod is genuinely
    # missing on the build host, the operator should know before
    # producing an image they'll then have to debug under OVMF. Set
    # AEGIS_ALLOW_MISSING_DEPMOD=1 to bypass.
    if command -v depmod >/dev/null 2>&1; then
        depmod_stderr=$(depmod -b "$STAGE_DIR" "$KVER" 2>&1 >/dev/null) || {
            log "FATAL: depmod -b '$STAGE_DIR' '$KVER' failed"
            log "  stderr: $depmod_stderr"
            log "  busybox modprobe would miss dependencies at boot time; aborting."
            log "  (set AEGIS_ALLOW_MISSING_DEPMOD=1 to bypass — not recommended)"
            [ -n "${AEGIS_ALLOW_MISSING_DEPMOD:-}" ] || exit 1
            log "WARNING: proceeding despite depmod failure — AEGIS_ALLOW_MISSING_DEPMOD set."
        }
    else
        log "FATAL: depmod not on PATH; cannot regenerate modules.dep for staged modules."
        log "  install kmod (e.g. 'apt-get install kmod' / 'dnf install kmod') and retry."
        log "  (set AEGIS_ALLOW_MISSING_DEPMOD=1 to bypass — not recommended)"
        [ -n "${AEGIS_ALLOW_MISSING_DEPMOD:-}" ] || exit 1
        log "WARNING: proceeding without depmod — AEGIS_ALLOW_MISSING_DEPMOD set."
    fi
else
    log "WARNING: no kernel modules source found; iso9660 mounts will fail"
    log "  set AEGIS_KMOD_SRC=/lib/modules/<kver> if your target kernel needs modules"
fi
# Applets. Covered: mount, umount, mkdir, ls, sh, cat, mdev.
# rescue-tui doesn't call these directly — they exist for the init script
# below and for emergency shell fallback.
for applet in sh mount umount mkdir ls cat dmesg switch_root losetup \
              mdev blkid lsblk modprobe sleep echo ln readlink rmdir \
              findfs uname grep sed cp rm tee date; do
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
# util-linux losetup is dynamically linked.
if [[ -f "$STAGE_DIR/sbin/losetup.util-linux" ]]; then
    copy_libs "$STAGE_DIR/sbin/losetup.util-linux"
fi
# tpm2_pcrextend pulls in a bunch of libtss2 — copy them all.
if [[ -f "$STAGE_DIR/usr/bin/tpm2_pcrextend" ]]; then
    copy_libs "$STAGE_DIR/usr/bin/tpm2_pcrextend"
fi

# --- PID 1 init script -------------------------------------------------------
cat > "$STAGE_DIR/init" <<'INIT_SH'
#!/bin/sh
# aegis-boot PID 1 — minimal init that sets up the rescue environment and
# hands control to /usr/bin/rescue-tui.
#
# Deliberately does NOT use `set -e`. Rescue-environment commands routinely
# return non-zero (mount failures on absent filesystems, missing optional
# devices, etc.); aborting PID 1 on any of them triggers a kernel panic and
# reboot loop. Each command handles its own errors explicitly. (#68)

/bin/echo "init: aegis-boot /init starting (PID 1)"

/bin/mount -t proc  proc  /proc
/bin/mount -t sysfs sys   /sys
if /bin/mount -t devtmpfs dev /dev; then
    /bin/echo "init: mounted devtmpfs at /dev"
else
    /bin/echo "init: WARNING devtmpfs mount failed — falling back to tmpfs (no devices visible)"
    /bin/mount -t tmpfs tmpfs /dev
fi
/bin/mount -t tmpfs  run   /run

# Enable kernel SysRq for emergency escape hatches that rescue-tui's
# Help overlay advertises (Alt+SysRq+b reboot, +s sync, +e SIGTERM).
# Without this, those keybind cheatsheets lie — kernel.sysrq=0 is the
# common distro default. Write 1 (all SysRq functions enabled) since
# this is a rescue environment an operator explicitly booted. (#93)
if /bin/echo 1 > /proc/sys/kernel/sysrq 2>/dev/null; then
    /bin/echo "init: SysRq enabled (kernel.sysrq=1) — operator escape hatches active"
else
    /bin/echo "init: WARNING could not enable SysRq (kernel built without CONFIG_MAGIC_SYSRQ?)"
fi

# #109 shakedown: every /bin/echo "init: ..." below is ALSO captured
# to /run/aegis-init.log via a simple helper. After AEGIS_ISOS
# mounts, the file is copied onto the data partition so the
# diagnostics survive a reboot.
INIT_LOG=/run/aegis-init.log
: > "$INIT_LOG" 2>/dev/null

# Load storage controller modules so /dev/sd* / /dev/nvme* appear on
# real hardware. Order matters: bus cores before hosts before class
# drivers. Ignore failures (modules may be built-in on some kernels
# — modprobe logs a no-op and returns 0, or errors out if truly
# absent which is fine). (#72)
/bin/echo "init: loading storage modules"
# filesystem modules: isofs for mounted ISOs, udf for DVD-style
# isos, exfat for the AEGIS_ISOS data partition (default since
# #243), nls_* for FAT character-set translation tables (the ESP is
# vfat; without nls_iso8859-1 + nls_cp437 the kernel hot-plug
# automount path logs "IO charset iso8859-1 not found").
#
# Real-hardware validation of #132 caught the exfat omission: the
# module shipped in the initramfs but was never modprobed, so
# `mount -t exfat /dev/sda2` returned "No such device" and
# rescue-tui discovered 0 ISOs on a fresh direct-install stick.
for m in scsi_mod sd_mod \
         libata libahci ahci \
         nvme-core nvme \
         usbcore usb-common xhci-hcd xhci-pci ehci-hcd ehci-pci \
         usb-storage uas \
         nls_utf8 nls_cp437 nls_iso8859-1 \
         loop isofs udf exfat; do
    /bin/modprobe "$m" 2>/dev/null || true
done

# Give the kernel a moment to enumerate USB/NVMe devices before we look.
# USB bus probe can take a second or two on real hardware (hub reset
# sequence, UAS enumeration). 3s is conservative.
/bin/sleep 3
/bin/echo "init: kernel cmdline: $(/bin/cat /proc/cmdline 2>/dev/null || echo '?')"
/bin/echo "init: mounts active:"
/bin/cat /proc/mounts 2>/dev/null | /bin/sed 's/^/init:   /' || /bin/echo "init:   (cat /proc/mounts failed)"

# Prefer the stick's dedicated AEGIS_ISOS data partition if present.
# Resolve LABEL=AEGIS_ISOS via three fallback strategies because busybox's
# findfs does not always recognize FAT32 labels (#68 — observed silently
# returning empty on Ubuntu busybox 1.30 against a FAT32 partition with
# label AEGIS_ISOS, leading to "0 ISOs discovered" on otherwise-loaded
# sticks):
#   1. /bin/findfs LABEL=...           (works for ext*, sometimes FAT)
#   2. /bin/blkid -L AEGIS_ISOS        (label cache, broader fs support)
#   3. /dev/disk/by-label/AEGIS_ISOS   (udev/devtmpfs symlink, most reliable)
/bin/mkdir -p /run/media/aegis-isos
AEGIS_DEV=""
for resolver in \
    "/bin/findfs LABEL=AEGIS_ISOS" \
    "/bin/blkid -L AEGIS_ISOS" \
    "/bin/readlink -f /dev/disk/by-label/AEGIS_ISOS"; do
    candidate=$($resolver 2>/dev/null || true)
    if [ -n "$candidate" ] && [ -b "$candidate" ]; then
        AEGIS_DEV="$candidate"
        break
    fi
done
if [ -n "$AEGIS_DEV" ]; then
    # busybox mount type-autodetect is unreliable; explicit types in
    # fallback order. vfat needs `codepage=437,iocharset=utf8` because
    # the default `iocharset=iso8859-1` is a module (`nls_iso8859-1`)
    # we don't ship — without overriding it the mount fails with
    # "FAT-fs: IO charset iso8859-1 not found". `iocharset=cp437` is
    # NOT a valid value (cp437 is only a codepage / CCS); we ship
    # `nls_utf8` and use that instead. ext4 is the right pick for
    # >4 GiB ISOs and needs no nls. (#68, #109)
    # rw so /init can write aegis-boot-<ts>.log and rescue-tui can
    # tee F10 save-log evidence to the partition. ISO bytes
    # themselves are never modified — iso-probe opens .iso files
    # read-only via loop-mount.
    mount_ok=0
    # Try exfat first since it's the default for AEGIS_ISOS as of #243.
    # ext4 second (the Linux-only DATA_FS=ext4 opt-in path). vfat last
    # — legacy DATA_FS=fat32 sticks; the explicit codepage/iocharset
    # variants are kept because the kernel default `iocharset=iso8859-1`
    # is a module we don't ship (#68, #109).
    for spec in \
        "exfat:rw" \
        "ext4:rw" \
        "vfat:rw,codepage=437,iocharset=utf8" \
        "vfat:rw"; do
        fstype="${spec%%:*}"
        opts="${spec#*:}"
        mount_err=$(/bin/mount -t "$fstype" -o "$opts" "$AEGIS_DEV" /run/media/aegis-isos 2>&1)
        if [ -z "$mount_err" ]; then
            /bin/echo "init: mounted $AEGIS_DEV (LABEL=AEGIS_ISOS, fs=$fstype, rw) -> /run/media/aegis-isos"
            mount_ok=1
            break
        fi
        /bin/echo "init:   tried fs=$fstype: $mount_err"
    done
    [ "$mount_ok" = 0 ] && /bin/echo "init: WARNING: found $AEGIS_DEV but all mount attempts failed (see above)"
else
    /bin/echo "init: AEGIS_ISOS label not resolved by findfs/blkid/by-label — secondary auto-mount loop will try /dev/sd*"
fi

# Diagnostic — dump what we see in /dev so we can debug "0 ISOs found"
# on real hardware. The output goes to the serial console BEFORE
# rescue-tui takes the alternate screen, so it's grep-able from a
# QEMU run log. (#68)
/bin/echo "init: block devices visible:"
for dev in /dev/sd* /dev/nvme* /dev/vd* /dev/mmcblk* /dev/disk/by-label/*; do
    [ -e "$dev" ] && /bin/echo "init:   $dev"
done
/bin/echo "init: end of block-device listing"

# Also auto-mount any other block device that looks like it has a
# filesystem. Covers the case where the user attaches an ISO on a
# secondary stick or USB drive alongside the boot media.
# (#113) Iterate PARTITIONS, not whole disks — /dev/sda doesn't have
# a filesystem and mount attempts print noisy "Can't open blockdev"
# errors. The name pattern requires a trailing digit (partition
# suffix): sd*[0-9] matches sda1/sdb2/... but not sda/sdb.
for dev in /dev/sd*[0-9] /dev/nvme*n*p* /dev/vd*[0-9] /dev/mmcblk*p*; do
    [ -b "$dev" ] || continue
    # Skip the AEGIS_ISOS partition we already mounted.
    [ "$dev" = "${AEGIS_DEV:-}" ] && continue
    name=$(echo "$dev" | /bin/sed 's|.*/||')
    mp="/run/media/$name"
    /bin/mkdir -p "$mp"
    # (#113, #109) Explicit vfat options when auto-mount without a
    # type fails — Linux vfat defaults to iocharset=iso8859-1 which
    # is a module (`nls_iso8859-1`) we don't ship. We ship `nls_utf8`
    # so iocharset=utf8 works. (cp437 is a codepage, not an iocharset
    # — using it as iocharset silently falls back to the default and
    # fails the same way.) Try auto first (ext4/ntfs/etc work fine),
    # fall back to explicit vfat on failure.
    if /bin/mount -o ro "$dev" "$mp" 2>/dev/null; then
        /bin/echo "init: secondary-mount $dev -> $mp"
    elif /bin/mount -t vfat -o ro,codepage=437,iocharset=utf8 \
            "$dev" "$mp" 2>/dev/null; then
        /bin/echo "init: secondary-mount $dev -> $mp (fs=vfat)"
    else
        /bin/rmdir "$mp" 2>/dev/null
    fi
done

export AEGIS_ISO_ROOTS=/run/media/aegis-isos:/run/media
# Prefer util-linux losetup over busybox applet — iso-parser's
# loop-mount path works reliably with real losetup semantics.
if [ -x /sbin/losetup.util-linux ]; then
    /bin/ln -sf /sbin/losetup.util-linux /usr/sbin/losetup
    export PATH=/usr/sbin:/usr/bin:/sbin:/bin
else
    export PATH=/usr/bin:/usr/sbin:/bin:/sbin
fi

# (loop / isofs / udf already modprobed in the early bulk load above.)

# #109 shakedown: snapshot diagnostics into /run/aegis-init.log
# before rescue-tui takes the alternate screen. Everything here is
# readable post-reboot (copied to AEGIS_ISOS just below) and
# post-TUI-exit (still on tmpfs when the shell drops).
{
    /bin/echo "=== /proc/cmdline ==="
    /bin/cat /proc/cmdline 2>/dev/null
    /bin/echo ""
    /bin/echo "=== /proc/mounts ==="
    /bin/cat /proc/mounts 2>/dev/null
    /bin/echo ""
    /bin/echo "=== /dev (block devices) ==="
    /bin/ls -la /dev/sd* /dev/nvme* /dev/vd* /dev/mmcblk* 2>/dev/null
    /bin/ls -la /dev/disk/by-label/ 2>/dev/null
    /bin/echo ""
    /bin/echo "=== /lib/modules ==="
    /bin/ls /lib/modules/ 2>/dev/null
    /bin/echo ""
    /bin/echo "=== dmesg tail ==="
    /bin/dmesg 2>/dev/null | /bin/tail -40 2>/dev/null || /bin/echo "(dmesg not accessible)"
} >> "$INIT_LOG" 2>&1

# Copy the snapshot to AEGIS_ISOS so it survives a reboot. Best-
# effort — cp fails silently if the partition isn't mounted or is
# read-only. /run/aegis-init.log stays on tmpfs regardless.
if [ -d /run/media/aegis-isos ]; then
    _ts=$(/bin/date +%Y%m%d-%H%M%S 2>/dev/null || /bin/echo "boot")
    if /bin/cp "$INIT_LOG" "/run/media/aegis-isos/aegis-boot-${_ts}.log" 2>/dev/null; then
        /bin/echo "init: wrote init log to AEGIS_ISOS/aegis-boot-${_ts}.log"
    fi
fi

# Verbose-pause: if the kernel cmdline has aegis.verbose=1, pause
# for 30s (or until Enter) so the operator can read the pre-TUI
# diagnostics before the alt-screen takes over. (#109)
if /bin/grep -q "aegis.verbose=1" /proc/cmdline 2>/dev/null; then
    /bin/echo ""
    /bin/echo "init: aegis.verbose=1 — pausing 30s before rescue-tui."
    /bin/echo "init: press Enter to continue sooner."
    /bin/echo ""
    /bin/echo "Full init log: $INIT_LOG"
    /bin/echo ""
    /bin/sleep 30 &
    _pid=$!
    read -r _line 2>/dev/null || true
    /bin/kill "$_pid" 2>/dev/null
fi

export TERM=linux

# Hand off. Exit code semantics (#90):
#   0        — operator chose Quit → drop to emergency shell (old default)
#   42       — operator chose the rescue-shell entry explicitly
#   anything — crash / unclean exit → emergency shell
# All paths land in /bin/sh; the different branches only differ in the
# banner so an operator reading the serial log can tell which happened.
/usr/bin/rescue-tui
rc=$?
case "$rc" in
    0)   /bin/echo "init: rescue-tui quit cleanly; dropping to emergency shell" ;;
    42)  /bin/echo "init: rescue shell requested by operator (#90)" ;;
    *)   /bin/echo "init: rescue-tui exited unexpectedly (rc=$rc); dropping to emergency shell" ;;
esac
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
