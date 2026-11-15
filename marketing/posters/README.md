# Backtracking posters (direction C "Rewind")

28 PNGs: 5 product-edition posters and 9 social formats, each in English (`en`) and Traditional
Chinese for Taiwan (`zh-TW`). Open `index.html` to see them all with their measured sizes.

## Sizes and where each image goes

| File (`out/Backtracking-<name>-<lang>.png`) | Size (px) | Use |
| --- | --- | --- |
| `edition-master` | 1080 × 1350 | Master poster: "TAKE IT / BACK." Shares, press kit, print |
| `edition-windows` | 1080 × 1350 | Windows desktop app: source choice, A/B rows, Alt + ` |
| `edition-macos` | 1080 × 1350 | macOS desktop app: waveform-only strip, sources, Fn/Globe + ` |
| `edition-plugin` | 1080 × 1350 | VST3 / CLAP plug-in on a generic, unbranded DAW insert chain |
| `edition-linux` | 1080 × 1350 | Linux shortcut helper: Super + ` sends a snapshot request to a plug-in instance |
| `github-social` | 1280 × 640 | GitHub repository social preview (Settings > Social preview) |
| `instagram-feed` | 1080 × 1440 | Instagram feed post, 3:4. Master headline and the three beats |
| `instagram-story` | 1080 × 1920 | Instagram / Facebook story. No text in the top 250 px or bottom 340 px |
| `facebook-cover` | 1640 × 720 | Facebook page cover. Content in the central 1640 × 624, clear of the left 300 px (profile picture) |
| `facebook-feed` | 1080 × 1350 | Facebook feed post, 4:5. Panel, three beats, headline |
| `threads-post` | 1080 × 1080 | Threads post, square |
| `buymeacoffee-cover` | 1600 × 400 | Buy Me a Coffee page cover |
| `youtube-channel` | 2560 × 1440 | YouTube channel banner. All text and the icon sit inside the central 1546 × 423 safe area; outside it is decoration only |
| `youtube-thumbnail` | 1920 × 1080 | YouTube video thumbnail (each file is well under 2 MB) |

## Re-render

Requires Node 18+ and Google Chrome. No network access is used; fonts come from `../fonts/`, the
icon from `../../assets/`.

```sh
node marketing/posters/render.mjs                  # all 28 PNGs + out/gallery-data.js
node marketing/posters/render.mjs edition-linux    # only names containing a filter
node marketing/posters/render.mjs --check          # also reports fonts loaded and fitted headline sizes
```

Set `CHROME=<path>` if Chrome is not in a standard location, `JOBS=<n>` for parallel renders.

## How the sources work

- `src/poster.html?d=<name>&lang=<en|zh-TW>` builds any one poster in a browser.
- `src/copy/en.js`, `src/copy/zh-TW.js`: **all text**, one file per language, taken from
  `../brand/README.md`. `nobreak` lists phrases that must stay on one line when text wraps.
  Edit copy here only.
- `src/layouts.js`: one composition function per design. Mega headlines are fitted after the fonts
  load: English lines share one size; Chinese lines each get their own size, set square in
  Noto Sans TC 900, with Latin words (App, Mac) kept in Archivo.
- `src/system.css`: direction C tokens and components (adapted from `../style-frames/C.css`).
- `src/bt-panel.js|css`: copy of the shared app-panel mock-up builder.
- `src/manifest.js`: names, sizes and uses; read by the page, the render script and the gallery.
