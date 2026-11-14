# 波形編輯與介面回應：2026-11-14 / Responsive editing

**狀態：本機檢查與指定 Windows 操作已完成；遠端指定平台 jobs 已分別通過。**Windows／macOS 在產品 commit `55baeac` 通過，Linux X11 在修正後 `52af947` 的 Linux-only 執行通過。原完整執行與前兩次重跑仍保留 failure，沒有改寫為同一完整矩陣全綠；Mac GUI／Fn／DAW、Wayland 及新版完整安裝流程仍未驗收。

**Status: local checks and specified Windows interactions are complete; the named platform CI jobs passed across separate runs.** Windows/macOS passed on product `55baeac`, and Linux X11 passed in the Linux-only `52af947` follow-up. The original failed full run remains a failure; GUI/Fn/DAW, Wayland, and new-payload installation coverage is not implied.

本輪更新與[較早的 Windows 硬體監測](HARDWARE_USAGE.md)分開記錄。`benchmarks/2026-11-14-windows/` 的歷史追蹤、摘要與雜湊不因這次互動規則改變而重寫；它不能代表新版的 CPU、記憶體或延遲結果。

## 互動規則 / Interaction rules

已確認採用使用者選定的「固定選取時的畫面與素材，背景繼續錄音」。背景錄音與編輯狀態分開：介面停止追隨即時尾端，不代表擷取停止；這項互動選擇已確定，實作驗證仍依下方進度記錄。

| 操作 / Action | 本輪規則 / Rule |
| --- | --- |
| 框選或拖曳旗標 / Select or adjust flags | 固定開始編輯時的素材與視野；背景持續擷取，不因這些操作暫停。 / Hold the editing material/view while background capture continues. |
| 縮放或平移 / Zoom or pan | 依目前編輯素材移動時間軸，保留選區、端點與原始取樣位置。 / Navigate the same editing material and retain its original sample positions. |
| 準備拖放匯出 / Prepare an export drag | 使用固定素材準備檔案，背景照常錄製；檔案準備不是隱含的 Pause。 / Prepare from fixed material without implicitly pausing capture. |
| 背景循環覆寫或縮短 History / Live rollover or a shorter history limit | 背景只保留新上限內的最新音訊；已固定的編輯素材與選區仍保留原取樣，A/B 共用同一起訖。 / Trim live history while keeping the editing snapshot and aligned A/B sample boundaries. |
| 取消、普通點擊或匯出結束 / Cancel, ordinary click, or export completion | 離開編輯狀態並回到即時視野；不因這些操作強制解除手動 Pause。 / Return to live view without forcing a manual pause to resume. |
| Pause / Play | 擷取由 Play 與既有自動續錄另行控制；0 秒設定下手動 Pause 維持。Play 保留目前編輯素材與選區，不等於回到即時視野。 / Capture controls remain separate; Play preserves editing material/selection. |

保存結果只對應發起該次請求的編輯。全段 Save 在背景準備／完成時，不會把後來才建立的選區誤當成已存選段而取消；不能把「某個保存完成」一律解讀成「清除目前編輯」。插件全段 Save 的取樣準備亦移到 worker，這項變更的實際 DAW 流程尚待對應宿主驗證。

編輯素材固定後，繼續錄到的音訊不會偷偷改變原來選區的內容。使用者可先完成或取消這次編輯，再回到最新錄音。這項選擇也意味長時間保留編輯素材可能額外占記憶體；本輪尚未提供新版的實測記憶體增量。

Editing uses immutable material: later incoming audio must not silently change a selected clip, including after live-buffer rollover or history reduction. The user returns to the latest capture after finishing/canceling the edit. Keeping editing material can consume additional memory; its current implementation cost is not inferred from the older hardware dataset.

## 吸附與顯示 / Snapping and rendering

- `Shift` 加左鍵拖曳可直接吸附框選；Windows 的 `Ctrl`／macOS 的 `Option` 保留自由框選，也可再加 `Shift` 吸附。依優先順序吸附到附近**可見旗標 → 已繪出的小節線 → 已繪出的秒數線**；同類目標選最近位置。拖曳端點時排除正在移動的那個端點，避免自我吸附。
- 吸附與繪圖使用相同的可見參考線集合。因透明度而沒有畫出的參考線不可成為隱藏吸附點；背景全透明時旗標仍可見、可操作並可吸附。
- 波形取樣區間數、筆畫寬度與參考線密度依**實際音訊繪圖區域**、可見時間跨度和互動狀態調整。視窗的外框、工具列或空白區不計入音訊寬度。
- 繪圖與吸附仍須有上限，避免非常大的視窗或極端縮放產生無界工作量。共享政策目前上限為 4,096 個波形區間及 2,048 條參考線；最終各平台整合是否正確仍須驗證。

Shift-drag directly creates a snapped selection. Ctrl on Windows / Option on macOS keeps free selection; adding Shift also enables snapping. Visible flags take priority over painted bars, then painted seconds. The moving handle excludes itself. Rendering and snapping use the same visible guide set. Fully transparent backgrounds hide guides but retain interactive flags. Waveform bins, stroke widths, and guide density depend on the actual audio rectangle, visible duration, and interaction state, with bounded work.

## Options 排列 / Options presentation

Options 維持左側分類、右側直向清單與捲動。頁標題使用較大粗體，分組與欄位保持明確層級；主要內文／操作文字至少 15。下拉選單最多顯示 10 列，少於 10 項時按實際項目數決定高度，超過時在選單內捲動；不是固定留下 10 列空白。

操作說明放在文件；UI 保持控制與必要狀態，只保留標題、欄位名稱、值、按鈕及必要的狀態／錯誤／路徑。固定素材、背景擷取或內部處理流程的說明不作為新增 UI 文案，操作教學也不以浮層覆蓋波形。

Options retains left categories and a vertical scrolling detail list. Larger bold headings and body/control text of at least 15 distinguish hierarchy. Popups show up to ten rows, size to the actual count when shorter, and scroll when longer. Native macOS behavior and DAW-hosted editor sizing still require platform validation.

Operating instructions belong in documentation. The UI keeps controls and necessary state: headings, labels, values, buttons, and required status/error/path information. The implementation descriptions in this report are not proposed UI copy or floating gesture tutorials.

## 卡頓調查的方法 / Responsiveness investigation

調查基準為 `58dee34`，使用修改中工作目錄的 Release 核心探針及 Windows controller 回歸測試。核心探針在**新程序**中依序建立三個 Capture，輸入固定 `0.05f32`，以 10 ms 音訊區塊盡快批次處理 600 秒；這不是同步播放的逐秒 UI 監測，也不是每個案例各重開一次程序。涵蓋 48 kHz stereo、96 kHz stereo，以及 48 kHz stereo A/B。

The investigation uses a Release core workload and targeted Windows-controller regressions based on `58dee34`. The probe starts a new process and batches synthetic 10 ms audio blocks. It is not a reboot test, real-time playback session, or end-to-end UI latency measurement.

初次本機探針與正式範例重跑已分別保存到[獨立證據目錄](benchmarks/2026-11-14-responsive-editing/README.md)，包含兩份各 36 筆的核心結果、原始 harness、偏好保存資料及來源雜湊；沒有修改較早的硬體證據。正式範例為 [`retention_probe.rs`](../crates/backtracking-core/examples/retention_probe.rs)：

```powershell
cargo run --release --locked -p backtracking-core --example retention_probe
```

核心探針用排序後最近 rank 作為 median／P95；n=20 的 median 是中間偏上的那個觀測值，不是兩個中間值的平均。偏好保存探針另用 type-7 線性插值，兩者方法分開記錄。

### 已找到的等待邊界 / Identified waiting boundaries

1. **UI 與 capture 共用鎖。**保留時間變更雖透過背景命令處理，但 worker 在排空佇列、配置／釋放頁面和複製整段快照時持有 capture mutex。舊的 UI 狀態、revision、音量表和波形讀取也會阻塞取得同一把鎖，因此「設定指令在背景」本身不能保證 UI 不等待。
2. **整段快照複製。**將複製移到 capture worker 後，UI 仍須以非阻塞方式取回結果；否則會從另一個同步等待邊界再次等到同一份工作。
3. **同步保存偏好設定。**Windows 的 UI 路徑已找到 `save_preference → Preferences::save → write_all → File::sync_all → rename`。獨立 harness 已量測真實 production save 呼叫，背景保存修正仍在整合。這條同步路徑可花數十毫秒，但尚未將它證實為使用者觀察到的秒級卡頓原因。

The controller investigation identifies shared-lock waits between capture work and UI reads, complete snapshot copies, and a separate synchronous preference-save path that includes a file flush and rename. These are concrete blocking boundaries, not proof of a particular multi-second user-visible incident.

### 修正設計與回歸方法 / Fix design and regression method

- UI 繪製讀取最近一次完整發布的 metadata；capture 忙碌時以 `try_lock`／`try_render_waveforms` 沿用上一份完整資料，不改壞已顯示的波形緩衝。
- 快照請求立即回傳接收器，實際取樣複製放在 capture worker；UI 之後輪詢結果，再使用固定素材進入編輯或準備匯出。
- 明確 Pause／Play 仍是控制命令，和一般繪製、選區及編輯視野分離。偏好保存改由背景工作處理，執行中的 UI 輪詢錯誤；明確 `flush` 與 writer 結束仍須等實際寫入，不能把這兩個邊界說成非阻塞。
- controller 回歸會刻意持有 capture mutex，直到模擬繪製返回。舊版無法在 100 ms 的檢查界線內完成；修改後應在鎖仍被持有時返回快取資料，並保留舊 peak buffers。**這個測試證明的是非阻塞邊界，不是本機實際發生了 100 ms 或秒級卡頓。**

The regression deliberately holds the capture mutex until simulated paint returns. The baseline misses its 100 ms completion bound; the revised path returns cached metadata/peaks while the mutex is still held. It tests nonblocking behavior under controlled contention, not the observed duration of a real UI freeze.

### 核心探針初步觀察 / Preliminary core observations

以下先列[初次探針](benchmarks/2026-11-14-responsive-editing/retention-initial.jsonl)，單位為毫秒；建立、縮短及快照階段各只有一次觀測，波形掃描有 20 次，區塊追加各有 6,000 次。它們不代表新版各平台的整體效能驗收。

| 操作 / Operation | n per case | 48 kHz stereo | 96 kHz stereo | 48 kHz stereo A/B |
| --- | ---: | ---: | ---: | ---: |
| 基礎建立 / Construct | 1 | 0.7751 | 0.3306 | 0.4720 |
| 600 → 180 s | 1 | 9.4209 | 17.7460 | 17.2019 |
| 180 → 30 s | 1 | 2.7955 | 5.0302 | 4.9714 |
| 180 → 600 s limit | 1 | 0.0006 | 0.0004 | 0.0002 |
| 複製 600 s 單路快照 / Copy lane snapshot | 1 | 42.4938 | 81.7917 | 67.9370 |
| 384 bins、雙 trace 掃描 median | 20 | 1.0342 | 2.2918 | 0.9350 |
| 4,096 bins、雙 trace 掃描 median | 20 | 3.1213 | 5.1696 | 3.4351 |
| 首 60 s、10 ms 區塊 append max | 6,000 | 0.4559 | 0.1961 | 0.4246 |
| 末 60 s、10 ms 區塊 append max | 6,000 | 0.2768 | 0.1256 | 0.5302 |

提高上限的 setter 幾乎不做立即配置；較長容量隨後續音訊增加。表內 180 → 30 秒階段是在先提高上限至 600 秒、沒有補回音訊後執行，當時內容仍只有 180 秒。已縮短內容上的 1,000 次 expand/shrink pair 最大為 0.0002／0.0002／0.0004 ms，**不是每次重新填滿 600 秒再釋放**。首次追加區塊未顯示秒級冷啟動，不能據此判定重新開機後的狀態。

The limit-increase setter does not immediately allocate the full requested history. The 1,000 warm expand/shrink pairs operate on already-shortened content; they are not repeated full-buffer refills. The observed core timings did not reproduce a multi-second stall.

正式範例的[第二次重跑](benchmarks/2026-11-14-responsive-editing/retention-example-rerun.jsonl)有明顯變異。以下每格為「初次 → 重跑」，保留較慢結果，不能把兩次當作修正前後的 A/B 比較：

| Case | 600 → 180 s，n=1 | 180 → 30 s content，n=1 | 單路 snapshot，n=1 | 4,096 bins median，n=20 |
| --- | ---: | ---: | ---: | ---: |
| 48 kHz stereo | 9.4209 → 11.4556 | 2.7955 → 3.2422 | 42.4938 → 55.0034 | 3.1213 → 4.6799 |
| 96 kHz stereo | 17.7460 → 25.8434 | 5.0302 → 7.8156 | 81.7917 → 191.0946 | 5.1696 → 11.2410 |
| 48 kHz stereo A/B | 17.2019 → 22.2858 | 4.9714 → 6.5432 | 67.9370 → 91.9242 | 3.4351 → 4.7619 |

第二次 96 kHz 首 60 秒區塊 append 最大值為 1.7190 ms；兩次提高上限仍各低於 0.001 ms。背景負載與 CPU 頻率未控制，這些差異不能歸因特定修正、paging 或重啟初始化。

The second run varied substantially, including a 191.0946 ms one-lane snapshot at 96 kHz. Both complete result sets are retained. They are not a controlled before/after experiment, and neither reproduces a seconds-long UI freeze.

### 偏好保存的獨立量測 / Standalone preference-save timings

[完整結果](benchmarks/2026-11-14-responsive-editing/synchronous-preference-save.json)來自獨立 Release harness，包括真實 `Preferences::save` 的建立目錄、寫暫存檔、`sync_all`、關檔及 rename；不是只量 `sync_all`。controller enum 是供 parsing／default helpers 編譯的替身，每一筆寫入仍使用 production save。所有路徑都在 workspace 的 `target/`，未建立或寫入使用者真實設定目錄，並讀回確認最後保留時間為 600 秒。

| 情境 / Case | n | Median ms | P95 ms | Max ms |
| --- | ---: | ---: | ---: | ---: |
| Fresh-directory first save | 8 | 16.84635 | 38.31071 | 48.08810 |
| Existing-directory history replacement | 64 | 13.04415 | 25.403295 | 54.83320 |

第一筆 fresh save 為 48.0881 ms。新目錄不是冷開機、清快取、HDD 或防毒模擬；這些是整個 durable-save 呼叫時間，不是 GUI 幀延遲，也不能當成背景 writer 的完成時間或吞吐量驗收。

This standalone run measures complete durable saves, not isolated flush time or GUI latency. Fresh directories do not imply cold boot, cache eviction, HDD, or antivirus emulation. It supports removing synchronous disk work from the UI path without proving the original reported freeze's cause. See the evidence README for the archived harness and reconstruction command.

### 背景保存 API 的後續量測 / Background-writer API follow-up

另一個獨立 Release 程序使用當時 production `PreferenceWriter::new/save/flush` 及真實 durable sink，依序執行 8 個 fresh-writer round，每次快速排入 64 次 History 變更（共 512 次），另外量測 flush。[完整後續數據](benchmarks/2026-11-14-responsive-editing/asynchronous-preference-save.json)保留每次值與各 round；8 次成功 flush 後皆讀回 600 秒，error poll 皆為空。

| 階段 / Stage | n | Median ms | P95 ms | Max ms |
| --- | ---: | ---: | ---: | ---: |
| Constructor | 8 | 0.05340 | 0.06734 | 0.06860 |
| Single enqueue | 512 | 0.00010 | 0.00040 | 0.01260 |
| 64 enqueues total per round | 8 | 0.01775 | 0.029835 | 0.03330 |
| Separate flush | 8 | 11.33770 | 32.56018 | 43.19500 |
| Drop after flush | 8 | 0.07175 | 0.174315 | 0.21320 |

`save` 成功只代表驗證與排隊，不是已持久化；同一 key 的待處理更新可合併。只有成功 flush 加讀回才確認最後值，不能把 enqueue 的 0.0001 ms 說成磁碟寫入時間或 GUI 延遲，也不能跨兩次未控制負載的測試宣稱修正加速倍率。接近計時器解析度的小值（包括原始資料中的 0）不代表零成本。

**這份後續量測早於後來的 queue-drain／合併調整與最終審查。**[來源紀錄](benchmarks/2026-11-14-responsive-editing/preference-probe-provenance.json)保留當時 production source、harness 及執行檔雜湊，不宣稱它們與最終版本相同。後續實作將每批額外 drain 限為 1,024 次，避免持續 producer 無限延後落盤；這項邊界不在上述量測版本中。

明確 flush 與 Drop 仍等待實際磁碟，以保留持久化語意。表中的 Drop 是**已完成 flush 後**的 drop，不能用其 0.07175 ms 中位數保證實際退出速度。執行中可輪詢保存錯誤；Windows 退出後的錯誤只寫 stderr，不等同於有可見的完成／失敗提示。最終程式檢查與 Windows 操作範圍列於下方。

A successful enqueue validates and queues work; it is not a durable write. The separately timed flush plus reload establishes each final value. This later run is not a controlled speedup comparison. It predates subsequent queue-drain/coalescing and review changes, so its recorded source/binary hashes are historical measurement provenance, not a final-binary latency guarantee. The complete measured production source was not archived; the preserved harness rebuilds against the current source and cannot recreate the historical binary exactly.

## 目前程式檢查 / Current code checks

最終重新執行的程式檢查如下。Windows／插件 Release 已重建，Windows 清理文案後的原生檢查亦完成；這些數字不宣稱所有平台已驗收。

| 範圍 / Scope | 最新結果 / Latest result | 邊界 / Limit |
| --- | --- | --- |
| Shared core | 61 passed / 1 ignored；另 2 個 example 檢查通過 | 核心與範例檢查，不是 GUI／宿主／裝置音訊量測 |
| Windows desktop | 47 passed / 2 ignored | 程式測試及下方指定原生操作；不是未執行手勢的驗收 |
| Plug-in | 46 passed / 0 ignored | 含全段 Save 背景準備等回歸；不等於各 DAW 的 VST3／CLAP 全流程驗證 |
| macOS | 遠端 native app build／codesign 及指定 CI 測試通過（`55baeac`） | GUI 編輯、實體 Fn／Globe 與權限、DAW runtime 未驗證 |

These checks are distinct from broad platform acceptance. Local Windows observations are recorded below; remote macOS native build/test outcomes are recorded separately in the CI section.

## 遠端跨平台 CI / Remote cross-platform CI

[產品 commit `55baeac` 的執行結果](https://github.com/kekekewww/Backtracking/actions/runs/43537657689)已完成：Windows 與 macOS jobs 全部指定步驟成功，但 Linux 的 X11 快捷鍵驗證失敗，**整體結果為 failure**。其後 [workflow-only commit `f3d9b0a` 的 Linux 重跑](https://github.com/kekekewww/Backtracking/actions/runs/43538353959)也在同一步驟失敗；該輪 Windows／macOS 為 skipped，不能視為重新通過。

| Run / commit | Windows | macOS | Linux | 整體 / Overall |
| --- | --- | --- | --- | --- |
| `43537657689` / `55baeac` | success（含 Release） | success | failure：X11 shortcut activation / duplicate registration | failure |
| `43538353959` / workflow-only `f3d9b0a` | skipped | skipped | failure：同一 X11 步驟 | failure |
| `43539038043` / Linux diagnostic `8b2e68b` | skipped | skipped | failure：同一 X11 步驟，取得下方診斷 | failure |
| `43539505931` / Linux fix `52af947` | skipped | skipped | success：helper／Sender／原 X11 assertions | success（Linux-only） |

macOS 已通過原生 App 建置／驗證（包含 codesign）、signed update／rollback、Swift capture routes 與 paired WAV export、core／plugin tests。這是遠端 macOS runner 執行的建置及指定測試，**不是互動 GUI、實體 Fn／Globe 事件／輸入權限或 DAW 宿主操作的驗收，也不代表 notarization**。

Linux 前三輪的 shortcut-helper 與 independent Sender steps 通過，失敗的是 `Verify X11 shortcut activation and duplicate registration`。第二輪 stderr 前綴為 `No physical TLDE key or Super modifier ...`。第三輪[診斷執行](https://github.com/kekekewww/Backtracking/actions/runs/43539038043)確認同一個 Xvfb keymap 有 physical TLDE（49）、LWIN（133）、RWIN（134）及 Mod4，Super mask 正常為 `0x40`，但 `XkbGetKeyboard(...GBN_KeyNamesMask)` 回傳 NULL，TLDE 查詢因此為 0。這排除了「fixture 缺少實體鍵或 Super」的說法，也不把合法 ByName mask／limits 的用法說成錯誤。

這是額外找到的既有 Linux keyboard query-path 相容性問題。修正改用 active `XkbGetMap(AllClientInfoMask)` 再 `XkbGetNames(KeyNamesMask)`，保留 physical TLDE 查找，沒有硬編碼 keycode 或 symbol fallback，並清理錯誤路徑資源。第四輪 [Linux-only 驗證](https://github.com/kekekewww/Backtracking/actions/runs/43539505931)於 `52af947` 成功：helper、Sender、原 X11 registration、duplicate-client rejection 及 held-key 單次啟動的 `count -eq 1` assertions 都保留並通過，沒有弱化測試。

第四輪 Windows／macOS 為 skipped，不能記成在 `52af947` 重新驗證它們；兩者成功來源仍是 `55baeac`。Windows／macOS 產品程式與本機 Setup 雜湊未因這條 Linux 修正改變。X11 fixture 成功也不驗證 Wayland portal 或實體鍵盤／桌面 compositor 行為。

[公開 CI metadata](benchmarks/2026-11-14-responsive-editing/cross-platform-ci.json)保存 run／job／step 結果與 commit。下方 Windows 原生 GUI hash、本機插件與 Setup hash 仍是本機觀察／產物來源，**沒有改寫為 CI binary 的雜湊，也不宣稱 CI binary 與本機檔案相同**。

The original product run passed Windows/macOS but failed Linux X11; two retries also failed. Active-map/name querying fixed the recorded X11 fixture, and the fourth Linux-only run passed the unchanged registration/duplicate/held-key assertions. Windows/macOS were skipped in that run, so their successful source remains `55baeac`. Local artifact hashes remain separate from CI binaries. X11 CI does not establish Wayland or physical-keyboard/compositor acceptance; macOS build/tests do not establish GUI/Fn/DAW acceptance.

## Windows 原生操作實測 / Native Windows observations

此輪使用隔離的測試設定、合成音訊 Sender 及觀察尺寸 1,500 × 500 的視窗。測試執行檔 SHA-256 為 `0D31F1B6CC302D61C6D6CB258828158675DE9FE142727122C3022F99D8F3B279`；**它早於最後一行 footer 的移除及後續插件全段 Save 修正，不是最終打包版本雜湊**。以下是人工操作觀察與新 WAV 的唯讀 metadata 檢查，不是延遲或硬體用量量測。

| 原生操作 / Native action | 觀察 / Observation |
| --- | --- |
| History 下拉選單 | 7 個選項完整展開；10 s → 600 s → 60 s → 10 s 切換可操作，未觀察到長時間卡住，但沒有計時數據。 |
| 選段與 Play | 先按 Pause，再拖曳原生端點形成約 19.77 s 選區；按 Play 後，畫面及旗標固定，紅色錄音燈與 dBFS 持續更新。 |
| 縮短及 live rollover | 背景 History 改為 10 s，經過超過 10 s 後，固定選區仍顯示約 19.77 s。 |
| 選段快照 | Alt+grave 保存這個固定選段後回到即時視野；讀回檔案為 948,868 frames / 48,000 Hz = 19.768083333333333 s，stereo float32，所有樣本值有限。 |
| 縮放 | 原生 zoom 改變可見範圍及參考線密度。 |

[去識別的觀察與 WAV metadata](benchmarks/2026-11-14-responsive-editing/native-editing-observations.json)也保存第一份基準短快照：1,919,040 frames、48 kHz、stereo float32、39.98 s。當時較長的 History 尚在填入，**不能把這個 39.98 s 檔案記成已驗證 60 s**。

| 檔案角色 / Role | Frames | Seconds | All samples finite | SHA-256 |
| --- | ---: | ---: | --- | --- |
| 尚在填入時的基準短快照 | 1,919,040 | 39.98 | true | `d51bb7f7cfce636a762409901a25904797a355849e10e0134185b6cecd7777b7` |
| live rollover 後保存的固定選段 | 948,868 | 19.768083333333333 | true | `855bef12255f4cb56ab41df4ad0943c57e5aee9086273126110a06b83300a5df` |

這支持「19.77 s 編輯素材沒有被 10 s 即時保留上限重切，且可在 Play 後保留選區並保存」的本次觀察。顯示的 19.77 s 是四捨五入值，沒有拿已知來源起訖逐幀比對；有限樣本值也不等於沒有遺失／重複幀或可聽爆音。實測是一個 stereo Sender，沒有在原生 UI 完成雙來源 A/B 音訊對照。

原生實體 Shift-drag 沒有執行：本輪自動化無法在拖曳期間持續按住修飾鍵；Shift 行為由 RawInput／共享測試覆蓋，和原生端點拖曳分開記錄。這個功能測試版未做原生縮窗，後續清理文案後版本的縮窗觀察另列於下節。WAV 和私有截圖不公開。

Native endpoint editing preserved the approximately 19.77-second view/selection while capture indicators continued after Play. Reducing live history to ten seconds and waiting beyond rollover still produced a 19.768083333333333-second selected WAV. This is a single-stereo-Sender observation against the recorded intermediate build, not an exact source-sequence, A/B, artifact-free, or latency proof. Physical Shift-drag and native window resizing were not exercised in this run.

### 清理文案後的最終 Windows 版本 / Final Windows copy-cleanup build

最終桌面版雜湊為 `5A4A20484991E8C85249BAAAC22D2F453C4B5E886BC24A17A8029C7C4EA2390A`。以這個執行檔重新觀察原生介面，Options footer 為空白，沒有 helper 說明段落或浮動操作教學。視窗從觀察尺寸 900 × 300 原生拖縮至 700 × 230 仍可操作，波形、小節線與秒數線正常顯示；較早的 1,500 × 500 及 scroll-zoom 觀察屬於上一個功能測試版本。

所有合成 fixture 已停止；最終桌面版已使用原有使用者設定重新啟動，Options 乾淨可操作。這不等於把前述每個功能案例全部在最後版本重跑，也不是新的 CPU／GPU／延遲量測。

The final desktop hash above matches the natively checked copy-cleanup binary: empty Options footer, no instructional helper paragraphs or floating gesture tutorial, and operable resizing from observed 900×300 to 700×230 with waveform/bar/seconds rendering. Synthetic fixtures were stopped and the final desktop was restarted with the existing user profile. The earlier selection/rollover and 1500×500/zoom observations remain tied to their intermediate build.

### 最終產物 / Final artifacts

[建置與驗證清單](benchmarks/2026-11-14-responsive-editing/final-builds.json)保留桌面版、插件 DLL、Setup 的大小與 SHA-256。桌面版為 8,190,976 bytes；插件 DLL 為 7,344,128 bytes，雜湊為 `ED28229A16714BB2C18243FD5F4A2AFA2E51624920D376A300EAF9C4F3789866`。

最終 `Backtracking-0.1.5-Windows-x64-Setup.exe` 為 **3,486,718 bytes**，SHA-256 為 `A65B99A0B5C2A1502A3F58EB164CE75AD3D51AA36AE62857D6967A4A3E6F72B7`。本輪手冊固定後重新打包，payload 的桌面執行檔、VST3、CLAP 及 USER_GUIDE 均與各自現檔逐項 bytes 比對相同；這是打包完整性檢查。

Setup 本輪只重新打包 payload，安裝程式邏輯沒有改動。較早的 17 項編譯情境及 19 項安裝測試屬於歷史驗證，**本輪沒有拿新 payload 重跑這些安裝情境**；不能把產物成功建置、payload 相符及雜湊核對寫成新版安裝／解除安裝完整驗收。

The final Setup was rebuilt after freezing the guide. Its desktop, VST3, CLAP, and bundled-guide payload bytes match their sources. Installer logic was unchanged; the earlier 17 compile scenarios and 19 installation cases were not rerun against this payload.

## 限制與待驗證項目 / Limits and pending validation

- 尚未在現機重現使用者描述的秒級卡死；沒有證據可宣稱記憶體置換、冷開機或重新啟動初始化是原因。新程序、第一次配置與真正重新開機是不同條件。
- 上述 Windows 原生操作只涵蓋指定版本與步驟；macOS 原生 build／指定 CI tests 與 Linux X11 原 assertions 已於所列 commits 通過。Mac GUI／實體 Fn、各 DAW、原生實體 Shift-drag、Wayland／實體 Linux 桌面及新版 payload 完整安裝仍未驗證。
- 編輯／取消不強制解除手動 Pause、0 秒手動暫停維持、5／10 秒既有自動續錄、跨 rollover 的選段保存及 A/B 原始取樣對齊，須分清程式回歸與原生實測範圍；本次原生觀察不是全部情境的證明。
- 吸附優先序、可見目標及 modifier 規則有 RawInput／共享測試覆蓋，但本輪未執行原生實體 Shift-drag；macOS／各 DAW 的實體手勢仍需相應環境驗證。
- Windows 的指定字級／選單／捲動程式檢查與原生 7 項 History、縮窗已記錄；macOS 原生 build 與 CLI／Swift 路由／成對 WAV 檢查不包含互動 Options，macOS 及 DAW 的鍵盤、捲動、最小尺寸與外框仍需相應環境檢查。
- 核心探針沒有同時量測 GUI、DAW、GPU、背景錄音延遲、缺幀或可聽爆音；不能用它取代新的硬體監測或音訊品質驗收。

The reported code checks, builds, and specified Windows observations are complete. The remaining platform/gesture/installer scopes above are explicitly unverified, and this bounded investigation does not claim to have reproduced or completely explained the reported multi-second freeze.
