# 安靜片段與連續波形：本輪驗證紀錄 / Quiet capture and continuous waveform

**狀態：實作、本機套件、指定 Windows GUI、本機打包核對及同 source 三平台 CI 完成。** 本頁記錄 source commit `5fe9224c3dc54d0a2a655ec6b66522c43bd7a238` 的 quiet retention 與混合 renderer。本機套件及下列 native quiet smoke 已通過；深度 zoom 的 Windows GUI 觀察屬中間 Release，最終成品、payload 與正常啟動分開核對。已完成檢查不代表所有平台 GUI／DAW runtime 驗收。既有[硬體用量](HARDWARE_USAGE.md)與[固定素材互動報告](RESPONSIVE_EDITING_2026-11-14.md)及其 evidence、build hashes 保留原測試版本，不改寫成新功能的證據。

## 本輪範圍

| 項目 | 採用規則 | 驗證狀態 |
| --- | --- | --- |
| 保留安靜片段 | Windows／macOS／VST3／CLAP 增加預設 Off 的 Keep quiet sections；macOS 標籤為「保留安靜片段」 | 本機套件通過；Windows GUI 預設 Off／切換已觀察，macOS GUI／DAW 互動未驗 |
| 擷取內容 | On 保留往後送達的 quiet／zero 及已知來源時鐘可確認的安靜；Off 沿用靜音壓縮 | 程式回歸通過，Windows 隔離來源與 Mac native harness 另有結果；真實 DAW／各種裝置未普遍驗證 |
| 狀態與素材 | 切換不清 history、不重切固定素材／A/B、不改手動 Pause；不補回先前丟掉的 quiet，也不回溯壓縮已保留片段 | 本機整合回歸通過；Windows native quiet case 檢查 Pause 與固定 snapshot range |
| 混合 renderer | 遠／中倍率連續 filled min/max envelope＋outline；近倍率連接原始 samples | 本機套件通過；Windows 10.00→4.51→0.42→0.04→0.01 秒 zoom 已觀察連續畫法 |
| 共用幾何 | Empty／Envelope／Samples；Envelope 保留 bin 邊界與中心的 min/max；Samples 要求一對一原始 frames，使用 `x = i / N`，無 half-bin offset；helper 接收的非有限值分 runs | 本機 core／平台套件已通過；不等同所有平台原生畫面驗收 |
| 縮放與視窗 | 最小範圍依實際 occupied width，最深約四個邏輯像素／sample；輸入有界，resize 保留視野，後續 wheel zoom 遵守有效上下限 | 自動化回歸通過，Windows wheel zoom 已觀察；本輪未新增 native resize、Mac／DAW GUI 實測 |
| Windows 原生 quiet smoke | 隔離的無聲 Process Loopback 來源；檢查 Off／On／Pause／Resume 與固定記憶體 snapshot | 首次 crash 保留；ownership 修復後同 case 通過，見下節 |
| UI | 說明放文件；UI 保持控制、標題、值及必要的狀態／錯誤／路徑 | 中間 Release 已觀察無 helper；最終僅移除次要 Options 標題 PID，沒有冒稱重做 final GUI |

方案與更新前的點狀／綠色密集柱狀分析見[波形顯示研究](WAVEFORM_DISPLAY_RESEARCH_2026-11-14.md)；工作項目見[本輪計畫](superpowers/plans/2026-11-14-retain-quiet-sections.md)。不複製使用者私人圖片、音訊或其本機路徑。

## 驗證方法與限制

- 擷取測試分開檢查送達的低音量、數位零值、已知時鐘 gap、On／Off 的 pending-input 邊界、history 上限、A/B 對齊、可用 marker 與固定素材。新增 quiet 不代表重新建立先前丟掉的音訊。
- Windows 無封包 padding 只針對可用、連線中的適用擷取來源與前進時鐘；未連線來源或斷線 Sender 不憑空產生未知音訊。macOS 完全沒有 callback 時，須等後續真實 timestamp 才確認相應區間，不能先承諾尾端即時補齊。
- 插件僅在 **CLAP live processing** 且 `retain_silence || sender_enabled` 時回傳 KeepAlive／`CLAP_CONTINUE`；離線處理與 **VST3** 維持 Normal，VST3 為 `kNoTail`，避免宣告 infinite tail 而改變 offline export／freeze。即時執行緒使用 `AtomicBool`，沒有新增 vendor hook、dummy parameter、restart 或 wake 呼叫。公開 `GuiContext`／`ProcessContext` 未提供安全的 `request_process` 入口；已休眠或停止的宿主須有下一個真實 callback 才能生效，沒有 callback 時不捏造音訊。實際 DAW 行為尚未驗證。
- 幾何測試要涵蓋 peak、zero、DC、脈衝、非有限值 runs、bin 邊界、原始 sample 座標及 mode／zoom 邊界。原生觀察另查遠近外觀、縮放、pan、resize、選區與 A/B；平滑外觀不等於音訊完整性證明。
- helper 的非有限值分段只涵蓋它收到的資料；既有 extrema／view-peaks 可能將全 NaN bin 轉成零，不能宣稱整條顯示管線已保留所有 NaN gaps。
- 新 CPU／記憶體／frame-time 數字若未實測就不列出；舊硬體 trace 不作新版 renderer 的效能結果。中間開發版的單一套件計數也不當作最終整合通過。

## Windows 原生 quiet smoke：失敗、修復與重跑

去識別的兩次結果見 [native-quiet-smoke.json](benchmarks/2026-11-14-quiet-waveform/native-quiet-smoke.json)；[證據說明](benchmarks/2026-11-14-quiet-waveform/README.md)列出重現方式與邊界。測試在 debug test binary 內使用專用 Process Loopback，來源是純 std、沒有建立音訊串流或播放的子程序。它驗證精確 PID、workspace `target` 內的 `backtracking-quiet*` 執行檔，不擷取 System Audio，也沒有 fallback。音訊 snapshot 僅在記憶體；On 階段的固定 snapshot 檢查為全零，不寫 WAV、使用者偏好、registry 或 UI。

首次在 **2026-11-14 02:23:28 UTC** 執行未完成，程序以 **`0xc0000374 / STATUS_HEAP_CORRUPTION`** 結束。新 native opt-in 測試揭露既有 Process Loopback activation ownership 問題：在 baseline `d321bd310f284b9df5e807ada4c13ec6a7f741b7` 已存在的 `VT_BLOB` 借用 Box 內的 params，但 windows 0.62.2 的 `PROPVARIANT` Drop 會呼叫 `PropVariantClear`，把該借用指標當成 owned allocation。

修復以外層 `ManuallyDrop<PROPVARIANT>` 保留 Box 的唯一 ownership，property 使用 `&*variant`，completion callback 持有 Box 到最後一個 COM reference 結束。實作者回報 64 次 construct／clone／drop 的專屬回歸通過，並完成獨立程式 review；最終完整套件結果另列，不將單一回歸當作全產品驗收。

修復後同一 native case 於 **2026-11-14 02:30:00 UTC** 通過：**1 passed／0 failed，3.27 秒**。來源為 48 kHz，觀察如下。

| 階段 | 記錄值與通過的 case assertion |
| --- | --- |
| 預設 Off | 無聲來源不推進歷史 |
| Quiet On | 新增 28,800 frames；該階段觀察區間 0.5932481 秒，並檢查未回補先前 Off 區間 |
| 切回 Off | recorded-end 為 29,760；完成切換後等待期間維持不變 |
| 手動 Pause＋On | recorded-end 仍為 29,760，保持 Pause |
| Resume | 新增 12,435 frames；觀察區間 0.3066807 秒，檢查未回補 Pause 區間，固定 snapshot 的 frame range 不變 |

這些 elapsed intervals 與 counters 是這個 case 的觀察，不是零延遲保證。首次失敗與成功 run 的 fixture executable hashes 分別保留；首次未記錄 fixture source hash，不回填推測值。成功 run 的 fixture source SHA256 為 `AE09D1B4459CF520CBA5917D60742CD6FF1ED2D00FE1EAD716F0856362C8175E`。不將此 ownership 修復歸因於先前秒級 UI 卡頓、paging 或啟動行為；也未由此驗證 macOS、Fn、DAW、各種真實裝置、波形原生 GUI 或普遍無缺幀／無爆音。

## 最終測試、原生操作與建置

本輪整合者重新執行後回報的本機結果如下；[local-validation.json](benchmarks/2026-11-14-quiet-waveform/local-validation.json)保留來源與範圍。單一 native case 與預設忽略的測試分開計算。

| 檢查 | 本輪本機結果 |
| --- | --- |
| Core library | **82 passed／1 ignored**；ignored 為 TCP loopback |
| Timeline Sender example | **2 passed** |
| Windows desktop | **73 passed／3 ignored** |
| Windows native quiet smoke | **1 passed**，另外 opt-in 執行，詳見上節 |
| VST3／CLAP plug-in | **64 passed／0 ignored** |

Windows 中間 Release SHA256 為 `5B95B567EB5858447C0EAD266CB1C31136CD3C032767CC03C914FAB6BD2E0C7A`。該 GUI 已觀察 Capture checkbox 預設未勾選，On→Off 可切換，沒有 helper 段落；Clear 後填滿的 **10.00 秒**波形呈連續 envelope。其後手動 Pause，以 wheel 逐步 zoom 至 **4.51／0.42／0.04／0.01 秒**，畫面均保持連續，沒有 comb 或散點外觀。視野上的短跨度受手動 zoom／history 狀態影響，不能單憑顯示時間推斷擷取缺幀。這些是指定功能觀察，沒有量測 frame time、CPU 或匯出音訊品質，也沒有新增實體 Shift-drag 或 native resize 實測。

此 SHA 是移除 Options 次要標題 PID 之前的受測建置，**不是最終重建或 Setup payload hash**。原生 GUI 隔離測試已結束。最終正常 tray／toolwindow 無法由 computer-use inventory 選定，因此沒有冒稱在最終執行檔重新操作 GUI。不覆寫先前報告的 runtime hash，也不把 build 成功當成 DAW 或全平台操作已驗收。

### 最終本機成品與已安裝桌面版

[package-validation.json](benchmarks/2026-11-14-quiet-waveform/package-validation.json)保留本機成品與去識別 refresh metadata。Setup 的 desktop、VST3、CLAP 及固定 Guide 四個 payload 已逐一與來源 bytes 比對一致，hash／size 也另行重核。

| 成品 | Bytes | SHA256 |
| --- | ---: | --- |
| Windows desktop Release | 8,248,320 | `0F22316808DD6001C03633B711546890D4DE20B7AFEC45B9EB696E94DD4F588C` |
| Windows VST3／CLAP DLL payload | 7,362,048 | `B241A8497AF2DE172919C50724919D372CA4F6FEFFD92B1266C4F94F6B689766` |
| Windows Setup | 3,503,467 | `48BF289A04AF0B71A09EAEBC899BEA65AA3CED79EB887B5931A69C9F2BA7B3D8` |
| 隨附 USER_GUIDE.md | 52,648 | `3B0123DB559B67A492D1CB3E239137EA477677B3749DA33AD26A7E7710D88767` |

實際已安裝桌面版於 **2026-11-14 03:02:57 UTC** 以備份後的 desktop-only refresh 更新 `Backtracking.exe`、`USER_GUIDE.md`、`BUILD-INFO.txt`，正常無參數啟動，執行檔 hash 與最終 desktop `0F2231…F588C` 一致。這次沒有更新已安裝插件；其既有 DLL hash 為 `5C24211C6755D4C5A3F9CE20BF3FE8D878B95F688D0650CAB18610B7B6A6877F`，不可把它寫成新版 `B241…9766` 的 DAW 載入驗證。Installer 邏輯未改，本輪核對新 payload，**沒有重跑新版 Setup 的 install／uninstall**。

### 本輪遠端 CI：同一 source 三平台通過

同一 source commit 的 [run 43549431632](https://github.com/kekekewww/Backtracking/actions/runs/43549431632) 已完成，**整輪 SUCCESS，Windows／macOS／Linux 三個 jobs 全部成功**。最後 Windows job 於 **03:07:32 UTC** 完成，包含 capture core、native desktop、native plug-in 三個 test steps 及 Windows Release build；各步驟時間與結論保留於 [cross-platform-ci.json](benchmarks/2026-11-14-quiet-waveform/cross-platform-ci.json)。

macOS 完成 app build／驗證、signed update／rollback 及 native Swift harness。在 **03:02:43 UTC** 的 harness 輸出確認 waveform geometry、capture routing、quiet retention、alignment 與 A/B WAV export 通過；routing entry 實際呼叫 geometry／RenderGate assertions，不只是編譯未執行。macOS core 為 **82 passed／1 ignored**，plugin 為 **64 passed／0 ignored**。Linux job 完成快捷鍵 helper、獨立 Sender connections 與 X11 activation／duplicate registration 驗證。

Mac GUI、Fn、DAW、實體 Shift-drag、本輪新增 native resize 與 Linux Wayland／實體 compositor 操作仍未驗證，也沒有新版 renderer 的 CPU／RAM／frame-time 量測。CI 成功不替代這些互動／裝置條件，遠端產物也不改寫上表本機成品與中間 GUI build 的來源。
