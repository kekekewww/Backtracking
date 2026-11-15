# Direction A — Evolved ("Night Session")

The existing kit's identity, tightened: same dark green, mint and pink, but one typeface family,
a stricter grid, a real-app panel instead of the bar-chart illustration, and more air.

Files: `A-poster.html/png` (1080×1350), `A-hero.html/png` (1920×1080), tokens and components in `A.css`.

## Palette

| Token | Hex | Use |
| --- | --- | --- |
| `--a-bg-0` | `#050D0B` | Base background (bottom of the vertical gradient) |
| `--a-bg-1` | `#0A1814` | Top of the vertical gradient |
| `--a-glow` | `#11362B` | Soft radial glow behind the panel |
| `--a-grid` | `rgba(176,251,211,.055)` | 54–60 px hairline grid, masked to fade at the edges |
| `--a-rule` | `#1D332C` | Footer rule |
| `--a-mint-hi` | `#B0FBD3` | Icon gradient start (highlights) |
| `--a-mint` | `#86EFBD` | Second headline line, "now" marker |
| `--a-mint-deep` | `#55DCA7` | Icon gradient end |
| `--a-pink` | `#FF6AA8` | Selection outline, drag trail, ghost border |
| `--a-pink-deep` | `#E2367F` | Selection fill in the panel, WAV file glyph |
| `--a-text` | `#EEF6F2` | Primary text |
| `--a-muted` | `#93ABA1` | Sub-line, labels |
| `--a-dim` | `#58726A` | Ruler, licence line |

The panel keeps the real app's neutral greys (`#1F2225` body, `#373B3F` border); only its selection uses the kit pink.

## Typefaces

- **Geist** (variable, OFL) — wordmark, headline (weight 640, tracking −0.04em, line-height 0.96), sub-line (400).
- **Geist Mono** (variable, OFL) — labels, ruler, editions line, file name on the drag ghost.
- **Open Sans** — only inside the UI mock-up, because the app itself uses it.

Traditional Chinese: Geist has no CJK. Pair with **Noto Sans TC** (OFL) Medium/Bold for headlines and Regular for body. Both are neutral grotesques with similar stroke contrast. Set CJK at about 92 % of the Latin size, with line-height around 1.15 rather than 0.96, and keep Geist Mono for numerals in zh-TW layouts. Workable.

## Layout rules

- Margins: 86 px on 1080-wide formats, 120 px on 1920-wide. Everything is left-aligned to that margin. The only right-aligned elements are the tag line and the licence line.
- Order, top to bottom: lockup (icon 62–64 px + wordmark) → headline → sub-line → time ruler → panel → drag ghost → hairline rule → editions line.
- The headline is always two ideas in two colours: what you heard (text white) and where it goes (mint). Portrait formats break it into four short lines. Landscape formats use two lines.
- The ruler above the panel reads `−20 s … now` (20 s is the default History) and aligns exactly with the panel's waveform area.
- The drag ghost always leaves the panel by the shortest route, with a pink dotted trail and the system arrow cursor. It states the real export defaults: 48 kHz, 32-bit float.
- Glow sits only behind the panel. No gradients on text.

## Motion

Slow and nocturnal. The waveform scrolls in from the right in real time while the grid stays still. A selection then wipes across in pink, and the ghost peels off with a short ease-out (about 300 ms) and drifts to its drop point. Cuts are rare. Elements fade and slide no more than about 24 px, as if the camera were holding steady in a dark studio.

## Copy used

- Headline: **What you just heard, / straight into your DAW.** (the existing kit's line, punctuated as one sentence)
- Sub-line: *Backtracking keeps the last 10 seconds to 10 minutes of audio in memory. Select a range on the waveform and drag it out as a WAV.*
- Tag: *Retrospective audio sampler*
- Editions line: *Windows · macOS · VST3 · CLAP* — *Open source · Apache-2.0*
- In the panel: "Drag and Drop" tooltip, "6.17 s" selection length, "−9.8 dBFS" meter (strings and positions as in the real app).
