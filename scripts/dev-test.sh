#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# Full local validation run. Replaces GHA CI during development when
# billing is constrained or you want instant feedback.
#
# Runs in order, short-circuiting on failure:
#   1. cargo fmt --check            (no sudo)
#   2. cargo clippy -D warnings     (no sudo)
#   3. cargo test --workspace       (no sudo)
#   4. ./scripts/build-initramfs.sh (no sudo)
#   5. ./scripts/mkusb.sh           (needs kernel read — sudo once)
#   6. ./scripts/qemu-try.sh --headless (boots under OVMF SB)
#   7. ./scripts/qemu-kexec-e2e.sh  (sudo for loop-mount + kexec)
#   8. cargo run -p aegis-fitness   (repo + artifact health audit)
#
# Approx 6-8 minutes on a Framework laptop.
#
# Prereqs (installed once):
#   sudo apt-get install -y \
#     qemu-system-x86 ovmf \
#     shim-signed grub-efi-amd64-signed linux-image-generic \
#     mtools dosfstools exfatprogs gdisk \
#     busybox-static cpio xorriso util-linux
#
# Prereq (once per kernel update):
#   sudo chmod 0644 /boot/vmlinuz-* /boot/initrd.img-*

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

step() { printf '\n\033[1;36m== %s ==\033[0m\n' "$*"; }
warn() { printf '\033[1;33m[warn]\033[0m %s\n' "$*"; }

# Check prereqs up front — fail fast if something is missing.
missing=()
for t in cargo qemu-system-x86_64 mcopy mmd sgdisk xorriso busybox cpio; do
    command -v "$t" >/dev/null 2>&1 || missing+=("$t")
done
for f in /usr/lib/shim/shimx64.efi.signed \
         /usr/lib/grub/x86_64-efi-signed/grubx64.efi.signed \
         /usr/share/OVMF/OVMF_CODE_4M.secboot.fd \
         /usr/share/OVMF/OVMF_VARS_4M.ms.fd; do
    [[ -r "$f" ]] || missing+=("$f")
done
if (( ${#missing[@]} > 0 )); then
    echo "missing prerequisites:" >&2
    printf '  %s\n' "${missing[@]}" >&2
    echo "" >&2
    echo "run (one-time):" >&2
    echo "  sudo apt-get install -y qemu-system-x86 ovmf shim-signed \\" >&2
    echo "       grub-efi-amd64-signed linux-image-generic mtools dosfstools \\" >&2
    echo "       exfatprogs gdisk busybox-static cpio xorriso util-linux" >&2
    exit 1
fi

# Find a readable kernel. If none is 0644, the user needs to chmod.
KERNEL_READABLE=""
for k in /boot/vmlinuz-*-virtual /boot/vmlinuz-*-generic /boot/vmlinuz-*; do
    [[ -e "$k" && -r "$k" && ! -L "$k" ]] || continue
    KERNEL_READABLE="$k"
    break
done
if [[ -z "$KERNEL_READABLE" ]]; then
    echo "no readable /boot/vmlinuz-* found" >&2
    echo "run (one-time): sudo chmod 0644 /boot/vmlinuz-* /boot/initrd.img-*" >&2
    exit 1
fi

export SOURCE_DATE_EPOCH=1700000000

step "1/8 cargo fmt --check"
cargo fmt --all -- --check

step "2/8 cargo clippy"
cargo clippy --workspace --all-targets -- -D warnings

step "3/8 cargo test"
cargo test --workspace

step "4/8 build-initramfs"
cargo build --release -p rescue-tui
rm -rf out
./scripts/build-initramfs.sh

step "5/8 mkusb"
DISK_SIZE_MB="${DISK_SIZE_MB:-1024}" ./scripts/mkusb.sh

step "6/8 qemu-try (headless, 60s timeout)"
TIMEOUT_SECONDS=60 timeout 90 bash -c '
    cp /usr/share/OVMF/OVMF_VARS_4M.ms.fd /tmp/aegis-dev-vars.fd
    chmod 0644 /tmp/aegis-dev-vars.fd
    qemu-system-x86_64 -nographic -no-reboot \
        -machine q35,smm=on \
        -global driver=cfi.pflash01,property=secure,value=on \
        -m 1024M \
        -drive if=pflash,format=raw,unit=0,file=/usr/share/OVMF/OVMF_CODE_4M.secboot.fd,readonly=on \
        -drive if=pflash,format=raw,unit=1,file=/tmp/aegis-dev-vars.fd \
        -drive if=ide,format=raw,file=out/aegis-boot.img \
        -boot order=c \
        -serial mon:stdio </dev/null
' 2>&1 | tee /tmp/aegis-qemu-try.log || true
if grep -qiE 'secure boot (is )?enabled' /tmp/aegis-qemu-try.log \
   && grep -q 'aegis-boot rescue-tui starting' /tmp/aegis-qemu-try.log; then
    echo "  qemu-try: PASS"
else
    echo "  qemu-try: FAIL — see /tmp/aegis-qemu-try.log" >&2
    exit 1
fi

step "7/8 kexec E2E (sudo required for loop-mount)"
if sudo -n true 2>/dev/null; then
    sudo -E ./scripts/qemu-kexec-e2e.sh
else
    warn "sudo not passwordless; run manually:"
    echo "  sudo -E ./scripts/qemu-kexec-e2e.sh"
fi

step "8/8 aegis-fitness audit"
cargo run --quiet --release -p aegis-fitness

step "ALL GREEN — ready to push"
