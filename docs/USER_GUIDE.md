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

點擊工具列的齒輪，或在 Backtracking 視窗內按滑鼠右鍵，可開啟同一個 **設定** 選單。可調整保留時間、自動續錄、背景透明度、WAV 匯出格式及存檔資料夾。Windows 和 macOS 桌面版的 **置頂／Keep on top** 可直接在工具列切換。插件視窗的位置由 DAW 控制。

macOS 桌面版可使用滑鼠右鍵或觸控板的次要點擊開啟選單。`Control` 搭配波形上的點擊與拖曳用於框選。

設定選單的背景透明度可選 0%、25%、50%、75% 或 100%。0% 為不透明，100% 為透明。桌面版在 100% 時仍可開啟設定選單調整透明度。VST3／CLAP 的 **Panel transparency** 調整插件內的波形面板背景，實際看到的底色取決於 DAW 的視窗背景。

### 只顯示波形

開啟 **只顯示波形／Show waveform only** 後，Backtracking 會隱藏自身的外框與深色底板，留下波形和其左上角的 **Play／Pause**。桌面版的背景會變透明。VST3／CLAP 內嵌在 DAW 時，仍可能看到宿主視窗的外框或不透明底色。要回到完整介面，在視窗內按右鍵開啟設定，再取消勾選。這個選項預設關閉，並跟著桌面偏好設定或 DAW 專案中的插件設定儲存。

### 移動與縮放桌面視窗

Windows 和 macOS 桌面版採用無邊框視窗。用下列手勢移動或調整大小。要關閉桌面程式，請從設定選單選擇 **結束／Quit**。

| 動作 | Windows 桌面版 | macOS 桌面版 |
| --- | --- | --- |
| 移動視窗 | 在內容區按住 `Alt`，用滑鼠左鍵拖曳。 | 在波形外的內容區按住 `Option`，用左鍵拖曳。在內容區任意位置可按住 `Command` + `Option` 再用左鍵拖曳。 |
| 調整視窗大小 | 將游標移到視窗內容區的內緣或內角，按住左鍵拖曳。 | 將游標移到視窗內容區的內緣或內角，按住左鍵拖曳。 |
| 框選音訊 | 在波形上按住 `Ctrl` 並用左鍵拖曳。 | 在波形上按住 `Option` 或 `Control` 並用左鍵拖曳。 |

### 讀懂波形與錄音狀態

工具列的 **Play／Pause** 圖示顯示可執行的動作。將游標移到按鈕上，可看到 **開始錄音／Start capture** 或 **暫停錄音／Pause capture**。將游標移到狀態燈上，可查看目前狀態。

- **等待聲音／Waiting for sound**。錄音已待命，聲音超過靜音偵測門檻時便開始收錄。
- **錄音中／Recording**。紅色狀態燈閃爍。波形右側是最新收錄的音訊，達到保留上限後，新的音訊會逐步取代最舊的音訊。
- **已暫停／Paused**。按下 **Pause** 或操作波形會暫停錄音，讓波形固定供選段和匯出。暫停期間的聲音會略過。
- **未連線／Capture unavailable**。桌面版無法擷取時，可用 **Play** 重新連線。

**保留／History／Buffer** 可選 10、20 或 30 秒，預設為 20 秒。這是記憶體中的音訊長度。假設兩段各 5 秒的聲音相隔一分鐘，兩段會接在一起，中間只保留靜音偵測需要的短緩衝。非常小聲的內容可能低於門檻。縮短保留時間會裁掉最早的部分，事後加長只會增加接下來可保留的長度。

### 即時音量表

齒輪旁的音量表顯示目前輸入的音訊，預設模式是 dBFS。從齒輪選單的音量表設定切換單位。macOS 桌面版也可直接點擊讀值切換。暫停錄音或目前無可用讀值時，顯示 `—`。

| 模式 | 讀值 |
| --- | --- |
| **dBFS** | 數位取樣峰值，0 dBFS 是數位滿刻度。 |
| **VU** | 以 300 毫秒 RMS 估算平均電平。以 1 kHz、峰值 −18 dBFS 的正弦波校準為 0 VU。 |
| **LUFS** | 瞬時響度。以 K 加權和 400 毫秒滑動視窗估算。 |

響度量測參考文件。 [ITU-R BS.1770](https://www.itu.int/rec/R-REC-BS.1770-5-202311-I/en) · [EBU Tech 3341](https://tech.ebu.ch/publications/tech3341)

### 點擊、框選與拖曳

在完整介面中，未框選時將游標移到波形上會預覽整段保留音訊。從波形任意位置點擊或開始拖曳，都會匯出這整段音訊。例如保留時間設為 20 秒，錄滿時匯出 20 秒，目前只收錄 7 秒時就匯出 7 秒。完整介面和只顯示波形模式都以波形線條的反相配色標示 `Ctrl` 或 `Option` 框選區間。匯出時使用該區間。

完整介面會在相關操作時顯示波形提示。介面會依右、左、下、上的順序尋找可以放下提示的位置，並將提示留在視窗內。

| 動作 | Windows | macOS | 結果 |
| --- | --- | --- | --- |
| 查看可用操作 | 將游標移到波形 | 將游標移到波形 | 提示會列出 **Click** 及 **Ctrl + Click** 或 **Option + Click**。已有選取區或 WAV 可拖曳時，主要提示會顯示 **Drag and Drop**。 |
| 進入框選模式 | 游標在波形上時按住 `Ctrl` | 游標在波形上時按住 `Option` | 提示切換為 **Drag to Select**。 |
| 直接存檔 | 點一下波形 | 點一下波形 | 固定波形並存成 WAV。未框選時存下整段保留音訊。 |
| 匯入 DAW | 在波形上按住滑鼠左鍵並拖出 | 在波形上按住滑鼠左鍵並拖出 | 按住時顯示 **Drag and Drop**。檔案就緒後，拖到 REAPER 時間軸、FL Studio Playlist 或其他接受 WAV 的位置。 |
| 指定起訖點 | 按住 `Ctrl`，在波形上按住左鍵拖曳 | 按住 `Option`，在波形上按住左鍵拖曳 | 顯示 **Drag to Select**。放開後保留框選區間，再放開修飾鍵並點擊或拖出選取區。 |

完成框選後，Backtracking 會先準備選取區的 WAV，以便接著拖出。第一次匯出或需要轉換取樣率時，檔案可能需要短暫準備。拖曳時可先維持按住，等檔案可拖出再放開。若 DAW 沒有接受拖放，可開啟存檔資料夾並直接匯入 WAV。

### 啟用、暫停與自動續錄

按 **Play** 進入錄音待命，按 **Pause** 手動暫停。重新待命後，下一段收錄的音訊會接在原有歷史後方，受保留時間上限約束。

**自動續錄／Auto-resume** 初始設定為 **10 秒**。可在齒輪選單改為 20、30 秒，或選擇 **手動／Off**，只用按鈕控制續錄。手動暫停或操作波形暫停後，會從取樣器視窗內最後一次點擊或手勢結束時開始計時。拖曳進行中會保持暫停。系統會依設定時間恢復待命，音量門檻仍決定何時實際收錄。

### 清除目前錄音

點擊工具列的垃圾桶圖示。提示文字依版本為 **清除目前錄音**、**Clear capture** 或 **Clear captured audio**。這會清空目前的波形、選取區間與記憶體中的錄音。若錄音已待命，接下來收到的聲音會重新填入波形。先前匯出的 WAV 可從存檔資料夾取用。

### 輸出位置與檔案規格

從設定選單使用 **選擇儲存資料夾…／Select save folder**，指定 WAV 的實際目的地。例如選擇 `My Song/Samples`，檔案就會直接存入該資料夾。先前已儲存的目的地會繼續沿用。

選擇 **前往儲存資料夾／Go to save folder** 可用系統預設的檔案管理程式開啟目前的目的地。資料夾尚未存在時，Backtracking 會先建立再開啟。

| 版本 | 預設儲存資料夾 |
| --- | --- |
| macOS 桌面版 | `~/Backtracking/output/` |
| Windows 桌面版 | `%USERPROFILE%\Backtracking\output\` |
| VST3／CLAP | 使用者家目錄下的 `Backtracking/output/` |

取樣率和編碼可在齒輪選單獨立調整。

| 版本 | 取樣率選項的位置 | 編碼選項的位置 |
| --- | --- | --- |
| Windows 桌面版 | **WAV rate** | **WAV encoding** |
| macOS 桌面版 | **WAV 匯出格式 → 取樣率** | **WAV 匯出格式 → 編碼** |
| VST3／CLAP | **WAV sample rate** | **WAV sample format** |

| WAV 設定 | 選項 | 預設 |
| --- | --- | --- |
| 取樣率 | 44.1 kHz、48 kHz、96 kHz | 48 kHz |
| 編碼 | 16-bit PCM、24-bit PCM、32-bit IEEE float | 32-bit float |

點擊存檔、框選後匯出及拖曳到 DAW 都使用目前選定的 WAV 規格。錄音與音量表以來源取樣率運作，取樣率轉換在匯出時進行。WAV 保留擷取結果的 mono 或 stereo 聲道配置，Windows 桌面版會將多聲道輸出裝置的混音轉為 stereo。回溯歷史儲存在記憶體，關閉桌面程式或移除插件前，請先存下需要的片段。

### 保存與還原設定

Windows 和 macOS 桌面版在變更後會自動保存保留時間、自動續錄、背景透明度、音量表模式、WAV 取樣率與編碼、存檔資料夾、置頂及只顯示波形的設定，下次開啟時還原。

- **Windows**。設定儲存在 `%LOCALAPPDATA%\Backtracking\Settings`。若找不到 `LOCALAPPDATA`，則使用 `%APPDATA%\Backtracking\Settings`。程式會以預設視窗尺寸開啟，由系統安排位置。
- **macOS**。設定儲存在系統的使用者偏好中，也會記住桌面視窗的位置與尺寸。

**VST3／CLAP** 的保留時間、自動續錄、面板透明度、音量表模式、WAV 取樣率與編碼、存檔資料夾及只顯示波形的設定由 DAW 專案保存，每個插件實例可獨立設定。修改後請手動儲存專案。重新開啟同一個專案時，DAW 會還原各實例的設定。

### 排除問題

| 狀況 | 檢查方式 |
| --- | --- |
| 播放有聲音，但波形沒有更新 | 將游標移到狀態燈確認狀態。若已暫停，按 **Play**。Windows 確認來源使用預設播放裝置。macOS 確認系統音訊錄製權限。插件確認音軌正在播放或監聽，且 Backtracking 的插入點收得到訊號。 |
| Windows 切換喇叭或耳機後無法擷取 | 確認新的裝置已是系統預設播放裝置，再按 **Play** 重新連線。 |
| DAW 掃描不到插件 | 確認已複製完整 `.vst3` 或 `.clap`，架構與 DAW 相容，且檔案位於 DAW 會掃描的資料夾。依下方步驟重新掃描。 |
| 能存檔，但拖不到 DAW | 等 WAV 完成後再拖，確認落點是可接受音訊檔的時間軸或 Playlist。也可從輸出資料夾將 WAV 拖入或手動匯入。 |
| 無法寫入 WAV | 確認波形有可用的音訊，再選擇可寫入的存檔資料夾。 |

**離線渲染**。CLAP 在宿主回報離線模式時會略過擷取。VST3 使用的 nice-plug 0.4.2 有時會回報前一次的處理模式，離線渲染仍可能進入回溯歷史。渲染前若要保留目前的素材，請先匯出。若要在渲染期間維持凍結，先關閉自動續錄，再按 **Pause**。

## English

### Choose what to capture

| What you want to sample | Which edition to use |
| --- | --- |
| A browser, media player, or another desktop app | Open the desktop recorder. Windows uses WASAPI loopback on the default playback device. macOS uses a Core Audio system tap. |
| One DAW track, a bus, or the master output | Insert the VST3 or CLAP plug-in on the relevant effects chain. It captures mono or stereo audio at that point and passes the input through unchanged. |
| A software instrument with effects | Insert Backtracking after the instrument and any effects you want in the sample. Move it ahead of those effects to capture the dry signal. |

On Windows, route the source to the default playback device. If a DAW uses ASIO, exclusive mode, or another direct output path, use the plug-in to sample its track signal. On macOS, grant system audio recording permission when the desktop app first prompts you. [Windows WASAPI](https://learn.microsoft.com/en-us/windows/win32/coreaudio/loopback-recording) · [Apple Core Audio](https://developer.apple.com/documentation/coreaudio/capturing-system-audio-with-core-audio-taps)

### Open settings

Click the gear in the toolbar or right-click inside the Backtracking window to open the same **Settings** menu. Use the menu to set the history length, auto-resume delay, background transparency, WAV export format, and save folder. In the Windows and macOS desktop apps, you can toggle **Keep on top** directly from the toolbar. The DAW controls the plug-in window.

In the macOS desktop app, use a mouse right-click or the trackpad's secondary click to open the menu. Use `Control` with a click and drag over the waveform to select audio.

The settings menu offers five background transparency levels. Choose 0%, 25%, 50%, 75%, or 100%, where 0% is opaque and 100% is clear. On the desktop apps, you can open Settings and adjust the value even at 100%. In VST3 and CLAP, **Panel transparency** fades the waveform panel's fill. The DAW provides the background behind it.

### Show waveform only

Turn on **Show waveform only** to hide Backtracking's frame and dark panel background. The waveform remains visible with **Play/Pause** at its top left. The desktop window uses a transparent background. An embedded VST3 or CLAP editor may still show the DAW's window frame or opaque background. To restore the full interface, right-click inside the window and turn the option off. This setting defaults to Off and is saved with the desktop preferences or the plug-in's DAW project settings.

### Move and resize the desktop window

The Windows and macOS desktop apps use a frameless window. Use the gestures below to move or resize it. To close the desktop app, choose **Quit** from Settings.

| Action | Windows desktop | macOS desktop |
| --- | --- | --- |
| Move the window | Hold `Alt` and left-drag inside the content area. | Hold `Option` and left-drag outside the waveform. Hold `Command` + `Option` and left-drag from anywhere in the content area. |
| Resize the window | Move the pointer to an inside edge or corner, then left-drag. | Move the pointer to an inside edge or corner, then left-drag. |
| Select audio | Hold `Ctrl` and left-drag over the waveform. | Hold `Option` or `Control` and left-drag over the waveform. |

### Read the waveform and capture status

The toolbar's **Play/Pause** icon shows the available action. Hover over the button to see **Start capture** or **Pause capture**, and hover over the indicator to see the capture state.

- **Waiting for sound**. Capture is armed and starts when the incoming signal crosses the silence threshold.
- **Recording**. The red indicator blinks. The newest audio appears on the right. Once the buffer fills, new audio gradually replaces the oldest samples.
- **Paused**. Pressing **Pause** or working with the waveform freezes the current history for selection and export. Incoming audio is skipped during the pause.
- **Capture unavailable**. If the desktop recorder loses its connection, press **Play** to retry.

**History / Buffer** can hold 10, 20, or 30 seconds and defaults to 20 seconds. This is the amount of audio retained in memory. Two five-second sounds a minute apart are joined with only a little silence at the join. Very quiet material may fall below the capture threshold. Shortening the buffer trims the oldest audio. Lengthening it gives future audio more room.

### Live audio meter

The meter next to the gear reads the incoming audio signal and defaults to dBFS. Choose the unit in the gear menu. In the macOS desktop app, you can also click the readout to switch units. The meter displays `—` while paused or when a reading is unavailable.

| Mode | What it shows |
| --- | --- |
| **dBFS** | Digital sample peak level. 0 dBFS is digital full scale. |
| **VU** | A 300-millisecond RMS estimate of average level. A 1 kHz sine wave with a −18 dBFS peak reads 0 VU. |
| **LUFS** | Momentary loudness estimated with K-weighting and a 400-millisecond sliding window. |

Loudness measurement references. [ITU-R BS.1770](https://www.itu.int/rec/R-REC-BS.1770-5-202311-I/en) · [EBU Tech 3341](https://tech.ebu.ch/publications/tech3341)

### Click, select, and drag

In the full interface, hover over an unselected waveform to preview all retained audio. Clicking or starting a drag anywhere on the waveform exports that full range. For example, a full 20-second buffer exports 20 seconds. If only 7 seconds have been captured, the WAV is 7 seconds long. Both display modes mark a `Ctrl` or `Option` selection by inverting the waveform line color within that range. The selected range is used for export.

Gesture hints appear as you interact in the full interface. The tooltip tries the right, left, below, and above in that order and stays within the interface.

| Action | Windows | macOS | Result |
| --- | --- | --- | --- |
| See the available actions | Hover over the waveform | Hover over the waveform | The hint shows **Click** alongside **Ctrl + Click** or **Option + Click**. If a selection or WAV is ready to drag, the main hint changes to **Drag and Drop**. |
| Enter selection mode | Hold `Ctrl` over the waveform | Hold `Option` over the waveform | The hint changes to **Drag to Select**. |
| Save the clip | Click the waveform | Click the waveform | Capture pauses and saves a WAV. With no selection, the file contains all retained audio. |
| Drop a clip into a DAW | Hold the primary mouse button on the waveform and drag out | Hold the primary mouse button on the waveform and drag out | The hint shows **Drag and Drop**. Once ready, drop the WAV onto the REAPER timeline, FL Studio Playlist, or another WAV drop target. |
| Choose both ends | Hold `Ctrl` and drag across the waveform | Hold `Option` and drag across the waveform | The hint shows **Drag to Select**. Release to keep the selection, then release the modifier and click or drag the highlighted range. |

When you finish selecting a range, Backtracking prepares its WAV so you can drag it next. The first export and clips that need resampling may take a moment to prepare. Keep the mouse button held until the file is ready to drag. If the DAW does not accept the drop, open the save folder and import the WAV from there.

### Arm, pause, and auto-resume capture

Press **Play** to arm capture or **Pause** to pause it manually. Once re-armed, the next sound is appended to the existing history within the selected limit.

**Auto-resume** defaults to **10 seconds**. Open the gear menu to choose 20 or 30 seconds, or select **Off** for manual control. After a manual pause or waveform interaction, the timer starts after the last click in the sampler or the end of a gesture. Capture stays paused during a drag. The timer re-arms capture, and the incoming signal must still cross the silence threshold before audio is recorded.

### Clear the current recording

Click the trash icon in the toolbar. It is labeled **Clear capture** on Windows desktop and **Clear captured audio** in the plug-ins. This empties the waveform, selection, and audio held in memory. If capture is armed, incoming sound starts filling the waveform again. You can find previously exported WAVs in the save folder.

### Output location and file format

In Settings, choose **Select save folder** (**選擇儲存資料夾…** in the macOS app) and select the exact WAV destination. If you select `My Song/Samples`, WAVs go directly into that folder. Previously saved destinations are carried forward.

Choose **Go to save folder** (**前往儲存資料夾** in the macOS app) to open the current destination in your system's file manager. Backtracking creates the folder first if it does not exist yet.

| Edition | Default save folder |
| --- | --- |
| macOS desktop | `~/Backtracking/output/` |
| Windows desktop | `%USERPROFILE%\Backtracking\output\` |
| VST3 / CLAP | `Backtracking/output/` in your home directory |

Choose the sample rate and encoding independently in the gear menu.

| Edition | Sample rate menu | Encoding menu |
| --- | --- | --- |
| Windows desktop | **WAV rate** | **WAV encoding** |
| macOS desktop | **WAV 匯出格式 → 取樣率** | **WAV 匯出格式 → 編碼** |
| VST3 / CLAP | **WAV sample rate** | **WAV sample format** |

| WAV setting | Options | Default |
| --- | --- | --- |
| Sample rate | 44.1 kHz, 48 kHz, 96 kHz | 48 kHz |
| Encoding | 16-bit PCM, 24-bit PCM, 32-bit IEEE float | 32-bit float |

Clicking to save, exporting a selection, and dragging into a DAW all use the current WAV format. Capture and metering run at the source sample rate. Any sample rate conversion happens during export. The WAV preserves the captured mono or stereo layout. The Windows desktop app mixes multichannel playback to stereo. The rolling history lives in memory, so export any clips you need before closing the app or removing the plug-in.

### Save and restore settings

The Windows and macOS desktop apps save your history length, auto-resume delay, background transparency, meter mode, WAV sample rate and encoding, save folder, keep-on-top setting, and waveform-only mode as you change them. Those preferences are restored the next time you open the app.

- **Windows**. Settings are stored in `%LOCALAPPDATA%\Backtracking\Settings`, with `%APPDATA%\Backtracking\Settings` as a fallback when `LOCALAPPDATA` is unavailable. The app opens at its default window size and Windows places the window.
- **macOS**. Settings are stored in the macOS user preferences. The app also restores the desktop window's size and position.

For **VST3 and CLAP**, the DAW project stores each plug-in instance's history length, auto-resume delay, panel transparency, meter mode, WAV sample rate and encoding, save folder, and waveform-only mode. Save the project manually after making changes. When you reopen that project, the DAW restores each instance's settings.

### Troubleshooting

| Symptom | What to check |
| --- | --- |
| Audio plays but the waveform stays still | Hover over the indicator to check the capture state. If paused, press **Play**. On Windows, verify the source uses the default playback device. On macOS, verify system audio recording permission. In a DAW, check playback or monitoring and the signal at the plug-in insert. |
| Capture stops after switching Windows playback devices | Set the new device as the default playback device, then press **Play** to reconnect. |
| The plug-in does not appear in the DAW | Verify that the entire `.vst3` or `.clap` is in a scanned folder and that its architecture matches the DAW. Rescan using the steps below. |
| A WAV saves but the DAW refuses the drop | Wait for export to finish and drop on an audio timeline or Playlist. You can also drag the file from the output folder or import it manually. |
| The WAV could not be written | Check that the waveform contains audio and choose a writable save folder. |

**Offline rendering**. CLAP skips capture when the host reports offline mode. The nice-plug 0.4.2 VST3 wrapper can pass the previous processing mode to Backtracking, so an offline render may enter the capture history. Export important clips before rendering. To keep capture frozen during the render, turn off auto-resume and press **Pause** first.

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
2. 將自動續錄設為 **手動／Off**。播放有聲音時確認紅燈閃爍，再按 **Pause**，確認波形停止更新。按 **Play** 重新待命，播放聲音後確認接續收錄。

   Set auto-resume to **Off**. Play a sound and check that the red indicator blinks. Press **Pause** and check that the waveform freezes. Press **Play**, then play audio and check that capture continues.
3. 選擇儲存資料夾，將 WAV 設為 44.1 kHz、24-bit PCM。框選一小段並拖進 DAW，再用 **前往儲存資料夾** 確認 WAV 直接存到指定位置，且規格正確。若使用插件，分別在 VST3 和 CLAP 實例操作一次。

   Choose a save folder and set the WAV format to 44.1 kHz, 24-bit PCM. Select a short clip and drag it into the DAW. Use **Go to save folder** to check that the WAV is in the selected folder and uses the chosen format. For plug-in use, try a VST3 instance and a CLAP instance.
4. 修改一項偏好後重開桌面版，確認設定有還原。測試插件時，修改設定後先儲存 DAW 專案，再重開該專案確認。

   Change a preference and relaunch the desktop app to check that it is restored. For the plug-in, change a setting, save the DAW project, and reopen it to check the saved value.
5. 開啟 **只顯示波形**，確認波形與左上角 **Play／Pause** 可操作，框選時可看到區間內的波形變色。再按右鍵開啟設定並取消勾選，確認可回到完整介面。

   Turn on **Show waveform only**. Check that the waveform and its top-left **Play/Pause** control work, and that selecting a range changes the waveform color in that range. Right-click to open Settings and turn the option off to restore the full interface.

[回到專案首頁 / Back to the project README](../README.md)
