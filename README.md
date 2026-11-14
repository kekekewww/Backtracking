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
3. 按住波形拖出 Backtracking 視窗，放到接受 WAV 的位置，即可匯出整段保留音訊。只需其中一段時，Windows 用 `Ctrl`、macOS 用 `Option` 自由框選，或直接用 `Shift` 加拖曳吸附框選，再拖出變色區間。
4. 在完整介面用工具列的 **Play／Pause** 啟用或暫停錄音。紅燈閃爍時正在收錄。框選、拖曳旗標、縮放及準備匯出會固定編輯素材與視野，背景繼續擷取；取消選取或匯出結束回到即時視野，不會強制解除手動 Pause；Play 與既有自動續錄另行控制擷取。

齒輪旁的即時音量表可顯示 **dBFS、VU 或瞬時 LUFS**。桌面版可點齒輪或按右鍵開啟獨立的 **Options** 視窗。**只顯示波形** 預設關閉，開啟後畫面只保留波形，可從 Options 啟用或暫停錄音，也可切回完整介面。桌面版可拖曳視窗內緣縮成細條。本輪將背景錄音與固定編輯視野分開：選區不再隱含暫停錄音，手動 Pause 獨立；清除錄音只會清空記憶體中的擷取內容。Options 使用較大粗體標題、至少 15 的主要文字，下拉選單按實際項目高度顯示，最多 10 列後捲動。

切換至 **只顯示波形** 時，桌面原生視窗縮到波形面板加可見外框的高度，切回完整模式時恢復原尺寸。背景透明度仍可在 Options 調整，全透明時波形與旗標仍可見、可操作；Windows／macOS 在此模式關閉原生陰影。桌面版最小尺寸為 **240 × 48**，分別以 Windows 邏輯像素和 macOS 點計算。視窗越窄，同一段錄音顯示在越短的距離內，波形在畫面上也就捲動得越慢。無邊框視窗可自由拖曳，靠近螢幕邊緣時不會自動貼齊。可在工具列切換置頂，或從設定選單結束程式。

在齒輪或右鍵選單可用調色盤更改主題色。框選後按 `Esc` 或 `Backspace` 會取消選取並回到即時視野，不寫 WAV，也不解除手動 Pause。

桌面版會還原上次的視窗位置。Windows 從系統匣控制顯示、隱藏與結束，macOS 從選單列控制。可在設定啟用登入時自動啟動。VST3／CLAP 的設定跟著各自的插件實例存進 DAW 專案，修改後請儲存專案。要保留錄音素材，請先匯出 WAV。

Windows 按 Alt 加數字 1 左側的實體鍵（~／grave 鍵，不必按 Shift），macOS 使用 Fn／Globe 加同一位置的鍵，即可在背景儲存快照。Linux 請使用[快照快捷鍵常駐程式](desktop/linux/README.md)。Options 可指定一個插件實例為全域快捷鍵的目標，也可直接儲存目前錄音器的快照。有選區時保存固定素材中的選段，A／B 起訖對齊；保存不隱含暫停背景擷取，也不解除手動 Pause。


在 Options 可選擇 WAV 取樣率 **44.1、48 或 96 kHz**，以及 **16-bit PCM、24-bit PCM 或 32-bit IEEE float** 編碼。預設為 **48 kHz、32-bit float**。

旗標標記範圍。按住 `Shift` 依可見旗標、小節線、秒數線的順序吸附，拖動端點時排除自己；全透明背景隱藏參考線，但旗標仍可操作。Options 可選擇合併或 L/R 波形；桌面版可獨立擷取 A／B 來源及聲道路由，再匯出兩個起訖對齊的 WAV。更新設定支援啟動時安裝與手動檢查。[來源與波形](docs/USER_GUIDE.md#來源波形與旗標) · [啟動更新](docs/UPDATES.md)


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
3. Drag the waveform outside Backtracking onto a WAV target to export retained audio. For a shorter clip, use `Ctrl` on Windows / `Option` on macOS for free selection, or `Shift`-drag directly for snapped selection, then drag out the highlighted range.
4. In the full interface, use **Play/Pause** to arm or pause capture. A blinking red indicator means audio is being recorded. Selecting, adjusting flags, zooming, or preparing export fixes the editing material/view while background capture continues. Canceling or finishing export returns to live view without forcing manual Pause to resume; Play and existing auto-resume control capture separately.

The live meter next to the gear can show **dBFS, VU, or momentary LUFS**. On desktop, click the gear or right-click to open separate **Options**. **Show waveform only** defaults to Off; use Options to arm/pause or restore the full interface. Drag an inside edge to resize the desktop window into a thin strip. This update separates fixed editing views from background capture: selections no longer implicitly pause recording, and manual Pause is independent. Clear empties captured audio in memory. Options uses larger bold headings, main text of at least 15, and popups sized to their item count with up to ten rows before scrolling.

Switching to **Show waveform only** shrinks the native desktop window to the waveform panel plus its visible border; returning to the full interface restores its prior size. Background transparency remains adjustable in Options, and waveforms and flags stay visible and interactive when fully transparent. Windows/macOS native shadows are disabled in this mode. The desktop window can shrink to **240 × 48** in Windows logical pixels or macOS points. A narrower window shows the same audio over a shorter distance, so the waveform scrolls more slowly on screen. The frameless desktop windows can be moved without snapping to screen edges. Use **Keep on top** in the toolbar or **Quit** in the settings menu.

Use the color picker in the gear or right-click menu to change the theme accent. Press `Esc` or `Backspace` to cancel the selection and return to live view without writing a WAV or clearing a manual Pause.

The desktop app restores its last window position. On Windows, use the system tray to show, hide, or quit the app. On macOS, use the menu bar. You can enable launch at sign-in in Settings. For VST3 and CLAP, save your DAW project after making changes to store each instance's settings. Export a WAV to keep captured audio.

Use Alt plus the physical key to the left of 1 (the ~ / grave key, without Shift) on Windows, or Fn/Globe plus that key on macOS, to save from the background. On Linux, use the [snapshot shortcut helper](desktop/linux/README.md). Options can route the shortcut to one plug-in instance and save the current recorder. A selection saves from its fixed material with aligned A/B boundaries. Saving does not implicitly pause background capture or clear a manual Pause.


Choose a WAV sample rate of **44.1, 48, or 96 kHz** and **16-bit PCM, 24-bit PCM, or 32-bit IEEE float** encoding in Options. The default is **48 kHz, 32-bit float**.

Flags mark a range. Hold `Shift` to snap to visible flags first, then painted bars, then painted seconds; a moving endpoint excludes itself. Fully transparent backgrounds hide guides but keep flags interactive. Choose merged or L/R waveforms in Options. The desktop app can capture independent A/B sources and routes, then save two aligned WAVs. Update settings support installation at launch and manual checks. [Capture and waveform](docs/USER_GUIDE.md#sources-waveforms-and-flags) · [Startup updates](docs/UPDATES.md)

[Full user guide](docs/USER_GUIDE.md#english) · [Desktop window controls](docs/USER_GUIDE.md#move-and-resize-the-desktop-window) · [Installation and build](docs/USER_GUIDE.md#安裝與建置--installation-and-build)

## 波形時間軸與 Sender / Waveform timeline and Sender

框選、旗標與縮放使用固定的編輯素材，背景繼續錄音；即使即時歷史循環覆寫或縮短保留時間，原選區取樣與 A／B 起訖仍保留。滾輪以游標為中心縮放，按住滾輪左右拖曳以平移。Shift 依可見旗標 → 已畫小節線 → 已畫秒數線吸附。波形密度與筆畫隨實際音訊區域、時間跨度及互動調整。Options 可開關參考線並選 1／5／10 秒間距；錄音保留時間為 10／20／30／60 秒或 3／5／10 分鐘，縮短只裁掉即時歷史中最舊內容。

Selection, flags, and zoom use fixed editing material while background capture continues. The original samples and A/B boundaries survive live rollover or a shorter history limit. Scroll around the pointer to zoom and middle-drag to pan. Shift prioritizes visible flags, then painted bars, then painted seconds. Waveform density/strokes adapt to the actual audio area, duration, and interaction. Options toggles guides and a 1/5/10-second grid. History offers 10/20/30/60 seconds or 3/5/10 minutes; shortening trims only the oldest live-history frames.

本輪程式檢查、Windows 指定原生操作及 Release 建置結果已記錄；方法、版本雜湊與未測範圍見[波形編輯與介面回應](docs/RESPONSIVE_EDITING_2026-11-14.md)。 / Code checks, specified Windows native observations, and Release builds are recorded in the [report with build hashes and validation limits](docs/RESPONSIVE_EDITING_2026-11-14.md).

Windows 功能測試版的原生端點編輯可在 Play 後保持約 19.77 秒選區，超過 10 秒即時保留時間後仍保存為 19.768083 秒的 stereo float32 WAV；最終清理文案版另確認 Options 與原生縮窗可操作。 / A Windows functional test build retained an approximately 19.77-second selection beyond ten-second live rollover and saved a 19.768083-second stereo float32 WAV; the final copy-cleanup build separately passed the observed Options/native-resize checks.

操作說明放在文件，UI 保持控制與必要狀態。 / Instructions live in documentation; the UI keeps controls and necessary state.

每個插件可個別啟用 Sender。桌面 Options 的 A／B 來源可選擇同一台電腦上的 Sender，也可另外開啟擷取視窗。Audacity 若用 loopback 錄製系統播放，在 Backtracking 選擇 System Audio。若只要擷取 Audacity 的播放輸出，請選擇 Audacity 應用程式來源。

Enable Sender per plug-in instance. Desktop recorders can select a local Sender as either source or open another capture window. To capture the same system playback that Audacity records through loopback, choose System Audio. To capture playback from Audacity itself, choose Audacity as the application source.

## 硬體使用量 / Hardware utilization

2026-11-14 在 Windows 11、i9-13980HX（32 個邏輯 CPU）實機監測桌面版，涵蓋 7 種保留時間、波形／透明度、dBFS／VU／LUFS、暫停、縮短保留時間及間歇音。完整介面、L/R、Options 可見時，7 組保留時間設定的 CPU 中位數約 **4.6–6.2% 單核心／0.14–0.20% 全機**。600 秒設定下連續擷取超過 10 分鐘後，RSS 中位數約 **427 MiB**、private 約 **401 MiB**；縮短為 180／30 秒後 private 約 **240／193 MiB**。

Measured the Windows desktop app on 2026-11-14 using Windows 11 and an i9-13980HX with 32 logical CPUs. Across seven history settings with full/L/R and Options visible, median CPU was approximately **4.6–6.2% of one logical CPU / 0.14–0.20% of the machine**. After over ten minutes with the 600-second limit selected, median RSS/private were approximately **427/401 MiB**; private memory fell to approximately **240/193 MiB** at 180/30 seconds.

各階段緩衝內容、時長與樣本數不同；外觀階段仍在累積音訊，不能把記憶體變化歸因於單一設定。主監測期間未寫出 WAV；其後另保存並檢查 **97.013 秒、96 kHz、stereo float32** 新快照。該次資料未驗證原 600 秒內容、缺幀、爆音或選區匯出，也未量測 Windows DAW 插件或 macOS；新版選段操作另見上方報告。完整數據、限制、去識別追蹤及 Linux 歷史測量見[硬體使用量頁](docs/HARDWARE_USAGE.md)。

Buffer occupancy and phase lengths differ, so these observations do not isolate setting costs. No WAV was written during monitoring; a separate later snapshot passed checks at **97.013 seconds, 96 kHz, stereo float32**. That dataset did not verify the earlier 600-second content, frame continuity, artifacts, selected-range export, Windows DAW plug-ins, or macOS; newer selection observations are reported above. See the [hardware usage page](docs/HARDWARE_USAGE.md) for full data, limits, traces, and historical Linux measurements.

## 開發測試 / Development checks

```sh
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

## 授權 / License

本儲存庫中的 Backtracking 原創內容採用 Apache License 2.0。第三方依賴與素材各自適用其授權。 [授權全文](LICENSE) · [Open Sans 字型授權](crates/backtracking-core/assets/OpenSans-LICENSE.txt)

Original Backtracking work in this repository is licensed under the Apache License 2.0. Third-party dependencies and assets retain their respective licenses. [Full license](LICENSE) · [Open Sans font license](crates/backtracking-core/assets/OpenSans-LICENSE.txt)
