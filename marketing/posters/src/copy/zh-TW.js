/* 繁體中文（台灣）文案。來源：marketing/brand/README.md 文案表與名詞表；
   模型內的小標籤取自名詞表與 docs/USER_GUIDE.md。修改文字只改這裡，不改版面檔。
   標題行數與斷行由此檔決定（每行 3–5 字，方塊字不壓扁）。 */
(window.BT_COPY = window.BT_COPY || {})["zh-TW"] = {
  htmlLang: "zh-Hant-TW",
  tag: "回溯錄音取樣器",
  mega: ["剛剛那段", "拿回來"],
  sub: "框選剛剛聽到的聲音，直接拖出成 WAV。Backtracking 會在記憶體中保留最近最長 <b>10&nbsp;分鐘</b>的音訊。",
  oneLiner: "它一直都在聽",
  scale: ["10 秒", "20 秒", "30 秒", "60 秒", "3 分鐘", "5 分鐘", "10 分鐘"],
  editions: ["Windows", "macOS", "VST3", "CLAP"],
  licence: "開放原始碼 · Apache-2.0",
  cta: {
    download: "從 GitHub 下載",
    guide: "閱讀使用手冊",
    support: "在 Buy Me a Coffee 贊助"
  },
  url: "backtracking.kamelkyp.com",
  /* 換行時不可拆開的詞組 */
  nobreak: ["框選後直接拖出 WAV。", "從 GitHub 下載。", "10 秒", "10 分鐘", "秒數線", "小節線", "記憶體中", "指定 App", "Super + `", "Backtracking 插件", "macOS 14.2 以上", "WAV。", "繼續錄音。", "資料夾"],
  repo: "github.com/kekekewww/Backtracking",

  beats: [
    { h: "它一直都在聽", b: "Backtracking 在記憶體中保留最近 10 秒到 10 分鐘的音訊。" },
    { h: "框選想要的那一段", b: "在波形上拖曳框選，按住 Shift 可吸附旗標、小節線或秒數線。" },
    { h: "直接拖出成 WAV", b: "放到 DAW 音軌或資料夾，背景會繼續錄音。" }
  ],

  editionPosters: {
    windows: {
      label: "Windows 桌面版",
      mega: ["任何 App", "都能留"],
      sub: "系統播放、麥克風或指定 App，框選後直接拖出 WAV。從 GitHub 下載。"
    },
    macos: {
      label: "macOS 桌面版",
      mega: ["Mac 的聲音", "拿回來"],
      sub: "系統播放、麥克風或指定 App，支援 macOS 14.2 以上。從 GitHub 下載。"
    },
    plugin: {
      label: "VST3／CLAP 插件",
      mega: ["每一軌", "都留著"],
      sub: "插在音軌、群組或主輸出上，留住經過的聲音，再拖出成 WAV。"
    },
    linux: {
      label: "Linux 快照快捷鍵",
      mega: ["一個按鍵", "存下來"],
      sub: "全域 Super + ` 快捷鍵，讓 Backtracking 插件儲存快照。從 GitHub 下載。"
    }
  },

  ui: {
    source: "來源",
    sources: ["系統播放", "麥克風", "指定 App"],
    abNote: "同時擷取 A／B 兩路，匯出兩個起訖對齊的 WAV",
    shortcutNote: "背景儲存快照",
    winReq: "Windows 10／11 x64",
    waveOnly: "只顯示波形，置頂",
    plugin: "插件",
    formats: "VST3 · CLAP",
    pluginNote: "擷取效果鏈該位置的音訊",
    tracks: ["鼓組", "貝斯", "鍵盤", "人聲"],
    group: "群組",
    master: "主輸出",
    inserts: ["EQ", "Comp", "Backtracking", "Limiter"],
    instance: "插件實例",
    request: "送出一次快照請求",
    helper: "Linux 快照快捷鍵常駐程式",
    command: "backtracking-shortcut --target INSTANCE_ID",
    snapshot: "快照已儲存"
  }
};
