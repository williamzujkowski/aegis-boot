# rescue-tui UX Overhaul — dual-pane + all-ISOs-visible

**Status:** Design approved 2026-04-23. Epic tracked in [#455].
**Owner:** aegis-boot
**Supersedes:** the SKIPPED-banner UX shipped pre-epic.

## Problem

A tester flashed a stick, dropped ISOs onto `AEGIS_ISOS` without running
`aegis-boot add`, and booted into rescue-tui expecting to see them. The TUI
reported them as a yellow `SKIPPED` count banner at the top of the list with
no actionable detail — the operator saw nothing to click on.

Three failure modes were conflated:

1. **Bare ISO** — file is bootable but has no sibling `.sha256` / `.minisig`,
   so we can't attest the operator verified the bytes.
2. **Parse failure** — `iso-parser` couldn't extract a kernel + initrd from
   the ISO (unfamiliar layout, corrupt filesystem, truncated file).
3. **Secure Boot mismatch** — the ISO's kernel isn't signed by a CA in the
   platform or MOK keyring, so `kexec_file_load` would reject at boot time.

The old UX hid all three as an aggregate count. The new UX surfaces them as
three distinct trust tiers, each with a verdict color, a glyph, and — for
blocked ISOs — a descriptive error the operator can act on.

## Trust-tier model

| Tier | Verdict                 | Color  | Glyph | Bootable? | Condition                                                                   |
| ---- | ----------------------- | ------ | ----- | --------- | --------------------------------------------------------------------------- |
| 1    | `OperatorAttested`      | green  | `[+]` | yes       | Hash verified against sibling `.sha256` OR minisig against trusted key.     |
| 2    | `BareUnverified`        | gray   | `[ ]` | yes*      | Parseable, kexec-bootable, no sibling verification material.                |
| 3    | `KeyNotTrusted`         | yellow | `[~]` | yes*      | Sig parses but key not in `AEGIS_TRUSTED_KEYS`. Operator override required. |
| 4    | `ParseFailed`           | red    | `[!]` | no        | iso-parser couldn't extract kernel/initrd. Descriptive reason shown.        |
| 5    | `SecureBootBlocked`     | red    | `[X]` | no        | Kernel signature fails platform keyring. Descriptive reason shown.          |
| 6    | `HashMismatch` (forged) | red    | `[X]` | no        | Sibling `.sha256` disagrees with ISO bytes. Strong anti-tamper signal.      |

\* Tier 2/3 boot requires a typed-confirmation challenge (`type boot`) to
prevent accidental trust widening. Tier 1 boots with a single Enter.

**Priority guarantee:** the operator always sees *everything on the stick*.
Tiers 4/5/6 are not hidden — they appear in the list with a descriptive
reason in the info pane and Enter disabled. This is the single biggest
departure from the previous design.

**Design principle:** *Secure Boot stays strict; operator attestation
relaxes gracefully.* An ISO that fails signature verification is never
bootable, regardless of operator opinion. An ISO that lacks operator
attestation is bootable with friction proportional to the missing
signal. Ventoy's failure mode is collapsing these two layers via a
permissive MOK; we keep them distinct.

## Layout — dual-pane

Reference analog: **gitui** (Rust + ratatui, 2-pane split, context footer).

```
┌────────────────────────────────────────────────────────────────────────┐
│ AEGIS-BOOT Rescue · stick AEGIS_ISOS (14.2 GiB free) · 5 ISOs          │  ← header (1 line)
├──────────────────────────────────┬─────────────────────────────────────┤
│ > [+] Ubuntu 24.04 LTS           │ Ubuntu 24.04 LTS                    │
│   [+] Fedora 40 Workstation      │ ─────────────────────────────────── │
│   [ ] alpine-standard-3.20.iso   │ Verdict:  OperatorAttested          │
│   [!] windows11-installer.iso    │ File:     ubuntu-24.04-live…iso     │
│   [X] my-custom-kernel.iso       │ Size:     2.47 GiB                  │
│                                  │ sha256:   a1b2c3d4…  (verified)     │
│                                  │ Signer:   aegis-catalog-2026 (✓)    │
│                                  │ Kernel:   casper/vmlinuz            │
│                                  │ Initrd:   casper/initrd             │
│                                  │ Cmdline:  boot=casper quiet splash  │
│                                  │ Distro:   Debian family             │
│                                  │ Quirks:   none                      │
│                                  │                                     │
│                                  │                                     │
├──────────────────────────────────┴─────────────────────────────────────┤
│ ↑↓ nav · Tab switch pane · Enter boot · / filter · ? help · q quit     │  ← footer (1 line)
└────────────────────────────────────────────────────────────────────────┘
      ↑ focus: list                  ↑ info pane (scrolls independently)
```

Widths: 40% list / 60% info pane (`Layout::horizontal` with
`Constraint::Percentage(40/60)`).

Vertical nest: `Length(2) + Min(0) + Length(1)` for header/body/footer.

**Focus model:** `enum Pane { List, Info }` + `Tab` toggle. Active pane gets
bright border and highlight cursor; inactive is dimmed. No widget library —
the existing theme system handles 16-color-safe styling.

## Info pane — what it shows per tier

### Tier 1 (OperatorAttested)
```
Verdict:   OperatorAttested       ← green, bold
File:      <filename>
Size:      <human-readable>
sha256:    <hex 12-char>…  (verified against <source>)
Signer:    <key id>       (✓ trusted)
Kernel:    <rel path>
Initrd:    <rel path>
Cmdline:   <kernel args>
Distro:    <family>
Quirks:    none | <quirks list>
```

### Tier 2 (BareUnverified)
```
Verdict:   BareUnverified          ← gray
File:      <filename>
Size:      <human-readable>
sha256:    —  (no sibling .sha256)
Signer:    —  (no sibling .minisig)
…kernel/initrd/cmdline/distro/quirks same as Tier 1…

Note: this ISO is bootable, but the operator has not attested the
      bytes match an expected hash. `aegis-boot add --scan` (issue #455)
      can retroactively generate sidecars.
```

### Tier 3 (KeyNotTrusted)
```
Verdict:   KeyNotTrusted           ← yellow, bold
…
Signer:    <key id>       (✗ not in AEGIS_TRUSTED_KEYS)

Note: signature is structurally valid but the signer is unknown to
      this stick. Add the key to AEGIS_TRUSTED_KEYS to upgrade this
      ISO to OperatorAttested.
```

### Tier 4 (ParseFailed)
```
Verdict:   ParseFailed             ← red, bold
File:      <filename>
Size:      <human-readable>

Reason: <wrapped iso-parser error, e.g.
        "mount failed: /dev/loop3: wrong fs type, bad option, bad
         superblock on /dev/loop3, missing codepage or helper program,
         or other error">

This ISO could not be loop-mounted or did not contain a recognized
boot layout. Boot is disabled.
```

### Tier 5 (SecureBootBlocked)
```
Verdict:   SecureBootBlocked       ← red, bold
File:      <filename>
Size:      <human-readable>

Reason: <wrapped signature-check error, e.g.
        "kernel signature rejected by platform keyring:
         unknown signer (key hash: 7fabc…)">

This ISO's kernel is not signed by a CA trusted by this platform.
`kexec_file_load` would reject at boot time. Boot is disabled.
```

### Tier 6 (HashMismatch)
```
Verdict:   HashMismatch — FORGED?  ← red, bold, blink
…

Reason: sibling .sha256 declares <expected>, ISO bytes hash to <actual>.
        Source: <sha256 file path>.

This ISO's bytes do not match the declared hash. Either the file was
modified after hashing, the hash file is wrong, or something is
tampering with the stick. Boot is disabled.
```

Long reason strings are pre-wrapped via the `textwrap` crate (see
[ratatui issue #2342] for why we don't rely on `Paragraph::wrap`).

## API changes

### iso-parser (#456a)

Add a new method (existing `scan_directory` stays as a thin wrapper for
backwards compat with any external consumer):

```rust
pub struct ScanReport {
    pub entries: Vec<BootEntry>,
    pub failures: Vec<ScanFailure>,
}

pub struct ScanFailure {
    pub iso_path: PathBuf,
    pub reason: String,       // human-readable, safe for TUI rendering
    pub kind: ScanFailureKind, // structured for tier decision
}

pub enum ScanFailureKind {
    MountFailed,
    NoBootEntries,
    UnknownLayout,
    IoError,
}

impl IsoParser {
    pub async fn scan_directory_with_failures(
        &self,
        path: &Path,
    ) -> Result<ScanReport, IsoError>;
}
```

`scan_directory_with_failures` does NOT return `NoBootEntries` when the
directory has ISOs that all failed — it returns a successful `ScanReport`
with empty `entries` and a populated `failures`. Empty-dir still errors
(caller needs to distinguish "no stick" from "stick with only broken
ISOs").

### iso-probe (#456b)

```rust
pub struct DiscoveryReport {
    pub isos: Vec<DiscoveredIso>,
    pub failed: Vec<FailedIso>,
}

pub struct FailedIso {
    pub iso_path: PathBuf,
    pub reason: String,
    pub kind: FailureKind, // maps 1:1 to ScanFailureKind
}

pub fn discover(roots: &[PathBuf]) -> Result<DiscoveryReport, ProbeError>;
```

`discover` returns `Ok(report)` whenever the filesystem walk succeeded,
even if every ISO failed parsing. `NoIsosFound` is reserved for "walk
found zero `.iso` files" — the caller should distinguish "empty stick"
from "stick with broken ISOs".

## rescue-tui changes

### State (#457)

```rust
pub enum TrustVerdict {
    OperatorAttested,
    BareUnverified,
    KeyNotTrusted,
    ParseFailed { reason: String },
    SecureBootBlocked { reason: String },
    HashMismatch { expected: String, actual: String, source: String },
}

pub struct AppState {
    // existing fields...
    pub isos: Vec<DiscoveredIso>,
    pub failed_isos: Vec<FailedIso>,   // NEW — tier 4 entries
    pub pane: Pane,                    // NEW — focus target
    pub info_scroll: u16,              // NEW — info pane scrollback
}

pub enum Pane { List, Info }
```

`visible_entries` becomes a unified `list_rows()` that interleaves
`isos` and `failed_isos` in a single `Vec<ListRow>` sorted by name,
with a tier-derived sort key for tie-breaking.

### Render (#458 + #459)

- `draw_body` splits `main` horizontally 40/60.
- `draw_list_pane(frame, area, app, focused)` renders the ISO list.
- `draw_info_pane(frame, area, app, focused)` renders the tier-specific
  info view for the selected row.
- Focus border: `Block::bordered().border_style(if focused { bright } else { dim })`.

### Footer (#460)

A single `KeybindingRegistry` struct holds `(KeyCode, context, label)`
rows. The event loop reads it for dispatch; the footer renderer reads
it for the one-line legend, filtered by `(pane, screen)`. Lazygit
pattern — docs and code share the same source.

### Snapshot tests (#461)

Introduce `insta` for ratatui buffer snapshots. Fixtures:

- Each `TrustVerdict` variant → expected info pane
- List with 5 ISOs across all tiers → expected list pane
- Focused vs unfocused border styles

## Programmatic documentation (#462)

Extend the `constants-docgen` pattern to a `tiers-docgen` binary that
emits:

- A tier table (matches the one above) rendered from `TrustVerdict` impls
- A keybinding reference rendered from `KeybindingRegistry`
- The output replaces marker regions in:
  - `docs/HOW_IT_WORKS.md` (tier table)
  - `docs/TOUR.md` (keybindings)
  - `crates/rescue-tui/README.md` (both)

CI enforces drift-freedom via `tiers-docgen --check` (same pattern as
`constants-docgen --check`).

## Migration

Phase 1 — **API surface** (#456). iso-parser + iso-probe ship the new
`DiscoveryReport` shape. Callers updated in the same commit. No UX
change yet.

Phase 2 — **State + verdict** (#457). `TrustVerdict` gets the new
variants; `AppState` gains `failed_isos`. Rendering still uses the old
single pane but now shows all rows including tier-4/5/6.

Phase 3 — **Dual pane** (#458). Layout split lands. Tab focus toggle.
Info pane is minimal (just the verdict + filename).

Phase 4 — **Info pane content** (#459). Full per-tier info rendering.

Phase 5 — **Footer** (#460). `KeybindingRegistry` + dynamic footer.

Phase 6 — **Tests** (#461). Snapshot coverage for each tier + each pane.

Phase 7 — **Programmatic docs** (#462). `tiers-docgen` + CI drift check.

Each phase lands as a standalone PR that passes CI independently.

## Non-goals

- Live filter / search reflow — existing `/` filter UX is retained
  unchanged, just bound into the new layout.
- Mouse support. Boot environment is headless; keyboard-only.
- ratatui 0.30+ API polyfills. We're already on 0.30.

## References

- [ratatui layout concepts](https://ratatui.rs/concepts/layout/)
- [ratatui issue #2342](https://github.com/ratatui/ratatui/issues/2342) — Paragraph wrap+scroll bug
- [gitui](https://github.com/gitui-org/gitui) — closest structural analog
- [constants-docgen pattern](../../crates/aegis-cli/src/bin/constants_docgen.rs) — Phase 2 of #286
- [#454](https://github.com/aegis-boot/aegis-boot/issues/454) — tester report that triggered this work
- A future `aegis-boot add --scan` command (complementary): lets an operator retroactively upgrade drag-and-drop ISOs to Tier 1 by generating sibling sidecars. Not part of this epic.

[#455]: https://github.com/aegis-boot/aegis-boot/issues/455
[ratatui issue #2342]: https://github.com/ratatui/ratatui/issues/2342
