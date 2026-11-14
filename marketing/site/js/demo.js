/* Backtracking landing page: in-browser demo of a retrospective sampler.
 *
 * - Music, started by a user gesture: a 16-bar licensed loop (media/demo-loop.mp3, "Fun Vibe" by Dyalla,
 *   YouTube Audio Library) played with an AudioBufferSourceNode. If that file is absent, cannot be
 *   decoded or takes longer than LOOP_WAIT_MS, an original loop synthesised with the Web Audio API
 *   (createEngine) plays instead. While the licensed loop is the source, Save WAV is turned off and no
 *   file or blob URL of the audio is ever created; Send to track keeps working inside the page.
 * - A capture tap (AudioWorklet, ScriptProcessor fallback) feeds a 60 s stereo ring buffer;
 *   the History setting (10/20/30/60 s) limits how much of it is retained and drawn.
 * - The waveform is drawn like the app: newest audio at the right, filled min/max envelope.
 * - Selecting fixes a copy of the retained audio ("editing material") while capture continues.
 * - A selection can be dragged (pointer events) or sent to a generic DAW track, played back,
 *   or (synthesised music only) saved as a 32-bit float stereo WAV at the AudioContext rate.
 * Page text comes from the JSON block #bt-demo-strings so the same script serves every language.
 */
(function () {
  "use strict";

  var SCRIPT_URL = document.currentScript ? document.currentScript.src : "js/demo.js";

  /* ---------- deterministic waveform shape (decorative idle view and background texture) ---------- */
  function rng(seed) {
    var a = seed >>> 0;
    return function () {
      a = (a + 0x6d2b79f5) | 0;
      var t = Math.imul(a ^ (a >>> 15), 1 | a);
      t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
      return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
    };
  }
  function envelope(n, seed, empty) {
    var r = rng(seed * 9973 + 17), ev = [], x, i, k;
    var beat = 1 / (12 + Math.floor(r() * 6));
    for (x = 0; x < 1.02; x += beat) {
      ev.push({ t: x + 0.002 * r(), a: 0.7 + 0.28 * r(), tau: 0.01 + 0.008 * r() });
      if (r() < 0.85) ev.push({ t: x + beat / 2 + 0.003 * r(), a: 0.38 + 0.3 * r(), tau: 0.005 + 0.004 * r() });
      if (r() < 0.5) ev.push({ t: x + beat * 0.75, a: 0.22 + 0.2 * r(), tau: 0.003 + 0.003 * r() });
    }
    var p1 = r() * 6.28, p2 = r() * 6.28;
    var top = new Float32Array(n), bot = new Float32Array(n);
    for (i = 0; i < n; i++) {
      x = i / (n - 1);
      var s = 0;
      for (k = 0; k < ev.length; k++) {
        if (x >= ev[k].t) {
          var d = (x - ev[k].t) / ev[k].tau;
          if (d < 9) s += ev[k].a * Math.exp(-d);
        }
      }
      var phrase = 0.72 + 0.18 * Math.sin(x * 7.1 + p1) + 0.1 * Math.sin(x * 17.3 + p2);
      s = (s + 0.16 * (0.75 + 0.5 * r())) * phrase;
      var a = Math.tanh(s * 1.15) * 0.96;
      if (x < empty) a = 0;
      else if (x < empty + 0.015) a *= (x - empty) / 0.015;
      top[i] = a * (0.86 + 0.14 * r());
      bot[i] = a * (0.8 + 0.2 * r());
    }
    return { top: top, bot: bot };
  }

  function renderBgWave() {
    var host = document.querySelector("[data-bgwave]");
    if (!host) return;
    var w = 2000, h = 560, env = envelope(Math.round(w / 3), 23, 0), n = env.top.length;
    var mid = h / 2, sc = (h / 2) * 0.94, d = "M0," + mid, i, x;
    for (i = 0; i < n; i++) { x = (i / (n - 1)) * w; d += "L" + x.toFixed(1) + "," + (mid - Math.max(0.5, env.top[i] * sc)).toFixed(1); }
    for (i = n - 1; i >= 0; i--) { x = (i / (n - 1)) * w; d += "L" + x.toFixed(1) + "," + (mid + Math.max(0.5, env.bot[i] * sc)).toFixed(1); }
    host.innerHTML = '<svg viewBox="0 0 ' + w + " " + h + '" preserveAspectRatio="none" focusable="false"><path d="' + d + 'Z" fill="currentColor"/></svg>';
  }

  renderBgWave();

  var root = document.getElementById("demo");
  if (!root) return;

  /* ---------- strings and elements ---------- */
  var S = {};
  try { S = JSON.parse(document.getElementById("bt-demo-strings").textContent); } catch (e) { S = {}; }
  function t(key, vars) {
    var s = S[key] != null ? String(S[key]) : key;
    if (vars) s = s.replace(/\{(\w+)\}/g, function (_, k) { return vars[k] != null ? vars[k] : ""; });
    return s;
  }
  function $(id) { return document.getElementById(id); }
  var el = {
    panel: $("bt-panel"), arm: $("bt-arm"), dot: $("bt-dot"), clear: $("bt-clear"), meter: $("bt-meter"),
    wave: $("bt-wave"), canvas: $("bt-canvas"), dur: $("bt-dur"), tip: $("bt-tip"), prompt: $("bt-prompt"),
    msg: $("bt-msg"), music: $("bt-music"), mute: $("bt-mute"), vol: $("bt-vol"), strip: $("bt-strip"),
    hist: $("bt-hist"), last4: $("bt-last4"), send: $("bt-send"), save: $("bt-save"), fmt: $("bt-fmt"),
    daw: $("bt-daw"), tplay: $("bt-tplay"), ttime: $("bt-ttime"), tclear: $("bt-tclear"), ruler: $("bt-ruler"),
    lane: $("bt-lane"), empty: $("bt-empty"), playhead: $("bt-playhead"), live: $("bt-live"),
    saveNote: $("bt-save-note"), credit: $("bt-credit")
  };
  var steps = root.querySelectorAll(".steps li");

  var ICON = {
    pause: '<svg viewBox="0 0 12 12" aria-hidden="true"><rect x="2.5" y="2" width="2.4" height="8" rx=".6" fill="currentColor"/><rect x="7.1" y="2" width="2.4" height="8" rx=".6" fill="currentColor"/></svg>',
    play: '<svg viewBox="0 0 12 12" aria-hidden="true"><path d="M3.5 2.2v7.6a.5.5 0 0 0 .76.43l6.1-3.8a.5.5 0 0 0 0-.86l-6.1-3.8a.5.5 0 0 0-.76.43Z" fill="currentColor"/></svg>',
    trash: '<svg viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.15" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M2 3.2h8M4.6 3.2V2.1h2.8v1.1M3 3.2l.5 6.9h5l.5-6.9M5 5v3.4M7 5v3.4"/></svg>',
    stop: '<svg viewBox="0 0 12 12" aria-hidden="true"><rect x="2.5" y="2.5" width="7" height="7" fill="currentColor"/></svg>',
    tri: '<svg viewBox="0 0 12 12" aria-hidden="true"><path d="M3 1.5v9l7.5-4.5Z" fill="currentColor"/></svg>',
    x: '<svg viewBox="0 0 12 12" aria-hidden="true"><path d="M2.5 2.5l7 7M9.5 2.5l-7 7" stroke="currentColor" stroke-width="1.8" fill="none"/></svg>',
    spk: '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M2 6h3l4-3v10L5 10H2Z" fill="currentColor"/><path d="M11 5.5a3.5 3.5 0 0 1 0 5M12.8 3.5a6 6 0 0 1 0 9" stroke="currentColor" stroke-width="1.5" fill="none" stroke-linecap="round"/></svg>',
    spkOff: '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M2 6h3l4-3v10L5 10H2Z" fill="currentColor"/><path d="M11 6l4 4M15 6l-4 4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/></svg>'
  };

  /* ---------- state ---------- */
  var MAX_SEC = 60, BIN = 128;
  var BPM = 100, BEAT = 60 / BPM, BAR = BEAT * 4; // synth tempo; the licensed loop sets 122
  /* licensed loop: 16 bars at 122 BPM with 0.5 s of real audio either side (buffer time, seconds) */
  var LOOP_URL = new URL("../media/demo-loop.mp3", SCRIPT_URL).href;
  var LOOP_START = 0.5, LOOP_END = 31.97541, LOOP_BPM = 122, LOOP_GAIN = 1.6, LOOP_WAIT_MS = 4000;
  var musicSrc = null;          // null until decided on the first Play, then "loop" or "synth"
  var sourcePending = false, loopBuf = null, loopNode = null, loopBus = null, loopStartedAt = 0;
  var ctx = null, sr = 48000, cap = 0, binCap = 0;
  var L = null, R = null, bMin = null, bMax = null;
  var total = 0, retained = 0, histSec = 20;
  var armed = true, silentRun = 0, lastAppend = -1e9;
  var meterAcc = 0, meterDb = -Infinity, meterNext = 0, lastFrameT = 0;
  var view = null, sel = null, gest = null, sinceFreeze = 0;
  var started = false, musicOn = false, muted = false;
  var master = null, musicBus = null, tap = null, clipBus = null, engine = null, captureMode = "none";
  var clips = [], clipSeq = 0, trackPlay = null, viewDur = BAR * 7;
  var stepDone = [false, false, false];
  var reduceMotion = window.matchMedia ? window.matchMedia("(prefers-reduced-motion: reduce)") : { matches: false };
  var idleEnv = envelope(420, 19, 0.28);

  function histFrames() { return Math.round((histSec * sr) / BIN) * BIN; }
  function kHz(rate) { var k = rate / 1000; return (Math.round(k * 10) / 10).toString(); }
  function secs(frames) { return (frames / sr).toFixed(2) + " " + t("sec"); }
  function fmtTime(s) {
    s = Math.max(0, s);
    var m = Math.floor(s / 60), r = s - m * 60;
    return m + ":" + (r < 10 ? "0" : "") + r.toFixed(1);
  }
  function announce(text) {
    el.live.textContent = "";
    setTimeout(function () { el.live.textContent = text; }, 30);
  }
  function showMsg(text) { el.msg.textContent = text; el.msg.hidden = !text; }

  /* ---------- audio graph ---------- */
  function ensureAudio() {
    if (ctx) {
      if (ctx.state !== "running") ctx.resume().catch(function () {});
      return ctx;
    }
    var AC = window.AudioContext || window.webkitAudioContext;
    if (!AC) { showMsg(t("noAudio")); return null; }
    try { ctx = new AC({ latencyHint: "interactive" }); } catch (e) { ctx = new AC(); }
    sr = ctx.sampleRate;
    cap = Math.ceil((MAX_SEC * sr) / BIN) * BIN;
    binCap = cap / BIN;
    L = new Float32Array(cap); R = new Float32Array(cap);
    bMin = new Float32Array(binCap); bMax = new Float32Array(binCap);

    master = ctx.createGain();
    master.gain.value = volGain();
    master.connect(ctx.destination);
    musicBus = ctx.createGain();
    var comp = ctx.createDynamicsCompressor();
    comp.threshold.value = -14; comp.knee.value = 8; comp.ratio.value = 3;
    comp.attack.value = 0.004; comp.release.value = 0.2;
    tap = ctx.createGain();
    tap.gain.value = 0.5; // keeps peaks around -6 dBFS at the capture point
    musicBus.connect(comp); comp.connect(tap); tap.connect(master);
    // the licensed loop skips the synth's compressor; LOOP_GAIN brings its -4.4 dBFS peaks to about
    // -6 dBFS at the capture point, the same as the synth
    loopBus = ctx.createGain();
    loopBus.connect(tap);
    clipBus = ctx.createGain();
    clipBus.connect(master);
    engine = createEngine(ctx, musicBus);
    setupCapture();
    el.fmt.textContent = t("fmt", { k: kHz(sr) });
    if (ctx.state !== "running") ctx.resume().catch(function () {});
    return ctx;
  }

  function setupCapture() {
    var sink = ctx.createGain();
    sink.gain.value = 0;
    sink.connect(ctx.destination);
    function fallback() {
      var sp = ctx.createScriptProcessor(2048, 2, 1);
      sp.onaudioprocess = function (e) {
        var ib = e.inputBuffer;
        var l = new Float32Array(ib.getChannelData(0));
        var r = new Float32Array(ib.numberOfChannels > 1 ? ib.getChannelData(1) : ib.getChannelData(0));
        append(l, r);
      };
      tap.connect(sp); sp.connect(sink);
      captureMode = "scriptprocessor";
    }
    if (ctx.audioWorklet && window.AudioWorkletNode) {
      ctx.audioWorklet.addModule(new URL("capture-worklet.js", SCRIPT_URL).href).then(function () {
        var node = new AudioWorkletNode(ctx, "bt-capture", {
          numberOfInputs: 1, numberOfOutputs: 1, outputChannelCount: [1],
          channelCount: 2, channelCountMode: "explicit", channelInterpretation: "speakers"
        });
        node.port.onmessage = function (e) { append(e.data.l, e.data.r); };
        tap.connect(node); node.connect(sink);
        captureMode = "audioworklet";
      }).catch(fallback);
    } else {
      fallback();
    }
  }

  function volGain() { var v = el.vol.value / 100; return muted ? 0 : v * v; }
  function setVolume() { if (master) master.gain.setTargetAtTime(volGain(), ctx.currentTime, 0.02); }

  /* ---------- capture: ring buffer with silence compression ---------- */
  function append(l, r) {
    var n = l.length, hf = histFrames(), maxSilent = Math.round(0.25 * sr), o, i;
    for (o = 0; o + BIN <= n; o += BIN) {
      var mn = 1e9, mx = -1e9;
      for (i = o; i < o + BIN; i++) {
        var a = l[i], b = r[i];
        if (a < mn) mn = a; if (a > mx) mx = a;
        if (b < mn) mn = b; if (b > mx) mx = b;
      }
      var pk = mx > -mn ? mx : -mn;
      if (pk > meterAcc) meterAcc = pk;
      silentRun = pk < 3.2e-4 ? silentRun + BIN : 0;
      if (!armed || silentRun > maxSilent) continue; // paused, or long silence is not retained
      var pos = total % cap;
      L.set(l.subarray(o, o + BIN), pos);
      R.set(r.subarray(o, o + BIN), pos);
      var bi = (total / BIN) % binCap;
      bMin[bi] = mn; bMax[bi] = mx;
      total += BIN;
      retained = Math.min(retained + BIN, hf);
      if (view) sinceFreeze += BIN;
      lastAppend = performance.now();
    }
  }

  function copyRing(src, absStart, n, dst) {
    var c = src.length, p = absStart % c, first = Math.min(n, c - p);
    dst.set(src.subarray(p, p + first), 0);
    if (first < n) dst.set(src.subarray(0, n - first), first);
  }

  /* fix the editing material: a copy of what is retained now; capture continues into the ring */
  function freeze() {
    if (view) return;
    var n = retained, start = total - n, nb = n / BIN;
    var v = {
      start: start, end: total, hist: histFrames(), sb: start / BIN,
      L: new Float32Array(n), R: new Float32Array(n), mn: new Float32Array(nb), mx: new Float32Array(nb)
    };
    copyRing(L, start, n, v.L); copyRing(R, start, n, v.R);
    copyRing(bMin, start / BIN, nb, v.mn); copyRing(bMax, start / BIN, nb, v.mx);
    view = v;
    sinceFreeze = 0;
  }

  function source() {
    if (view) return { end: view.end, hist: view.hist, start: view.start, sb: view.sb, n: view.mn.length, mn: view.mn, mx: view.mx, frozen: true };
    return { end: total, hist: histFrames(), start: total - retained, frozen: false };
  }

  /* ---------- drawing ---------- */
  var g2 = el.canvas.getContext("2d");
  var cssW = 300, cssH = 100, dpr = 1;

  function resizeCanvas() {
    var r = el.canvas.getBoundingClientRect();
    dpr = Math.min(window.devicePixelRatio || 1, 2.5);
    cssW = Math.max(10, r.width); cssH = Math.max(10, r.height);
    el.canvas.width = Math.round(cssW * dpr);
    el.canvas.height = Math.round(cssH * dpr);
    draw();
    updateSelUI();
  }

  function xOf(s, f) { return cssW - ((s.end - f) / s.hist) * cssW; }

  function envPath(xs, tops, bots, mid, half) {
    var p = new Path2D(), i, m = xs.length, fl = 0.5;
    p.moveTo(xs[0], mid - Math.max(fl, tops[0] * half));
    for (i = 1; i < m; i++) p.lineTo(xs[i], mid - Math.max(fl, tops[i] * half));
    for (i = m - 1; i >= 0; i--) p.lineTo(xs[i], mid + Math.max(fl, -bots[i] * half));
    p.closePath();
    return p;
  }

  function draw() {
    var g = g2, W = cssW, H = cssH, mid = H / 2, half = (H / 2) * 0.94;
    g.setTransform(dpr, 0, 0, dpr, 0, 0);
    g.clearRect(0, 0, W, H);
    g.fillStyle = "#3d4247";
    g.fillRect(0, Math.round(mid * dpr) / dpr - 0.5, W, 1);

    if (!started || !L) {
      // decorative idle waveform until sound starts
      var n = idleEnv.top.length, xs = [], tp = [], bt = [], i;
      for (i = 0; i < n; i++) { xs.push((i / (n - 1)) * W); tp.push(idleEnv.top[i] * 0.85); bt.push(-idleEnv.bot[i] * 0.85); }
      g.globalAlpha = 0.42;
      g.fillStyle = "#6fe8b0";
      g.fill(envPath(xs, tp, bt, mid, half));
      g.globalAlpha = 1;
      return;
    }

    var s = source();
    if (gest && gest.snap) {
      g.fillStyle = "rgba(255,255,255,0.13)";
      for (var k = 1; ; k++) { var gx = xOf(s, s.end - k * sr); if (gx < 0) break; g.fillRect(Math.round(gx), 0, 1, H); }
    }
    if (s.end - s.start <= 0) return;

    var cf = BIN * Math.max(1, Math.floor(s.hist / W / BIN)), bpc = cf / BIN;
    var lo = Math.max(s.start, s.end - s.hist);
    var k0 = Math.floor(lo / cf), k1 = Math.floor((s.end - 1) / cf);
    var X = [], T = [], B = [];
    for (var c = k0; c <= k1; c++) {
      var mn = 1e9, mx = -1e9;
      for (var b = c * bpc, be = b + bpc; b < be; b++) {
        var f = b * BIN, idx;
        if (f < lo || f >= s.end) continue;
        if (s.frozen) { idx = b - s.sb; if (idx < 0 || idx >= s.n) continue; if (s.mn[idx] < mn) mn = s.mn[idx]; if (s.mx[idx] > mx) mx = s.mx[idx]; }
        else { idx = b % binCap; if (bMin[idx] < mn) mn = bMin[idx]; if (bMax[idx] > mx) mx = bMax[idx]; }
      }
      if (mn > mx) continue;
      var f0 = Math.max(c * cf, lo), f1 = Math.min((c + 1) * cf, s.end);
      X.push(xOf(s, (f0 + f1) / 2)); T.push(Math.min(1, mx)); B.push(Math.max(-1, mn));
    }
    if (!X.length) return;
    if (X.length === 1) { X.unshift(X[0] - 1); T.unshift(T[0]); B.unshift(B[0]); }
    var path = envPath(X, T, B, mid, half);
    g.lineJoin = "round";
    g.lineWidth = 0.6;
    g.fillStyle = "#6fe8b0"; g.strokeStyle = "#9af2c9";
    g.fill(path); g.stroke(path);
    if (sel) {
      var xa = xOf(s, sel.a), xb = xOf(s, sel.b);
      g.fillStyle = "rgba(255,79,154,0.1)";
      g.fillRect(xa, 0, xb - xa, H);
      g.save();
      g.beginPath(); g.rect(xa, 0, Math.max(1, xb - xa), H); g.clip();
      g.fillStyle = "#c21f6c"; g.strokeStyle = "#ff4f9a";
      g.fill(path); g.stroke(path);
      g.restore();
    }
  }

  /* ---------- animation loop ---------- */
  var rafOn = false;
  function kick() { if (!rafOn) { rafOn = true; requestAnimationFrame(frame); } }
  function frame(now) {
    rafOn = false;
    draw();
    updateMeter(now);
    updateDot(now);
    updatePlayhead();
    updateButtons();
    if (started) kick();
  }
  document.addEventListener("visibilitychange", function () { if (!document.hidden) kick(); });

  function updateMeter(now) {
    var dt = Math.min(0.25, Math.max(0, (now - lastFrameT) / 1000));
    lastFrameT = now;
    var db = meterAcc > 0 ? 20 * Math.log(meterAcc) / Math.LN10 : -Infinity;
    meterAcc = 0;
    meterDb = Math.max(db, meterDb - 24 * dt);
    if (now < meterNext) return;
    meterNext = now + 110;
    var txt;
    if (!started || !armed || !isFinite(meterDb) || meterDb < -90) txt = "—";
    else txt = (meterDb < 0 ? "−" : "") + Math.abs(meterDb).toFixed(1);
    var b = el.meter.firstChild;
    if (b.textContent !== txt) b.textContent = txt;
  }

  var dotState = "";
  function updateDot(now) {
    var st = !started ? "idle" : !armed ? "paused" : now - lastAppend < 400 ? "rec" : "wait";
    if (st === dotState) return;
    dotState = st;
    el.dot.className = "btp-dot" + (st === "rec" ? " rec" : st === "wait" ? " wait" : "");
    el.dot.title = t(st);
    el.dot.setAttribute("aria-label", t(st));
    el.dot.setAttribute("role", "img");
  }

  /* ---------- selection ---------- */
  function frameAt(clientX) {
    var r = el.canvas.getBoundingClientRect(), s = source();
    var f = s.end - (1 - (clientX - r.left) / r.width) * s.hist;
    return Math.round(Math.min(s.end, Math.max(Math.max(s.start, s.end - s.hist), f)));
  }
  function snapF(f) {
    var s = source(), lo = Math.max(s.start, s.end - s.hist);
    var k = Math.round((s.end - f) / sr);
    if (s.end - k * sr < lo) k = Math.floor((s.end - lo) / sr); // stay on a whole second inside the audio
    return s.end - Math.max(0, k) * sr;
  }

  function updateSelUI() {
    var has = !!sel && started;
    el.dur.hidden = !has;
    el.tip.hidden = !has || !!(gest && gest.mode === "out" && gest.moved);
    if (has) {
      var s = source();
      el.dur.textContent = secs(sel.b - sel.a);
      var cx = (xOf(s, sel.a) + xOf(s, sel.b)) / 2;
      var tw = el.tip.offsetWidth || 90;
      el.tip.style.left = Math.min(cssW - tw / 2, Math.max(tw / 2, cx)) + "px";
    }
    updateButtons();
  }
  function updateButtons() {
    var any = !!(started && (sel || retained > 0));
    var last = !(started && retained > 0);
    var canSave = any && musicSrc === "synth";
    if (el.last4.disabled !== last) el.last4.disabled = last;
    if (el.send.disabled === any) el.send.disabled = !any;
    if (el.save.disabled === canSave) el.save.disabled = !canSave;
  }

  function cancelSel(say) {
    var had = !!sel;
    view = null; sel = null; gest = null;
    el.wave.classList.remove("over-sel", "dragging");
    updateSelUI(); kick();
    if (say && had) announce(t("cancelled"));
  }

  function selectLast(sec) {
    if (!started || retained <= 0) return;
    freeze();
    var len = Math.min(Math.round(sec * sr), view.end - view.start);
    sel = { a: view.end - len, b: view.end };
    selected();
  }
  function selected() {
    markStep(1);
    updateSelUI(); kick();
    announce(t("selected", { s: ((sel.b - sel.a) / sr).toFixed(2) }));
  }

  function nudge(dir, startOnly) {
    if (!sel || !view) return;
    var d = Math.round(0.5 * sr) * dir, lo = Math.max(view.start, view.end - view.hist), hi = view.end;
    if (startOnly) {
      sel.a = Math.min(sel.b - Math.round(0.1 * sr), Math.max(lo, sel.a + d));
    } else {
      var len = sel.b - sel.a, a = Math.min(hi - len, Math.max(lo, sel.a + d));
      sel = { a: a, b: a + len };
    }
    updateSelUI(); kick();
    announce(t("selected", { s: ((sel.b - sel.a) / sr).toFixed(2) }));
  }

  /* material for export: the selection, or everything retained (as the app does with no selection) */
  function exportAudio() {
    var l, r, n;
    if (sel && view) {
      var a = Math.round(sel.a - view.start), b = Math.round(sel.b - view.start);
      l = view.L.slice(a, b); r = view.R.slice(a, b); n = b - a;
    } else {
      n = retained; l = new Float32Array(n); r = new Float32Array(n);
      copyRing(L, total - n, n, l); copyRing(R, total - n, n, r);
    }
    return { l: l, r: r, n: n };
  }
  function fileName() { return "Backtracking-" + Date.now() + ".wav"; }

  /* ---------- pointer interaction on the waveform ---------- */
  var ghost = null;

  el.wave.addEventListener("pointerdown", function (e) {
    if (!started || e.button !== 0 || e.target.closest("button")) return;
    if (retained <= 0 && !view) return;
    e.preventDefault();
    el.wave.setPointerCapture(e.pointerId);
    var f = frameAt(e.clientX);
    if (sel && f >= sel.a && f <= sel.b) {
      gest = { mode: "out", id: e.pointerId, x0: e.clientX, y0: e.clientY, moved: false };
    } else {
      freeze();
      gest = { mode: "sel", id: e.pointerId, x0: e.clientX, anchor: frameAt(e.clientX), moved: false, had: !!sel, snap: e.shiftKey };
    }
  });

  el.wave.addEventListener("pointermove", function (e) {
    if (!gest || e.pointerId !== gest.id) {
      if (sel && started && e.pointerType === "mouse") {
        var f = frameAt(e.clientX);
        el.wave.classList.toggle("over-sel", f >= sel.a && f <= sel.b);
      }
      return;
    }
    if (gest.mode === "sel") {
      if (!gest.moved && Math.abs(e.clientX - gest.x0) < 4) return;
      gest.moved = true;
      gest.snap = e.shiftKey;
      var a = gest.anchor, b = frameAt(e.clientX);
      if (gest.snap) { a = snapF(a); b = snapF(b); }
      sel = { a: Math.min(a, b), b: Math.max(a, b) };
      updateSelUI(); kick();
    } else {
      if (!gest.moved && Math.hypot(e.clientX - gest.x0, e.clientY - gest.y0) < 6) return;
      if (!gest.moved) { gest.moved = true; startGhost(e.clientX, e.clientY); el.wave.classList.add("dragging"); updateSelUI(); }
      moveGhost(e.clientX, e.clientY);
    }
  });

  function endGesture(e, cancelled) {
    if (!gest || e.pointerId !== gest.id) return;
    var g = gest;
    if (g.mode === "sel") {
      gest = null;
      if (cancelled || !g.moved) { cancelSel(g.had); }
      else if (!sel || sel.b - sel.a < 0.05 * sr) { cancelSel(true); }
      else { selected(); }
    } else {
      gest = null;
      el.wave.classList.remove("dragging");
      if (!g.moved) { if (!cancelled) cancelSel(true); }
      else if (!cancelled && overLane(e.clientX, e.clientY)) { dropGhost(); addClip(laneTime(e.clientX)); }
      else { rejectGhost(); announce(t("rejected")); }
      updateSelUI();
    }
    kick();
  }
  el.wave.addEventListener("pointerup", function (e) { endGesture(e, false); });
  el.wave.addEventListener("pointercancel", function (e) { endGesture(e, true); });
  el.wave.addEventListener("lostpointercapture", function (e) { if (gest && e.pointerId === gest.id) endGesture(e, gest.mode === "sel" ? !gest.moved : false); });
  el.wave.addEventListener("pointerleave", function () { el.wave.classList.remove("over-sel"); });

  function startGhost(x, y) {
    var n = sel ? sel.b - sel.a : retained;
    ghost = document.createElement("div");
    ghost.className = "ghost";
    ghost.setAttribute("aria-hidden", "true");
    ghost.innerHTML = '<svg class="streaks" viewBox="0 0 120 34"><line x1="4" y1="4" x2="116" y2="4"/><line x1="44" y1="17" x2="116" y2="17" opacity=".7"/><line x1="78" y1="30" x2="116" y2="30" opacity=".45"/></svg>' +
      '<div class="ic">WAV</div><div><div class="n"></div><div class="m"></div></div>';
    ghost.querySelector(".n").textContent = fileName();
    ghost.querySelector(".m").textContent = secs(n) + " · " + kHz(sr) + " kHz · 32-bit float";
    if (reduceMotion.matches) ghost.querySelector(".streaks").style.display = "none";
    ghost._x0 = x; ghost._y0 = y;
    document.body.appendChild(ghost);
    moveGhost(x, y);
  }
  function moveGhost(x, y) {
    if (!ghost) return;
    ghost.style.transform = "translate(" + (x - 16) + "px," + (y - 22) + "px)";
    el.lane.classList.toggle("drop-ok", overLane(x, y));
  }
  function dropGhost() { if (ghost) { ghost.remove(); ghost = null; } el.lane.classList.remove("drop-ok"); }
  function rejectGhost() {
    el.lane.classList.remove("drop-ok");
    if (!ghost) return;
    var gh = ghost; ghost = null;
    if (reduceMotion.matches) { gh.remove(); return; }
    gh.classList.add("back");
    gh.style.transform = "translate(" + (gh._x0 - 16) + "px," + (gh._y0 - 22) + "px)";
    setTimeout(function () { gh.remove(); }, 260);
  }
  function overLane(x, y) {
    var r = el.lane.getBoundingClientRect();
    return x >= r.left && x <= r.right && y >= r.top - 8 && y <= r.bottom + 8;
  }
  function laneTime(x) {
    var r = el.lane.getBoundingClientRect();
    var tt = ((x - r.left - 16) / r.width) * viewDur;
    return Math.max(0, Math.round(tt / BEAT) * BEAT);
  }

  /* ---------- keyboard ---------- */
  el.wave.addEventListener("keydown", function (e) {
    if (!started) {
      if (e.key === "Enter" || e.key === " ") { e.preventDefault(); startMusic(); }
      return;
    }
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      if (!sel) selectLast(4); else sendToTrack();
    } else if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
      if (!sel) return;
      e.preventDefault();
      nudge(e.key === "ArrowLeft" ? -1 : 1, e.shiftKey);
    }
  });
  document.addEventListener("keydown", function (e) {
    if (e.key === "Escape" || e.key === "Esc") {
      if (gest && gest.mode === "out" && gest.moved) { gest = null; rejectGhost(); el.wave.classList.remove("dragging"); updateSelUI(); return; }
      if (view || sel) cancelSel(true);
    } else if (e.key === "Backspace") {
      var tag = (e.target && e.target.tagName) || "";
      if (/^(TEXTAREA|SELECT)$/.test(tag) || (tag === "INPUT" && e.target.type !== "range") || (e.target && e.target.isContentEditable)) return;
      if (view || sel) { e.preventDefault(); cancelSel(true); }
    }
  });
  // a click anywhere outside the demo cancels the selection
  document.addEventListener("pointerdown", function (e) {
    if ((view || sel) && !root.contains(e.target)) cancelSel(true);
  }, true);

  /* ---------- steps ---------- */
  function markStep(i) {
    if (stepDone[i]) return;
    stepDone[i] = true;
    for (var k = 0; k < steps.length; k++) {
      steps[k].classList.toggle("done", stepDone[k]);
      steps[k].querySelector(".step-state").textContent = stepDone[k] ? " (" + t("done") + ")" : "";
    }
    var next = stepDone.indexOf(false);
    for (k = 0; k < steps.length; k++) steps[k].classList.toggle("now", k === next);
  }

  /* ---------- toolbar and controls ---------- */
  function setArmUI() {
    el.arm.innerHTML = armed ? ICON.pause : ICON.play;
    var label = armed ? t("pause") : t("start");
    el.arm.setAttribute("aria-label", label);
    el.arm.title = label;
  }
  el.arm.addEventListener("click", function () { armed = !armed; setArmUI(); kick(); });
  el.clear.innerHTML = ICON.trash;
  el.clear.addEventListener("click", function () {
    retained = 0;
    cancelSel(false);
    announce(t("cleared"));
    kick();
  });

  function setMusicUI() {
    el.music.setAttribute("aria-pressed", musicOn ? "true" : "false");
    el.music.innerHTML = (musicOn ? ICON.stop : ICON.tri) + "<span></span>";
    el.music.lastChild.textContent = musicOn ? t("stop") : t("play");
  }
  function startMusic() {
    if (!ensureAudio()) return;
    if (!started) {
      started = true;
      el.prompt.hidden = true;
      kick();
    }
    musicOn = true;
    setMusicUI();
    markStep(0);
    if (musicSrc) playSource();
    else if (!sourcePending) chooseSource();
    setTimeout(function () {
      if (ctx && ctx.state !== "running") showMsg(t("blocked")); else showMsg("");
    }, 700);
  }
  function stopMusic() {
    if (loopNode) { try { loopNode.stop(); } catch (e) { /* not started */ } loopNode.disconnect(); loopNode = null; }
    if (engine) engine.stop();
    musicOn = false; setMusicUI();
  }
  function playSource() {
    if (musicSrc === "loop") {
      if (loopNode) return;
      loopNode = ctx.createBufferSource();
      loopNode.buffer = loopBuf;
      loopNode.loop = true;
      loopNode.loopStart = LOOP_START;
      loopNode.loopEnd = LOOP_END;
      var g = ctx.createGain();
      g.gain.value = LOOP_GAIN;
      loopNode.connect(g); g.connect(loopBus);
      loopStartedAt = ctx.currentTime + 0.03;
      loopNode.start(loopStartedAt, LOOP_START); // the first thing heard is the downbeat
    } else {
      engine.start();
    }
  }

  /* First Play: try the licensed loop, fall back to the synth on 404, decode error or timeout. */
  function chooseSource() {
    sourcePending = true;
    var done = false, ctl = window.AbortController ? new AbortController() : null;
    function finish(mode, buf) {
      if (done) return;
      done = true;
      sourcePending = false;
      musicSrc = mode; loopBuf = buf || null;
      if (mode === "loop") setTempo(LOOP_BPM);
      setSourceUI();
      if (musicOn) playSource();
    }
    var timer = setTimeout(function () { if (ctl) ctl.abort(); finish("synth"); }, LOOP_WAIT_MS);
    var fail = function () { clearTimeout(timer); finish("synth"); };
    if (!window.fetch) { fail(); return; }
    fetch(LOOP_URL, ctl ? { signal: ctl.signal } : undefined).then(function (res) {
      if (!res.ok) throw new Error("no loop");
      return res.arrayBuffer();
    }).then(function (data) {
      return new Promise(function (ok, no) {
        var p = ctx.decodeAudioData(data, ok, no); // callback form for older Safari
        if (p && p.catch) p.catch(no);
      });
    }).then(function (buf) {
      if (!buf || buf.duration < LOOP_END) throw new Error("short loop");
      clearTimeout(timer);
      finish("loop", smoothSeam(buf));
    }).catch(fail); // expected when the file is not deployed: the browser logs the 404 itself
  }
  /* The audio just before loopEnd does not continue into loopStart (the song is louder there than
     in the beat before the drop), so the wrap would step by up to ~0.03. Crossfade the last
     SEAM_SEC before loopEnd into the real audio that precedes loopStart (the 0.5 s pre-roll):
     at the wrap the signal is then already the pre-roll, which continues sample for sample. */
  var SEAM_SEC = 0.008;
  function smoothSeam(buf) {
    var rate = buf.sampleRate, e = Math.round(LOOP_END * rate), s = Math.round(LOOP_START * rate);
    var n = Math.min(Math.round(SEAM_SEC * rate), s);
    for (var c = 0; c < buf.numberOfChannels; c++) {
      var d = buf.getChannelData(c);
      for (var i = 0; i < n; i++) {
        var g = (i + 1) / n, a = Math.cos(g * Math.PI / 2), b = Math.sin(g * Math.PI / 2); // equal power
        d[e - n + i] = d[e - n + i] * a + d[s - n + i] * b;
      }
    }
    return buf;
  }
  function setSourceUI() {
    var lic = musicSrc === "loop";
    el.saveNote.hidden = !lic;
    el.credit.hidden = !lic;
    if (lic) {
      el.save.setAttribute("aria-disabled", "true");
      el.save.setAttribute("aria-describedby", "bt-save-note");
    } else {
      el.save.removeAttribute("aria-disabled");
      el.save.removeAttribute("aria-describedby");
    }
    updateButtons();
  }
  function setTempo(bpm) {
    BPM = bpm; BEAT = 60 / BPM; BAR = BEAT * 4;
    layoutTrack();
  }
  el.prompt.addEventListener("click", startMusic);
  el.music.addEventListener("click", function () { if (musicOn) stopMusic(); else startMusic(); });

  function setMuteUI() {
    el.mute.innerHTML = muted ? ICON.spkOff : ICON.spk;
    el.mute.setAttribute("aria-pressed", muted ? "true" : "false");
    el.mute.setAttribute("aria-label", muted ? t("unmute") : t("mute"));
    el.mute.title = muted ? t("unmute") : t("mute");
  }
  el.mute.addEventListener("click", function () { muted = !muted; setMuteUI(); setVolume(); });
  el.vol.addEventListener("input", function () { if (muted && el.vol.value > 0) { muted = false; setMuteUI(); } setVolume(); });

  el.strip.addEventListener("click", function () {
    var on = !el.panel.classList.contains("strip");
    el.panel.classList.toggle("strip", on);
    el.strip.setAttribute("aria-pressed", on ? "true" : "false");
  });

  var radios = el.hist.querySelectorAll("button[data-sec]");
  function setHist(sec) {
    histSec = sec;
    for (var i = 0; i < radios.length; i++) {
      var on = +radios[i].getAttribute("data-sec") === sec;
      radios[i].setAttribute("aria-checked", on ? "true" : "false");
      radios[i].tabIndex = on ? 0 : -1;
    }
    if (L) retained = Math.min(retained, histFrames()); // shortening trims the oldest live audio
    updateSelUI(); kick();
  }
  for (var ri = 0; ri < radios.length; ri++) {
    radios[ri].addEventListener("click", function () { setHist(+this.getAttribute("data-sec")); });
    radios[ri].addEventListener("keydown", function (e) {
      var keys = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 };
      if (!keys[e.key]) return;
      e.preventDefault();
      var arr = Array.prototype.slice.call(radios), i = arr.indexOf(this);
      var nx = arr[(i + keys[e.key] + arr.length) % arr.length];
      setHist(+nx.getAttribute("data-sec")); nx.focus();
    });
  }

  el.last4.addEventListener("click", function () { selectLast(4); });
  el.send.addEventListener("click", sendToTrack);
  el.save.addEventListener("click", saveWav);

  /* ---------- WAV ---------- */
  function encodeWav(l, r, rate) {
    var n = l.length, dataBytes = n * 8;
    var h = new ArrayBuffer(58), v = new DataView(h);
    function str(o, s) { for (var i = 0; i < s.length; i++) v.setUint8(o + i, s.charCodeAt(i)); }
    str(0, "RIFF"); v.setUint32(4, 50 + dataBytes, true); str(8, "WAVE");
    str(12, "fmt "); v.setUint32(16, 18, true);
    v.setUint16(20, 3, true);            // WAVE_FORMAT_IEEE_FLOAT
    v.setUint16(22, 2, true);            // stereo
    v.setUint32(24, rate, true);
    v.setUint32(28, rate * 8, true);     // byte rate
    v.setUint16(32, 8, true);            // block align
    v.setUint16(34, 32, true);           // bits per sample
    v.setUint16(36, 0, true);            // cbSize
    str(38, "fact"); v.setUint32(42, 4, true); v.setUint32(46, n, true);
    str(50, "data"); v.setUint32(54, dataBytes, true);
    var data = new Float32Array(n * 2);
    for (var i = 0, j = 0; i < n; i++) { data[j++] = l[i]; data[j++] = r[i]; }
    return new Blob([h, data.buffer], { type: "audio/wav" });
  }
  function saveWav() {
    if (!started || musicSrc !== "synth") return; // never hand out the licensed music as a file
    var a = exportAudio();
    if (!a.n) return;
    var name = fileName(), blob = encodeWav(a.l, a.r, sr), url = URL.createObjectURL(blob);
    var link = document.createElement("a");
    link.href = url; link.download = name; link.style.display = "none";
    document.body.appendChild(link); link.click(); link.remove();
    setTimeout(function () { URL.revokeObjectURL(url); }, 15000);
    markStep(2);
    announce(t("saved", { name: name }));
  }

  /* ---------- DAW track ---------- */
  function sendToTrack() {
    if (!started) return;
    var end = 0;
    clips.forEach(function (c) { end = Math.max(end, c.start + c.dur); });
    addClip(clips.length ? Math.ceil(end / BAR - 1e-6) * BAR : 0);
  }

  function addClip(startSec) {
    var a = exportAudio();
    if (!a.n || !ctx) return;
    var buf = ctx.createBuffer(2, a.n, sr);
    buf.getChannelData(0).set(a.l); buf.getChannelData(1).set(a.r);
    var c = { id: ++clipSeq, buf: buf, l: a.l, r: a.r, start: startSec, dur: a.n / sr, name: fileName() };
    var node = document.createElement("div");
    node.className = "clip" + (reduceMotion.matches ? "" : " landed");
    node.setAttribute("role", "listitem");
    node.innerHTML = '<div class="clip-top"><button type="button" class="cp"></button><span class="clip-name"></span><button type="button" class="cx">' + ICON.x + '</button></div><canvas aria-hidden="true"></canvas>';
    node.querySelector(".clip-name").textContent = c.name + " · " + secs(a.n);
    var label = t("clip") + " " + c.id + ", " + secs(a.n);
    node.setAttribute("aria-label", label);
    var bx = node.querySelector(".cx");
    bx.setAttribute("aria-label", t("removeClip") + " " + c.id); bx.title = t("removeClip");
    var bp = node.querySelector(".cp");
    c.el = node; c.btn = bp;
    setClipBtn(c, false);
    bp.addEventListener("click", function () { if (trackPlay && trackPlay.clip === c) stopTrack(); else playTrack(c); });
    bx.addEventListener("click", function () { removeClip(c); });
    clips.push(c);
    if (clips.length > 8) removeClip(clips[0]);
    el.lane.appendChild(node);
    layoutTrack();
    markStep(2);
    announce(t("added", { t: fmtTime(startSec) }));
    cancelSel(false); // finishing an export returns to live view
  }
  function setClipBtn(c, playing) {
    c.btn.innerHTML = playing ? ICON.stop : ICON.tri;
    var l = (playing ? t("stopClip") : t("playClip")) + " " + c.id;
    c.btn.setAttribute("aria-label", l); c.btn.title = l;
  }
  function removeClip(c) {
    if (trackPlay) stopTrack();
    var i = clips.indexOf(c);
    if (i >= 0) clips.splice(i, 1);
    c.el.remove();
    layoutTrack();
  }

  function layoutTrack() {
    var end = 0;
    clips.forEach(function (c) { end = Math.max(end, c.start + c.dur); });
    var bars = Math.max(7, Math.ceil((end + 0.5) / BAR));
    viewDur = bars * BAR;
    var step = bars > 24 ? 4 : bars > 12 ? 2 : 1, html = "", grid = "", i;
    for (i = 0; i < bars; i += step) html += '<span style="left:' + ((i * BAR) / viewDur * 100).toFixed(3) + '%">' + (i + 1) + "</span>";
    el.ruler.innerHTML = html;
    var beatsVisible = bars * 4 <= 64;
    for (i = 1; i < bars * 4; i++) {
      if (i % 4 && !beatsVisible) continue;
      grid += '<i class="gridline' + (i % 4 ? "" : " bar") + '" style="left:' + ((i * BEAT) / viewDur * 100).toFixed(3) + '%"></i>';
    }
    var old = el.lane.querySelectorAll(".gridline");
    for (i = 0; i < old.length; i++) old[i].remove();
    el.lane.insertAdjacentHTML("afterbegin", grid);
    clips.forEach(function (c) {
      c.el.style.left = (c.start / viewDur * 100) + "%";
      c.el.style.width = (c.dur / viewDur * 100) + "%";
    });
    el.empty.hidden = clips.length > 0;
    el.tplay.disabled = !clips.length;
    el.tclear.disabled = !clips.length;
    requestAnimationFrame(drawClips);
  }
  function drawClips() {
    var d = Math.min(window.devicePixelRatio || 1, 2.5);
    clips.forEach(function (c) {
      var cv = c.el.querySelector("canvas"), w = cv.clientWidth, h = cv.clientHeight;
      if (!w || !h) return;
      cv.width = Math.round(w * d); cv.height = Math.round(h * d);
      var g = cv.getContext("2d"), cols = Math.max(1, Math.round(w)), per = c.l.length / cols, mid = h / 2;
      g.setTransform(d, 0, 0, d, 0, 0);
      g.fillStyle = "rgba(7,12,61,0.82)";
      g.beginPath();
      var top = [], bot = [], i, j;
      for (i = 0; i < cols; i++) {
        var a = Math.floor(i * per), b = Math.max(a + 1, Math.floor((i + 1) * per)), mn = 0, mx = 0;
        for (j = a; j < b && j < c.l.length; j += 2) {
          var x = c.l[j], y = c.r[j];
          if (x < mn) mn = x; if (x > mx) mx = x; if (y < mn) mn = y; if (y > mx) mx = y;
        }
        top.push(mx); bot.push(mn);
      }
      g.moveTo(0, mid);
      for (i = 0; i < cols; i++) g.lineTo(i + 0.5, mid - Math.max(0.5, top[i] * mid * 0.92));
      for (i = cols - 1; i >= 0; i--) g.lineTo(i + 0.5, mid + Math.max(0.5, -bot[i] * mid * 0.92));
      g.closePath(); g.fill();
    });
  }
  if (window.ResizeObserver) {
    new ResizeObserver(function () { requestAnimationFrame(drawClips); }).observe(el.lane);
    new ResizeObserver(resizeCanvas).observe(el.canvas);
  } else {
    window.addEventListener("resize", function () { resizeCanvas(); drawClips(); });
  }

  function duck(on) {
    if (!musicBus) return;
    musicBus.gain.setTargetAtTime(on ? 0 : 1, ctx.currentTime, on ? 0.03 : 0.12);
    loopBus.gain.setTargetAtTime(on ? 0 : 1, ctx.currentTime, on ? 0.03 : 0.12);
  }
  function setTrackBtn(playing) {
    el.tplay.innerHTML = (playing ? ICON.stop : ICON.tri) + "<span></span>";
    el.tplay.lastChild.textContent = playing ? t("stopTrack") : t("playTrack");
  }
  function playTrack(only) {
    if (!ensureAudio()) return;
    stopTrack();
    var list = only ? [only] : clips.slice();
    if (!list.length) return;
    var offset = only ? only.start : 0, t0 = ctx.currentTime + 0.06, end = 0;
    var nodes = list.map(function (c) {
      var src = ctx.createBufferSource();
      src.buffer = c.buf; src.connect(clipBus);
      src.start(t0 + c.start - offset);
      end = Math.max(end, c.start + c.dur);
      c.el.classList.add("playing");
      return src;
    });
    duck(true);
    trackPlay = { t0: t0, offset: offset, end: end, nodes: nodes, clip: only || null, list: list };
    if (only) setClipBtn(only, true);
    setTrackBtn(!only);
    el.daw.classList.add("playing");
    kick();
  }
  function stopTrack() {
    if (!trackPlay) return;
    trackPlay.nodes.forEach(function (n) { try { n.stop(); } catch (e) { /* already stopped */ } n.disconnect(); });
    trackPlay.list.forEach(function (c) { c.el.classList.remove("playing"); });
    if (trackPlay.clip) setClipBtn(trackPlay.clip, false);
    trackPlay = null;
    duck(false);
    setTrackBtn(false);
    el.daw.classList.remove("playing");
    el.ttime.textContent = fmtTime(0);
  }
  function updatePlayhead() {
    if (!trackPlay || !ctx) return;
    var pos = ctx.currentTime - trackPlay.t0 + trackPlay.offset;
    if (pos > trackPlay.end + 0.05) { stopTrack(); return; }
    var w = el.lane.clientWidth;
    el.playhead.style.transform = "translateX(" + (Math.max(0, pos) / viewDur * w).toFixed(1) + "px)";
    el.ttime.textContent = fmtTime(pos);
  }
  el.tplay.addEventListener("click", function () { if (trackPlay && !trackPlay.clip) stopTrack(); else playTrack(null); });
  el.tclear.addEventListener("click", function () {
    stopTrack();
    clips.slice().forEach(removeClip);
  });

  /* ---------- fallback music: an original loop, synthesised on the page ---------- */
  function createEngine(ac, out) {
    var S16 = BEAT / 4;
    var noise = ac.createBuffer(1, ac.sampleRate, ac.sampleRate), nd = noise.getChannelData(0), nr = rng(7);
    for (var i = 0; i < nd.length; i++) nd[i] = nr() * 2 - 1;
    var hasPan = typeof ac.createStereoPanner === "function";

    var delay = ac.createDelay(1.5), fb = ac.createGain(), dlp = ac.createBiquadFilter(), wet = ac.createGain();
    delay.delayTime.value = S16 * 3;
    fb.gain.value = 0.3; dlp.type = "lowpass"; dlp.frequency.value = 2600; wet.gain.value = 0.3;
    delay.connect(dlp); dlp.connect(fb); fb.connect(delay); dlp.connect(wet); wet.connect(out);

    // A minor: Am9, Fmaj9, Cmaj7, G6 | Am9, Fmaj9, Dm9, E7sus4
    var CH = [
      { bass: 45, v: [60, 64, 67, 71] }, { bass: 41, v: [57, 60, 64, 67] },
      { bass: 48, v: [55, 59, 64, 67] }, { bass: 43, v: [59, 62, 64, 67] },
      { bass: 45, v: [60, 64, 67, 71] }, { bass: 41, v: [57, 60, 64, 67] },
      { bass: 38, v: [57, 60, 64, 65] }, { bass: 40, v: [57, 59, 62, 64] }
    ];
    var P = {
      kickA: "x.....x...x.....", kickB: "x.....x.x.x...x.", kickBuild: "x...x...x...x...",
      snare: "....x.......x...", hatA: "x.x.x.x.x.x.x.x.", hatB: "xxxxxxxxxxxxxxxx",
      bassA: "x..x..x...x.x...", bassB: "x..x..x.x.x..x.x",
      stabA: "...x.......x....", stabB: "..x..x....x..x..", arp: "x.x.x.x.x.x.x.x."
    };
    function mtof(m) { return 440 * Math.pow(2, (m - 69) / 12); }
    function env(gn, t0, peak, att, dec) {
      gn.gain.setValueAtTime(0.0001, t0);
      gn.gain.linearRampToValueAtTime(peak, t0 + att);
      gn.gain.exponentialRampToValueAtTime(0.0001, t0 + att + dec);
    }
    function pan(dest, p) {
      if (!hasPan || !p) return dest;
      var s = ac.createStereoPanner(); s.pan.value = p; s.connect(dest); return s;
    }

    function kick(r, t0, v) {
      var o = ac.createOscillator(), gn = ac.createGain();
      o.frequency.setValueAtTime(145, t0);
      o.frequency.exponentialRampToValueAtTime(46, t0 + 0.09);
      env(gn, t0, 0.95 * v, 0.003, 0.4);
      o.connect(gn); gn.connect(r.g);
      o.start(t0); o.stop(t0 + 0.45);
    }
    function snare(r, t0, v) {
      var n = ac.createBufferSource(), bp = ac.createBiquadFilter(), gn = ac.createGain();
      n.buffer = noise; bp.type = "bandpass"; bp.frequency.value = 1900; bp.Q.value = 0.8;
      env(gn, t0, 0.55 * v, 0.002, 0.17);
      n.connect(bp); bp.connect(gn); gn.connect(r.g);
      n.start(t0, Math.random() * 0.7); n.stop(t0 + 0.2);
      var o = ac.createOscillator(), og = ac.createGain();
      o.type = "triangle"; o.frequency.value = 185;
      env(og, t0, 0.3 * v, 0.002, 0.09);
      o.connect(og); og.connect(r.g); o.start(t0); o.stop(t0 + 0.12);
      var sd = ac.createGain(); sd.gain.value = 0.25 * v; gn.connect(sd); sd.connect(r.send);
    }
    function hat(r, t0, v, open) {
      var n = ac.createBufferSource(), hp = ac.createBiquadFilter(), gn = ac.createGain();
      n.buffer = noise; hp.type = "highpass"; hp.frequency.value = 7200;
      env(gn, t0, 0.2 * v, 0.001, open ? 0.24 : 0.035);
      n.connect(hp); hp.connect(gn); gn.connect(pan(r.g, 0.28));
      n.start(t0, Math.random() * 0.7); n.stop(t0 + (open ? 0.3 : 0.06));
    }
    function bass(r, t0, m, dur, v) {
      var o = ac.createOscillator(), o2 = ac.createOscillator(), lp = ac.createBiquadFilter(), gn = ac.createGain();
      o.type = "sawtooth"; o.frequency.value = mtof(m); o2.frequency.value = mtof(m);
      lp.type = "lowpass"; lp.Q.value = 5;
      lp.frequency.setValueAtTime(260, t0); lp.frequency.linearRampToValueAtTime(900, t0 + 0.02);
      lp.frequency.exponentialRampToValueAtTime(240, t0 + 0.25);
      gn.gain.setValueAtTime(0.0001, t0);
      gn.gain.linearRampToValueAtTime(0.3 * v, t0 + 0.006);
      gn.gain.setTargetAtTime(0.2 * v, t0 + 0.05, 0.08);
      gn.gain.setTargetAtTime(0.0001, t0 + dur, 0.03);
      o.connect(lp); o2.connect(lp); lp.connect(gn); gn.connect(r.g);
      o.start(t0); o2.start(t0); o.stop(t0 + dur + 0.2); o2.stop(t0 + dur + 0.2);
    }
    function stab(r, t0, notes, v, p) {
      var lp = ac.createBiquadFilter(), gn = ac.createGain(), dest = pan(r.g, p);
      lp.type = "lowpass"; lp.Q.value = 2;
      lp.frequency.setValueAtTime(3400, t0); lp.frequency.exponentialRampToValueAtTime(650, t0 + 0.2);
      env(gn, t0, 0.075 * v, 0.004, 0.32);
      lp.connect(gn); gn.connect(dest);
      var sd = ac.createGain(); sd.gain.value = 0.6; gn.connect(sd); sd.connect(r.send);
      notes.forEach(function (m) {
        [-7, 7].forEach(function (cents) {
          var o = ac.createOscillator();
          o.type = "sawtooth"; o.frequency.value = mtof(m); o.detune.value = cents;
          o.connect(lp); o.start(t0); o.stop(t0 + 0.4);
        });
      });
    }
    function pad(r, t0, notes, dur) {
      var lp = ac.createBiquadFilter(), gn = ac.createGain();
      lp.type = "lowpass"; lp.frequency.value = 1300;
      gn.gain.setValueAtTime(0.0001, t0);
      gn.gain.linearRampToValueAtTime(0.05, t0 + 0.5);
      gn.gain.setTargetAtTime(0.0001, t0 + dur - 0.1, 0.25);
      lp.connect(gn); gn.connect(r.g);
      var sd = ac.createGain(); sd.gain.value = 0.5; gn.connect(sd); sd.connect(r.send);
      notes.forEach(function (m, k) {
        var o = ac.createOscillator();
        o.type = "triangle"; o.frequency.value = mtof(m); o.detune.value = k % 2 ? 6 : -6;
        o.connect(lp); o.start(t0); o.stop(t0 + dur + 1.2);
      });
    }
    function pluck(r, t0, m, v, p) {
      var o = ac.createOscillator(), lp = ac.createBiquadFilter(), gn = ac.createGain();
      o.type = "square"; o.frequency.value = mtof(m);
      lp.type = "lowpass"; lp.frequency.setValueAtTime(4200, t0); lp.frequency.exponentialRampToValueAtTime(800, t0 + 0.15);
      env(gn, t0, 0.06 * v, 0.003, 0.26);
      o.connect(lp); lp.connect(gn); gn.connect(pan(r.g, p));
      var sd = ac.createGain(); sd.gain.value = 0.9; gn.connect(sd); sd.connect(r.send);
      o.start(t0); o.stop(t0 + 0.32);
    }

    function step(r, i, t0) {
      var bar = Math.floor(i / 16) % 16, s = i % 16, cycle = Math.floor(i / 256);
      var ch = CH[bar % 8], rr = rng(i * 131 + 7);
      var sec = bar < 4 ? "A" : bar < 8 ? "B" : bar < 12 ? "C" : bar < 14 ? "brk" : "build";
      var drums = sec !== "brk";
      if (drums) {
        var kp = sec === "B" ? P.kickB : sec === "build" ? P.kickBuild : P.kickA;
        if (kp[s] === "x") kick(r, t0, s === 0 ? 1 : 0.85);
        if (P.snare[s] === "x") snare(r, t0, 1);
        else if (sec === "build" && bar === 15 && s >= 8) snare(r, t0, 0.3 + (s - 8) * 0.08);
        else if (s === 15 && rr() < 0.35) snare(r, t0, 0.25);
        var hp = sec === "B" || sec === "build" ? P.hatB : P.hatA;
        if (hp[s] === "x") hat(r, t0, s % 4 === 2 ? 1 : s % 2 ? 0.45 : 0.7, sec === "B" && s === 14);
      }
      // bass
      if (sec === "brk") {
        if (s === 0) bass(r, t0, ch.bass - 12, BAR - 0.1, 0.8);
      } else {
        var bp = sec === "B" || sec === "build" ? P.bassB : P.bassA;
        if (bp[s] === "x") {
          var m = ch.bass + (s === 6 || s === 13 ? 12 : 0) + (s === 10 && sec !== "A" ? 7 : 0);
          bass(r, t0, m, S16 * (s === 0 ? 2.6 : 1.6), 1);
        }
      }
      // chords
      var voicing = cycle % 2 ? ch.v.map(function (n, k) { return k === 0 ? n + 12 : n; }) : ch.v;
      if (sec === "A" || sec === "C") { if (P.stabA[s] === "x") stab(r, t0, voicing, 1, s < 8 ? -0.3 : 0.3); }
      else if (sec === "B" || sec === "build") { if (P.stabB[s] === "x") stab(r, t0, voicing, 0.85, s % 2 ? 0.35 : -0.35); }
      else if (s === 0) pad(r, t0, ch.v, BAR);
      // arpeggio in the second half
      if ((sec === "C" || sec === "brk") && P.arp[s] === "x") {
        var order = [0, 2, 1, 3, 2, 1, 3, 0];
        var note = ch.v[order[(s / 2) % 8]] + 12;
        pluck(r, t0, note, sec === "brk" ? 1 : 0.8, ((s / 2) % 2 ? 0.4 : -0.4));
      }
    }

    var run = null;
    function tick() {
      if (!run) return;
      var ahead = document.hidden ? 1.5 : 0.15;
      while (run.next < ac.currentTime + ahead) {
        if (run.next > ac.currentTime - 0.05) step(run, run.i, run.next);
        run.next += S16; run.i++;
      }
    }
    function stop() {
      if (!run) return;
      var r = run; run = null;
      clearInterval(r.timer);
      r.g.gain.setTargetAtTime(0, ac.currentTime, 0.03);
      r.send.gain.setTargetAtTime(0, ac.currentTime, 0.03);
      setTimeout(function () { r.g.disconnect(); r.send.disconnect(); }, 2500);
    }
    function start() {
      stop();
      var g = ac.createGain(), send = ac.createGain();
      g.gain.value = 1; send.gain.value = 1;
      g.connect(out); send.connect(delay);
      run = { g: g, send: send, i: 0, next: ac.currentTime + 0.06, timer: setInterval(tick, 25) };
      tick();
    }
    return { start: start, stop: stop };
  }

  /* ---------- init ---------- */
  setArmUI();
  setMuteUI();
  setMusicUI();
  setTrackBtn(false);
  layoutTrack();
  resizeCanvas();
  updateDot(0);

  /* read-only hook for automated checks */
  window.BTDemo = {
    info: function () {
      var recentPeak = 0;
      if (L && retained > 0) {
        var n = Math.min(retained, Math.round(sr / 2)), a = new Float32Array(n);
        copyRing(L, total - n, n, a);
        for (var i = 0; i < n; i++) recentPeak = Math.max(recentPeak, Math.abs(a[i]));
      }
      return {
        started: started, armed: armed, musicOn: musicOn, sampleRate: sr, captureMode: captureMode,
        ctxState: ctx ? ctx.state : "none", total: total, retained: retained, histSec: histSec,
        histFrames: L ? histFrames() : 0, frozen: !!view, viewEnd: view ? view.end : null,
        viewStart: view ? view.start : null, sel: sel ? { a: sel.a, b: sel.b } : null, sinceFreeze: sinceFreeze,
        clips: clips.map(function (c) {
          var p = 0;
          for (var i = 0; i < c.l.length; i += 7) p = Math.max(p, Math.abs(c.l[i]), Math.abs(c.r[i]));
          return { start: c.start, dur: c.dur, frames: c.l.length, peak: p };
        }),
        trackPlaying: !!trackPlay, recentPeak: recentPeak, steps: stepDone.slice(), viewDur: viewDur,
        source: musicSrc, bpm: BPM, loopStartedAt: loopStartedAt, ctxTime: ctx ? ctx.currentTime : 0,
        saveDisabled: el.save.disabled, blobLinks: document.querySelectorAll("a[href^='blob:']").length
      };
    },
    /* the seam treatment applied to a buffer the caller decoded itself, and the loop points */
    prepareLoop: function (buf) { return smoothSeam(buf); },
    loop: { start: LOOP_START, end: LOOP_END, bpm: LOOP_BPM },
    /* RMS level (dBFS) of each 10 ms block of the retained audio, oldest first: enough to check
       that capture is continuous, far too coarse to rebuild any audio from */
    levels: function () {
      if (!L || retained <= 0) return [];
      var blk = Math.round(sr / 100), n = Math.floor(retained / blk), out = [], a = new Float32Array(blk);
      for (var k = 0; k < n; k++) {
        copyRing(L, total - retained + k * blk, blk, a);
        var e = 0;
        for (var i = 0; i < blk; i++) e += a[i] * a[i];
        out.push(Math.round(10 * Math.log10(e / blk + 1e-12) * 10) / 10);
      }
      return out;
    }
  };
})();
