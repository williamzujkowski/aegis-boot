#!/bin/sh
# aegis-boot installer.
#
# Downloads the latest (or a specific) release of the `aegis-boot`
# operator CLI, verifies its Sigstore cosign signature, and installs
# the binary to /usr/local/bin (or ~/.local/bin if non-root).
#
# Usage:
#   curl -sSL https://raw.githubusercontent.com/williamzujkowski/aegis-boot/main/scripts/install.sh | sh
#   sh install.sh --version v0.12.0
#   sh install.sh --prefix ~/.local/bin
#   sh install.sh --no-verify             # skip cosign (NOT recommended)
#
# Exit codes:
#   0  success
#   1  install failed (download / verify / write)
#   2  usage error
#   64 cosign missing and verification was requested
#
# This script is wrapped in a `main` function so that a truncated
# `curl | sh` download will fail to define `main` and exit cleanly
# rather than executing partial logic.

set -eu

REPO="williamzujkowski/aegis-boot"
DEFAULT_PREFIX_ROOT="/usr/local/bin"
DEFAULT_PREFIX_USER="$HOME/.local/bin"
COSIGN_IDENTITY_REGEXP='^https://github\.com/williamzujkowski/aegis-boot/\.github/workflows/release\.yml@refs/tags/v.+$'
COSIGN_OIDC_ISSUER='https://token.actions.githubusercontent.com'

usage() {
    cat <<EOF
aegis-boot installer

USAGE:
  install.sh [--version VER] [--prefix DIR] [--no-verify] [--help]

OPTIONS:
  --version VER   Install a specific release tag (e.g. v0.12.0).
                  Default: latest GitHub release.
  --prefix DIR    Install destination. Default: $DEFAULT_PREFIX_ROOT
                  if running as root, else $DEFAULT_PREFIX_USER.
  --no-verify     Skip cosign signature verification.
                  Strongly discouraged outside dev/test.
  --help          This message.

EXAMPLES:
  curl -sSL https://raw.githubusercontent.com/$REPO/main/scripts/install.sh | sh
  sh install.sh --version v0.12.0
  sudo sh install.sh --prefix /usr/local/bin
EOF
}

# --- platform detection ----------------------------------------------------

detect_os() {
    case "$(uname -s)" in
        Linux)  echo "linux" ;;
        Darwin) echo "darwin" ;;
        *) echo "unsupported" ;;
    esac
}

detect_arch() {
    case "$(uname -m)" in
        x86_64|amd64)  echo "x86_64" ;;
        aarch64|arm64) echo "aarch64" ;;
        *) echo "unsupported" ;;
    esac
}

# --- helpers ---------------------------------------------------------------

err() { printf 'install.sh: error: %s\n' "$*" >&2; }
note() { printf 'install.sh: %s\n' "$*"; }

require_cmd() {
    if ! command -v "$1" >/dev/null 2>&1; then
        err "missing required command: $1"
        return 1
    fi
}

# Print a download URL for an asset on a given release tag.
# Uses the `latest` API when tag = "" (empty).
asset_url() {
    asset="$1"
    tag="$2"
    if [ -z "$tag" ]; then
        printf 'https://github.com/%s/releases/latest/download/%s' "$REPO" "$asset"
    else
        printf 'https://github.com/%s/releases/download/%s/%s' "$REPO" "$tag" "$asset"
    fi
}

# Download with curl or wget, whichever's available. Writes to $2.
download() {
    url="$1"
    dest="$2"
    if command -v curl >/dev/null 2>&1; then
        curl --proto '=https' --tlsv1.2 --fail --silent --show-error --location \
            --output "$dest" "$url"
    elif command -v wget >/dev/null 2>&1; then
        wget --quiet --output-document "$dest" "$url"
    else
        err "neither curl nor wget found — install one and retry"
        return 1
    fi
}

# --- main ------------------------------------------------------------------

main() {
    version=""
    prefix=""
    skip_verify=0
    while [ "$#" -gt 0 ]; do
        case "$1" in
            --version)
                shift
                version="${1:-}"
                if [ -z "$version" ]; then
                    err "--version requires a tag argument"
                    return 2
                fi
                ;;
            --version=*)
                version="${1#*=}"
                ;;
            --prefix)
                shift
                prefix="${1:-}"
                if [ -z "$prefix" ]; then
                    err "--prefix requires a directory argument"
                    return 2
                fi
                ;;
            --prefix=*)
                prefix="${1#*=}"
                ;;
            --no-verify)
                skip_verify=1
                ;;
            --help|-h)
                usage
                return 0
                ;;
            *)
                err "unknown option: $1"
                usage >&2
                return 2
                ;;
        esac
        shift
    done

    # Resolve install prefix.
    if [ -z "$prefix" ]; then
        if [ "$(id -u)" -eq 0 ]; then
            prefix="$DEFAULT_PREFIX_ROOT"
        else
            prefix="$DEFAULT_PREFIX_USER"
        fi
    fi

    # Detect platform.
    os="$(detect_os)"
    arch="$(detect_arch)"
    if [ "$os" = "unsupported" ] || [ "$arch" = "unsupported" ]; then
        err "unsupported platform: $(uname -s)/$(uname -m)"
        err "supported today: linux/x86_64 (linux/aarch64, darwin/*, windows tracked in #137 and #123)"
        return 1
    fi
    if [ "$os" != "linux" ] || [ "$arch" != "x86_64" ]; then
        err "no published binary for $os/$arch yet"
        err "today's release ships only linux/x86_64; cross-platform expansion is tracked in #123"
        return 1
    fi

    asset="aegis-boot-${arch}-${os}"
    note "platform: $os/$arch"
    note "release:  ${version:-latest}"
    note "prefix:   $prefix"
    if [ "$skip_verify" -eq 1 ]; then
        note "WARNING: cosign verification disabled (--no-verify). Trust at your own risk."
    fi

    # Need curl or wget for downloads.
    require_cmd uname || return 1
    if ! command -v curl >/dev/null 2>&1 && ! command -v wget >/dev/null 2>&1; then
        err "need curl or wget"
        return 1
    fi

    # Cosign required unless --no-verify.
    if [ "$skip_verify" -eq 0 ]; then
        if ! command -v cosign >/dev/null 2>&1; then
            err "cosign not found in PATH. Install from https://docs.sigstore.dev/cosign/installation/"
            err "or re-run with --no-verify (not recommended)"
            return 64
        fi
    fi

    # Stage downloads in a tempdir.
    tmpdir="$(mktemp -d "${TMPDIR:-/tmp}/aegis-boot-install.XXXXXX")"
    trap 'rm -rf "$tmpdir"' EXIT

    note "downloading $asset..."
    download "$(asset_url "$asset" "$version")" "$tmpdir/$asset"
    if [ "$skip_verify" -eq 0 ]; then
        download "$(asset_url "${asset}.sig" "$version")" "$tmpdir/$asset.sig"
        download "$(asset_url "${asset}.pem" "$version")" "$tmpdir/$asset.pem"
    fi

    # Verify cosign signature.
    if [ "$skip_verify" -eq 0 ]; then
        note "verifying cosign signature..."
        if ! cosign verify-blob \
            --certificate-identity-regexp "$COSIGN_IDENTITY_REGEXP" \
            --certificate-oidc-issuer "$COSIGN_OIDC_ISSUER" \
            --signature "$tmpdir/$asset.sig" \
            --certificate "$tmpdir/$asset.pem" \
            "$tmpdir/$asset" >/dev/null 2>&1
        then
            err "cosign verification FAILED for $asset"
            err "this binary did NOT come from $REPO's release.yml workflow"
            err "do NOT install. Re-download from a different network if you suspect MITM."
            return 1
        fi
        note "  cosign: OK (identity matches $REPO release workflow)"
    fi

    # Install.
    chmod +x "$tmpdir/$asset"
    target="$prefix/aegis-boot"

    if [ ! -d "$prefix" ]; then
        # Try to create with the right tool.
        if [ "$prefix" = "$DEFAULT_PREFIX_USER" ] || [ -w "$(dirname "$prefix")" ]; then
            mkdir -p "$prefix"
        else
            err "$prefix does not exist and $(id -un) cannot create it"
            err "create it first (e.g. sudo mkdir -p $prefix) and re-run"
            return 1
        fi
    fi

    if [ -w "$prefix" ]; then
        cp "$tmpdir/$asset" "$target"
    elif command -v sudo >/dev/null 2>&1; then
        note "elevating with sudo to write $prefix"
        sudo cp "$tmpdir/$asset" "$target"
    else
        err "$prefix not writable and sudo not available"
        err "re-run with --prefix \$HOME/.local/bin or as root"
        return 1
    fi

    note "installed: $target"

    # Helpful PATH notice.
    case ":$PATH:" in
        *":$prefix:"*) ;;
        *)
            note ""
            note "NOTE: $prefix is not in your PATH."
            note "      Add it to your shell rc, e.g.:"
            note "        echo 'export PATH=\"$prefix:\$PATH\"' >> ~/.bashrc"
            ;;
    esac

    # Completion install — best-effort, never fail the installer. The
    # completion files are versioned in the repo (not shipped as release
    # assets) so fetch them directly from GitHub raw. If a specific
    # --version was requested, pull completions from the same tag for
    # consistency; otherwise pull from main.
    install_completions "$version"

    note ""
    note "Try it:"
    note "  $target --version"
    note "  $target doctor"
    note "  $target recommend"
}

# Install bash + zsh completion files from the repo (raw.githubusercontent.com).
# Best-effort — any failure prints a hint and proceeds. The binary install
# is the main deliverable; completions are a convenience.
install_completions() {
    version_or_main="$1"
    # Completions live in completions/ at the repo root. For a tagged
    # install, pull from the exact tag; otherwise track main.
    ref="${version_or_main:-main}"
    if [ "$ref" = "latest" ] || [ -z "$ref" ]; then
        ref=main
    fi
    bash_url="https://raw.githubusercontent.com/$REPO/$ref/completions/aegis-boot.bash"
    zsh_url="https://raw.githubusercontent.com/$REPO/$ref/completions/_aegis-boot"

    # Bash: system path for root, per-user otherwise.
    if [ "$(id -u)" -eq 0 ]; then
        bash_dest=/usr/share/bash-completion/completions/aegis-boot
    else
        bash_dest="$HOME/.local/share/bash-completion/completions/aegis-boot"
    fi
    bash_dir="$(dirname "$bash_dest")"
    if mkdir -p "$bash_dir" 2>/dev/null \
        && curl -fsSL --proto '=https' --tlsv1.2 -o "$bash_dest" "$bash_url" 2>/dev/null; then
        note "bash completion: $bash_dest"
    else
        note "(bash completion not installed — download from $bash_url)"
    fi

    # Zsh: system path only when root (per-user zsh fpath varies wildly).
    if [ "$(id -u)" -eq 0 ]; then
        zsh_dest=/usr/share/zsh/site-functions/_aegis-boot
        zsh_dir="$(dirname "$zsh_dest")"
        if mkdir -p "$zsh_dir" 2>/dev/null \
            && curl -fsSL --proto '=https' --tlsv1.2 -o "$zsh_dest" "$zsh_url" 2>/dev/null; then
            note "zsh completion:  $zsh_dest"
        else
            note "(zsh completion not installed — download from $zsh_url)"
        fi
    else
        note "(zsh completion skipped; re-run as root, or fetch manually:"
        note "   curl -sSL $zsh_url -o ~/.zsh/completions/_aegis-boot"
        note "   and add ~/.zsh/completions to your fpath)"
    fi
}

main "$@"
