/* 繁體中文字幕。內容取自 marketing/brand/README.md 的文案表；功能名稱取自 docs/USER_GUIDE.md 的繁中標題與詞彙表。 */
window.STR = {
  htmlLang: "zh-Hant-TW",
  /* 標題式短句不加句尾「。」（文案指南〈Punctuation in headlines〉）；內文句子保留標點。 */
  oneLiner: ["它一直", "都在聽"],                         // 一句話：它一直都在聽
  tag: "回溯錄音取樣器",
  mega: ["剛剛那段", "拿回來"],                           // 主標：剛剛那段 / 拿回來
  scale: ["10 秒", "20 秒", "30 秒", "60 秒", "3 分鐘", "5 分鐘", "10 分鐘"],
  beats: [
    { n: "1", title: "它一直都在聽", body: "Backtracking 在記憶體中保留最近 10 秒到 10 分鐘的音訊。" },
    { n: "2", title: "框選想要的那一段", body: "在波形上拖曳框選，按住 Shift 可吸附旗標、小節線或秒數線。" },
    { n: "3", title: "直接拖出成 WAV", body: "放到 DAW 音軌或資料夾，背景會繼續錄音。" }
  ],
  features: [
    { name: "擷取來源", body: "擷取系統播放、麥克風或指定 App。" },
    { name: "A／B", body: "同時擷取 A／B 兩路，匯出兩個起訖對齊的 WAV。" },
    { name: "插件", body: "插入任一 DAW 音軌，擷取效果鏈該位置的音訊。" },
    { name: "快照快捷鍵", body: "背景儲存快照：Windows 按 Alt + `，macOS 按 Fn／Globe + `。" },
    { name: "只顯示波形", body: "把視窗縮成細長波形條，可調透明度並置頂。" },
    { name: "即時音量表", body: "即時音量：dBFS、VU 或瞬時 LUFS。" },
    { name: "WAV 匯出", body: "44.1、48 或 96 kHz；16-bit、24-bit PCM 或 32-bit float。" }
  ],
  editions: ["Windows", "macOS", "VST3", "CLAP"],
  licence: "開放原始碼 · Apache-2.0",
  cta: "從 GitHub 下載",
  url: "backtracking.kamelkyp.com"
};
