# Backtracking

給作曲與混音用的回溯錄音取樣器。桌面版擷取 macOS 系統混音，VST3／CLAP 擷取 DAW 送進插件的音訊。三個版本共用 Rust 錄音與匯出核心。

## 操作

- 保留最近 10、20 或 30 秒的有效音訊。長靜音不消耗歷史長度，下一段聲音接在上一段後面。
- 滑鼠移到波形可預覽範圍。點擊存 WAV，拖曳可把 WAV 交給接受檔案拖放的程式。
- 按住 macOS `Option` 或 Windows `Ctrl` 拖曳可框選，再拖出選取區。
- 操作波形時凍結錄音。按「繼續錄音」，或設定 10、20、30 秒的自動恢復。計時以取樣器視窗內的滑鼠操作為準，手勢進行中暫停計時。
- 匯出格式為 48 kHz、32-bit IEEE float WAV，保留原聲道數。可指定專案目錄，檔案會寫入其 `output/` 子目錄。
- 插件放在效果鏈的錄音位置。插件的輸出原樣直通，擷取範圍由宿主路由決定。

## 建置

需要近期的穩定版 Rust。macOS 桌面版另需 macOS 14.2 或更新版本和 Xcode Command Line Tools。

### macOS 桌面版

從專案根目錄執行下列指令。

```sh
bash desktop/macos/build.sh
open target/bundled/Backtracking.app
```

首次啟動依 macOS 提示授權系統音訊錄製。若授權先前遭拒，可在「系統設定 → 隱私權與安全性」中確認系統音訊錄製權限後重試。

### VST3 和 CLAP

從專案根目錄建置。在 macOS 終端機執行下列指令。

```sh
cargo install cargo-nice-plug --locked
CARGO_TARGET_DIR="$PWD/target" cargo nice-plug bundle backtracking-plugin --release --locked \
  --manifest-path plugins/backtracking-plugin/Cargo.toml
```

在 Windows PowerShell 執行下列指令。

```powershell
cargo install cargo-nice-plug --locked
$env:CARGO_TARGET_DIR = Join-Path (Get-Location) target
cargo nice-plug bundle backtracking-plugin --release --locked --manifest-path plugins/backtracking-plugin/Cargo.toml
```

輸出位於 `target/bundled/`。macOS 可把整個 VST3 bundle 放入 `~/Library/Audio/Plug-Ins/VST3/`，CLAP bundle 放入 `~/Library/Audio/Plug-Ins/CLAP/`，再從 REAPER 設定重新掃描。

### 測試核心

```sh
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

## DAW 驗收

1. 從桌面版播兩段音訊，中間停一分鐘。確認波形保留兩段，跨接點框選後拖進 REAPER。
2. 播放音訊時點擊波形，確認暫停期間畫面和匯出內容固定。分別測試手動恢復及閒置自動恢復。
3. 分別載入 VST3 和 CLAP。關閉編輯器再打開，確認 DAW 仍有提供音訊時會持續收錄。將素材從插件直接拖到同一個 DAW 的時間軸。
4. 用 44.1 或 96 kHz 專案錄音。確認輸出 WAV 為 48 kHz float32，聲道及選取長度正確。驗證 CLAP 在宿主回報離線渲染時略過錄音。
5. 在 Windows／FL Studio 驗證插件拖曳到 Playlist。Windows 桌面版的系統擷取後端仍待實作。

桌面版和 DAW 介面須在各自平台實機驗證。Linux 上的 Rust 測試涵蓋錄音環形緩衝區、靜音與暫停、佇列交接、選段與 WAV 匯出。

VST3 的 nice-plug 0.4.2 wrapper 可能提供前一次的處理模式，離線渲染仍可能寫入歷史。需要保留現有素材時，可以先點擊波形凍結。插件的輸出目錄與時間設定目前為每個實例的工作階段設定，桌面版會保存設定。
