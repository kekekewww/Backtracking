/* Every poster design: name, pixel size, set, where it is used.
   Read by src/poster.html (frame size), render.mjs (what to render) and index.html (gallery). */
(function (root) {
  var designs = [
    { name: "edition-master",  w: 1080, h: 1350, set: "edition", use: "Master poster: print, shares, press kit" },
    { name: "edition-windows", w: 1080, h: 1350, set: "edition", use: "Windows desktop app announcement" },
    { name: "edition-macos",   w: 1080, h: 1350, set: "edition", use: "macOS desktop app announcement" },
    { name: "edition-plugin",  w: 1080, h: 1350, set: "edition", use: "VST3 / CLAP plug-in announcement" },
    { name: "edition-linux",   w: 1080, h: 1350, set: "edition", use: "Linux shortcut helper for the plug-in" },
    { name: "github-social",      w: 1280, h: 640,  set: "social", use: "GitHub repository social preview (Settings > Social preview)" },
    { name: "instagram-feed",     w: 1080, h: 1440, set: "social", use: "Instagram feed post, 3:4 portrait" },
    { name: "instagram-story",    w: 1080, h: 1920, set: "social", use: "Instagram / Facebook story (text kept out of top 250 px, bottom 340 px)" },
    { name: "facebook-cover",     w: 1640, h: 720,  set: "social", use: "Facebook page cover (key content in central 1640x624, clear of left 300 px)" },
    { name: "facebook-feed",      w: 1080, h: 1350, set: "social", use: "Facebook feed post, 4:5 portrait" },
    { name: "threads-post",       w: 1080, h: 1080, set: "social", use: "Threads post, square" },
    { name: "buymeacoffee-cover", w: 1600, h: 400,  set: "social", use: "Buy Me a Coffee page cover" },
    { name: "youtube-channel",    w: 2560, h: 1440, set: "social", use: "YouTube channel banner (text and icon in central 1546x423)" },
    { name: "youtube-thumbnail",  w: 1920, h: 1080, set: "social", use: "YouTube video thumbnail (under 2 MB)" }
  ];
  var langs = ["en", "zh-TW"];
  var api = { designs: designs, langs: langs };
  if (typeof module !== "undefined" && module.exports) module.exports = api;
  else root.BT_MANIFEST = api;
})(this);
