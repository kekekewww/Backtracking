# Backtracking 本輪功能檢查

檢查日期：2026-11-13（Asia/Taipei）。程式碼基準：`d321bd3`，`Add Backtracking waveform controls, Sender and dynamic capture history`。

**同日後續更新：** 依使用者指示，Windows 全域快照已改為 **Alt＋數字 1 左側實體鍵，不含 Shift**。Windows 桌面完整測試含原生快捷鍵註冊／衝突／釋放測試共 **26 項通過、0 失敗**，Release 建置成功，執行檔位於 `desktop/windows/target/release/Backtracking.exe`。Options、README 及 USER_GUIDE 快照說明已同步。macOS 仍使用 Fn／Globe，未變更其綁定。下文保留修改前 `d321bd3` 的檢查結果；原 Win 組合鍵占用已不阻擋新的 Alt 綁定。

**2026-11-14 後續更新：** Windows、macOS 與插件 Options 已改為左側分類、右側向下排列及捲動的表單。Windows 實機確認保留時間、透明度、波形配置與電平表選單可操作，波形模式高度由 144 縮至約 61 像素，切回後恢復。最新本機檢查為 Windows 26 項、核心 51 項、插件 24 項通過；另有原生快捷鍵及 TCP loopback 各 1 項未於本次重跑。macOS 仍未經本機原生驗證。Windows 硬體監測的方法、逐秒記錄與限制見[硬體使用量](HARDWARE_USAGE.md)。主監測期間未執行快照寫檔；後續經使用者允許，另外驗證了新實例匯出的 97.01 秒 WAV 格式與資料完整性。本次監測不構成無缺幀／無爆音驗收。

範圍：Windows／macOS 桌面版、VST3、CLAP；Linux 僅檢查本輪涉及的全域快照常駐元件。原始檢查僅新增此報告；後續快捷鍵程式碼變更如上，未修改個人設定或既有 `misc/` 檔案。

## 結論與證據範圍

主要功能已有實作，Windows 桌面與插件在本機可完成 Debug 建置；共用核心、Windows 桌面、插件及 Sender 連線測試合計 **99 項通過**。另外實際執行 Windows 全域快捷鍵註冊測試時，**1 項失敗**：目前工作階段的 Win＋左上角實體鍵已被占用，Windows 回報 `0x80070581`。

尚不能將整輪標記為驗收完成：已發現快照請求可能遺失、macOS／插件旗標操作不一致等程式碼缺口，且本次未在 DAW 內載入插件，也未執行 macOS／Linux 原生測試。以下「已實作」表示已追查實際執行路徑，不代表所有平台與宿主均已實測。

## 逐項規格對照

| 規格 | 當前狀態 | 證據與限制 |
| --- | --- | --- |
| DAW 小節線 | 已實作；核心與插件單元測試通過 | `plugins/backtracking-plugin/src/lib.rs:152` 要求有效播放狀態、BPM、拍號、拍位置及小節起點；不足時不產生新標記。`crates/backtracking-core/src/capture.rs:317` 將標記投影到實際保留的音訊，包含 pre-roll；`synchronized.rs:534` 處理 A／B 共用時間軸。仍需宿主變速、變拍、循環與跳轉實測。 |
| 秒數線 1／5／10 秒 | 三種介面均已實作 | 以保留音訊的 frame 座標計算，經可見時間範圍轉換後繪製；Windows `ui.rs:2479`、macOS `BacktrackingApp.swift:2690`、插件 `editor.rs:1095`。 |
| 小節線／秒數線獨立開關，跟隨背景透明度 | 已實作 | Windows `ui.rs:1514`、macOS `BacktrackingApp.swift:1631`、插件 `editor.rs:1676`；全透明時隱藏參考線。 |
| Shift 吸附可見小節線與旗標 | 部分符合 | Windows 路徑符合；macOS 目前選區旗標未完整納入吸附，插件調整端點時排除了目前選區的固定旗標。詳見問題 3。 |
| 每個插件實例獨立 Sender | 已實作；底層連線測試通過 | 每個實例有獨立 Session、Worker、Sender queue 與 endpoint ID。`session.rs:741`、`sender.rs:98`。同名來源使用 ID 區分，可同時接收同一 Sender。 |
| Sender 傳遞音訊與 DAW 時間資訊 | 音訊及宿主推算的小節位置已實作 | `sender.rs:234` 傳送音訊、單調時鐘 timestamp、取樣率與小節 frame offsets；未傳原始 BPM／拍號／DAW 播放位置欄位。現有波形小節線使用傳來的標記；若後續功能需要原始 transport 資料，仍須擴充協定。 |
| 【Sender】軌道名稱、自訂名稱、同名分別選取 | 已實作 | Windows `ui.rs:1626`／`ui.rs:2870`；macOS `BacktrackingApp.swift:1187`／`:1205`；宿主名稱優先，未提供時使用自訂名稱。設定保存來源 ID，不只保存顯示文字。 |
| 插件、桌面與各接收視窗各自管理狀態 | 已實作；仍需整合實測 | `session.rs:817` 的本地 accepting 狀態與 Sender active 狀態分開；桌面暫停只影響該接收端。Windows 以 `--new-capture-window` 開啟另一程序，macOS 另開獨立實例。須補「本地選區暫停時桌面仍連續錄音」實測。 |
| 桌面 Sender 可選 A 或 B | 已實作 | Windows `controller.rs:220`、macOS `SystemAudioTap.swift:192`；沿用共用 A／B 同步核心。 |
| Main／Sidechain／Both，主輸出原樣通過 | 已實作；合成音訊測試通過 | `lib.rs:73` 宣告 mono／stereo 主軌與輔助輸入配置；`:135` 僅讀取輸入。`session.rs:1202` 測試 stereo Main＋mono Sidechain、共用起訖及輸入不被修改。Both 模式本地分兩路擷取，但 Sender 目前只傳 Main（UI 有說明）；規格若要求同一 Sender 同時傳兩路，這部分尚未實作。 |
| VST3 輔助輸入框架修正 | 已有修正；宿主驗收未完成 | vendored nice-plug 的 `wrapper/vst3/wrapper.rs:1455` 使用正確的 `>=` 邊界檢查；`:1479` 同樣保護輔助輸出。仍須驗證未接線、不同主／輔聲道數及舊專案載入。 |
| Audacity loopback／應用程式來源 | 已提供相符路徑；未實機驗收 | Windows System Audio 走 render loopback；App 來源走 process-tree capture（`loopback.rs:295`、`:309`、`:584`）。macOS 提供系統及 App tap。這些是播放輸出的擷取，不會直接讀取 Audacity 正在錄入但未播放的資料。 |
| 框選放開保留選區與暫停；選區期間不計續錄 | 已實作；插件狀態測試通過 | Windows `ui.rs:2190`／`:2745`；macOS `BacktrackingApp.swift:2481`／`:1959`；插件 `editor.rs:1370`、`session.rs:424`／`:595`。 |
| 游標中心滾輪縮放、中鍵水平平移 | 已實作 | Windows `ui.rs:2253`；macOS `BacktrackingApp.swift:2235`；插件 `editor.rs:1159`。僅變更可見 frame 範圍，保留選區／暫停，不改視窗大小。 |
| 拖旗標、A／B 共用起訖、原始取樣切割 | 核心切割已實作並通過測試；旗標互動有缺口 | 匯出使用原始整數 frame 範圍，再按設定轉換 WAV：Windows `ui.rs:506`、macOS `BacktrackingApp.swift:327`、插件 `editor.rs:1592`。macOS／插件點擊旗標而未拖曳也可能改動端點，詳見問題 4。 |
| 普通點擊、匯出拖曳結束立即續錄；內部操作／取消不留 WAV | 主要路徑已實作 | 三種介面只在拖出整個 App 後準備匯出，失效或未接受的檔案會清理。Windows 清理測試通過；macOS／插件原生拖放仍需宿主驗收。 |
| Esc／Backspace 取消選區、續錄、不寫檔 | 選區取消已實作 | Windows 同時取消任一波形手勢；macOS／插件對「沒有選區的匯出準備中」只處理 Esc，Backspace 不完整。這是額外的操作一致性邊界，不等同於一般選區取消失效。 |
| Win／Super 或 Fn／Globe＋實體鍵，移除 Ctrl+S | 已實作；本機 Windows 鍵位被占用 | Windows 掃描碼 `0x29` 配合前景鍵盤配置、`MOD_WIN | MOD_NOREPEAT`（`snapshot_hotkey.rs:62`）；macOS 使用全域 event tap、Fn 與實體鍵判斷；已檢查來源碼無原 Ctrl+S 儲存處理。 |
| 背景觸發、不切視窗、長按一次、單一目標 | 已實作；原生事件驗收未完成 | Windows 專用 WM_HOTKEY worker、只有主桌面視窗註冊；macOS listen-only tap 有權限處理與重試；Linux helper 有 X11／Wayland 路徑及按住去重。每次只選桌面或一個實例，不向所有實例廣播。 |
| 選段快照後續錄；全段快照持續錄；Options 保留儲存 | 已實作；相關單元測試通過，但插件請求佇列有缺口 | 桌面與插件先取得固定快照，選段情況立即恢復待命；Options 儲存目前錄音器。插件端另有可能合併／丟棄請求，詳見問題 2。 |
| Linux 全域常駐元件 | 已實作；本機未執行 | `desktop/linux/` 提供快捷鍵 helper、X11／Wayland portal 與桌面項目；需另行安裝並常駐。目前不是 Linux A／B 桌面 GUI。CI 有 Xvfb 按住一次觸發及重複註冊檢查，但本次未查證遠端 CI 結果。 |
| 10／20／30／60 秒、3／5／10 分鐘；初始一分鐘容量 | 已實作；核心測試通過 | `capture.rs:19` 提供 7 個選項；`capture/history.rs:119` 預留 60 個一秒頁，實際保留音訊超過基礎容量才新增頁。 |
| 縮短只裁最舊資料，釋放多餘容量；短內容完整保留 | 已實作；指定情境與 A／B 測試通過 | `capture.rs:597`、`history.rs:181`／`:192`。核心測試 `grows_only_as_audio_is_kept_and_shrinks_without_moving_the_recording_boundary` 覆蓋 20 秒保留、10→3 分鐘、回到 30 秒與後續錄製。 |
| 背景增減容量，切換無清空／缺幀／爆音 | 背景處理及資料連續性已涵蓋；實際裝置待驗收 | Windows controller worker `controller.rs:432`；macOS captureQueue `SystemAudioTap.swift:905`；插件 worker `session.rs:755`。合成資料確認不整段清空、A／B 相同邊界；未量測實際裝置 callback、queue overflow 或切換時可聽爆音。 |
| 波形模式原生視窗貼合、切回恢復大小 | Windows 與插件請求路徑已實作；macOS 幾何有小缺口 | Windows `ui.rs:1233`／`:2771` 保存完整尺寸並縮原生 viewport；插件 `editor.rs:929` 的 InnerSize 可到達 VST3／CLAP host resize callback，最終由 DAW 決定。macOS 關閉外框仍固定留 1pt 周邊，詳見問題 5。 |
| 波形模式透明度、獨立陰影、全透明仍可操作 | 已實作；原生視窗效果待實機 | Windows `ui.rs:138` 關閉 compact 模式陰影、原生邊框與圓角；macOS `BacktrackingApp.swift:1621` 設 clear／非 opaque、保持 mouse events，compact 時關閉陰影；波形與旗標繪製不依背景 alpha 消失。 |
| 可選細外框、主題／自訂色、可選跟透明度 | 已實作 | Windows `ui.rs:1531`／`:2363`；macOS `BacktrackingApp.swift:1310`／`:471`；插件 `editor.rs:1687`。外框在波形面板外緣。 |
| Options 左分類／右細項、適用項目、插件捲動 | 已實作 | Windows 六類 `ui.rs:310`；macOS 左側分類與右側內容 `BacktrackingApp.swift:991`；插件 `editor.rs:665`／`:673` 五個適用分類及右側 vertical ScrollArea，不顯示桌面自動更新項目。 |

## 需處理的問題

### 1. 本機 Windows 全域快照鍵目前不可用（環境阻礙）

實際執行 `win_grave_registers_reports_conflicts_and_is_released_on_exit`，對照用的另一組快捷鍵可註冊，但指定 Win＋實體鍵註冊失敗：`快速鍵已登錄。 (0x80070581)`。測試在 `desktop/windows/src/snapshot_hotkey.rs:129` 中止。

這不是編譯失敗；是目前 Windows 工作階段無法取得指定組合鍵。尚未確認占用者。程式有顯示註冊錯誤的路徑；本次未更動其他 App 的快捷鍵設定，也未停用任何程序。要在此環境驗收背景快照，需先排除這個占用，再測前景為 DAW、視窗隱藏與長按情境。

### 2. 插件全域快照可能遺失請求（高優先，程式碼判讀）

- `crates/backtracking-core/src/sender.rs:191` 將所有已收到的 S 請求寫入同一個 `bool`，`:204` 一次取走。若同一輪處理到多個獨立觸發，只會建立一份快照。
- `plugins/backtracking-plugin/src/session.rs:941` 的寫檔佇列容量只有 2，`:955` 使用 `try_send`；寫檔跟不上時會丟棄新 job。
- 選段在 `session.rs:576` 已先恢復錄音，而請求端 `sender.rs:360` 只確認 TCP 寫入，未收到目標接受／排入佇列的確認。桌面可能顯示已發送，實際卻沒產生檔案。

這與「長按只觸發一次」不同：上述遺失可發生於多次獨立按鍵。既有單一請求測試未覆蓋連續觸發或慢速寫檔；應補請求計數／排程與可回報的接受狀態，再測選段不遺失。

### 3. 可見旗標與 Shift 吸附目標不一致（中優先，程式碼判讀）

macOS `BacktrackingApp.swift:2750` 同時畫出舊 marker 與目前選區，但 `:2370` 只把舊 marker 納入 Shift 吸附，第一次框選的兩個可見旗標無法成為吸附目標；`:2357` 命中測試又只使用 `selection ?? marker`，有選區時可見的舊旗標不能拖曳。

插件 `editor.rs:1143` 在 ResizeSelection 期間排除整個目前選區，連沒有移動、仍可見的固定端點也無法吸附。Windows 的繪製、命中與吸附使用同一組旗標，未發現這項落差。

### 4. macOS／插件只點旗標也可能改動選段（中優先，程式碼判讀）

macOS `BacktrackingApp.swift:2485` 在 mouseUp 無條件重新套用拖曳座標；插件 `editor.rs:1312` 在按下旗標時直接用點擊 x 重建移動端點，`:1370` 放開便提交。兩者都沒有先判斷是否真的移動。

旗標有約 8px 命中容差，實際圖形又會向面板內側挪位置，因此點在可見旗標上也可能改掉原始 frame 邊界。Windows `ui.rs:2177` 有移動門檻。需補「點擊不拖曳維持原端點」測試。

### 5. macOS 關閉外框後仍留固定周邊（低優先，幾何已確認）

`BacktrackingApp.swift:1739`、`:1747` 固定使用 `waveformFrameInset`，不隨外框開關改變，因而保留左右上下各 1pt 的額外原生範圍。這不完全符合「波形面板＋可見外框」的尺寸要求。全透明時這圈是否攔截下層視窗點擊，仍須 macOS WindowServer 實測，不能僅憑程式碼宣稱已重現。

### 6. CLAP 舊專案聲道配置 ID 有相容性風險（待宿主重現）

前一版本 `AUDIO_IO_LAYOUTS` 的 ID 0／1 是 stereo／mono main-only；本輪 `lib.rs:74` 改為 stereo Main＋stereo／mono Sidechain。wrapper 在 `wrapper/clap/wrapper.rs:2429` 把陣列 index 當 config ID，`:2464` 也按 index 還原。

若宿主保存並還原舊 config ID，原本 mono 的 ID 1 可能改成 stereo Main＋mono Sidechain。這是可從差異確認的 ID 語意變更，尚未用真實舊專案重現。VST3 仍保留 main-only 配置，且按明確 bus／channel 數匹配，但也不能略過舊專案驗收。

### 7. 文件與本輪行為不一致（需同步）

`docs/USER_GUIDE.md:281`／`:283` 等處仍指示 Ctrl+S；`:289` 與 `:426` 仍描述框選放開後續錄；`:248` 只列出最長 60 秒；`:210` 描述波形模式強制隱藏背景。這些說明會讓使用者依舊行為驗收。README 已加入本輪概述，但完整使用手冊尚未同步。

### 8. CLAP 輔助輸入還有防禦性邊界問題（低優先）

`wrapper/clap/wrapper.rs:2065` 使用 `aux_input_idx > audio_inputs_count`，相等時仍會在 `:2069` 解參考。VST3 對應處已使用 `>=`。

此問題應修補，但目前不能列為符合協定宿主的已重現崩潰：CLAP 的 process 契約要求 buffer count 等於宣告的 port count，正常配置下不會走到缺少該元素的情況。這屬異常宿主輸入的健全性檢查。

## 本次實際執行的驗證

環境：Windows x64、`rustc 1.95.0`、`cargo 1.95.0`。插件缺少的 Cargo 相依套件已下載後完成測試；原始碼與 lockfile 未因此修改。

| 命令 | 結果 |
| --- | --- |
| `cargo test --locked --offline --workspace` | 51 通過、0 失敗、1 預設略過；doc tests 無測試。 |
| `cargo test --locked --offline --workspace same_named_senders_fan_out_and_snapshot_targets_are_independent -- --ignored --nocapture` | 額外 Sender TCP loopback 測試 1 通過，驗證同名來源、兩個接收端及獨立快照目標。 |
| `cargo test --locked --offline --manifest-path desktop/windows/Cargo.toml` | 25 通過、0 失敗、1 預設略過。 |
| `cargo test --locked --offline --manifest-path desktop/windows/Cargo.toml snapshot_hotkey::tests::win_grave_registers_reports_conflicts_and_is_released_on_exit -- --ignored --exact --nocapture` | 1 失敗；Win＋實體鍵已被占用，`0x80070581`。 |
| `cargo test --locked --offline --manifest-path plugins/backtracking-plugin/Cargo.toml` | 22 通過、0 失敗；doc tests 無測試。 |
| `cargo build --locked --offline --manifest-path desktop/windows/Cargo.toml` | Debug 建置成功。 |
| `cargo build --locked --offline --manifest-path plugins/backtracking-plugin/Cargo.toml` | Debug DLL 建置成功；有既有未使用方法警告。此 DLL 包含 VST3／CLAP 匯出，不等於已在宿主載入成功。 |

本次沒有建置發佈安裝包，也沒有確認遠端 CI 狀態。`misc/BUILD-INFO.txt` 與 `misc/Backtracking-Windows-x64/BUILD-INFO.txt` 記錄的來源 commit 分別為 `6bc91b6`、`c5f3a30`，均不是本次 `d321bd3`；不能用這些包直接代表本輪程式碼的驗收結果。

## 發佈前仍須驗收

1. 解決本機 Windows 快捷鍵占用；在 DAW 前景及桌面視窗隱藏時觸發，確認不搶焦點、長按一次、桌面與指定插件只擇一保存。
2. 修正快照請求遺失，驗證短時間多次按鍵、長音訊／慢速匯出及選段續錄。
3. macOS 實機建置，驗證 Fn／Globe 事件、ANSI／ISO 鍵位、輸入權限拒絕及重新授權。
4. 在 VST3／CLAP 宿主驗證 tempo／拍號變更、停止、循環、跳轉、缺少 transport 資料，以及靜音壓縮後的小節線。
5. Main／Sidechain 未接線、mono／stereo 混搭、切換 Main／Sidechain／Both、主輸出逐樣本通過及舊專案還原。
6. 兩個同名 Sender、桌面 A／B、兩個接收視窗；本地插件框選暫停時，桌面仍持續接收；接收端各自暫停不影響其他端。
7. 修正旗標互動後，逐平台驗證 Shift 可見目標、旗標零位移點擊、端點拖曳、縮放／平移後樣本邊界與 A／B 匯出一致。
8. 真實裝置持續錄製，執行 10→3 分鐘、5 分鐘→30 秒及僅有 20 秒內容的切換，記錄 queue overflow／缺幀並檢查可聽爆音。
9. 波形模式切換前後量測原生視窗範圍，背景全透明／外框關閉時測上下邊緣下層點擊，確認陰影及完整尺寸還原；插件測 DAW 接受／拒絕 resize。
10. Audacity 同時 loopback 錄音＋Backtracking System Audio，以及 Audacity 自己播放＋App 來源；確認輸出裝置和監聽路由一致。
11. 真實 DAW 成功拖放、拒絕、取消與拖回原視窗，確認無多餘 WAV；另測沒有選區的匯出準備中按 Esc／Backspace 的一致性。
12. 同步 USER_GUIDE，使用本輪 commit 重新打包，將 BUILD-INFO 與驗收版本對齊。
