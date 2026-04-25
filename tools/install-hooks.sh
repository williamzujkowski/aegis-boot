#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# tools/install-hooks.sh — opt-in installer for the pre-push hook
# that runs `tools/local-ci.sh quick` (~9s) before letting `git push`
# leave the machine. Closes the inner-loop gap (#583) so contributors
# who want the safety net get it; everyone else pushes as before.
#
# Usage:
#   tools/install-hooks.sh           # install (or refresh)
#   tools/install-hooks.sh --uninstall
#   tools/install-hooks.sh --status
#
# Idempotent: re-running is a no-op when the hook is already aegis-boot's.
# Does NOT clobber a custom hook installed under a different identity —
# refuses to overwrite, telling the user how to back it up first.
#
# Escape hatch for WIP pushes: `git push --no-verify` skips the hook
# without disabling it. Documented in CONTRIBUTING.md.
#
# Refs: epic #580 Phase 0, issue #583.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$SCRIPT_DIR"

# Marker line — used to detect "this hook is ours" vs "operator wrote
# their own." Refuse to overwrite a hook that doesn't carry the marker.
readonly HOOK_MARKER="# aegis-boot pre-push hook (#583) — managed by tools/install-hooks.sh"

readonly HOOK_PATH=".git/hooks/pre-push"

usage() {
    cat <<'EOF'
tools/install-hooks.sh — opt-in pre-push hook installer

Usage:
  tools/install-hooks.sh             install (or refresh) the hook
  tools/install-hooks.sh --uninstall remove an aegis-boot-managed hook
  tools/install-hooks.sh --status    report whether the hook is installed
  tools/install-hooks.sh --help      this message

What the hook does:
  Runs `tools/local-ci.sh quick` (cargo fmt + check + clippy + lib unit
  tests, ~9s). Aborts the push on non-zero exit. To bypass for a WIP
  push: `git push --no-verify`.

Refusal cases (we never clobber):
  * .git/hooks/pre-push exists without the aegis-boot marker line
  * .git/ doesn't exist (not a git repo)
EOF
}

cmd_install() {
    if [[ ! -d .git ]]; then
        echo "error: .git/ not found — run from the repo root" >&2
        exit 2
    fi
    if [[ -f $HOOK_PATH ]] && ! grep -q "$HOOK_MARKER" "$HOOK_PATH"; then
        echo "error: $HOOK_PATH exists and is NOT managed by this script" >&2
        echo "  refusing to overwrite a custom hook" >&2
        echo "  back it up: mv $HOOK_PATH $HOOK_PATH.bak" >&2
        echo "  then re-run: tools/install-hooks.sh" >&2
        exit 1
    fi
    cat > "$HOOK_PATH" <<HOOK
#!/usr/bin/env bash
$HOOK_MARKER
# Runs the fast subset of local-ci.sh before letting the push leave the
# machine. To bypass for WIP work: \`git push --no-verify\`.
set -euo pipefail
exec ./tools/local-ci.sh quick
HOOK
    chmod +x "$HOOK_PATH"
    echo "✓ installed $HOOK_PATH (will run \`tools/local-ci.sh quick\` on git push)"
    echo "  bypass for one push: git push --no-verify"
    echo "  uninstall: tools/install-hooks.sh --uninstall"
}

cmd_uninstall() {
    if [[ ! -f $HOOK_PATH ]]; then
        echo "no hook installed at $HOOK_PATH"
        exit 0
    fi
    if ! grep -q "$HOOK_MARKER" "$HOOK_PATH"; then
        echo "error: $HOOK_PATH is not managed by this script — refusing to remove" >&2
        echo "  delete it manually if you no longer want it" >&2
        exit 1
    fi
    rm -f "$HOOK_PATH"
    echo "✓ removed $HOOK_PATH"
}

cmd_status() {
    if [[ ! -f $HOOK_PATH ]]; then
        echo "not installed"
        exit 0
    fi
    if grep -q "$HOOK_MARKER" "$HOOK_PATH"; then
        echo "installed (managed by tools/install-hooks.sh)"
    else
        echo "exists but NOT managed by this script (custom hook)"
    fi
}

main() {
    case "${1:-install}" in
        install|"")    cmd_install ;;
        --uninstall)   cmd_uninstall ;;
        --status)      cmd_status ;;
        --help|-h)     usage ;;
        *)
            echo "error: unknown arg '$1'" >&2
            usage >&2
            exit 2
            ;;
    esac
}

main "$@"
