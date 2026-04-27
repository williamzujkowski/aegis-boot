# rescue-tui

The [ratatui](https://ratatui.rs)-based interactive picker that runs inside the
aegis-boot signed Linux rescue environment. Discovers ISOs on the
`AEGIS_ISOS` partition, lets the operator pick one, and hands off to
`kexec-loader` for signed kexec.

## Trust-tier model

Every ISO on the stick is classified into one of 6 tiers that drive the
list-pane badge, info-pane header, and boot gating. Tiers 4/5/6 refuse
boot; tiers 1/2/3 are bootable (tiers 2/3 with a typed-confirmation
challenge).

<!-- tiers:BEGIN:TRUST_TIER_TABLE -->
| Tier | Verdict             | Glyph | Bootable | Meaning                                    |
| ---- | ------------------- | ----- | -------- | ------------------------------------------ |
| 1    | VERIFIED            | `[+]` | yes      | Hash or sig verified vs trusted source     |
| 2    | UNVERIFIED          | `[ ]` | yes      | No sidecar — bootable with typed confirm   |
| 3    | UNTRUSTED KEY       | `[~]` | yes      | Sig parses, signer untrusted               |
| 4    | PARSE FAILED        | `[!]` | **no**   | iso-parser couldn't extract kernel         |
| 5    | BOOT BLOCKED        | `[X]` | **no**   | Kernel rejected by platform keyring        |
| 6    | HASH MISMATCH       | `[!]` | **no**   | ISO bytes don't match declared hash        |
<!-- tiers:END:TRUST_TIER_TABLE -->

This table is generated from the `TrustVerdict` enum in
[`src/verdict.rs`](src/verdict.rs) by the `tiers-docgen` devtool. To
regenerate after an enum change:

```bash
cargo run -p rescue-tui --bin tiers-docgen
```

CI runs `tiers-docgen --check` to enforce drift-freedom (see
[#462](https://github.com/aegis-boot/aegis-boot/issues/462)).

## Keybindings

Context-sensitive — the footer legend filters by the current screen
and focused pane. Press `?` inside the TUI for the full help overlay.

<!-- tiers:BEGIN:KEYBINDINGS -->
| Key | Screens | Pane | Filter-editing | Description |
| --- | ------- | ---- | -------------- | ----------- |
| `?` | any | any | no | Show the help overlay with all keybindings |
| `q` | any | any | no | Quit rescue-tui (returns control to the boot menu) |
| `↑↓/jk` | List | any | no | Move the list cursor (pane=List) or scroll info pane (pane=Info) |
| `Tab` | List | any | no | Toggle focus between the ISO list and the info pane |
| `Enter` | List | List | no | Confirm the selected ISO (only valid in the list pane) |
| `/` | List | any | no | Open the substring filter — typed chars match label + path |
| `s` | List | any | no | Cycle sort order: name → size → distro → name |
| `v` | List, Confirm | any | no | Re-compute sha256 of the selected ISO in a background thread |
| `D` | List | List | no | Delete the highlighted ISO + sidecar from the data partition (confirm prompt) |
| `y` | ConfirmDelete | any | no | Confirm — unlink the ISO and its `.aegis.toml` sidecar |
| `n/Esc` | ConfirmDelete | any | no | Cancel — return to the list without deleting |
| `n` | List, Confirm | any | no | Open the Network overlay (enable DHCP per-interface) |
| `Enter` | Network | any | no | Enable DHCP on the highlighted interface |
| `r` | Network | any | no | Re-enumerate interfaces and reset op state |
| `Esc/q` | Network | any | no | Close the Network overlay and return to the prior screen |
| `Enter` | List | any | yes | Commit the current filter and close the input |
| `Esc` | List | any | yes | Close the filter input and clear the current filter |
| `Enter` | Confirm | any | no | Kexec into the selected ISO (may trigger a trust challenge) |
| `e` | Confirm | any | no | Edit the kernel command line before boot |
| `Esc/h` | Confirm, Error | any | no | Return to the list without booting |
| `Enter` | EditCmdline | any | no | Save the edited kernel command line and return to Confirm |
| `Esc` | EditCmdline, Verifying, TrustChallenge | any | no | Discard edits and return to Confirm |
| `F10` | Error | any | no | Write a failure-log bundle to AEGIS_ISOS for post-mortem analysis |
| `boot+Enter` | TrustChallenge | any | no | Type the word 'boot' and press Enter to proceed past the trust challenge |
<!-- tiers:END:KEYBINDINGS -->

Derived from the `KEYBINDINGS` registry in
[`src/keybindings.rs`](src/keybindings.rs).

## Architecture

- **state.rs** — pure state machine (no I/O). Screen enum, AppState
  builder, transitions.
- **render.rs** — ratatui rendering. Tested with `TestBackend`.
- **verdict.rs** — the 6-tier `TrustVerdict` enum + mappers.
- **keybindings.rs** — `KEYBINDINGS` registry driving the footer.
- **theme.rs** — color palettes (default / high-contrast / monochrome /
  okabe-ito / aegis).
- **persistence.rs** — last-choice state saved across reboots.
- **tpm.rs** — pre-kexec PCR 12 measurement.
- **failure_log.rs** — F10 bundle writer for post-mortem analysis.

See [`docs/design/rescue-tui-ux-overhaul.md`](../../docs/design/rescue-tui-ux-overhaul.md)
for the dual-pane UX design (epic [#455](https://github.com/aegis-boot/aegis-boot/issues/455)).
