# 硬體使用量 / Hardware usage

本頁分開記錄各平台、版本與測試方式。Windows 為實機桌面版監測；下方保留較早的 Linux CLAP／共用核心資料。不同日期、版本、宿主及繪圖後端的數據不直接相比。

Windows results below cover the desktop application on a physical PC. The earlier Linux CLAP/shared-core dataset is preserved separately. Different builds, hosts, dates, and rendering backends are not controlled comparisons.

## Windows 桌面版：2026-11-14 / Windows desktop

本輪完成 7 種保留時間、波形模式及透明度、合併波形與三種音量表、長時間錄音後段、暫停、縮短保留時間及間歇測試音的資源監測。測試視窗在 600 秒設定下持續擷取超過 10 分鐘；這是時間與介面觀察，**原 600 秒內容尚未以 WAV 驗證，缺幀或爆音亦未驗證**。其後另完成一個新短片段的快照格式與樣本值檢查，見下方後續小節。

This run exercised seven history limits, waveform/transparency settings, merged view and three meters, extended recording, pause, shorter history limits, and gated audio. Capture continued for over ten minutes with a 600-second limit. **The earlier 600-second retained content, frame continuity, and click-free audio were not verified.** A separate short snapshot was exported and inspected afterward, as reported below.

### 環境與測試範圍 / Environment and scope

| 項目 / Item | 記錄 / Record |
| --- | --- |
| 時間 / Time | 2026-11-13 15:59:55–16:32:28 UTC；台灣時間 2026-11-13 23:59:55 至 11-14 00:32:28 |
| 系統 / OS | Windows 11 Home 64-bit, 10.0.26200 / build 26200 |
| CPU | Intel Core i9-13980HX, 24 cores / 32 logical CPUs |
| 記憶體 / Memory | 33,959,825,408 bytes visible physical memory, approximately 31.6 GiB |
| 顯示裝置 / Display devices | Intel UHD Graphics, NVIDIA GeForce RTX 4070 Laptop GPU, IddSampleDriver virtual display driver |
| 應用程式 / App | Backtracking v0.1.5 Release；以 `d321bd3` 為基礎的修改中工作目錄 / modified working tree based on `d321bd3` |
| 測試執行檔 SHA-256 | `1A26B26399B3ACA9BCAE220F7F1D9ED0722025285C630784D5DA7CB26C3A54DB` |
| 工具鏈 / Toolchain | rustc 1.95.0 (`59807616e`, 2026-04-14), cargo 1.95.0 (`f2d3ce0bd`, 2026-03-21), `x86_64-pc-windows-msvc`, LLVM 22.1.2; Python 3.12.11; ffplay 8.0.1-full_build-www.gyan.dev |
| 擷取 / Capture | System Audio，單一來源、兩聲道；合成音訊由 ffplay 循環播放 / one stereo source, synthetic fixture looped by ffplay |
| 視窗 / Windows | 獨立次要擷取實例，`--new-capture-window --show-options`；Options 始終可見 / independent secondary instance with Options visible |
| 原有實例 / Original instance | 持續執行且設定未改；另外監測，數據不加進測試實例 / continued with unchanged settings and a separate collector |

硬體型號不代表本輪已確定哪張 GPU 負責繪圖。此處只報 Windows 的程序 GPU 引擎計數器，未建立引擎與實體顯卡的對應；`Win32_VideoController.AdapterRAM` 是 32-bit 欄位，可能截斷 VRAM 容量，故不列為實際顯示記憶體容量。插件 DLL 的雜湊留在硬體清單作為建置追蹤，不代表已測插件效能。

The GPU adapter list does not identify which device rendered the app. Results use per-process Windows GPU engine counters without an engine-to-adapter mapping. The 32-bit `AdapterRAM` field is omitted because it can truncate VRAM capacity. A plug-in binary hash in the manifest identifies a build artifact and does not imply that plug-in performance was measured.

### 方法與單位 / Method and units

[`generate_test_audio.py`](../tools/generate_test_audio.py) 產生 48 kHz、stereo PCM16 測試音，峰值約 −21.94 dBFS。連續音檔每 20 秒循環；間歇音檔每 30 秒循環，每 5 秒音訊接 5 秒數位靜音，音訊邊緣有 10 ms 淡入淡出。[音訊產生規格與雜湊](benchmarks/2026-11-14-windows/audio-fixtures.json)只包含測試素材的描述及數據；未公開擷取音訊，也未改系統音量。

Fixtures are deterministic 48 kHz stereo PCM16 at approximately −21.94 dBFS peak. The continuous fixture loops every 20 seconds. The gated fixture alternates five seconds of audio with five seconds of digital silence and uses 10 ms edge fades. Fixture metadata and hashes are linked above; captured audio is not published, and system volume was not changed.

[`monitor_backtracking.ps1`](../tools/monitor_backtracking.ps1) 約每秒取樣程序 CPU、working set（RSS）、private bytes、I/O 及全機負載；GPU 每約 5 秒在背景查詢。本輪 GPU 使用 `Get-Counter` 回退路徑。**單核心 CPU 的 100% 是一個邏輯 CPU；全機 CPU 則除以本機 32 個邏輯 CPU。**例如程序 6.159% 單核心相當於約 0.193% 全機，不是全機 6.159%。MiB 為 1,048,576 bytes。

[`summarize_hardware.py`](../tools/summarize_hardware.py) 依 collector、session 及 collector 的情境序號分組，排除切換期與穩定階段前 5 秒。表內為有效樣本數、首末有效取樣的時間差，以及中位數；CPU 另列 P95。GPU 必須符合當前情境、`transition=false`，且查詢開始時間在暖機結束之後，再以 session／`observed_utc` 去除快取重複。GPU 主欄位為最忙引擎，不是整張顯卡的總用量；引擎用量加總可能超過 100%。P95 使用排序樣本的線性插值。

CPU/RAM are sampled approximately once per second; asynchronous GPU queries run approximately every five seconds through the performance-counter fallback. One-core CPU is normalized to one logical processor; machine CPU is normalized to 32. Transitions and the first five seconds of stable phases are excluded. GPU queries are matched to the phase, filtered by collection start time, and deduplicated. Tables show effective sample counts, elapsed coverage between the first/last retained samples, medians, and CPU P95. Full min/median/P95/max/mean values are in the summary JSON. Process I/O includes cached/non-disk I/O, and system CPU includes unrelated apps and measurement tools.

測試實例共記錄 1,735 筆，排除切換與暖機後使用 864 筆 CPU／記憶體樣本、164 筆獨立 GPU 觀測。這是單次連續測試，不是多次獨立程序的效能基準；短階段與長階段的樣本數不同。

The test instance produced 1,735 records, with 864 CPU/memory samples and 164 independent GPU observations retained after filtering. This is one continuous run, not a repeated fresh-process benchmark; phase lengths differ. Power mode, CPU clocks, and other applications' background activity were not controlled.

### 保留時間設定 / History settings

完整介面、L/R 波形、dBFS，背景不透明，Options 停在 Capture；小節線與 5 秒參考線設定啟用。來源沒有 DAW 時間資料，本輪不驗證小節線位置。以下依測試順序，較長保留時間仍在累積音訊，**不是每組都已填滿所選時長**。

Full interface, L/R, dBFS, opaque background, Capture Options visible, with bars and a five-second grid enabled. The source supplied no DAW timing, so bar placement was not validated. Longer-history rows were measured while history was still growing; they do not represent prefilled buffers.

| 情境 / Phase | 有效 n / 秒 | CPU 單核心 median / P95 % | CPU 全機 median % | RSS median MiB | Private median MiB | GPU 最忙引擎 median % / n |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 10 s (`2`) | 136 / 136.5 | 4.641 / 17.380 | 0.145 | 169.9 | 181.6 | 0.236 / 27 |
| 20 s (`4`) | 18 / 17.3 | 5.460 / 10.471 | 0.170 | 170.0 | 181.6 | 0.261 / 4 |
| 30 s (`6`) | 38 / 37.4 | 4.635 / 14.564 | 0.145 | 170.0 | 181.5 | 0.202 / 7 |
| 60 s (`8`) | 47 / 46.6 | 6.196 / 20.377 | 0.194 | 170.1 | 181.6 | 0.228 / 9 |
| 3 min (`10`) | 54 / 53.6 | 4.656 / 9.779 | 0.145 | 185.2 | 196.9 | 0.210 / 10 |
| 5 min (`12`) | 34 / 33.4 | 6.111 / 9.767 | 0.191 | 211.4 | 223.2 | 0.175 / 6 |
| 10 min, growing (`14`) | 30 / 29.4 | 6.184 / 16.051 | 0.193 | 232.4 | 244.0 | 0.300 / 6 |


20 秒設定只有 18 筆有效樣本（約 17.3 秒覆蓋），不足以視為同樣長度的重複比較。這 7 組 CPU 中位數約為 4.6–6.2% 單核心／0.14–0.20% 全機；隨後記憶體增加同時受持續累積的音訊影響。

The 20-second setting has only 18 usable samples (17.3 seconds of coverage). Across these seven phases, median CPU was approximately 4.6–6.2% of one logical CPU, or 0.14–0.20% of the machine. Increasing memory also reflects retained audio accumulating over time.

### 波形、透明度與音量表 / Waveform, transparency, and meters

下列階段都保持 600 秒上限及 Options 可見。透明度 0% 是不透明，100% 是全透明。切換到僅波形時，工具取得的主視窗區域高度由 144 降至 61 像素；Options 的直向清單及捲動可操作。這是本次視窗與 DPI 的觀察。

All rows kept the 600-second limit and visible Options. Transparency is 0% for opaque and 100% for fully transparent. The captured main-window region shrank from 144 to 61 pixels high in waveform-only mode, and the vertical Options list could be scrolled. These dimensions apply to this observed window/DPI configuration.

| 情境 / Phase | 有效 n / 秒 | CPU 單核心 median / P95 % | CPU 全機 median % | RSS median MiB | Private median MiB | GPU 最忙引擎 median % / n |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Waveform / L-R / 0% (`16`) | 30 / 29.3 | 6.160 / 7.790 | 0.193 | 255.3 | 266.2 | 0.238 / 5 |
| Waveform / L-R / 50% (`18`) | 34 / 33.4 | 6.096 / 12.339 | 0.191 | 277.9 | 289.0 | 0.191 / 6 |
| Waveform / L-R / 100% (`20`) | 36 / 35.4 | 6.152 / 10.773 | 0.192 | 300.5 | 311.4 | 0.181 / 7 |
| Full / Merged / dBFS (`22`) | 36 / 35.4 | 4.651 / 9.481 | 0.145 | 343.2 | 355.3 | 0.198 / 6 |
| Full / Merged / VU (`24`) | 38 / 37.5 | 6.149 / 9.397 | 0.192 | 366.1 | 378.0 | 0.190 / 7 |
| Full / Merged / LUFS (`26`) | 34 / 33.4 | 4.650 / 7.742 | 0.145 | 380.0 | 391.8 | 0.193 / 6 |


**這些數字不能用來歸因外觀選項的成本。**各階段依序進行，錄音緩衝仍在增長，且沒有固定滿緩衝、相同視窗位置／前景狀態的重複 A/B 比較。RSS 從 255.3 增至 380.0 MiB 不代表透明度或 LUFS 本身增加了同樣多的記憶體。

**These are observations, not isolated setting costs.** History was growing across sequential phases; buffer occupancy, foreground state, and repeated A/B runs were not controlled. The RSS increase from 255.3 to 380.0 MiB cannot be attributed to transparency or LUFS alone.

### 長錄音、暫停、縮短及間歇音 / Extended recording, pause, shrink, and gated audio

600 秒設定從約 16:14:48 UTC 持續到 16:28 的暫停操作，超過 10 分鐘。下表前兩列是其後段；完整介面、合併波形、LUFS，Options 可見。WAV 輸出選項設為 48 kHz float32，但未執行匯出。

The 600-second limit remained selected for over ten minutes before capture was paused around 16:28 UTC. The first two rows are later observations with full/merged/LUFS and visible Options. Export settings were 48 kHz float32, but no WAV export ran.

| 情境 / Phase | 有效 n / 秒 | CPU 單核心 median / P95 % | CPU 全機 median % | RSS median MiB | Private median MiB | GPU 最忙引擎 median % / n |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 600 s, late phase 1 (`28`) | 98 / 98.2 | 6.159 / 12.582 | 0.193 | 427.0 | 401.3 | 0.190 / 20 |
| 600 s, late phase 2; export not run (`29`) | 40 / 39.5 | 4.639 / 10.786 | 0.145 | 427.2 | 401.3 | 0.166 / 8 |
| Paused / 600 s (`31`) | 31 / 30.4 | 3.101 / 4.665 | 0.097 | 426.9 | 401.4 | 0.198 / 5 |
| 600 -> 180 s / Recording (`33`) | 36 / 35.4 | 3.857 / 7.734 | 0.121 | 265.2 | 240.4 | 0.169 / 7 |
| 180 -> 30 s / Recording (`35`) | 42 / 41.5 | 4.611 / 9.141 | 0.144 | 218.4 | 193.0 | 0.178 / 8 |
| Gated audio / 30 s (`37`) | 52 / 51.6 | 3.103 / 6.883 | 0.097 | 218.4 | 193.0 | 0.213 / 10 |


從長錄音後段的 private 中位數 401.3 MiB，縮短為 180 秒後降至 240.4 MiB，再縮為 30 秒後為 193.0 MiB；介面顯示恢復錄音且波形仍有內容。這支持縮短時有釋放程序私有記憶體，**不能單憑 RSS／畫面證明恰好保留最新 N 秒、容量精確回到 1 分鐘，或音訊沒有缺幀及爆音**。後期 30 秒情境與最早短保留情境的 UI／對話框／配置器暖機狀態不同，不能以 RSS 差額判定緩衝容量未回基礎配置。間歇音階段涵蓋音訊與靜音交替，CPU 中位數 3.103% 單核心／0.097% 全機。

Median private memory fell from 401.3 MiB late in extended recording to 240.4 MiB after selecting 180 seconds, then 193.0 MiB at 30 seconds. The UI showed capture recording again with waveform content present. This supports private-memory release, but does not prove exact newest-N-second retention, exact one-minute capacity, or audio continuity. Later 30-second and initial short-history phases have different UI/dialog/allocator warm-up states; their RSS difference cannot establish buffer capacity. The gated phase alternated audio/silence and used median CPU of 3.103% one-core / 0.097% machine.

### 原有實例與背景負載 / Original instance and background load

原有實例沒有改設定，另外監測。無合成音時的視窗是否可見未由 UI 工具確認；播放合成音後，它也會經 System Audio 收到測試音，因此以下是不同時段的背景觀察，不是可直接相減的空載成本。

The original instance retained its settings. Its window visibility was not confirmed for the pre-fixture period. It also received the fixture through System Audio once playback began; these are background observations, not a controlled idle-cost subtraction.

| 情境 / Phase | 有效 n / 秒 | CPU 單核心 median / P95 % | CPU 全機 median % | RSS median MiB | Private median MiB | GPU 最忙引擎 median % / n |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Original / before fixture (`1`) | 292 / 294.6 | 1.559 / 10.704 | 0.049 | 130.1 | 145.1 | 0.104 / 57 |
| Original / fixture playing (`2`) | 1240 / 1253.8 | 4.644 / 10.801 | 0.145 | 308.7 | 339.7 | 0.077 / 248 |


合成音於 `2026-11-13T16:04:55.3928914Z` 開始，原先的無合成音標籤到 16:08:55 才更新。摘要已排除這段錯用舊標籤的資料，沒有算進無合成音基準。原有實例與測試實例的 CPU／記憶體不相加為單一實例的成本。

Playback began at `2026-11-13T16:04:55.3928914Z`, before the original collector's label was updated at 16:08:55. Stale no-fixture labels after playback began are excluded. Usage from the two application instances is kept separate.

### 未完成的驗證 / Validation limits

- 本輪未測 Windows VST3／CLAP 在 DAW 內的硬體用量、macOS、A/B 雙來源、Sender、Sidechain、宿主小節定位或全域快照路由。暫停使用 Pause 按鈕；沒有做框選、縮放或平移的實機測試。
- Options 始終可見；未測只開主波形、關閉／最小化 Options 的成本。測試使用可列於工作列的次要 Options 視窗；它不寫入原有實例的偏好設定。
- 主監測期間，資料夾對話框被測試 Options 視窗遮擋，選取未完成並取消；這段操作標為切換期排除。當時快照保存位置仍是預設 Downloads，音訊寫入遭自動審核拒絕，因此該次沒有執行。使用者其後明確核准另一次新快照測試，結果見下節；原 600 秒內容、原始取樣切割、取消拖放不留檔、缺幀及爆音仍未驗證。
- System Audio 可能包含其他系統聲音。未發布擷取音訊、資料夾對話框或無關桌面畫面；合成素材可由工具重建。GPU 指標不是整張 RTX／Intel 顯卡的歸屬或總負載證明。
- 程序 RSS 包含繪圖、快取及記憶體配置器保留的頁面；private bytes 也不是純錄音緩衝容量。全機 CPU／可用記憶體含其他應用程式，不能當作 Backtracking 的程序用量。

The hardware run did not measure Windows plug-ins inside a DAW, macOS, dual-source A/B, Sender, Sidechain, host bar positioning, or global shortcut routing. Options stayed visible. A folder-picker interaction was canceled after the test Options window obscured the dialog and is excluded as a transition. Automated approval review rejected the snapshot write to the then-default Downloads location, so that attempt wrote no WAV. The user subsequently approved a separate new snapshot test, documented below. The earlier 600-second content, exact sample cuts, canceled drag/drop file behavior, frame continuity, and click-free audio remain unverified. RSS/private bytes include more than the recording buffer; whole-machine counters include other workloads.

同輪程式檢查為 Windows 26 passed／1 ignored、共用核心 51 passed／1 ignored、插件 24 passed；原生快捷鍵與 TCP loopback 的 ignored 測試未在這次重跑。Release 插件 DLL 可載入，VST3 `GetPluginFactory` 非空，CLAP 入口回報 1.2.2 且初始化／解初始化／factory 指標有效。這些檢查只補充程式與入口可用性，**不是 DAW 相容性或插件硬體效能驗收**。

Related checks passed: Windows 26, core 51, plug-in 24 tests; one native-hotkey test and one TCP-loopback test remained ignored. A native DLL smoke check found a non-null VST3 factory and CLAP 1.2.2 entry/function pointers. This verifies loading/entry points, not DAW compatibility or plug-in resource usage.

### 後續快照檢查：2026-11-14 / Subsequent snapshot inspection

硬體監測結束後，使用者明確核准快照寫檔。原測試實例已關閉，因此另開新的次要實例，播放同一份 `continuous-stereo.wav`，從 GUI 選擇 **Save snapshot**。介面顯示 **Saved snapshot**，錄音持續。這次沒有框選，也沒有重新執行前述硬體量測；以下只描述這個新檔案。

After monitoring ended, the user explicitly approved writing a snapshot. The earlier test instance had closed, so a new secondary instance played the same continuous fixture and used **Save snapshot** in the GUI. The UI reported **Saved snapshot** and capture continued. No range was selected, and hardware telemetry was not repeated. These results apply only to the new file.

[`inspect_wav.py`](../tools/inspect_wav.py) 以唯讀方式檢查 RIFF／WAVE 結構和全部樣本；[完整檢查結果](benchmarks/2026-11-14-windows/snapshot-followup.json)未包含本機檔案路徑，WAV 本身不公開。

| 項目 / Item | 結果 / Result |
| --- | --- |
| 檢查時間 / Inspected | 2026-11-13 23:07:38 UTC / 台灣時間 2026-11-14 07:07:38 |
| 格式 / Format | RIFF/WAVE, IEEE float32, 96,000 Hz, 2 channels |
| 幀數與時長 / Frames and duration | 9,313,206 frames / 97.0125625 s |
| 檔案／音訊資料大小 / File and audio data | 74,505,706 / 74,505,648 bytes |
| 結構 / Structure | RIFF extent matches file size; `fact` agrees with `data` frame count; complete frames and matching byte rate |
| 有限樣本值 / Finite values | 18,626,412 / 18,626,412; NaN/Infinity = 0 |
| 滿刻度或以上 / At or above full scale | Stored samples with absolute value ≥ 1.0: 0 |
| Peak / RMS | −21.937760 / −28.687470 dBFS |
| WAV SHA-256 | `98ae3da2989cf1306c2e899add381732aa7cd82fd874bbca4aac190851955fc9` |

時長由這個檔案的幀數除以取樣率算出，沒有和已知的擷取起訖逐幀對照。因此本次確認的是新快照可寫出、格式及內部長度一致、樣本值有限且沒有儲存值達到滿刻度；**不是原 600 秒保留內容或選區匯出的驗證，也不證明沒有遺失／重複幀、先前削波、爆音或其他可聽失真**。這次 96 kHz 檔案與主監測中選擇的 48 kHz 匯出設定屬於不同實例、不同測試。

Duration is derived from this file's frame count and sample rate; it was not compared frame-by-frame with an expected capture interval. This verifies a newly written snapshot's format, internal lengths, finite values, and stored-sample full-scale count. **It does not validate the earlier 600-second history or selected-range export, or establish the absence of missing/duplicated frames, prior clipping, clicks, or audible artifacts.** The 96 kHz follow-up file and the hardware run's 48 kHz export setting belong to different instances/tests.

### 證據與重算 / Evidence and reproduction

公開證據是**去識別的純量追蹤（sanitized scalar trace）**，不是原始本機記錄的逐字副本。保留每秒時間、設定、CPU／記憶體／I/O、GPU 純量和全機負載；省略絕對路徑、使用者名稱、程序名稱／PID、錯誤原文及自由註記、GPU 實例識別字與各引擎陣列。JSONL 仍保留每秒重複的 GPU 快取，摘要才去重。

Public files are sanitized scalar traces, not unchanged raw logs. Repeated GPU cache entries remain in the traces and are deduplicated by the summarizer. Local paths/user tokens, process names/IDs, free-form errors/notes, and GPU identifiers/engine arrays are omitted.

- [可重算摘要 / Summary](benchmarks/2026-11-14-windows/summary.json)
- [測試實例逐秒數據 / Test instance trace](benchmarks/2026-11-14-windows/test-instance.scalar-trace.jsonl)
- [原有實例逐秒數據 / Original instance trace](benchmarks/2026-11-14-windows/ambient-main.scalar-trace.jsonl)
- [設定切換 / Settings events](benchmarks/2026-11-14-windows/settings-events.jsonl)
- [硬體與建置雜湊 / Hardware and binary hashes](benchmarks/2026-11-14-windows/hardware.json)
- [後續短快照檢查 / Subsequent short-snapshot inspection](benchmarks/2026-11-14-windows/snapshot-followup.json)
- [檔案及工具 SHA-256 / Evidence manifest](benchmarks/2026-11-14-windows/evidence-manifest.json)

以原始本機目錄重算，先把每個 collector 的 `scenario.json` 標為切換／穩定階段，並使用 UTC `marked_utc`。建立 `stop.marker` 後，等 collector 回報 `state=stopped` 再匯出：

```powershell
python tools/generate_test_audio.py --output-dir target/perf-audio
powershell -NoProfile -File tools/monitor_backtracking.ps1 -ProcessId <BacktrackingPID> -OutputDirectory target/hardware-run/test-instance -DurationSeconds 1800
python tools/summarize_hardware.py target/hardware-run --publish-directory docs/benchmarks/2026-11-14-windows
```

若只用公開追蹤重算，將兩個 `*.scalar-trace.jsonl` 各放入獨立子目錄並改名為 `telemetry.jsonl`，再傳入兩者的共同父目錄；用 `--baseline-audio-start 2026-11-13T16:04:55.3928914Z` 排除原有實例的過期無測試音標籤。監測用的 `--show-options` 參數只供建立可操作的次要測試設定視窗，測試中的原有實例沒有因此變更設定。

To recompute from public traces, place each trace in its own collector directory as `telemetry.jsonl`, pass their common parent to the summarizer, and provide the baseline audio start explicitly. Starting a new collection requires the app's actual PID and verified UI/settings tags; replaying summary data is not a substitute for a new measurement.

## 歷史測量：Linux（2026-11-12）/ Historical Linux measurements

以下資料來自先前 README，對應 `ba19f18`，保留當時的測試方法與完整數據。其固定 60 秒預配置緩衝區描述僅適用於該次歷史版本；後續動態增長緩衝區須另行量測。

The following dataset was originally published in the README for `ba19f18`. Its fixed 60-second preallocation description applies to that historical build. Later dynamically growing buffers require separate measurements.

測試於 2026-11-12（UTC+8）進行，版本為 Backtracking v0.1.5（`ba19f18`）。環境為 Linux 6.18.57 x86-64，回報的 CPU 為 AMD EPYC 7763，程序可使用 9 個邏輯 CPU。量測涵蓋 Linux CLAP 及共用擷取、波形與 WAV 匯出核心。這份歷史資料未量測 Windows、macOS、VST3 或實體 GPU；Windows 的較新量測另列於本頁上方。

Measured on 2026-11-12 (UTC+8) using Backtracking v0.1.5 (`ba19f18`) on Linux 6.18.57 x86-64. The environment reported an AMD EPYC 7763 and 9 logical CPUs available to the process. Measurements cover Linux CLAP and the shared capture, waveform, and WAV export core. This historical dataset did not measure Windows, macOS, VST3, or a physical GPU. Newer Windows measurements are reported separately above.

### 關閉插件介面 / Plug-in editor closed

小型測試宿主載入 Release 版 CLAP，並固定使用兩個邏輯 CPU。音訊輸入為 48 kHz 立體聲 float32，每 10 ms 送出 480 幀。每種情境以三個獨立程序各量測 8 秒，先暖機，持續錄音的情境也會先填滿保留區間。CPU 是宿主與插件所在程序的用量，以單一邏輯 CPU 為 100%，表列中位數及最小至最大值。RSS 每秒取樣，表列各次量測中位數的中位數。

A minimal host loaded the release CLAP and was pinned to two logical CPUs. Audio input was 48 kHz stereo float32, 480 frames every 10 ms. Three fresh processes ran each scenario for 8 seconds after warm-up, with history prefilled for continuous recording. CPU includes the host and plug-in, with one logical CPU equal to 100%. The table shows the median and observed range. RSS was sampled once per second and summarized as the median of the per-run medians.

| 情境 / Scenario | 保留時間 / History | CPU (%) | RSS (MiB) |
| --- | ---: | ---: | ---: |
| 測試宿主基準 / Host only | — | 0.87 (0.52–1.02) | 1.33 |
| 插件啟用、宿主未送音訊 / Active, no audio callbacks | 20 s | 1.46 (1.33–2.36) | 26.89 |
| 待命、輸入數位靜音 / Armed, digital silence | 20 s | 2.18 (1.72–3.04) | 26.85 |
| 持續錄音 / Continuous recording | 10 s | 1.60 (1.19–2.09) | 26.75 |
| 持續錄音 / Continuous recording | 20 s | 1.73 (1.39–2.75) | 26.71 |
| 持續錄音 / Continuous recording | 30 s | 1.56 (1.22–2.02) | 26.92 |
| 持續錄音 / Continuous recording | 60 s | 2.12 (1.33–2.61) | 26.71 |

CLAP 會依 60 秒上限預先配置錄音環形緩衝區。48 kHz 立體聲 float32 的取樣資料約占 22 MiB，因此各種保留時間的程序 RSS 接近。

CLAP preallocates its recording ring for the 60-second limit. Its 48 kHz stereo float32 samples take about 22 MiB, which accounts for the similar process RSS across history settings.

### 開啟插件介面 / Plug-in editor open

640 × 212 的 Release 版 CLAP 介面以 Xorg 虛擬顯示器執行，使用 Mesa 25.0.7 的 llvmpipe 軟體 OpenGL。測試宿主與 Xorg 固定在同兩個邏輯 CPU。音訊規格同上，先填滿 20 秒的保留區間並使用合併波形。暫停組以介面手動暫停，宿主持續送入音訊。各情境用兩個獨立程序各量測 8 秒。表列宿主與插件所在程序的 CPU 範圍及 RSS，RSS 每秒取樣並採各次量測中位數的中位數。

The 640 × 212 release CLAP editor used Mesa 25.0.7 llvmpipe software OpenGL on an Xorg virtual display. The host and Xorg were pinned to the same two logical CPUs. The input format was unchanged. The host prefilled 20 seconds of history and the editor used the merged waveform view. The paused cases used the editor's Pause control while the host continued sending audio. Two fresh processes ran each scenario for 8 seconds. The table shows the CPU range and RSS for the host process with the plug-in loaded. RSS was sampled once per second and summarized as the median of the per-run medians.

| 介面 / Editor | 狀態 / Capture state | CPU (%) | RSS (MiB) |
| --- | --- | ---: | ---: |
| 完整 / Full | 錄音中 / Recording | 11.58–14.52 | 103.6 |
| 完整 / Full | 暫停 / Paused | 8.74–9.98 | 111.0 |
| 僅波形 / Waveform only | 錄音中 / Recording | 12.88–13.08 | 102.1 |
| 僅波形 / Waveform only | 暫停 / Paused | 6.73–7.14 | 110.7 |

暫停會建立可供框選與匯出的凍結快照，本次暫停情境的 RSS 高出約 7–9 MiB。Xorg 程序另占 0.87–1.50% CPU 與約 37–38 MiB RSS。

Pausing creates a frozen snapshot for selection and export. In this test the paused cases used about 7–9 MiB more RSS. The separate Xorg process used 0.87–1.50% CPU and about 37–38 MiB RSS.

### A／B 擷取與波形核心 / A/B capture and waveform core

Release 版共用核心以單一邏輯 CPU 處理合成 A／B 音訊。輸入已分為兩路 float32 單聲道，每 10 ms 為一個區塊。每組先填滿指定的錄音歷史，並暖機 10 次波形掃描。五個獨立程序各量測 60 秒音訊的批次處理，以及兩路各 384 個區間的 30 次波形掃描。表列程序 CPU 時間的中位數，RSS 在錄音歷史填滿後取樣。範圍包含擷取核心、音量表與波形極值掃描。

The shared release core processed synthetic A/B audio on one logical CPU. The input was already split into two mono float32 lanes in 10 ms blocks. Each run prefilled the selected history and warmed up with 10 waveform scans. Five fresh processes each measured a 60-second batch of audio and 30 waveform scans at 384 bins per lane. The table reports median process CPU time and RSS after prefill. The workload covers the capture core, metering, and waveform peak scans.

| 來源取樣率 / Source rate | 保留時間 / History | 處理每秒音訊 / CPU ms per second of audio | 更新兩路波形 / CPU ms per A/B scan | RSS (MiB) |
| --- | ---: | ---: | ---: | ---: |
| 48 kHz | 20 s | 1.38 | 2.86 | 23.8 |
| 48 kHz | 60 s | 1.20 | 9.35 | 23.8 |
| 96 kHz | 20 s | 3.22 | 9.86 | 46.3 |
| 96 kHz | 60 s | 2.95 | 20.44 | 46.3 |

### WAV 匯出成本 / WAV export cost

Release 版共用匯出核心將 20 秒、48 kHz、立體聲 float32 合成音訊寫到 `/dev/shm`。每種取樣率與編碼各用 5 個獨立程序量測，並固定在同一個邏輯 CPU。表列程序 CPU 時間的中位數，包含取樣率轉換、編碼和寫入。峰值 RSS 取各格式的中位數，再列出同取樣率各編碼的範圍。

The shared release export core wrote 20 seconds of synthetic 48 kHz stereo float32 audio to `/dev/shm`. Five fresh processes ran each rate and encoding combination on the same logical CPU. Median process CPU time covers resampling, encoding, and writing. The peak RSS column spans the median peak RSS for each encoding at that rate.

| 輸出取樣率 / Output rate | 16-bit PCM CPU (ms) | 24-bit PCM CPU (ms) | 32-bit float CPU (ms) | 峰值 RSS / Peak RSS (MiB) |
| --- | ---: | ---: | ---: | ---: |
| 44.1 kHz | 253.5 | 247.1 | 232.4 | 15.29–15.32 |
| 48 kHz | 18.4 | 17.9 | 14.0 | 8.41–8.44 |
| 96 kHz | 501.3 | 482.5 | 450.8 | 23.16–23.18 |

另外以同樣長度的 48 kHz 合成輸入測量 A／B 成對匯出。每次分別輸出一個 Side 立體聲和一個 Mid 單聲道的 32-bit float WAV。每組仍採 5 個獨立程序的中位數。

A separate test used the same input length and source rate to export an A/B pair. Each run wrote a stereo Side file and a mono Mid file as 32-bit float WAVs. Each result is the median of five fresh processes.

| 輸出取樣率 / Output rate | 成對匯出 CPU / Pair CPU (ms) | 峰值 RSS / Peak RSS (MiB) |
| --- | ---: | ---: |
| 44.1 kHz | 516.9 | 11.92 |
| 48 kHz | 35.3 | 8.39 |
| 96 kHz | 945.1 | 15.91 |
