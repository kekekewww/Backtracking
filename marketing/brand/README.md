# Backtracking campaign guide — direction C "Rewind"

Binding for every campaign asset (posters, videos, landing page). The visual system is defined by
`../style-frames/C-notes.md`, `../style-frames/C.css`, `../style-frames/C-poster.html`,
`../style-frames/C-hero.html` and the app-panel mock-up builder `../style-frames/shared/bt-panel.js|css`.
Fonts and licences are in `../fonts/` (`fonts.css` has every `@font-face`).

## Ground rules

- Product truth comes from the repository `README.md`, `docs/USER_GUIDE.md`, `docs/UPDATES.md` and
  `desktop/linux/README.md`. If a claim is not there, it does not go in an asset.
- Never invent: user counts, testimonials, ratings, awards, press quotes, prices, "free", "#1",
  comparisons with named competitors, latency/CPU promises, or features.
- No third-party logos (Windows, Apple, Linux/Tux, DAW brands, social networks). Platform and
  format names as plain text are fine. A DAW in a mock-up is a generic, unbranded timeline.
- The app icon (`assets/backtracking-icon.svg`) is used unaltered.
- The background is flat ultramarine. No gradients, no blur, no glow. Depth is hard ink offset shadows.
- Every asset exists in English (`en`) and Traditional Chinese (`zh-TW`, Taiwan usage). Chinese is
  written for Taiwan readers, not translated word for word. Use the terms in the glossary below.
- Chinese mega headlines are set in Noto Sans TC weight 900, square (never horizontally squeezed),
  3–5 characters per line, with their own line breaks and sizes.

## Availability (state it exactly like this)

- **Windows**: a ready-to-run download exists. The latest GitHub release is v0.1.5 and contains the
  desktop app plus VST3 and CLAP for Windows x64.
- **macOS desktop, macOS plug-ins, Linux shortcut helper**: built from source following the user
  guide. There is no packaged download for them. Never show a "Download for macOS" button. Use
  "Build from source" wording and link to the guide.
- Requirements that may be shown: Windows 10/11 x64; macOS 14.2 or later.

## Links

- Repository: https://github.com/kekekewww/Backtracking
- Windows download: https://github.com/kekekewww/Backtracking/releases/latest
- User guide: https://github.com/kekekewww/Backtracking/blob/main/docs/USER_GUIDE.md
- Build instructions: https://github.com/kekekewww/Backtracking/blob/main/docs/USER_GUIDE.md#安裝與建置--installation-and-build
- Support: https://buymeacoffee.com/kamelkyp/e/596694
- Landing page (planned): https://backtracking.kamelkyp.com
- Short URL to print on posters and end cards: `backtracking.kamelkyp.com`

## Master copy

| Slot | English | 繁體中文 |
| --- | --- | --- |
| Mega headline | TAKE IT / BACK. | 剛剛那段 / 拿回來。 |
| Tag | Retrospective audio sampler | 回溯錄音取樣器 |
| Sub-line | Select what you just heard and drag it out as a WAV. Backtracking keeps up to 10 minutes of audio in memory. | 框選剛剛聽到的聲音，直接拖出成 WAV。Backtracking 會在記憶體中保留最近最長 10 分鐘的音訊。 |
| One-liner | It was already listening. | 它一直都在聽。 |
| History scale | 10 s · 20 s · 30 s · 60 s · 3 min · 5 min · 10 min | 10 秒 · 20 秒 · 30 秒 · 60 秒 · 3 分鐘 · 5 分鐘 · 10 分鐘 |
| Editions line | Windows / macOS / VST3 / CLAP | Windows / macOS / VST3 / CLAP |
| Licence line | Open source · Apache-2.0 | 開放原始碼 · Apache-2.0 |
| Primary CTA | Download for Windows | 下載 Windows 版 |
| Secondary CTA | View on GitHub | 前往 GitHub |
| Build CTA | Build from source | 從原始碼建置 |
| Support CTA | Support on Buy Me a Coffee | 在 Buy Me a Coffee 贊助 |

### The three beats (used by posters, videos and the landing page in this order)

| # | English | 繁體中文 |
| --- | --- | --- |
| 1 | **It's already listening.** Backtracking keeps the last 10 seconds to 10 minutes of audio in memory. | **它一直都在聽。** Backtracking 在記憶體中保留最近 10 秒到 10 分鐘的音訊。 |
| 2 | **Select the part you want.** Drag across the waveform. Hold Shift to snap to flags, bars or seconds. | **框選想要的那一段。** 在波形上拖曳框選，按住 Shift 可吸附旗標、小節線或秒數線。 |
| 3 | **Drag it out as a WAV.** Drop it on a DAW track or into a folder. Recording keeps running in the background. | **直接拖出成 WAV。** 放到 DAW 音軌或資料夾，背景會繼續錄音。 |

### Features (names are fixed; one-line descriptions may be shortened, not embellished)

| Feature | English | 繁體中文 |
| --- | --- | --- |
| Sources | Capture system playback, a microphone, or one chosen app. | 擷取系統播放、麥克風或指定 App。 |
| A/B | Record two sources at once and save two aligned WAVs. | 同時擷取 A／B 兩路，匯出兩個起訖對齊的 WAV。 |
| Plug-in | Insert it on any DAW track to capture audio at that point in the chain. | 插入任一 DAW 音軌，擷取效果鏈該位置的音訊。 |
| Snapshot shortcut | Save from the background: Alt + ` on Windows, Fn/Globe + ` on macOS. | 背景儲存快照：Windows 按 Alt + `，macOS 按 Fn／Globe + `。 |
| Waveform only | Shrink the window to a thin waveform strip, with adjustable transparency, kept on top. | 把視窗縮成細長波形條，可調透明度並置頂。 |
| Meter | Live level in dBFS, VU or momentary LUFS. | 即時音量：dBFS、VU 或瞬時 LUFS。 |
| WAV export | 44.1, 48 or 96 kHz; 16-bit, 24-bit PCM or 32-bit float. | 44.1、48 或 96 kHz；16-bit、24-bit PCM 或 32-bit float。 |
| Keep quiet sections | Optionally keep silence instead of compressing it. | 可選擇保留安靜片段，不壓縮靜音。 |
| Sender | A plug-in instance can send its audio to the desktop recorder on the same computer. | 插件可將音訊送到同一台電腦上的桌面版錄音器。 |

### Edition posters

Each edition poster keeps the tag, icon lock-up, History scale and footer of the master, and swaps the
mega headline and sub-line. Headlines below are the starting point. Line breaks may change; meaning may not.

| Edition | Mega headline EN | 繁體中文 | Sub-line EN | 繁體中文 |
| --- | --- | --- | --- | --- |
| Windows desktop | FROM ANY / APP. | 任何 App / 都能留。 | System playback, a microphone, or one chosen app. Select it and drag out a WAV. Download for Windows 10/11 x64. | 系統播放、麥克風或指定 App，框選後直接拖出 WAV。提供 Windows 10／11 x64 下載。 |
| macOS desktop | OFF YOUR / MAC. | Mac 的聲音 / 拿回來。 | System playback, a microphone, or one chosen app, on macOS 14.2 or later. Build from source. | 系統播放、麥克風或指定 App，支援 macOS 14.2 以上。請從原始碼建置。 |
| VST3 / CLAP plug-in | ON ANY / TRACK. | 每一軌 / 都留著。 | Insert it on a track, group or master. It keeps what passed through and hands it back as a WAV. | 插在音軌、群組或主輸出上，留住經過的聲音，再拖出成 WAV。 |
| Linux shortcut helper | ONE KEY. / SAVED. | 一個按鍵 / 存下來。 | A global Super + ` shortcut that tells a Backtracking plug-in instance to save a snapshot. Build from source. | 全域 Super + ` 快捷鍵，讓 Backtracking 插件儲存快照。請從原始碼建置。 |

The Linux item is only a shortcut helper for the plug-in. Do not present it as a Linux recorder app.

## Glossary (zh-TW)

retrospective audio sampler 回溯錄音取樣器 · waveform 波形 · select 框選 · selection 選區 · drag out 拖出 ·
flag 旗標 · snapshot 快照 · system playback 系統播放 · microphone 麥克風 · chosen app 指定 App ·
plug-in 插件 · track 音軌 · history 保留時間 · keep on top 置頂 · show waveform only 只顯示波形 ·
keep quiet sections 保留安靜片段 · open source 開放原始碼 · build from source 從原始碼建置

## Folder ownership (parallel work; stay in your own folder)

- `marketing/brand/`, `marketing/fonts/`, `marketing/style-frames/`: shared and read-only. Copy what
  you need into your own folder instead of editing these.
- `marketing/posters/`: poster agent.
- `marketing/video/`: video agent.
- `marketing/site/`: landing-page agent. This folder must be deployable on its own, so it carries
  its own copies of fonts, CSS, JS and the icon.
