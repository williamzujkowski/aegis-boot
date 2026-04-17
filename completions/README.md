# Shell completions for aegis-boot

Hand-written completion files for bash and zsh. Dynamic slug enumeration reads `aegis-boot recommend --slugs-only`, so the completions stay in sync with the installed catalog without requiring a rebuild.

## Bash

**System-wide:**

```bash
sudo install -m 0644 completions/aegis-boot.bash \
  /usr/share/bash-completion/completions/aegis-boot
```

**Per-user:**

```bash
mkdir -p ~/.local/share/bash-completion/completions
cp completions/aegis-boot.bash \
   ~/.local/share/bash-completion/completions/aegis-boot
```

New shells pick it up automatically if `bash-completion` is installed. To activate in the current shell without reopening:

```bash
source /usr/share/bash-completion/completions/aegis-boot
```

## Zsh

**System-wide:**

```bash
sudo install -m 0644 completions/_aegis-boot \
  /usr/share/zsh/site-functions/_aegis-boot
```

**Per-user** — add `completions/` to `$fpath` in `~/.zshrc`:

```zsh
fpath=(/path/to/aegis-boot/completions $fpath)
autoload -U compinit && compinit
```

## What gets completed

| Context                               | Completion                                                                    |
| ------------------------------------- | ----------------------------------------------------------------------------- |
| `aegis-boot <TAB>`                    | Subcommand list: `init flash list add doctor recommend fetch attest eject`    |
| `aegis-boot init <TAB>`               | `/dev/sd*` block devices                                                      |
| `aegis-boot init --profile <TAB>`     | `panic-room minimal server`                                                   |
| `aegis-boot recommend <TAB>`          | Catalog slugs (live from `aegis-boot recommend --slugs-only`)                 |
| `aegis-boot fetch <TAB>`              | Catalog slugs (live)                                                          |
| `aegis-boot add <TAB>`                | `*.iso` files in cwd                                                          |
| `aegis-boot flash/list/eject <TAB>`   | `/dev/sd*` block devices                                                      |
| `aegis-boot attest <TAB>`             | `list show`                                                                   |
| `aegis-boot <cmd> --<TAB>`            | Per-subcommand flags                                                          |

## Why not clap-auto-generated?

aegis-boot uses raw argv parsing (not clap) to keep the static-musl binary small (~855 KiB). Hand-written completion files are ~60 / ~90 lines each; clap would add ~300 KiB to the binary for a UX that tab-complete already delivers.

## Keeping the completions honest

- The bash completion hardcodes the subcommand list — if a new subcommand is added to `main.rs`, add it to the `subcommands` variable at the top of `aegis-boot.bash`.
- Same for zsh's `subcommands` array.
- Profile names are dynamic via `aegis-boot init --list-profiles`.
- Catalog slugs are dynamic via `aegis-boot recommend --slugs-only`.
