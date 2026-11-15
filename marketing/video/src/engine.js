/* Backtracking motion engine: deterministic, time-driven building blocks.
 *
 * Every visual is a pure function of time. A page builds its DOM once, then exposes
 * window.seek(t) which sets every element from t (seconds). Nothing animates on its own
 * (no CSS transitions/animations, no requestAnimationFrame), so frame-by-frame capture is
 * exact and en / zh-TW renders share identical timing.
 *
 * Waveform: an "audio" model returns a peak amplitude for any audio time x (seconds). The
 * panel draws a continuous filled min/max envelope with an outline (how current builds draw
 * wide views), newest audio at the right edge. Columns are aligned to audio time, so the
 * shape translates rigidly while it scrolls. With `period` set, the audio repeats exactly,
 * which makes seamless loops possible.
 */
(function () {
  "use strict";
  var E = {};
  window.E = E;

  /* ---------- math ---------- */
  E.clamp = function (x, a, b) { return Math.max(a, Math.min(b, x)); };
  E.lerp = function (a, b, u) { return a + (b - a) * u; };
  E.prog = function (t, a, b) { return b <= a ? (t >= a ? 1 : 0) : E.clamp((t - a) / (b - a), 0, 1); };
  E.outCubic = function (u) { return 1 - Math.pow(1 - u, 3); };
  E.inOut = function (u) { return u < 0.5 ? 4 * u * u * u : 1 - Math.pow(-2 * u + 2, 3) / 2; };
  E.outBack = function (u, s) { s = s == null ? 1.9 : s; var c = s + 1; return 1 + c * Math.pow(u - 1, 3) + s * Math.pow(u - 1, 2); };
  E.outQuint = function (u) { return 1 - Math.pow(1 - u, 5); };

  function hash(n, seed) {
    var x = Math.imul(n | 0, 374761393) ^ Math.imul((seed | 0) + 0x9e37, 668265263);
    x = Math.imul(x ^ (x >>> 13), 1274126177);
    x = x ^ (x >>> 16);
    return (x >>> 0) / 4294967296;
  }
  E.hash = hash;
  function mod(a, n) { return ((a % n) + n) % n; }

  /* ---------- audio model ---------- */
  var STEP = 0.125; // a 16th note at 120 BPM
  E.STEP = STEP;

  /* o: {seed, period (s, optional), kind: "beat"|"voice", moments:[{t0,len}], since (audio s; empty before)} */
  E.Audio = function (o) {
    var per = o.period ? Math.round(o.period / STEP) : 0;
    var seed = o.seed || 1;
    var kind = o.kind || "beat";
    var moments = o.moments || [];
    function idx(s) { return per ? mod(s, per) : s; }
    function stepEvents(s, out) {
      var i = idx(s), pos = mod(i, 16), t = s * STEP;
      if (kind === "beat") {
        if (pos === 0 || pos === 8 || (pos === 10 && hash(i, seed + 3) < 0.45)) out.push(t, 0.72 + 0.22 * hash(i, seed), 0.075);
        else if (pos === 4 || pos === 12) out.push(t, 0.5 + 0.22 * hash(i, seed + 1), 0.095);
        else if (pos % 2 === 0) { if (hash(i, seed + 2) < 0.8) out.push(t, 0.16 + 0.16 * hash(i, seed + 4), 0.028); }
        else if (hash(i, seed + 5) < 0.35) out.push(t, 0.1 + 0.12 * hash(i, seed + 6), 0.02);
      }
    }
    function body(x) {
      if (kind === "voice") {
        // phrases: one-second cells, some voiced, smooth inside
        var c = Math.floor(x), ci = per ? mod(c, Math.round(per * STEP)) : c;
        var on = hash(ci, seed + 9) < 0.62;
        if (!on) return 0.012;
        var u = x - c, a = 0.22 + 0.42 * hash(ci, seed + 10);
        var env = Math.sin(Math.PI * E.clamp(u * 1.08, 0, 1));
        var wob = 0.8 + 0.2 * Math.sin((x * 9.3 + hash(ci, seed + 11) * 6) * 2 * Math.PI / 3);
        return a * Math.pow(env, 0.7) * wob;
      }
      var bar = Math.floor(x / 2), bi = per ? mod(bar, Math.round(per / 2)) : bar;
      var u2 = x / 2 - bar;
      var b0 = hash(bi, seed + 7), b1 = hash(per ? mod(bi + 1, Math.round(per / 2)) : bi + 1, seed + 7);
      return 0.1 + 0.09 * E.lerp(b0, b1, u2 * u2 * (3 - 2 * u2));
    }
    function moment(x) {
      var s = 0;
      for (var k = 0; k < moments.length; k++) {
        var m = moments[k];
        if (x < m.t0 || x > m.t0 + m.len) continue;
        var u = x - m.t0, r = m.t0 + m.len - x;
        var env = Math.min(1, u / 0.18) * Math.min(1, r / 0.35);
        var swell = 0.5 + 0.22 * Math.sin(Math.PI * u / m.len);
        var wob = 0.86 + 0.14 * Math.sin(u * 2 * Math.PI * 4);
        s += env * swell * wob;
      }
      return s;
    }
    var ev = [];
    function amp(x) {
      if (o.since != null && x < o.since) return 0;
      var s = Math.floor(x / STEP), sum = body(x) + moment(x);
      ev.length = 0;
      for (var k = s - 4; k <= s; k++) stepEvents(k, ev);
      for (var j = 0; j < ev.length; j += 3) {
        var d = x - ev[j];
        if (d >= 0) sum += ev[j + 1] * Math.exp(-d / ev[j + 2]);
      }
      var a = Math.tanh(sum * 1.15) * 0.96;
      if (o.since != null && x < o.since + 0.02) a *= (x - o.since) / 0.02;
      return a;
    }
    /* peak in [a,b): decays are monotonic between onsets, so check a and every onset inside */
    function peak(a, b) {
      var m = amp(a);
      for (var k = Math.ceil(a / STEP); k * STEP < b; k++) {
        var v = amp(k * STEP + 1e-5);
        if (v > m) m = v;
      }
      if (moments.length || kind === "voice") { var v2 = amp((a + b) / 2); if (v2 > m) m = v2; }
      return m;
    }
    function mean(a, b) {
      var n = 12, s = 0;
      for (var i = 0; i < n; i++) s += amp(a + (b - a) * (i + 0.5) / n);
      return s / n;
    }
    return { amp: amp, peak: peak, mean: mean, period: o.period || 0, seed: seed };
  };

  /* meter readouts (display refresh 10 Hz, like a UI timer) */
  function minus(v) { return (v < 0 ? "−" : v > 0 ? "+" : "") + Math.abs(v).toFixed(1); }
  E.meterValue = function (audio, now, mode) {
    var tq = Math.floor(now * 10 + 1e-6) / 10;
    if (mode === "VU") { var m = audio.mean(tq - 0.3, tq); return minus(Math.max(-20, 20 * Math.log10(Math.max(m, 1e-4)) + 9.5)); }
    if (mode === "LUFS") { var l = audio.mean(tq - 0.4, tq); return minus(Math.max(-60, 20 * Math.log10(Math.max(l, 1e-4)) - 8.5)); }
    var p = audio.peak(tq - 0.1, tq);
    return minus(Math.max(-60, 20 * Math.log10(Math.max(p, 1e-4)) - 3.2)).replace("+", "");
  };

  /* ---------- waveform drawing ---------- */
  /* o: {audio, x, y, w, h, tEnd, pps (px per audio s), col (px per column), fill, stroke, lw,
         sel:[a,b] audio times, selFill, selStroke, axis, guides (color), floor} */
  E.wavePath = function (o) {
    var dt = o.col / o.pps;
    if (o.audio.period) { var N = Math.max(1, Math.round(o.audio.period / dt)); dt = o.audio.period / N; }
    var nPer = o.audio.period ? Math.round(o.audio.period / dt) : 0;
    var tStart = o.tEnd - o.w / o.pps;
    var k0 = Math.floor(tStart / dt) - 1, k1 = Math.floor(o.tEnd / dt) - 1;
    var mid = o.y + o.h / 2, sc = (o.h / 2) * 0.94, fl = o.floor == null ? 0.5 : o.floor;
    var xs = [], tops = [], bots = [];
    for (var k = k0; k <= k1; k++) {
      var ta = k * dt, a = o.audio.peak(ta, ta + dt);
      var kj = nPer ? mod(k, nPer) : k;
      var X = o.x + o.w - (o.tEnd - (ta + dt / 2)) * o.pps;
      xs.push(X);
      tops.push(mid - Math.max(fl, a * sc * (0.86 + 0.14 * hash(kj, o.audio.seed + 21))));
      bots.push(mid + Math.max(fl, a * sc * (0.8 + 0.2 * hash(kj, o.audio.seed + 22))));
    }
    var p = new Path2D();
    var n = xs.length;
    if (!n) return p;
    var xr = o.x + o.w;
    p.moveTo(xs[0], tops[0]);
    for (var i = 1; i < n; i++) p.lineTo(xs[i], tops[i]);
    p.lineTo(xr, tops[n - 1]);
    p.lineTo(xr, bots[n - 1]);
    for (i = n - 1; i >= 0; i--) p.lineTo(xs[i], bots[i]);
    p.closePath();
    return p;
  };

  E.drawWave = function (ctx, o) {
    var tStart = o.tEnd - o.w / o.pps;
    ctx.save();
    ctx.beginPath(); ctx.rect(o.x, o.y - 2, o.w, o.h + 4); ctx.clip();
    if (o.guides) {
      ctx.fillStyle = o.guides;
      for (var g = Math.ceil(tStart); g <= o.tEnd; g++) {
        var gx = o.x + o.w - (o.tEnd - g) * o.pps;
        ctx.fillRect(Math.round(gx * 2) / 2 - 0.5, o.y, 1, o.h);
      }
    }
    if (o.band) { ctx.fillStyle = o.band.color; ctx.fillRect(o.band.x0, o.y, o.band.x1 - o.band.x0, o.h); }
    if (o.axis) { ctx.fillStyle = o.axis; ctx.fillRect(o.x, o.y + o.h / 2 - 0.5, o.w, 1); }
    var p = E.wavePath(o);
    ctx.fillStyle = o.fill; ctx.fill(p);
    if (o.stroke) { ctx.lineWidth = o.lw || 0.6; ctx.lineJoin = "round"; ctx.strokeStyle = o.stroke; ctx.stroke(p); }
    if (o.sel) {
      var sx0 = o.x + o.w - (o.tEnd - o.sel[0]) * o.pps, sx1 = o.x + o.w - (o.tEnd - o.sel[1]) * o.pps;
      ctx.save(); ctx.beginPath(); ctx.rect(sx0, o.y - 2, sx1 - sx0, o.h + 4); ctx.clip();
      ctx.fillStyle = o.selFill; ctx.fill(p);
      if (o.selStroke) { ctx.strokeStyle = o.selStroke; ctx.stroke(p); }
      ctx.restore();
    }
    if (o.hi) { // highlight colour for a time range (used for "the moment" outside the app)
      var hx0 = o.x + o.w - (o.tEnd - o.hi[0]) * o.pps, hx1 = o.x + o.w - (o.tEnd - o.hi[1]) * o.pps;
      ctx.save(); ctx.beginPath(); ctx.rect(hx0, o.y - 2, hx1 - hx0, o.h + 4); ctx.clip();
      ctx.fillStyle = o.hiFill; ctx.fill(p); ctx.restore();
    }
    ctx.restore();
  };

  /* ---------- DOM helpers ---------- */
  E.el = function (tag, cls, parent, html) {
    var d = document.createElement(tag);
    if (cls) d.className = cls;
    if (html != null) d.innerHTML = html;
    if (parent) parent.appendChild(d);
    return d;
  };
  E.pos = function (el, x, y) { el.style.left = x + "px"; el.style.top = y + "px"; };
  E.show = function (el, on) { el.style.display = on ? "" : "none"; };
  E.canvas = function (parent, w, h, scale) {
    var c = E.el("canvas", "", parent);
    scale = scale || 1;
    c.width = Math.round(w * scale); c.height = Math.round(h * scale);
    c.style.width = w + "px"; c.style.height = h + "px";
    c._s = scale; c._w = w; c._h = h;
    return c;
  };
  E.ctx = function (c) {
    var x = c.getContext("2d");
    x.setTransform(c._s, 0, 0, c._s, 0, 0);
    x.clearRect(0, 0, c._w, c._h);
    return x;
  };

  /* slam-in: hidden before t0; drops in from below with a slight overshoot in ~120 ms */
  E.slam = function (el, t, t0, o) {
    o = o || {};
    if (t < t0 || (o.out != null && t >= o.out)) { el.style.visibility = "hidden"; return false; }
    el.style.visibility = "visible";
    var u = E.prog(t, t0, t0 + (o.dur || 0.12));
    var e = E.outBack(u, 2.2);
    var dy = (o.dy == null ? 46 : o.dy) * (1 - e);
    var dx = (o.dx || 0) * (1 - e);
    var s = 1 + (o.s == null ? 0.06 : o.s) * (1 - e);
    el.style.transform = "translate(" + dx.toFixed(2) + "px," + dy.toFixed(2) + "px) scale(" + s.toFixed(4) + ")";
    return true;
  };

  /* ---------- icons ---------- */
  var ICON = {
    pause: '<svg viewBox="0 0 12 12"><rect x="2.5" y="2" width="2.4" height="8" rx=".6" fill="currentColor"/><rect x="7.1" y="2" width="2.4" height="8" rx=".6" fill="currentColor"/></svg>',
    trash: '<svg viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.15" stroke-linecap="round" stroke-linejoin="round"><path d="M2 3.2h8M4.6 3.2V2.1h2.8v1.1M3 3.2l.5 6.9h5l.5-6.9M5 5v3.4M7 5v3.4"/></svg>',
    pin: '<svg viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round"><path d="M3.6 1.8h4.8M4.4 1.8v3.4L2.8 7.2h6.4L7.6 5.2V1.8M6 7.2v3.4"/></svg>',
    gear: '<svg viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.1"><circle cx="6" cy="6" r="1.7"/><path d="M6 1v1.4M6 9.6V11M1 6h1.4M9.6 6H11M2.46 2.46l1 1M8.54 8.54l1 1M2.46 9.54l1-1M8.54 3.46l1-1"/><circle cx="6" cy="6" r="3.4"/></svg>',
    speaker: '<svg viewBox="0 0 48 48" fill="none" stroke="currentColor" stroke-width="4" stroke-linejoin="round" stroke-linecap="round"><path d="M8 18h8l10-8v28l-10-8H8Z" fill="currentColor"/><path d="M32 17c3 4 3 10 0 14M37 12c6 7 6 17 0 24"/></svg>',
    mic: '<svg viewBox="0 0 48 48" fill="none" stroke="currentColor" stroke-width="4" stroke-linecap="round"><rect x="17" y="5" width="14" height="24" rx="7" fill="currentColor"/><path d="M11 22c0 7 6 13 13 13s13-6 13-13M24 35v8M16 43h16"/></svg>',
    app: '<svg viewBox="0 0 48 48" fill="none" stroke="currentColor" stroke-width="4" stroke-linejoin="round"><rect x="5" y="8" width="38" height="32" rx="3"/><path d="M5 16h38" /><path d="M20 22v12l10-6Z" fill="currentColor"/></svg>',
    folder: '<svg viewBox="0 0 48 48" fill="none" stroke="currentColor" stroke-width="4" stroke-linejoin="round"><path d="M4 12h15l4 5h21v23H4Z" fill="currentColor"/></svg>',
    globe: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3c3 3 3 15 0 18M12 3c-3 3-3 15 0 18"/></svg>',
    cursor: '<svg width="28" height="40" viewBox="0 0 14 20"><path d="M1 1v15.5l3.8-3.6 2.6 6 2.4-1-2.5-5.9h5.3Z" fill="#fff" stroke="#070c3d" stroke-width="1.1" stroke-linejoin="round"/></svg>'
  };
  E.ICON = ICON;

  /* ---------- Backtracking panel ---------- */
  /* o: {x, y, z (scale), width (logical, default 560), waveH (70), rows, plugin (no pin),
         status (bool, default true), audio, audioB} */
  E.Panel = function (parent, o) {
    var P = { o: o };
    var W = o.width || 560, H = o.waveH || 70, z = o.z || 1;
    var ww = W - 22; // padding 10 + border 1, each side
    P.W = W; P.H = H; P.ww = ww; P.z = z;
    P.pps = o.pps || ww / (o.history || 20);
    var wrap = E.el("div", "pw", parent);
    E.pos(wrap, o.x, o.y);
    var b = E.el("div", "btp", wrap);
    b.style.width = W + "px";
    b.style.transform = "scale(" + z + ")";
    var bar = E.el("div", "btp-bar", b);
    E.el("span", "btp-title", bar, "Backtracking");
    E.el("span", "btp-btn", bar, ICON.pause);
    var dot = E.el("span", "btp-dot", bar);
    E.el("span", "btp-btn", bar, ICON.trash);
    if (!o.plugin) E.el("span", "btp-btn on", bar, ICON.pin);
    E.el("span", "btp-spacer", bar);
    var m1 = E.el("span", "btp-meter", bar), m2 = E.el("span", "btp-meter ab", bar);
    E.el("span", "btp-btn", bar, ICON.gear);
    var wv = E.el("div", "btp-wave", b);
    wv.style.height = H + "px";
    var cv = E.canvas(wv, ww, H, z * (o.ss || 1));
    var dur = E.el("span", "btp-dur", wv);
    var tip = E.el("span", "btp-tip", wv);
    var st = E.el("div", "btp-status", b);
    if (o.status === false) st.style.display = "none";
    P.wrap = wrap; P.box = b; P.bar = bar;
    /* map waveform-local coordinates (logical px) to stage px */
    P.sx = function (lx) { return o.x + z * (11 + lx); };
    P.sy = function (ly) { return o.y + z * (1 + (P.strip ? 10 : 38) + ly); };
    /* audio time -> local x, given the view end */
    P.tx = function (ta, tEnd) { return ww - (tEnd - ta) * P.pps; };
    P.xt = function (lx, tEnd) { return tEnd - (ww - lx) / P.pps; };

    /* s: {now, view (tEnd of view; default now), sel:[a,b], rows:1|2, sinceB, meters:[...], mode,
           rec (bool), status, tip:{text,x,y}, dur, strip (bool), alpha (bg opacity 0..1), hoverAll} */
    P.draw = function (s) {
      var now = s.now, view = s.view == null ? now : s.view;
      var rows = s.rows || 1;
      P.strip = !!s.strip;
      bar.style.display = s.strip ? "none" : "";
      b.classList.toggle("strip", !!s.strip);
      st.style.display = s.strip || o.status === false ? "none" : "";
      var alpha = s.alpha == null ? 1 : s.alpha;
      b.style.background = "rgba(31,34,37," + alpha + ")";
      b.style.borderColor = s.strip && s.border === false ? "transparent" : "";
      // "Show waveform only" disables the native shadow, so the campaign's ink shadow goes too
      b.style.boxShadow = s.shadow === false || s.strip ? "none" : "";
      // rec indicator blinks at 1 Hz while recording
      var blinkOn = mod(now, 1) < 0.62;
      dot.style.background = s.rec === false ? "#5b6066" : blinkOn ? "#e5484d" : "#6a2b30";
      // meters
      var mode = s.mode || "dBFS";
      if (rows === 2) {
        m1.className = "btp-meter ab"; m2.style.display = "";
        m1.textContent = "A " + E.meterValue(o.audio, now, mode) + " " + mode;
        m2.textContent = "B " + (now - (s.sinceB == null ? -1e9 : s.sinceB) < 0.4 ? "—" : E.meterValue(o.audioB, now, mode)) + " " + mode;
      } else {
        m1.className = "btp-meter"; m2.style.display = "none";
        m1.textContent = (s.meterText || E.meterValue(o.audio, now, mode)) + " " + mode;
      }
      // waveform
      var ctx = E.ctx(cv);
      var rh = H / rows;
      var guides = s.guides === false ? null : "rgba(255,255,255,0.075)";
      var band = null;
      if (s.sel) {
        band = { color: "rgba(255,79,154,0.09)", x0: P.tx(s.sel[0], view), x1: P.tx(s.sel[1], view) };
      } else if (s.hoverAll) {
        band = { color: "rgba(111,232,176,0.16)", x0: 0, x1: ww };
      }
      for (var r = 0; r < rows; r++) {
        var au = r === 0 ? o.audio : o.audioB;
        if (rows === 2 && r === 1 && s.sinceB != null) au = s.audioBSince || au;
        E.drawWave(ctx, {
          audio: au, x: 0, y: r * rh, w: ww, h: rh, tEnd: view, pps: P.pps, col: 1.25,
          fill: "#6fe8b0", stroke: "#9af2c9", lw: 0.6, axis: "#3d4247",
          guides: r === 0 ? guides : null, band: r === 0 ? band : null,
          sel: s.sel, selFill: "#c21f6c", selStroke: "#ff4f9a", floor: rows === 2 ? 0.4 : 0.5
        });
        if (rows === 2 && r === 1 && guides) { // guides span both rows
          ctx.fillStyle = guides;
          for (var g = Math.ceil(view - ww / P.pps); g <= view; g++) ctx.fillRect(Math.round(P.tx(g, view) * 2) / 2 - 0.5, rh, 1, rh);
        }
      }
      if (rows === 2) { ctx.fillStyle = "#373b3f"; ctx.fillRect(0, rh - 0.5, ww, 1); }
      // readouts
      E.show(dur, !!s.dur); if (s.dur) dur.textContent = s.dur;
      E.show(tip, !!s.tip);
      if (s.tip) { tip.textContent = s.tip.text; tip.style.left = s.tip.x + "px"; tip.style.top = (s.tip.y == null ? 50 : s.tip.y) + "px"; }
      st.textContent = s.status || " ";
    };
    return P;
  };

  /* ---------- cursor ---------- */
  E.Cursor = function (parent, scale) {
    var C = { scale: scale || 1.25 };
    var ring = E.el("div", "ring", parent);
    var c = E.el("div", "cursor", parent, ICON.cursor);
    c.style.transform = "scale(" + C.scale + ")";
    /* hotspot of the arrow is ~ (2,2) in the 28x40 svg */
    C.set = function (x, y, o) {
      o = o || {};
      E.show(c, o.hide !== true);
      E.pos(c, x - 2 * C.scale, y - 2 * C.scale);
      var pt = o.pressT; // seconds since press
      if (pt != null && pt >= 0 && pt < 0.32 && o.hide !== true) {
        var u = E.outCubic(pt / 0.32), r = 10 + 30 * u;
        ring.style.display = "";
        ring.style.width = ring.style.height = 2 * r + "px";
        E.pos(ring, x - r, y - r);
        ring.style.opacity = (1 - u).toFixed(3);
      } else ring.style.display = "none";
    };
    C.el = c;
    return C;
  };

  /* piecewise cursor path: keys [{t, x, y}] with eased segments */
  E.path = function (keys, t) {
    if (t <= keys[0].t) return keys[0];
    for (var i = 1; i < keys.length; i++) {
      var a = keys[i - 1], b = keys[i];
      if (t <= b.t) {
        var u = E.prog(t, a.t, b.t);
        var e = b.ease === "lin" ? u : b.ease === "out" ? E.outCubic(u) : E.inOut(u);
        return { x: E.lerp(a.x, b.x, e), y: E.lerp(a.y, b.y, e) };
      }
    }
    return keys[keys.length - 1];
  };

  /* ---------- keycaps ---------- */
  E.Keys = function (parent, parts) {
    var K = E.el("div", "keys abs", parent);
    var caps = [];
    parts.forEach(function (p) {
      if (p === "+") { E.el("span", "plus", K, "+"); return; }
      var k = E.el("span", "key" + (p === "`" ? " grave" : ""), K);
      if (p === "fn") k.innerHTML = "fn " + '<span style="display:inline-block;width:34px;height:34px">' + ICON.globe + "</span>";
      else k.textContent = p;
      caps.push(k);
    });
    return { el: K, caps: caps, press: function (on) { caps.forEach(function (k) { k.classList.toggle("down", !!on); }); } };
  };

  /* ---------- WAV slab + streaks ---------- */
  E.Slab = function (parent, name, meta, scale) {
    var s = E.el("div", "slab abs", parent, '<div class="ic">WAV</div><div><div class="n"></div><div class="mm"></div></div>');
    s.querySelector(".n").textContent = name;
    s.querySelector(".mm").textContent = meta;
    s.style.transformOrigin = "0 0";
    var sv = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    sv.setAttribute("class", "abs");
    sv.style.left = "0"; sv.style.top = "0"; sv.style.overflow = "visible";
    sv.setAttribute("width", "10"); sv.setAttribute("height", "10");
    parent.insertBefore(sv, s);
    var lines = [];
    for (var i = 0; i < 3; i++) {
      var l = document.createElementNS("http://www.w3.org/2000/svg", "line");
      l.setAttribute("stroke", "#b0fbd3"); l.setAttribute("stroke-width", "5"); l.setAttribute("stroke-linecap", "round");
      l.setAttribute("opacity", [1, 0.7, 0.45][i]);
      sv.appendChild(l); lines.push(l);
    }
    var S = { el: s, svg: sv, scale: scale || 1 };
    S.meta = function (m) { s.querySelector(".mm").textContent = m; };
    /* x,y = top-left of slab; vx,vy = velocity (px/s) for the streaks */
    S.set = function (x, y, o) {
      o = o || {};
      var vis = o.hide !== true;
      E.show(s, vis); E.show(sv, vis);
      if (!vis) return;
      E.pos(s, x, y);
      s.style.transform = "scale(" + (S.scale * (o.k || 1)).toFixed(4) + ")";
      var vx = o.vx || 0, vy = o.vy || 0, sp = Math.hypot(vx, vy);
      var k = S.scale * (o.k || 1);
      var sw = s.offsetWidth * k, sh = s.offsetHeight * k;
      var ux = sp ? vx / sp : 1, uy = sp ? vy / sp : 0, nx = -uy, ny = ux;
      var cx = x + sw / 2, cy = y + sh / 2;
      // trailing edge of the slab's box along the direction of travel
      var d = Math.min(Math.abs(ux) > 1e-3 ? sw / 2 / Math.abs(ux) : 1e9, Math.abs(uy) > 1e-3 ? sh / 2 / Math.abs(uy) : 1e9) + 16;
      for (var i = 0; i < 3; i++) {
        var L = sp < 260 ? 0 : Math.min(300, (sp - 200) * 0.12) * [1, 0.68, 0.4][i];
        var off = [-0.3, 0, 0.3][i] * sh;
        var ax = cx - ux * d + nx * off, ay = cy - uy * d + ny * off;
        lines[i].setAttribute("x1", ax.toFixed(1)); lines[i].setAttribute("y1", ay.toFixed(1));
        lines[i].setAttribute("x2", (ax - ux * L).toFixed(1)); lines[i].setAttribute("y2", (ay - uy * L).toFixed(1));
        lines[i].style.display = L > 4 ? "" : "none";
      }
    };
    return S;
  };

  /* ---------- generic DAW timeline (unbranded) ---------- */
  /* o: {x,y,w,h, tracks, head (px), bars, pxBar, rowH} ; 120 BPM, 1 bar = 2 s */
  E.DAW = function (parent, o) {
    var D = { o: o };
    var box = E.el("div", "daw", parent);
    E.pos(box, o.x, o.y); box.style.width = o.w + "px"; box.style.height = o.h + "px";
    var head = o.head || 150, rowH = o.rowH || 110, rulerH = 34;
    var laneW = o.w - head;
    var pxBar = o.pxBar || laneW / (o.bars || 8);
    D.pxBar = pxBar; D.pps = pxBar / 2; D.head = head; D.rowH = rowH; D.rulerH = rulerH;
    var ruler = E.el("div", "daw-ruler", box);
    var grid = E.canvas(box, o.w, o.h, o.ss || 1);
    grid.style.position = "absolute"; grid.style.left = "0"; grid.style.top = "0";
    D.box = box; D.ruler = ruler; D.grid = grid;
    D.tracks = [];
    D.scroll = 0; D.vscroll = 0;
    var holder = E.el("div", "", box);
    holder.style.cssText = "position:absolute;left:0;right:0;top:" + rulerH + "px;bottom:0;overflow:hidden";
    var inner = E.el("div", "", holder);
    inner.style.cssText = "position:absolute;left:0;right:0;top:0;height:4000px";
    D.inner = inner;
    for (var i = 0; i < (o.tracks || 3); i++) {
      var tr = E.el("div", "daw-track", inner);
      tr.style.top = i * rowH + "px"; tr.style.height = rowH + "px";
      var hd = E.el("div", "daw-head", tr); hd.style.width = head + "px";
      E.el("i", "", hd);
      var b1 = E.el("b", "", hd), b2 = E.el("b", "", hd);
      b1.style.left = "16px"; b2.style.left = "46px";
      var lane = E.el("div", "daw-lane", tr); lane.style.left = head + "px";
      D.tracks.push({ el: tr, lane: lane });
    }
    var play = E.el("div", "daw-play", box);
    D.drawGrid = function () {
      var ctx = E.ctx(grid);
      ctx.fillStyle = "rgba(255,255,255,0.05)";
      var nb = Math.ceil(laneW / pxBar) + 2;
      ruler.innerHTML = "";
      for (var bI = 0; bI < nb * 4; bI++) {
        var x = head + bI * pxBar / 4 - D.scroll;
        if (x < head - 1 || x > o.w) continue;
        var isBar = bI % 4 === 0;
        ctx.fillStyle = isBar ? "rgba(255,255,255,0.13)" : "rgba(255,255,255,0.045)";
        ctx.fillRect(Math.round(x), rulerH, 1, o.h - rulerH);
        ctx.fillRect(Math.round(x), isBar ? 14 : 24, 1, isBar ? 20 : 10);
        if (isBar) { var sp = E.el("span", "", ruler, String(bI / 4 + 1 + (o.barOffset || 0))); sp.style.left = x + "px"; }
      }
    };
    /* lane x (from lane start) -> stage x */
    D.sx = function (lx) { return o.x + 1 + head + lx - D.scroll; };
    D.sy = function (track) { return o.y + 1 + rulerH + track * rowH - D.vscroll; };
    D.trackAt = function (sy) { return Math.floor((sy - o.y - 1 - rulerH + D.vscroll) / rowH); };
    D.setVScroll = function (v) { D.vscroll = v; inner.style.transform = "translateY(" + (-v) + "px)"; };
    D.playhead = function (lx) { if (lx == null) { play.style.display = "none"; return; } play.style.display = ""; play.style.left = head + lx - D.scroll + "px"; };
    D.drawGrid();
    return D;
  };

  /* clip element on a DAW lane, with the selected audio drawn inside */
  E.Clip = function (D, track, lx, audio, a, b, name) {
    var lane = D.tracks[track].lane;
    var c = E.el("div", "daw-clip", lane);
    var w = (b - a) * D.pps;
    c.style.left = lx + "px"; c.style.width = w + "px";
    var hd = E.el("div", "hd", c); hd.textContent = name;
    var h = D.rowH - 12 - 4 - 20;
    var cv = E.canvas(c, w - 4, h, D.o.ss || 1);
    cv.style.position = "absolute"; cv.style.left = "0"; cv.style.top = "20px";
    /* drawn on every frame it is visible: a canvas painted while hidden can come back blank */
    c.redraw = function () {
      E.drawWave(E.ctx(cv), { audio: audio, x: 0, y: 2, w: w - 4, h: h - 4, tEnd: b, pps: (w - 4) / (b - a), col: 1.5, fill: "#070c3d", axis: "rgba(7,12,61,0.35)", floor: 0.4 });
    };
    return c;
  };

  /* ---------- page plumbing ---------- */
  E.lang = (new URLSearchParams(location.search).get("lang")) || "en";
  E.ready = function (extraFaces) {
    var faces = ["900 100px Archivo", "800 100px Archivo", "700 100px Archivo", "400 100px Archivo", "600 100px Archivo",
      "500 20px 'Open Sans'", "700 20px 'Open Sans'", "600 20px 'Geist Mono'", "500 20px 'Geist Mono'"].concat(extraFaces || []);
    return Promise.all(faces.map(function (f) { return document.fonts.load(f, f.indexOf("Noto") >= 0 ? "它一直都在聽" : "Ab"); }))
      .then(function () { return document.fonts.ready; })
      .then(function () {
        var out = [];
        document.fonts.forEach(function (f) { if (f.status === "loaded") out.push(f.family.replace(/"/g, "")); });
        document.documentElement.setAttribute("data-fonts-loaded", out.join("|"));
        return out;
      });
  };
})();
