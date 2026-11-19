# Theme palette provenance

The 24 original presets use the human names, appearance, foreground, background,
and first 16 ANSI colors published with Earendil's article, “There are many themes,
but this one is yours”, by Maximilian Blazek (2026-10-09).

- Article: https://earendil.com/posts/system-theme/
- Demo palette data: https://earendil.com/static/posts/system-theme/themes.json
- Data retrieved: 2026-10-15
- Original data SHA-256: `e69ba4594e0de7a65ec2c41125b78e2aa355ee33db0de91d759f66099d344f8a`

These values identify the named source palettes. The additional computed Pi
semantic tokens in that file are **not** bundled or used. AgentWatcher's role
mapping, WCAG contrast search, and gamut mapping are its own implementation.
The article's fitted polynomial coefficients and perceptual-contrast code are
not included.

Catppuccin Mocha is mentioned in the article but absent from its 24-theme demo.
Its source colors are the official Catppuccin palette's `mocha.colors.base`,
`mocha.colors.text`, and the 16 `mocha.ansiColors` entries in ANSI code order.

- Official source: https://github.com/catppuccin/palette
- Palette data: https://raw.githubusercontent.com/catppuccin/palette/main/palette.json
- License: https://raw.githubusercontent.com/catppuccin/palette/main/LICENSE
- Palette verified: 2026-10-15
- Copyright (c) 2021 Catppuccin; MIT license in `CATPPUCCIN-LICENSE.txt`.

The Catppuccin license also accompanies the Frappé and Latte source colors.
Names credit their respective theme authors; no endorsement is implied.

## Bundled presets

| Name | Appearance |
| --- | --- |
| Rosé Pine | Dark |
| Tokyo Night | Dark |
| Solarized Dark | Dark |
| One Dark | Dark |
| Material Dark | Dark |
| Gruvbox Dark | Dark |
| Dracula | Dark |
| Ghostty Default | Dark |
| Nord | Dark |
| Catppuccin Frappé | Dark |
| Catppuccin Mocha | Dark |
| Palenight | Dark |
| Ocean | Dark |
| Tokyo Night Day | Light |
| Nord Light | Light |
| Catppuccin Latte | Light |
| Ayu Light | Light |
| One Light | Light |
| GitHub Light | Light |
| Monokai Pro Light | Light |
| Rosé Pine Dawn | Light |
| Solarized Light | Light |
| Gruvbox Light | Light |
| Kanagawa Lotus | Light |
| Everforest Light | Light |

## Semantic adaptation

The original canvas RGB value is preserved exactly. Pane, control, selection,
and source-diff backgrounds are modest shifts/tints derived from that canvas.
Primary, secondary, metadata, action, status, and syntax colors are checked on
all nine app backgrounds. Foreground lightness is adjusted in standard OKLab,
with hue retained and chroma reduced only when required by the sRGB gamut.
The solver measures the final quantized RGB values using the WCAG 2 formula,
with normal text at least 4.5:1 and focus indicators at least 3:1. Decorative
pane separators and preview swatches are not normal text or focus indicators.

Color-space reference: https://bottosson.github.io/posts/oklab/
Contrast reference: https://www.w3.org/WAI/WCAG22/Understanding/contrast-minimum.html

The catalog is compile-time data. Semantic palettes use a single immutable
process-wide cache: no runtime files, network calls, parsing, user terminal
queries, or per-frame color solving. “System”, “Light”, and “Dark” retain the
existing AgentWatcher palettes; named presets have a fixed appearance.
