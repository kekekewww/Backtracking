# Direction C — Rewind

Loud and graphic, closer to a record sleeve or gig poster than a software page. A flat ultramarine field
carries giant ultra-condensed caps, and the icon's own mint is the only highlight. The real History
options (10 s to 10 min) become a tape-counter scale. A tone-on-tone waveform fills the lower half
as texture.

Files: `C-poster.html/png` (1080×1350), `C-hero.html/png` (1920×1080), tokens and components in `C.css`.

## Palette

| Token | Hex | Use |
| --- | --- | --- |
| `--c-blue` | `#1E32D2` | Background, always flat |
| `--c-blue-2` | `#2A3FE6` | Tone-on-tone background waveform |
| `--c-ink` | `#070C3D` | Hard offset shadows (panel 18 px, slab 10 px at 1×), cursor outline |
| `--c-paper` | `#F4F1E8` | First headline line, text, rules |
| `--c-paper-2` | `rgba(244,241,232,.72)` | Sub-line, scale labels |
| `--c-mint` | `#B0FBD3` | Second headline line, active scale mark, drag slab (= icon gradient start) |
| `--c-mint-2` | `#55DCA7` | Icon gradient end (reserve) |

In the panel: real app chrome. The selection uses `#C21F6C` with a `#FF4F9A` outline, which keeps the app's inverted-magenta logic but is brighter so it reads against blue.

## Typefaces

- **Archivo** (variable, OFL; `wdth` 62–125, `wght` 100–900). One family carries the whole system:
  - Mega headline: `font-stretch: 62%`, weight 900, uppercase, line-height 0.8, 300–330 px.
  - Labels, scale, editions: `font-stretch: 125%`, weight 700, +0.06em tracking. Units and "macOS" keep their real case; only descriptors are uppercase.
  - Sub-line, file name: `font-stretch: 100%`, 400/800.
- **Geist Mono** — only for numeric readouts inside the UI mock-up. **Open Sans** — the UI mock-up's own text.

Traditional Chinese: Archivo has no CJK. Pair with **Noto Sans TC Black (900)** for the mega line and Regular/Medium for body. No condensed CJK exists to match `wdth 62`, so zh-TW mega lines should be short (about 3–5 characters per line) and set square rather than squeezed. Never use synthetic horizontal scaling. Workable, but the zh-TW posters need their own line breaks and will look heavier and more blocky than the English ones.

## Layout rules

- Margins: 64 px (1080 wide) / 96 px (1920 wide). The background is always flat blue. No gradients and no blur.
- The mega headline is the main shape and takes 40–50 % of the frame. Line 1 is paper, line 2 is mint, with a full stop.
- Objects (panel, drag slab) get hard ink offset shadows. Depth comes from layering, never from blur.
- The drag slab is solid mint with three mint motion streaks behind it, which state the direction of travel.
- The History scale is a 2 px paper rule with ticks at all seven real values. The active mark (10 min = the maximum) is mint and taller.
- Footer: 2 px paper rule, editions joined by mint slashes, licence on the right.

## Motion

Punchy and rhythmic, cut to the beat. The scale scrubs backwards from 10 min with a tape-rewind whir. The mega words slam in one per beat on hard cuts. Then the panel drops in on its offset shadow, and the mint slab shoots out sideways with streaks (fast in, about 120 ms, slight overshoot).

## Copy used

- Headline: **TAKE IT / BACK.**
- Sub-line: *Select what you just heard and drag it out as a WAV. Backtracking keeps up to 10 minutes of audio in memory.*
- Tag: *Retrospective audio sampler*
- Scale: 10 s · 20 s · 30 s · 60 s · 3 min · 5 min · 10 min
- Editions: *Windows / macOS / VST3 / CLAP* — *Open source* (poster), *Open source · Apache-2.0* (hero)
- In the panel: "Drag and Drop" tooltip, "7.40 s" selection, "−14.1 LUFS" meter (momentary LUFS mode).
