# rescue-tui visual previews

Two complementary previews live here:

1. **ANSI dumps** (`rescue-tui-preview.ansi`) — fast, deterministic,
   rendered from in-code fixtures via the `tui-screenshots` binary.
   Covers all tier states (#477).
2. **Real-VM PNGs** (`*.png`) — captured from a real boot under
   QEMU+OVMF SecureBoot via `scripts/capture-tui-screenshots.sh`
   (#478). Proves the on-VM render matches intent.

Regenerate ANSI dumps with:

```bash
cargo run -p rescue-tui --bin tui-screenshots > docs/screenshots/rescue-tui-preview.ansi
```

Regenerate PNGs with (~5 min, needs sudo for kernel-read + losetup):

```bash
scripts/capture-tui-screenshots.sh -d ./test-isos
```

## Viewing

Colors display when you `cat` the file in a UTF-8 terminal that
supports ANSI escape sequences (any modern xterm / tmux / iTerm /
Windows Terminal / VS Code integrated terminal):

```bash
cat docs/screenshots/rescue-tui-preview.ansi
# or, for paging:
less -R docs/screenshots/rescue-tui-preview.ansi
```

To render in a browser, convert with `aha`:

```bash
aha < docs/screenshots/rescue-tui-preview.ansi > /tmp/preview.html
xdg-open /tmp/preview.html
```

Install `aha`: `sudo apt install aha` (or `brew install aha` on macOS).

## Scenarios included

| Slug | What it shows |
| ---- | ------------- |
| `01-empty-list` | Empty stick — no ISOs, no failures. Rescue-shell entry still available. |
| `02-mixed-tiers-list-focused` | 6 ISOs across all tiers. List pane focused (default). |
| `03-mixed-tiers-info-focused` | Same list, info pane focused via Tab. Border brightens, list dims. |
| `04-tier4-parse-failed-selected` | Tier 4 (ParseFailed) row selected. Info pane shows reason + disables boot. |
| `05-tier5-secure-boot-blocked` | Tier 5 (SecureBootBlocked) — Windows ISO. Info pane names the kexec incompatibility. |
| `06-tier6-hash-mismatch` | Tier 6 (HashMismatch) — tamper signal with expected vs actual digests. |
| `07-filter-editing` | `/` filter active, typed "ubuntu" to narrow the list. |
| `08-help-overlay` | Help overlay (`?`) — registry-driven keybinding reference. |
| `09-confirm-screen` | Confirm screen with one-frame evidence for the selected ISO. |
| `10-trust-challenge` | Typed-confirmation challenge for tier 2/3 boots. |

## PNG scenarios (real-VM capture)

PNGs are produced by booting the aegis-boot stick image under
QEMU+OVMF SecureBoot, driving rescue-tui via QMP `send-key`, and
dumping the VNC framebuffer. Each PNG ~6-15 KB.

| Slug | What it shows |
| ---- | ------------- |
| `01-list-default` | Initial list view, default sort: name. |
| `02-list-sort-cycled` | After `s` — sort cycled to size↓. |
| `03-confirm` | Confirm-kexec view for the first ISO (GRAY verdict for unsigned). |
| `04-help` | `?` help overlay — full keybindings reference. |
| `05-filter-empty` | After `/` — filter input opened, list still full. |
| `06-filter-typed` | After typing `ub` — list narrowed to "Ubuntu" matches. |
| `07-second-iso-selected` | After `↓` — second ISO highlighted. |

See `scripts/capture-tui-screenshots.sh` for the full pipeline +
`docs/screenshots/README.md` for the per-scenario details.

## Why both ANSI and PNG

ANSI dumps are bin-only, no image dependencies, and deterministic
(no live filesystem / TPM / Secure-Boot reads), so a reviewer gets
byte-identical output regardless of host. They cover all tier states
including synthetic-only ones (parse-failed, SB-blocked Windows,
hash mismatch).

PNGs prove that the same code path renders identically on a real
boot — they're the real-hardware oracle. They're limited to scenarios
reachable from arbitrary ISO inputs.

## Updating

The fixtures live in
[`crates/rescue-tui/src/bin/tui_screenshots.rs`](../../crates/rescue-tui/src/bin/tui_screenshots.rs)
as plain Rust functions. Add a new scenario by appending to the
`scenarios` vec in `main()` and re-running the command above.

Commit the regenerated `.ansi` file alongside the fixture change so
reviewers see the visual diff.
