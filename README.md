# <img src="assets/backtracking-icon.svg" width="42" height="42" alt=""> Backtracking

> **[贊助 Backtracking / Support Backtracking on Buy Me a Coffee](https://buymeacoffee.com/kamelkyp/e/596694)**

[繁體中文](#繁體中文) · [English](#english) · [硬體使用量 / Hardware utilization](#硬體使用量--hardware-utilization)

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

在 Backtracking 有焦點時，按 `Ctrl+S` 可把當下快照保存到保存資料夾。已框選時只保存該段，並立即恢復錄音待命。錄音中未框選時，保存整段保留的音訊，錄音會持續進行。成功拖放使用預設輸出資料夾。單擊、框選及留在視窗內的拖曳不會寫檔。兩個資料夾都可在 Options 自訂。

在 Options 可選擇 WAV 取樣率 **44.1、48 或 96 kHz**，以及 **16-bit PCM、24-bit PCM 或 32-bit IEEE float** 編碼。預設為 **48 kHz、32-bit float**。

上次選段的兩端以旗標標記。框選時按住 `Shift`，游標靠近旗標即可吸附。Options 可切換合併或 L／R 波形。桌面版可為 A／B 各指定來源及聲道路由，同次 `Ctrl+S` 或拖放會匯出同起訖的兩個 WAV。**更新**設定提供啟動時自動安裝與手動檢查。 [擷取與波形](docs/USER_GUIDE.md#來源波形與旗標) · [啟動更新](docs/UPDATES.md)

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

With Backtracking focused, press `Ctrl+S` to save the current snapshot to the saved snapshots folder. If a range is selected, it saves that range and immediately re-arms capture. With no selection, it saves all retained audio while active capture continues. Successful drops use the default export folder. Clicking, selecting, and dragging within the window do not write files. Both folders can be set in Options.

Choose a WAV sample rate of **44.1, 48, or 96 kHz** and **16-bit PCM, 24-bit PCM, or 32-bit IEEE float** encoding in Options. The default is **48 kHz, 32-bit float**.

Flags mark the previous range. Hold `Shift` near either flag while selecting to snap to it. Choose a merged or L/R waveform in Options. The desktop app can capture independent A/B sources and channel routes, then save or drag out two aligned WAVs. The update settings support installation at launch and manual checks. [Capture and waveform](docs/USER_GUIDE.md#sources-waveforms-and-flags) · [Startup updates](docs/UPDATES.md)

[Full user guide](docs/USER_GUIDE.md#english) · [Desktop window controls](docs/USER_GUIDE.md#move-and-resize-the-desktop-window) · [Installation and build](docs/USER_GUIDE.md#安裝與建置--installation-and-build)

## 硬體使用量 / Hardware utilization

測試於 2026-11-09 進行。程式版本為 `09f4ee8`，環境是 Linux 6.18.57 x86-64，CPU 顯示為 AMD EPYC 9V74，有 9 個邏輯 CPU 可用。量測範圍為 Linux CLAP 與共用 WAV 匯出核心。Windows、macOS、VST3 及實體 GPU 尚待量測。

Measured on 2026-11-09 at revision `09f4ee8` in a Linux 6.18.57 x86-64 environment reporting an AMD EPYC 9V74 and 9 available logical CPUs. Measurements cover Linux CLAP and the shared WAV export core. Windows, macOS, VST3, and physical GPU measurements are pending.

### 關閉插件介面 / Plug-in editor closed

Release 版 CLAP 由小型測試宿主載入。輸入為 48 kHz 立體聲 float32，每 10 ms 送出 480 幀。各情境在暖機後各量測一次、每次 30 秒，持續錄音情境在量測前填滿緩衝區。CPU 以一個邏輯 CPU 的 100% 為基準，包含測試宿主及插件的執行緒。RSS 是量測結束時的程序常駐記憶體。

A minimal host loaded the release CLAP. The input used 48 kHz stereo float32, with 480 frames per 10 ms. Each scenario was measured once for 30 seconds after warm-up. For continuous recording, the history was filled before measurement. CPU is relative to one logical CPU and includes the host and plug-in threads. RSS is the process resident memory at the end of the interval.

| 情境 / Scenario | 保留時間 / History | CPU (%) | RSS (MiB) |
| --- | ---: | ---: | ---: |
| 測試宿主基準 / Host only | — | 0.18 | 1.26 |
| 插件啟用、宿主未送音訊 / Active, no audio callbacks | 20 s | 0.53 | 15.57 |
| 待命、輸入數位靜音 / Armed, digital silence | 20 s | 0.60 | 15.63 |
| 持續錄音 / Continuous recording | 10 s | 0.66 | 15.46 |
| 持續錄音 / Continuous recording | 20 s | 0.67 | 15.65 |
| 持續錄音 / Continuous recording | 30 s | 0.71 | 15.65 |

量測當時的版本預先配置 30 秒的錄音環形緩衝區，因此表內不同保留時間的常駐記憶體接近。新版可保留 60 秒，48 kHz、立體聲 float32 的緩衝區容量約為 22 MiB。表內的數值為舊版實測。

The measured revision preallocated a 30-second recording ring, which accounts for the similar RSS across history lengths. With 60-second support, the ring capacity is about 22 MiB at 48 kHz stereo float32. The table reports measurements from the earlier revision.

### 開啟插件介面 / Plug-in editor open

使用 640 × 212 的 Release 版 CLAP 介面，在 Xvfb 以 llvmpipe 軟體 OpenGL 繪製。測試宿主與 Xvfb 限用兩個邏輯 CPU。音訊規格同上，保留時間設為 20 秒並預先填滿，自動續錄關閉。各情境量測兩次、每次 12 秒。表中的 CPU 是兩次量測的範圍，RSS 為每秒取樣的中位數，兩者均指測試宿主和插件所在的程序。Xvfb 另占 0.67–0.83% CPU、約 37–38 MiB RSS。

The 640 × 212 release CLAP editor used llvmpipe software OpenGL under Xvfb. The host and Xvfb were pinned to two logical CPUs. The input format was unchanged, with a prefilled 20-second history and auto-resume set to Off. Each scenario ran twice for 12 seconds. CPU is the range across both runs. RSS is the median of one-second samples. Both describe the host process with the plug-in loaded. Xvfb separately used 0.67–0.83% CPU and about 37–38 MiB RSS.

| 介面 / Editor | 狀態 / Capture state | CPU (%) | RSS (MiB) |
| --- | --- | ---: | ---: |
| 完整 / Full | 錄音中 / Recording | 9.41–10.16 | 94.8 |
| 完整 / Full | 暫停 / Paused | 6.25–6.83 | 102.1 |
| 僅波形 / Waveform only | 錄音中 / Recording | 9.50–9.58 | 85.5 |
| 僅波形 / Waveform only | 暫停 / Paused | 6.00–6.50 | 92.8 |

按下暫停時會建立凍結快照，供框選及拖曳使用。這組測試的暫停 RSS 約增加 7.3 MiB。

Pausing creates a frozen snapshot for selection and dragging. In this test, paused RSS was about 7.3 MiB higher.

### WAV 匯出成本 / WAV export cost

使用 Release 版共用匯出核心，將 20 秒、48 kHz、立體聲 float32 合成音訊寫入 RAM 檔案系統。每個格式採 5 次測量的中位數。CPU 時間從呼叫匯出函式量到 WAV 寫完，包含取樣率轉換、編碼和寫入。峰值 RSS 包含測試程式、來源樣本和轉換用記憶體。

The release export core wrote 20 seconds of synthetic 48 kHz stereo float32 audio to a RAM filesystem. Each result is the median of 5 runs. CPU time covers resampling, encoding, and writing the WAV. Peak RSS includes the harness, input samples, and resampling buffers.

| 輸出取樣率 / Output rate | 16-bit PCM CPU (ms) | 24-bit PCM CPU (ms) | 32-bit float CPU (ms) | 峰值 RSS / Peak RSS (MiB) |
| --- | ---: | ---: | ---: | ---: |
| 44.1 kHz | 203.3 | 204.8 | 195.2 | 15.27 |
| 48 kHz | 11.2 | 11.8 | 8.3 | 8.60 |
| 96 kHz | 367.3 | 375.0 | 381.5 | 23.15–23.18 |

## 開發測試 / Development checks

```sh
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```
