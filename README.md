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
2. 播放聲音。波形可保留最近 10、20、30、60 秒或 3、5、10 分鐘的有效音訊，長靜音期間會停在原處。
3. 按住波形拖出 Backtracking 視窗，放到接受 WAV 的位置，即可匯出整段保留音訊。只需其中一段時，Windows 用 `Ctrl`、macOS 用 `Option` 加拖曳框選，再拖出波形變色的區間。
4. 在完整介面用工具列的 **Play／Pause** 啟用或暫停錄音。紅燈閃爍時正在收錄。框選放開後會保留選區並維持暫停；取消選取、普通波形點擊或匯出拖曳結束後立即恢復待命。

齒輪旁的即時音量表可顯示 **dBFS、VU 或瞬時 LUFS**。桌面版可點齒輪或按右鍵開啟獨立的 **Options** 視窗。**只顯示波形** 預設關閉，開啟後畫面只保留波形，可從 Options 啟用或暫停錄音，也可切回完整介面。桌面版可拖曳視窗內緣縮成細條。自動續錄可選 **0、5 或 10 秒**，預設 10 秒；選區維持期間暫停，這三種延遲都不計時。取消選取、普通波形點擊或匯出拖曳結束後立即恢復待命。清除錄音只會清空記憶體中的擷取內容。

切換至 **只顯示波形** 時，桌面原生視窗縮到波形面板加可見外框的高度，切回完整模式時恢復原尺寸。背景透明度仍可在 Options 調整，全透明時波形與旗標仍可見、可操作；Windows／macOS 在此模式關閉原生陰影。桌面版最小尺寸為 **240 × 48**，分別以 Windows 邏輯像素和 macOS 點計算。視窗越窄，同一段錄音顯示在越短的距離內，波形在畫面上也就捲動得越慢。無邊框視窗可自由拖曳，靠近螢幕邊緣時不會自動貼齊。可在工具列切換置頂，或從設定選單結束程式。

在齒輪或右鍵選單可用調色盤更改主題色。框選後按 `Esc` 或 `Backspace` 會取消選取並立即恢復錄音待命，這次框選不會產生 WAV。

桌面版會還原上次的視窗位置。Windows 從系統匣控制顯示、隱藏與結束，macOS 從選單列控制。可在設定啟用登入時自動啟動。VST3／CLAP 的設定跟著各自的插件實例存進 DAW 專案，修改後請儲存專案。要保留錄音素材，請先匯出 WAV。

Windows 按 Alt 加數字 1 左側的實體鍵（~／grave 鍵，不必按 Shift），macOS 使用 Fn／Globe 加同一位置的鍵，即可在背景儲存快照。Linux 請使用[快照快捷鍵常駐程式](desktop/linux/README.md)。Options 可指定一個插件實例為全域快捷鍵的目標，也可直接儲存目前錄音器的快照。有選區時儲存選段並立即恢復待命。儲存整段保留音訊時會持續擷取。


在 Options 可選擇 WAV 取樣率 **44.1、48 或 96 kHz**，以及 **16-bit PCM、24-bit PCM 或 32-bit IEEE float** 編碼。預設為 **48 kHz、32-bit float**。

旗標標記前一次範圍，框選時按住 `Shift` 可吸附附近可見的小節線或旗標。Options 可選擇合併或 L/R 波形；桌面版可獨立擷取 A／B 來源及聲道路由，再匯出兩個起訖對齊的 WAV。更新設定支援啟動時安裝與手動檢查。[來源與波形](docs/USER_GUIDE.md#來源波形與旗標) · [啟動更新](docs/UPDATES.md)


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
2. Play some audio. The waveform can keep the last 10, 20, 30, or 60 seconds, or 3, 5, or 10 minutes of captured sound and holds its position through long gaps.
3. Drag the waveform outside the Backtracking window and drop it on a target that accepts WAV files to export all retained audio. For a shorter clip, hold `Ctrl` on Windows or `Option` on macOS while dragging to select a range, then drag out the highlighted range.
4. In the full interface, use the toolbar's **Play/Pause** control to arm or pause capture. A blinking red indicator means audio is being recorded. Releasing a selection keeps it paused; canceling the selection, an ordinary waveform click, or finishing an export drag re-arms capture immediately.

The live meter next to the gear can show **dBFS, VU, or momentary LUFS**. On desktop, click the gear or right-click to open the separate **Options** window. **Show waveform only** defaults to Off. When enabled, only the waveform is visible. Open Options to arm or pause capture, or to restore the full interface. On desktop, drag an inside edge to resize it into a thin strip. Auto-resume offers **0, 5, or 10 seconds**, with 10 seconds as the default. A retained selection holds capture paused and suspends all three delays. Canceling the selection, an ordinary waveform click, or finishing an export drag re-arms capture immediately. The clear action empties the audio held in memory.

Switching to **Show waveform only** shrinks the native desktop window to the waveform panel plus its visible border; returning to the full interface restores its prior size. Background transparency remains adjustable in Options, and waveforms and flags stay visible and interactive when fully transparent. Windows/macOS native shadows are disabled in this mode. The desktop window can shrink to **240 × 48** in Windows logical pixels or macOS points. A narrower window shows the same audio over a shorter distance, so the waveform scrolls more slowly on screen. The frameless desktop windows can be moved without snapping to screen edges. Use **Keep on top** in the toolbar or **Quit** in the settings menu.

Use the color picker in the gear or right-click menu to change the theme accent. Press `Esc` or `Backspace` to cancel a selection and re-arm capture immediately without writing a WAV for that selection.

The desktop app restores its last window position. On Windows, use the system tray to show, hide, or quit the app. On macOS, use the menu bar. You can enable launch at sign-in in Settings. For VST3 and CLAP, save your DAW project after making changes to store each instance's settings. Export a WAV to keep captured audio.

Use Alt plus the physical key to the left of 1 (the ~ / grave key, without Shift) on Windows, or Fn/Globe plus that key on macOS, to save from the background. On Linux, use the [snapshot shortcut helper](desktop/linux/README.md). Options can route the shortcut to one plug-in instance and also provides Save snapshot for the current recorder. Saving a selection re-arms capture. Saving the full retained history leaves capture running.


Choose a WAV sample rate of **44.1, 48, or 96 kHz** and **16-bit PCM, 24-bit PCM, or 32-bit IEEE float** encoding in Options. The default is **48 kHz, 32-bit float**.

Flags mark the previous range. Hold `Shift` near either flag while selecting to snap to it. Choose a merged or L/R waveform in Options. The desktop app can capture independent A/B sources and channel routes, then save or drag out two aligned WAVs. The update settings support installation at launch and manual checks. [Capture and waveform](docs/USER_GUIDE.md#sources-waveforms-and-flags) · [Startup updates](docs/UPDATES.md)

[Full user guide](docs/USER_GUIDE.md#english) · [Desktop window controls](docs/USER_GUIDE.md#move-and-resize-the-desktop-window) · [Installation and build](docs/USER_GUIDE.md#安裝與建置--installation-and-build)

## 波形時間軸與 Sender / Waveform timeline and Sender

框選放開後會保持暫停。用滾輪以游標為中心縮放，按住滾輪左右拖曳以平移，拖曳旗標可微調選區。按住 Shift 可吸附附近的可見小節線或旗標。Options 可開關秒數線及小節線，並選擇 1、5 或 10 秒間距。錄音保留時間為 10／20／30／60 秒或 3／5／10 分鐘，縮短時只會裁除超出新保留時間的最舊內容。

A completed selection holds capture paused. Scroll to zoom around the pointer, drag with the middle button to pan, and drag a flag to adjust the selection. Shift snaps to nearby visible bars and flags. Options can show bar lines and a seconds grid at 1, 5, or 10 seconds. The recording history can retain 10, 20, 30, or 60 seconds, or 3, 5, or 10 minutes. Shortening it removes only the oldest frames outside the new duration.

每個插件可個別啟用 Sender。桌面 Options 的 A／B 來源可選擇同一台電腦上的 Sender，也可另外開啟擷取視窗。Audacity 若用 loopback 錄製系統播放，在 Backtracking 選擇 System Audio。若只要擷取 Audacity 的播放輸出，請選擇 Audacity 應用程式來源。

Enable Sender per plug-in instance. Desktop recorders can select a local Sender as either source or open another capture window. To capture the same system playback that Audacity records through loopback, choose System Audio. To capture playback from Audacity itself, choose Audacity as the application source.

## 硬體使用量 / Hardware utilization

2026-11-14 在 Windows 11、i9-13980HX（32 個邏輯 CPU）實機監測桌面版，涵蓋 7 種保留時間、波形／透明度、dBFS／VU／LUFS、暫停、縮短保留時間及間歇音。完整介面、L/R、Options 可見時，7 組保留時間設定的 CPU 中位數約 **4.6–6.2% 單核心／0.14–0.20% 全機**。600 秒設定下連續擷取超過 10 分鐘後，RSS 中位數約 **427 MiB**、private 約 **401 MiB**；縮短為 180／30 秒後 private 約 **240／193 MiB**。

Measured the Windows desktop app on 2026-11-14 using Windows 11 and an i9-13980HX with 32 logical CPUs. Across seven history settings with full/L/R and Options visible, median CPU was approximately **4.6–6.2% of one logical CPU / 0.14–0.20% of the machine**. After over ten minutes with the 600-second limit selected, median RSS/private were approximately **427/401 MiB**; private memory fell to approximately **240/193 MiB** at 180/30 seconds.

各階段緩衝內容、時長與樣本數不同；外觀階段仍在累積音訊，不能把記憶體變化歸因於單一設定。主監測期間未寫出 WAV；其後另成功保存新快照並檢查為 **97.013 秒、96 kHz、stereo float32**，結構與樣本值檢查通過。原 600 秒保留內容、缺幀、爆音及選區匯出仍未驗證，也未量測 Windows DAW 插件或 macOS。完整數據、限制、去識別逐秒追蹤及先前 Linux 測量見[硬體使用量頁](docs/HARDWARE_USAGE.md)。

Buffer occupancy and phase lengths differ, so these observations do not isolate individual setting costs. No WAV was written during monitoring; a separate later snapshot passed file-structure/sample-value checks at **97.013 seconds, 96 kHz, stereo float32**. The earlier 600-second content, frame continuity, audible artifacts, selected-range export, Windows DAW plug-in performance, and macOS remain unverified. See the [hardware usage page](docs/HARDWARE_USAGE.md) for full results, limits, sanitized scalar traces, and the earlier Linux measurements.

## 開發測試 / Development checks

```sh
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

## 授權 / License

本儲存庫中的 Backtracking 原創內容採用 Apache License 2.0。第三方依賴與素材各自適用其授權。 [授權全文](LICENSE) · [Open Sans 字型授權](crates/backtracking-core/assets/OpenSans-LICENSE.txt)

Original Backtracking work in this repository is licensed under the Apache License 2.0. Third-party dependencies and assets retain their respective licenses. [Full license](LICENSE) · [Open Sans font license](crates/backtracking-core/assets/OpenSans-LICENSE.txt)
