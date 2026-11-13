# <img src="assets/backtracking-icon.svg" width="42" height="42" alt=""> Backtracking

> **[贊助 Backtracking / Support Backtracking on Buy Me a Coffee](https://buymeacoffee.com/kamelkyp/e/596694)**

[繁體中文](#繁體中文) · [English](#english) · [硬體使用量 / Hardware utilization](#硬體使用量--hardware-utilization) · [授權 / License](#授權--license)

## 繁體中文

Backtracking 是給作曲與混音用的回溯錄音取樣器。聽到想留下的聲音時，可從波形存成 WAV 或直接拖進 DAW。

| 版本 | 擷取來源 |
| --- | --- |
| Windows 桌面版 | 系統播放、麥克風、指定 App，可選 A／B 兩路 |
| macOS 桌面版 | 系統播放、麥克風、指定 App，可選 A／B 兩路 |
| VST3／CLAP | DAW 在效果鏈插入點送進插件的音訊 |

### 快速開始

1. 開啟桌面版，或把 **Backtracking** 插入 DAW 音軌的效果鏈。
2. 播放聲音。波形可保留最近 10、20、30 或 60 秒的有效音訊，長靜音期間會停在原處。
3. 按住波形拖出 Backtracking 視窗，放到接受 WAV 的位置，即可匯出整段保留音訊。只需其中一段時，Windows 用 `Ctrl`、macOS 用 `Option` 加拖曳框選，再拖出波形變色的區間。
4. 在完整介面用工具列的 **Play／Pause** 啟用或暫停錄音。紅燈閃爍時正在收錄。操作波形也會暫停錄音，完成後可選 **Play** 或等待已設定的自動續錄。

齒輪旁的即時音量表可顯示 **dBFS、VU 或瞬時 LUFS**。桌面版可點齒輪或按右鍵開啟獨立的 **Options** 視窗。**只顯示波形** 預設關閉，開啟後畫面只保留波形，可從 Options 啟用或暫停錄音，也可切回完整介面。桌面版可拖曳視窗內緣縮成細條。自動續錄可選 **0、5 或 10 秒**，預設 10 秒。選 0 秒會在波形操作完成後立刻恢復待命。清除錄音只會清空記憶體中的擷取內容。

切換至 **只顯示波形** 時，波形會以原本的高度垂直置中。Windows 和 macOS 桌面版的最小尺寸為 **240 × 48**，分別以邏輯像素和點計算。視窗越窄，同一段錄音顯示在越短的距離內，波形在畫面上也就捲動得越慢。無邊框視窗可自由拖曳，靠近螢幕邊緣時不會自動貼齊。可在工具列切換置頂，或從設定選單結束程式。

在齒輪或右鍵選單可用調色盤更改主題色。框選後按 `Esc` 或 `Backspace` 會取消選取並立即恢復錄音待命，這次框選不會產生 WAV。

桌面版會還原上次的視窗位置。Windows 從系統匣控制顯示、隱藏與結束，macOS 從選單列控制。可在設定啟用登入時自動啟動。VST3／CLAP 的設定跟著各自的插件實例存進 DAW 專案，修改後請儲存專案。要保留錄音素材，請先匯出 WAV。

Windows 按 Win 加數字 1 左側的實體鍵，macOS 使用 Fn／Globe 加同一位置的鍵，即可在背景儲存快照。Linux 請使用[快照快捷鍵常駐程式](desktop/linux/README.md)。Options 可指定一個插件實例為全域快捷鍵的目標，也可直接儲存目前錄音器的快照。有選區時儲存選段並立即恢復待命。儲存整段保留音訊時會持續擷取。


在 Options 可選擇 WAV 取樣率 **44.1、48 或 96 kHz**，以及 **16-bit PCM、24-bit PCM 或 32-bit IEEE float** 編碼。預設為 **48 kHz、32-bit float**。

Windows 按 Win 加數字 1 左側的實體鍵，macOS 使用 Fn／Globe 加同一位置的鍵，即可在背景儲存快照。Linux 請使用[快照快捷鍵常駐程式](desktop/linux/README.md)。Options 可指定一個插件實例為全域快捷鍵的目標，也可直接儲存目前錄音器的快照。有選區時儲存選段並立即恢復待命。儲存整段保留音訊時會持續擷取。


[完整操作手冊](docs/USER_GUIDE.md#繁體中文) · [桌面視窗操作](docs/USER_GUIDE.md#移動與縮放桌面視窗) · [安裝與建置](docs/USER_GUIDE.md#安裝與建置--installation-and-build)

## English

Backtracking is a retrospective audio sampler for music production and mixing. When you hear something worth keeping, save it from the waveform as a WAV or drag it straight into your DAW.

| Edition | Capture source |
| --- | --- |
| Windows desktop | System playback, microphone, or a selected app on up to two tracks |
| macOS desktop | System playback, microphone, or a selected app on up to two tracks |
| VST3 / CLAP | Audio the DAW sends to the plug-in at its insert point |

### Quick start

1. Open the desktop app or insert **Backtracking** in a DAW track's effects chain.
2. Play some audio. The waveform can keep the last 10, 20, 30, or 60 seconds of captured sound and holds its position through long gaps.
3. Drag the waveform outside the Backtracking window and drop it on a target that accepts WAV files to export all retained audio. For a shorter clip, hold `Ctrl` on Windows or `Option` on macOS while dragging to select a range, then drag out the highlighted range.
4. In the full interface, use the toolbar's **Play/Pause** control to arm or pause capture. A blinking red indicator means audio is being recorded. Working with the waveform also pauses capture. Choose **Play** when done or let the configured auto-resume timer re-arm capture.

The live meter next to the gear can show **dBFS, VU, or momentary LUFS**. On desktop, click the gear or right-click to open the separate **Options** window. **Show waveform only** defaults to Off. When enabled, only the waveform is visible. Open Options to arm or pause capture, or to restore the full interface. On desktop, drag an inside edge to resize it into a thin strip. Auto-resume offers **0, 5, or 10 seconds**, with 10 seconds as the default. At 0 seconds, capture re-arms as soon as the waveform gesture ends. The clear action empties the audio held in memory.

The waveform keeps its previous height and moves to the vertical center when you switch to **Show waveform only**. The desktop window can shrink to **240 × 48** in Windows logical pixels or macOS points. A narrower window shows the same audio over a shorter distance, so the waveform scrolls more slowly on screen. The frameless desktop windows can be moved without snapping to screen edges. Use **Keep on top** in the toolbar or **Quit** in the settings menu.

Use the color picker in the gear or right-click menu to change the theme accent. Press `Esc` or `Backspace` to cancel a selection and re-arm capture immediately without writing a WAV for that selection.

The desktop app restores its last window position. On Windows, use the system tray to show, hide, or quit the app. On macOS, use the menu bar. You can enable launch at sign-in in Settings. For VST3 and CLAP, save your DAW project after making changes to store each instance's settings. Export a WAV to keep captured audio.

Use Win plus the physical key to the left of 1 on Windows, or Fn/Globe plus that key on macOS, to save from the background. On Linux, use the [snapshot shortcut helper](desktop/linux/README.md). Options can route the shortcut to one plug-in instance and also provides Save snapshot for the current recorder. Saving a selection re-arms capture. Saving the full retained history leaves capture running.


Choose a WAV sample rate of **44.1, 48, or 96 kHz** and **16-bit PCM, 24-bit PCM, or 32-bit IEEE float** encoding in Options. The default is **48 kHz, 32-bit float**.

Flags mark the previous range. Hold `Shift` near either flag while selecting to snap to it. Choose a merged or L/R waveform in Options. The desktop app can capture independent A/B sources and channel routes, then save or drag out two aligned WAVs. The update settings support installation at launch and manual checks. [Capture and waveform](docs/USER_GUIDE.md#sources-waveforms-and-flags) · [Startup updates](docs/UPDATES.md)

[Full user guide](docs/USER_GUIDE.md#english) · [Desktop window controls](docs/USER_GUIDE.md#move-and-resize-the-desktop-window) · [Installation and build](docs/USER_GUIDE.md#安裝與建置--installation-and-build)

## 波形時間軸與 Sender / Waveform timeline and Sender

框選放開後會保持暫停。用滾輪以游標為中心縮放，按住滾輪左右拖曳以平移，拖曳旗標可微調選區。按住 Shift 可吸附附近的可見小節線或旗標。Options 可開關秒數線及小節線，並選擇 1、5 或 10 秒間距。錄音保留時間為 10／20／30／60 秒或 3／5／10 分鐘，縮短時只會裁除超出新保留時間的最舊內容。

A completed selection holds capture paused. Scroll to zoom around the pointer, drag with the middle button to pan, and drag a flag to adjust the selection. Shift snaps to nearby visible bars and flags. Options can show bar lines and a seconds grid at 1, 5, or 10 seconds. The recording history can retain 10, 20, 30, or 60 seconds, or 3, 5, or 10 minutes. Shortening it removes only the oldest frames outside the new duration.

每個插件可個別啟用 Sender。桌面 Options 的 A／B 來源可選擇同一台電腦上的 Sender，也可另外開啟擷取視窗。Audacity 若用 loopback 錄製系統播放，在 Backtracking 選擇 System Audio。若只要擷取 Audacity 的播放輸出，請選擇 Audacity 應用程式來源。

Enable Sender per plug-in instance. Desktop recorders can select a local Sender as either source or open another capture window. To capture the same system playback that Audacity records through loopback, choose System Audio. To capture playback from Audacity itself, choose Audacity as the application source.

## 硬體使用量 / Hardware utilization

測試於 2026-11-12（UTC+8）進行，版本為 Backtracking v0.1.5（`ba19f18`）。環境為 Linux 6.18.57 x86-64，回報的 CPU 為 AMD EPYC 7763，程序可使用 9 個邏輯 CPU。量測涵蓋 Linux CLAP 及共用擷取、波形與 WAV 匯出核心。Windows、macOS、VST3 與實體 GPU 的硬體使用量尚待量測。

Measured on 2026-11-12 (UTC+8) using Backtracking v0.1.5 (`ba19f18`) on Linux 6.18.57 x86-64. The environment reported an AMD EPYC 7763 and 9 logical CPUs available to the process. Measurements cover Linux CLAP and the shared capture, waveform, and WAV export core. Windows, macOS, VST3, and physical GPU utilization remain unmeasured.

### 關閉插件介面 / Plug-in editor closed

小型測試宿主載入 Release 版 CLAP，並固定使用兩個邏輯 CPU。音訊輸入為 48 kHz 立體聲 float32，每 10 ms 送出 480 幀。每種情境以三個獨立程序各量測 8 秒，先暖機，持續錄音的情境也會先填滿保留區間。CPU 是宿主與插件所在程序的用量，以單一邏輯 CPU 為 100%，表列中位數及最小至最大值。RSS 每秒取樣，表列各次量測中位數的中位數。

A minimal host loaded the release CLAP and was pinned to two logical CPUs. Audio input was 48 kHz stereo float32, 480 frames every 10 ms. Three fresh processes ran each scenario for 8 seconds after warm-up, with history prefilled for continuous recording. CPU includes the host and plug-in, with one logical CPU equal to 100%. The table shows the median and observed range. RSS was sampled once per second and summarized as the median of the per-run medians.

| 情境 / Scenario | 保留時間 / History | CPU (%) | RSS (MiB) |
| --- | ---: | ---: | ---: |
| 測試宿主基準 / Host only | — | 0.87 (0.52–1.02) | 1.33 |
| 插件啟用、宿主未送音訊 / Active, no audio callbacks | 20 s | 1.46 (1.33–2.36) | 26.89 |
| 待命、輸入數位靜音 / Armed, digital silence | 20 s | 2.18 (1.72–3.04) | 26.85 |
| 持續錄音 / Continuous recording | 10 s | 1.60 (1.19–2.09) | 26.75 |
| 持續錄音 / Continuous recording | 20 s | 1.73 (1.39–2.75) | 26.71 |
| 持續錄音 / Continuous recording | 30 s | 1.56 (1.22–2.02) | 26.92 |
| 持續錄音 / Continuous recording | 60 s | 2.12 (1.33–2.61) | 26.71 |

CLAP 會依 60 秒上限預先配置錄音環形緩衝區。48 kHz 立體聲 float32 的取樣資料約占 22 MiB，因此各種保留時間的程序 RSS 接近。

CLAP preallocates its recording ring for the 60-second limit. Its 48 kHz stereo float32 samples take about 22 MiB, which accounts for the similar process RSS across history settings.

### 開啟插件介面 / Plug-in editor open

640 × 212 的 Release 版 CLAP 介面以 Xorg 虛擬顯示器執行，使用 Mesa 25.0.7 的 llvmpipe 軟體 OpenGL。測試宿主與 Xorg 固定在同兩個邏輯 CPU。音訊規格同上，先填滿 20 秒的保留區間並使用合併波形。暫停組以介面手動暫停，宿主持續送入音訊。各情境用兩個獨立程序各量測 8 秒。表列宿主與插件所在程序的 CPU 範圍及 RSS，RSS 每秒取樣並採各次量測中位數的中位數。

The 640 × 212 release CLAP editor used Mesa 25.0.7 llvmpipe software OpenGL on an Xorg virtual display. The host and Xorg were pinned to the same two logical CPUs. The input format was unchanged. The host prefilled 20 seconds of history and the editor used the merged waveform view. The paused cases used the editor's Pause control while the host continued sending audio. Two fresh processes ran each scenario for 8 seconds. The table shows the CPU range and RSS for the host process with the plug-in loaded. RSS was sampled once per second and summarized as the median of the per-run medians.

| 介面 / Editor | 狀態 / Capture state | CPU (%) | RSS (MiB) |
| --- | --- | ---: | ---: |
| 完整 / Full | 錄音中 / Recording | 11.58–14.52 | 103.6 |
| 完整 / Full | 暫停 / Paused | 8.74–9.98 | 111.0 |
| 僅波形 / Waveform only | 錄音中 / Recording | 12.88–13.08 | 102.1 |
| 僅波形 / Waveform only | 暫停 / Paused | 6.73–7.14 | 110.7 |

暫停會建立可供框選與匯出的凍結快照，本次暫停情境的 RSS 高出約 7–9 MiB。Xorg 程序另占 0.87–1.50% CPU 與約 37–38 MiB RSS。

Pausing creates a frozen snapshot for selection and export. In this test the paused cases used about 7–9 MiB more RSS. The separate Xorg process used 0.87–1.50% CPU and about 37–38 MiB RSS.

### A／B 擷取與波形核心 / A/B capture and waveform core

Release 版共用核心以單一邏輯 CPU 處理合成 A／B 音訊。輸入已分為兩路 float32 單聲道，每 10 ms 為一個區塊。每組先填滿指定的錄音歷史，並暖機 10 次波形掃描。五個獨立程序各量測 60 秒音訊的批次處理，以及兩路各 384 個區間的 30 次波形掃描。表列程序 CPU 時間的中位數，RSS 在錄音歷史填滿後取樣。範圍包含擷取核心、音量表與波形極值掃描。

The shared release core processed synthetic A/B audio on one logical CPU. The input was already split into two mono float32 lanes in 10 ms blocks. Each run prefilled the selected history and warmed up with 10 waveform scans. Five fresh processes each measured a 60-second batch of audio and 30 waveform scans at 384 bins per lane. The table reports median process CPU time and RSS after prefill. The workload covers the capture core, metering, and waveform peak scans.

| 來源取樣率 / Source rate | 保留時間 / History | 處理每秒音訊 / CPU ms per second of audio | 更新兩路波形 / CPU ms per A/B scan | RSS (MiB) |
| --- | ---: | ---: | ---: | ---: |
| 48 kHz | 20 s | 1.38 | 2.86 | 23.8 |
| 48 kHz | 60 s | 1.20 | 9.35 | 23.8 |
| 96 kHz | 20 s | 3.22 | 9.86 | 46.3 |
| 96 kHz | 60 s | 2.95 | 20.44 | 46.3 |

### WAV 匯出成本 / WAV export cost

Release 版共用匯出核心將 20 秒、48 kHz、立體聲 float32 合成音訊寫到 `/dev/shm`。每種取樣率與編碼各用 5 個獨立程序量測，並固定在同一個邏輯 CPU。表列程序 CPU 時間的中位數，包含取樣率轉換、編碼和寫入。峰值 RSS 取各格式的中位數，再列出同取樣率各編碼的範圍。

The shared release export core wrote 20 seconds of synthetic 48 kHz stereo float32 audio to `/dev/shm`. Five fresh processes ran each rate and encoding combination on the same logical CPU. Median process CPU time covers resampling, encoding, and writing. The peak RSS column spans the median peak RSS for each encoding at that rate.

| 輸出取樣率 / Output rate | 16-bit PCM CPU (ms) | 24-bit PCM CPU (ms) | 32-bit float CPU (ms) | 峰值 RSS / Peak RSS (MiB) |
| --- | ---: | ---: | ---: | ---: |
| 44.1 kHz | 253.5 | 247.1 | 232.4 | 15.29–15.32 |
| 48 kHz | 18.4 | 17.9 | 14.0 | 8.41–8.44 |
| 96 kHz | 501.3 | 482.5 | 450.8 | 23.16–23.18 |

另外以同樣長度的 48 kHz 合成輸入測量 A／B 成對匯出。每次分別輸出一個 Side 立體聲和一個 Mid 單聲道的 32-bit float WAV。每組仍採 5 個獨立程序的中位數。

A separate test used the same input length and source rate to export an A/B pair. Each run wrote a stereo Side file and a mono Mid file as 32-bit float WAVs. Each result is the median of five fresh processes.

| 輸出取樣率 / Output rate | 成對匯出 CPU / Pair CPU (ms) | 峰值 RSS / Peak RSS (MiB) |
| --- | ---: | ---: |
| 44.1 kHz | 516.9 | 11.92 |
| 48 kHz | 35.3 | 8.39 |
| 96 kHz | 945.1 | 15.91 |

## 開發測試 / Development checks

```sh
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

## 授權 / License

本儲存庫中的 Backtracking 原創內容採用 Apache License 2.0。第三方依賴與素材各自適用其授權。 [授權全文](LICENSE) · [Open Sans 字型授權](crates/backtracking-core/assets/OpenSans-LICENSE.txt)

Original Backtracking work in this repository is licensed under the Apache License 2.0. Third-party dependencies and assets retain their respective licenses. [Full license](LICENSE) · [Open Sans font license](crates/backtracking-core/assets/OpenSans-LICENSE.txt)
