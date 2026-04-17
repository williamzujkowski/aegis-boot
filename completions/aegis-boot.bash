# Bash completion for aegis-boot.
#
# Install (system-wide, needs root):
#   sudo install -m 0644 completions/aegis-boot.bash \
#     /usr/share/bash-completion/completions/aegis-boot
#
# Install (per-user):
#   mkdir -p ~/.local/share/bash-completion/completions
#   cp completions/aegis-boot.bash \
#     ~/.local/share/bash-completion/completions/aegis-boot
#
# Dynamic slug completion reads `aegis-boot recommend --slugs-only`,
# so the completion stays in sync with the catalog across releases —
# no hardcoded ISO list here that would drift.

_aegis_boot() {
    local cur prev words cword
    _init_completion || return

    local subcommands="init flash list add doctor recommend fetch attest eject update verify compat completions --help --version"
    local attest_actions="list show"

    # Top-level subcommand or global flag.
    if [[ $cword -eq 1 ]]; then
        COMPREPLY=($(compgen -W "$subcommands" -- "$cur"))
        return 0
    fi

    # First arg after a subcommand.
    local sub="${words[1]}"
    case "$sub" in
        init|flash|add|list|doctor|eject|update|verify)
            # Device arg — complete block device paths + typical flags.
            case "$prev" in
                --profile)
                    # Pull live profile list from the binary; stays in
                    # sync with whatever the installed aegis-boot ships.
                    local profiles
                    profiles="$(aegis-boot init --list-profiles 2>/dev/null)"
                    if [[ -n "$profiles" ]]; then
                        COMPREPLY=($(compgen -W "$profiles" -- "$cur"))
                    fi
                    return 0
                    ;;
            esac
            if [[ "$cur" == --* ]]; then
                case "$sub" in
                    init)   COMPREPLY=($(compgen -W "--profile --yes --no-doctor --no-gpg --help" -- "$cur")) ;;
                    flash)  COMPREPLY=($(compgen -W "--yes --help" -- "$cur")) ;;
                    doctor) COMPREPLY=($(compgen -W "--stick --json --help" -- "$cur")) ;;
                    add)    COMPREPLY=($(compgen -W "--help" -- "$cur")) ;;
                    list)   COMPREPLY=($(compgen -W "--json --help" -- "$cur")) ;;
                    eject)  COMPREPLY=($(compgen -W "--help" -- "$cur")) ;;
                    update) COMPREPLY=($(compgen -W "--json --help" -- "$cur")) ;;
                    verify) COMPREPLY=($(compgen -W "--json --help" -- "$cur")) ;;
                esac
                return 0
            fi
            # Prefer /dev/sd* block devices; fall back to file completion for add.
            if [[ "$sub" == "add" ]]; then
                _filedir iso
            else
                COMPREPLY=($(compgen -f -- "$cur" | grep -E '^/dev/' 2>/dev/null))
                [[ ${#COMPREPLY[@]} -eq 0 ]] && _filedir
            fi
            return 0
            ;;
        recommend|fetch)
            if [[ "$cur" == --* ]]; then
                case "$sub" in
                    recommend) COMPREPLY=($(compgen -W "--slugs-only --json --help" -- "$cur")) ;;
                    fetch)     COMPREPLY=($(compgen -W "--out --no-gpg --dry-run --help" -- "$cur")) ;;
                esac
                return 0
            fi
            # Pull live slug list from the binary itself — stays current
            # with whatever catalog the installed aegis-boot knows about.
            local slugs
            slugs="$(aegis-boot recommend --slugs-only 2>/dev/null)"
            if [[ -n "$slugs" ]]; then
                COMPREPLY=($(compgen -W "$slugs" -- "$cur"))
            fi
            return 0
            ;;
        attest)
            if [[ $cword -eq 2 ]]; then
                COMPREPLY=($(compgen -W "$attest_actions --json --help" -- "$cur"))
                return 0
            fi
            _filedir json
            return 0
            ;;
        compat)
            if [[ "$cur" == --* ]]; then
                COMPREPLY=($(compgen -W "--my-machine --json --help" -- "$cur"))
                return 0
            fi
            # Vendor tokens when `jq` is installed; silently skipped otherwise.
            if command -v jq >/dev/null 2>&1; then
                local vendors
                vendors="$(aegis-boot compat --json 2>/dev/null \
                    | jq -r '.entries[]?.vendor' 2>/dev/null | sort -u)"
                if [[ -n "$vendors" ]]; then
                    COMPREPLY=($(compgen -W "$vendors" -- "$cur"))
                fi
            fi
            return 0
            ;;
        completions)
            COMPREPLY=($(compgen -W "bash zsh --help" -- "$cur"))
            return 0
            ;;
    esac
}

complete -F _aegis_boot aegis-boot
