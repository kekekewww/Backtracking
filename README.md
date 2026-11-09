# Backtracking

[繁體中文](#繁體中文) · [English](#english)

## 繁體中文

Backtracking 是給作曲與混音用的回溯錄音取樣器。聽到想留下的聲音時，可從波形存成 WAV 或直接拖進 DAW。

| 版本 | 擷取來源 |
| --- | --- |
| Windows 桌面版 | 預設播放裝置的共享混音 |
| macOS 桌面版 | Core Audio 系統音訊 |
| VST3／CLAP | DAW 在效果鏈插入點送進插件的音訊 |

### 快速開始

1. 開啟桌面版，或把 **Backtracking** 插入 DAW 音軌的效果鏈。
2. 播放聲音。波形會保留最近 10、20 或 30 秒的有效音訊，長靜音期間會停在原處。
3. 未框選時，在波形任意位置點擊可存下整段保留音訊，按住拖曳可把整段 WAV 送入 DAW。若只需其中一段，Windows 用 `Ctrl`、macOS 用 `Option` 加拖曳可框選起訖點，再點擊或拖出以白底標示的區間。
4. 用工具列的 **Play／Pause** 圖示手動啟用或暫停錄音。紅燈閃爍時正在收錄。操作波形也會暫停錄音，完成後可按 **Play** 或等待已設定的自動續錄。

齒輪旁的即時音量表可顯示 **dBFS、VU 或瞬時 LUFS**。點齒輪或按右鍵可開啟相同的設定選單。**只顯示波形** 預設關閉，開啟後只保留波形與 **Play／Pause**，可用右鍵選單切回完整介面。自動續錄的初始設定是 **10 秒**。清除錄音只會清空記憶體中的擷取內容。

Windows 和 macOS 桌面版採用無邊框視窗，可直接在工具列切換置頂，或從設定選單結束程式。

桌面版會在下次開啟時還原偏好設定。VST3／CLAP 的設定跟著各自的插件實例存進 DAW 專案，修改後請儲存專案。要保留錄音素材，請先匯出 WAV。

在設定選單可選擇 WAV 取樣率 **44.1、48 或 96 kHz**，以及 **16-bit PCM、24-bit PCM 或 32-bit IEEE float** 編碼。預設為 **48 kHz、32-bit float**。用 **選擇儲存資料夾／Select save folder** 指定 WAV 的實際存放位置，再用 **前往儲存資料夾／Go to save folder** 開啟。

[完整操作手冊](docs/USER_GUIDE.md#繁體中文) · [桌面視窗操作](docs/USER_GUIDE.md#移動與縮放桌面視窗) · [安裝與建置](docs/USER_GUIDE.md#安裝與建置--installation-and-build)

## English

Backtracking is a retrospective audio sampler for music production and mixing. When you hear something worth keeping, save it from the waveform as a WAV or drag it straight into your DAW.

| Edition | Capture source |
| --- | --- |
| Windows desktop | Shared mix on the default playback device |
| macOS desktop | System audio through Core Audio |
| VST3 / CLAP | Audio the DAW sends to the plug-in at its insert point |

### Quick start

1. Open the desktop app or insert **Backtracking** in a DAW track's effects chain.
2. Play some audio. The waveform keeps the last 10, 20, or 30 seconds of captured sound and holds its position through long gaps.
3. With no selection, click anywhere on the waveform to save all retained audio, or hold and drag to drop the full WAV into your DAW. For a shorter clip, hold `Ctrl` on Windows or `Option` on macOS while dragging to select a range, then click or drag out the range highlighted in white.
4. Use the toolbar's **Play/Pause** control to arm or pause capture. A blinking red indicator means audio is being recorded. Working with the waveform also pauses capture. Press **Play** when done or let the configured auto-resume timer re-arm capture.

The live meter next to the gear can show **dBFS, VU, or momentary LUFS**. Click the gear or right-click to open the same settings menu. **Show waveform only** defaults to Off. When enabled, it leaves the waveform and **Play/Pause** visible. Use the right-click menu to return to the full interface. Auto-resume defaults to **10 seconds**. The clear action empties the audio held in memory.

The Windows and macOS desktop apps use a frameless window. Use **Keep on top** in the toolbar or **Quit** in the settings menu.

The desktop apps restore preferences the next time you open them. For VST3 and CLAP, save your DAW project after making changes to store each instance's settings. Export a WAV to keep captured audio.

Choose a WAV sample rate of **44.1, 48, or 96 kHz** and **16-bit PCM, 24-bit PCM, or 32-bit IEEE float** encoding in the settings menu. The default is **48 kHz, 32-bit float**. Use **Select save folder** to choose the exact destination and **Go to save folder** to open it.

[Full user guide](docs/USER_GUIDE.md#english) · [Desktop window controls](docs/USER_GUIDE.md#move-and-resize-the-desktop-window) · [Installation and build](docs/USER_GUIDE.md#安裝與建置--installation-and-build)

## 開發測試 / Development checks

```sh
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```
