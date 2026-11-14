# Campaign music

The overview video, the vertical short and the landing-page demo use one track:

- **"Fun Vibe" by Dyalla**, from the YouTube Audio Library (Dance & Electronic, 2:48, 122 BPM).
- Licence shown in the library: **YouTube Audio Library License**, no attribution required.
- Downloaded 2026-11-14 from the product owner's own YouTube Studio account
  (`youtube.com/audiolibrary`, filter "Attribution not required").

## What the licence allows, as shown in the library

Paraphrased from the licence dialog in YouTube Studio (read there in Traditional Chinese): the track
may be used in all your videos, including monetised ones, with no attribution; the music files may
not be made available, distributed or performed separately from the videos and other content they
have been incorporated into, and standalone distribution of the files is not permitted.

Consequences for this project:

- The audio files are **not committed**. `.gitignore` in this folder excludes the source MP3 and
  everything in `out/`. The landing-page copy of the demo loop is excluded in `marketing/site/` too.
- The videos carry the music as their soundtrack. That is the intended use.
- The landing-page demo plays a 16-bar loop of the track. While the track is the demo's sound,
  **"Save WAV" is turned off**, because saving would hand visitors a piece of the music on its own.
  Without the loop file the demo falls back to its built-in synthesised music and saving works.
- This is a reading of the licence text, not legal advice. Using the track inside an interactive
  page (rather than a video) relies on the "other content" wording.

## Rebuild

Put the source file here as `Dyalla - Fun Vibe.mp3`, then:

```sh
node marketing/audio-src/build-audio.mjs      # set FFMPEG=<path to ffmpeg> if it is not on PATH
```

| Output (`out/`) | Used by | Cut |
| --- | --- | --- |
| `overview-music.m4a` | `marketing/video` overview (57.5 s) | starts 5.6 s before the drop, so the drop lands on the hard cut at 5.6 s; fades out over the end card; -14 LUFS |
| `short-music.m4a` | `marketing/video` short (21 s) | starts 3.6 s before the drop, so the drop lands on the cut at 3.6 s; -14 LUFS |
| `demo-loop.mp3` | `marketing/site` demo | 16 bars from the drop plus 0.5 s either side; loop buffer time 0.5 to 31.97541 s; -16 LUFS |

The five feature loops stay silent (they autoplay muted on the page).

To change the track: replace the source file, re-measure tempo and the drop time, and update the
constants at the top of `build-audio.mjs`.
