# Backtracking 操作手冊 / User Guide

[繁體中文](#繁體中文) · [English](#english) · [安裝與建置 / Installation and build](#安裝與建置--installation-and-build)

## 繁體中文

### 選擇擷取來源

| 我想留下的聲音 | 使用方式 |
| --- | --- |
| 瀏覽器、播放器或其他程式的聲音 | 開啟桌面版，在 Options 選擇系統播放或指定 App。 |
| 麥克風，或系統聲加麥克風 | 開啟桌面版，在 Options 選擇麥克風，或啟用 A／B 分開擷取。 |
| 某一軌、群組或主輸出的聲音 | 把 VST3 或 CLAP 插入對應的效果鏈。插件接收該位置的 mono 或 stereo 音訊，並將音訊原樣傳給下游。 |
| 軟體樂器加效果後的聲音 | 把插件放在樂器及要納入取樣的效果後面。若需要乾聲，就放在那些效果前面。 |

Windows 上請把要取樣的程式輸出到預設播放裝置。若 DAW 使用 ASIO、獨占模式或其他直接輸出路徑，使用音軌插件可直接取得該軌訊號。macOS 桌面版首次啟動時，請依系統提示允許錄製系統音訊。 [Windows WASAPI](https://learn.microsoft.com/en-us/windows/win32/coreaudio/loopback-recording) · [Apple Core Audio](https://developer.apple.com/documentation/coreaudio/capturing-system-audio-with-core-audio-taps)

### 開啟設定

點擊工具列的齒輪，或在 Backtracking 視窗內按滑鼠右鍵，可開啟 **Options**。桌面版會開啟獨立的設定視窗，VST3／CLAP 則在 DAW 的插件編輯器內開啟設定。可按分類找到錄音控制、主題色、保留時間、自動續錄、背景透明度、WAV 格式及資料夾。桌面版也可直接從工具列切換 **置頂／Keep on top**。

macOS 桌面版可使用滑鼠右鍵或觸控板的次要點擊開啟選單。`Control` 搭配波形上的點擊與拖曳用於框選。

設定選單的背景透明度可選 0%、25%、50%、75% 或 100%。0% 為不透明，100% 為透明。桌面版在 100% 時仍可開啟設定選單調整透明度。VST3／CLAP 的 **Panel transparency** 調整插件內的波形面板背景，實際看到的底色取決於 DAW 的視窗背景。

### 來源、波形與旗標

- 在桌面版 Options 選擇系統播放、麥克風或指定 App。Windows 的指定 App 擷取需 OS build 20348 以上。macOS 首次擷取系統聲或麥克風時，請允許對應權限。
- 每次打開來源下拉選單都會更新清單。Windows 會列出仍有播放串流的 App，包含串流已暫停的情況。macOS 會列出 Core Audio 辨識到有播放輸出的 App。找不到要選的 App 時，先讓它播放一段音訊，再重新打開選單。麥克風來源會列出系統目前可用的輸入裝置。
- 可同時啟用 A 和 B。兩路各自選擇來源與 Left、Right、Center、Mid、Side 或 Merged。Merged 沿用來源的單聲道或雙聲道輸出。Center 需要來源提供原生中央聲道。
- Windows 的 A／B 選用同一來源時，會以裝置的原始取樣率保留兩路音訊，例如 System Side／System Mid。若 WAV 設定使用不同取樣率，會在匯出整段選取時才轉換。暫停後重新開始錄音時，會平滑前後片段的接縫。
- 單一來源可顯示合併波形或上下兩列 L／R。啟用 A／B 時，上列顯示 A、下列顯示 B，各占波形區的一半，並各自顯示音量。
- 完成框選或明確保存整段時，波形會標記範圍頭尾。框選時按住 `Shift`，靠近可見旗標即可吸附。旗標會隨錄音歷史移動，並在各自離開可視範圍時消失。
- `Ctrl+S` 與成功拖放會同時匯出 A／B。兩個 WAV 使用同一起訖和取樣率。若 DAW 要求指定匯入位置，可選擇分開音軌並使用相同開始位置。

[桌面版更新設定](UPDATES.md)

### 只顯示波形

開啟 **只顯示波形／Show waveform only** 後，Backtracking 會隱藏自身的外框與深色底板，畫面只保留波形。桌面版的背景會變透明。VST3／CLAP 的視窗尺寸由 DAW 宿主決定，內嵌時可能看得到宿主視窗的外框或不透明底色。這個選項預設關閉，並跟著桌面偏好設定或 DAW 專案中的插件設定儲存。

在波形上按右鍵開啟 Options。錄音已暫停時，可選 **開始錄音／Play**。錄音已待命或正在收錄時，可選 **暫停錄音／Pause**。

切換時，波形會保持完整介面中的高度並垂直置中，視窗尺寸也會保留。拖曳桌面視窗的左右內緣可調整寬度，拖曳上下內緣可調整高度。視窗變窄時，同樣的錄音保留時間會顯示在更短的距離上，波形每秒移動的像素會減少。可用高度不足時，波形會跟著縮小。取消勾選時，視窗會視需要放大以容納完整介面。

| 桌面版 | 只顯示波形最小尺寸 | 完整介面最小尺寸 |
| --- | --- | --- |
| Windows | 240 × 48 邏輯像素 | 510 × 132 邏輯像素 |
| macOS | 240 × 48 點 | 560 × 100 點 |

要回到完整介面，在波形上按右鍵，再取消勾選 **只顯示波形／Show waveform only**。只顯示波形時，Backtracking 會向 Windows 要求關閉原生陰影、邊框與圓角。

### 移動與縮放桌面視窗

Windows 和 macOS 桌面版採用無邊框視窗。以下手勢可自由移動或調整大小，靠近螢幕邊緣時不會自動貼齊。VST3／CLAP 的外層視窗由 DAW 管理。要關閉桌面程式，請從設定選單選擇 **結束／Quit**。

Windows 桌面版啟用 **置頂／Keep on top** 時，若工作列等同一桌面的視窗蓋住 Backtracking，程式會重新提升自身層級，並保留目前的輸入焦點。安全桌面與受保護的系統畫面由 Windows 管理。

Windows 桌面版可從工作列右側的系統匣顯示或隱藏視窗。隱藏視窗後，錄音仍可在背景繼續。要停止程式，可從系統匣或設定選擇 **Quit**。先將 `Backtracking.exe` 放在固定位置，再開啟 **Start with Windows**。Backtracking 會在登入 Windows 時開啟並還原上次的視窗位置。

macOS 桌面版可從選單列顯示、隱藏或結束 Backtracking，也可啟用登入時開啟。系統要求核准時，請在 macOS 系統設定允許 Backtracking 的登入項目。

| 動作 | Windows 桌面版 | macOS 桌面版 |
| --- | --- | --- |
| 移動視窗 | 在內容區按住 `Alt`，用滑鼠左鍵拖曳。 | 在波形外的內容區按住 `Option`，用左鍵拖曳。在內容區任意位置可按住 `Command` + `Option` 再用左鍵拖曳。 |
| 調整視窗大小 | 將游標移到視窗內容區的內緣或內角，按住左鍵拖曳。 | 將游標移到視窗內容區的內緣或內角，按住左鍵拖曳。 |
| 框選音訊 | 在波形上按住 `Ctrl` 並用左鍵拖曳。 | 在波形上按住 `Option` 或 `Control` 並用左鍵拖曳。 |

### 讀懂波形與錄音狀態

在完整介面中，工具列的 **Play／Pause** 圖示顯示可執行的動作。將游標移到按鈕上，可看到 **開始錄音／Start capture** 或 **暫停錄音／Pause capture**。將游標移到狀態燈上，可查看目前狀態。

- **等待聲音／Waiting for sound**。錄音已待命，聲音超過靜音偵測門檻時便開始收錄。
- **錄音中／Recording**。紅色狀態燈閃爍。波形右側是最新收錄的音訊，達到保留上限後，新的音訊會逐步取代最舊的音訊。
- **已暫停／Paused**。按下 **Pause** 或操作波形會暫停錄音，讓波形固定供選段和匯出。暫停期間的聲音會略過。
- **未連線／Capture unavailable**。桌面版無法擷取時，可用 **Play** 重新連線。

**保留／History／Buffer** 可選 10、20、30 或 60 秒，預設為 20 秒。這是記憶體中的音訊長度。假設兩段各 5 秒的聲音相隔一分鐘，兩段會接在一起，中間只保留靜音偵測需要的短緩衝。非常小聲的內容可能低於門檻。縮短保留時間會裁掉最早的部分，事後加長只會增加接下來可保留的長度。

### 即時音量表

齒輪旁的音量表顯示目前輸入的音訊，預設模式是 dBFS。從齒輪選單的音量表設定切換單位。macOS 桌面版也可直接點擊讀值切換。暫停錄音或目前無可用讀值時，顯示 `—`。

| 模式 | 讀值 |
| --- | --- |
| **dBFS** | 數位取樣峰值，0 dBFS 是數位滿刻度。 |
| **VU** | 以 300 毫秒 RMS 估算平均電平。以 1 kHz、峰值 −18 dBFS 的正弦波校準為 0 VU。 |
| **LUFS** | 瞬時響度。以 K 加權和 400 毫秒滑動視窗估算。 |

響度量測參考文件。 [ITU-R BS.1770](https://www.itu.int/rec/R-REC-BS.1770-5-202311-I/en) · [EBU Tech 3341](https://tech.ebu.ch/publications/tech3341)

### 框選與拖放

在完整介面中，未框選時將游標移到波形上會預覽整段保留音訊。從波形按住左鍵拖出 Backtracking 視窗，成功放到接受 WAV 的位置後，即可匯出這整段音訊。例如保留時間設為 20 秒，錄滿時匯出 20 秒，目前只收錄 7 秒時就匯出 7 秒。完整介面和只顯示波形模式都以波形線條的反相配色標示 `Ctrl` 或 `Option` 框選區間。匯出時使用該區間。

完整介面會在相關操作時顯示波形提示。介面會依右、左、下、上的順序尋找可以放下提示的位置，並將提示留在視窗內。

| 動作 | Windows | macOS | 結果 |
| --- | --- | --- | --- |
| 查看可用操作 | 將游標移到波形 | 將游標移到波形 | 提示會顯示拖放與框選的操作方式。 |
| 進入框選模式 | 游標在波形上時按住 `Ctrl` | 游標在波形上時按住 `Option` | 提示切換為 **Drag to Select**。 |
| 匯入 DAW | 在波形上按住滑鼠左鍵並拖出 | 在波形上按住滑鼠左鍵並拖出 | 按住時顯示 **Drag and Drop**。檔案就緒後，拖到 REAPER 時間軸、FL Studio Playlist 或其他接受 WAV 的位置。 |
| 指定起訖點 | 按住 `Ctrl`，在波形上按住左鍵拖曳 | 按住 `Option`，在波形上按住左鍵拖曳 | 顯示 **Drag to Select**。放開後保留框選區間，再放開修飾鍵並拖出選取區。 |

單擊、框選和視窗內的拖曳只會操作波形。要匯出框選片段，請拖出後放到可接受 WAV 的位置，或按 `Ctrl+S` 保存。取消拖曳、拖回原視窗放開或目標拒絕時，不會留下匯出檔。若 WAV 還在準備，請保持按住滑鼠再放開。DAW 拒絕拖放時，可按 `Ctrl+S` 保存，再從保存資料夾手動匯入。

Backtracking 視窗有焦點時，按 `Esc` 或 `Backspace` 可取消框選並立刻恢復錄音待命。

### 保存快照

在 Backtracking 有焦點時，按 `Ctrl+S`，或從 Options 選擇 **Save snapshot**。有框選時只保存選取的音訊，並立即恢復錄音待命。未框選時保存按鍵當下保留的整段音訊。錄音中可以一邊保存一邊錄音，已暫停時則在取得快照後恢復待命。

如果 DAW 攔截 `Ctrl+S` 作為專案儲存快捷鍵，可從插件的 Options 選擇 **Save snapshot**。

### 啟用、暫停與自動續錄

選擇 **Play** 進入錄音待命，或選擇 **Pause** 手動暫停。完整介面可使用工具列，只顯示波形時可從右鍵開啟 Options。重新待命後，下一段收錄的音訊會接在原有歷史後方，受保留時間上限約束。

**自動續錄／Auto-resume** 可在設定選單選擇 **0、5 或 10 秒**，預設 **10 秒**。選 **0 秒**時，點擊、框選或拖出 WAV 完成後會立刻恢復待命。框選的片段會保留，可在錄音接續時拖出。手動按 **Pause** 後會維持暫停，直到按 **Play** 或完成下一次波形操作。選 **5 秒**或 **10 秒**時，手動暫停或操作波形後會從取樣器視窗內最後一次點擊或手勢結束時開始計時。拖曳期間會保持暫停。恢復待命後，聲音超過靜音偵測門檻時才會收錄。

舊版儲存的 **Off、20 秒、30 秒**會改用 **10 秒**。

### 清除目前錄音

點擊工具列的垃圾桶圖示。提示文字依版本為 **清除目前錄音**、**Clear capture** 或 **Clear captured audio**。這會清空目前的波形、選取區間與記憶體中的錄音。若錄音已待命，接下來收到的聲音會重新填入波形。先前匯出的 WAV 可從存檔資料夾取用。

### 輸出位置與檔案規格

在 Options 可各自指定並開啟兩個資料夾。成功拖放使用預設輸出資料夾，`Ctrl+S` 和 **Save snapshot** 使用保存資料夾。原先設定的匯出目的地會沿用。

| 版本 | 預設輸出資料夾 | 保存資料夾的預設位置 |
| --- | --- | --- |
| macOS 桌面版 | `~/Backtracking/output/` | `~/Backtracking/saved/` |
| Windows 桌面版 | `%USERPROFILE%\Backtracking\output\` | `%USERPROFILE%\Backtracking\saved\` |
| VST3／CLAP | 使用者家目錄下的 `Backtracking/output/` | 使用者家目錄下的 `Backtracking/saved/` |

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

快照保存及拖放匯出都使用目前選定的 WAV 規格。錄音與音量表以來源取樣率運作，取樣率轉換在匯出時進行。WAV 保留擷取結果的 mono 或 stereo 聲道配置，Windows 桌面版會將多聲道輸出裝置的混音轉為 stereo。回溯歷史儲存在記憶體，關閉桌面程式或移除插件前，請先存下需要的片段。

### 保存與還原設定

Windows 和 macOS 桌面版在變更後會自動保存保留時間、自動續錄、主題色、背景透明度、音量表模式、WAV 取樣率與編碼、兩個資料夾、置頂及只顯示波形的設定，下次開啟時還原。

- **Windows**。設定儲存在 `%LOCALAPPDATA%\Backtracking\Settings`。若找不到 `LOCALAPPDATA`，則使用 `%APPDATA%\Backtracking\Settings`。程式會還原上次的視窗位置。
- **macOS**。設定儲存在系統的使用者偏好中，也會記住桌面視窗的位置與尺寸。

**VST3／CLAP** 的保留時間、自動續錄、主題色、面板透明度、音量表模式、WAV 取樣率與編碼、兩個資料夾及只顯示波形的設定由 DAW 專案保存，每個插件實例可獨立設定。修改後請手動儲存專案。重新開啟同一個專案時，DAW 會還原各實例的設定。

### 排除問題

| 狀況 | 檢查方式 |
| --- | --- |
| 播放有聲音，但波形沒有更新 | 將游標移到狀態燈確認狀態。若已暫停，按 **Play**。Windows 確認來源使用預設播放裝置。macOS 確認系統音訊錄製權限。插件確認音軌正在播放或監聽，且 Backtracking 的插入點收得到訊號。 |
| Windows 切換喇叭或耳機後無法擷取 | 確認新的裝置已是系統預設播放裝置，再按 **Play** 重新連線。 |
| DAW 掃描不到插件 | 確認已複製完整 `.vst3` 或 `.clap`，架構與 DAW 相容，且檔案位於 DAW 會掃描的資料夾。依下方步驟重新掃描。 |
| DAW 拒絕拖放 | 拖出視窗後保持按住滑鼠，等 WAV 準備完成，再放到可接受音訊檔的時間軸或 Playlist。也可按 `Ctrl+S` 保存，再從保存資料夾手動匯入。 |
| 無法寫入 WAV | 確認波形有可用的音訊，再選擇可寫入的存檔資料夾。 |

**離線渲染**。CLAP 在宿主回報離線模式時會略過擷取。VST3 使用的 nice-plug 0.4.2 有時會回報前一次的處理模式，離線渲染仍可能進入回溯歷史。渲染前若要保留目前的素材，請先匯出。若要在渲染期間維持凍結，先將自動續錄設為 **0 秒**，再按 **Pause**。

## English

### Choose what to capture

| What you want to sample | Which edition to use |
| --- | --- |
| A browser, media player, or another desktop app | Open the desktop recorder and choose system playback or the app in Options. |
| A microphone, or system playback plus a microphone | Choose a microphone in desktop Options or enable A/B to capture them separately. |
| One DAW track, a bus, or the master output | Insert the VST3 or CLAP plug-in on the relevant effects chain. It captures mono or stereo audio at that point and passes the input through unchanged. |
| A software instrument with effects | Insert Backtracking after the instrument and any effects you want in the sample. Move it ahead of those effects to capture the dry signal. |

On Windows, route the source to the default playback device. If a DAW uses ASIO, exclusive mode, or another direct output path, use the plug-in to sample its track signal. On macOS, grant system audio recording permission when the desktop app first prompts you. [Windows WASAPI](https://learn.microsoft.com/en-us/windows/win32/coreaudio/loopback-recording) · [Apple Core Audio](https://developer.apple.com/documentation/coreaudio/capturing-system-audio-with-core-audio-taps)

### Open settings

Click the gear in the toolbar or right-click inside the Backtracking window to open **Options**. The desktop app opens a separate window. VST3 and CLAP open Options inside the DAW's plug-in editor. The controls are grouped by category, including capture, theme accent, history length, auto-resume, transparency, WAV format, and folders. On desktop, you can also toggle **Keep on top** from the toolbar.

In the macOS desktop app, use a mouse right-click or the trackpad's secondary click to open the menu. Use `Control` with a click and drag over the waveform to select audio.

The settings menu offers five background transparency levels. Choose 0%, 25%, 50%, 75%, or 100%, where 0% is opaque and 100% is clear. On the desktop apps, you can open Settings and adjust the value even at 100%. In VST3 and CLAP, **Panel transparency** fades the waveform panel's fill. The DAW provides the background behind it.

### Sources, waveforms, and flags

- Choose system playback, a microphone, or a specific app in desktop Options. App capture requires Windows OS build 20348 or later. On macOS, grant the relevant system audio or microphone permission when prompted.
- The source list updates each time you open the drop-down. Windows lists apps with an open playback stream, including paused streams. macOS lists apps that Core Audio identifies as playback sources. If an app is missing, play some audio in it and reopen the list. Microphone entries come from the input devices the system currently makes available.
- Enable A and B to capture two routes. For each, choose a source and Left, Right, Center, Mid, Side, or Merged. Merged uses the source's mono or stereo output format. Center requires a native center channel.
- On Windows, A/B routes from the same source, such as System Side and System Mid, use the device's native sample rate. Export converts the whole selection if the WAV rate differs. Capture also smooths the splice when you resume after a pause.
- For one source, choose a merged waveform or two L/R rows. With A/B enabled, A and B each use half the waveform area and have independent meters.
- Flags mark the boundaries after selecting a range or saving the full capture. Hold `Shift` while selecting near a visible flag to snap to it. Each flag follows the retained audio and disappears when it leaves view.
- `Ctrl+S` and accepted drops export both tracks with the same boundaries and sample rate. If your DAW prompts for placement, choose separate tracks at the same start position.

[Desktop update settings](UPDATES.md)

### Show waveform only

Turn on **Show waveform only** to hide Backtracking's frame and dark panel background. Only the waveform is visible. The desktop window uses a transparent background. The DAW controls the VST3 or CLAP editor size and may supply its own window frame or opaque background. This setting defaults to Off and is saved with the desktop preferences or the plug-in's DAW project settings.

Right-click the waveform to open Options. Choose **Play** (**開始錄音** in the macOS app) when capture is paused, or **Pause** (**暫停錄音**) when capture is armed or recording.

When you switch modes, the waveform keeps the height it had in the full interface and moves to the vertical center. The window keeps its size. Drag either inside side edge to adjust the desktop window's width or the top and bottom edges to adjust its height. A narrower window shows the same retained duration over a shorter distance, reducing the number of pixels the waveform scrolls per second. The waveform shrinks to fit a shorter window. When you turn the mode off, the window grows if needed to fit the full interface.

| Desktop app | Waveform-only minimum size | Full interface minimum size |
| --- | --- | --- |
| Windows | 240 × 48 logical pixels | 510 × 132 logical pixels |
| macOS | 240 × 48 points | 560 × 100 points |

To restore the full interface, right-click the waveform and turn off **Show waveform only**. In waveform-only mode, Backtracking asks Windows to suppress the native shadow, border, and rounded corners.

### Move and resize the desktop window

The Windows and macOS desktop apps use a frameless window. The gestures below move and resize it without automatic snapping near screen edges. The DAW manages the outer window for VST3 and CLAP. To close the desktop app, choose **Quit** from Settings.

With **Keep on top** enabled on Windows, Backtracking reasserts its position when the taskbar or another window on the same desktop covers it, while leaving input focus where it is. Windows manages the secure desktop and protected system surfaces.

Use the Windows system tray icon to show or hide the window. Capture can continue while the window is hidden. Choose **Quit** from the tray or Settings to exit. Place `Backtracking.exe` in a permanent location and enable **Start with Windows** to open Backtracking at its last window position when you sign in.

On macOS, use the menu bar item to show, hide, or quit Backtracking, or enable launch at login. If macOS asks for approval, allow the Backtracking login item in System Settings.

| Action | Windows desktop | macOS desktop |
| --- | --- | --- |
| Move the window | Hold `Alt` and left-drag inside the content area. | Hold `Option` and left-drag outside the waveform. Hold `Command` + `Option` and left-drag from anywhere in the content area. |
| Resize the window | Move the pointer to an inside edge or corner, then left-drag. | Move the pointer to an inside edge or corner, then left-drag. |
| Select audio | Hold `Ctrl` and left-drag over the waveform. | Hold `Option` or `Control` and left-drag over the waveform. |

### Read the waveform and capture status

In the full interface, the toolbar's **Play/Pause** icon shows the available action. Hover over the button to see **Start capture** or **Pause capture**, and hover over the indicator to see the capture state.

- **Waiting for sound**. Capture is armed and starts when the incoming signal crosses the silence threshold.
- **Recording**. The red indicator blinks. The newest audio appears on the right. Once the buffer fills, new audio gradually replaces the oldest samples.
- **Paused**. Pressing **Pause** or working with the waveform freezes the current history for selection and export. Incoming audio is skipped during the pause.
- **Capture unavailable**. If the desktop recorder loses its connection, press **Play** to retry.

**History / Buffer** can hold 10, 20, 30, or 60 seconds and defaults to 20 seconds. This is the amount of audio retained in memory. Two five-second sounds a minute apart are joined with only a little silence at the join. Very quiet material may fall below the capture threshold. Shortening the buffer trims the oldest audio. Lengthening it gives future audio more room.

### Live audio meter

The meter next to the gear reads the incoming audio signal and defaults to dBFS. Choose the unit in the gear menu. In the macOS desktop app, you can also click the readout to switch units. The meter displays `—` while paused or when a reading is unavailable.

| Mode | What it shows |
| --- | --- |
| **dBFS** | Digital sample peak level. 0 dBFS is digital full scale. |
| **VU** | A 300-millisecond RMS estimate of average level. A 1 kHz sine wave with a −18 dBFS peak reads 0 VU. |
| **LUFS** | Momentary loudness estimated with K-weighting and a 400-millisecond sliding window. |

Loudness measurement references. [ITU-R BS.1770](https://www.itu.int/rec/R-REC-BS.1770-5-202311-I/en) · [EBU Tech 3341](https://tech.ebu.ch/publications/tech3341)

### Select and drag out

In the full interface, hover over an unselected waveform to preview all retained audio. Drag from anywhere on the waveform outside the Backtracking window and drop on a target that accepts WAV files to export that full range. For example, a full 20-second buffer exports 20 seconds. If only 7 seconds have been captured, the WAV is 7 seconds long. Both display modes mark a `Ctrl` or `Option` selection by inverting the waveform line color within that range. The selected range is used for export.

Gesture hints appear as you interact in the full interface. The tooltip tries the right, left, below, and above in that order and stays within the interface.

| Action | Windows | macOS | Result |
| --- | --- | --- | --- |
| See the available actions | Hover over the waveform | Hover over the waveform | The hint shows how to drag out audio or select a range. |
| Enter selection mode | Hold `Ctrl` over the waveform | Hold `Option` over the waveform | The hint changes to **Drag to Select**. |
| Drop a clip into a DAW | Hold the primary mouse button on the waveform and drag out | Hold the primary mouse button on the waveform and drag out | The hint shows **Drag and Drop**. Once ready, drop the WAV onto the REAPER timeline, FL Studio Playlist, or another WAV drop target. |
| Choose both ends | Hold `Ctrl` and drag across the waveform | Hold `Option` and drag across the waveform | The hint shows **Drag to Select**. Release to keep the selection, then release the modifier and drag out the highlighted range. |

Clicking, selecting, and dragging within the window only affect the waveform. To export a selected clip, drag it out and drop it on a target that accepts WAV files, or press `Ctrl+S` to save. Canceled or rejected drops, including drops back in the source window, leave no export file. Keep the mouse button held while an export finishes preparing. If the DAW rejects the drop, press `Ctrl+S` and import the WAV from the saved snapshots folder.

With Backtracking focused, press `Esc` or `Backspace` to clear the selection and re-arm capture immediately.

### Save a snapshot

With Backtracking focused, press `Ctrl+S` or choose **Save snapshot** in Options. With a selection, it saves only that range and re-arms capture immediately. With no selection, it saves all audio retained at the time of the keypress. Active capture continues while saving. If capture was paused, it re-arms after the snapshot is taken.

If your DAW intercepts `Ctrl+S` to save its project, choose **Save snapshot** in the plug-in's Options.

### Arm, pause, and auto-resume capture

Choose **Play** to arm capture or **Pause** to pause it manually. Use the toolbar in the full interface or right-click to open Options in waveform-only mode. Once re-armed, the next sound is appended to the existing history within the selected limit.

**Auto-resume** offers **0, 5, or 10 seconds** in Settings and defaults to **10 seconds**. At **0 seconds**, capture re-arms as soon as you finish clicking, selecting a range, or dragging out a WAV. A selected clip remains available to drag out while capture continues. After you manually press **Pause**, capture stays paused until you press **Play** or finish another waveform gesture. With **5 seconds** or **10 seconds**, the timer starts after the last click in the sampler or the end of a gesture. Capture stays paused during a drag. Once re-armed, capture waits for the incoming signal to cross the silence threshold.

Existing **Off**, **20-second**, and **30-second** settings use the **10-second** default.

### Clear the current recording

Click the trash icon in the toolbar. It is labeled **Clear capture** on Windows desktop and **Clear captured audio** in the plug-ins. This empties the waveform, selection, and audio held in memory. If capture is armed, incoming sound starts filling the waveform again. You can find previously exported WAVs in the save folder.

### Output location and file format

You can choose and open both folders from Options. Successful drops use the default export folder. `Ctrl+S` and **Save snapshot** use the saved snapshots folder. Existing export destinations are carried forward.

| Edition | Default export folder | Default saved snapshots folder |
| --- | --- | --- |
| macOS desktop | `~/Backtracking/output/` | `~/Backtracking/saved/` |
| Windows desktop | `%USERPROFILE%\Backtracking\output\` | `%USERPROFILE%\Backtracking\saved\` |
| VST3 / CLAP | `Backtracking/output/` in your home directory | `Backtracking/saved/` in your home directory |

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

Saving a snapshot and exporting by drag and drop both use the current WAV format. Capture and metering run at the source sample rate. Any sample rate conversion happens during export. The WAV preserves the captured mono or stereo layout. The Windows desktop app mixes multichannel playback to stereo. The rolling history lives in memory, so export any clips you need before closing the app or removing the plug-in.

### Save and restore settings

The Windows and macOS desktop apps save your history length, auto-resume delay, theme accent, background transparency, meter mode, WAV sample rate and encoding, both folders, keep-on-top setting, and waveform-only mode as you change them. Those preferences are restored the next time you open the app.

- **Windows**. Settings are stored in `%LOCALAPPDATA%\Backtracking\Settings`, with `%APPDATA%\Backtracking\Settings` as a fallback when `LOCALAPPDATA` is unavailable. The app restores its previous window position.
- **macOS**. Settings are stored in the macOS user preferences. The app also restores the desktop window's size and position.

For **VST3 and CLAP**, the DAW project stores each plug-in instance's history length, auto-resume delay, theme accent, panel transparency, meter mode, WAV sample rate and encoding, both folders, and waveform-only mode. Save the project manually after making changes. When you reopen that project, the DAW restores each instance's settings.

### Troubleshooting

| Symptom | What to check |
| --- | --- |
| Audio plays but the waveform stays still | Hover over the indicator to check the capture state. If paused, press **Play**. On Windows, verify the source uses the default playback device. On macOS, verify system audio recording permission. In a DAW, check playback or monitoring and the signal at the plug-in insert. |
| Capture stops after switching Windows playback devices | Set the new device as the default playback device, then press **Play** to reconnect. |
| The plug-in does not appear in the DAW | Verify that the entire `.vst3` or `.clap` is in a scanned folder and that its architecture matches the DAW. Rescan using the steps below. |
| The DAW refuses the drop | Drag outside the window, keep the mouse button held while the WAV prepares, and release on an audio timeline or Playlist. You can also press `Ctrl+S` and import the WAV from the saved snapshots folder. |
| The WAV could not be written | Check that the waveform contains audio and choose a writable save folder. |

**Offline rendering**. CLAP skips capture when the host reports offline mode. The nice-plug 0.4.2 VST3 wrapper can pass the previous processing mode to Backtracking, so an offline render may enter the capture history. Export important clips before rendering. To keep capture frozen during the render, set auto-resume to **0 seconds** and press **Pause** first.

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
bash plugins/backtracking-plugin/bundle-macos.sh
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
2. 將自動續錄設為 **0 秒**。播放有聲音時確認紅燈閃爍，再按 **Pause**，確認波形停止更新。按 **Play** 重新待命，接著框選一段波形，確認放開滑鼠後恢復待命，而且仍能拖出剛選的片段。

   Set auto-resume to **0 seconds**. Play a sound and check that the red indicator blinks. Press **Pause** and check that the waveform freezes. Press **Play**, then select a range and verify that capture re-arms on release and you can still drag out the selected clip.
3. 選擇預設輸出資料夾，將 WAV 設為 44.1 kHz、24-bit PCM。分別單擊、框選、在視窗內拖放，以及拖到無效位置，確認輸出資料夾沒有新增 WAV。再將框選的片段拖進 DAW，確認有一個新 WAV，且規格正確。若使用插件，分別在 VST3 和 CLAP 實例操作一次。

   Choose an export folder and set the WAV format to 44.1 kHz, 24-bit PCM. Click the waveform, select a range, drag inside the window, and drop outside on an invalid target. Verify that none leave a new WAV in the export folder. Then drag a selected clip into your DAW and check that one new WAV uses the chosen format. For plug-in use, try a VST3 instance and a CLAP instance.
4. 修改一項偏好後重開桌面版，確認設定有還原。測試插件時，修改設定後先儲存 DAW 專案，再重開該專案確認。

   Change a preference and relaunch the desktop app to check that it is restored. For the plug-in, change a setting, save the DAW project, and reopen it to check the saved value.
5. 開啟 **只顯示波形**，確認波形高度維持原樣並垂直置中。按右鍵開啟 Options，測試暫停和啟用錄音，再確認框選時區間內的波形會變色。桌面版可將視窗拖到螢幕邊緣附近，確認仍停在放開的位置，再將視窗縮到最小尺寸，並從 Options 切回完整介面。

   Turn on **Show waveform only** and check that the waveform keeps its height and moves to the vertical center. Right-click to open Options and test pausing and arming capture. Check that selecting a range changes its waveform color. On desktop, move the window near a screen edge and check that it stays where you release it, then resize it to the minimum size and restore the full interface from Options.
6. 將保留時間切到 **60 秒**，並用調色盤改色。切到 **10 秒**自動續錄，框選後按 `Esc`，再用 `Backspace` 重試。每次都應立刻恢復待命，且存檔資料夾不應新增 WAV。

   Set history to **60 seconds** and change the theme color. Set auto-resume to **10 seconds**, select a range, and press `Esc`. Repeat with `Backspace`. Capture should re-arm immediately each time and the save folder should have no new WAV.
7. 移動桌面視窗，用系統匣或選單列隱藏視窗，播放音訊，再顯示並檢查波形。結束並重新開啟後，確認視窗位置有還原。若開啟了登入時自動啟動，可登出再登入確認。

   Move the desktop window, hide it from the tray or menu bar, play audio, then show it and check the waveform. Quit and reopen the app to check that the position is restored. If you enabled launch at login, sign out and back in to verify it.
8. 指定保存資料夾。錄音中按 `Ctrl+S`，確認波形持續更新，再框選一段並按 `Ctrl+S`。確認保存資料夾內分別出現完整快照和框選片段，且存下框選後已恢復待命。

   Choose a saved snapshots folder. Press `Ctrl+S` while recording and check that the waveform continues updating. Select a range and press `Ctrl+S` again. Verify that the saved snapshots folder contains the full snapshot and the selected clip, and that capture re-arms after saving the selection.

[回到專案首頁 / Back to the project README](../README.md)
