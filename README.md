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
3. 未框選時，在波形任意位置點擊可存下整段保留音訊，按住拖曳可把整段 WAV 送入 DAW。若只需其中一段，Windows 用 `Ctrl`、macOS 用 `Option` 加拖曳可框選起訖點，再點擊或拖出選取區。
4. 用工具列的 **Play／Pause** 圖示手動啟用或暫停錄音。紅燈閃爍時正在收錄。操作波形也會暫停錄音，完成後可按 **Play** 或等待已設定的自動續錄。

點擊工具列的齒輪，可調整保留時間、自動續錄與專案資料夾。Windows 和 macOS 桌面版採用無邊框視窗，可直接在工具列切換置頂，或從齒輪選單結束程式。

匯出為 **48 kHz、32-bit float WAV**。指定專案資料夾後，檔案會存到它的 `output/` 子目錄。

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
3. With no selection, click anywhere on the waveform to save all retained audio, or hold and drag to drop the full WAV into your DAW. For a shorter clip, hold `Ctrl` on Windows or `Option` on macOS while dragging to select a range, then click or drag out that selection.
4. Use the toolbar's **Play/Pause** control to arm or pause capture. A blinking red indicator means audio is being recorded. Working with the waveform also pauses capture. Press **Play** when done or let the configured auto-resume timer re-arm capture.

Click the gear in the toolbar to set the history length, auto-resume delay, and project folder. The Windows and macOS desktop apps use a frameless window. Use **Keep on top** in the toolbar or **Quit** in the gear menu.

Exports are **48 kHz, 32-bit float WAV** files. After you choose a project folder, files go into its `output/` subfolder.

[Full user guide](docs/USER_GUIDE.md#english) · [Desktop window controls](docs/USER_GUIDE.md#move-and-resize-the-desktop-window) · [Installation and build](docs/USER_GUIDE.md#安裝與建置--installation-and-build)

## 開發測試 / Development checks

```sh
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```
