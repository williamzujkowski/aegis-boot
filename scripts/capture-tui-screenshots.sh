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
  -h, --help          this message

Captures these scenarios as PNGs:
  01-list-default            initial rescue-tui list view
  02-list-info-focused       after Tab — info pane focused
  03-confirm                 after Enter on first ISO
  04-help                    after ? — help overlay
  05-filter-empty            after / — filter input opened
  06-filter-typed            after typing 'ub' — filter narrowing
  07-second-iso-info         after Down + Tab — different ISO selected, info shown
USAGE
}

while (( $# > 0 )); do
    case "$1" in
        -d|--iso-dir)  ISO_DIR="$2"; shift 2 ;;
        -o|--out)      OUT_DIR="$2"; shift 2 ;;
        -s|--size)     SIZE_MB="$2"; shift 2 ;;
        -k|--keep)     KEEP=1; shift ;;
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

# --- Build the base image via mkusb.sh ---------------------------------
log "building base image via scripts/mkusb.sh (sudo may prompt for kernel read)"
DISK_SIZE_MB="$SIZE_MB" "$ROOT_DIR/scripts/mkusb.sh"
IMG="$ROOT_DIR/out/aegis-boot.img"
[[ -f "$IMG" ]] || { echo "mkusb.sh did not produce $IMG" >&2; exit 1; }

# --- Copy ISOs onto AEGIS_ISOS (mirrors qemu-loaded-stick.sh) ---------
if (( ${#ISOS[@]} > 0 )); then
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
    if (( KEEP == 0 )); then
        rm -rf -- "$WORK"
        [[ -f "$IMG" ]] && rm -f "$IMG"
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

# --- Wait for rescue-tui ready signal on serial -----------------------
# rescue-tui prints `aegis-boot rescue-tui starting` to stderr at startup;
# `-serial file:` captures stderr from /init's exec. Poll for that line
# before driving keystrokes — typing into a not-yet-ready TUI is a
# silent flake source.
log "waiting for rescue-tui ready signal (up to 180s)..."
for i in $(seq 1 90); do
    if [[ -f "$SERIAL_LOG" ]] && grep -q "aegis-boot rescue-tui starting" "$SERIAL_LOG" 2>/dev/null; then
        log "rescue-tui ready (took ~${i}*2s)"
        break
    fi
    sleep 2
    if (( i == 90 )); then
        warn "rescue-tui ready signal not seen in 180s — capturing whatever's on screen anyway"
        log "serial tail:"
        tail -40 "$SERIAL_LOG" 2>/dev/null >&2 || true
    fi
done
# Extra settle so the first frame is fully painted.
sleep 3

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
    ("02-list-info-focused", ["tab"], 0.8),
    ("03-confirm", ["tab", "ret"], 1.5),  # tab to restore list-pane focus, then Enter
    ("04-help", ["esc", "shift-slash"], 1.0),  # esc back to list, ? for help
    ("05-filter-empty", ["esc", "slash"], 0.8),  # esc closes help, / opens filter
    ("06-filter-typed", ["u", "b"], 0.8),
    ("07-second-iso-info", ["esc", "esc", "down", "tab"], 1.0),  # clear filter, down, focus info
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
