# Backtracking 操作手冊 / User Guide

[繁體中文](#繁體中文) · [English](#english) · [安裝與建置 / Installation and build](#安裝與建置--installation-and-build)

## 繁體中文

### 選擇擷取來源

| 我想留下的聲音 | 使用方式 |
| --- | --- |
| 瀏覽器、播放器或其他程式的聲音 | 開啟桌面版。Windows 使用預設播放裝置的 WASAPI 共享混音，macOS 使用 Core Audio 系統音訊擷取。 |
| 某一軌、群組或主輸出的聲音 | 把 VST3 或 CLAP 插入對應的效果鏈。插件接收該位置的 mono 或 stereo 音訊，並將音訊原樣傳給下游。 |
| 軟體樂器加效果後的聲音 | 把插件放在樂器及要納入取樣的效果後面。若需要乾聲，就放在那些效果前面。 |

Windows 上請把要取樣的程式輸出到預設播放裝置。若 DAW 使用 ASIO、獨占模式或其他直接輸出路徑，使用音軌插件可直接取得該軌訊號。macOS 桌面版首次啟動時，請依系統提示允許錄製系統音訊。 [Windows WASAPI](https://learn.microsoft.com/en-us/windows/win32/coreaudio/loopback-recording) · [Apple Core Audio](https://developer.apple.com/documentation/coreaudio/capturing-system-audio-with-core-audio-taps)

### 開啟設定

點擊工具列的齒輪，開啟 **設定** 選單。選單內可以調整 **保留／History／Buffer**、**自動續錄／Auto-resume** 和 **專案資料夾／Project folder**。Windows 和 macOS 桌面版的 **置頂／Keep on top** 可直接在工具列切換。插件視窗的位置由 DAW 控制。

### 讀懂波形與錄音狀態

- **等待聲音／Waiting for sound**。錄音已待命，聲音超過靜音偵測門檻時便開始收錄。
- **錄音中／Recording**。右側是最新收錄的音訊。當錄音達到保留上限，新的音訊會逐步取代最舊的音訊。
- **已暫停／Paused**。操作波形時會固定當前內容，以便選段和匯出。暫停期間的聲音會略過。

**保留／History／Buffer** 可選 10、20 或 30 秒，預設為 20 秒。這是記憶體中的音訊長度。假設兩段各 5 秒的聲音相隔一分鐘，兩段會接在一起，中間只保留靜音偵測需要的短緩衝。非常小聲的內容可能低於門檻。縮短保留時間會裁掉最早的部分，事後加長只會增加接下來可保留的長度。

### 點擊、框選與拖曳

將游標放在波形上，介面會預覽從游標到最新音訊的區段。若已有框選，操作會使用框選區間。波形的提示只在相關操作時出現。介面會依右、左、下、上的順序尋找可以放下提示的位置，並將提示留在視窗內。

| 動作 | Windows | macOS | 結果 |
| --- | --- | --- | --- |
| 查看可用操作 | 將游標移到波形 | 將游標移到波形 | 提示會列出 **Click** 及 **Ctrl + Click** 或 **Option + Click**。已有選取區或 WAV 可拖曳時，主要提示會顯示 **Drag and Drop**。 |
| 進入框選模式 | 游標在波形上時按住 `Ctrl` | 游標在波形上時按住 `Option` | 提示切換為 **Drag to Select**。 |
| 直接存檔 | 點一下波形 | 點一下波形 | 固定波形並將目前區段存成 WAV。 |
| 匯入 DAW | 在波形上按住滑鼠左鍵並拖出 | 在波形上按住滑鼠左鍵並拖出 | 按住時顯示 **Drag and Drop**。檔案就緒後，拖到 REAPER 時間軸、FL Studio Playlist 或其他接受 WAV 的位置。 |
| 指定起訖點 | 按住 `Ctrl`，在波形上按住左鍵拖曳 | 按住 `Option`，在波形上按住左鍵拖曳 | 顯示 **Drag to Select**。放開後保留框選區間，再放開修飾鍵並點擊或拖出選取區。 |

完成框選後，Backtracking 會先準備選取區的 WAV，以便接著拖出。第一次匯出或需要轉換取樣率時，檔案可能需要短暫準備。拖曳時可先維持按住，等檔案可拖出再放開。若 DAW 沒有接受拖放，可以從 `output/` 直接匯入已存好的 WAV。

### 恢復錄音

按 **繼續錄音／Resume** 可回到錄音待命狀態。已保留的音訊會接續使用，後續收錄仍受保留上限約束。若只想手動控制，將 **自動續錄／Auto-resume** 設成 **手動／Off**。也可設定 10、20 或 30 秒，從取樣器視窗內最後一次點擊或手勢結束時開始計時。拖曳進行中會保持暫停，輸入聲音本身也不會觸發續錄。

### 輸出位置與檔案規格

從齒輪選單選擇 **專案資料夾／Project folder**，指定專案根目錄。例如選擇 `My Song`，檔案會寫到 `My Song/output/`。程式會在需要時建立該子目錄。插件的資料夾由使用者選擇，與 DAW 的專案儲存位置各自設定。

| 版本 | 未指定專案時的輸出位置 | 設定保存 |
| --- | --- | --- |
| macOS 桌面版 | `~/Backtracking/output/` | 記住保留時間、自動續錄與輸出位置。 |
| Windows 桌面版 | `%USERPROFILE%\Backtracking\output\` | 設定適用於本次開啟的桌面程式。 |
| VST3／CLAP | 使用者家目錄下的 `Backtracking/output/` | 每個插件實例各自設定，重新載入時使用預設值。 |

匯出格式固定為 **48 kHz、32-bit IEEE float WAV**。來源若是 44.1 或 96 kHz，會在匯出時轉成 48 kHz。WAV 保留擷取結果的 mono 或 stereo 聲道配置，Windows 桌面版會將多聲道輸出裝置的混音轉為 stereo。回溯歷史儲存在記憶體，關閉桌面程式或移除插件前，請先存下需要的片段。

### 排除問題

| 狀況 | 檢查方式 |
| --- | --- |
| 播放有聲音，但波形沒有更新 | 先看狀態是否暫停。Windows 確認來源使用預設播放裝置。macOS 確認系統音訊錄製權限，再按 **繼續錄音**。插件確認音軌正在播放或監聽，且 Backtracking 的插入點收得到訊號。 |
| Windows 切換喇叭或耳機後無法擷取 | 確認新的裝置已是系統預設播放裝置，再用 **Resume** 重新連線。 |
| DAW 掃描不到插件 | 確認已複製完整 `.vst3` 或 `.clap`，架構與 DAW 相容，且檔案位於 DAW 會掃描的資料夾。依下方步驟重新掃描。 |
| 能存檔，但拖不到 DAW | 等 WAV 完成後再拖，確認落點是可接受音訊檔的時間軸或 Playlist。也可從輸出資料夾將 WAV 拖入或手動匯入。 |
| 無法寫入 WAV | 確認波形有可用的音訊，再選擇可寫入且仍然存在的專案資料夾。 |

**離線渲染**。CLAP 在宿主回報離線模式時會略過擷取。VST3 使用的 nice-plug 0.4.2 有時會回報前一次的處理模式，離線渲染仍可能進入回溯歷史。渲染前若要保留目前的素材，請先匯出。若要在渲染期間維持凍結，先關閉自動續錄，再點擊波形暫停。

## English

### Choose what to capture

| What you want to sample | Which edition to use |
| --- | --- |
| A browser, media player, or another desktop app | Open the desktop recorder. Windows uses WASAPI loopback on the default playback device. macOS uses a Core Audio system tap. |
| One DAW track, a bus, or the master output | Insert the VST3 or CLAP plug-in on the relevant effects chain. It captures mono or stereo audio at that point and passes the input through unchanged. |
| A software instrument with effects | Insert Backtracking after the instrument and any effects you want in the sample. Move it ahead of those effects to capture the dry signal. |

On Windows, route the source to the default playback device. If a DAW uses ASIO, exclusive mode, or another direct output path, use the plug-in to sample its track signal. On macOS, grant system audio recording permission when the desktop app first prompts you. [Windows WASAPI](https://learn.microsoft.com/en-us/windows/win32/coreaudio/loopback-recording) · [Apple Core Audio](https://developer.apple.com/documentation/coreaudio/capturing-system-audio-with-core-audio-taps)

### Open settings

Click the gear in the toolbar to open **Settings**. The menu contains **History / Buffer**, **Auto-resume**, and **Project folder**. In the Windows and macOS desktop apps, you can toggle **Keep on top** directly from the toolbar. The DAW controls the plug-in window.

### Read the waveform and capture status

- **Waiting for sound**. Capture is armed and starts when the incoming signal crosses the silence threshold.
- **Recording**. The newest audio appears on the right. Once the buffer fills, new audio gradually replaces the oldest samples.
- **Paused**. Working with the waveform freezes the current history for selection and export. Incoming audio is skipped during the pause.

**History / Buffer** can hold 10, 20, or 30 seconds and defaults to 20 seconds. This is the amount of audio retained in memory. Two five-second sounds a minute apart are joined with only a little silence at the join. Very quiet material may fall below the capture threshold. Shortening the buffer trims the oldest audio. Lengthening it gives future audio more room.

### Click, select, and drag

Point at the waveform to preview a clip running from the pointer to the newest audio. An explicit selection takes precedence. Gesture hints appear as you interact. The tooltip tries the right, left, below, and above in that order and stays within the interface.

| Action | Windows | macOS | Result |
| --- | --- | --- | --- |
| See the available actions | Hover over the waveform | Hover over the waveform | The hint shows **Click** alongside **Ctrl + Click** or **Option + Click**. If a selection or WAV is ready to drag, the main hint changes to **Drag and Drop**. |
| Enter selection mode | Hold `Ctrl` over the waveform | Hold `Option` over the waveform | The hint changes to **Drag to Select**. |
| Save the clip | Click the waveform | Click the waveform | Capture pauses and the current clip is saved as a WAV. |
| Drop a clip into a DAW | Hold the primary mouse button on the waveform and drag out | Hold the primary mouse button on the waveform and drag out | The hint shows **Drag and Drop**. Once ready, drop the WAV onto the REAPER timeline, FL Studio Playlist, or another WAV drop target. |
| Choose both ends | Hold `Ctrl` and drag across the waveform | Hold `Option` and drag across the waveform | The hint shows **Drag to Select**. Release to keep the selection, then release the modifier and click or drag the highlighted range. |

When you finish selecting a range, Backtracking prepares its WAV so you can drag it next. The first export and clips that need resampling may take a moment to prepare. Keep the mouse button held until the file is ready to drag. If the DAW does not accept the drop, import the saved WAV from `output/`.

### Resume capture

Choose **Resume** to arm capture again. The existing audio stays in the rolling history, and new audio is appended within the selected limit. Leave **Auto-resume** set to **Off** for manual control, or choose 10, 20, or 30 seconds. The timer starts after the last click in the sampler or the end of a gesture. Capture stays paused during a drag, and incoming audio alone cannot resume it.

### Output location and file format

In the gear menu, choose **Project folder** and select the project root. If you select `My Song`, clips go to `My Song/output/`. The application creates the subfolder as needed. Set the plug-in output folder separately from your DAW's project save location.

| Edition | Default output folder | Settings |
| --- | --- | --- |
| macOS desktop | `~/Backtracking/output/` | Remembers history, auto-resume, and the output folder. |
| Windows desktop | `%USERPROFILE%\Backtracking\output\` | Applies to the current app session. |
| VST3 / CLAP | `Backtracking/output/` in your home directory | Each instance has its own settings for the current session. |

Every export is a **48 kHz, 32-bit IEEE float WAV**. A 44.1 or 96 kHz source is resampled during export. The WAV preserves the captured mono or stereo layout. The Windows desktop app mixes multichannel playback to stereo. The rolling history lives in memory, so export any clips you need before closing the app or removing the plug-in.

### Troubleshooting

| Symptom | What to check |
| --- | --- |
| Audio plays but the waveform stays still | Check whether capture is paused. On Windows, verify the source uses the default playback device. On macOS, verify system audio recording permission, then select **Resume**. In a DAW, check playback or monitoring and the signal at the plug-in insert. |
| Capture stops after switching Windows playback devices | Set the new device as the default playback device, then select **Resume** to reconnect. |
| The plug-in does not appear in the DAW | Verify that the entire `.vst3` or `.clap` is in a scanned folder and that its architecture matches the DAW. Rescan using the steps below. |
| A WAV saves but the DAW refuses the drop | Wait for export to finish and drop on an audio timeline or Playlist. You can also drag the file from the output folder or import it manually. |
| The WAV could not be written | Check that the waveform contains audio and choose a project folder that exists and allows writing. |

**Offline rendering**. CLAP skips capture when the host reports offline mode. The nice-plug 0.4.2 VST3 wrapper can pass the previous processing mode to Backtracking, so an offline render may enter the capture history. Export important clips before rendering. To keep capture frozen during the render, turn off auto-resume and click the waveform to pause first.

## 安裝與建置 / Installation and build

取得原始碼後，在專案根目錄執行對應指令。請使用 Rust 1.95 或更新的穩定版。

Run the commands below from the project root. Use Rust 1.95 or a newer stable version.

```sh
git clone https://github.com/kekekewww/Backtracking.git
cd Backtracking
```

### Windows 桌面版 / Windows desktop

需要 Windows 10／11 x64、Visual Studio C++ Build Tools 和 Windows SDK。以下指令適用於 PowerShell。

Requires Windows 10 or 11 x64, Visual Studio C++ Build Tools, and the Windows SDK. Run these commands in PowerShell.

```powershell
cargo build --manifest-path desktop/windows/Cargo.toml --release --locked
.\desktop\windows\target\release\Backtracking.exe
```

### macOS 桌面版 / macOS desktop

需要 macOS 14.2 或更新版本和 Xcode Command Line Tools。從 Finder 開啟建置後的 `.app`，或執行下列 `open` 指令。第一次啟動時允許系統音訊錄製。如果先前拒絕，請到「系統設定 → 隱私權與安全性」調整權限後重試。

Requires macOS 14.2 or later and Xcode Command Line Tools. Open the built `.app` in Finder or use the `open` command below. Grant system audio recording permission on first launch. If you previously declined, check **System Settings → Privacy & Security** and try again. [Apple Core Audio](https://developer.apple.com/documentation/coreaudio/capturing-system-audio-with-core-audio-taps)

```sh
bash desktop/macos/build.sh
open target/bundled/Backtracking.app
```

### VST3／CLAP 插件 / VST3 and CLAP plug-ins

Windows 需要 Visual Studio C++ Build Tools 和 Windows SDK，macOS 需要 Xcode Command Line Tools。建置時請使用與 DAW 相符的架構。

On Windows, use Visual Studio C++ Build Tools and the Windows SDK. On macOS, use Xcode Command Line Tools. Build for the architecture your DAW uses.

**Windows PowerShell**

```powershell
cargo install cargo-nice-plug --locked
$env:CARGO_TARGET_DIR = Join-Path (Get-Location) target
cargo nice-plug bundle backtracking-plugin --release --locked --manifest-path plugins/backtracking-plugin/Cargo.toml
```

**macOS Terminal**

```sh
cargo install cargo-nice-plug --locked
CARGO_TARGET_DIR="$PWD/target" cargo nice-plug bundle backtracking-plugin --release --locked \
  --manifest-path plugins/backtracking-plugin/Cargo.toml
```

將 `target/bundled/` 產出的整個 `.vst3` 或 `.clap` 複製到 DAW 掃描的資料夾。

Copy the complete `.vst3` or `.clap` from `target/bundled/` into a folder scanned by your DAW.

| 系統 / System | VST3 | CLAP |
| --- | --- | --- |
| Windows | `%CommonProgramFiles%\VST3\` | `%CommonProgramFiles%\CLAP\` |
| macOS | `/Library/Audio/Plug-Ins/VST3/` | `/Library/Audio/Plug-Ins/CLAP/` |

macOS 若要安裝到目前使用者的資料夾，也可使用 `~/Library/Audio/Plug-Ins/` 下對應的 `VST3` 或 `CLAP` 子目錄，並確認 DAW 有掃描該路徑。

For a per-user macOS install, use the corresponding `VST3` or `CLAP` subfolder under `~/Library/Audio/Plug-Ins/` and verify that your DAW scans it. [Steinberg VST3 locations](https://steinbergmedia.github.io/vst3_dev_portal/pages/Technical%2BDocumentation/Locations%2BFormat/Plugin%2BLocations.html)

1. **REAPER**。開啟 Preferences，在 Plug-ins 的 **VST** 或 **LV2/CLAP** 頁面確認路徑並重新掃描。開啟目標音軌的 **FX**，搜尋 **Backtracking**。

   Open Preferences. Under Plug-ins, check the path on the **VST** or **LV2/CLAP** page and rescan. Open **FX** on the target track and search for **Backtracking**. [REAPER User Guide](https://dlx.reaper.fm/userguide/html/)
2. **FL Studio**。開啟 **Options → File settings → Manage plugins**，執行 **Find installed plugins** 並確認已啟用 **Verify plugins**。在目標 Mixer insert 的效果欄加入 **Backtracking**。

   Open **Options → File settings → Manage plugins**. Run **Find installed plugins** with **Verify plugins** enabled. Add **Backtracking** to an effect slot on the target Mixer insert. [FL Studio manual](https://www.image-line.com/fl-studio-learning/fl-studio-online-manual/html/basics_externalplugins.htm)

### 裝好後做一次快速測試 / Check the installation

1. 播放兩小段聲音，中間留一段長靜音。確認波形留下兩段有效音訊。

   Play two short sounds with a long pause between them and check that both remain in the waveform.
2. 點擊波形並繼續播放。確認波形保持固定，按 **繼續錄音／Resume** 後才接續收錄。

   Click the waveform and continue playback. Check that the waveform stays frozen until you choose **Resume**.
3. 框選一小段，拖進 DAW 並確認存檔位置及 WAV 規格。若使用插件，分別在 VST3 和 CLAP 實例操作一次。

   Select a short clip, drag it into the DAW, and check the file location and WAV format. For plug-in use, try one instance of each format.

[回到專案首頁 / Back to the project README](../README.md)
