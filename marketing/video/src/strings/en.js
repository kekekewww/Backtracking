/* English captions. Every line is copied from marketing/brand/README.md (copy deck). */
window.STR = {
  htmlLang: "en",
  oneLiner: ["It was", "already", "listening."],          // One-liner: "It was already listening."
  tag: "Retrospective audio sampler",
  mega: ["Take it", "back."],                           // Mega headline: TAKE IT / BACK.
  scale: ["10 s", "20 s", "30 s", "60 s", "3 min", "5 min", "10 min"],
  beats: [
    { n: "1", title: "It’s already listening.", body: "Backtracking keeps the last 10 seconds to 10 minutes of audio in memory." },
    { n: "2", title: "Select the part you want.", body: "Drag across the waveform. Hold Shift to snap to flags, bars or seconds." },
    { n: "3", title: "Drag it out as a WAV.", body: "Drop it on a DAW track or into a folder. Recording keeps running in the background." }
  ],
  /* Feature bodies use a small markup (SC.rich): {…} never breaks, [X] is drawn as a keycap,
     \n is a deliberate line break. The words are the copy deck's. */
  features: [
    { name: "Sources", body: "Capture system playback, a microphone, or one chosen app." },
    { name: "A/B", body: "Record two sources at once and save two aligned WAVs." },
    { name: "Plug-in", body: "Insert it on any DAW track to capture audio at that point in the chain." },
    { name: "Snapshot shortcut", body: "Save from the background: {[Alt] + [`] on Windows,} {[Fn/Globe] + [`] on macOS.}" },
    { name: "Waveform only", body: "Shrink the window to a thin waveform strip, with adjustable transparency, kept on top." },
    { name: "Meter", body: "Live level in dBFS, VU or {momentary LUFS.}" },
    { name: "WAV export", body: "{44.1, 48 or 96 kHz;} {16-bit,} {24-bit PCM} or {32-bit float.}" }
  ],
  editions: ["Windows", "macOS", "VST3", "CLAP"],
  licence: "Open source · Apache-2.0",
  cta: "Download from GitHub",
  url: "backtracking.kamelkyp.com"
};
