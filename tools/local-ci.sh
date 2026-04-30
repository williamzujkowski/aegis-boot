#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# tools/local-ci.sh — local libvirt/USB E2E harness mirroring CI.
#
# Closes the inner-loop gap: edit → push → wait 5-15 min for CI is
# replaced with edit → tools/local-ci.sh <subcommand> → result in
# 1-3 min on a developer host with libvirt + qemu installed.
#
# Each subcommand wraps the same scripts/*.sh that the corresponding
# .github/workflows/*.yml file invokes — no reimplementation, just a
# friendlier dispatcher with prerequisite checks and clearer output.
#
# Usage:
#   tools/local-ci.sh <subcommand> [args]
#   tools/local-ci.sh --help
#   tools/local-ci.sh --list
#
# Exit codes:
#   0  — success
#   1  — subcommand failed (boot smoke, build, etc.)
#   2  — usage error (unknown subcommand, missing arg)
#   3  — missing prerequisite (qemu, OVMF firmware, etc.)
#
# Refs: epic #580, issue #581.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$SCRIPT_DIR"

# ---- Prerequisite probes ----------------------------------------------------

require_cmd() {
    local cmd="$1"
    local hint="${2:-}"
    if ! command -v "$cmd" >/dev/null 2>&1; then
        echo "error: required command '$cmd' not found in PATH" >&2
        if [[ -n "$hint" ]]; then
            echo "  hint: $hint" >&2
        fi
        exit 3
    fi
}

require_file() {
    local path="$1"
    local hint="${2:-}"
    if [[ ! -e "$path" ]]; then
        echo "error: required file '$path' not found" >&2
        if [[ -n "$hint" ]]; then
            echo "  hint: $hint" >&2
        fi
        exit 3
    fi
}

require_qemu() {
    require_cmd qemu-system-x86_64 \
        "install qemu (apt: qemu-system-x86; dnf: qemu-system-x86; pacman: qemu-base)"
}

require_ovmf() {
    # Standard distro paths for OVMF firmware. Caller picks one.
    local found=""
    for candidate in \
        /usr/share/OVMF/OVMF_CODE_4M.secboot.fd \
        /usr/share/OVMF/OVMF_CODE.secboot.fd \
        /usr/share/edk2-ovmf/x64/OVMF_CODE.secboot.fd \
        /usr/share/edk2/x64/OVMF_CODE.secboot.4m.fd; do
        if [[ -f "$candidate" ]]; then
            found="$candidate"
            break
        fi
    done
    if [[ -z "$found" ]]; then
        echo "error: OVMF firmware not found in standard distro paths" >&2
        echo "  hint: install ovmf (apt: ovmf; dnf: edk2-ovmf; pacman: edk2-ovmf)" >&2
        exit 3
    fi
}

require_rust() {
    require_cmd cargo "install rust toolchain via rustup (https://rustup.rs)"
}

# Run cargo against CI's pinned toolchain (1.95.0) when available, falling
# back to the default toolchain. We use a function rather than a string flag
# because passing an empty +<toolchain> argument to cargo would be
# interpreted as a positional and fail. CI uses 1.95.0 — matching it
# locally avoids "looks fine here, fails there" lint surprises.
cargo_run() {
    if rustup toolchain list 2>/dev/null | grep -q "1.95.0"; then
        cargo +1.95.0 "$@"
    else
        cargo "$@"
    fi
}

# Emit the "pin missing" warning at most once per harness run. The repeat-
# noise during a multi-step subcommand (build + initramfs + boot) is not
# worth the volume.
warn_about_toolchain_once() {
    if [[ -n "${_TOOLCHAIN_WARNED:-}" ]]; then
        return
    fi
    _TOOLCHAIN_WARNED=1
    if ! rustup toolchain list 2>/dev/null | grep -q "1.95.0"; then
        echo "warn: Rust 1.95.0 toolchain not installed; using default toolchain" >&2
        echo "  hint: \`rustup toolchain install 1.95.0\` to match CI's lint set exactly" >&2
    fi
}

# ---- Subcommands ------------------------------------------------------------

cmd_quick() {
    # Fast inner loop — cargo check + clippy + lib unit tests on the workspace.
    # Targets ~30s on a moderate dev host. No QEMU, no E2E.
    require_rust
    warn_about_toolchain_once
    echo "==> cargo fmt --check"
    cargo_run fmt --check
    echo "==> cargo check (workspace)"
    cargo_run check --workspace --all-targets
    echo "==> cargo clippy (workspace, tests, -D warnings)"
    cargo_run clippy --workspace --all-targets --tests -- -D warnings
    echo "==> cargo test --lib (workspace)"
    cargo_run test --workspace --lib
    echo "✓ quick suite passed"
}

cmd_kexec() {
    # Mirrors .github/workflows/kexec-e2e.yml. Builds rescue-tui + initramfs,
    # then runs scripts/qemu-kexec-e2e.sh (which boots a QEMU VM, runs
    # rescue-tui in AEGIS_AUTO_KEXEC mode, and asserts the target kernel's
    # boot banner). Requires sudo for losetup and -enable-kvm.
    require_qemu
    require_rust
    warn_about_toolchain_once
    require_file ./scripts/qemu-kexec-e2e.sh "expected at scripts/qemu-kexec-e2e.sh"
    echo "==> Building rescue-tui (release)"
    SOURCE_DATE_EPOCH=1700000000 cargo_run build --release -p rescue-tui
    echo "==> Building initramfs"
    SOURCE_DATE_EPOCH=1700000000 ./scripts/build-initramfs.sh
    echo "==> Running kexec E2E (sudo required for losetup)"
    TIMEOUT_SECONDS="${TIMEOUT_SECONDS:-180}" sudo -E ./scripts/qemu-kexec-e2e.sh
    echo "✓ kexec E2E passed"
}

cmd_ovmf_secboot() {
    # Mirrors .github/workflows/ovmf-secboot.yml's E2E job. Boots the signed
    # chain (shim → grub → kernel) under SB-enforcing OVMF, then asserts
    # rescue-tui banner.
    require_qemu
    require_ovmf
    require_rust
    warn_about_toolchain_once
    require_file ./scripts/ovmf-secboot-e2e.sh "expected at scripts/ovmf-secboot-e2e.sh"
    echo "==> Building rescue-tui (release)"
    SOURCE_DATE_EPOCH=1700000000 cargo_run build --release -p rescue-tui
    echo "==> Building initramfs"
    SOURCE_DATE_EPOCH=1700000000 ./scripts/build-initramfs.sh
    echo "==> Running OVMF SecBoot E2E"
    TIMEOUT_SECONDS="${TIMEOUT_SECONDS:-120}" ./scripts/ovmf-secboot-e2e.sh
    echo "✓ OVMF SecBoot E2E passed"
}

cmd_mkusb() {
    # Mirrors .github/workflows/mkusb.yml. Builds the bootable image via
    # scripts/mkusb.sh and runs the QEMU boot smoke against it.
    require_qemu
    require_rust
    warn_about_toolchain_once
    require_file ./scripts/mkusb.sh "expected at scripts/mkusb.sh"
    echo "==> Building rescue-tui (release)"
    SOURCE_DATE_EPOCH=1700000000 cargo_run build --release -p rescue-tui
    echo "==> Building initramfs"
    SOURCE_DATE_EPOCH=1700000000 ./scripts/build-initramfs.sh
    echo "==> Building USB image via scripts/mkusb.sh"
    sudo -E MKUSB_GRUB_DEFAULT=1 ./scripts/mkusb.sh
    if [[ -x ./scripts/qemu-loaded-stick.sh ]]; then
        echo "==> Boot smoke against built image"
        TIMEOUT_SECONDS="${TIMEOUT_SECONDS:-120}" ./scripts/qemu-loaded-stick.sh
    fi
    echo "✓ mkusb build + smoke passed"
}

cmd_qemu_smoke() {
    # Mirrors .github/workflows/qemu-smoke.yml — minimal initramfs boot smoke.
    require_qemu
    require_rust
    warn_about_toolchain_once
    require_file ./scripts/qemu-smoke.sh "expected at scripts/qemu-smoke.sh"
    echo "==> Building rescue-tui (release)"
    SOURCE_DATE_EPOCH=1700000000 cargo_run build --release -p rescue-tui
    echo "==> Building initramfs"
    SOURCE_DATE_EPOCH=1700000000 ./scripts/build-initramfs.sh
    echo "==> Running QEMU initramfs boot smoke"
    TIMEOUT_SECONDS="${TIMEOUT_SECONDS:-60}" ./scripts/qemu-smoke.sh
    echo "✓ QEMU smoke passed"
}

cmd_test_mode() {
    # Local QEMU smoke for the aegis-hwsim test modes (#675/#676/#695).
    # Builds rescue-tui + initramfs, boots under QEMU with
    # `aegis.test=<NAME>` injected via QEMU_SMOKE_TEST_MODE, asserts
    # the per-mode start landmark fires.
    #
    # Use to verify the dispatcher + landmark wording locally without
    # running a full aegis-hwsim scenario over real hardware. ~2-3 min
    # per mode on a moderate dev host.
    local mode="${1:-}"
    case "$mode" in
        kexec-unsigned|mok-enroll|manifest-roundtrip)
            ;;
        ""|--help|-h)
            cat >&2 <<EOF
error: test-mode subcommand requires a mode name

Usage:
  tools/local-ci.sh test-mode <NAME>

Where <NAME> is one of:
  kexec-unsigned        attempt unsigned kexec; expect EKEYREJECTED (#675)
  mok-enroll            print MOK enrollment walkthrough (#676)
  manifest-roundtrip    parse on-stick manifest, compare PCRs (#695)

See docs/rescue-tui-serial-format.md for the landmarks each mode emits.
EOF
            exit 2
            ;;
        *)
            echo "error: unknown test-mode '$mode'" >&2
            echo "  valid: kexec-unsigned, mok-enroll, manifest-roundtrip" >&2
            exit 2
            ;;
    esac
    require_qemu
    require_rust
    warn_about_toolchain_once
    require_file ./scripts/qemu-smoke.sh "expected at scripts/qemu-smoke.sh"
    echo "==> Building rescue-tui (release)"
    SOURCE_DATE_EPOCH=1700000000 cargo_run build --release -p rescue-tui
    echo "==> Building initramfs"
    SOURCE_DATE_EPOCH=1700000000 ./scripts/build-initramfs.sh
    echo "==> Running QEMU smoke with aegis.test=$mode"
    QEMU_SMOKE_TEST_MODE="$mode" \
        TIMEOUT_SECONDS="${TIMEOUT_SECONDS:-60}" \
        ./scripts/qemu-smoke.sh
    echo "✓ test-mode '$mode' smoke passed"
}

cmd_thumb_drive() {
    # Real-hardware E2E against the operator's attached USB stick. Writes to
    # the device — gated behind --confirm-write so a typo can't nuke /dev/sda.
    local target=""
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --confirm-write)
                target="${2:-}"
                shift 2 || { echo "error: --confirm-write needs a device path" >&2; exit 2; }
                ;;
            *)
                echo "error: unknown thumb-drive arg: $1" >&2
                exit 2
                ;;
        esac
    done
    if [[ -z "$target" ]]; then
        cat >&2 <<EOF
error: thumb-drive subcommand requires --confirm-write /dev/sdX

This subcommand WRITES to a physical block device. The --confirm-write gate
exists so a typo can't nuke your system disk. Verify the device path with
\`lsblk\` first, then re-run:

  tools/local-ci.sh thumb-drive --confirm-write /dev/sdX

Hint: removable USB drives typically show up as sdb / sdc on a host whose
system disk is /dev/nvme0n1 (which is most modern Linux laptops).
EOF
        exit 2
    fi
    if [[ ! -b "$target" ]]; then
        echo "error: $target is not a block device" >&2
        exit 2
    fi
    # Pure-USB / removable verification: check the kernel's removable bit.
    local devname
    devname="$(basename "$target")"
    if [[ -e /sys/block/"$devname"/removable ]]; then
        local rem
        rem="$(cat /sys/block/"$devname"/removable)"
        if [[ "$rem" != "1" ]]; then
            echo "error: $target is not flagged removable by the kernel (sysfs removable=$rem)" >&2
            echo "  hint: refusing to write to a fixed disk; use --confirm-write only on real USB drives" >&2
            exit 2
        fi
    fi
    require_rust
    warn_about_toolchain_once
    echo "==> Building aegis-boot CLI (release)"
    cargo_run build --release -p aegis-bootctl
    echo "==> Flashing $target via aegis-boot flash"
    # The CLI itself owns the safety prompts beyond this; we just delegate.
    sudo ./target/release/aegis-boot flash --stick "$target" --yes
    echo "✓ thumb-drive flashed; eject + boot manually to validate"
    echo "  (or: tools/local-ci.sh qemu-smoke once you've re-imaged the image to a loopback)"
}

cmd_all() {
    # Run the full local suite, fail-fast. Skips thumb-drive (no implicit
    # device write) — the operator must invoke that explicitly.
    cmd_quick
    cmd_qemu_smoke
    cmd_kexec
    cmd_ovmf_secboot
    cmd_mkusb
    echo "✓ all (sans thumb-drive) passed"
}

# ---- Dispatch ---------------------------------------------------------------

print_help() {
    cat <<'EOF'
tools/local-ci.sh — local libvirt/USB E2E harness mirroring CI

Usage:
  tools/local-ci.sh <subcommand> [args]

Subcommands:
  quick           cargo fmt + check + clippy + lib unit tests (~30s)
  kexec           rescue-tui → target kernel kexec E2E under QEMU (~3min)
  ovmf-secboot    SB-enforcing signed chain → rescue-tui under QEMU (~4min)
  mkusb           build bootable image + boot smoke under QEMU (~4min)
  qemu-smoke      minimal initramfs boot smoke (~2min)
  test-mode <NAME>  smoke an aegis-hwsim test mode (kexec-unsigned, mok-enroll,
                    manifest-roundtrip) — ~2min per mode
  thumb-drive     write to attached USB stick (requires --confirm-write /dev/sdX)
  all             run the full suite (sans thumb-drive), fail-fast (~12min)
  --help          this help
  --list          list subcommands without descriptions

Each subcommand calls the same scripts/*.sh that the corresponding
.github/workflows/*.yml invokes — no reimplementation, just a friendlier
dispatcher with prerequisite checks. Intended to replace the
edit → push → wait 5-15 min CI loop with a 1-3 min local one.

Exit codes:
  0  success
  1  subcommand failed
  2  usage error
  3  missing prerequisite (qemu, OVMF firmware, etc.)

Environment overrides:
  TIMEOUT_SECONDS   per-subcommand QEMU timeout (defaults vary by command)
  SOURCE_DATE_EPOCH overridden internally to 1700000000 to match CI

See docs/development/LOCAL_CI.md for a fuller operator playbook.
EOF
}

main() {
    if [[ $# -eq 0 ]]; then
        print_help
        exit 2
    fi
    local sub="$1"
    shift
    case "$sub" in
        quick)         cmd_quick "$@" ;;
        kexec)         cmd_kexec "$@" ;;
        ovmf-secboot)  cmd_ovmf_secboot "$@" ;;
        mkusb)         cmd_mkusb "$@" ;;
        qemu-smoke)    cmd_qemu_smoke "$@" ;;
        test-mode)     cmd_test_mode "$@" ;;
        thumb-drive)   cmd_thumb_drive "$@" ;;
        all)           cmd_all "$@" ;;
        --help|-h)     print_help ;;
        --list)        echo "quick kexec ovmf-secboot mkusb qemu-smoke test-mode thumb-drive all" ;;
        *)
            echo "error: unknown subcommand '$sub'" >&2
            echo "       run 'tools/local-ci.sh --help' for usage" >&2
            exit 2
            ;;
    esac
}

main "$@"
