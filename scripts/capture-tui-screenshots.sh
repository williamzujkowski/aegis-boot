#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Automated PNG capture pipeline for the rescue-tui (#478, picks up from
# #477's ANSI-only `tui-screenshots` binary).
#
# Builds an aegis-boot stick image with mkusb.sh, optionally loads ISOs
# from `test-isos/` onto its AEGIS_ISOS partition, boots it under
# QEMU+OVMF SecureBoot with a VNC display + QMP control socket, waits
# for the rescue-tui ready banner on the serial console, then drives a
# scripted sequence of keystrokes via QMP `send-key` and captures each
# resulting screen via QMP `screendump`. Each PPM is converted to PNG
# via ImageMagick and saved under `docs/screenshots/`.
#
# Differences from `tui-screenshots` ANSI dumps:
#   - This is a REAL boot — exercises the full signed chain, not synthetic
#     fixture data. PNGs prove the on-VM render matches the intent.
#   - Coverage limited to scenarios reachable from arbitrary ISO inputs.
#     Synthetic-only fixtures (tier-4 parse-failed, tier-5 SecureBoot-
#     blocked Windows, tier-6 hash mismatch) stay covered by the ANSI
#     binary — this script focuses on the boot-chain-validated cases.
#
# Usage:
#   scripts/capture-tui-screenshots.sh                       # ./test-isos
#   scripts/capture-tui-screenshots.sh -d ~/Downloads/isos
#   scripts/capture-tui-screenshots.sh --keep                # keep VM artifacts
#   scripts/capture-tui-screenshots.sh --out docs/screenshots
#
# Prereqs: same as qemu-loaded-stick.sh (qemu-system-x86, ovmf, mkusb deps)
# PLUS:    imagemagick (for PPM→PNG), python3 (for QMP I/O).
#
# CI: this script is gated behind `workflow_dispatch` in
# .github/workflows/tui-screenshots.yml — runs on demand, not per-PR.
# A full capture takes ~3-5 minutes (image build + boot + 8 captures).

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ISO_DIR="$ROOT_DIR/test-isos"
OUT_DIR="$ROOT_DIR/docs/screenshots"
KEEP=0
SKIP_BUILD=0
SIZE_MB=""
VNC_PORT="9"   # VNC display :N → TCP port 5900+N
QMP_SOCK=""    # set in setup
SERIAL_LOG=""  # set in setup
WORK=""        # set in setup
IMG=""         # set in setup
QEMU_PID=""    # set when QEMU starts

usage() {
    cat <<USAGE
Usage: $0 [options]

  -d, --iso-dir DIR   directory of .iso files to load (default: ./test-isos)
  -o, --out DIR       output dir for PNGs (default: docs/screenshots)
  -s, --size MB       stick image size in MiB (default: auto)
  -k, --keep          keep the built image + working dir after exit
      --skip-build    reuse out/aegis-boot.img + out/initramfs.cpio.gz from a
                      prior --keep run; skips cargo build + mkusb. Useful for
                      iterating on scenario keys/timings without rebuilding
                      the 1-2 GiB image (saves ~2 minutes per iteration). (#626)
  -h, --help          this message

Captures these scenarios as PNGs:
  01-list-default            initial rescue-tui list view
  02-list-sort-cycled        after s — sort changes from name → size
  03-confirm                 after Enter on first ISO — Confirm kexec view
  04-help                    after ? — help overlay
  05-filter-empty            after / — filter input opened
  06-filter-typed            after typing 'ub' — filter narrowing
  07-second-iso-selected     after Down — second ISO highlighted
USAGE
}

while (( $# > 0 )); do
    case "$1" in
        -d|--iso-dir)  ISO_DIR="$2"; shift 2 ;;
        -o|--out)      OUT_DIR="$2"; shift 2 ;;
        -s|--size)     SIZE_MB="$2"; shift 2 ;;
        -k|--keep)     KEEP=1; shift ;;
        --skip-build)  SKIP_BUILD=1; shift ;;
        -h|--help)     usage; exit 0 ;;
        *) echo "unknown option: $1" >&2; usage; exit 2 ;;
    esac
done

log()  { printf '[capture-tui] %s\n' "$*" >&2; }
warn() { printf '[capture-tui][WARN] %s\n' "$*" >&2; }

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
require python3
require convert  # ImageMagick — produces PNG from QEMU's PPM screendump

# --- ISO discovery (informational; empty list also works) ------------
[[ -d "$ISO_DIR" ]] || mkdir -p "$ISO_DIR"
mapfile -t ISOS < <(find "$ISO_DIR" -maxdepth 1 -type f -name '*.iso' | sort)
log "found ${#ISOS[@]} ISO file(s) in $ISO_DIR"
for iso in "${ISOS[@]}"; do
    log "  - $(basename "$iso")"
done

# --- Sizing (mirrors qemu-loaded-stick.sh) -----------------------------
if [[ -z "$SIZE_MB" ]]; then
    total_iso_bytes=0
    for iso in "${ISOS[@]}"; do
        total_iso_bytes=$(( total_iso_bytes + $(stat -c '%s' "$iso") ))
    done
    needed_mb=$(( total_iso_bytes * 3 / 2 / 1024 / 1024 + 600 ))
    SIZE_MB=$(( needed_mb > 2048 ? needed_mb : 2048 ))
fi
log "stick size: ${SIZE_MB} MiB"

IMG="$ROOT_DIR/out/aegis-boot.img"

if (( SKIP_BUILD == 0 )); then
    # --- Rebuild rescue-tui + initramfs ---------------------------------
    # build-initramfs.sh assembles target/release/rescue-tui into
    # out/initramfs.cpio.gz, but neither it nor mkusb.sh rebuild rescue-tui
    # itself. Without these two explicit steps a stale binary from an
    # earlier checkout silently ships into the image — the captured PNGs
    # then reflect that old source. Both are deterministic under
    # SOURCE_DATE_EPOCH so re-running on an unchanged tree is a no-op.
    # (#478)
    log "ensuring target/release/rescue-tui is up to date with current source"
    SOURCE_DATE_EPOCH=1700000000 cargo build --release -p rescue-tui
    log "rebuilding out/initramfs.cpio.gz so it embeds the fresh rescue-tui"
    SOURCE_DATE_EPOCH=1700000000 "$ROOT_DIR/scripts/build-initramfs.sh"

    # --- Build the base image via mkusb.sh ------------------------------
    # IMPORTANT: do NOT pass MKUSB_GRUB_DEFAULT=1 here. That env var picks
    # the serial-primary GRUB entry, which puts /init's stderr (and the
    # rescue-tui's render output) on ttyS0. We're capturing the VNC
    # display, so we want entry 0 (default) which leaves the TUI on tty0
    # = the VNC framebuffer. Mkusb's CI/E2E scripts use serial-primary
    # because they run -nographic; this capture script wants the
    # opposite. (#478)
    log "building base image via scripts/mkusb.sh (sudo may prompt for kernel read)"
    DISK_SIZE_MB="$SIZE_MB" "$ROOT_DIR/scripts/mkusb.sh"
    [[ -f "$IMG" ]] || { echo "mkusb.sh did not produce $IMG" >&2; exit 1; }
else
    # --- Reuse the existing image (#626) --------------------------------
    # --skip-build is for fast iteration on scenario keys/timings: skip
    # the cargo build + initramfs assembly + mkusb (which together take
    # ~2 minutes) and reuse what's already on disk. The expected
    # workflow is `--keep` once, then `--skip-build` repeatedly. If the
    # rescue-tui source has actually changed, --skip-build will silently
    # capture stale behavior — bypass deliberate.
    [[ -f "$IMG" ]] || {
        echo "--skip-build set but $IMG does not exist." >&2
        echo "Run once without --skip-build (and with --keep) first." >&2
        exit 1
    }
    [[ -f "$ROOT_DIR/out/initramfs.cpio.gz" ]] || {
        echo "--skip-build set but out/initramfs.cpio.gz is missing." >&2
        exit 1
    }
    log "--skip-build: reusing $IMG ($(stat -c '%y' "$IMG" | cut -d. -f1))"
fi

# --- Copy ISOs onto AEGIS_ISOS (mirrors qemu-loaded-stick.sh) ---------
# Skipped under --skip-build — the kept image already has them from the
# previous run, and re-copying serves no purpose. (#626)
if (( ${#ISOS[@]} > 0 && SKIP_BUILD == 0 )); then
    log "loop-mounting AEGIS_ISOS to copy ISOs"
    LOOP=$(sudo losetup --find --show --partscan "$IMG")
    cleanup_loop() {
        sudo umount "$MNT" 2>/dev/null || true
        sudo losetup -d "$LOOP" 2>/dev/null || true
        rmdir "$MNT" 2>/dev/null || true
    }
    trap cleanup_loop EXIT
    MNT=$(mktemp -d --tmpdir aegis-capture-tui-mnt-XXXXXX)
    DATA_PART="${LOOP}p2"
    [[ -b "$DATA_PART" ]] || {
        echo "expected partition device $DATA_PART not present" >&2
        exit 1
    }
    sudo mount "$DATA_PART" "$MNT"
    for iso in "${ISOS[@]}"; do
        log "  copying $(basename "$iso")"
        sudo cp "$iso" "$MNT/"
        for sidecar in "${iso}.sha256" "${iso}.SHA256SUMS" "${iso}.minisig"; do
            [[ -f "$sidecar" ]] && sudo cp "$sidecar" "$MNT/"
        done
    done
    sudo sync
    cleanup_loop
    trap - EXIT
fi

# --- Set up QEMU control sockets + log paths ---------------------------
WORK=$(mktemp -d --tmpdir aegis-capture-tui-XXXXXX)
QMP_SOCK="$WORK/qmp.sock"
SERIAL_LOG="$WORK/serial.log"
PPM_DIR="$WORK/ppm"
mkdir -p "$PPM_DIR" "$OUT_DIR"

OVMF_CODE="${OVMF_CODE:-/usr/share/OVMF/OVMF_CODE_4M.secboot.fd}"
OVMF_VARS_SRC="${OVMF_VARS_SRC:-/usr/share/OVMF/OVMF_VARS_4M.ms.fd}"
[[ -r "$OVMF_CODE" ]] || { echo "missing $OVMF_CODE" >&2; exit 1; }
[[ -r "$OVMF_VARS_SRC" ]] || { echo "missing $OVMF_VARS_SRC" >&2; exit 1; }

cp "$OVMF_VARS_SRC" "$WORK/vars.fd"
chmod 0644 "$WORK/vars.fd"

final_cleanup() {
    if [[ -n "$QEMU_PID" ]] && kill -0 "$QEMU_PID" 2>/dev/null; then
        log "stopping QEMU (pid $QEMU_PID)"
        kill "$QEMU_PID" 2>/dev/null || true
        sleep 1
        kill -9 "$QEMU_PID" 2>/dev/null || true
    fi
    # --skip-build implies "user is iterating against an existing image";
    # keep the image even without --keep so the next --skip-build run
    # has something to reuse. The work dir always gets cleaned up unless
    # --keep is set explicitly. (#626)
    if (( KEEP == 0 && SKIP_BUILD == 0 )); then
        rm -rf -- "$WORK"
        [[ -f "$IMG" ]] && rm -f "$IMG"
    elif (( KEEP == 0 && SKIP_BUILD == 1 )); then
        rm -rf -- "$WORK"
        log "preserving $IMG for the next --skip-build run"
    else
        log "kept image at $IMG, work dir at $WORK"
    fi
}
trap final_cleanup EXIT

# --- Boot QEMU in the background ---------------------------------------
log "booting QEMU+OVMF (vnc :$VNC_PORT, qmp $QMP_SOCK, serial $SERIAL_LOG)"
qemu-system-x86_64 \
    -display "none" \
    -vnc ":$VNC_PORT" \
    -qmp "unix:$QMP_SOCK,server,nowait" \
    -serial "file:$SERIAL_LOG" \
    -machine q35,smm=on \
    -global driver=cfi.pflash01,property=secure,value=on \
    -m 2048M \
    -drive "if=pflash,format=raw,unit=0,file=$OVMF_CODE,readonly=on" \
    -drive "if=pflash,format=raw,unit=1,file=$WORK/vars.fd" \
    -drive "if=none,id=stick,format=raw,file=$IMG" \
    -device "virtio-blk-pci,drive=stick" \
    -boot order=c \
    -no-reboot &
QEMU_PID=$!
log "QEMU pid: $QEMU_PID"

# --- Wait for kernel ready signal on serial ---------------------------
# Tricky bit: with the default GRUB entry (entry 0), the kernel cmdline
# is `console=ttyS0 console=tty0` so the LAST console (= tty0) wins for
# /init's stderr. That means rescue-tui's own `aegis-boot rescue-tui
# starting` line goes to tty0 (= VNC framebuffer), NOT ttyS0 = serial.
# We can't poll for that.
#
# What DOES land on serial: the kernel's own boot messages. The
# "EFI stub: UEFI Secure Boot is enabled." line is the last reliable
# EFI stub print before the kernel jumps into init. After that, the
# stick uses loglevel=4 so most kernel messages are suppressed AND
# /init's `echo` writes to /dev/console = the last `console=` (tty0
# under entry 0), so /init prints don't reach serial either.
#
# That means once we see EFI stub, we have no further serial signal
# to wait on — we just have to wait long enough for /init to mount,
# modprobe, and exec rescue-tui. Empirically this is ~5-15s on this
# machine; we wait 20s to keep margin under load. The cost of waiting
# too long is a few extra seconds; the cost of waiting too short is
# capturing /init's "loading storage modules" output instead of the
# TUI (silent flake). (#478)
log "waiting for kernel boot signal on serial (up to 60s)..."
KERNEL_READY_RE='EFI stub: UEFI Secure Boot is enabled'
TUI_STARTUP_GRACE_SECONDS=20
for i in $(seq 1 30); do
    if [[ -f "$SERIAL_LOG" ]] && grep -q "$KERNEL_READY_RE" "$SERIAL_LOG" 2>/dev/null; then
        log "kernel up (took ~${i}*2s); waiting ${TUI_STARTUP_GRACE_SECONDS}s for /init + rescue-tui startup"
        sleep "$TUI_STARTUP_GRACE_SECONDS"
        break
    fi
    sleep 2
    if (( i == 30 )); then
        warn "kernel ready signal not seen in 60s — capturing whatever's on screen anyway"
        log "serial tail:"
        tail -40 "$SERIAL_LOG" 2>/dev/null >&2 || true
    fi
done

# --- QMP driver (Python) ----------------------------------------------
# Single Python helper drives the whole capture sequence. Speaking JSON
# over a unix socket is awkward in pure bash; Python keeps the QMP loop
# in one place. We sleep between actions so the TUI's redraw lands
# before we screendump.
PYTHONUNBUFFERED=1 python3 - "$QMP_SOCK" "$PPM_DIR" "$OUT_DIR" <<'PYEOF'
import json
import os
import socket
import subprocess
import sys
import time

QMP_SOCK, PPM_DIR, OUT_DIR = sys.argv[1], sys.argv[2], sys.argv[3]


class Qmp:
    def __init__(self, path):
        self.s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.s.connect(path)
        self.f = self.s.makefile("rwb", buffering=0)
        # Banner from QEMU.
        self._recv()
        self.cmd("qmp_capabilities")

    def cmd(self, name, **args):
        msg = {"execute": name}
        if args:
            msg["arguments"] = args
        self.f.write((json.dumps(msg) + "\n").encode())
        # Drain replies until we see one that's not an event.
        while True:
            r = self._recv()
            if "event" in r:
                continue
            return r

    def _recv(self):
        line = self.f.readline()
        if not line:
            raise RuntimeError("QMP socket closed unexpectedly")
        return json.loads(line.decode())

    def send_keys(self, keys, hold_ms=80):
        # `keys` is a list of qcode strings. Single press at a time so
        # the TUI sees discrete events; combos use the inner list form.
        for k in keys:
            entries = [{"type": "qcode", "data": x} for x in (k if isinstance(k, list) else [k])]
            self.cmd("send-key", keys=entries, **{"hold-time": hold_ms})
            time.sleep(0.15)

    def screendump(self, path):
        self.cmd("screendump", filename=path)


def ppm_to_png(ppm_path, png_path):
    # ImageMagick's `convert`. PPM → PNG keeps colors + dimensions
    # exactly as QEMU dumped them — no scaling, no resampling, no
    # color-space surprise.
    subprocess.check_call(["convert", ppm_path, png_path])


SCENARIOS = [
    # Each scenario: (slug, [pre-keys], settle-seconds)
    # pre-keys are sent BEFORE the screendump; the cumulative state
    # carries forward, so e.g. scenario 5 lands after scenarios 1-4's
    # keys have been processed. Reset back to list with `esc` between
    # disjoint sequences.
    ("01-list-default", [], 1.0),
    # `s` cycles sort: name → size → distro. The info bar's "sort:"
    # label changes (visible delta), unlike `tab` which only flips
    # which pane border is highlighted (visually subtle at typical
    # console resolution). (#478)
    ("02-list-sort-cycled", ["s"], 0.8),
    ("03-confirm", ["ret"], 1.5),  # Enter on list opens Confirm screen
    # `?` = shift+/. QEMU's qcode list has no "shift-slash" — combos
    # are sent as a list of qcodes pressed simultaneously. (#478)
    ("04-help", ["esc", ["shift", "slash"]], 1.0),  # esc back to list, ? for help
    ("05-filter-empty", ["esc", "slash"], 0.8),  # esc closes help, / opens filter
    ("06-filter-typed", ["u", "b"], 0.8),
    ("07-second-iso-selected", ["esc", "esc", "down"], 1.0),  # clear filter, move down
]

q = Qmp(QMP_SOCK)
print(f"[capture-tui-py] connected to QMP at {QMP_SOCK}", file=sys.stderr)

for slug, keys, settle in SCENARIOS:
    if keys:
        q.send_keys(keys)
    time.sleep(settle)
    ppm_path = os.path.join(PPM_DIR, f"{slug}.ppm")
    png_path = os.path.join(OUT_DIR, f"{slug}.png")
    q.screendump(ppm_path)
    # `screendump` returns when QEMU has scheduled the dump, not when
    # it's been written. Tiny grace period so the file exists + is fully
    # flushed before we hand it to ImageMagick.
    for _ in range(20):
        if os.path.exists(ppm_path) and os.path.getsize(ppm_path) > 0:
            break
        time.sleep(0.1)
    ppm_to_png(ppm_path, png_path)
    print(f"[capture-tui-py] {slug}: {png_path}", file=sys.stderr)

# Clean shutdown so QEMU doesn't write an unsaved-shutdown nag to OVMF vars.
q.cmd("quit")
print("[capture-tui-py] sent quit", file=sys.stderr)
PYEOF

# QEMU exits cleanly via the Python `quit` command above; harvest its
# exit code so a non-zero status surfaces to the script's caller.
wait "$QEMU_PID" 2>/dev/null || true
QEMU_PID=""

log "captures complete — PNGs in $OUT_DIR"
ls -lh "$OUT_DIR"/*.png 2>/dev/null | sed 's/^/  /' >&2
