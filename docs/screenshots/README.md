# rescue-tui visual previews

Terminal-ready ANSI dumps of the new dual-pane rescue-tui (epic #455)
rendered from in-code fixtures. Regenerate with:

```bash
cargo run -p rescue-tui --bin tui-screenshots > docs/screenshots/rescue-tui-preview.ansi
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

## Why ANSI files, not PNG

Bin-only, no image dependencies. The fixtures are deterministic
(no live filesystem / TPM / Secure-Boot reads), so a reviewer gets
byte-identical output regardless of host. For real-hardware
screenshots (stick booted in QEMU + OVMF), see
[LOCAL_TESTING.md](../LOCAL_TESTING.md#iterating-on-specific-tests)
and the `scripts/qemu-loaded-stick.sh` path — that route requires
building the initramfs and flashing a stick or image (~10 min).

## Updating

The fixtures live in
[`crates/rescue-tui/src/bin/tui_screenshots.rs`](../../crates/rescue-tui/src/bin/tui_screenshots.rs)
as plain Rust functions. Add a new scenario by appending to the
`scenarios` vec in `main()` and re-running the command above.

Commit the regenerated `.ansi` file alongside the fixture change so
reviewers see the visual diff.
