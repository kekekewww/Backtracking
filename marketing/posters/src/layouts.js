/* Backtracking poster layouts (direction C "Rewind").
   poster.html?d=<design>&lang=<en|zh-TW> builds one design from the copy in copy/<lang>.js.
   Every text string comes from BT_COPY[lang]; this file only holds composition.
   Mega headlines are fitted after the fonts load: English lines share one size (the C look),
   Chinese lines each get their own size, set square in Noto Sans TC 900. */
(function () {
  "use strict";
  var q = new URLSearchParams(location.search);
  var NAME = q.get("d") || "edition-master";
  var LANG = q.get("lang") || "en";
  var C = window.BT_COPY[LANG];
  var ZH = LANG !== "en";
  var D = window.BT_MANIFEST.designs.filter(function (d) { return d.name === NAME; })[0];
  if (!C || !D) { document.body.textContent = "Unknown design or language"; return; }

  var ICON = "../../../assets/backtracking-icon.svg";
  var FILE = "Backtracking-1794…-13.wav";
  var html = document.documentElement;
  html.lang = C.htmlLang;
  html.classList.add(ZH ? "zh" : "en");
  document.title = "Backtracking " + NAME + " " + LANG;
  document.body.style.width = D.w + "px";
  document.body.style.height = D.h + "px";

  /* ---------- helpers ---------- */
  var KEYS = { l: "left", t: "top", r: "right", b: "bottom", w: "width", h: "height", fs: "font-size", z: "zoom" };
  function S(o) {
    if (!o) return "";
    var s = "";
    for (var k in o) {
      if (o[k] == null) continue;
      var prop = KEYS[k] || k;
      var v = o[k];
      s += prop + ":" + (typeof v === "number" && !/^(zoom|opacity|z-index|line-height)$/.test(prop) ? v + "px" : v) + ";";
    }
    return ' style="' + s + '"';
  }
  function L(en, zh) { return ZH ? zh : en; }           // per-language composition value
  /* phrases from the copy file that must not be split across lines (C.nobreak) */
  function nb(h) {
    (C.nobreak || []).forEach(function (p) { h = h.split(p).join('<span class="nb">' + p + "</span>"); });
    return h;
  }
  function esc(t) { return nb(String(t).replace(/&(?![a-z#0-9]+;)/g, "&amp;").replace(/</g, "&lt;")); }
  /* wrap Latin runs inside Chinese mega lines so they keep the Latin face */
  function lat(t) {
    var e = String(t).replace(/&/g, "&amp;").replace(/</g, "&lt;");
    if (!ZH) return e;
    return e.replace(/([A-Za-z0-9`][A-Za-z0-9 .\-\/+`]*[A-Za-z0-9.`]|[A-Za-z0-9`])/g, '<span class="lat">$1</span>');
  }

  function lockup(o) {
    return '<div class="lockup"' + S({ l: o.l, t: o.t, r: o.r, gap: o.gap }) + '><img src="' + ICON + '" width="' + o.s + '" height="' + o.s +
      '" alt=""><span class="wm"' + S({ fs: o.fs }) + ">Backtracking</span></div>";
  }
  function tag(o) { return '<div class="tag wide"' + S(o) + ">" + esc(C.tag) + "</div>"; }
  /* fit: {w,h,max,mode:"one"|"each"}; mint: indexes of mint lines (default last); inline: one row */
  function mega(lines, o) {
    var mint = o.mint || [lines.length - 1];
    /* Chinese blocks read heavier than the condensed caps: default to a smaller cap and a little more air */
    var max = ZH ? (o.zmax || Math.round(o.max * 0.82)) : o.max;
    var fit = { w: o.w, h: o.h, max: max, mode: o.mode || (ZH ? "each" : "one"), inline: !!o.inline, align: o.align || "left" };
    var inner = lines.map(function (ln, i) {
      return '<span class="ln' + (mint.indexOf(i) >= 0 ? " m" : "") + '">' + lat(ln) + "</span>";
    }).join(o.inline ? '<span class="sp"> </span>' : "");
    return '<h1 class="mega' + (o.inline ? " inline" : "") + '" data-fit=\'' + JSON.stringify(fit) + "'" +
      S({ l: o.l, t: o.t + (ZH && !o.inline ? (o.zdy == null ? 10 : o.zdy) : 0), w: o.w, "text-align": o.align }) + ">" + inner + "</h1>";
  }
  function sub(text, o) { return '<p class="sub"' + S(o) + ">" + nb(text) + "</p>"; }
  function scale(o, bare) {
    return '<div class="scale wide' + (bare ? " bare" : "") + '"' + S(o) + ">" + C.scale.map(function (s, i) {
      return "<span" + (i === C.scale.length - 1 ? ' class="on"' : "") + ">" + esc(s) + "</span>";
    }).join("") + "</div>";
  }
  function editions(o) {
    return '<div class="editions wide"' + S(o) + ">" + C.editions.map(function (e) { return "<span>" + esc(e) + "</span>"; }).join("") + "</div>";
  }
  /* footer: rule, editions + licence, then printed URL + the one CTA */
  function footer(o) {
    var right = o.W - o.l - o.w;
    var out = '<div class="rule"' + S({ l: o.l, t: o.t, w: o.w }) + "></div>";
    out += editions({ l: o.l, t: o.t + 20, fs: o.fs });
    out += '<div class="license wide"' + S({ r: right, t: o.t + 20, fs: o.fs }) + ">" + esc(C.licence) + "</div>";
    if (o.row2 !== false) {
      out += '<div class="url"' + S({ l: o.l, t: o.t + 20 + o.fs * 2.3, fs: o.fs }) + ">" + esc(C.url) + "</div>";
      out += cta({ r: right, t: o.t + 20 + o.fs * 2.3, fs: o.fs }, "text");
    }
    return out;
  }
  var DL = '<svg viewBox="0 0 20 20" width="1em" height="1em"><path d="M10 2.5v10M5.2 8.2 10 13l4.8-4.8M3 16.8h14" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="square"/></svg>';
  function cta(o, kind) {
    if (kind === "text") return '<div class="wide cta-t"' + S(Object.assign({ color: "var(--c-paper)", display: "flex", "align-items": "center", gap: "0.5em" }, o)) + ">" + DL + esc(C.cta.download) + "</div>";
    return '<div class="btn"' + S(o) + ">" + DL + esc(C.cta.download) + "</div>";
  }
  function panel(opts, o) {
    return '<div class="panel"' + S({ l: o.l, t: o.t }) + '><div class="btp c' + (o.s10 ? " s10" : "") + '"' + S({ z: o.z }) +
      " data-btp='" + JSON.stringify(opts) + "'></div></div>";
  }
  function slab(o) {
    var name = o.name || FILE, meta = o.meta || "7.40 s · 48 kHz · 32-bit float";
    return '<div class="slabw"' + S({ l: o.l, t: o.t }) + '><div style="zoom:' + (o.z || 1) + ';position:relative">' +
      '<div class="slab"><div class="ic">WAV</div><div><div class="n">' + esc(name) + '</div><div class="m">' + esc(meta) + "</div></div></div>" +
      (o.cursor === false ? "" : '<svg class="cursor" style="position:absolute;left:-14px;top:-14px" width="28" height="40" viewBox="0 0 14 20"><path d="M1 1v15.5l3.8-3.6 2.6 6 2.4-1-2.5-5.9h5.3Z" fill="#fff" stroke="#070c3d" stroke-width="1.1" stroke-linejoin="round"/></svg>') +
      "</div></div>";
  }
  /* three mint motion streaks ending at x2 */
  function streaks(x1, x2, y, o) {
    o = o || {};
    var g = o.gap || 22, sw = o.sw || 4, len = x2 - x1;
    return '<svg class="streaks"' + S({ l: 0, t: 0 }) + ' width="' + D.w + '" height="' + D.h + '" viewBox="0 0 ' + D.w + " " + D.h + '">' +
      '<line x1="' + x1 + '" y1="' + y + '" x2="' + x2 + '" y2="' + y + '" stroke-width="' + sw + '"/>' +
      '<line x1="' + (x1 + len * 0.32) + '" y1="' + (y + g) + '" x2="' + x2 + '" y2="' + (y + g) + '" stroke-width="' + sw + '" opacity=".7"/>' +
      '<line x1="' + (x1 + len * 0.61) + '" y1="' + (y + 2 * g) + '" x2="' + x2 + '" y2="' + (y + 2 * g) + '" stroke-width="' + sw + '" opacity=".45"/></svg>';
  }
  function bgwave(o, opts) {
    return '<div class="bgwave"' + S(o) + " data-wave='" + JSON.stringify(Object.assign({ seed: 23, style: "mixed", step: 3 }, opts)) + "'></div>";
  }
  var GLOBE = '<svg viewBox="0 0 20 20"><g fill="none" stroke="currentColor" stroke-width="1.7"><circle cx="10" cy="10" r="8"/><ellipse cx="10" cy="10" rx="3.6" ry="8"/><path d="M2.3 7.3h15.4M2.3 12.7h15.4"/></g></svg>';
  function keys(list, o) {
    return '<div class="keys"' + S(o) + ">" + list.map(function (k) {
      if (k === "`") return '<span class="key tick">`</span>';
      if (k === "fn") return '<span class="key">fn' + GLOBE + "</span>";
      return '<span class="key">' + esc(k) + "</span>";
    }).join('<span class="plus">+</span>') + "</div>";
  }
  var SRC_ICON = [
    '<svg width="22" height="22" viewBox="0 0 22 22"><path d="M3 8.5h4l5-4v13l-5-4H3Z" fill="currentColor"/><path d="M15 7.5a5 5 0 0 1 0 7M17.6 5a8.6 8.6 0 0 1 0 12" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"/></svg>',
    '<svg width="22" height="22" viewBox="0 0 22 22"><rect x="7.5" y="2.5" width="7" height="11" rx="3.5" fill="currentColor"/><path d="M4.5 10.5a6.5 6.5 0 0 0 13 0M11 17v3" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"/></svg>',
    '<svg width="22" height="22" viewBox="0 0 22 22"><rect x="2.5" y="4" width="17" height="14" rx="1.5" fill="none" stroke="currentColor" stroke-width="2"/><path d="M2.5 8h17" stroke="currentColor" stroke-width="2"/><path d="M9 11v5l4.5-2.5Z" fill="currentColor"/></svg>'
  ];
  function srcCard(o, active) {
    return '<div class="srcs"' + S({ l: o.l, t: o.t, w: o.w, "z-index": o.z }) + '><div class="hd">' + esc(C.ui.source) + " A</div>" +
      C.ui.sources.map(function (s, i) {
        return '<div class="it' + (i === active ? " on" : "") + '"' + S({ fs: o.fs }) + ">" + SRC_ICON[i] + "<span>" + esc(s) + "</span></div>";
      }).join("") + "</div>";
  }
  function srcChips(o, active) {
    return '<div' + S(Object.assign({ display: "flex", gap: (o.gap || 14) + "px" }, { l: o.l, t: o.t })) + ">" + C.ui.sources.map(function (s, i) {
      return '<span class="chip' + (i === active ? " on" : "") + '"' + S({ fs: o.fs }) + ">" + SRC_ICON[i] + esc(s) + "</span>";
    }).join("") + "</div>";
  }
  function label(text, o, cls) { return '<div class="label ' + (cls || "") + '"' + S(o) + ">" + esc(text) + "</div>"; }
  function chip(text, o, cls) { return '<div class="chip ' + (cls || "") + '"' + S(o) + "><span>" + esc(text) + "</span></div>"; }
  /* down/right arrow drawn as SVG (the Latin subsets have no arrow glyphs) */
  function arrow(x1, y1, x2, y2, o) {
    o = o || {};
    var sw = o.sw || 4, head = o.head || 16;
    var ang = Math.atan2(y2 - y1, x2 - x1);
    var hx1 = x2 - head * Math.cos(ang - 0.5), hy1 = y2 - head * Math.sin(ang - 0.5);
    var hx2 = x2 - head * Math.cos(ang + 0.5), hy2 = y2 - head * Math.sin(ang + 0.5);
    return '<svg' + S({ l: 0, t: 0 }) + ' width="' + D.w + '" height="' + D.h + '"><line x1="' + x1 + '" y1="' + y1 + '" x2="' + (x2 - Math.cos(ang) * 4) + '" y2="' + (y2 - Math.sin(ang) * 4) +
      '" stroke="' + (o.color || "var(--c-mint)") + '" stroke-width="' + sw + '"' + (o.dash ? ' stroke-dasharray="' + o.dash + '"' : "") + '/><path d="M' + x2 + " " + y2 + "L" + hx1 + " " + hy1 + "L" + hx2 + " " + hy2 + 'Z" fill="' + (o.color || "var(--c-mint)") + '"/></svg>';
  }
  function beat(i, o) {
    var b = C.beats[i];
    return '<div class="beat"' + S({ l: o.l, t: o.t, w: o.w }) + ">" +
      (o.num ? '<div class="num"' + S({ fs: o.num, "margin-bottom": o.numGap == null ? 14 : o.numGap }) + ">" + (i + 1) + "</div>" : "") +
      '<div class="h"' + S({ fs: o.h }) + ">" + esc(b.h) + "</div>" +
      (o.b ? '<div class="b"' + S({ fs: o.b, "margin-top": o.gap || 8 }) + ">" + esc(b.b) + "</div>" : "") + "</div>";
  }
  function beatRow(i, o) {
    var b = C.beats[i];
    return '<div class="beat"' + S({ l: o.l, t: o.t, w: o.w, display: "flex", gap: (o.colGap || 28) + "px", "align-items": "flex-start" }) + ">" +
      '<div class="num"' + S({ fs: o.num, width: o.numW, "flex": "none" }) + ">" + (i + 1) + "</div>" +
      '<div><div class="h"' + S({ fs: o.h }) + ">" + esc(b.h) + "</div>" +
      (o.b ? '<div class="b"' + S({ fs: o.b, "margin-top": o.gap || 6 }) + ">" + esc(b.b) + "</div>" : "") + "</div></div>";
  }
  var ED = C.editionPosters;
  var STD_PANEL = { width: 556, waveHeight: 70, seed: 19, style: "mixed", selection: [0.3, 0.56], duration: "7.40 s", tooltip: { text: "Drag and Drop", at: 0.43, y: 54 }, meter: "&minus;14.1", meterUnit: "LUFS" };
  function P(over) { return Object.assign({}, STD_PANEL, over); }

  /* edition poster scaffold: lock-up, tag, footer (same as master) */
  function editionFrame(extra) {
    return bgwave(extra.wave || { l: -20, t: 760 }, { width: 1120, height: 520 }) +
      lockup({ l: 64, t: 58, s: 60, fs: 30 }) + tag({ r: 64, t: 78, fs: L(13, 16) }) + extra.body +
      footer({ l: 64, t: 1222, w: 952, W: 1080, fs: L(16, 17) });
  }

  /* ---------- designs ---------- */
  var DESIGNS = {
    "edition-master": function () {
      return editionFrame({ body:
        mega(C.mega, { l: 56, t: 146, w: 960, h: 470, max: 300 }) +
        sub(C.sub, { l: 64, t: L(668, 650), w: L(760, 820), fs: L(25, 25) }) +
        scale({ l: 64, t: 790, w: 952, fs: 17 }) +
        panel(STD_PANEL, { l: 64, t: 862, z: 1.66 }) +
        streaks(160, 440, 1128) +
        slab({ l: 480, t: 1112 })
      });
    },

    "edition-windows": function () {
      var e = ED.windows;
      return editionFrame({ wave: { l: -20, t: 800 }, body:
        chip(e.label, { l: 64, t: 146, fs: L(15, 18) }, "ink") +
        mega(e.mega, { l: 56, t: 214, w: 948, h: 380, max: 280 }) +
        sub(e.sub, { l: 64, t: 632, w: L(600, 620), fs: L(24, 24) }) +
        srcCard({ l: 736, t: L(586, 572), w: 280, fs: L(19, 21), z: 3 }, 2) +
        panel(P({ width: 580, waveHeight: 92, rows: 2, seed: 7, selection: [0.36, 0.62], tooltip: null, duration: null, meter: "&minus;12.6", meterUnit: "dBFS" }), { l: 64, t: 792, z: 1.32 }) +
        '<div class="ab"' + S({ l: 28, t: 792 + 38 * 1.32 + 6, h: 92 * 1.32 - 12 }) + ">" +
          '<span style="width:34px;height:34px;font-size:20px">A</span><span style="width:34px;height:34px;font-size:20px">B</span></div>' +
        label(C.ui.abNote, { l: 64, t: 1008, fs: L(14, 16), color: "var(--c-paper)", "text-transform": "none", "letter-spacing": "0.02em" }) +
        keys(["Alt", "`"], { l: 64, t: 1050, fs: 30 }) +
        label(C.ui.shortcutNote, { l: 268, t: 1068, fs: L(14, 16) }) +
        label(C.ui.winReq, { r: 64, t: 1068, fs: L(14, 16), "text-transform": "none" }) +
        scale({ l: 64, t: 1150, w: 952, fs: 15 })
      });
    },

    "edition-macos": function () {
      var e = ED.macos;
      return editionFrame({ wave: { l: -20, t: 820 }, body:
        panel({ width: 600, waveHeight: 46, seed: 19, style: "mixed", toolbar: false, selection: [0.52, 0.71] }, { l: 64, t: 152, z: 1.58, s10: true }) +
        // strip + ink shadow end at about y 274; label and tag sit 18 px clear of it
        '<svg' + S({ l: 64, t: L(299, 300) }) + ' width="22" height="22" viewBox="0 0 12 12" fill="none" stroke="#b0fbd3" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"><path d="M3.6 1.8h4.8M4.4 1.8v3.4L2.8 7.2h6.4L7.6 5.2V1.8M6 7.2v3.4"/></svg>' +
        label(C.ui.waveOnly, { l: 96, t: L(301, 300), fs: L(14, 16), color: "var(--c-paper)" }) +
        chip(e.label, { r: 64, t: 292, fs: L(14, 17) }, "ink") +
        mega(e.mega, { l: 56, t: L(352, 364), w: 950, h: 440, max: 290 }) +
        sub(e.sub, { l: 64, t: L(832, 844), w: 900, fs: L(24, 24) }) +
        label(C.ui.source, { l: 64, t: 962, fs: L(13, 15) }) +
        srcChips({ l: 64, t: 992, fs: L(15, 18), gap: 12 }, 2) +
        label(C.ui.shortcutNote, { r: 64, t: 962, fs: L(13, 15) }) +
        keys(["fn", "`"], { r: 70, t: 984, fs: 34 }) +
        scale({ l: 64, t: 1130, w: 952, fs: 15 })
      });
    },

    "edition-plugin": function () {
      var e = ED.plugin, u = C.ui;
      var lanes = [u.tracks[0], u.tracks[1], u.tracks[3], u.group, u.master];
      var dawW = 952, dawH = 372, ruler = 32, laneH = (dawH - ruler) / lanes.length, head = 168;
      var d = '<div class="daw"' + S({ l: 64, t: 142, w: dawW, h: dawH }) + '><div class="ruler"' + S({ h: ruler }) + ">";
      for (var x = head + 20; x < dawW; x += 64) d += "<i" + S({ l: x, h: (x - head - 20) % 256 === 0 ? 14 : 7 }) + "></i>";
      d += "</div>";
      var clips = [[[20, 300], [340, 520], [560, 760]], [[20, 760]], [[120, 420], [470, 700]], [[20, 760]], [[20, 760]]];
      lanes.forEach(function (n, i) {
        var top = ruler + i * laneH;
        d += '<div class="lane"' + S({ t: top, h: laneH }) + '><div class="th"' + S({ w: head, fs: L(16, 18) }) + ">" + esc(n) +
          (i >= 2 ? '<span' + S({ "margin-left": "auto", "margin-right": 14, width: 12, height: 12, background: "var(--c-mint)", display: "block" }) + "></span>" : "") + "</div>";
        clips[i].forEach(function (c, k) {
          d += '<div class="clip"' + S({ l: head + c[0], t: 8, w: c[1] - c[0], h: laneH - 14 }) + ' data-wave=\'{"width":' + (c[1] - c[0]) + ',"height":' + Math.round(laneH - 20) + ',"seed":' + (i * 7 + k + 3) + ',"style":"' + (i === 0 ? "beat" : "dense") + '","step":2}\'></div>';
        });
        d += "</div>";
      });
      d += '<div class="playhead"' + S({ l: head + 420, t: 0, h: dawH }) + "></div></div>";
      return editionFrame({ wave: { l: -20, t: 900 }, body:
        d +
        '<div class="inserts"' + S({ l: 716, t: 168, w: 270 }) + '><div class="ttl">' + esc(u.tracks[3]) + " · " + esc(u.formats) + "</div>" +
          u.inserts.map(function (s, i) { return '<div class="slot' + (i === 2 ? " on" : "") + '"><i></i>' + esc(s) + "</div>"; }).join("") +
          '<div class="slot empty"></div></div>' +
        panel(P({ width: 470, waveHeight: 64, seed: 5, selection: [0.5, 0.74], tooltip: { text: "Drag and Drop", at: 0.62, y: 46 }, meter: "&minus;9.8", meterUnit: "dBFS" }), { l: 230, t: 446, z: 1.34 }) +
        label(u.pluginNote, { l: 64, t: 656, fs: L(14, 16), color: "var(--c-paper)", "text-transform": "none", "letter-spacing": "0.02em" }) +
        mega(e.mega, { l: 56, t: 700, w: 950, h: 330, max: 230 }) +
        chip(e.label, { r: 64, t: L(930, 884), fs: L(15, 17) }, "ink") +
        sub(e.sub, { l: 64, t: 1046, w: 900, fs: L(23, 23) }) +
        scale({ l: 64, t: 1150, w: 952, fs: 15 })
      });
    },

    "edition-linux": function () {
      var e = ED.linux, u = C.ui;
      return editionFrame({ wave: { l: -20, t: 860 }, body:
        chip(e.label, { l: 64, t: 146, fs: L(15, 18) }, "ink") +
        mega(e.mega, { l: 56, t: 214, w: 950, h: 390, max: 280 }) +
        keys(["Super", "`"], { l: 70, t: 668, fs: 50 }) +
        label(u.request, { l: 452, t: L(676, 672), fs: L(12, 15), color: "var(--c-paper)" }) +
        arrow(452, 722, 686, 722, { sw: 5, head: 20, dash: "14 10" }) +
        '<div class="inserts"' + S({ l: 704, t: 648, w: 312 }) + '><div class="ttl">' + esc(u.instance) + "</div>" +
          '<div class="slot on"><i></i>Backtracking</div>' +
          '<div style="margin-top:10px;zoom:0.96"><div class="btp c" style="box-shadow:none" data-btp=\'{"width":300,"waveHeight":40,"seed":12,"toolbar":false,"style":"beat"}\'></div></div></div>' +
        arrow(860, 832, 860, 872, { sw: 5, head: 18 }) +
        slab({ l: 704, t: 884, name: u.snapshot, meta: FILE, cursor: false }) +
        '<div class="cmd"' + S({ l: 64, t: 894, fs: 18 }) + '><span class="p">$</span>' + esc(u.command) + "</div>" +
        sub(e.sub, { l: 64, t: 1014, w: 940, fs: L(23, 23) }) +
        scale({ l: 64, t: 1150, w: 952, fs: 15 })
      });
    },

    "github-social": function () {
      return bgwave({ l: -20, t: 380 }, { width: 1320, height: 300, seed: 31 }) +
        lockup({ l: 64, t: 52, s: 52, fs: 26 }) + tag({ r: 64, t: 70, fs: L(12, 15) }) +
        mega(C.mega, { l: 58, t: 140, w: 560, h: 340, max: 220 }) +
        panel(P({ width: 500, tooltip: { text: "Drag and Drop", at: 0.43, y: 52 } }), { l: 668, t: 144, z: 1.08, s10: true }) +
        streaks(668, 790, 352, { gap: 18 }) +
        slab({ l: 820, t: 336, z: 0.86 }) +
        sub(C.sub, { l: 668, t: 446, w: 540, fs: L(18, 18) }) +
        footer({ l: 64, t: 560, w: 1152, W: 1280, fs: L(13, 15), row2: false }) +
        '<div class="url"' + S({ l: 64, t: 500, fs: L(15, 16) }) + ">" + esc(C.repo) + "</div>";
    },

    "instagram-feed": function () {
      var rows = [0, 1, 2].map(function (i) { return 676 + i * 190; });
      return bgwave({ l: -20, t: 1060 }, { width: 1120, height: 330, seed: 23 }) +
        lockup({ l: 64, t: 58, s: 60, fs: 30 }) + tag({ r: 64, t: 78, fs: L(13, 16) }) +
        mega(C.mega, { l: 56, t: 150, w: 960, h: 470, max: 300 }) +
        rows.map(function (t, i) {
          return '<div class="rule"' + S({ l: 64, t: t - 24, w: 952, opacity: 0.35 }) + "></div>" +
            beatRow(i, { l: 64, t: t, w: 600, num: 112, numW: 70, h: L(31, 31), b: L(19, 19), colGap: 22 });
        }).join("") +
        panel({ width: 290, waveHeight: 56, seed: 3, toolbar: false, style: "mixed" }, { l: 726, t: rows[0] + 6, z: 1, s10: true }) +
        panel({ width: 290, waveHeight: 56, seed: 3, toolbar: false, style: "mixed", selection: [0.34, 0.68] }, { l: 726, t: rows[1] + 6, z: 1, s10: true }) +
        streaks(680, 744, rows[2] + 20, { gap: 14, sw: 3 }) +
        slab({ l: 760, t: rows[2] + 8, z: 0.72 }) +
        footer({ l: 64, t: 1290, w: 952, W: 1080, fs: L(15, 16) });
    },

    "instagram-story": function () {
      return '<div class="mintline"' + S({ l: 0, t: 0, w: 1080, h: 14 }) + "></div>" +
        bgwave({ l: -20, t: 30 }, { width: 1120, height: 200, seed: 51 }) +
        streaks(64, 640, 170, { gap: 22 }) +
        lockup({ l: 64, t: 282, s: 64, fs: 32 }) + tag({ r: 64, t: 304, fs: L(14, 17) }) +
        mega(C.mega, { l: 56, t: 400, w: 960, h: 560, max: 370 }) +
        [0, 1, 2].map(function (i) {
          return beatRow(i, { l: 64, t: 1010 + i * 132, w: 952, num: 96, numW: 62, h: L(34, 34), b: L(21, 21), colGap: 26 });
        }).join("") +
        cta({ l: 64, t: 1410, fs: L(26, 28) }) +
        '<div class="rule"' + S({ l: 64, t: 1512, w: 952 }) + "></div>" +
        '<div class="url"' + S({ l: 64, t: 1534, fs: L(18, 18) }) + ">" + esc(C.url) + "</div>" +
        editions({ r: 64, t: 1534, fs: L(16, 16) }) +
        bgwave({ l: -20, t: 1590 }, { width: 1120, height: 360, seed: 23 }) +
        panel({ width: 560, waveHeight: 80, seed: 19, toolbar: false, style: "mixed", selection: [0.3, 0.56] }, { l: 64, t: 1640, z: 1.66 });
    },

    "facebook-cover": function () {
      return bgwave({ l: -20, t: 330 }, { width: 1700, height: 420, seed: 23 }) +
        lockup({ l: 340, t: 72, s: 48, fs: 25 }) + tag({ r: 72, t: 88, fs: L(12, 15) }) +
        mega(C.mega, { l: 334, t: 150, w: 620, h: 380, max: 240 }) +
        panel(STD_PANEL, { l: 1010, t: 150, z: 1.0, s10: true }) +
        streaks(980, 1080, 362, { gap: 18 }) +
        slab({ l: 1104, t: 346, z: 0.9 }) +
        sub(C.sub, { l: 1010, t: 452, w: 560, fs: L(19, 19) }) +
        footer({ l: 340, t: 584, w: 1228, W: 1640, fs: L(13, 15), row2: false }) +
        '<div class="url"' + S({ l: L(800, 760), t: 604, fs: L(13, 15) }) + ">" + esc(C.url) + "</div>";
    },

    "facebook-feed": function () {
      return bgwave({ l: -20, t: 40 }, { width: 1120, height: 420, seed: 77 }) +
        lockup({ l: 64, t: 58, s: 60, fs: 30 }) + tag({ r: 64, t: 78, fs: L(13, 16) }) +
        panel(STD_PANEL, { l: 64, t: 166, z: 1.66 }) +
        streaks(150, 440, 404) +
        slab({ l: 480, t: 388 }) +
        [0, 1, 2].map(function (i) {
          return beat(i, { l: 64 + i * 328, t: 520, w: 296, num: 88, h: L(25, 25), b: L(17, 17) });
        }).join("") +
        scale({ l: 64, t: 812, w: 952, fs: 15 }) +
        mega(C.mega, { l: 56, t: 880, w: 950, h: 316, max: 300, zmax: 160 }) +
        footer({ l: 64, t: 1222, w: 952, W: 1080, fs: L(16, 17) });
    },

    "threads-post": function () {
      return bgwave({ l: -20, t: 560 }, { width: 1120, height: 440, seed: 23 }) +
        lockup({ l: 64, t: 58, s: 56, fs: 28 }) + tag({ r: 64, t: 76, fs: L(13, 16) }) +
        mega(C.mega, { l: 56, t: 168, w: 960, h: 520, max: 330 }) +
        streaks(64, 400, L(762, 746), { gap: 22 }) +
        slab({ l: 440, t: L(744, 728), z: 1.2 }) +
        '<div class="beat"' + S({ l: 64, t: 868, w: 952 }) + '><div class="h"' + S({ fs: L(40, 44) }) + ">" + esc(C.oneLiner) + "</div></div>" +
        '<div class="url"' + S({ r: 64, t: 888, fs: L(16, 17) }) + ">" + esc(C.url) + "</div>" +
        footer({ l: 64, t: 950, w: 952, W: 1080, fs: L(15, 16), row2: false });
    },

    "buymeacoffee-cover": function () {
      return bgwave({ l: -20, t: 170 }, { width: 1660, height: 260, seed: 23 }) +
        (ZH
          ? mega(C.mega, { l: 330, t: 64, w: 560, h: 272, max: 150 })
          : mega(C.mega, { l: 150, t: 128, w: 860, h: 150, max: 220, inline: true })) +
        '<div' + S({ l: L(1076, 966), t: L(104, 96), w: L(420, 420) }) + ">" +
          lockup({ s: 44, fs: 23 }).replace(' style=""', "").replace('class="lockup"', 'class="lockup" style="position:static"') +
          '<div class="beat"' + S({ "margin-top": 24 }) + '><div class="h"' + S({ fs: L(28, 32) }) + ">" + esc(C.oneLiner) + "</div></div>" +
          '<div class="wide"' + S({ fs: L(12, 15), "margin-top": 14, color: "var(--c-paper-2)" }) + ">" + esc(C.tag) + "</div>" +
          '<div class="url"' + S({ fs: L(13, 15), "margin-top": 10 }) + ">" + esc(C.url) + "</div></div>";
    },

    "youtube-channel": function () {
      /* safe area: x 507..2053, y 508.5..931.5 — every text and the icon stay inside it */
      return bgwave({ l: -40, t: 520 }, { width: 2640, height: 700, seed: 23, step: 3 }) +
        bgwave({ l: -40, t: 90, transform: "scaleY(-1)" }, { width: 2640, height: 330, seed: 61, step: 3 }) +
        '<div class="mintline"' + S({ l: 0, t: 0, w: 2560, h: 16 }) + "></div>" +
        streaks(0, 470, 740, { gap: 30, sw: 6 }) +
        '<div class="mintline"' + S({ l: 2090, t: 700, w: 470, h: 12 }) + "></div>" +
        scale({ l: 120, t: 1040, w: 2320, fs: 20 }, true) +
        lockup({ l: 540, t: 530, s: 52, fs: 28 }) + tag({ r: 540, t: 548, fs: L(15, 18) }) +
        (ZH
          ? mega(C.mega, { l: 534, t: 632, w: 1490, h: 200, max: 190, zmax: 182, inline: true })
          : mega(C.mega, { l: 534, t: 616, w: 1490, h: 232, max: 300, inline: true })) +
        '<div class="rule"' + S({ l: 540, t: 856, w: 1480 }) + "></div>" +
        '<div class="beat"' + S({ l: 540, t: 872 }) + '><div class="h"' + S({ fs: L(28, 30), "white-space": "nowrap" }) + ">" + esc(C.oneLiner) + "</div></div>" +
        '<div class="url"' + S({ r: 540, t: 878, fs: L(20, 20) }) + ">" + esc(C.url) + "</div>";
    },

    "youtube-thumbnail": function () {
      return bgwave({ l: -40, t: 640 }, { width: 2000, height: 480, seed: 23 }) +
        lockup({ l: 96, t: 64, s: 64, fs: 34 }) +
        mega(C.mega, { l: 84, t: 170, w: 1000, h: 720, max: 440 }) +
        panel(P({ width: 520 }), { l: 1150, t: 190, z: 1.3 }) +
        streaks(L(1050, 1090), 1180, 530, { gap: 26, sw: 6 }) +
        slab({ l: 1210, t: 504, z: 1.32 }) +
        '<div class="beat"' + S({ l: 96, t: L(880, 880), w: 1100 }) + '><div class="h"' + S({ fs: L(64, 68), color: "var(--c-paper)" }) + ">" + esc(C.oneLiner) + "</div></div>";
    }
  };

  var frame = document.createElement("div");
  frame.className = "frame d-" + NAME;
  frame.style.width = D.w + "px";
  frame.style.height = D.h + "px";
  frame.innerHTML = DESIGNS[NAME]();
  document.body.appendChild(frame);

  /* ---------- fit mega lines once the fonts are in ---------- */
  function fitAll() {
    var list = document.querySelectorAll("[data-fit]");
    for (var n = 0; n < list.length; n++) {
      var el = list[n], f = JSON.parse(el.getAttribute("data-fit"));
      var lines = el.querySelectorAll(".ln");
      el.style.fontSize = "100px";
      var widths = [];
      for (var i = 0; i < lines.length; i++) {
        lines[i].style.display = "inline-block";
        lines[i].style.fontSize = "100px";
        widths.push(lines[i].getBoundingClientRect().width / 100);
      }
      var lh = parseFloat(getComputedStyle(lines[0]).lineHeight) / 100 || 0.8;
      var sizes;
      if (f.inline) {
        var sp = el.querySelector(".sp");
        var total = widths.reduce(function (a, b) { return a + b; }, 0) + (sp ? sp.getBoundingClientRect().width / 100 * (lines.length - 1) : 0);
        var s = Math.min(f.max, f.w / total, f.h / lh);
        sizes = widths.map(function () { return s; });
        el.style.fontSize = s + "px";
      } else {
        sizes = widths.map(function (w) { return Math.min(f.max, f.w / w); });
        if (f.mode === "one") { var mn = Math.min.apply(null, sizes); sizes = sizes.map(function () { return mn; }); }
        var gap = ZH ? 0.06 : 0;
        var tot = sizes.reduce(function (a, b) { return a + b * (lh + gap); }, 0) - sizes[sizes.length - 1] * gap;
        if (tot > f.h) { var k = f.h / tot; sizes = sizes.map(function (x) { return x * k; }); }
      }
      for (i = 0; i < lines.length; i++) {
        lines[i].style.display = f.inline ? "inline" : "block";
        lines[i].style.fontSize = Math.floor(sizes[i] * 10) / 10 + "px";
      }
      el.setAttribute("data-sizes", sizes.map(function (x) { return Math.round(x); }).join(","));
    }
  }
  var loads = [
    document.fonts.load('900 100px "Archivo"', "TAKE"),
    document.fonts.load('700 30px "Archivo"', "Backtracking"),
    document.fonts.load('400 30px "Archivo"', "a"),
    document.fonts.load('500 16px "Open Sans"', "Backtracking"),
    document.fonts.load('600 11px "Geist Mono"', "7.40 s")
  ];
  if (ZH) {
    loads.push(document.fonts.load('900 100px "Noto Sans TC"', "剛"));
    loads.push(document.fonts.load('400 30px "Noto Sans TC"', "框"));
  }
  Promise.all(loads).then(function () { return document.fonts.ready; }).then(function () {
    fitAll();
    html.setAttribute("data-ready", "1");
  });
})();
