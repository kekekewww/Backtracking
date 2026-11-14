# Direction B — Field Manual

Backtracking presented as a precise studio instrument: bone paper, black ink and one signal colour
taken from the app's own REC dot. The page reads like a page from a gear manual, with a header strip, a
numbered figure, a legend and a spec row. The dark app panel is the only dark object on the page, so the
product stands out.

Files: `B-poster.html/png` (1080×1350), `B-hero.html/png` (1920×1080), tokens and components in `B.css`.

## Palette

| Token | Hex | Use |
| --- | --- | --- |
| `--b-paper` | `#ECE7DB` | Background (flat, no texture) |
| `--b-paper-2` | `#E2DBCB` | Reserved for table fills/cards in later formats |
| `--b-ink` | `#151513` | Headline, rules, file-tag outline and hard shadow |
| `--b-graphite` | `#55514A` | Body text, metadata |
| `--b-faint` | `#8A8476` | Small labels (dt) |
| `--b-rule` | `#C3BAA5` | Cell dividers |
| `--b-red` | `#D93A1E` | **Only** callout markers and the headline full stop |

The panel uses the real app's colours unchanged, including the true selection colour. The app inverts the
mint waveform colour inside a selection, which gives a deep magenta (`#A3195B`). Mint never appears
on the paper. It lives only inside the panel and the icon.

## Typefaces

- **IBM Plex Sans** (variable, OFL) — headline at `font-stretch: 85%` (condensed), weight 600, tracking −0.025em, line-height 0.9. Body and legend text at 100 % width, weight 400.
- **IBM Plex Mono** (400/500/600, OFL) — header cells, labels (uppercase, +0.08–0.1em tracking), spec values, file tag.
- **Open Sans** — only inside the UI mock-up.

Traditional Chinese: the best of the three directions here. **IBM Plex Sans TC** (OFL, part of IBM's Plex
family) is designed to match Plex Sans, so zh-TW and English share the same voice. It has no condensed
width, so zh-TW headlines use the normal width at SemiBold and stay large because they need fewer
characters. Confirm the exact package and weights when the CJK files are fetched; they were not downloaded in this step.

## Layout rules

- Margins: 64 px (1080 wide) / 96 px (1920 wide). Content sits on a visible structure: a header strip with a 3 px ink rule under it, a "Fig. 1" label rule, and a ruled editions row.
- Header strip: lockup (icon 44–46 px + wordmark) | descriptor | reference/URL | licence, with 1 px dividers.
- The headline is always ink, condensed and flush left, and ends with a red full stop. Nothing else is that large.
- The figure is the real panel, shown flat with a soft paper shadow. Numbered red markers (30 px) sit on it, and a legend explains each number. Leader lines are not used because they cross the UI.
- Facts appear as key/value blocks in mono (History values with the 20 s default highlighted, WAV formats).
- The drag ghost is a paper file tag with a 4 px hard ink shadow and the system cursor.
- The editions row has four equal cells: Windows desktop, macOS desktop, VST3/CLAP, Linux helper.

## Motion

Precise and diagrammatic. Rules draw in left to right, then the panel appears and the waveform runs. Each numbered marker
pops in on cue with the matching legend line, while a mono counter ticks the selection length. Moves are quick and
linear (150–200 ms) with hard stops and no bounce, like a technical explainer.

## Copy used

- Headline: **Hit record after the fact.**
- Sub-line: *Backtracking holds the last 10 seconds to 10 minutes of audio in memory. Select the range you want and drag it out as a WAV, onto a DAW timeline or into any folder.*
- Key/values: History 10 s · 20 s · 30 s · 60 s / 3 min · 5 min · 10 min; WAV export 44.1 / 48 / 96 kHz, 16/24-bit PCM · 32-bit float.
- Legend: 1 Select — Ctrl- or Option-drag for a free range. Shift snaps to flags, then bars, then seconds. 2 Drag out — Drop the WAV on a DAW timeline or a folder. Capture keeps running behind you. 3 Meter — Live level in dBFS, VU or momentary LUFS. 4 Keep on top — Pin it above your DAW or shrink it to a waveform strip.
- Editions: Windows desktop / macOS desktop — System playback, a mic or one app; optional A/B. VST3 / CLAP — Captures audio at its insert point in your DAW. Linux — Global snapshot-shortcut helper for the plug-in.
- Footer: github.com/kekekewww/Backtracking · Open source · Apache-2.0
