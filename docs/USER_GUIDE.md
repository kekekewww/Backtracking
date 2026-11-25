# Backtracking 操作手冊 / User Guide

[繁體中文](#繁體中文) · [English](#english) · [安裝與建置 / Installation and build](#安裝與建置--installation-and-build) · [硬體使用量 / Hardware usage](HARDWARE_USAGE.md)

本輪將編輯視野與背景擷取分開；最新規則如下，整合驗證進度見[波形編輯與介面回應](RESPONSIVE_EDITING_2026-11-14.md)。 / Editing and background capture are separated in this update; see [integration status and limits](RESPONSIVE_EDITING_2026-11-14.md).

## 繁體中文

### 選擇擷取來源

| 我想留下的聲音 | 使用方式 |
| --- | --- |
| 瀏覽器、播放器或其他程式的聲音 | 開啟桌面版，在 Options 選擇系統播放或指定 App。 |
| 麥克風，或系統聲加麥克風 | 開啟桌面版，在 Options 選擇麥克風，或啟用 A／B 分開擷取。 |
| 某一軌、群組或主輸出的聲音 | 把 VST3 或 CLAP 插入對應的效果鏈。插件接收該位置的 mono、stereo 或環繞聲音訊（見下方「多聲道來源」），並將音訊原樣傳給下游。 |
| 軟體樂器加效果後的聲音 | 把插件放在樂器及要納入取樣的效果後面。若需要乾聲，就放在那些效果前面。 |
| 在桌面版擷取 DAW 某一軌的聲音 | 在插件 Options 的 Sources 勾選 **Send to Backtracking desktop**，再到桌面版 Options 的 Sources，把音訊來源選為 **【Sender】**加上該軌名稱。Sender 列在系統音訊和預設麥克風之後。 |

Windows 上請把要取樣的程式輸出到預設播放裝置。若 DAW 使用 ASIO、獨占模式或其他直接輸出路徑，使用音軌插件可直接取得該軌訊號。macOS 桌面版首次啟動時，請依系統提示允許錄製系統音訊。 [Windows WASAPI](https://learn.microsoft.com/en-us/windows/win32/coreaudio/loopback-recording) · [Apple Core Audio](https://developer.apple.com/documentation/coreaudio/capturing-system-audio-with-core-audio-taps)

### 開啟設定

點擊工具列的齒輪可開啟 **Options**。在插件視窗內按滑鼠右鍵也會開啟 Options；Windows／macOS 桌面版的右鍵則開啟選單，依序為 **Pause／Resume／Retry capture**、**Clear**、**Save snapshot**、**Edit separately／分開編輯**、**Show waveform only**、**Keep on top**、**Options…** 與 **Quit**。桌面版視窗有焦點時，按 `Space` 可暫停或恢復擷取，按 `Delete`（Mac 鍵盤為 `Fn` + `Delete`）清除目前錄音；有選區時，`Space` 改為試聽選區，`Delete` 則先取消選區。波形有一列以上時，單獨輕按右側 `Ctrl`（Mac 為右側 `Command`）可切換[分開編輯](#分開編輯)。這些是預設按鍵，Windows 桌面版可在 Options 的 **Hotkeys／快速鍵** 頁變更，見[快速鍵](#快速鍵windows-桌面版)。桌面版會開啟獨立的設定視窗，VST3／CLAP 則在 DAW 的插件編輯器內開啟設定。可按分類找到錄音控制、主題色、保留時間、自動續錄、背景透明度、WAV 格式及資料夾。桌面版也可直接從工具列切換 **置頂／Keep on top**。

Options 左側是分類，右側以一列一項的直向清單顯示設定。頁標題使用較大粗體，分組與欄位有明確層級，主要文字至少 15。保留時間、自動續錄、透明度和 WAV 格式使用下拉選單；少於 10 項時依實際項目數決定高度，最多顯示 10 列，超過則在選單內捲動。右側內容較長時向下捲動。桌面設定視窗可調整大小，底部關閉按鈕維持可見。插件會向 DAW 要求放大設定視窗，關閉後恢復原尺寸。

操作說明集中於本文；UI 保留控制、標題、欄位名稱、值與必要的狀態／錯誤／路徑。

### 保留安靜片段

在 **Options → Capture／擷取** 勾選 **Keep quiet sections**。預設關閉，舊設定及沒有此欄位的插件專案也使用關閉狀態。

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
- 可同時啟用 A 和 B。桌面版的 **Source B** 預設為 **Off**，選擇來源後即啟用。兩路各自選擇來源與 Left、Right、Center、Mid、Side 或 Merged。Merged 沿用來源的單聲道或雙聲道輸出。Center 需要來源提供原生中央聲道。
- Windows 的 A／B 選用同一來源時，會以裝置的原始取樣率保留兩路音訊，例如 System Side／System Mid。若 WAV 設定使用不同取樣率，會在匯出整段選取時才轉換。暫停後重新開始錄音時，會平滑前後片段的接縫。
- 單一來源可顯示合併波形或上下兩列 L／R。啟用 A／B 時，上列顯示 A、下列顯示 B，各占波形區的一半，並各自顯示音量。
- 完成框選或保存整段時，旗標標記範圍頭尾。`Shift` 加左鍵拖曳可直接吸附框選；Windows 的 `Ctrl`／macOS 的 `Option` 保留自由框選，也可再加 `Shift` 吸附。目標依附近可見旗標、已畫小節線、已畫秒數線的順序，同類選最近位置；拖動中的端點不吸附自己。只吸附目前畫出的目標；全透明時參考線隱藏，旗標仍可見、可操作。
- 框選、調整旗標、縮放或平移會固定原編輯素材與視野，背景照常擷取。即時歷史循環覆寫或縮短保留時間，不會改變固定選區的原始取樣；A／B 共用起訖。完成或取消編輯後回到即時視野，手動 Pause 狀態獨立。
- 波形區間數、筆畫和參考線密度依實際音訊區域、可見時間跨度及互動狀態調整。
- 遠／中倍率以連續 min/max 填色與輪廓顯示峰值範圍，放大到個別取樣時改為連接原始 samples 的折線。最小縮放範圍依波形實際寬度決定，約讓一個 sample 占四個邏輯像素；調整視窗尺寸保留原視野，後續縮放遵守有效上下限。這些畫法不改變選區或匯出取樣；[本輪驗證紀錄](QUIET_CAPTURE_AND_WAVEFORM_2026-11-14.md)另列測試狀態。
- **Save snapshot**、全域快照快捷鍵與成功拖放會同時匯出 A／B。兩個 WAV 使用同一起訖和取樣率。若 DAW 要求指定匯入位置，可選擇分開音軌並使用相同開始位置。開啟[分開編輯](#分開編輯)時，選區只匯出選取的列，沒有選區時拖出的是游標所在的那一列（macOS 為那一軌）。

[桌面版更新設定](UPDATES.md)

### 只顯示波形

開啟 **只顯示波形／Show waveform only** 後，Windows／macOS 原生桌面視窗會縮到波形面板加可見外框的高度，並關閉原生陰影。背景透明度仍依 Options 設定，可保留實色、半透明或全透明；100% 透明時，波形與旗標仍可見、可操作。插件可向 DAW 要求縮小，宿主外框、陰影與底色由 DAW 管理。這個選項預設關閉，並跟著桌面偏好設定或 DAW 專案中的插件設定儲存。

桌面版在波形上按右鍵開啟選單：錄音已暫停時可選 **Resume capture**，已待命或正在收錄時可選 **Pause capture**。插件則按右鍵開啟 Options，再選 **Play** 或 **Pause**。

切換時保留波形面板高度，桌面視窗貼合面板，移除上下多餘的透明區。拖曳桌面視窗的左右內緣可調整寬度，拖曳上下內緣可調整高度。視窗變窄時，同樣的錄音保留時間會顯示在更短的距離上，波形每秒移動的像素會減少。可用高度不足時，波形會跟著縮小。取消勾選時，桌面視窗恢復切換前的完整模式尺寸。

| 桌面版 | 只顯示波形最小尺寸 | 完整介面最小尺寸 |
| --- | --- | --- |
| Windows | 240 × 48 邏輯像素 | 510 × 132 邏輯像素 |
| macOS | 240 × 48 點 | 560 × 100 點 |

要回到完整介面，在波形上按右鍵，再取消勾選 **只顯示波形／Show waveform only**。Windows／macOS 在此模式關閉原生陰影；可見細外框則由 Options 的外觀設定控制。Windows／macOS 桌面版與外掛還可以在 Options 的 Appearance 頁選擇外框寬度（1 至 4 px），並在 **Show** 區決定要顯示哪些項目：工具列上的程式名稱、音量表，以及擷取狀態標記（**Always**、**Only when paused or failing**（外掛為 **Only when paused**）或 **Never**）。選擇 **Never** 後，擷取失敗時視窗上也不會出現標記。

### 移動與縮放桌面視窗

Windows 和 macOS 桌面版採用無邊框視窗。以下手勢可自由移動或調整大小，靠近螢幕邊緣時不會自動貼齊。VST3／CLAP 的外層視窗由 DAW 管理。要關閉桌面程式，請從設定選單選擇 **結束／Quit**。

Windows 桌面版啟用 **置頂／Keep on top** 時，若工作列等同一桌面的視窗蓋住 Backtracking，程式會重新提升自身層級，並保留目前的輸入焦點。安全桌面與受保護的系統畫面由 Windows 管理。

Windows 桌面版可從工作列右側的系統匣顯示或隱藏視窗。隱藏視窗後，錄音仍可在背景繼續。要停止程式，可從系統匣或設定選擇 **Quit**。先將 `Backtracking.exe` 放在固定位置，再開啟 **Start with Windows**。Backtracking 會在登入 Windows 時開啟並還原上次的視窗位置。

macOS 桌面版可從選單列顯示或隱藏視窗、暫停或恢復擷取、開啟 Options 或結束 Backtracking，也可啟用登入時開啟（**Start at login**）。系統要求核准時，請在 macOS 系統設定允許 Backtracking 的登入項目。

| 動作 | Windows 桌面版 | macOS 桌面版 |
| --- | --- | --- |
| 移動視窗 | 在內容區按住 `Alt`，用滑鼠左鍵拖曳。 | 在波形外的內容區按住 `Option`，用左鍵拖曳。在內容區任意位置可按住 `Command` + `Option` 再用左鍵拖曳。 |
| 調整視窗大小 | 將游標移到視窗內容區的內緣或內角，按住左鍵拖曳。 | 將游標移到視窗內容區的內緣或內角，按住左鍵拖曳。 |
| 框選音訊 | `Ctrl` 加左鍵自由框選，或 `Shift` 加左鍵吸附框選；兩者可合用。 | `Option`／`Control` 加左鍵自由框選，或 `Shift` 加左鍵吸附框選；可再加 Shift 吸附。 |
| 分開編輯 | 單獨輕按右側 `Ctrl`（預設，可在 Options → **Hotkeys** 變更）。 | 單獨輕按右側 `Command`。 |

### 讀懂波形與錄音狀態

在完整介面中，工具列的 **Play／Pause** 圖示顯示可執行的動作：**開始錄音／Start capture** 或 **暫停錄音／Pause capture**。

- **等待聲音／Waiting for sound**。錄音已待命；保留安靜片段關閉時，聲音超過靜音偵測門檻才開始收錄。開啟時可保留有效來源的已知安靜區間，來源尚不可用則另行處理。
- **錄音中／Recording**。紅色狀態燈閃爍。波形右側是最新收錄的音訊，達到保留上限後，新的音訊會逐步取代最舊的音訊。
- **已暫停／Paused**。按 **Pause** 暫停背景擷取，期間聲音略過。擷取由 **Play** 與既有自動續錄另行控制；0 秒設定下手動 Pause 維持。框選、旗標、縮放及匯出準備只固定編輯視野，不會隱含暫停或強制恢復擷取。
- **未連線／Capture unavailable**。桌面版無法擷取時，可用 **Play** 重新連線。

**保留／History／Buffer** 可選 10、20、30、60 秒或 3、5、10 分鐘，預設為 20 秒。這是即時歷史的音訊長度。預設壓縮靜音時，兩段各 5 秒、相隔一分鐘的聲音會接在一起，中間只保留偵測需要的短緩衝；開啟保留安靜片段後，已知安靜時間也占用 History。非常小聲的內容在預設模式可能低於門檻。縮短會裁掉即時歷史最舊的部分，事後加長只增加接下來可保留的長度；固定編輯素材不因此重切。

### 依頻率為波形上色（Windows 桌面版）

在 Options 的 **Appearance** 頁，把 **波形顏色／Waveform color** 設為 **依頻率／By frequency**，波形的每個時刻會依它含有的聲音上色：低頻偏向第一個顏色，高頻偏向最後一個。預設由低頻的紅色到高頻的藍色，所以低沉的嗡聲偏紅、鈸聲偏藍，完整的混音落在兩者之間。預設的 **單一顏色／Single color** 照舊以一種顏色繪製。

**頻率顏色／Frequency colors** 顯示漸層，並為每個顏色列一行，由低到高。按顏色可以更改它；拖曳中間顏色的滑桿，可在相鄰兩色之間移動它；**移除／Remove** 拿掉一個中間顏色；**新增顏色／Add color** 在最寬的間隔加入一個。漸層有 2 到 8 個顏色，**重設／Reset** 恢復預設。在波形背景上不易看清的顏色，會在它的位置改用預設顏色繪製，下方也會註明。

有選區時，選區保留它的顏色，其餘波形變暗並偏灰，讓選區更明顯。

附加元件的外觀（見「附加元件」）也可以提供這組顏色。只有在開啟依頻率、而且你沒有自己的顏色時才會使用，顏色下方會註明來自哪個附加元件。更改其中任何一個顏色，它們就以外觀的顏色為起點成為你自己的；之後按 **重設／Reset** 會改回外觀的顏色。附加元件無法替你開啟依頻率上色。

從目前原始碼建置的 macOS 版也可設定頻率著色與漸層，詳見下方的 macOS 桌面版說明。

### 多個來源（Windows 桌面版）

Windows 桌面版可以同時擷取 A、B 以外的更多來源，最多 64 個，各占一條波形列，依序以 A、B、C……Z、AA……BL 命名。

- 在 Options 的 **Sources** 頁，每個來源各有一段，標題為 **Source A**、**Source B** 等，下方是與以往相同的音訊來源、聲道路由與檔名欄位。清單最下方的 **新增來源／Add source** 選單列出可用來源，選擇後會加在清單最後。已有 64 個來源時，這個選單無法使用，並說明原因。
- 來源超過一個時，每段標題右側有 **移除／Remove**。移除後，後面的來源會往前遞補一個字母，例如移除 B 後，原本的 C 變成 B，它的音量表單位也跟著移動。只剩一個來源時不能移除。來源 B 之後的每個來源也可在音訊來源選單選 **Off**，效果與移除相同。目前不能調整來源順序。
- 新增、移除或更換來源會重新開始記錄歷史，頁面底部也會註明。
- 所有來源合計最多 64 個聲道。頁面底部顯示目前使用了幾個（**N of 64 channels in use／已使用 N 個聲道，上限 64 個**）。超過上限的來源無法開始，原因會寫在該來源那一段，正常視窗的狀態列也會顯示，並以該來源的字母開頭；波形左側的字母改為警示色。
- 保留歷史的記憶體由所有聲道共用（電腦記憶體的八分之一，介於 256 MiB 與 2 GiB 之間）。聲道越多，能保留的時間越短；放不下所選的保留時間時，**Capture** 頁和 **Sources** 頁會寫出實際能保留多久，例如 **With 64 channels, memory holds the latest 2 minutes 54 seconds.**。
- 超過兩個來源時，波形左側顯示每個來源的字母；多聲道來源在字母旁再列出各聲道名稱。將游標移到字母上，會顯示該來源是什麼與它的狀態。
- 列數多到在視窗內每列放不下 14 像素時，整個波形區會一起上下捲動：在波形左右邊緣或右側捲軸上捲動滑鼠滾輪，或拖曳捲軸。在波形中間捲動仍是縮放。框選、旗標、拖出與[分開編輯](#分開編輯)在捲動後的列上照常使用；分開編輯時拖到波形上下緣會自動捲動。視窗會先長高，最多到 560 像素，放不下時才捲動。
- 工具列只顯示放得下的音量表，其餘收在 **+N** 按鈕；按下它會列出每個來源的音量表，按其他地方或 `Esc` 關閉。
- 匯出、拖出與快照為每個來源各寫一個 WAV 檔，檔名含該來源的字母，例如 `Backtracking_Spotify_C_….wav`。
- VST3／CLAP 插件維持原有的來源數量。

### macOS 桌面版（從原始碼建置）

目前原始碼的 macOS 版在 Options → **Sources** 提供 **Add source** 和 **Remove**，最多 64 個來源，合計最多 64 個聲道。來源從 A 編到 BL。使用 **All channels** 可保留多聲道來源，波形顯示立體聲 L/R 混合列及個別聲道。波形列放不下時，在左右邊緣捲動可瀏覽其他列。**Edit separately** 可選單一列，L/R 混合列視為一組。

Options → **Capture** 可選多聲道來源的 L/R 聲音為立體聲或雙耳音訊。Options → **Export** 可選每聲道一個雙耳或單聲道檔、單一多聲道 WAV，或單一雙耳混音。Options → **Appearance** 的 **Waveform color** 可切換頻率著色並調整漸層。

Options → **Hotkeys** 可為每個動作設定兩組快捷鍵。全域快照預設用 Fn／Globe 加數字 1 左側的實體鍵，需要「輸入監控」權限。分開編輯預設輕按右側 Command，可用 **Reset** 還原。

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

Backtracking 視窗有焦點時，按 `Esc` 或 `Backspace` 可取消編輯並回到即時視野，不寫檔；背景錄音保持原狀，不會解除手動 Pause。Windows 桌面版的這兩個鍵是預設值，可在[快速鍵](#快速鍵windows-桌面版)變更。

### 試聽選區與淡入淡出

有選區且 Backtracking 視窗有焦點時，按 `Space` 會從選區起點播放到終點，播放線在波形上由左往右移動；再按一次 `Space` 即停止。沒有選區時，`Space` 仍用於暫停或恢復擷取。取消選區、重新框選、移動旗標、保存或拖出選區時，播放都會停止；按住 `Alt` 調整淡化則不會。試聽依匯出方式擺放 A／B 軌，並套用 **全部轉成單聲道**。

試聽期間，所有擷取系統播放的 Backtracking 錄音器都會錄入靜音，試聽內容不會進入歷史。麥克風、單一應用程式與插件傳送端照常錄音。插件的試聽從插件所在的軌道輸出，宿主處理音訊時才會播放；部分 DAW 停止走帶時不處理音訊。試聽是該軌輸出的一部分，因此訊號鏈後段的 Backtracking 或錄音會收到它，試聽期間進行的匯出或 bounce 也會包含它。

按住 `Alt`（macOS 為 `Option`）拖曳選區起點旗標可調整淡入，拖曳終點旗標可調整淡出。往選區內拖得越深，淡化越長；淡入與淡出不會重疊。拖曳時波形會依淡化收窄，完整介面也會顯示淡化長度。淡化為線性，頭尾從無聲開始與結束。拖出的 WAV 與保存的選區快照都會套用淡化；試聽中調整，接下來播放的聲音就會套用新的淡化。重新框選會清除淡化；移動旗標時淡化保留，選區變短時會隨之縮短。

### 分開編輯

可以只框選、試聽、淡化、拖出或保存部分波形列：A、B 其中一軌，立體聲來源分成 L／R 兩列時的其中一側，或多聲道來源的部分聲道。在波形上按右鍵選擇 **Edit separately／分開編輯**，或在 Backtracking 視窗有焦點時單獨輕按一下右側 `Ctrl`（Mac 為右側 `Command`）；再選一次或再輕按一次，就回到全部一起編輯。這個模式預設關閉，結束程式後不保留。此功能需要畫面上有兩列以上的波形。按住超過半秒，或同時按下其他按鍵或滑鼠按鈕，都不算輕按。

Windows 版開啟這個模式後，選區涵蓋從開始拖曳的那一列到游標所在的那一列：往上或往下拖可納入更多列，拖回來就再放掉。拖過可捲動軌道的上緣或下緣時，軌道會跟著捲動。多聲道軌道最上方的 L 與 R 兩列一律一起選取。macOS 版的選區則屬於開始拖曳的那一軌。

| 動作 | 一起編輯（預設） | 分開編輯 |
| --- | --- | --- |
| 框選 | 選區涵蓋所有列 | 選區只涵蓋拖過的列（macOS：開始拖曳的那一軌），只有這些列變色 |
| 試聽（`Space`） | 依匯出方式播放全部 | 只播放選取的列 |
| 淡入淡出 | 套用到全部 | 只套用到選取的列 |
| 拖出選區 | 匯出每一軌 | 只匯出選取的列 |
| 未框選時拖出 | 匯出 A 和 B 的整段音訊 | 只匯出游標所在那一列（macOS：那一軌）的整段音訊 |
| **Save snapshot**／全域快照快捷鍵 | 保存 A 和 B | 有選區時只保存選取的列；沒有選區時仍保存 A 和 B |

一次只有一個選區，選取的列共用旗標，旗標畫在選取的列上方。完整介面的狀態列與長度標籤會寫出選取的列，例如「Selection · C–Ls」；包含 L 與 R 兩列時寫成「L/R」。切換模式不會取消選區：關閉時選區恢復涵蓋所有列，再開啟時又縮回原先拖過的列（macOS：開始框選的那一軌）。框選、拖出或調整淡化進行中時，切換不會生效。

### 保存快照

從 Options 選擇 **Save snapshot**，即可保存目前錄音器的快照。有框選時使用固定素材中的原始選段，A／B 依相同取樣邊界切割；開啟分開編輯時只保存選取的列。未框選時保存觸發當下保留的整段音訊。準備和保存不隱含暫停背景錄音，也不強制解除手動 Pause。保存選段完成後可回到即時視野；全段保存於背景完成時，不會取消之後才建立的新選區。

Windows 按 Alt 加數字 1 左側的實體鍵（~／grave 鍵，不必按 Shift），macOS 使用 Fn／Globe 加同一位置的鍵，可在背景觸發快照，不必讓 Backtracking 或插件視窗取得焦點。Linux 請使用[快照快捷鍵常駐程式](../desktop/linux/README.md)。在 Options 可指定單一插件實例作為全域快照快捷鍵的目標。Windows 的這個快捷鍵是預設值，可在[快速鍵](#快速鍵windows-桌面版)變更。Backtracking 預設沒有綁定 `Ctrl+S`；在 DAW 中仍可用它儲存專案。

### 快速鍵（Windows 桌面版）

Options 的 **Hotkeys／快速鍵** 頁列出所有鍵盤快速鍵，都可以變更。點一下動作旁的快速鍵按鈕，按鈕會顯示「Press keys…／請按下按鍵…」，接著按下想用的按鍵組合即可；按 `Esc` 會停止且不變更。每個動作最多兩組快速鍵，**Add another／再新增一組** 可加入第二組。**Clear／清除** 移除該動作的快速鍵，**Reset／重設** 恢復預設，頁面底部的 **Reset all／全部重設** 恢復全部預設。變更立即生效並保存，不必重新啟動。

| 動作 | 預設 |
| --- | --- |
| 從任何地方儲存快照（全系統） | `Alt` 加數字 1 左側的鍵 |
| 暫停或繼續擷取 | `Space` |
| 清除擷取內容 | `Delete` |
| 取消選取 | `Esc`、`Backspace` |
| 分開編輯 | 單獨輕按右側 `Ctrl` |
| 儲存快照、只顯示波形、保持在最上層、開啟 Options、結束 | 未設定 |

全系統快速鍵需要搭配 `Ctrl` 或 `Alt`，在其他程式位於前景時也有效；錄製新按鍵期間會暫時停用，以免觸發。其他快速鍵只在 Backtracking 視窗位於前景時有效。有選區時，暫停擷取的按鍵改為試聽選區，清除的按鍵改為先取消選區。與其他動作重複的按鍵會被拒絕，原本的設定不變，該列下方會說明原因。少數按鍵組合無法使用，因為 Windows 或視窗本身要用，例如 `Alt` + `F4`、縮放用的 `Ctrl` + `=`、`Ctrl` + `-`、`Ctrl` + `0`，以及 `Shift` + `Tab`。輕按右 `Ctrl` 無法錄製，分開編輯另有 **Use a tap of Right Ctrl／改用輕按右 Ctrl** 按鈕。`Esc` 用於停止錄製，要把 `Esc` 設回取消選取請按 **Reset／重設**。Win 鍵與複製、剪下、貼上的按鍵無法錄製。另開的擷取視窗也有這一頁，但變更只套用到該視窗，不會保存。滑鼠手勢（`Ctrl`／`Shift`／`Alt` 拖曳、滾輪縮放或在列數放不下的軌道左右邊緣捲動列、中鍵拖曳平移、右鍵選單）固定不變，列在頁面底部供參考。

### 啟用、暫停與自動續錄

選擇 **Play** 進入錄音待命，或選擇 **Pause** 手動暫停。完整介面可使用工具列，只顯示波形時，桌面版可用右鍵選單或 `Space`，插件可從右鍵開啟 Options。Play 保留目前固定的編輯素材與選區；取消、普通點擊或匯出結束才回到即時視野。重新待命後，下一段收錄的音訊會接在即時歷史後方，受保留時間上限約束。

**自動續錄／Auto-resume** 的 **0、5、10 秒**選項仍保留，預設 **10 秒**。一般編輯不隱含暫停擷取：框選、旗標、縮放、平移及拖放準備固定的是素材與視野，背景仍按原狀錄音。取消、普通波形點擊或匯出結束會回到即時視野，不因此強制解除手動 Pause。擷取由 **Play** 與既有自動續錄另行控制：0 秒下手動 Pause 維持，5／10 秒依原有計時規則。待命時是否保留低音量／零值，依保留安靜片段設定。

舊版儲存的 **Off、20 秒、30 秒**會改用 **10 秒**。

### 清除目前錄音

點擊工具列的垃圾桶圖示可執行 **清除目前錄音／Clear capture／Clear captured audio**。這會清空目前的波形、選取區間與記憶體中的錄音。若錄音已待命，接下來收到的聲音會重新填入波形。先前匯出的 WAV 可從存檔資料夾取用。

### 輸出位置與檔案規格

在 Options 可各自指定並開啟兩個資料夾。成功拖放使用預設輸出資料夾，全域快照快捷鍵和 **Save snapshot** 使用保存資料夾。原先設定的匯出目的地會沿用。Windows／macOS 桌面版與外掛可在 Options 的 Export 頁用 **Delete old drag-and-drop files** 讓 Backtracking 自動刪除預設輸出資料夾裡超過 10、20、30、60 或 90 天的拖放檔，預設為 Off。只會永久刪除 Backtracking 自己命名的 WAV；保存資料夾裡的快照不會被刪除，兩個資料夾相同時也不會刪除任何檔案。仍在專案中直接引用這些檔案的 DAW 會找不到它們。外掛會把這項設定存在專案裡，而且只在外掛視窗開啟時清理。

| 版本 | 預設輸出資料夾 | 保存資料夾的預設位置 |
| --- | --- | --- |
| macOS 桌面版 | `~/Backtracking/output/` | `~/Backtracking/saved/` |
| Windows 桌面版 | `%USERPROFILE%\Backtracking\output\` | `%USERPROFILE%\Backtracking\saved\` |
| VST3／CLAP | 使用者家目錄下的 `Backtracking/output/` | 使用者家目錄下的 `Backtracking/saved/` |

取樣率和編碼可在 Options 的 Export 頁獨立調整。三個版本的選項名稱相同：**WAV sample rate** 與 **WAV encoding**。

| WAV 設定 | 選項 | 預設 |
| --- | --- | --- |
| 取樣率 | 44.1 kHz、48 kHz、96 kHz | 48 kHz |
| 編碼 | 16-bit PCM、24-bit PCM、32-bit IEEE float | 32-bit float |

快照保存及拖放匯出都使用目前選定的 WAV 規格，取樣率轉換在匯出時進行。桌面版的 **Left** 和 **Right** 匯出為單側有聲的立體聲，**Side** 以左側原相、右側反相匯出為立體聲，**Mid** 和 **Center** 匯出為單聲道。**Merged** 和插件沿用擷取結果的聲道數；插件擷取超過兩個聲道時，依「多聲道來源」的匯出設定寫出檔案。

在 WAV 設定勾選 **全部轉成單聲道／Export every file as mono**，每個檔案都會單獨轉成單聲道。立體聲 Merged 與插件的左右聲道會取平均，Side 則輸出擷取到的 Side 訊號。A／B 仍各自匯出，選項預設關閉。回溯歷史儲存在記憶體，關閉桌面程式或移除插件前，請先存下需要的片段。

**檔名**。桌面版以來源名稱為 WAV 命名。同時擷取 A 和 B 時為 `Backtracking_<名稱>_A_<日期>_<時間>_<長度>s.wav` 與 `Backtracking_<名稱>_B_…`；只有一個來源時不加 A／B，例如 `Backtracking_Spotify_2026-11-17_09-05-00_12.4s.wav`。分開編輯 A 和 B 時單獨匯出的一軌仍標示 A 或 B。名稱預設為來源清單中的裝置或 App 名稱，不含 `.exe` 等副檔名；系統播放為 `System audio`，Sender 為音軌名稱。要改名時，在 Options → Sources 該來源下方的 **File name** 欄位輸入，欄位下方會顯示檔名的開頭；清空欄位即恢復預設。名稱屬於裝置而不是 A／B：把系統播放命名為 `Loop` 後，不論它在 A 還是 B 擷取，檔名都使用 `Loop`。檔名不能使用的字元（`\ / : * ? " < > |`）會改成空格，名稱最多保留 48 個字元。VST3／CLAP 的檔名不含來源名稱，為 `Backtracking_<日期>_<時間>_<長度>s.wav`，DAW 提供速度時在長度後加上 BPM，例如 `_120bpm`；擷取 A 和 B 時，兩個檔案分別以 `-A`、`-B` 結尾。**Delete old drag-and-drop files** 也認得舊版的檔名。

### 附加元件（Windows 桌面版）

附加元件（add-on）是獨立的程式，Backtracking 會啟動它並擷取它送出的音訊。附加元件可以提供新的音訊來源、開啟檔案、有自己的設定與按鈕。附加元件檔案的副檔名是 `.btaddon`。目前只有 Windows 桌面版支援，macOS 桌面版與插件尚未支援。

**安裝。** 把 `.btaddon` 檔拖到 Backtracking 視窗，或在 Options 的 **Add-ons** 頁按 **Install from file…**。Backtracking 先顯示確認畫面：名稱、版本、作者、它會執行的程式、解壓後的大小，以及它要求的功能。按 **Install** 之前不會寫入任何東西。附加元件是以你的權限執行的程式，沒有沙箱，請只安裝你信任的附加元件。裝好後它會立刻啟動，之後每次開啟 Backtracking 也會啟動。

**管理。** **Add-ons** 頁列出每個附加元件的狀態、最新訊息、設定與按鈕。取消勾選 **Enabled** 可停用。**Remove…** 會移除程式，並可選擇一併刪除它保存的資料。附加元件裝在 `%LOCALAPPDATA%\Backtracking\addons`，它的資料在 `%LOCALAPPDATA%\Backtracking\addon-data`。

**使用附加元件的來源。** 附加元件提供的來源會出現在 Options 的 **Sources** 頁的 **Audio source** 清單。選了這種來源後：

- **Format** 取代 **Channel route**，列出這個附加元件提供的格式（例如 Stereo、5.1、7.1.4）。一到兩個聲道的格式仍可選擇聲道路徑。
- 能開啟檔案的附加元件會多一個 **Open file…** 按鈕，也可以直接把檔案拖到視窗上。
- 能播放的來源會有 **Play**／**Pause**、**Stop** 和位置滑桿，主視窗的波形左下角也有一個播放／暫停按鈕。

**使用附加元件的外觀。** 具有 `appearance` 功能的附加元件可以為視窗提供一套外觀：波形、選區、背景、外框、旗標等的顏色，以及外框寬度、圓角和波形線條的粗細。外觀預設不會套用：到 Options 的 **Appearance** 頁，在 **附加元件的外觀／Look from add-on** 選擇一個正在提供外觀的執行中附加元件。一次只會套用一個外觀，選 **無／None** 即恢復 Backtracking 自己的外觀。這個選擇會被保存；若該附加元件沒有執行或目前沒有提供外觀，視窗會使用自己的外觀，這一列也會註明它目前沒有提供外觀，等它再次提供時就會自動套用。外觀只會改變視窗的樣子：擷取、選取、匯出、檔案、快捷鍵、版面（外框寬度除外）、文字，以及錄音、等待、暫停與錯誤的標記都維持原樣。外觀啟用期間，它的外框寬度會取代你設定的寬度（視窗大小不變，波形區域隨之調整），但你自己選的外框顏色仍優先於外觀的外框顏色，背景透明度也仍依你的設定。若某個值會讓波形、選區、旗標、播放線、聲道名稱或擷取狀態標記難以看清，Backtracking 會忽略那個值，改用自己的。

想自己寫附加元件，請看 [`ADDON_CONTRACT.md`](ADDON_CONTRACT.md)。

### 讓附加元件控制 Backtracking（Windows 桌面版）

具有 `control` 功能的附加元件可以像坐在視窗前的人一樣操作 Backtracking，例如讓 AI 助理或腳本替你暫停、選取並存檔。

- 預設不允許。在 Options 的 **Add-ons** 頁，這種附加元件會多一個勾選框 **允許此附加元件控制 Backtracking（開始與暫停錄音、選取、儲存檔案、變更來源）／Let this add-on control Backtracking (start and pause recording, select, save files, change sources)**。未勾選時，它的每個要求都會被拒絕；若它曾經要求過，勾選框下方會註明 **它要求控制 Backtracking，已被拒絕／It asked to control Backtracking and was refused**。安裝確認畫面也會列出這項功能。
- 允許後，它可以讀取 Backtracking 目前的狀態（是否在擷取、各來源、保留的長度、選區、目前顯示的範圍、存檔資料夾），開始、暫停與清除擷取，框選一段、選擇分開編輯的列、縮放，把選區存成 WAV 檔，試聽選區，以及新增或移除來源。
- 存檔等同把選區拖出視窗：寫出的檔案和 **Save** 對同一個選區寫出的相同，附加元件會收到每個檔案的路徑、來源字母、聲道數、取樣率與長度。
- 新增或移除來源和在 **Sources** 頁操作一樣，會重新開始記錄歷史，原本保留的音訊會消失。需要的部分請先存檔。
- 附加元件動手時，狀態列會註明是誰做的，例如 **已開始擷取 · 由 Example control 操作／Capture started · by Example control**；它框選的選區顯示為 **Selection · 由 Example control 操作**。
- 你正在拖曳、拖出或存檔時，附加元件的要求會被告知「忙碌中」，不會打斷你。
- 它不能切換自己或其他附加元件的這個勾選框，不能安裝、移除、啟用或停用附加元件，不能更改其他設定、開啟檔案、結束 Backtracking 或更新。
- 隨時取消勾選即可收回；還在等待的要求會被拒絕。這個選擇依附加元件分別保存，重新開啟 Backtracking 後仍有效；移除附加元件後就會忘記，重新安裝時又是不允許。

### 讓 AI 助理使用 Backtracking（Windows 桌面版）

**Backtracking Agent** 附加元件讓 Claude Code、Claude Desktop 或其他 MCP 用戶端透過上述的控制使用 Backtracking：讀取 Backtracking 保留的內容、暫停與開始錄音、框選一段、存成 WAV 檔、試聽，以及新增或移除來源。安裝檔請從 [`addons/backtracking-agent`](../addons/backtracking-agent/README.md) 建置。

1. 像其他附加元件一樣安裝 `backtracking-agent.btaddon`。
2. 在 **Add-ons** 頁勾選 Backtracking Agent 的 **允許此附加元件控制 Backtracking／Let this add-on control Backtracking**。勾選之前，用戶端可以連上，但每個要求都會被拒絕，附加元件的區塊也會這樣註明。
3. 按 **Copy Claude Code command**，在終端機執行複製的指令；或按 **Copy Claude Desktop entry**，把它加進 Claude Desktop 的設定。兩者都會以 `--stdio` 啟動附加元件的程式，它會自己找到執行中的附加元件。

- 用戶端操作時，Backtracking 必須開著，而且這個附加元件要啟用。否則用戶端會立刻收到說明。
- 附加元件只在這台電腦上（`127.0.0.1`）服務，而且要有它的權杖（token）。以 `--stdio` 啟動的用戶端會自己讀取權杖；需要 `http://` 位址的用戶端，可按 **Show connection details** 顯示權杖 2 分鐘，按 **New token** 可換一個新的。
- 透過用戶端新增或移除來源，和上述一樣會重新開始記錄歷史。用戶端做的每件事，狀態列都會註明 **· 由 Backtracking Agent 操作／· by Backtracking Agent**。
- 附加元件區塊的文字只有英文。

### 多聲道來源（Windows 桌面版與插件）

**擷取系統、應用程式或麥克風的所有聲道。** 系統播放、單一應用程式或麥克風的裝置超過兩個聲道時（例如 5.1 或 7.1 喇叭），Options → Sources 的 **Channel route** 會多一個 **All channels／所有聲道**，選了之後每個聲道各成一列，聲道名稱來自 Windows 的喇叭配置（`FL`、`FR`、`FC`、`LFE` 等）。裝置只有一或兩個聲道時不會列出這個選項；之前選過、但裝置現在只有立體聲時，Backtracking 改用合併聲道擷取，並在來源下方說明。只有一或兩個聲道的來源仍是一列或 L／R 兩列，不會變成多聲道軌道。

**在 VST3／CLAP 插件中擷取環繞聲。** 把 VST3 插件插在 5.1 至 9.1.6 的環繞聲匯流排上，插件會擷取每個聲道，每個聲道各成一列，名稱來自 DAW 的喇叭配置；7.1.4 依序為 `FL`、`FR`、`FC`、`LFE`、`Lb`、`Rb`、`Ls`、`Rs`、`TFL`、`TFR`、`TBL`、`TBR`。名稱沿用 Windows 的寫法：喇叭配置同時有後方與側邊兩對時，後方一對標為 `Lb`、`Rb`，側邊一對標為 `Ls`、`Rs`。CLAP 也能擷取 6 到 16 個聲道，但 CLAP 不提供喇叭配置，聲道只以編號標示，檔名為 `-Ch01_Channel 1.wav` 這類形式；這些聲道沒有方向，混成 L 與 R 或雙耳音訊時左右一樣大聲。插件的多聲道波形、**L/R of multichannel sources** 與 **Multichannel sources** 和下文相同，兩個設定都隨 DAW 專案保存，舊專案使用預設值。與桌面版不同的地方：

- 插件一律顯示並匯出所有聲道，沒有 **Hide silent channels／隱藏無聲的聲道**、聲道群組切換與分開編輯，所以保存與拖放一律匯出所有列，並寫出 L/R 檔案。
- 出現多聲道軌道時，插件會向 DAW 要求較高的視窗；DAW 讓視窗維持較小時，用滾輪捲動各列。
- Capture 頁的 **Preview of multichannel sources** 預設為 **In their own channels**。當主輸入與主輸出的聲道數和喇叭配置都與錄製時相同，每個聲道會從匯流排的對應聲道播放，由 DAW 監聽。配置不同或選擇 **As L/R** 時，會從前兩個輸出聲道播放 Stereo 或 Binaural 混音；單聲道輸出則合併左右兩聲道。
- 同時擷取側鏈時（**Both**），側鏈仍在下方，以標為 **B** 的一列顯示，並照舊匯出為 `-B` 結尾的檔案；多聲道軌道的檔案則在長度後加上 `-A`。
- 多聲道軌道的檔名不含 BPM。

**多聲道波形。** 超過兩個聲道的來源，最上方是 **L** 與 **R** 兩列：所有聲道混成立體聲的結果，下方以細線分隔。再往下每個聲道各佔一列並標上聲道名稱；7.1.4 就是 L、R 加 12 列。列數放不下時可用滾輪捲動。來源若有多個聲道群組（例如聲道與物件），波形左下角可切換目前顯示的群組；L 與 R 一律由聲道群組混成。多聲道來源的音量表只有 Peak 與 VU，沒有 LUFS。聲道越多，記憶體能保留的歷史越短；實際保留的長度會顯示在 Options 的 **Capture** 頁。

**隱藏無聲的聲道。** Options → **Capture** 的 **Hide silent channels／隱藏無聲的聲道**（預設開啟）讓系統、應用程式與麥克風來源在 L 與 R 下方只顯示有聲音的聲道：保留的歷史中只要有樣本高於 -90 dBFS，該聲道就算有聲音。除了前左與前右以外都沒有聲音時，只顯示 L 與 R 兩列。聲道開始有聲音時，那一列就出現；它的聲音從保留的歷史中移出後，那一列才隱藏。附加元件的來源一律顯示所有聲道。隱藏的聲道仍會擷取。

**L 與 R 的聲音。** Options → **Capture** 的 **L/R of multichannel sources／多聲道來源的 L/R** 可選 **Stereo／立體聲**（預設）或 **Binaural／雙耳**。立體聲依每個聲道的位置把它混到左右；雙耳把每個聲道放在頭部周圍它的喇叭方向，用耳機就聽得出哪個聲道在哪裡。這個設定只改變試聽與匯出的聲音，波形相同。

**試聽。** 按 `Space` 也能試聽多聲道來源：播放選區涵蓋的聲道依上述設定混成的聲音，分開編輯時只播放選取的列。帶有位置資料的物件（例如附加元件送出的物件）會依位置移動，以雙耳方式播放，與匯出相同。

**匯出多聲道片段。** 在 Options 的 **Export** 頁，**Multichannel sources／多聲道來源** 決定多聲道來源寫成哪些檔案：

| 選項 | 寫出的檔案 |
| --- | --- |
| **Binaural, one file per channel／雙耳音訊，每聲道一個檔案**（預設） | 每個聲道一個雙聲道 WAV，結尾為 `-Ch01_聲道名稱_binaural.wav`，用耳機就聽得出那個聲道在哪裡 |
| **Mono, one file per channel／單聲道，每聲道一個檔案** | 每個聲道一個單聲道 WAV，結尾為 `-Ch01_聲道名稱.wav` |
| **One multichannel file／一個多聲道檔案** | 一個依原本聲道順序包含所有聲道的 WAV |
| **One binaural mix／一個雙耳混音檔案** | 一個雙聲道 WAV：整個聲道群組時結尾為 `-Binaural.wav`，部分聲道或含物件時為 `-Mix-Binaural.wav` |

以前選過單聲道或多聲道檔案的人，設定會保留。每個聲道的雙耳檔案相加，就等於同一段的雙耳混音。

匯出整條多聲道軌道時（沒有分開編輯，或選取了所有列），另外寫一個 L 與 R 的檔案：**L/R of multichannel sources／多聲道來源的 L/R** 為立體聲時是 `-Stereo.wav`，為雙耳時是 `-Binaural.wav`；與上表的檔案相同時只寫一次。分開編輯時只匯出選取的列，選取 L 與 R 兩列就寫這個 L/R 檔案。只會匯出目前顯示的聲道群組。開啟 **Hide silent channels／隱藏無聲的聲道** 時，系統、應用程式與麥克風軌道整條匯出只寫畫面上顯示的聲道（有聲音的聲道）和 L/R 檔案；除了前左與前右都沒有聲音時，只寫 L/R 檔案。

附加元件可以為一組聲道送出位置資料，例如電影音軌中的物件。這些聲道在兩種雙耳選項中會依位置移動；其中在整個片段都沒有位置資料的聲道無法定位，會寫成單聲道檔案，狀態列會說明一次。匯出的內容含有帶位置資料的聲道時，旁邊會多一個 `-Positions.json`，記錄這些聲道在片段中的位置與時間，以片段的第一個取樣為 0。

檔名開頭與其他來源相同，依[檔名](#輸出位置與檔案規格)的規則包含來源名稱（A／B 分開擷取時還有 A 或 B），依名稱排序即為聲道順序，例如 `Backtracking_Test tone_2026-11-17_09-05-00_12.4s-Ch01_L_binaural.wav`。附加元件的來源同樣可在 Options → Sources 的 **File name** 欄位改名。淡入淡出會套用到每個檔案。

### 保存與還原設定

Windows 和 macOS 桌面版在變更後會自動保存保留時間、自動續錄、主題色、背景透明度、音量表模式、WAV 匯出設定、兩個資料夾、置頂、只顯示波形的設定及各來源的檔名，下次開啟時還原。分開編輯 A 和 B 不保存，每次開啟都是關閉。

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
| One DAW track, a bus, or the master output | Insert the VST3 or CLAP plug-in on the relevant effects chain. It captures mono, stereo or surround audio at that point (see Multichannel sources below) and passes the input through unchanged. |
| A software instrument with effects | Insert Backtracking after the instrument and any effects you want in the sample. Move it ahead of those effects to capture the dry signal. |
| One DAW track inside the desktop recorder | In the plug-in, open Options → Sources and tick **Send to Backtracking desktop**. Then open desktop Options → Sources and choose **【Sender】** followed by the track name as the audio source. Senders are listed right after system audio and the default microphone. |

On Windows, route the source to the default playback device. If a DAW uses ASIO, exclusive mode, or another direct output path, use the plug-in to sample its track signal. On macOS, grant system audio recording permission when the desktop app first prompts you. [Windows WASAPI](https://learn.microsoft.com/en-us/windows/win32/coreaudio/loopback-recording) · [Apple Core Audio](https://developer.apple.com/documentation/coreaudio/capturing-system-audio-with-core-audio-taps)

### Open settings

Click the gear in the toolbar to open **Options**. Right-clicking inside the plug-in also opens Options. In the Windows and macOS desktop apps, right-clicking opens a menu instead: **Pause / Resume / Retry capture**, **Clear**, **Save snapshot**, **Edit separately**, **Show waveform only**, **Keep on top**, **Options…**, and **Quit**. With the desktop window focused, `Space` pauses or resumes capture and `Delete` (`Fn` + `Delete` on a Mac keyboard) clears the current recording. With a selection, `Space` previews the selection instead and `Delete` cancels the selection first. With more than one waveform row, tapping the right `Ctrl` key on its own (the right `Command` key on a Mac) switches [Edit separately](#edit-separately) on or off. These are the default keys; in the Windows desktop app you can change them on the **Hotkeys** page of Options (see [Hotkeys](#hotkeys-windows-desktop)). The desktop app opens a separate window. VST3 and CLAP open Options inside the DAW's plug-in editor. The controls are grouped by category, including capture, theme accent, history length, auto-resume, transparency, WAV format, and folders. On desktop, you can also toggle **Keep on top** from the toolbar.

Choose a category on the left, then use the vertical settings list on the right. Larger bold titles, group headings, and body/control text of at least 15 distinguish hierarchy. History, auto-resume, transparency, and WAV format use dropdowns sized to their item count, up to ten rows before scrolling. Scroll the details pane for longer content. Desktop Options can be resized and keeps Close visible. The plug-in requests a larger editor and restores its prior size when closed.

Operating instructions live in this guide; the UI keeps controls, headings, labels, values, and necessary status/error/path information.

### Keep quiet sections

Enable **Keep quiet sections** under **Options → Capture**. It defaults to Off, including older settings/projects without this field.

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
- Enable A and B to capture two routes. In the desktop apps, **Source B** is **Off** until you choose a source for it. For each, choose a source and Left, Right, Center, Mid, Side, or Merged. Merged uses the source's mono or stereo output format. Center requires a native center channel.
- On Windows, A/B routes from the same source, such as System Side and System Mid, use the device's native sample rate. Export converts the whole selection if the WAV rate differs. Capture also smooths the splice when you resume after a pause.
- For one source, choose a merged waveform or two L/R rows. With A/B enabled, A and B each use half the waveform area and have independent meters.
- Flags mark range boundaries. `Shift`-drag directly creates a snapped selection; `Ctrl` on Windows / `Option` on macOS keeps free selection, with optional Shift for snapping. Targets prioritize visible flags, then painted bars, then painted seconds; the moving handle excludes itself. Only painted targets are eligible. Fully transparent backgrounds hide guides but retain interactive flags.
- Selecting, adjusting flags, zooming, or panning fixes the original editing material/view while background capture continues. The selected samples and aligned A/B boundaries survive live rollover or a shorter history limit. Finishing/canceling returns to live view without changing manual Pause.
- Waveform bins, strokes, and guide density adapt to the actual audio rectangle, visible duration, and interaction state.
- Wider views use a continuous filled min/max envelope and outline; close views connect the original samples with a polyline. The width-based zoom limit allows about four logical pixels per sample. Resizing preserves the view; subsequent zooming follows valid limits. Drawing does not change selected or exported samples; see the [current-round record](QUIET_CAPTURE_AND_WAVEFORM_2026-11-14.md) for validation status.
- **Save snapshot**, the global snapshot shortcut, and accepted drops export both tracks with the same boundaries and sample rate. If your DAW prompts for placement, choose separate tracks at the same start position. With [Edit separately](#edit-separately) on, a selection exports its selected rows only, and dragging out without a selection exports the row under the pointer (the lane, in the macOS app).

[Desktop update settings](UPDATES.md)

### Show waveform only

Turn on **Show waveform only** to shrink the native Windows/macOS desktop window to the waveform panel plus its visible border and disable the native shadow. Background transparency still follows Options: opaque, translucent, or fully transparent. Waveforms and flags remain visible and interactive at 100% transparency. Plug-ins can request a smaller editor; the DAW manages its outer frame, shadow, and background. This setting defaults to Off and is saved with the desktop preferences or the plug-in's DAW project settings.

In the desktop apps, right-click the waveform to open the menu. Choose **Resume capture** when capture is paused, or **Pause capture** when capture is armed or recording. In the plug-in, right-click to open Options and choose **Play** or **Pause**.

When you switch modes, the desktop window fits the waveform panel's existing height and removes excess transparent space above and below it. Drag either inside side edge to adjust the width or the top and bottom edges to adjust the height. A narrower window shows the same retained duration over a shorter distance, reducing the number of pixels the waveform scrolls per second. The waveform shrinks to fit a shorter window. Turning the mode off restores the previous full-interface window size.

| Desktop app | Waveform-only minimum size | Full interface minimum size |
| --- | --- | --- |
| Windows | 240 × 48 logical pixels | 510 × 132 logical pixels |
| macOS | 240 × 48 points | 560 × 100 points |

To restore the full interface, right-click the waveform and turn off **Show waveform only**. Windows/macOS native shadows are disabled in this mode; the optional thin border is controlled separately in Appearance Options. The desktop apps and the plug-in also let you set the border width (1 to 4 px) on the Appearance page of Options, and choose what is shown under **Show**: the app name in the toolbar, the level meters, and the capture status mark (**Always**, **Only when paused or failing** (**Only when paused** in the plug-in), or **Never**). With **Never**, nothing on the window marks a capture failure either.

### Move and resize the desktop window

The Windows and macOS desktop apps use a frameless window. The gestures below move and resize it without automatic snapping near screen edges. The DAW manages the outer window for VST3 and CLAP. To close the desktop app, choose **Quit** from Settings.

With **Keep on top** enabled on Windows, Backtracking reasserts its position when the taskbar or another window on the same desktop covers it, while leaving input focus where it is. Windows manages the secure desktop and protected system surfaces.

Use the Windows system tray icon to show or hide the window. Capture can continue while the window is hidden. Choose **Quit** from the tray or Settings to exit. Place `Backtracking.exe` in a permanent location and enable **Start with Windows** to open Backtracking at its last window position when you sign in.

On macOS, use the menu bar item to show or hide the window, pause or resume capture, open Options, or quit Backtracking, or enable **Start at login**. If macOS asks for approval, allow the Backtracking login item in System Settings.

| Action | Windows desktop | macOS desktop |
| --- | --- | --- |
| Move the window | Hold `Alt` and left-drag inside the content area. | Hold `Option` and left-drag outside the waveform. Hold `Command` + `Option` and left-drag from anywhere in the content area. |
| Resize the window | Move the pointer to an inside edge or corner, then left-drag. | Move the pointer to an inside edge or corner, then left-drag. |
| Select audio | Ctrl-left-drag for free selection or Shift-left-drag for snapping; combine both if desired. | Option/Control-left-drag for free selection or Shift-left-drag for snapping; adding Shift also snaps. |
| Edit separately | Tap the right `Ctrl` key on its own (the default; change it in Options → **Hotkeys**). | Tap the right `Command` key on its own. |

### Read the waveform and capture status

In the full interface, the toolbar's **Play/Pause** icon shows the available action: **Start capture** or **Pause capture**.

- **Waiting for sound**. Armed capture waits for the silence threshold when Keep quiet sections is Off. With it On, known silence from a valid source can be retained; unavailable sources are handled separately.
- **Recording**. The red indicator blinks. The newest audio appears on the right. Once the buffer fills, new audio gradually replaces the oldest samples.
- **Paused**. **Pause** stops background capture and skips incoming audio. **Play** and existing auto-resume control capture separately; manual Pause remains at the zero-second setting. Selection, flags, zoom, and export preparation fix the editing view without implicitly pausing or forcing capture to resume.
- **Capture unavailable**. If the desktop recorder loses its connection, press **Play** to retry.

**History / Buffer** offers 10/20/30/60 seconds or 3/5/10 minutes and defaults to 20 seconds. With default silence compression, two five-second sounds a minute apart join with only a short gap; Keep quiet sections also counts known silence against History. Very quiet input may fall below the default gate. Shortening trims the oldest live audio; lengthening gives future audio more room. Fixed editing material is not recut.

### Color the waveform by frequency (Windows desktop)

On the **Appearance** page of Options, set **Waveform color** to **By frequency** to color each moment of the waveform by the sound it holds: low frequencies toward the first color, high frequencies toward the last. The default runs from red for low to blue for high, so a low hum reads red, a hi-hat blue, and a full mix lands in between. **Single color**, the default, draws the waveform in one color as before.

**Frequency colors** shows the gradient and one row for each color, lowest first. Click a color to change it, drag a middle color's slider to move it between its neighbors, **Remove** takes a middle color out, and **Add color** puts a new one in the widest gap. A gradient has 2 to 8 colors. **Reset** brings back the default. A color too hard to see on the waveform's background is drawn in the default color at its place, and a note under the colors says so.

While there is a selection, the selected part keeps its colors and the rest of the waveform dims toward gray, so the selection stands out.

A look from an add-on (see Add-ons) can supply these colors. They are used only while By frequency is on and you have no colors of your own, and a note under the colors names the add-on. Changing any of them makes them your own, starting from the look's; **Reset** then goes back to the look's colors. An add-on cannot turn By frequency on for you.

The macOS app built from the current source also has frequency coloring and editable gradient stops. See the macOS desktop section below.

### More than two sources (Windows desktop)

The Windows desktop app can capture more sources than A and B at once, up to 64. Each gets its own lane, lettered in order A, B, C … Z, AA … BL.

- On the **Sources** page of Options, each source has a section headed **Source A**, **Source B** and so on, with the same Audio source, Channel route and File name fields as before. Below the list, the **Add source** menu lists the sources you can pick; the one you choose is added at the end. With 64 sources the menu is unavailable and says why.
- While there is more than one source, each heading has **Remove** at its right. The sources after it move up a letter: remove B and C becomes B, taking its meter unit with it. The only source left cannot be removed. Choosing **Off** for any source after A does the same as Remove. Sources cannot be reordered yet.
- Adding, removing or changing a source starts a new history; the foot of the page says so.
- All sources together hold at most 64 channels. The foot of the page shows how many are in use (**N of 64 channels in use**). A source past the limit cannot start: the reason shows in its own section, and on the status line of the full window, led by its letter. Its letter beside the waveform turns amber.
- The memory for history is shared by every channel (one eighth of the computer's memory, from 256 MiB to 2 GiB). The more channels, the shorter the history; when the chosen length does not fit, the **Capture** and **Sources** pages say how much does, for example **With 64 channels, memory holds the latest 2 minutes 54 seconds.**
- With more than two sources, each lane's letter shows at the left of the waveform; a multichannel source lists its channel names beside its letter. Hover over a letter to see which source it is and how it is doing.
- When there are more rows than fit at 14 pixels each, the whole waveform scrolls up and down as one: turn the wheel over its left margin or the scroll bar on the right, or drag the bar. The wheel over the waveform itself still zooms. Selecting, flags, dragging out and [Edit separately](#edit-separately) work on scrolled rows; while editing separately, a drag past the top or bottom edge scrolls. The window first grows taller, up to 560 pixels, and scrolls only past that.
- The toolbar shows the meters that fit and gathers the rest behind a **+N** button. Click it to see every source's meter; click elsewhere or press `Esc` to close the list.
- Exports, drags and snapshots write a WAV file for each source, named with its letter, for example `Backtracking_Spotify_C_….wav`.
- The VST3 and CLAP plug-ins keep their original source counts.

### macOS desktop from source

The macOS app built from the current source has **Add source** and **Remove** in Options → **Sources**. It accepts up to 64 sources and 64 channels in total, with lanes lettered A through BL. **All channels** preserves a multichannel source. Its waveform has a stereo L/R pair and one row per channel. Scroll over either edge to see rows that do not fit. **Edit separately** selects an individual row and treats the L/R pair as one target.

Options → **Capture** sets whether the wide L/R pair sounds stereo or binaural. Options → **Export** offers one binaural or mono file per channel, one multichannel WAV, or one binaural mix. In Options → **Appearance**, choose **By frequency** under **Waveform color** to enable and edit the gradient.

Options → **Hotkeys** lets you set up to two shortcuts per action. The default system-wide snapshot shortcut uses Fn/Globe and the physical key left of 1 and requires Input Monitoring permission. **Reset** restores the Right Command tap for **Edit separately**.

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

With Backtracking focused, press `Esc` or `Backspace` to cancel editing and return to live view without writing a file. Background capture keeps its prior state; a manual Pause is not cleared. In the Windows desktop app these are the default keys and can be changed under [Hotkeys](#hotkeys-windows-desktop).

### Preview a selection and fade its ends

With a selection on screen and the Backtracking window focused, press `Space` to play the selection from its start flag to its end flag. A playhead moves across the waveform from left to right. Press `Space` again to stop. Without a selection, `Space` still pauses or resumes capture. Playback also stops when you cancel the selection, draw a new one, move a flag, or save or drag out the selection. Fading with `Alt` does not stop it. The preview places lanes A and B the way their WAVs are exported and follows **Export every file as mono**.

While a preview plays, every Backtracking recorder that captures System playback records silence, so the preview never enters the history. Microphones, single apps, and plug-in senders record as usual. In the plug-in, the preview plays through the plug-in's track while the host processes audio. Some DAWs stop processing audio while their transport is stopped. The preview is part of the track's output, so a Backtracking or a recording further down the signal path keeps it, and so does an export or bounce made while it plays.

Hold `Alt` (`Option` on a Mac) and drag the selection's start flag to fade it in, or its end flag to fade it out. The further into the selection you drag, the longer the fade. A fade in and a fade out never overlap. The waveform tapers as you drag, and the full interface shows the fade's length. Fades are linear and start and end in silence. Dragged-out WAVs and saved snapshots of the selection include the fades. A fade changed during a preview applies to the audio that plays next. A new selection starts without fades. Moving a flag keeps them and shortens them if the selection becomes too short.

### Edit separately

You can select, preview, fade, drag out or save some waveform rows on their own: one of the lanes A and B, one side of a stereo source shown as L and R rows, or some channels of a multichannel source. Right-click the waveform and choose **Edit separately**, or, with the Backtracking window focused, tap the right `Ctrl` key on its own (the right `Command` key on a Mac). Choose it or tap again to edit everything together. The mode is off by default and is not kept when you quit. It needs more than one waveform row on screen. Holding the key for more than half a second, or pressing another key or a mouse button with it, is not a tap.

On Windows, while the mode is on, a selection covers the rows from the one you start dragging in to the one under the pointer: drag up or down to take in more rows, and back to let them go again. Drag past the top or bottom of a lane that scrolls to scroll it. The L and R rows on top of a multichannel lane are always taken together. In the macOS app a selection covers the row you start dragging in or the L/R pair.

| Action | Together (default) | Separately |
| --- | --- | --- |
| Select | The selection covers every row | The selection covers the rows you drag across (macOS: the starting row or L/R pair), and only those rows are highlighted |
| Preview (`Space`) | Plays everything as it exports | Plays the selected rows only |
| Fade in and out | Applies to everything | Applies to the selected rows only |
| Drag out a selection | Exports every lane | Exports the selected rows only |
| Drag out without a selection | Exports all of A and B | Exports all of the row under the pointer (macOS: the starting row or L/R pair) |
| **Save snapshot** / global snapshot shortcut | Saves A and B | Saves the selected rows; without a selection it still saves A and B |

There is one selection at a time, and its rows share the flags, which stand on top of the selected rows. The status line and the length label of the full interface name the rows, for example "Selection · C–Ls", and the L and R rows of a multichannel lane as "L/R". Switching keeps the selection: turning the mode off widens it to every row, and turning it on again narrows it back to the rows you dragged across (macOS: the lane it began in). Switching does nothing while you are selecting, dragging out or fading.

### Save a snapshot

Choose **Save snapshot** in Options to save from the current recorder. A selection uses its original fixed samples with the same A/B sample boundaries; with Edit separately on, it saves the selected rows only. With no selection, it saves all audio retained when triggered. Saving neither implicitly pauses capture nor forces manual Pause to resume. Selected-range completion can return to live view; completion of a background full-history save does not cancel a selection created afterward.

On Windows, press Alt plus the physical key to the left of 1 (the ~ / grave key, without Shift). On macOS, press Fn/Globe plus that same physical key. The global shortcut works from the background, without focusing Backtracking or the plug-in editor. On Linux, use the [snapshot shortcut helper](../desktop/linux/README.md). Options can select one plug-in instance as the global shortcut target. On Windows this shortcut is the default and can be changed under [Hotkeys](#hotkeys-windows-desktop). By default Backtracking does not bind `Ctrl+S`; it remains available to save the DAW project.

### Hotkeys (Windows desktop)

The **Hotkeys** page of Options lists every keyboard shortcut, and each one can be changed. Click the shortcut button beside an action; it shows **Press keys…**. Then press the keys you want. `Esc` stops without a change. An action can have two shortcuts: **Add another** records the second. **Clear** removes an action's shortcuts, **Reset** restores its default, and **Reset all** at the bottom restores every default. Changes apply at once and are saved; there is no need to restart.

| Action | Default |
| --- | --- |
| Save snapshot from anywhere (system-wide) | `Alt` plus the key left of 1 |
| Pause or resume capture | `Space` |
| Clear capture | `Delete` |
| Cancel selection | `Esc`, `Backspace` |
| Edit separately | A tap of the right `Ctrl` key on its own |
| Save snapshot, Show waveform only, Keep on top, Open Options, Quit | Not set |

The system-wide shortcut needs `Ctrl` or `Alt` and works while another app is in front; it is paused while you record a new one, so it does not fire. The other shortcuts work while the Backtracking window is in front. With a selection, the key that pauses capture plays the selection instead, and the key that clears cancels the selection first. A key another action already uses is refused: the old shortcut stays and the row says why. A few key combinations cannot be used because Windows or the window itself uses them, such as `Alt` + `F4`, the zoom keys `Ctrl` + `=`, `Ctrl` + `-` and `Ctrl` + `0`, and `Shift` + `Tab`. A tap of Right Ctrl cannot be recorded, so Edit separately has a **Use a tap of Right Ctrl** button. `Esc` stops recording, so to give `Esc` back to Cancel selection, use **Reset**. The Windows key and the copy, cut and paste keys cannot be recorded. An extra capture window has this page too, but its changes apply to that window only and are not kept. The mouse gestures (`Ctrl`, `Shift` and `Alt` drags, the wheel, which zooms or, over the side margins of a track whose rows do not fit, scrolls its rows, middle-drag pan, right-click menu) are fixed; the bottom of the page lists them.

### Arm, pause, and auto-resume capture

Choose **Play** to arm capture or **Pause** to pause it manually. Use the toolbar in the full interface. In waveform-only mode, use the right-click menu or `Space` in the desktop apps, or right-click to open Options in the plug-in. Play preserves fixed editing material/selection; canceling, an ordinary click, or export completion returns to live view. Once re-armed, the next sound appends to live history within the limit.

**Auto-resume** retains **0/5/10 seconds** and its **10-second** default. Editing fixes material/view without implicitly pausing capture. Canceling, an ordinary waveform click, or export completion returns live without forcing manual Pause to resume. **Play** and existing auto-resume control capture separately: manual Pause remains at zero seconds; 5/10 seconds retain their timer rules. Keep quiet sections determines whether quiet/zero input is retained while armed.

Existing **Off**, **20-second**, and **30-second** settings use the **10-second** default.

### Clear the current recording

Click the trash icon in the toolbar. It is labeled **Clear capture** in the desktop apps and **Clear captured audio** in the plug-ins. This empties the waveform, selection, and audio held in memory. If capture is armed, incoming sound starts filling the waveform again. You can find previously exported WAVs in the save folder.

### Output location and file format

You can choose and open both folders from Options. Successful drops use the default export folder. The global snapshot shortcut and **Save snapshot** use the saved snapshots folder. Existing export destinations are carried forward. In the desktop apps and the plug-in, **Delete old drag-and-drop files** on the Export page of Options lets Backtracking delete dragged files in the default export folder once they are older than 10, 20, 30, 60, or 90 days. It is Off by default. Only WAV files that Backtracking named are deleted, permanently. Snapshots in the saved snapshots folder are never deleted, and nothing is deleted while both settings name the same folder. A DAW project that still reads one of those files in place will lose it. The plug-in keeps this choice with the project and only cleans while its window is open.

| Edition | Default export folder | Default saved snapshots folder |
| --- | --- | --- |
| macOS desktop | `~/Backtracking/output/` | `~/Backtracking/saved/` |
| Windows desktop | `%USERPROFILE%\Backtracking\output\` | `%USERPROFILE%\Backtracking\saved\` |
| VST3 / CLAP | `Backtracking/output/` in your home directory | `Backtracking/saved/` in your home directory |

Choose the sample rate and encoding independently on the Export page of Options. All three editions use the same names: **WAV sample rate** and **WAV encoding**.

| WAV setting | Options | Default |
| --- | --- | --- |
| Sample rate | 44.1 kHz, 48 kHz, 96 kHz | 48 kHz |
| Encoding | 16-bit PCM, 24-bit PCM, 32-bit IEEE float | 32-bit float |

Snapshot saves and drag-and-drop exports use the current WAV settings. Any sample rate conversion happens during export. On desktop, **Left** and **Right** export as stereo with audio on the corresponding side, **Side** exports as stereo with an inverted right channel, and **Mid** and **Center** export as mono. **Merged** and the plug-ins use the captured channel count; a plug-in capture of more than two channels writes the files chosen under Multichannel sources.

Enable **Export every file as mono** to export each WAV in mono. Stereo Merged and stereo plug-in captures average the left and right channels. Side exports the captured Side signal. A and B still export as individual files. The option defaults to Off. The rolling history lives in memory, so export any clips you need before closing the app or removing the plug-in.

**File names**. The desktop apps name each WAV after its source. With A and B recording, the files are `Backtracking_<name>_A_<date>_<time>_<length>s.wav` and `Backtracking_<name>_B_…`. With one source the letter is left out, as in `Backtracking_Spotify_2026-11-17_09-05-00_12.4s.wav`. A lane exported on its own while editing A and B separately keeps its letter. The name defaults to the device or app name in the source list, without an extension such as `.exe`: `System audio` for system playback, and the track name for a Sender. To change it, type in the **File name** field under the source in Options → Sources. The note under the field shows how the files will begin. Clear the field to go back to the default. A name belongs to the device, not to A or B: name system playback `Loop` and its files say `Loop` whether it records on A or on B. Characters that file names cannot hold (`\ / : * ? " < > |`) become spaces, and a name keeps at most 48 characters. VST3 and CLAP files carry no source name: `Backtracking_<date>_<time>_<length>s.wav`, with the tempo after the length, as in `_120bpm`, when the DAW reports one. With A and B recording, the two files end in `-A` and `-B`. **Delete old drag-and-drop files** also recognizes the names that earlier versions used.

### Add-ons (Windows desktop)

An add-on is a separate program that Backtracking starts and captures audio from. An add-on can provide new audio sources, open files, and have its own settings and buttons. Add-on files end in `.btaddon`. Only the Windows desktop app hosts add-ons today; the macOS app and the plug-ins do not.

**Install.** Drop a `.btaddon` file on a Backtracking window, or press **Install from file…** on the **Add-ons** page of Options. Backtracking first shows a confirmation: the name, version, author, the program it will run, its unpacked size, and what it asks for. Nothing is written before you press **Install**. An add-on is a program that runs with your own permissions and is not sandboxed, so install only add-ons you trust. Once installed it starts at once, and again each time Backtracking starts.

**Manage.** The **Add-ons** page lists each add-on with its state, its latest message, its settings, and its buttons. Clear **Enabled** to turn one off. **Remove…** removes the program and offers to delete the data it saved as well. Add-ons are installed in `%LOCALAPPDATA%\Backtracking\addons`; their data is in `%LOCALAPPDATA%\Backtracking\addon-data`.

**Use an add-on's source.** Sources that add-ons provide appear in the **Audio source** list on the **Sources** page of Options. With such a source selected:

- **Format** takes the place of **Channel route** and lists the formats the add-on offers, for example Stereo, 5.1, or 7.1.4. Formats with one or two channels keep the channel route as well.
- An add-on that opens files shows an **Open file…** button. You can also drop the file on a window.
- A source that plays shows **Play** / **Pause**, **Stop**, and a position slider. The main window has a small play/pause button at the bottom left of the waveform.

**Use an add-on's look.** An add-on with the `appearance` capability can offer a look for the window: colors for the waveform, the selection, the backgrounds, the border, the flags and more, and the border width, corner rounding and the thickness of the waveform's line. A look is off until you choose it: on the **Appearance** page of Options, pick a running add-on that offers one under **Look from add-on**. Only one look is on at a time, and **None** brings back Backtracking's own look. The choice is saved. When that add-on is not running or offers no look, the window shows its own look and the row says the add-on is not offering one right now; its look comes back when it does. A look changes only how the window looks: capture, selection, export, files, shortcuts, layout (other than the border width), text, and the recording, waiting, paused and error marks stay as they are. While a look is on, its border width replaces yours (the window keeps its size and the waveform area adjusts), but a border color you chose yourself still wins over the look's, and your background transparency still applies. Backtracking ignores any value that would make the waveform, the selection, the flags, the playhead, channel names or the capture status hard to see, and uses its own there instead.

To write an add-on, see [`ADDON_CONTRACT.md`](ADDON_CONTRACT.md).

### Let an add-on control Backtracking (Windows desktop)

An add-on with the `control` capability can work Backtracking the way a person at the window does, so that an AI assistant or a script can, for example, pause, select and save for you.

- It is off until you allow it. On the **Add-ons** page of Options such an add-on has a check box, **Let this add-on control Backtracking (start and pause recording, select, save files, change sources)**. While it is clear, every request from the add-on is refused; when the add-on has asked, the line under the box says **It asked to control Backtracking and was refused**. The install confirmation lists the capability too.
- Once allowed, it can read what Backtracking is doing (whether it is capturing, the sources, how much is held, the selection, what the window shows, the save folder), start, pause and clear capture, select a stretch, choose the rows to edit separately, zoom, save the selection as WAV files, play the selection, and add or remove sources.
- Saving is the add-on's way to drag a clip out of the window: it writes the files **Save** writes for the same selection, and the add-on is told each file's path, lane letter, channel count, sample rate and length.
- Adding or removing a source starts a new history, as it does on the **Sources** page: the audio held until then is gone. Save what you want to keep first.
- While an add-on acts, the status line says which one, for example **Capture started · by Example control**; a selection it made reads **Selection · by Example control**.
- While you are dragging, dragging out or saving, its requests are told Backtracking is busy rather than interrupting you.
- It cannot change its own check box or another add-on's, install, remove, enable or disable add-ons, change other options, open files, quit Backtracking, or update it.
- Clear the box at any time to take control back; requests still waiting are refused. The choice is kept for each add-on when Backtracking restarts, and forgotten when the add-on is removed: installed again, it starts off.

### Let an AI agent use Backtracking (Windows desktop)

The **Backtracking Agent** add-on lets Claude Code, Claude Desktop or another MCP client use Backtracking through the control described above. The client can read what Backtracking holds, pause and start recording, select a stretch, save it as WAV files, play it, and add or remove sources. Build the package from [`addons/backtracking-agent`](../addons/backtracking-agent/README.md).

1. Install `backtracking-agent.btaddon` as any add-on.
2. On the **Add-ons** page, switch on **Let this add-on control Backtracking** under Backtracking Agent. Until you do, the client can connect, but every request is refused, and the card says so.
3. Press **Copy Claude Code command** and run it in a terminal, or press **Copy Claude Desktop entry** and add it to Claude Desktop's configuration. Both start the add-on's program with `--stdio`, which finds the running add-on by itself.

- Backtracking must be running with the add-on on whenever the client works. If it is not, the client is told so at once.
- The add-on serves the client only on this computer (`127.0.0.1`), and only with its token. Clients started with `--stdio` read the token themselves; **Show connection details** shows it for 2 minutes for clients that need the `http://` address, and **New token** replaces it.
- Adding or removing a source through the client starts a new history, as above. The status line says **· by Backtracking Agent** for everything the client does.
- The add-on's card is in English only.

### Multichannel sources (Windows desktop and plug-ins)

**Capture every channel of system, app or microphone audio.** When the device behind system playback, a single app or a microphone has more than two channels (5.1 or 7.1 speakers, say), **Channel route** in Options → Sources offers **All channels**. With it, every channel gets its own row, named after the Windows speaker layout (`FL`, `FR`, `FC`, `LFE`, and so on). A device with one or two channels does not offer it. If you chose it earlier and the device now has two channels, Backtracking captures it merged and says so under the source. A source with one or two channels stays one row, or L and R rows; it never becomes a multichannel lane.

**Capture surround in the VST3 and CLAP plug-ins.** Insert the VST3 plug-in on a surround bus from 5.1 to 9.1.6 and it captures every channel, each in its own row, named from the DAW's speaker layout; 7.1.4 is `FL`, `FR`, `FC`, `LFE`, `Lb`, `Rb`, `Ls`, `Rs`, `TFL`, `TFR`, `TBL`, `TBR`, in that order. The names are the Windows ones: when a layout has both a back pair and a side pair, the back pair is `Lb` and `Rb` and the side pair `Ls` and `Rs`. CLAP captures 6 to 16 channels too, but CLAP gives no speaker layout, so its channels are only numbered, with files such as `-Ch01_Channel 1.wav`; they have no direction, so L and R and the binaural choices give them the same level on both sides. In the plug-in, the multichannel waveform, **L/R of multichannel sources** and **Multichannel sources** work as described below, both choices are saved with the DAW project, and older projects load with the defaults. Where the plug-in differs from the desktop app:

- It always shows and exports every channel. It has no **Hide silent channels**, no channel group switch and no editing separately, so a save or drag-out always exports every row, L/R file included.
- When a multichannel lane appears, the plug-in asks the DAW for a taller window. If the DAW keeps the window smaller, scroll the rows with the mouse wheel.
- On the Capture page, **Preview of multichannel sources** defaults to **In their own channels**. When the main input and output both match the capture's width and speaker layout, each channel plays on the same bus channel through your DAW's monitoring. With a different bus or **As L/R** selected, the Stereo or Binaural mix plays on the first two outputs. A mono output combines both sides.
- With the sidechain captured too (**Both**), the sidechain stays below as one row labelled **B** and exports as before, in a file ending in `-B`; the multichannel lane's files have `-A` after the length.
- Multichannel lane files carry no tempo in their names.

**Multichannel waveforms.** A source with more than two channels shows two rows on top, **L** and **R**: all its channels mixed to stereo, with a thin line below them. Under them each channel has its own row, labelled with the channel's name; 7.1.4 is L, R and twelve rows. Scroll with the mouse wheel when the rows do not fit. When a source has more than one channel group, for example channels and objects, a switch at the bottom left of the waveform chooses the group on show; L and R are always mixed from the channels group. Meters for multichannel sources offer Peak and VU, not LUFS. The more channels a source has, the less history fits in memory; the **Capture** page of Options shows how much is kept.

**Hide silent channels.** With **Hide silent channels** on the **Capture** page of Options (on by default), system, app and microphone sources show under L and R only the channels that have sound: a channel has sound when any sample of it in the retained history is above -90 dBFS. When nothing but the front left and right has sound, only L and R show. A channel's row appears when it starts to have sound and goes once that sound has left the retained history. Add-on sources always show every channel. Hidden channels are still captured.

**How L and R sound.** **L/R of multichannel sources** on the **Capture** page of Options is **Stereo** (the default) or **Binaural**. Stereo mixes each channel to the left and right by where its speaker stands. Binaural places each channel around your head at its speaker's direction, so on headphones you can hear which channel is where. The choice changes what preview and export sound like; the waveform stays the same.

**Preview.** `Space` previews multichannel sources too: it plays the channels the selection covers, mixed as set above, and only the selected rows while editing separately. Objects that carry positions, such as the objects an add-on sends, move as their positions say and play binaurally, as they export.

**Export multichannel clips.** On the **Export** page of Options, **Multichannel sources** decides which files a multichannel source writes:

| Choice | Files written |
| --- | --- |
| **Binaural, one file per channel** (the default) | One two-channel WAV per channel, ending in `-Ch01_channel name_binaural.wav`; on headphones you hear where that channel is |
| **Mono, one file per channel** | One mono WAV per channel, ending in `-Ch01_channel name.wav` |
| **One multichannel file** | One WAV with every channel in its original order |
| **One binaural mix** | One two-channel WAV, ending in `-Binaural.wav` for the whole channels group and in `-Mix-Binaural.wav` for some channels or a mix with objects |

If you chose mono files or one multichannel file before, that choice is kept. The binaural files of the channels of a clip add up to the binaural mix of the same clip.

Exporting a whole multichannel lane (not editing separately, or every row selected) also writes one file for L and R: `-Stereo.wav` when **L/R of multichannel sources** is Stereo, `-Binaural.wav` when it is Binaural, written once when the table above already produces it. While editing separately, only the selected rows are exported, and selecting the L and R rows writes that L/R file. Only the channel group on show is exported. With **Hide silent channels** on, a whole system, app or microphone lane writes only the channels it shows (the ones that had sound), plus the L/R file; when nothing but the front left and right has sound, it writes the L/R file alone.

An add-on can send positions for a group of channels, such as the objects of a film soundtrack. Those channels move as the positions say in both binaural choices. One of them that has no position anywhere in the clip cannot be placed: it is written as a mono file, and the status line says so once. When the export includes channels with positions, a `-Positions.json` file is written beside the audio. It records where those channels were during the clip and when, counting from the clip's first sample.

The files begin like any other source's, with its name (and A or B when two lanes record) as described under [file names](#output-location-and-file-format), and sorting by name puts the channels in order, as in `Backtracking_Test tone_2026-11-17_09-05-00_12.4s-Ch01_L_binaural.wav`. An add-on's sources can be renamed in the **File name** field in Options → Sources too. Fades apply to every file.

### Save and restore settings

The Windows and macOS desktop apps save your history length, auto-resume delay, theme accent, background transparency, meter mode, WAV export settings, both folders, keep-on-top setting, waveform-only mode, and each source's file name as you change them. Those preferences are restored the next time you open the app. Edit A and B separately is not saved and is off each time the app opens.

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

插件接收它所在位置的聲道：單聲道或立體聲軌道，或 5.1 至 9.1.6 的 VST3 環繞聲匯流排（CLAP 為 6 到 16 個聲道，沒有聲道名稱）。超過兩個聲道時，視窗會顯示每個聲道，見「多聲道來源」。

The plug-in takes the channels where it sits: a mono or stereo track, or a VST3 surround bus from 5.1 to 9.1.6 (6 to 16 channels under CLAP, without channel names). With more than two channels its window shows every channel; see Multichannel sources.

1. **REAPER**。開啟 Preferences，在 Plug-ins 的 **VST** 或 **LV2/CLAP** 頁面確認路徑並重新掃描。開啟目標音軌的 **FX**，搜尋 **Backtracking**。

   Open Preferences. Under Plug-ins, check the path on the **VST** or **LV2/CLAP** page and rescan. Open **FX** on the target track and search for **Backtracking**. [REAPER User Guide](https://dlx.reaper.fm/userguide/html/)
2. **FL Studio**。開啟 **Options → File settings → Manage plugins**，執行 **Find installed plugins** 並確認已啟用 **Verify plugins**。在目標 Mixer insert 的效果欄加入 **Backtracking**。

   Open **Options → File settings → Manage plugins**. Run **Find installed plugins** with **Verify plugins** enabled. Add **Backtracking** to an effect slot on the target Mixer insert. [FL Studio manual](https://www.image-line.com/fl-studio-learning/fl-studio-online-manual/html/basics_externalplugins.htm)
3. **Cubase／Nuendo 的 Dolby Atmos**。要擷取 Dolby Atmos 混音的 7.1.4 聲道，把 Backtracking 的 VST3 插在 **Main Mix** 輸出聲道上、**Renderer for Dolby Atmos** 之後的 insert。依 Steinberg 手冊描述的訊號流程，renderer 之後是床聲道加物件的完整 7.1.4 輸出；插在 renderer 之前只收得到床聲道，沒有物件。這個位置尚未在 Cubase 或 Nuendo 中實際確認。renderer 的 **Downmix** 設為 Binaural 時，插件收到的是 7.1.4 還是兩個雙耳聲道，也尚未確認。

   To capture the 7.1.4 channels of a Dolby Atmos mix, insert the Backtracking VST3 on the **Main Mix** output channel, after the **Renderer for Dolby Atmos**. By the signal flow in Steinberg's manual, the full 7.1.4 render, bed and objects, leaves the renderer there; before the renderer, Main Mix carries only the bed, without the objects. This placement has not yet been checked in Cubase or Nuendo. Whether the plug-in still gets 7.1.4, or two binaural channels, when the renderer's **Downmix** is set to Binaural is not checked either.

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
9. 以系統播放為來源錄一段聲音後框選。按 `Space`，確認從起點旗標播放到終點旗標，播放線由左往右移動並在終點消失；播放中再按 `Space` 應立即停止。按住 `Alt`（Mac 為 `Option`）拖曳起點與終點旗標，確認波形收窄並顯示淡化長度，播放中調整也聽得出變化。拖出並保存選區，確認 WAV 頭尾有淡化；繼續錄音後，確認試聽內容沒有出現在歷史波形中。

   Record some System playback and select part of it. Press `Space` and check that it plays from the start flag to the end flag, with the playhead moving left to right and leaving at the end. Press `Space` during playback to stop at once. Hold `Alt` (`Option` on a Mac) and drag the start and end flags: the waveform should taper and show the fade's length, and the change should be heard during playback. Drag out and save the selection and check that both WAVs fade at their ends. After capture continues, check that the preview does not appear in the history waveform.
10. 以兩個來源擷取。在 Options → Sources 為 A 的來源輸入檔名，確認欄位下方顯示的檔名開頭；拖出整段，確認兩個 WAV 各以自己來源的名稱與字母開頭。把該來源改放到 B，確認名稱跟著來源。輕按右側 `Ctrl`（Mac 為右側 `Command`），確認 A、B 之間出現細線。在 B 框選後按 `Space`，確認只播放 B；拖出該選區，確認只寫出 B 的檔案。再在 B 框選並選擇 **Save snapshot**，確認只保存 B。不框選，從 A 拖出，確認只寫出完整的 A。最後在 B 框選後再輕按一次，確認選區擴大為涵蓋兩軌。

   Record two sources. In Options → Sources, type a file name for source A and check the beginning shown under the field. Drag everything out and check that each WAV begins with its own source's name and letter. Move that source to B and check that its name follows it. Tap the right `Ctrl` key (right `Command` on a Mac) and check that a line appears between A and B. Select part of B and press `Space`: only B should play. Drag that selection out and check that only a B file is written. Select part of B again and choose **Save snapshot**: only B should be saved. Without a selection, drag from lane A and check that all of A, and only A, is written. Finally, select part of B, tap again, and check that the selection widens to both lanes.

[回到專案首頁 / Back to the project README](../README.md)
