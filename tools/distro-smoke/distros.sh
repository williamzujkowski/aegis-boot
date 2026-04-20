#!/bin/sh
# distros.sh — per-distro probe recipes for distro-smoke.
#
# Each probe prints a self-contained bootstrap script to stdout. The
# script assumes it will run inside a fresh container of the named
# image, with /usr/local/bin/aegis-boot bind-mounted read-only from
# the host (release build).
#
# The probe is INTENTIONALLY minimal — install only the deps `doctor`
# needs, then run `aegis-boot doctor --json` and `aegis-boot doctor`.
# We do NOT install `aegis-boot` itself from the release channel
# (that'd also exercise install.sh + cosign, which is separate surface).
#
# Output format: each bootstrap script should end with a marker line
# `=== DISTRO-SMOKE END ===` so the orchestrator can detect clean exit.

# Update when adding distros. Format: "<name>|<image>|<probe-fn>".
# Keep the order stable — summary.md groups by this list.
DISTROS="
opensuse|opensuse/tumbleweed:latest|probe_opensuse
ubuntu|ubuntu:24.04|probe_ubuntu
alpine|alpine:3.20|probe_alpine
fedora|fedora:40|probe_fedora
arch|archlinux:latest|probe_arch
"

probe_opensuse() {
    cat <<'EOF'
set -eu
zypper --non-interactive refresh >/dev/null 2>&1
# util-linux-systemd is the package that actually provides lsblk on
# openSUSE Tumbleweed; plain util-linux doesn't. Smoke-harness detail.
zypper --non-interactive install -y \
    gptfdisk util-linux util-linux-systemd curl coreutils gpg2 mokutil sudo \
    >/dev/null 2>&1
echo "=== ENV ==="
echo "PATH=$PATH"
echo "USER=$(id -un) ($(id -u))"
sgdisk_probe="$(command -v sgdisk 2>&1 || echo NOT_ON_PATH)"
echo "sgdisk-on-path: $sgdisk_probe"
ls -la /usr/sbin/sgdisk 2>&1 || true
echo "=== DOCTOR (human) ==="
aegis-boot doctor 2>&1 | head -40 || true
echo "=== DOCTOR (json) ==="
aegis-boot doctor --json 2>&1 | head -100 || true
echo "=== INSTALL.SH --help ==="
/usr/local/bin/install-sh --help 2>&1 | head -30 || true
echo "=== DISTRO-SMOKE END ==="
EOF
}

probe_ubuntu() {
    cat <<'EOF'
set -eu
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null 2>&1
apt-get install -yqq \
    gdisk util-linux curl coreutils gnupg mokutil sudo \
    >/dev/null 2>&1
echo "=== ENV ==="
echo "PATH=$PATH"
echo "USER=$(id -un) ($(id -u))"
sgdisk_probe="$(command -v sgdisk 2>&1 || echo NOT_ON_PATH)"
echo "sgdisk-on-path: $sgdisk_probe"
ls -la /usr/sbin/sgdisk 2>&1 || true
echo "=== DOCTOR (human) ==="
aegis-boot doctor 2>&1 | head -40 || true
echo "=== DOCTOR (json) ==="
aegis-boot doctor --json 2>&1 | head -100 || true
echo "=== INSTALL.SH --help ==="
/usr/local/bin/install-sh --help 2>&1 | head -30 || true
echo "=== DISTRO-SMOKE END ==="
EOF
}

probe_alpine() {
    cat <<'EOF'
set -eu
apk add --no-cache sgdisk util-linux curl coreutils gnupg sudo gcompat \
    >/dev/null 2>&1
echo "=== ENV ==="
echo "PATH=$PATH"
echo "USER=$(id -un) ($(id -u))"
sgdisk_probe="$(command -v sgdisk 2>&1 || echo NOT_ON_PATH)"
echo "sgdisk-on-path: $sgdisk_probe"
ls -la /usr/sbin/sgdisk 2>&1 || true
echo "=== DOCTOR (human) ==="
aegis-boot doctor 2>&1 | head -40 || true
echo "=== DOCTOR (json) ==="
aegis-boot doctor --json 2>&1 | head -100 || true
echo "=== INSTALL.SH --help ==="
/usr/local/bin/install-sh --help 2>&1 | head -30 || true
echo "=== DISTRO-SMOKE END ==="
EOF
}

probe_fedora() {
    cat <<'EOF'
set -eu
dnf -q -y install \
    gdisk util-linux curl coreutils gnupg2 mokutil sudo \
    >/dev/null 2>&1
echo "=== ENV ==="
echo "PATH=$PATH"
echo "USER=$(id -un) ($(id -u))"
sgdisk_probe="$(command -v sgdisk 2>&1 || echo NOT_ON_PATH)"
echo "sgdisk-on-path: $sgdisk_probe"
ls -la /usr/sbin/sgdisk 2>&1 || true
echo "=== DOCTOR (human) ==="
aegis-boot doctor 2>&1 | head -40 || true
echo "=== DOCTOR (json) ==="
aegis-boot doctor --json 2>&1 | head -100 || true
echo "=== INSTALL.SH --help ==="
/usr/local/bin/install-sh --help 2>&1 | head -30 || true
echo "=== DISTRO-SMOKE END ==="
EOF
}

probe_arch() {
    cat <<'EOF'
set -eu
pacman -Sy --noconfirm --needed \
    gptfdisk util-linux curl coreutils gnupg sudo \
    >/dev/null 2>&1
echo "=== ENV ==="
echo "PATH=$PATH"
echo "USER=$(id -un) ($(id -u))"
sgdisk_probe="$(command -v sgdisk 2>&1 || echo NOT_ON_PATH)"
echo "sgdisk-on-path: $sgdisk_probe"
ls -la /usr/sbin/sgdisk 2>&1 || true
echo "=== DOCTOR (human) ==="
aegis-boot doctor 2>&1 | head -40 || true
echo "=== DOCTOR (json) ==="
aegis-boot doctor --json 2>&1 | head -100 || true
echo "=== INSTALL.SH --help ==="
/usr/local/bin/install-sh --help 2>&1 | head -30 || true
echo "=== DISTRO-SMOKE END ==="
EOF
}
