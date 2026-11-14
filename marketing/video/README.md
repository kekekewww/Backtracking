# Backtracking campaign videos (direction C "Rewind")

Coded motion, not screen recordings. Every frame is rendered from an HTML page whose animation is a
pure function of time (`window.seek(t)`), captured frame by frame in Chrome and encoded with ffmpeg.
The overview and the short carry a music soundtrack (see below); the feature loops are silent. Copy comes from `../brand/README.md` (zh-TW headline-style
captions carry no trailing 「。」, per its punctuation rule; body sentences keep it); the look from
`../style-frames/C-*` (copied and adapted here, the shared folders are untouched).

## Files (`out/`)

All H.264 MP4, yuv420p, 30 fps, `+faststart`, tagged BT.709 (primaries, transfer and matrix) with TV
range. Loops also as VP9 WebM with the same tags; loops have no audio stream.

**Soundtrack (overview and short only).** AAC 192 kbit/s, 48 kHz stereo, −14 LUFS, cut to the scene
timings: the overview's music (57.5 s) drops at 5.6 s on the hard cut to the one-liner and fades out over
the last 2.2 s of the end card; the short's (21.0 s) drops at 3.6 s on its one-liner cut and fades out
over the last 1.4 s. Both are cut from "Fun Vibe" by Dyalla (YouTube Audio Library licence, no
attribution required). Source, licence and the script that rebuilds the cuts are in
[`../audio-src/README.md`](../audio-src/README.md). The music may only ship inside the videos: the
render script muxes it from `../audio-src/out/` by stream copy and never writes it as a separate file
here. If that file is missing (e.g. a fresh clone without the licensed audio), the video is written
silent and the script prints a warning.

| File | Size | Duration | What it is |
| --- | --- | --- | --- |
| `overview-en.mp4` | 5.81 MB | 57.5 s | 1920×1080 overview, English, with soundtrack |
| `overview-zh-TW.mp4` | 5.82 MB | 57.5 s | 1920×1080 overview, Traditional Chinese, with soundtrack |
| `short-en.mp4` | 1.72 MB | 21.0 s | 1080×1920 vertical short, English, with soundtrack |
| `short-zh-TW.mp4` | 1.74 MB | 21.0 s | 1080×1920 vertical short, Traditional Chinese, with soundtrack |
| `loop-select.mp4` / `.webm` | 332 KB / 280 KB | 6.0 s | Shift-drag selection, Esc cancels (silent) |
| `loop-drag-out.mp4` / `.webm` | 313 KB / 271 KB | 7.0 s | Drag the selection out onto a DAW track (silent) |
| `loop-ab-sources.mp4` / `.webm` | 407 KB / 335 KB | 6.0 s | One source, then A/B with two rows and meters (silent) |
| `loop-waveform-only.mp4` / `.webm` | 446 KB / 479 KB | 7.0 s | Waveform-only strip, 0–100 % transparency (silent) |
| `loop-snapshot-shortcut.mp4` / `.webm` | 492 KB / 482 KB | 7.0 s | Alt + ` and fn/Globe + ` snapshots (silent) |

Posters: `overview-poster-en.jpg`, `overview-poster-zh-TW.jpg` (1920×1080, from the end card at 52.6 s),
`short-poster-en.jpg`, `short-poster-zh-TW.jpg` (1080×1920, from the end card at 18.6 s).

The loops carry no sentences (app UI, keycaps, icons), so one file serves both languages. The UI
strings inside the app mock-up ("Selection", "Saved …", "Drag and Drop") are the app's own English UI.

## Storyboard

**Overview (16:9, 57.5 s)**

| Time | Beat |
| --- | --- |
| 0.0–5.6 | Wordless problem moment: music scrolls in from the right; the great part (mint) goes by; the cursor hits a record button too late and the counter starts at 0:00 after it. |
| 5.6–10.4 | Hard cut. "IT WAS / ALREADY / LISTENING." (zh: 它一直 / 都在聽) slams in one line per beat while the History scale's needle scrubs back from 10 min, then the 10 min mark lands in mint. The tone-on-tone waveform rewinds behind it. |
| 10.4–33.0 | Layout for all three beats: caption row (number, heading, sentence) across the top, the app panel as the hero across the full frame width, and below it the History scale (10 s lit, until the DAW arrives) over the tone-on-tone waveform band. |
| 10.4–16.0 | Beat 1 caption. The app panel drops in on its ink shadow; live capture, newest audio at the right, record light blinking, meter moving. The mint "moment" arrives in the buffer. |
| 16.0–23.6 | Beat 2 caption. Shift keycap down, press on a painted second line, drag right; the selection end snaps second by second (readout 1.00 → 4.00 s), turns magenta, the view stays fixed ("Selection") while the record light keeps blinking. Shift released, cursor rests on the selection, "Drag and Drop" tooltip. |
| 23.6–33.0 | Beat 3 caption. A generic, unbranded DAW timeline slams in. Press on the selection: the mint WAV slab shoots out (120 ms, overshoot, streaks), is dragged to track 2, a drop preview appears, release: the clip lands with the selected waveform; the panel returns to live view (capture never stopped) and shows "Saved C:\Users\you\Backtracking\output\…wav"; the DAW playhead plays the clip. |
| 33.0–51.2 | Feature run, 2.6 s each, hard cuts; feature name and sentence centred on the left, the visual filling the right column, the waveform band across the lower third: Sources (speaker / mic / app tiles), A/B (two-row panel, A and B meters, two aligned `-A.wav` / `-B.wav` slabs), Plug-in (generic track + FX chain with a Backtracking slot; the plug-in panel starts filling at insert), Snapshot shortcut (Alt + ` keycaps, "Saved snapshot …" status, a 10.00 s WAV), Waveform only (toolbar goes, the strip steps through 0/25/50/75/100 % transparency over another window), Meter (dBFS → VU → LUFS), WAV export (44.1/16-bit, 48/24-bit, 96/32-bit float slabs). |
| 51.2–57.5 | End card: icon lock-up, tag, "TAKE IT / BACK." (zh: 剛剛那段 / 拿回來), "Download from GitHub" (zh: 從 GitHub 下載), `backtracking.kamelkyp.com`, editions line, licence line. |

**Short (9:16, 21 s).** Text stays between y = 250 and y = 1580. 0–3.6 problem moment; 3.6–7.0
one-liner + History scale; 7.0–11.9 "Select the part you want." with the Shift-drag selection;
11.9–16.4 "Drag it out as a WAV." with the drop onto a DAW track; 16.4–21.0 end card. In the beats the
panel spans nearly the full safe width; the Shift keycap and History scale (then the DAW) sit below it
over the waveform band.

**Loops (1280×720).** Each loop's synthetic waveform signal repeats with the loop length, the record light blinks at
1 Hz and the meter updates at 10 Hz, so the last frame flows into the first. Each loop is laid out
compactly and then scaled to fill the frame (a fixed "camera" in `loop.html`, canvases supersampled to
match) so the UI still reads when embedded around 400 px wide.

- `loop-select` (6 s): live → Shift → drag-select (snaps to seconds, view fixed, light still blinking) → Esc cancels → back to live.
- `loop-drag-out` (7 s): held selection → drag the WAV slab onto a DAW track → clip lands, panel goes live, "Saved …" → tracks scroll up one row → a new Shift-selection is made → same frame as the start.
- `loop-ab-sources` (6 s): one source (speaker = A, merged row) → B (microphone) enabled: two rows, A and B meters; the B lane starts empty and fills from the right → back to one source.
- `loop-waveform-only` (7 s): full panel → waveform-only strip → background transparency 0 → 25 → 50 → 75 → 100 % over another window → back.
- `loop-snapshot-shortcut` (7 s): Alt + ` (Windows), then fn/Globe + ` (macOS) while Backtracking stays in the background. Each half starts with an empty status line; once the file is written it reads "Saved snapshot C:\Users\you\Backtracking\saved\…" for Alt + ` and "Saved snapshot /Users/you/Backtracking/saved/…" for fn/Globe + ` (the default saved snapshots folders from `docs/USER_GUIDE.md`), and a WAV slab shoots into a folder. The status line clears where the loop switches machine, mid-loop and at the wrap.

## Re-render

Needs Node 18+, Chrome (or Chromium) and ffmpeg (with libx264 and libvpx-vp9).

```sh
cd marketing/video
npm install                      # puppeteer-core only; it drives the installed Chrome
node src/render.mjs              # everything: 4 language videos, 4 posters, 5 loops × (mp4 + webm)
node src/render.mjs overview     # only jobs whose name starts with "overview" (also: short, loop-select, …)
node src/render.mjs --stills overview-zh-TW 12,30.5   # PNG stills for checking a moment
node src/render.mjs --mux        # re-mux the soundtrack into the rendered overview/short only
```

Environment: `CHROME` (default `C:\Program Files\Google\Chrome\Application\chrome.exe`), `FFMPEG`
(default `ffmpeg` on PATH), `BT_NODE_MODULES` (folder holding `puppeteer-core`, default
`./node_modules`), `BT_SCRATCH` (lossless intermediates and stills, default the OS temp dir).

The script serves the repository root over a local HTTP server (fonts from `../fonts/`, the icon from
`/assets/`), waits for every font to load and fails if Archivo, Open Sans, Geist Mono or (zh-TW)
Noto Sans TC did not load, then seeks and screenshots every frame. A full render takes about
10 minutes.

## Source (`src/`)

- `engine.js` — timing helpers, the deterministic audio/waveform model (filled min/max envelope,
  newest audio at the right, optional exact period for loops), the app panel (copied from
  `style-frames/shared/bt-panel.*` and extended: canvas waveform, blinking record light, live meter,
  A/B rows with A/B meters, status line, waveform-only strip with transparency), cursor, keycaps, WAV
  slab with streaks, generic DAW timeline and clips.
- `scenes.js` — scene builders shared by the overview and the short (problem, one-liner, the three
  beats on the panel, end card).
- `overview.html`, `short.html`, `loop.html` — the timelines. Language via `?lang=en|zh-TW`, loop via
  `?id=…`.
- `strings/en.js`, `strings/zh-TW.js` — every caption, copied from the copy deck. zh-TW feature names
  come from the zh headings in `docs/USER_GUIDE.md` and the glossary. Feature bodies carry a small
  markup (rendered by `SC.rich` in `scenes.js`): `{…}` never breaks across lines, `[X]` is drawn as an
  inline keycap (the backtick key is unreadable as a bare character), `\n` is a deliberate line break.
  Per the copy deck, Chinese never breaks inside a term (瞬時, 指定 App, 起訖, 透明度, 快照, 24-bit, whole
  key combinations).
- `video.css` — C tokens and components (Archivo, Noto Sans TC 900 for zh mega lines, set square).
- `render.mjs` — the one render script.

## Where the mock-up simplifies the real app

- The waveform is a synthetic, seeded signal, not real audio; History is shown at 10 s so a
  4-second selection is large enough to read.
- The "Drag and Drop" tooltip follows the approved style frames and the earlier screen recordings;
  current builds keep instructions in the docs and show "Selection" in the status line (also shown).
- Selection uses Shift (snapped) because the copy deck names Shift; Ctrl (Windows) / Option (macOS)
  free selection is not shown.
- Options, source drop-downs and the right-click menu are not drawn; source changes, A/B and
  waveform-only are shown as their resulting states.
- The DAW is a generic, unbranded timeline; the drop preview and clip placement are simplified.
