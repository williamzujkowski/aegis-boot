# aegis-boot brand guidelines

Source: nexus-agents `ux_expert` spec, 2026-04-15. Track evolution in [#76](https://github.com/aegis-boot/aegis-boot/issues/76).

## Name

**aegis-boot** — lowercase, hyphenated. Never "Aegis-Boot", "AegisBoot", or "Aegis Boot".

## Tagline

**Signed boot. Any ISO. Your keys.**

38 characters. Use verbatim in README, social card, and TUI splash. Keep the periods — the three-clause rhythm is the point.

Alternate (longer form, for contexts where the tagline needs explanation): *"Boot any ISO from USB under UEFI Secure Boot — with keys you control."*

## Logo

Direction: **shield with keyhole**. The aegis (shield) name requires the shield to be visible; the keyhole adds the specificity for signing + access control. ASCII-native so it can live natively in a TUI.

SVG master → PNG renders at 16/32/180/192/256/512/1024 + multi-res favicon.ico.

The compact three-line variant is the authoritative in-terminal form:

```
╔═╗
║◆║
╚▲╝
```

## Color palette

| Role | Hex | ratatui `Color` | Usage |
|---|---|---|---|
| Brand primary | `#3B82F6` | `Rgb(59, 130, 246)` | Logo glyphs, active borders, focused widgets |
| Brand secondary | `#94A3B8` | `Rgb(148, 163, 184)` | Version text, inactive borders, muted labels |
| Success | `#22C55E` | `Rgb(34, 197, 94)` | Verified signatures, green verdicts |
| Warning | `#EAB308` | `Rgb(234, 179, 8)` | Unsigned media, yellow verdicts, caution prompts |
| Error | `#EF4444` | `Rgb(239, 68, 68)` | Invalid signatures, red verdicts, blocks |

All five tested against dark backgrounds and verified under deuteranopia and protanopia simulators. Distinct from Ubuntu orange, Fedora blue, Arch blue. See [palette.css](./palette.css) for CSS custom properties including `oklch()` values.

### 16-color fallback for serial consoles

| Role | 16-color ANSI |
|---|---|
| Brand primary | `Blue` |
| Secondary | `DarkGray` |
| Success | `Green` |
| Warning | `Yellow` |
| Error | `Red` |

Selected via `AEGIS_THEME=aegis-16` when 24-bit RGB is unavailable.

## Typography

- **Display:** Inter (SIL OFL) — README headings, social card.
- **Body:** Source Sans 3 (SIL OFL) — docs, rendered markdown.
- **Mono:** JetBrains Mono (SIL OFL) — TUI rendering, code blocks, terminal screenshots.

All three are free, libre, and broadly pre-installed. Never embed webfonts in the README or docs.

Fallback stacks:
- Display: `'Inter', 'Helvetica Neue', Arial, sans-serif`
- Body: `'Source Sans 3', 'Segoe UI', 'Liberation Sans', sans-serif`
- Mono: `'JetBrains Mono', 'Fira Code', 'Cascadia Code', 'Consolas', monospace`

## Usage rules

- Always preserve the compact logo's proportions — never stretch.
- Minimum clear space: one shield-width on all sides.
- On backgrounds other than `#0d1117` / pure dark, use the monochrome variant.
- Do not recolor the shield. The brand primary is load-bearing.
- Do not place the tagline without the logo (or the logo without the tagline) in marketing contexts.

## Files

See [ascii/](./ascii/) for terminal-native forms — `logo-full.txt` is the 10-line README hero variant; `logo-compact.txt` is the 3-line TUI header. Rastered PNG renders of the SVG master (`aegis-boot-logo.svg` / `aegis-boot-logo-mono.svg`) are an on-demand build step via any standard SVG-to-PNG tool (Inkscape, rsvg-convert); the repo doesn't ship pre-rendered PNGs because there's no current consumer.
