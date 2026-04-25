<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

# aegis-boot demos

Inline asciinema recordings of the three operator-facing flows
referenced in [#348](https://github.com/aegis-boot/aegis-boot/issues/348),
rendered to SVG so GitHub markdown displays them natively without
depending on an external host (asciinema.org).

## Status

This directory holds the **harness** — the scripts that record + render
the demos — but **not yet the recordings themselves**. Recording the
three flows is interactive (asciinema needs a real terminal driving the
TUI / CLI flows + a built `aegis-boot.img` for the QEMU demo); a
maintainer or contributor with the dev environment runs the harness
once and commits the resulting `.svg` files.

When the SVGs land, this directory will contain:

| File                          | Flow                                                       |
| ----------------------------- | ---------------------------------------------------------- |
| `01-quickstart.svg`           | `aegis-boot quickstart /dev/sdc` — sub-10-minute happy path |
| `02-init.svg`                 | `aegis-boot init /dev/sdc --yes` — 3-distro panic-room      |
| `03-qemu-boot.svg`            | QEMU+OVMF SecureBoot → rescue-tui ISO selection             |

The README's "What it does" section embeds these via plain
`<img src="docs/demos/<n>-<name>.svg">` once they exist.

## Recording

```bash
# Record one flow (asciinema is interactive — drives a real terminal).
sudo scripts/record-demos.sh quickstart      # 01-quickstart.cast
sudo scripts/record-demos.sh init            # 02-init.cast
scripts/record-demos.sh qemu-boot            # 03-qemu-boot.cast

# Or all three, sequentially.
sudo scripts/record-demos.sh all
```

`AEGIS_DEMO_TARGET=/dev/loopN scripts/record-demos.sh init` overrides
the default target (`/dev/loop99`) for the destructive flash flows
(quickstart, init). Use a loop device for reproducibility — the QEMU
demo doesn't need a target since it boots a pre-built `aegis-boot.img`.

## Rendering

```bash
# Renders every .cast in docs/demos/casts/ to docs/demos/<name>.svg.
scripts/render-demos.sh
```

Tries `svg-term-cli` first (preferred — produces SVG), falls back to
`agg` (produces GIF) if svg-term-cli isn't installed:

```bash
# Either of:
npm install -g svg-term-cli
go install github.com/asciinema/agg@latest
```

## Why inline SVG and not asciinema.org

Three reasons, per the [maintainer alignment in #348](https://github.com/aegis-boot/aegis-boot/issues/348#issuecomment-4320353920):

1. **Self-contained repo.** Operators reading the README on a
   no-network preview (e.g. cloned offline) still see the demos.
2. **No external availability dependency.** asciinema.org outages
   would invisibly break the README's primary onboarding visual.
3. **Reproducible rendering.** SVG is checked in; the rendered file
   is what every reader sees, no per-viewer player initialization or
   third-party JS.

`agg`'s GIF fallback is supported but discouraged — bigger files (≥10×
SVG size) and worse legibility on mobile/print.
