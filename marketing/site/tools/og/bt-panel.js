/* Backtracking UI mock-up builder (no dependencies, no network).
 *
 *   <div data-btp='{"width":560,"waveHeight":70,"seed":7,"selection":[0.42,0.66]}'></div>
 *   <script src="shared/bt-panel.js"></script>   (renders every [data-btp] on load)
 *
 * Or BTPanel.render(element, options). Deterministic: the same seed draws the same waveform,
 * so a motion build can redraw identical frames. The waveform is a continuous filled min/max
 * envelope with an outline, which is how current builds draw wide views (README: "continuous
 * filled min/max envelope and outline"). Inside a selection the app inverts the waveform colour.
 *
 * Options (all optional):
 *   width        panel width in logical px (default 560)
 *   waveHeight   waveform area height (default 70)
 *   seed         integer, waveform shape (default 1)
 *   style        "beat" (drum-led) | "dense" (full mix) | "mixed" (default "mixed")
 *   empty        fraction of the buffer on the left that is still empty (flat), default 0
 *   selection    [start, end] as 0..1 of width, or null
 *   band         show a faint band behind the selection (default true)
 *   duration     readout at top-right, e.g. "6.17 s" (shown only with a selection)
 *   tooltip      {text:"Drag and Drop", at:0.55, y:28} or null
 *   state        "rec" (pause button + red dot) | "paused" (play button + grey dot)
 *   meter        e.g. "−9.8"  meterUnit  "dBFS" | "VU" | "LUFS"
 *   pin          keep-on-top highlighted (default true)
 *   guides       vertical guide spacing in px (0 = off, default 0)
 *   rows         1 or 2 (A/B or L/R) (default 1)
 *   toolbar      false = "Show waveform only" strip (no toolbar)
 */
(function () {
  "use strict";

  function rng(seed) {
    var a = seed >>> 0;
    return function () {
      a = (a + 0x6d2b79f5) | 0;
      var t = Math.imul(a ^ (a >>> 15), 1 | a);
      t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
      return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
    };
  }

  function envelope(n, seed, style, empty) {
    var r = rng(seed * 9973 + 17);
    var ev = [];
    var x, i;
    var beat = 1 / (12 + Math.floor(r() * 6));
    if (style === "beat" || style === "mixed") {
      for (x = 0; x < 1.02; x += beat) {
        ev.push({ t: x + 0.002 * r(), a: 0.7 + 0.28 * r(), tau: 0.010 + 0.008 * r() });
        if (r() < 0.85) ev.push({ t: x + beat / 2 + 0.003 * r(), a: 0.38 + 0.3 * r(), tau: 0.005 + 0.004 * r() });
        if (r() < 0.5) ev.push({ t: x + beat * 0.75, a: 0.22 + 0.2 * r(), tau: 0.003 + 0.003 * r() });
      }
    }
    if (style === "dense") {
      for (x = 0; x < 1.02; x += 0.004 + 0.006 * r()) ev.push({ t: x, a: 0.25 + 0.3 * r(), tau: 0.006 + 0.01 * r() });
    }
    // slow phrase shape
    var p1 = r() * 6.28, p2 = r() * 6.28;
    var body = style === "dense" ? 0.42 : style === "mixed" ? 0.16 : 0.05;
    var top = new Float32Array(n), bot = new Float32Array(n);
    for (i = 0; i < n; i++) {
      x = i / (n - 1);
      var s = 0;
      for (var k = 0; k < ev.length; k++) {
        var e = ev[k];
        if (x >= e.t) {
          var d = (x - e.t) / e.tau;
          if (d < 9) s += e.a * Math.exp(-d);
        }
      }
      var phrase = 0.72 + 0.18 * Math.sin(x * 7.1 + p1) + 0.1 * Math.sin(x * 17.3 + p2);
      s = (s + body * (0.75 + 0.5 * r())) * phrase;
      var a = Math.tanh(s * 1.15) * 0.96;
      if (x < empty) a = 0;
      else if (x < empty + 0.015) a *= (x - empty) / 0.015;
      top[i] = a * (0.86 + 0.14 * r());
      bot[i] = a * (0.8 + 0.2 * r());
    }
    return { top: top, bot: bot };
  }

  function envPath(env, w, h, y0) {
    var n = env.top.length, mid = y0 + h / 2, sc = (h / 2) * 0.94, floor = 0.5;
    var d = "M0," + (mid - floor).toFixed(2);
    var i, x;
    for (i = 0; i < n; i++) {
      x = (i / (n - 1)) * w;
      d += "L" + x.toFixed(2) + "," + (mid - Math.max(floor, env.top[i] * sc)).toFixed(2);
    }
    for (i = n - 1; i >= 0; i--) {
      x = (i / (n - 1)) * w;
      d += "L" + x.toFixed(2) + "," + (mid + Math.max(floor, env.bot[i] * sc)).toFixed(2);
    }
    return d + "Z";
  }

  var ICON = {
    pause: '<svg viewBox="0 0 12 12"><rect x="2.5" y="2" width="2.4" height="8" rx=".6" fill="currentColor"/><rect x="7.1" y="2" width="2.4" height="8" rx=".6" fill="currentColor"/></svg>',
    play: '<svg viewBox="0 0 12 12"><path d="M3.5 2.2v7.6a.5.5 0 0 0 .76.43l6.1-3.8a.5.5 0 0 0 0-.86l-6.1-3.8a.5.5 0 0 0-.76.43Z" fill="currentColor"/></svg>',
    trash: '<svg viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.15" stroke-linecap="round" stroke-linejoin="round"><path d="M2 3.2h8M4.6 3.2V2.1h2.8v1.1M3 3.2l.5 6.9h5l.5-6.9M5 5v3.4M7 5v3.4"/></svg>',
    pin: '<svg viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round"><path d="M3.6 1.8h4.8M4.4 1.8v3.4L2.8 7.2h6.4L7.6 5.2V1.8M6 7.2v3.4"/></svg>',
    gear: '<svg viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.1"><circle cx="6" cy="6" r="1.7"/><path d="M6 1v1.4M6 9.6V11M1 6h1.4M9.6 6H11M2.46 2.46l1 1M8.54 8.54l1 1M2.46 9.54l1-1M8.54 3.46l1-1"/><circle cx="6" cy="6" r="3.4"/></svg>'
  };

  var uid = 0;

  function render(el, o) {
    o = o || {};
    var W = o.width || 560;
    var H = o.waveHeight || 70;
    var rows = o.rows || 1;
    var pad = 10;
    var ww = W - pad * 2 - 2;
    var id = "btp" + ++uid;
    var sel = o.selection || null;
    var cols = Math.round(ww / 1.25);

    var svg = '<svg width="' + ww + '" height="' + H + '" viewBox="0 0 ' + ww + " " + H + '">';
    svg += '<defs><clipPath id="' + id + 's">' +
      (sel ? '<rect x="' + (sel[0] * ww).toFixed(2) + '" y="0" width="' + ((sel[1] - sel[0]) * ww).toFixed(2) + '" height="' + H + '"/>' : "") +
      "</clipPath></defs>";
    if (o.guides) {
      for (var gx = ww - o.guides; gx > 0; gx -= o.guides) {
        svg += '<line x1="' + gx + '" y1="0" x2="' + gx + '" y2="' + H + '" stroke="var(--btp-guide)" stroke-width="1"/>';
      }
    }
    if (sel && o.band !== false) {
      svg += '<rect x="' + (sel[0] * ww).toFixed(2) + '" y="0" width="' + ((sel[1] - sel[0]) * ww).toFixed(2) + '" height="' + H + '" fill="var(--btp-sel-band)"/>';
    }
    var rh = H / rows;
    for (var rI = 0; rI < rows; rI++) {
      var y0 = rI * rh;
      var env = envelope(cols, (o.seed || 1) + rI * 31, o.style || "mixed", o.empty || 0);
      var d = envPath(env, ww, rh, y0);
      svg += '<line x1="0" y1="' + (y0 + rh / 2) + '" x2="' + ww + '" y2="' + (y0 + rh / 2) + '" stroke="var(--btp-axis)" stroke-width="1"/>';
      svg += '<path d="' + d + '" fill="var(--btp-wave)" stroke="var(--btp-wave-line)" stroke-width=".6" stroke-linejoin="round"/>';
      if (sel) svg += '<path clip-path="url(#' + id + 's)" d="' + d + '" fill="var(--btp-sel)" stroke="var(--btp-sel-line)" stroke-width=".6" stroke-linejoin="round"/>';
    }
    svg += "</svg>";

    var bar = "";
    if (o.toolbar !== false) {
      var rec = (o.state || "rec") === "rec";
      bar = '<div class="btp-bar">' +
        '<span class="btp-title">Backtracking</span>' +
        '<span class="btp-btn">' + (rec ? ICON.pause : ICON.play) + "</span>" +
        '<span class="btp-dot' + (rec ? " rec" : "") + '"></span>' +
        '<span class="btp-btn">' + ICON.trash + "</span>" +
        '<span class="btp-btn' + (o.pin === false ? "" : " on") + '">' + ICON.pin + "</span>" +
        '<span class="btp-spacer"></span>' +
        '<span class="btp-meter"><b>' + (o.meter || "−9.8") + "</b> " + (o.meterUnit || "dBFS") + "</span>" +
        '<span class="btp-btn">' + ICON.gear + "</span>" +
        "</div>";
    }
    var extra = "";
    if (sel && o.duration) extra += '<span class="btp-dur">' + o.duration + "</span>";
    if (o.tooltip) {
      extra += '<span class="btp-tip" style="left:' + (o.tooltip.at * ww).toFixed(1) + "px;top:" + (o.tooltip.y || H * 0.42) + 'px">' + (o.tooltip.text || "Drag and Drop") + "</span>";
    }
    el.classList.add("btp");
    if (o.toolbar === false) el.style.paddingTop = pad + "px";
    el.style.width = W + "px";
    el.innerHTML = bar + '<div class="btp-wave" style="height:' + H + 'px">' + svg + extra + "</div>";
    return el;
  }

  function renderAll() {
    var list = document.querySelectorAll("[data-btp]");
    for (var i = 0; i < list.length; i++) {
      var raw = list[i].getAttribute("data-btp");
      render(list[i], raw ? JSON.parse(raw) : {});
    }
  }

  /* Standalone waveform (no panel chrome) for backgrounds and decorative strips. */
  function wave(el, o) {
    o = o || {};
    var w = o.width || 1000, h = o.height || 100;
    var env = envelope(Math.round(w / (o.step || 2)), o.seed || 1, o.style || "mixed", o.empty || 0);
    el.innerHTML = '<svg width="' + w + '" height="' + h + '" viewBox="0 0 ' + w + " " + h + '"><path d="' +
      envPath(env, w, h, 0) + '" fill="' + (o.fill || "currentColor") + '" stroke="' + (o.stroke || "none") +
      '" stroke-width="' + (o.strokeWidth || 0.6) + '"/></svg>';
  }
  function waveAll() {
    var list = document.querySelectorAll("[data-wave]");
    for (var i = 0; i < list.length; i++) wave(list[i], JSON.parse(list[i].getAttribute("data-wave") || "{}"));
  }

  window.BTPanel = { render: render, renderAll: renderAll, wave: wave, waveAll: waveAll, icons: ICON };
  renderAll();
  waveAll();

  /* Render check: lists the web fonts that actually loaded on <html data-fonts-loaded="...">
     (read it with chrome --headless --dump-dom). */
  if (document.fonts && document.fonts.ready) {
    document.fonts.ready.then(function () {
      var out = [];
      document.fonts.forEach(function (f) {
        if (f.status === "loaded") out.push(f.family.replace(/"/g, "") + " " + f.weight);
      });
      document.documentElement.setAttribute("data-fonts-loaded", out.join(" | "));
    });
  }
})();
