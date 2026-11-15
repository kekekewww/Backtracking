# Backtracking landing page

Static site for `backtracking.kamelkyp.com`, direction C "Rewind" (see `../brand/README.md`).
Plain HTML/CSS/JS: no build step, no framework, no cookies, no analytics, no third-party requests.
This folder is self-contained: publish the folder as it is (Cloudflare Pages: output directory = this folder, no build command).

## Structure

```
index.html               English page
zh-tw/index.html         Traditional Chinese page (lang="zh-Hant-TW"), same CSS/JS
guide/index.html         user guide, English (generated, see "User guide")
zh-tw/guide/index.html   user guide, Traditional Chinese (generated)
css/site.css             all styles, @font-face rules, design tokens
css/guide.css            extra styles for the guide pages
js/demo.js               the in-browser demo (also draws the background waveform texture)
js/capture-worklet.js    AudioWorklet capture tap used by the demo
js/media.js              feature loops, lazy video posters, missing-file fallbacks
js/guide.js              guide contents panel, copy buttons, current-section highlight
fonts/                   Archivo, Geist Mono, Open Sans (Latin woff2, unmodified Google Fonts subsets),
                         NotoSansTC-subset.woff2 (generated, see below), OFL licence texts
img/icon.svg             app icon (unaltered copy of assets/backtracking-icon.svg), favicon
img/apple-touch-icon.png 180x180, rendered from the icon
img/og-en.png            Open Graph / Twitter image, 1200x630, English
img/og-zh-TW.png         Open Graph / Twitter image, 1200x630, Traditional Chinese
favicon.ico              copy of assets/backtracking.ico for browsers that request /favicon.ico
media/                   videos, loops, posters, demo music (copied in by tools/sync-media.py)
.gitignore               keeps the generated media/ folder (copies, plus the licensed demo music) out of git
tools/sync-media.py      copies posters, videos and the demo music in; refreshes the generated page blocks
tools/build-guide.py     generates the two guide pages from docs/USER_GUIDE.md
tools/subset-zh.py       regenerates the Chinese font subset
tools/render-og.cjs      regenerates the OG images and the apple-touch icon
tools/og/                OG image template (og.html) and copies of the style-frame panel builder
```

All page URLs are relative, except the canonical, `hreflang` and Open Graph URLs, which must be absolute
and point at `https://backtracking.kamelkyp.com/`.

## Run locally

```sh
cd marketing/site
python -m http.server 8765
# open http://127.0.0.1:8765/  and  http://127.0.0.1:8765/zh-tw/
```

Serve over http(s). Opening `index.html` as a file works for the page, but browsers refuse to load the
AudioWorklet module from `file://`, so the demo then falls back to a ScriptProcessorNode.

## How the demo works

- Nothing audio-related starts until the visitor presses **Play something** (a user gesture). Until then
  the panel shows a dimmed, pre-drawn waveform and a pulsing prompt.
- **Music** has two modes, decided once, on the first Play:
  - **Licensed loop** (when `media/demo-loop.bin` is deployed): 16 bars of "Fun Vibe" by Dyalla (YouTube
    Audio Library licence, no attribution required; see `../audio-src/README.md`), 122 BPM. The site
    never serves the music as a playable file: `sync-media.py` writes the MP3 as `media/demo-loop.bin`,
    behind an 8-byte marker and a fixed, reversible byte transform (XOR with a key table and the byte
    index), so it has no ID3/MPEG header and players and `ffprobe` do not recognise it as audio. This is
    **not copy protection** (the reverse transform is in `demo.js` for anyone to read); its only purpose
    is that the site does not hand out a music file on its own. The page fetches the `.bin` (URL resolved
    relative to `demo.js`, so it works from `/` and `/zh-tw/`), reverses the transform in memory, decodes
    the MP3 and plays it with an `AudioBufferSourceNode`, `loop = true`, loop points 0.5 s to 31.97541 s (64 beats),
    starting at 0.5 s so the first sound is the downbeat. Before playing, the last 8 ms before the loop end
    are crossfaded into the real audio that precedes the loop start (the file carries 0.5 s of it), so the
    wrap has no sample step. A gain of 1.6 brings its -4.4 dBFS peaks to about -6 dBFS at the capture
    point. The DAW track grid switches to 122 BPM. A small credit line appears under the demo.
    **Save WAV is off** in this mode: the button stays visible but `disabled` + `aria-disabled="true"`,
    with a note ("Saving is off in this demo because the music is licensed. The app saves real WAV
    files."). No file, blob URL or download link of the audio is ever created; dragging the slab off the
    track exports nothing (it never did: the drag is pointer events inside the page, not a file drag).
    **Send to track** and clip playback work as usual, inside the page.
  - **Synthesised fallback** (file absent/404, not decodable, or not arrived within 4 s): `createEngine()`
    synthesises an original 16-bar loop in A minor at 100 BPM with oscillators and a noise buffer (kick,
    snare, hats, bass, chord stabs, a pad breakdown and an arpeggio, with a feedback delay) through a light
    compressor. Save WAV works, the note and the credit stay hidden. The expected 404 shows only as the
    browser's own network line in the console.

  In both modes the music feeds the same capture point at about -6 dBFS peak; the volume/mute control sits
  after the capture point, so muting changes what the visitor hears, not what is captured.
- **Capture**: an AudioWorklet (`capture-worklet.js`) posts 1024-frame stereo blocks to the page; if
  AudioWorklet is unavailable a ScriptProcessorNode is used instead. The page writes them into a 60 s
  stereo ring buffer plus a min/max peak ring (one pair per 128 frames). The History scale (10/20/30/60 s,
  default 20 s) limits how much is retained: shortening trims the oldest audio, lengthening gives future
  audio room. Long silence (over 0.25 s below about -70 dBFS) is not retained, like the app's default
  silence compression, so the waveform stops scrolling after the sound stops ("Waiting for sound").
  Pause stops capture; Clear empties the history.
- **Drawing**: a canvas draws the filled min/max envelope with the newest audio at the right, in columns
  aligned to absolute time so the shape scrolls without shimmering. The meter shows sample peak in dBFS
  (`—` while paused). The REC dot blinks while audio is being written.
- **Selecting** (drag; Shift snaps both ends to whole seconds counted back from the newest sample; or the
  **Select last 4 s** button; or Enter on the focused waveform) copies the retained audio into fixed
  editing material. Capture keeps running into the ring buffer, but the view and the selection stay put.
  Esc, Backspace, a plain click on the waveform or a click outside the demo cancels and returns to live view.
- **Drag out**: pressing inside the selection and dragging shows a mint WAV slab that follows the pointer
  (pointer events, so mouse and touch both work). Dropping it on the generic DAW track places a clip on
  the nearest beat; dropping anywhere else exports nothing. **Send to track** does the same without a drag.
  As in the app, finishing an export returns to live view. With no selection, the whole retained history
  is used.
- **Track**: clips show their waveform; each clip can be played or removed, and **Play track** plays the
  timeline. While clips play, the demo music is ducked.
- **Save WAV** (synthesised music only) downloads the selection (or the whole history) as a stereo 32-bit
  IEEE float WAV at the AudioContext rate (normally 48 kHz; the label under the panel shows the real rate).
- Keyboard: every control is a native button or input; with the waveform focused, Enter selects the last
  4 s, arrows move the selection by 0.5 s, Shift+arrows move its start, Enter again sends it to the track.
- `prefers-reduced-motion`: no pulse, no blinking REC dot, no slab streaks or animations.
- `window.BTDemo.info()` returns a read-only state snapshot used by automated checks (including which
  music mode is active); `BTDemo.levels()` returns 10 ms RMS levels (dBFS) of the retained audio, enough to
  check continuity and far too coarse to rebuild audio from.

All visible demo text comes from the `<script type="application/json" id="bt-demo-strings">` block at the
end of each page, so `demo.js` contains no language strings.

## Chinese font subset

The zh pages use `fonts/NotoSansTC-subset.woff2`: Noto Sans TC (variable) cut down to only the CJK and
full-width characters that appear in `zh-tw/index.html` (including the demo strings and the poster
captions) and in the chrome, contents and headings of `zh-tw/guide/index.html` (the guide's body text uses
the system stack "Noto Sans TC", "PingFang TC", "Microsoft JhengHei"), with the weight axis limited to
400-900. The full 12 MB font is not shipped; the output is byte-for-byte reproducible. `sync-media.py` runs
it for you; otherwise re-run after **any** change to the Chinese text or a Chinese guide heading:

```sh
python -m pip install --user fonttools brotli   # once
python tools/subset-zh.py                        # reads ../fonts/NotoSansTC-Variable.ttf by default
```

The English pages never download the subset (their "中文" label and the few Chinese UI names quoted in
the English guide use system CJK fonts).

## OG images and touch icon

`tools/og/og.html?lang=en|zh` is the template. To re-render (needs Chrome and `playwright-core`):

```sh
npm install playwright-core            # in any scratch folder
NODE_PATH=<scratch>/node_modules node tools/render-og.cjs
```

## Media: posters, videos, loops, demo music

Everything under `media/` is copied in from the campaign folders by one re-runnable script (it skips files
that are unchanged, missing, too small or still being written):

```sh
python tools/sync-media.py      # needs ffmpeg with libwebp ($FFMPEG, PATH, or the WinGet Gyan.FFmpeg folder)
```

| From | To | Used by |
| --- | --- | --- |
| `../posters/out/Backtracking-<name>-<lang>.png` (28) | `media/posters/` (full size, the download) + `media/posters/thumbs/*.webp` (640 px wide) | Posters section |
| `../video/out/overview-<lang>.mp4`, `overview-poster-<lang>.jpg` | `media/` | Watch it, 16:9 player |
| `../video/out/short-<lang>.mp4`, `short-poster-<lang>.jpg` | `media/` | Watch it, 9:16 player |
| `../video/out/loop-<name>.mp4` / `.webm` | `media/loops/` + a still frame `loop-<name>.webp` | feature loops |
| `../audio-src/out/demo-loop.mp3` | `media/demo-loop.bin` (scrambled, see "How the demo works"; licensed, never committed) | demo music |

It then rewrites the blocks between `<!-- posters:start -->` / `<!-- posters:end -->` and
`<!-- watch:start -->` / `<!-- watch:end -->` in both landing pages from what is actually in `media/`
(gallery order and labels from `../posters/src/manifest.js`, pixel sizes read from the PNGs), and runs
`tools/subset-zh.py` when fontTools is available. Do not edit those blocks by hand.

**`media/` and git.** The whole `media/` folder is listed in `.gitignore`. The posters and videos in it
are copies of files the repository already holds under `../posters/out` and `../video/out`, so run
`sync-media.py` after cloning and before previewing or deploying. The licence of "Fun Vibe" does not allow
the music file to be made available on its own, so the site carries it only as the scrambled
`media/demo-loop.bin` (never as an `.mp3`; `sync-media.py` deletes any old `media/demo-loop.mp3`). It is
deployed with the site when the deploy is made from a working copy that has
`../audio-src/out/demo-loop.mp3`, and is never committed.
A deploy from a fresh clone ships without it, and the demo then plays its synthesised fallback (with Save
WAV working).

**Feature loops** (beat 2 = select, beat 3 = drag-out, cards A/B, Snapshot shortcut, Waveform only): each
card holds `<figure class="loop" data-src="media/loops/loop-…">`; `js/media.js` creates
`<video autoplay muted loop playsinline preload="none">` with WebM and MP4 sources only when the figure comes
within 300 px of the viewport, plays it while it is on screen and pauses it off screen. Under
`prefers-reduced-motion: reduce` it shows the still `.webp` instead and never requests the videos. If both
video files fail (or the still, in reduced motion), the figure is removed and the card is a plain text card.
Without JavaScript the figures are hidden.

**Watch it** has the overview (16:9) and the vertical short (9:16) side by side from 1100 px, stacked below.
`<video preload="none">`; the poster image is set by `media.js` only when the player comes near the
viewport. A missing video shows "The video could not be loaded." under its frame when played; a missing short
is left out of the page by `sync-media.py`.

**Posters** (before the open source block): the page's language only, five edition posters first, then
the nine social formats. Each thumbnail is a lazy `<img>` with width/height set, linking to the full PNG in a
new tab, with a caption (name and pixel size) and a download link.

None of this loads with the first screen: on first load the English page requests 9 files (about 300 KB),
the Chinese page 10 files (about 390 KB, of which 95 KB is the font subset); thumbnails, loops and video
posters load as they are scrolled to.

## User guide

`guide/index.html` and `zh-tw/guide/index.html` are generated from `../../docs/USER_GUIDE.md` (no copied
text, no dependencies beyond Python 3.8 and git):

```sh
python tools/build-guide.py
python tools/subset-zh.py       # when a Chinese heading changed
```

- The English page has the "English" part, the zh page the "繁體中文" part; every other section is shared.
  In shared sections a "中文 / English" heading, table cell or link text shows its own side, and where
  paragraphs alternate (Chinese, then English) each page keeps its own. When the Chinese half starts with a
  bold Latin label ("**REAPER**。") the English half gets the same label.
- The built-in Markdown converter handles headings, paragraphs, nested/loose lists, tables, fenced code,
  inline code, bold, italic, links and blockquotes.
- Heading ids follow GitHub's rules, so links into GitHub anchors keep working. Anchors into the other
  language's part go to the other guide page; relative repository links go to
  `https://github.com/kekekewww/Backtracking/blob/main/…`. The script exits non-zero if an anchor or a
  linked repository file is missing.
- The top line reads "Generated from docs/USER_GUIDE.md" with the short hash and date of the file's last
  commit (`git log -1 --format="%h %cs" -- docs/USER_GUIDE.md`) and a "View on GitHub" link. The output
  depends only on the Markdown and that commit, so two runs give identical files.
- Contents: a sticky sidebar from 1100 px, a collapsible panel above the text below that. Tables scroll
  sideways inside their own box; code blocks have a copy button.
