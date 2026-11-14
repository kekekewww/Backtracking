# Backtracking 操作手冊 / User Guide

[繁體中文](#繁體中文) · [English](#english) · [安裝與建置 / Installation and build](#安裝與建置--installation-and-build) · [硬體使用量 / Hardware usage](HARDWARE_USAGE.md)

本輪將編輯視野與背景擷取分開；最新規則如下，整合驗證進度見[波形編輯與介面回應](RESPONSIVE_EDITING_2026-11-14.md)。 / Editing and background capture are separated in this update; see [integration status and limits](RESPONSIVE_EDITING_2026-11-14.md).

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

Options 左側是分類，右側以一列一項的直向清單顯示設定。頁標題使用較大粗體，分組與欄位有明確層級，主要文字至少 15。保留時間、自動續錄、透明度和 WAV 格式使用下拉選單；少於 10 項時依實際項目數決定高度，最多顯示 10 列，超過則在選單內捲動。右側內容較長時向下捲動。桌面設定視窗可調整大小，底部關閉按鈕維持可見。插件會向 DAW 要求放大設定視窗，關閉後恢復原尺寸。

操作說明集中於本文；UI 保留控制、標題、欄位名稱、值與必要的狀態／錯誤／路徑。

### 保留安靜片段

在 **Options → Capture／擷取** 勾選 **Keep quiet sections**（macOS：**保留安靜片段**）。預設關閉，舊設定及沒有此欄位的插件專案也使用關閉狀態。

- **關閉**：沿用靜音壓縮，低於靜音偵測門檻的長安靜區間不占完整保留時間。
- **開啟**：保留往後收到的低音量、數位零值，以及由已知來源時鐘確認的安靜區間。安靜本身會占用 History，可能使更早的音訊較快離開保留範圍。
- 切換只影響後續擷取，不清空已保留歷史、不重切固定編輯素材／A/B 選區，也不暫停、恢復或重啟錄音。關閉時不回溯壓縮已保留片段，開啟時不補回以前已丟棄的安靜音訊。
- 手動 Pause、來源不可用及 Sender 斷線仍各自處理；沒有有效來源／時鐘時，不會只因勾選就憑空補出未知音訊或 DAW 時間資訊。

Windows 的正常連線來源可針對無封包的安靜時段作週期性 padding；macOS 若完全沒有 callback，只能等待後續真實 timestamp 才確認相應區間，不能先保證尾端即時補齊。這些平台情境仍依[本輪驗證計畫](superpowers/plans/2026-11-14-retain-quiet-sections.md)確認，不把舊版硬體／互動測試當作新選項已通過的證據。桌面偏好及各插件實例會保存此選項。

macOS 桌面版可使用滑鼠右鍵或觸控板的次要點擊開啟選單。`Control` 搭配波形上的點擊與拖曳用於框選。

設定選單的背景透明度可選 0%、25%、50%、75% 或 100%。0% 為不透明，100% 為透明。桌面版在 100% 時仍可開啟設定選單調整透明度。VST3／CLAP 的 **Panel transparency** 調整插件內的波形面板背景，實際看到的底色取決於 DAW 的視窗背景。

### 來源、波形與旗標

- 在桌面版 Options 選擇系統播放、麥克風或指定 App。Windows 的指定 App 擷取需 OS build 20348 以上。macOS 首次擷取系統聲或麥克風時，請允許對應權限。
- 每次打開來源下拉選單都會更新清單。Windows 會列出仍有播放串流的 App，包含串流已暫停的情況。macOS 會列出 Core Audio 辨識到有播放輸出的 App。找不到要選的 App 時，先讓它播放一段音訊，再重新打開選單。麥克風來源會列出系統目前可用的輸入裝置。
- 可同時啟用 A 和 B。兩路各自選擇來源與 Left、Right、Center、Mid、Side 或 Merged。Merged 沿用來源的單聲道或雙聲道輸出。Center 需要來源提供原生中央聲道。
- Windows 的 A／B 選用同一來源時，會以裝置的原始取樣率保留兩路音訊，例如 System Side／System Mid。若 WAV 設定使用不同取樣率，會在匯出整段選取時才轉換。暫停後重新開始錄音時，會平滑前後片段的接縫。
- 單一來源可顯示合併波形或上下兩列 L／R。啟用 A／B 時，上列顯示 A、下列顯示 B，各占波形區的一半，並各自顯示音量。
- 完成框選或保存整段時，旗標標記範圍頭尾。`Shift` 加左鍵拖曳可直接吸附框選；Windows 的 `Ctrl`／macOS 的 `Option` 保留自由框選，也可再加 `Shift` 吸附。目標依附近可見旗標、已畫小節線、已畫秒數線的順序，同類選最近位置；拖動中的端點不吸附自己。只吸附目前畫出的目標；全透明時參考線隱藏，旗標仍可見、可操作。
- 框選、調整旗標、縮放或平移會固定原編輯素材與視野，背景照常擷取。即時歷史循環覆寫或縮短保留時間，不會改變固定選區的原始取樣；A／B 共用起訖。完成或取消編輯後回到即時視野，手動 Pause 狀態獨立。
- 波形區間數、筆畫和參考線密度依實際音訊區域、可見時間跨度及互動狀態調整。
- 遠／中倍率以連續 min/max 填色與輪廓顯示峰值範圍，放大到個別取樣時改為連接原始 samples 的折線。最小縮放範圍依波形實際寬度決定，約讓一個 sample 占四個邏輯像素；調整視窗尺寸保留原視野，後續縮放遵守有效上下限。這些畫法不改變選區或匯出取樣；[本輪驗證紀錄](QUIET_CAPTURE_AND_WAVEFORM_2026-11-14.md)另列測試狀態。
- **Save snapshot**、全域快照快捷鍵與成功拖放會同時匯出 A／B。兩個 WAV 使用同一起訖和取樣率。若 DAW 要求指定匯入位置，可選擇分開音軌並使用相同開始位置。

[桌面版更新設定](UPDATES.md)

### 只顯示波形

開啟 **只顯示波形／Show waveform only** 後，Windows／macOS 原生桌面視窗會縮到波形面板加可見外框的高度，並關閉原生陰影。背景透明度仍依 Options 設定，可保留實色、半透明或全透明；100% 透明時，波形與旗標仍可見、可操作。插件可向 DAW 要求縮小，宿主外框、陰影與底色由 DAW 管理。這個選項預設關閉，並跟著桌面偏好設定或 DAW 專案中的插件設定儲存。

在波形上按右鍵開啟 Options。錄音已暫停時，可選 **開始錄音／Play**。錄音已待命或正在收錄時，可選 **暫停錄音／Pause**。

切換時保留波形面板高度，桌面視窗貼合面板，移除上下多餘的透明區。拖曳桌面視窗的左右內緣可調整寬度，拖曳上下內緣可調整高度。視窗變窄時，同樣的錄音保留時間會顯示在更短的距離上，波形每秒移動的像素會減少。可用高度不足時，波形會跟著縮小。取消勾選時，桌面視窗恢復切換前的完整模式尺寸。

| 桌面版 | 只顯示波形最小尺寸 | 完整介面最小尺寸 |
| --- | --- | --- |
| Windows | 240 × 48 邏輯像素 | 510 × 132 邏輯像素 |
| macOS | 240 × 48 點 | 560 × 100 點 |

要回到完整介面，在波形上按右鍵，再取消勾選 **只顯示波形／Show waveform only**。Windows／macOS 在此模式關閉原生陰影；可見細外框則由 Options 的外觀設定控制。

### 移動與縮放桌面視窗

Windows 和 macOS 桌面版採用無邊框視窗。以下手勢可自由移動或調整大小，靠近螢幕邊緣時不會自動貼齊。VST3／CLAP 的外層視窗由 DAW 管理。要關閉桌面程式，請從設定選單選擇 **結束／Quit**。

Windows 桌面版啟用 **置頂／Keep on top** 時，若工作列等同一桌面的視窗蓋住 Backtracking，程式會重新提升自身層級，並保留目前的輸入焦點。安全桌面與受保護的系統畫面由 Windows 管理。

Windows 桌面版可從工作列右側的系統匣顯示或隱藏視窗。隱藏視窗後，錄音仍可在背景繼續。要停止程式，可從系統匣或設定選擇 **Quit**。先將 `Backtracking.exe` 放在固定位置，再開啟 **Start with Windows**。Backtracking 會在登入 Windows 時開啟並還原上次的視窗位置。

macOS 桌面版可從選單列顯示、隱藏或結束 Backtracking，也可啟用登入時開啟。系統要求核准時，請在 macOS 系統設定允許 Backtracking 的登入項目。

| 動作 | Windows 桌面版 | macOS 桌面版 |
| --- | --- | --- |
| 移動視窗 | 在內容區按住 `Alt`，用滑鼠左鍵拖曳。 | 在波形外的內容區按住 `Option`，用左鍵拖曳。在內容區任意位置可按住 `Command` + `Option` 再用左鍵拖曳。 |
| 調整視窗大小 | 將游標移到視窗內容區的內緣或內角，按住左鍵拖曳。 | 將游標移到視窗內容區的內緣或內角，按住左鍵拖曳。 |
| 框選音訊 | `Ctrl` 加左鍵自由框選，或 `Shift` 加左鍵吸附框選；兩者可合用。 | `Option`／`Control` 加左鍵自由框選，或 `Shift` 加左鍵吸附框選；可再加 Shift 吸附。 |

### 讀懂波形與錄音狀態

在完整介面中，工具列的 **Play／Pause** 圖示顯示可執行的動作：**開始錄音／Start capture** 或 **暫停錄音／Pause capture**。

- **等待聲音／Waiting for sound**。錄音已待命；保留安靜片段關閉時，聲音超過靜音偵測門檻才開始收錄。開啟時可保留有效來源的已知安靜區間，來源尚不可用則另行處理。
- **錄音中／Recording**。紅色狀態燈閃爍。波形右側是最新收錄的音訊，達到保留上限後，新的音訊會逐步取代最舊的音訊。
- **已暫停／Paused**。按 **Pause** 暫停背景擷取，期間聲音略過。擷取由 **Play** 與既有自動續錄另行控制；0 秒設定下手動 Pause 維持。框選、旗標、縮放及匯出準備只固定編輯視野，不會隱含暫停或強制恢復擷取。
- **未連線／Capture unavailable**。桌面版無法擷取時，可用 **Play** 重新連線。

**保留／History／Buffer** 可選 10、20、30、60 秒或 3、5、10 分鐘，預設為 20 秒。這是即時歷史的音訊長度。預設壓縮靜音時，兩段各 5 秒、相隔一分鐘的聲音會接在一起，中間只保留偵測需要的短緩衝；開啟保留安靜片段後，已知安靜時間也占用 History。非常小聲的內容在預設模式可能低於門檻。縮短會裁掉即時歷史最舊的部分，事後加長只增加接下來可保留的長度；固定編輯素材不因此重切。

### 即時音量表

齒輪旁的音量表顯示目前輸入的音訊，預設模式是 dBFS。從齒輪選單的音量表設定切換單位。macOS 桌面版也可直接點擊讀值切換。暫停錄音或目前無可用讀值時，顯示 `—`。

| 模式 | 讀值 |
| --- | --- |
| **dBFS** | 數位取樣峰值，0 dBFS 是數位滿刻度。 |
| **VU** | 以 300 毫秒 RMS 估算平均電平。以 1 kHz、峰值 −18 dBFS 的正弦波校準為 0 VU。 |
| **LUFS** | 瞬時響度。以 K 加權和 400 毫秒滑動視窗估算。 |

響度量測參考文件。 [ITU-R BS.1770](https://www.itu.int/rec/R-REC-BS.1770-5-202311-I/en) · [EBU Tech 3341](https://tech.ebu.ch/publications/tech3341)

### 框選與拖放

在完整介面中，未框選時將游標移到波形上會預覽整段保留音訊。從波形按住左鍵拖出 Backtracking 視窗，成功放到接受 WAV 的位置後，即可匯出這整段音訊。例如保留時間設為 20 秒，錄滿時匯出 20 秒，目前只收錄 7 秒時就匯出 7 秒。完整介面和只顯示波形模式都以波形線條的反相配色標示框選區間，自由框選與 `Shift` 吸附框選皆適用。匯出時使用該區間。

| 動作 | Windows | macOS | 結果 |
| --- | --- | --- | --- |
| 進入框選模式 | 按住 `Ctrl` 自由框選，或 `Shift` 吸附框選 | 按住 `Option` 自由框選，或 `Shift` 吸附框選 | `Ctrl`／`Option` 再加 `Shift` 也可吸附。 |
| 匯入 DAW | 在波形上按住滑鼠左鍵並拖出 | 在波形上按住滑鼠左鍵並拖出 | 檔案就緒後，拖到 REAPER 時間軸、FL Studio Playlist 或其他接受 WAV 的位置。 |
| 指定起訖點 | `Ctrl` 或 `Shift` 加左鍵拖曳 | `Option` 或 `Shift` 加左鍵拖曳 | 放開後保留固定素材與選區，背景擷取維持原狀；放開修飾鍵後可拖出選取區。 |

單擊、框選和視窗內的拖曳只會操作波形。要匯出框選片段，請拖出後放到可接受 WAV 的位置，或從 Options 選擇 **Save snapshot**。取消拖曳、拖回原視窗放開或目標拒絕時，不會留下匯出檔。若 WAV 還在準備，請保持按住滑鼠再放開。DAW 拒絕拖放時，可從 Options 選擇 **Save snapshot**，再從保存資料夾手動匯入。

Backtracking 視窗有焦點時，按 `Esc` 或 `Backspace` 可取消編輯並回到即時視野，不寫檔；背景錄音保持原狀，不會解除手動 Pause。

### 保存快照

從 Options 選擇 **Save snapshot**，即可保存目前錄音器的快照。有框選時使用固定素材中的原始選段，A／B 依相同取樣邊界切割；未框選時保存觸發當下保留的整段音訊。準備和保存不隱含暫停背景錄音，也不強制解除手動 Pause。保存選段完成後可回到即時視野；全段保存於背景完成時，不會取消之後才建立的新選區。

Windows 按 Alt 加數字 1 左側的實體鍵（~／grave 鍵，不必按 Shift），macOS 使用 Fn／Globe 加同一位置的鍵，可在背景觸發快照，不必讓 Backtracking 或插件視窗取得焦點。Linux 請使用[快照快捷鍵常駐程式](../desktop/linux/README.md)。在 Options 可指定單一插件實例作為全域快照快捷鍵的目標。Backtracking 沒有綁定 `Ctrl+S`；在 DAW 中仍可用它儲存專案。

### 啟用、暫停與自動續錄

選擇 **Play** 進入錄音待命，或選擇 **Pause** 手動暫停。完整介面可使用工具列，只顯示波形時可從右鍵開啟 Options。Play 保留目前固定的編輯素材與選區；取消、普通點擊或匯出結束才回到即時視野。重新待命後，下一段收錄的音訊會接在即時歷史後方，受保留時間上限約束。

**自動續錄／Auto-resume** 的 **0、5、10 秒**選項仍保留，預設 **10 秒**。一般編輯不隱含暫停擷取：框選、旗標、縮放、平移及拖放準備固定的是素材與視野，背景仍按原狀錄音。取消、普通波形點擊或匯出結束會回到即時視野，不因此強制解除手動 Pause。擷取由 **Play** 與既有自動續錄另行控制：0 秒下手動 Pause 維持，5／10 秒依原有計時規則。待命時是否保留低音量／零值，依保留安靜片段設定。

舊版儲存的 **Off、20 秒、30 秒**會改用 **10 秒**。

### 清除目前錄音

點擊工具列的垃圾桶圖示可執行 **清除目前錄音／Clear capture／Clear captured audio**。這會清空目前的波形、選取區間與記憶體中的錄音。若錄音已待命，接下來收到的聲音會重新填入波形。先前匯出的 WAV 可從存檔資料夾取用。

### 輸出位置與檔案規格

在 Options 可各自指定並開啟兩個資料夾。成功拖放使用預設輸出資料夾，全域快照快捷鍵和 **Save snapshot** 使用保存資料夾。原先設定的匯出目的地會沿用。

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

快照保存及拖放匯出都使用目前選定的 WAV 規格，取樣率轉換在匯出時進行。桌面版的 **Left** 和 **Right** 匯出為單側有聲的立體聲，**Side** 以左側原相、右側反相匯出為立體聲，**Mid** 和 **Center** 匯出為單聲道。**Merged** 和插件沿用擷取結果的聲道數。

在 WAV 設定勾選 **全部轉成單聲道／Export every file as mono**，每個檔案都會單獨轉成單聲道。立體聲 Merged 與插件的左右聲道會取平均，Side 則輸出擷取到的 Side 訊號。A／B 仍各自匯出，選項預設關閉。回溯歷史儲存在記憶體，關閉桌面程式或移除插件前，請先存下需要的片段。

### 保存與還原設定

Windows 和 macOS 桌面版在變更後會自動保存保留時間、自動續錄、主題色、背景透明度、音量表模式、WAV 匯出設定、兩個資料夾、置頂及只顯示波形的設定，下次開啟時還原。

- **Windows**。設定儲存在 `%LOCALAPPDATA%\Backtracking\Settings`。若找不到 `LOCALAPPDATA`，則使用 `%APPDATA%\Backtracking\Settings`。程式會還原上次的視窗位置。
- **macOS**。設定儲存在系統的使用者偏好中，也會記住桌面視窗的位置與尺寸。

**VST3／CLAP** 的保留時間、自動續錄、主題色、面板透明度、音量表模式、WAV 匯出設定、兩個資料夾及只顯示波形的設定由 DAW 專案保存，每個插件實例可獨立設定。修改後請手動儲存專案。重新開啟同一個專案時，DAW 會還原各實例的設定。

### 排除問題

| 狀況 | 檢查方式 |
| --- | --- |
| 播放有聲音，但波形沒有更新 | 先確認是固定的編輯視野還是背景擷取已暫停；取消編輯可回到即時視野，手動暫停可用 **Play** 恢復。Windows 確認來源使用預設播放裝置。macOS 確認系統音訊錄製權限。插件確認音軌正在播放或監聽，且 Backtracking 的插入點收得到訊號。 |
| Windows 切換喇叭或耳機後無法擷取 | 確認新的裝置已是系統預設播放裝置，再按 **Play** 重新連線。 |
| DAW 掃描不到插件 | 確認已複製完整 `.vst3` 或 `.clap`，架構與 DAW 相容，且檔案位於 DAW 會掃描的資料夾。依下方步驟重新掃描。 |
| DAW 拒絕拖放 | 拖出視窗後保持按住滑鼠，等 WAV 準備完成，再放到可接受音訊檔的時間軸或 Playlist。也可從 Options 選擇 **Save snapshot**，再從保存資料夾手動匯入。 |
| 無法寫入 WAV | 確認波形有可用的音訊，再選擇可寫入的存檔資料夾。 |

**離線渲染**。CLAP 在宿主回報離線模式時會略過擷取。VST3 使用的 nice-plug 0.4.2 有時會回報前一次的處理模式，離線渲染仍可能進入回溯歷史。渲染前若要保留目前的素材，請先匯出。若要停止整段渲染期間的背景擷取，先將自動續錄設為 **0 秒**，再按 **Pause**，避免 5／10 秒計時續錄；只固定編輯視野不等於停止錄音。

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

Choose a category on the left, then use the vertical settings list on the right. Larger bold titles, group headings, and body/control text of at least 15 distinguish hierarchy. History, auto-resume, transparency, and WAV format use dropdowns sized to their item count, up to ten rows before scrolling. Scroll the details pane for longer content. Desktop Options can be resized and keeps Close visible. The plug-in requests a larger editor and restores its prior size when closed.

Operating instructions live in this guide; the UI keeps controls, headings, labels, values, and necessary status/error/path information.

### Keep quiet sections

Enable **Keep quiet sections** under **Options → Capture** (macOS label: **保留安靜片段**). It defaults to Off, including older settings/projects without this field.

- **Off:** existing silence compression omits long below-threshold gaps from the retained duration.
- **On:** retain subsequent quiet input, digital zeros, and silence established by a known source clock. Quiet sections consume History and can displace older audio sooner.
- Toggling changes future capture only. It preserves retained history, fixed editing material/aligned A/B ranges, and capture state. It neither retroactively compresses existing material nor restores discarded quiet audio.
- Manual Pause, unavailable sources, and disconnected Senders retain their own behavior. Enabling the option does not invent unknown audio or DAW timing without a valid source/clock.

Windows handles eligible no-packet silence with periodic padding. With no callback on macOS, a real subsequent timestamp is needed to establish the interval; immediate trailing padding is not promised. Platform validation is tracked in the [current plan](superpowers/plans/2026-11-14-retain-quiet-sections.md), separately from older benchmarks. Desktop preferences and individual plug-in instances store the option.

In the macOS desktop app, use a mouse right-click or the trackpad's secondary click to open the menu. Use `Control` with a click and drag over the waveform to select audio.

The settings menu offers five background transparency levels. Choose 0%, 25%, 50%, 75%, or 100%, where 0% is opaque and 100% is clear. On the desktop apps, you can open Settings and adjust the value even at 100%. In VST3 and CLAP, **Panel transparency** fades the waveform panel's fill. The DAW provides the background behind it.

### Sources, waveforms, and flags

- Choose system playback, a microphone, or a specific app in desktop Options. App capture requires Windows OS build 20348 or later. On macOS, grant the relevant system audio or microphone permission when prompted.
- The source list updates each time you open the drop-down. Windows lists apps with an open playback stream, including paused streams. macOS lists apps that Core Audio identifies as playback sources. If an app is missing, play some audio in it and reopen the list. Microphone entries come from the input devices the system currently makes available.
- Enable A and B to capture two routes. For each, choose a source and Left, Right, Center, Mid, Side, or Merged. Merged uses the source's mono or stereo output format. Center requires a native center channel.
- On Windows, A/B routes from the same source, such as System Side and System Mid, use the device's native sample rate. Export converts the whole selection if the WAV rate differs. Capture also smooths the splice when you resume after a pause.
- For one source, choose a merged waveform or two L/R rows. With A/B enabled, A and B each use half the waveform area and have independent meters.
- Flags mark range boundaries. `Shift`-drag directly creates a snapped selection; `Ctrl` on Windows / `Option` on macOS keeps free selection, with optional Shift for snapping. Targets prioritize visible flags, then painted bars, then painted seconds; the moving handle excludes itself. Only painted targets are eligible. Fully transparent backgrounds hide guides but retain interactive flags.
- Selecting, adjusting flags, zooming, or panning fixes the original editing material/view while background capture continues. The selected samples and aligned A/B boundaries survive live rollover or a shorter history limit. Finishing/canceling returns to live view without changing manual Pause.
- Waveform bins, strokes, and guide density adapt to the actual audio rectangle, visible duration, and interaction state.
- Wider views use a continuous filled min/max envelope and outline; close views connect the original samples with a polyline. The width-based zoom limit allows about four logical pixels per sample. Resizing preserves the view; subsequent zooming follows valid limits. Drawing does not change selected or exported samples; see the [current-round record](QUIET_CAPTURE_AND_WAVEFORM_2026-11-14.md) for validation status.
- **Save snapshot**, the global snapshot shortcut, and accepted drops export both tracks with the same boundaries and sample rate. If your DAW prompts for placement, choose separate tracks at the same start position.

[Desktop update settings](UPDATES.md)

### Show waveform only

Turn on **Show waveform only** to shrink the native Windows/macOS desktop window to the waveform panel plus its visible border and disable the native shadow. Background transparency still follows Options: opaque, translucent, or fully transparent. Waveforms and flags remain visible and interactive at 100% transparency. Plug-ins can request a smaller editor; the DAW manages its outer frame, shadow, and background. This setting defaults to Off and is saved with the desktop preferences or the plug-in's DAW project settings.

Right-click the waveform to open Options. Choose **Play** (**開始錄音** in the macOS app) when capture is paused, or **Pause** (**暫停錄音**) when capture is armed or recording.

When you switch modes, the desktop window fits the waveform panel's existing height and removes excess transparent space above and below it. Drag either inside side edge to adjust the width or the top and bottom edges to adjust the height. A narrower window shows the same retained duration over a shorter distance, reducing the number of pixels the waveform scrolls per second. The waveform shrinks to fit a shorter window. Turning the mode off restores the previous full-interface window size.

| Desktop app | Waveform-only minimum size | Full interface minimum size |
| --- | --- | --- |
| Windows | 240 × 48 logical pixels | 510 × 132 logical pixels |
| macOS | 240 × 48 points | 560 × 100 points |

To restore the full interface, right-click the waveform and turn off **Show waveform only**. Windows/macOS native shadows are disabled in this mode; the optional thin border is controlled separately in Appearance Options.

### Move and resize the desktop window

The Windows and macOS desktop apps use a frameless window. The gestures below move and resize it without automatic snapping near screen edges. The DAW manages the outer window for VST3 and CLAP. To close the desktop app, choose **Quit** from Settings.

With **Keep on top** enabled on Windows, Backtracking reasserts its position when the taskbar or another window on the same desktop covers it, while leaving input focus where it is. Windows manages the secure desktop and protected system surfaces.

Use the Windows system tray icon to show or hide the window. Capture can continue while the window is hidden. Choose **Quit** from the tray or Settings to exit. Place `Backtracking.exe` in a permanent location and enable **Start with Windows** to open Backtracking at its last window position when you sign in.

On macOS, use the menu bar item to show, hide, or quit Backtracking, or enable launch at login. If macOS asks for approval, allow the Backtracking login item in System Settings.

| Action | Windows desktop | macOS desktop |
| --- | --- | --- |
| Move the window | Hold `Alt` and left-drag inside the content area. | Hold `Option` and left-drag outside the waveform. Hold `Command` + `Option` and left-drag from anywhere in the content area. |
| Resize the window | Move the pointer to an inside edge or corner, then left-drag. | Move the pointer to an inside edge or corner, then left-drag. |
| Select audio | Ctrl-left-drag for free selection or Shift-left-drag for snapping; combine both if desired. | Option/Control-left-drag for free selection or Shift-left-drag for snapping; adding Shift also snaps. |

### Read the waveform and capture status

In the full interface, the toolbar's **Play/Pause** icon shows the available action: **Start capture** or **Pause capture**.

- **Waiting for sound**. Armed capture waits for the silence threshold when Keep quiet sections is Off. With it On, known silence from a valid source can be retained; unavailable sources are handled separately.
- **Recording**. The red indicator blinks. The newest audio appears on the right. Once the buffer fills, new audio gradually replaces the oldest samples.
- **Paused**. **Pause** stops background capture and skips incoming audio. **Play** and existing auto-resume control capture separately; manual Pause remains at the zero-second setting. Selection, flags, zoom, and export preparation fix the editing view without implicitly pausing or forcing capture to resume.
- **Capture unavailable**. If the desktop recorder loses its connection, press **Play** to retry.

**History / Buffer** offers 10/20/30/60 seconds or 3/5/10 minutes and defaults to 20 seconds. With default silence compression, two five-second sounds a minute apart join with only a short gap; Keep quiet sections also counts known silence against History. Very quiet input may fall below the default gate. Shortening trims the oldest live audio; lengthening gives future audio more room. Fixed editing material is not recut.

### Live audio meter

The meter next to the gear reads the incoming audio signal and defaults to dBFS. Choose the unit in the gear menu. In the macOS desktop app, you can also click the readout to switch units. The meter displays `—` while paused or when a reading is unavailable.

| Mode | What it shows |
| --- | --- |
| **dBFS** | Digital sample peak level. 0 dBFS is digital full scale. |
| **VU** | A 300-millisecond RMS estimate of average level. A 1 kHz sine wave with a −18 dBFS peak reads 0 VU. |
| **LUFS** | Momentary loudness estimated with K-weighting and a 400-millisecond sliding window. |

Loudness measurement references. [ITU-R BS.1770](https://www.itu.int/rec/R-REC-BS.1770-5-202311-I/en) · [EBU Tech 3341](https://tech.ebu.ch/publications/tech3341)

### Select and drag out

In the full interface, hover over an unselected waveform to preview all retained audio. Drag from anywhere on the waveform outside Backtracking onto a WAV target to export that range. For example, a full 20-second buffer exports 20 seconds; if only 7 seconds have been captured, the WAV is 7 seconds long. Both display modes invert the waveform-line color inside free or Shift-snapped selections. The selected range is used for export.

| Action | Windows | macOS | Result |
| --- | --- | --- | --- |
| Enter selection mode | Hold `Ctrl` for free selection or `Shift` for snapping | Hold `Option` for free selection or `Shift` for snapping | Adding Shift to Ctrl/Option also snaps. |
| Drop a clip into a DAW | Hold the primary mouse button on the waveform and drag out | Hold the primary mouse button on the waveform and drag out | Once ready, drop the WAV onto the REAPER timeline, FL Studio Playlist, or another WAV drop target. |
| Choose both ends | Hold `Ctrl` or `Shift` and left-drag | Hold `Option` or `Shift` and left-drag | Release to keep fixed material/selection while capture keeps its state; release modifiers to drag out the range. |

Clicking, selecting, and dragging within the window only affect the waveform. To export a selected clip, drag it out and drop it on a target that accepts WAV files, or choose **Save snapshot** in Options. Canceled or rejected drops, including drops back in the source window, leave no export file. Keep the mouse button held while an export finishes preparing. If the DAW rejects the drop, choose **Save snapshot** in Options and import the WAV from the saved snapshots folder.

With Backtracking focused, press `Esc` or `Backspace` to cancel editing and return to live view without writing a file. Background capture keeps its prior state; a manual Pause is not cleared.

### Save a snapshot

Choose **Save snapshot** in Options to save from the current recorder. A selection uses its original fixed samples with the same A/B sample boundaries. With no selection, it saves all audio retained when triggered. Saving neither implicitly pauses capture nor forces manual Pause to resume. Selected-range completion can return to live view; completion of a background full-history save does not cancel a selection created afterward.

On Windows, press Alt plus the physical key to the left of 1 (the ~ / grave key, without Shift). On macOS, press Fn/Globe plus that same physical key. The global shortcut works from the background, without focusing Backtracking or the plug-in editor. On Linux, use the [snapshot shortcut helper](../desktop/linux/README.md). Options can select one plug-in instance as the global shortcut target. Backtracking does not bind `Ctrl+S`; it remains available to save the DAW project.

### Arm, pause, and auto-resume capture

Choose **Play** to arm capture or **Pause** to pause it manually. Use the toolbar in the full interface or right-click to open Options in waveform-only mode. Play preserves fixed editing material/selection; canceling, an ordinary click, or export completion returns to live view. Once re-armed, the next sound appends to live history within the limit.

**Auto-resume** retains **0/5/10 seconds** and its **10-second** default. Editing fixes material/view without implicitly pausing capture. Canceling, an ordinary waveform click, or export completion returns live without forcing manual Pause to resume. **Play** and existing auto-resume control capture separately: manual Pause remains at zero seconds; 5/10 seconds retain their timer rules. Keep quiet sections determines whether quiet/zero input is retained while armed.

Existing **Off**, **20-second**, and **30-second** settings use the **10-second** default.

### Clear the current recording

Click the trash icon in the toolbar. It is labeled **Clear capture** on Windows desktop and **Clear captured audio** in the plug-ins. This empties the waveform, selection, and audio held in memory. If capture is armed, incoming sound starts filling the waveform again. You can find previously exported WAVs in the save folder.

### Output location and file format

You can choose and open both folders from Options. Successful drops use the default export folder. The global snapshot shortcut and **Save snapshot** use the saved snapshots folder. Existing export destinations are carried forward.

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

Snapshot saves and drag-and-drop exports use the current WAV settings. Any sample rate conversion happens during export. On desktop, **Left** and **Right** export as stereo with audio on the corresponding side, **Side** exports as stereo with an inverted right channel, and **Mid** and **Center** export as mono. **Merged** and the plug-ins use the captured channel count.

Enable **Export every file as mono** to export each WAV in mono. Stereo Merged and stereo plug-in captures average the left and right channels. Side exports the captured Side signal. A and B still export as individual files. The option defaults to Off. The rolling history lives in memory, so export any clips you need before closing the app or removing the plug-in.

### Save and restore settings

The Windows and macOS desktop apps save your history length, auto-resume delay, theme accent, background transparency, meter mode, WAV export settings, both folders, keep-on-top setting, and waveform-only mode as you change them. Those preferences are restored the next time you open the app.

- **Windows**. Settings are stored in `%LOCALAPPDATA%\Backtracking\Settings`, with `%APPDATA%\Backtracking\Settings` as a fallback when `LOCALAPPDATA` is unavailable. The app restores its previous window position.
- **macOS**. Settings are stored in the macOS user preferences. The app also restores the desktop window's size and position.

For **VST3 and CLAP**, the DAW project stores each plug-in instance's history length, auto-resume delay, theme accent, panel transparency, meter mode, WAV export settings, both folders, and waveform-only mode. Save the project manually after making changes. When you reopen that project, the DAW restores each instance's settings.

### Troubleshooting

| Symptom | What to check |
| --- | --- |
| Audio plays but the waveform stays still | Distinguish a fixed editing view from paused capture: cancel editing to return live, or use **Play** after manual Pause. On Windows, verify the default playback source. On macOS, check audio permission. In a DAW, check playback/monitoring and the signal at the insert. |
| Capture stops after switching Windows playback devices | Set the new device as the default playback device, then press **Play** to reconnect. |
| The plug-in does not appear in the DAW | Verify that the entire `.vst3` or `.clap` is in a scanned folder and that its architecture matches the DAW. Rescan using the steps below. |
| The DAW refuses the drop | Drag outside the window, keep the mouse button held while the WAV prepares, and release on an audio timeline or Playlist. You can also choose **Save snapshot** in Options and import the WAV from the saved snapshots folder. |
| The WAV could not be written | Check that the waveform contains audio and choose a writable save folder. |

**Offline rendering**. CLAP skips capture when the host reports offline mode. The nice-plug 0.4.2 VST3 wrapper can pass the previous processing mode to Backtracking, so an offline render may enter the capture history. Export important clips before rendering. To stop capture for the entire render, set auto-resume to **0 seconds**, then press **Pause**, avoiding the 5/10-second timer. A fixed editing view alone does not stop recording.

## 安裝與建置 / Installation and build

取得原始碼後，在專案根目錄執行對應指令。請使用 Rust 1.95 或更新的穩定版。

Run the commands below from the project root. Use Rust 1.95 or a newer stable version.

```sh
git clone https://github.com/kekekewww/Backtracking.git
cd Backtracking
```

### Windows 桌面版 / Windows desktop

可用 Windows Setup 安裝桌面版，分別勾選 VST3／CLAP，並各自選擇安裝的掃描根目錄。Setup 預設安裝到目前使用者的目錄；請將所選插件路徑加入 DAW 掃描設定。解除安裝會保留錄音與設定。[Setup 路徑選擇、建置與安裝說明](../desktop/windows/installer/README.md)

Use Windows Setup to install the desktop app, select VST3/CLAP independently, and choose a separate scan-root folder for each format. Defaults are per user; add your chosen plug-in roots to the DAW's scan paths. Uninstall preserves recordings and settings. [Setup path selection, build, and installation instructions](../desktop/windows/installer/README.md)

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

1. 先確認 **Keep quiet sections／保留安靜片段** 關閉，播放兩小段聲音，中間留長靜音，確認預設靜音壓縮。再開啟選項，用有效來源重試，History 設得比整段更長，確認已知安靜區間也保留。切換不應清歷史、取消固定選區或改變手動 Pause。

   With **Keep quiet sections** Off, play two short sounds separated by silence and check default compression. Turn it On and repeat with a valid source and History longer than the full interval; known silence should remain. Toggling must not clear history/fixed selections or change manual Pause.
2. 將自動續錄設為 **0 秒**，播放聲音確認紅燈閃爍。按 **Pause**，確認背景擷取停止，取消選取不應解除手動 Pause。按 **Play**，接著自由框選、`Shift` 吸附框選、拖旗標和縮放，確認編輯素材與視野固定，背景仍持續收錄。以 **5／10 秒**設定重試，分開確認既有自動續錄規則；取消編輯後應回到最新錄音。

   At **0-second** auto-resume, play audio and check the red indicator. Press **Pause** and verify capture stops; canceling must not clear manual Pause. Press **Play**, then test free selection, Shift-snapped selection, flags, and zoom while background capture continues. Repeat at **5/10 seconds**, checking existing auto-resume separately. Canceling editing should return to the latest recording.
3. 選擇預設輸出資料夾，將 WAV 設為 44.1 kHz、24-bit PCM。分別單擊、框選、在視窗內拖放，以及拖到無效位置，確認輸出資料夾沒有新增 WAV。再將框選的片段拖進 DAW，確認有一個新 WAV，且規格正確。若使用插件，分別在 VST3 和 CLAP 實例操作一次。

   Choose an export folder and set the WAV format to 44.1 kHz, 24-bit PCM. Click the waveform, select a range, drag inside the window, and drop outside on an invalid target. Verify that none leave a new WAV in the export folder. Then drag a selected clip into your DAW and check that one new WAV uses the chosen format. For plug-in use, try a VST3 instance and a CLAP instance.
4. 修改一項偏好後重開桌面版，確認設定有還原。測試插件時，修改設定後先儲存 DAW 專案，再重開該專案確認。

   Change a preference and relaunch the desktop app to check that it is restored. For the plug-in, change a setting, save the DAW project, and reopen it to check the saved value.
5. 開啟 **只顯示波形**，確認桌面視窗貼合原波形面板高度，並測試 0%／50%／100% 背景透明度；全透明時波形與旗標仍應可見、可操作。按右鍵開啟 Options，測試暫停和啟用錄音，再確認框選時區間內的波形會變色。桌面版可將視窗拖到螢幕邊緣附近，確認仍停在放開的位置，再將視窗縮到最小尺寸，並從 Options 切回完整介面，確認恢復原尺寸。

   Turn on **Show waveform only** and check that the desktop window fits the waveform panel's prior height. Test 0%/50%/100% background transparency; waveforms and flags should stay visible and interactive when fully transparent. Right-click to open Options and test pausing and arming capture. Check that selecting a range changes its waveform color. On desktop, move the window near a screen edge and check that it stays where you release it, then resize it to the minimum size and restore the full interface and its prior size from Options.
6. 將保留時間切到 **60 秒**，並用調色盤改色。框選後等待背景超出這個保留時間，再縮短 History，確認原選區與 A／B 端點仍指向固定素材；按 `Esc`，再用 `Backspace` 重試，應回到即時視野且不新增 WAV。0 秒手動 Pause 時取消選取仍應保持暫停；Play 應恢復擷取並保留編輯素材及選區。

   Set history to **60 seconds** and change the theme color. Select a range, let background capture roll past the limit, then shorten History. Original samples/A-B boundaries should still refer to fixed material. Test `Esc` and `Backspace`: both return to live view without a new WAV. At zero-second auto-resume, canceling must preserve manual Pause; Play should resume capture while keeping editing material/selection.
7. 移動桌面視窗，用系統匣或選單列隱藏視窗，播放音訊，再顯示並檢查波形。結束並重新開啟後，確認視窗位置有還原。若開啟了登入時自動啟動，可登出再登入確認。

   Move the desktop window, hide it from the tray or menu bar, play audio, then show it and check the waveform. Quit and reopen the app to check that the position is restored. If you enabled launch at login, sign out and back in to verify it.
8. 指定保存資料夾。錄音中從 Options 選擇 **Save snapshot**，確認背景持續擷取，再框選一段並再次保存。確認保存資料夾內分別出現完整快照和固定選段，A／B 使用相同起訖；另在 0 秒手動 Pause 下保存，確認不會強制解除暫停。切換到其他程式後，Windows 按 Alt 加數字 1 左側的實體鍵（~／grave 鍵，不必按 Shift），macOS 按 Fn／Globe 加同一位置的鍵，確認背景快照可用。使用插件時，從 Options 指定一個目標實例，確認全域快捷鍵只保存該實例；在 DAW 按 `Ctrl+S` 不應觸發快照。

   Choose a saved snapshots folder. Save while recording and verify capture continues. Select a range and save again; verify the full snapshot/fixed clip with aligned A/B boundaries. Save at zero-second manual Pause and verify it is not forced to resume. Switch apps and verify background saving with Alt plus the physical key left of 1 (the ~ / grave key, without Shift) on Windows, or Fn/Globe plus that key on macOS. For plug-ins, verify only the selected target is saved; `Ctrl+S` in the DAW should not trigger a snapshot.

[回到專案首頁 / Back to the project README](../README.md)
