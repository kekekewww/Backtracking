/* English copy for every poster. Source: marketing/brand/README.md (copy deck) and, for the small
   mock-up labels, the glossary and docs/USER_GUIDE.md. Edit text here, never in the layouts. */
(window.BT_COPY = window.BT_COPY || {}).en = {
  htmlLang: "en",
  tag: "Retrospective audio sampler",
  mega: ["Take it", "back."],
  sub: "Select what you just heard and drag it out as a WAV. Backtracking keeps up to <b>10&nbsp;minutes</b> of audio in memory.",
  oneLiner: "It was already listening.",
  scale: ["10 s", "20 s", "30 s", "60 s", "3 min", "5 min", "10 min"],
  editions: ["Windows", "macOS", "VST3", "CLAP"],
  licence: "Open source · Apache-2.0",
  cta: {
    download: "Download from GitHub",
    guide: "Read the user guide",
    support: "Support on Buy Me a Coffee"
  },
  url: "backtracking.kamelkyp.com",
  /* phrases kept on one line when text wraps */
  nobreak: ["plug-in", "Super + `", "Download from GitHub", "10 minutes", "10 seconds"],
  repo: "github.com/kekekewww/Backtracking",

  beats: [
    { h: "It’s already listening.", b: "Backtracking keeps the last 10 seconds to 10 minutes of audio in memory." },
    { h: "Select the part you want.", b: "Drag across the waveform. Hold Shift to snap to flags, bars or seconds." },
    { h: "Drag it out as a WAV.", b: "Drop it on a DAW track or into a folder. Recording keeps running in the background." }
  ],

  editionPosters: {
    windows: {
      label: "Windows desktop",
      mega: ["From any", "app."],
      sub: "System playback, a microphone, or one chosen app. Select it and drag out a WAV. Download from GitHub."
    },
    macos: {
      label: "macOS desktop",
      mega: ["Off your", "Mac."],
      sub: "System playback, a microphone, or one chosen app, on macOS 14.2 or later. Download from GitHub."
    },
    plugin: {
      label: "VST3 / CLAP plug-in",
      mega: ["On any", "track."],
      sub: "Insert it on a track, group or master. It keeps what passed through and hands it back as a WAV."
    },
    linux: {
      label: "Linux shortcut helper",
      mega: ["One key.", "Saved."],
      sub: "A global Super + ` shortcut that tells a Backtracking plug-in instance to save a snapshot. Download from GitHub."
    }
  },

  /* Short labels inside the mock-ups (glossary terms and feature names, shortened, never embellished). */
  ui: {
    source: "Source",
    sources: ["System playback", "Microphone", "Chosen app"],
    abNote: "Record two sources at once and save two aligned WAVs.",
    shortcutNote: "Save from the background",
    winReq: "Windows 10/11 x64",
    waveOnly: "Waveform only, kept on top",
    plugin: "Plug-in",
    formats: "VST3 · CLAP",
    pluginNote: "Captures audio at that point in the chain.",
    tracks: ["Drums", "Bass", "Keys", "Vocals"],
    group: "Group",
    master: "Master",
    inserts: ["EQ", "Comp", "Backtracking", "Limiter"],
    instance: "Plug-in instance",
    request: "One snapshot request",
    helper: "Linux shortcut helper",
    command: "backtracking-shortcut --target INSTANCE_ID",
    snapshot: "Snapshot saved"
  }
};
