/* Scene builders shared by overview.html and short.html. Each builder returns
   {a, b, el, render(u)} where u is seconds since the scene's start. */
(function () {
  "use strict";
  var S = window.STR;
  var ZH = E.lang === "zh-TW";
  var SC = {};
  window.SC = SC;

  SC.SAVED = "Saved C:\\Users\\you\\Backtracking\\output\\Backtracking-1794\u2026-13.wav";
  SC.SNAP = "Saved snapshot C:\\Users\\you\\Backtracking\\saved\\Backtracking-1794\u2026-14.wav";
  // macOS default saved snapshots folder per docs/USER_GUIDE.md: ~/Backtracking/saved/
  SC.SNAP_MAC = "Saved snapshot /Users/you/Backtracking/saved/Backtracking-1794…-14.wav";
  SC.NAME ="Backtracking-1794\u2026-13.wav";
  SC.META = function (d, rate, enc) { return d + " \u00b7 " + (rate || "48 kHz") + " \u00b7 " + (enc || "32-bit float"); };

  SC.stage = function (W, H) {
    document.documentElement.lang = S.htmlLang;
    var st = document.getElementById("stage");
    st.style.width = W + "px"; st.style.height = H + "px";
    document.body.style.width = W + "px"; document.body.style.height = H + "px";
    return st;
  };
  SC.scene = function (stage, a, b) { return { a: a, b: b, el: E.el("div", "scene", stage) }; };
  SC.run = function (scenes, duration) {
    window.DURATION = duration;
    window.seek = function (t) {
      scenes.forEach(function (s) {
        var on = t >= s.a && t < s.b;
        s.el.style.display = on ? "block" : "none";
        if (on) s.render(t - s.a, t);
      });
    };
  };
  SC.txt = function (parent, cls, x, y, html, css) {
    var d = E.el("div", "abs " + (cls || ""), parent, html);
    E.pos(d, x, y);
    if (css) d.style.cssText += ";" + css;
    return d;
  };
  function esc(s) { return String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;"); }
  SC.esc = esc;
  /* body markup: {…} = no line break inside, [X] = inline keycap, \n = line break */
  SC.rich = function (s) {
    return esc(s)
      .replace(/\[([^\]]+)\]/g, function (m, k) { return '<span class="ikey' + (k === "`" ? " grave" : "") + '"><span>' + k + "</span></span>"; })
      .replace(/\{([^}]+)\}/g, '<span class="nb">$1</span>')
      .replace(/\n/g, "<br>");
  };
  SC.plain = function (s) { return String(s).replace(/[{}\[\]]/g, ""); };

  /* mega headline: returns the line elements (each slams separately) */
  SC.mega = function (parent, x, y, lines, size, zhSize) {
    var box = SC.txt(parent, "mega", x, y, "");
    box.style.fontSize = (ZH ? zhSize : size) + "px";
    return lines.map(function (l, i) {
      var d = E.el("span", "ln" + (i === lines.length - 1 ? " m" : ""), box);
      d.textContent = l;
      d.style.transformOrigin = "0 100%";
      return d;
    });
  };

  /* history scale (real History options) */
  SC.scale = function (parent, x, y, w, font) {
    var d = SC.txt(parent, "scale wide scale-lbl", x, y, "");
    d.style.width = w + "px"; d.style.fontSize = font + "px";
    var spans = S.scale.map(function (s) { var sp = E.el("span", "", d); sp.textContent = s; return sp; });
    return { el: d, spans: spans };
  };

  /* tone-on-tone background waveform band (--c-blue-2), scrolling with time; flat fill */
  SC.band = function (parent, W, y, h, seed) {
    var cv = E.canvas(parent, W, h);
    cv.style.position = "absolute"; cv.style.left = "0"; cv.style.top = y + "px";
    parent.insertBefore(cv, parent.firstChild);
    var au = E.Audio({ seed: seed || 23 });
    return function (t) {
      E.drawWave(E.ctx(cv), { audio: au, x: 0, y: 0, w: W, h: h, tEnd: 200 + t, pps: W / 7, col: 3, fill: "#2a3fe6" });
    };
  };

  SC.footer = function (parent, x, y, w, font) {
    var r = SC.txt(parent, "rule", x, y, ""); r.style.width = w + "px";
    var ed = SC.txt(parent, "editions wide", x, y + 26, S.editions.map(function (e) { return "<span>" + esc(e) + "</span>"; }).join(""));
    ed.style.fontSize = font + "px";
    var li = SC.txt(parent, "wide tc", 0, y + 26, esc(S.licence));
    li.style.fontSize = font + "px"; li.style.color = "var(--c-paper-2)";
    li.style.left = "auto"; li.style.right = (parseFloat(document.getElementById("stage").style.width) - x - w) + "px";
    return { rule: r, ed: ed, li: li };
  };

  /* ------------------------------------------------------------------ problem */
  /* Wordless: music scrolls past (newest at the right), the great part (mint) goes by,
     the record button is hit too late: the counter starts at 0:00 after the moment. */
  SC.problem = function (stage, a, b, L) {
    var sc = SC.scene(stage, a, b);
    var cv = E.canvas(sc.el, L.W, L.waveH);
    cv.style.position = "absolute"; cv.style.left = "0"; cv.style.top = L.waveY + "px";
    var momLen = L.momLen || 2.6;
    var audio = E.Audio({ seed: 11, moments: [{ t0: 10, len: momLen }] });
    var rec = SC.txt(sc.el, "", L.recX - L.recD / 2, L.recY - L.recD / 2, "",
      "width:" + L.recD + "px;height:" + L.recD + "px;border-radius:50%;background:#f4f1e8;border:5px solid #070c3d;box-shadow:9px 9px 0 #070c3d");
    var dot = E.el("div", "abs", rec);
    dot.style.cssText = "left:27%;top:27%;width:46%;height:46%;border-radius:50%;position:absolute";
    var cnt = SC.txt(sc.el, "", L.cntX, L.cntY, "0:00.0",
      "font-family:'Geist Mono',monospace;font-weight:600;font-size:" + L.cntSize + "px;letter-spacing:0.02em");
    var cur = E.Cursor(sc.el, L.curScale || 1.5);
    var t0 = L.t0 || 9.4, click = L.click;
    var keys = [{ t: 0, x: L.curFrom.x, y: L.curFrom.y }, { t: click - 1.2, x: L.curFrom.x, y: L.curFrom.y },
      { t: click - 0.15, x: L.recX + 6, y: L.recY + 4 }];
    sc.render = function (u) {
      var tEnd = t0 + u, recStart = t0 + click;
      var on = u >= click;
      var ctx = E.ctx(cv);
      E.drawWave(ctx, {
        audio: audio, x: 0, y: 0, w: L.W, h: L.waveH, tEnd: tEnd, pps: L.pps, col: 3,
        fill: "rgba(244,241,232,0.30)", sel: on ? [recStart, tEnd + 1] : null, selFill: "#f4f1e8",
        hi: [10, 10 + momLen], hiFill: "#b0fbd3"
      });
      dot.style.background = on ? "#e5484d" : "#8a90b8";
      var pressed = u >= click && u < click + 0.12;
      rec.style.transform = pressed ? "translate(5px,5px)" : "";
      rec.style.boxShadow = pressed ? "4px 4px 0 #070c3d" : "9px 9px 0 #070c3d";
      var el = on ? u - click : 0;
      cnt.textContent = Math.floor(el / 60) + ":" + ("0" + Math.floor(el % 60)).slice(-2) + "." + Math.floor((el * 10) % 10);
      cnt.style.color = on ? "#f4f1e8" : "rgba(244,241,232,0.55)";
      var p = E.path(keys, u);
      cur.set(p.x, p.y, { pressT: u - click });
    };
    return sc;
  };

  /* ------------------------------------------------------------------ one-liner */
  SC.oneLiner = function (stage, a, b, L) {
    var sc = SC.scene(stage, a, b);
    var bg = E.canvas(sc.el, L.W, L.bgH);
    bg.style.position = "absolute"; bg.style.left = "0"; bg.style.top = L.bgY + "px";
    var bgAudio = E.Audio({ seed: 23 });
    var lines = SC.mega(sc.el, L.x, L.y, S.oneLiner, L.size, L.zhSize);
    var sca = SC.scale(sc.el, L.sx, L.sy, L.sw, L.sf);
    var needle = SC.txt(sc.el, "needle", 0, L.sy - 30, ""); needle.style.height = "30px";
    sc.render = function (u) {
      var rw = E.outCubic(E.prog(u, 0, 0.9));
      var tEnd = 120 - 16 * rw + Math.max(0, u - 0.9);
      E.drawWave(E.ctx(bg), { audio: bgAudio, x: 0, y: 0, w: L.W, h: L.bgH, tEnd: tEnd, pps: L.W / 7, col: 3, fill: "#2a3fe6" });
      lines.forEach(function (ln, i) { E.slam(ln, u, L.beats[i]); });
      // needle scrubs backwards from 10 min to 10 s, then the 10 min mark lands with the last word
      var s0 = sca.spans[0], s6 = sca.spans[6];
      var x6 = L.sx + s6.offsetLeft, x0 = L.sx + s0.offsetLeft;
      var landed = u >= L.beats[L.beats.length - 1];
      E.show(needle, u < 0.95);
      needle.style.left = (E.lerp(x6, x0, E.inOut(E.prog(u, 0.05, 0.9))) - 1) + "px";
      s6.classList.toggle("on", landed);
    };
    return sc;
  };

  /* ------------------------------------------------------------------ beats (panel) */
  /* L: {panel:{x,y,z}, N0, caps:[{at, beat, body}], cap:{x,y,w,num,title,body}, keys:{x,y},
         daw:{x,y,w,h}, T:{enter, press, dragEnd, release, keysUp, mid, mid2, press2, drag2, drop, play},
         curFrom:{x,y}} */
  SC.beats = function (stage, a, b, L) {
    var sc = SC.scene(stage, a, b);
    var T = L.T;
    var audio = E.Audio({ seed: 7, moments: [{ t0: 40.25, len: 3.35 }] });
    var SEL0 = 40, SEL1 = 44;
    // captions
    var band = L.band ? SC.band(sc.el, parseFloat(stage.style.width), L.band.y, L.band.h, 31) : null;
    var caps = L.caps.map(function (c) {
      var g = SC.txt(sc.el, "", L.cap.x, L.cap.y, "");
      g.style.width = L.cap.w + "px";
      var bt = S.beats[c.beat];
      var parts = [];
      var tx = g, rw = g;
      if (L.cap.row) { rw = E.el("div", "", g); rw.style.cssText = "display:flex;align-items:flex-start;gap:44px"; }
      if (L.cap.num) { var n = E.el("div", "cap-num", rw); n.textContent = bt.n; n.style.fontSize = L.cap.num + "px"; n.style.marginBottom = L.cap.row ? "0" : "34px"; parts.push(n); }
      if (L.cap.row) { tx = E.el("div", "", rw); tx.style.flex = "1"; tx.style.paddingTop = "6px"; }
      var ti = E.el("div", "cap-title", tx); ti.textContent = bt.title; ti.style.fontSize = (ZH ? L.cap.zhTitle || L.cap.title : L.cap.title) + "px"; parts.push(ti);
      if (c.body) { var bo = E.el("div", "sub", tx); bo.innerHTML = SC.rich(bt.body); bo.style.fontSize = L.cap.body + "px"; bo.style.marginTop = L.cap.row ? "18px" : "26px"; bo.style.color = "var(--c-paper)"; parts.push(bo); }
      parts.forEach(function (p) { p.style.transformOrigin = "0 100%"; });
      return { g: g, parts: parts, at: c.at };
    });
    var daw = E.DAW(sc.el, { x: L.daw.x, y: L.daw.y, w: L.daw.w, h: L.daw.h, tracks: 3, bars: 8, rowH: L.daw.rowH || 110 });
    var P = E.Panel(sc.el, { x: L.panel.x, y: L.panel.y, z: L.panel.z, width: L.panel.width, waveH: L.panel.waveH, audio: audio, history: 10 });
    // History scale under the panel until the DAW arrives (the panel shows History = 10 s)
    var hs = L.hscale ? SC.scale(sc.el, L.hscale.x, L.hscale.y, L.hscale.w, L.hscale.font) : null;
    if (hs) hs.spans[0].classList.add("on");
    var keys = E.Keys(sc.el, ["Shift"]); E.pos(keys.el, L.keys.x, L.keys.y);
    if (L.keys.big) keys.el.classList.add("big");
    var stageW = parseFloat(stage.style.width);
    var prev = E.el("div", "daw-prev", daw.tracks[1].lane);
    var clipW = (SEL1 - SEL0) * daw.pps;
    prev.style.width = clipW + "px";
    var clip = E.Clip(daw, 1, 0, audio, SEL0, SEL1, SC.NAME);
    var slab = E.Slab(sc.el, SC.NAME, SC.META("4.00 s"), L.slabScale || 1.3);
    var cur = E.Cursor(sc.el, L.curScale || 1.5);

    var view0 = L.N0 + T.press;           // view fixed by the selection gesture
    var yPress = Math.round(P.H * 0.49), yMid = Math.round(P.H * 0.31), yTip = Math.round(P.H * 0.63);
    var P1 = { x: P.sx(P.tx(SEL0, view0)) + 1, y: P.sy(yPress) };
    var P2 = { x: P.sx(P.tx(SEL1, view0)) + 7, y: P.sy(yPress + 4) };
    var Pm = { x: P.sx(P.tx((SEL0 + SEL1) / 2, view0)), y: P.sy(yMid) };
    var dropLx = 2 * daw.pxBar;           // clip lands at bar 3 of track 2
    var Pd = { x: daw.sx(dropLx) + 26, y: daw.sy(1) + daw.rowH * 0.5 };
    var path = [
      { t: 0, x: L.curFrom.x, y: L.curFrom.y }, { t: T.enter[0], x: L.curFrom.x, y: L.curFrom.y },
      { t: T.enter[1], x: P1.x, y: P1.y }, { t: T.press, x: P1.x, y: P1.y },
      { t: T.dragEnd, x: P2.x, y: P2.y }, { t: T.mid, x: P2.x, y: P2.y },
      { t: T.mid2, x: Pm.x, y: Pm.y }, { t: T.press2, x: Pm.x, y: Pm.y },
      { t: T.drag2, x: Pd.x, y: Pd.y }, { t: T.drop + 0.35, x: Pd.x, y: Pd.y },
      { t: T.drop + 0.95, x: Pd.x + 150, y: Pd.y + 70 }
    ];
    function snapEnd(u) {
      var p = E.path(path, u);
      var ta = P.xt((p.x - L.panel.x) / P.z - 11, view0);
      return Math.max(SEL0, Math.min(SEL1, Math.round(ta)));
    }
    var dawIn = T.dawIn;
    E.show(daw.box, false);

    sc.render = function (u) {
      var now = L.N0 + u;
      // captions: each holds until the next one
      caps.forEach(function (c, i) {
        var next = caps[i + 1] ? caps[i + 1].at : 1e9;
        var on = u >= c.at && u < next;
        E.show(c.g, on);
        if (on) c.parts.forEach(function (p, j) { E.slam(p, u, c.at + 0.16 * j); });
      });
      // panel drops in on its shadow
      var pd = E.prog(u, 0, 0.14);
      P.wrap.style.transform = "translateY(" + (-70 * (1 - E.outBack(pd, 1.6))).toFixed(2) + "px)";
      P.wrap.style.visibility = u >= 0 ? "visible" : "hidden";
      // DAW
      E.show(daw.box, u >= dawIn);
      if (u >= dawIn) daw.box.style.transform = "translateY(" + (50 * (1 - E.outBack(E.prog(u, dawIn, dawIn + 0.14), 1.6))).toFixed(2) + "px)";
      // selection state
      var editing = u >= T.press && u < T.drop;
      var sel = null, dur = null;
      if (editing) {
        var e1 = u < T.release ? snapEnd(u) : SEL1;
        if (e1 > SEL0) { sel = [SEL0, e1]; dur = (e1 - SEL0).toFixed(2) + " s"; }
      }
      if (band) band(u);
      if (hs) { var hOn = u < dawIn; E.show(hs.el, hOn); if (hOn) E.slam(hs.el, u, 0.3, { dy: 24 }); }
      var tip = u >= T.mid2 && u < T.press2 ? { text: "Drag and Drop", x: P.tx((SEL0 + SEL1) / 2, view0) + 72, y: yTip } : null;
      P.draw({ now: now, view: editing ? view0 : now, sel: sel, dur: dur, tip: tip,
        status: editing ? "Selection" : u >= T.drop ? SC.SAVED : "" });
      // Shift keycap
      var kOn = u >= T.enter[1] - 0.05 && u < T.keysUp + 0.3;
      E.show(keys.el, kOn);
      if (kOn) { E.slam(keys.el, u, T.enter[1] - 0.05, { dy: 30 }); keys.press(u >= T.enter[1] + 0.05 && u < T.keysUp); }
      // cursor + drag
      var c = E.path(path, u);
      var dragging = u >= T.press2 && u < T.drop;
      var pressT = u >= T.press2 ? u - T.press2 : u - T.press;
      cur.set(c.x, c.y, { pressT: pressT });
      if (dragging) {
        var c0 = E.path(path, u - 1 / 60), c1 = E.path(path, u + 1 / 60);
        var k = E.outBack(E.prog(u, T.press2, T.press2 + 0.12), 2.4);
        // keep the slab inside the frame: near the right edge it hangs to the left of the cursor
        var sw = slab.el.offsetWidth * slab.scale;
        var sxp = c.x + 22 + sw > stageW - 40 ? c.x - sw - 10 : c.x + 22;
        slab.set(sxp, c.y + 18, { k: 0.55 + 0.45 * k, vx: (c1.x - c0.x) * 30, vy: (c1.y - c0.y) * 30 });
      } else slab.set(0, 0, { hide: true });
      // drop preview / clip
      var over = dragging && daw.trackAt(c.y) === 1 && c.x > daw.sx(0);
      E.show(prev, over);
      if (over) prev.style.left = dropLx + "px";
      var dropped = u >= T.drop;
      E.show(clip, dropped);
      if (dropped) clip.redraw();
      if (dropped) clip.style.transform = "translateY(" + (-14 * (1 - E.outBack(E.prog(u, T.drop, T.drop + 0.12), 2))).toFixed(2) + "px)";
      clip.style.left = dropLx + "px";
      if (T.play && u >= T.play[0]) daw.playhead(dropLx + (u - T.play[0]) * daw.pps); else daw.playhead(null);
    };
    return sc;
  };

  /* ------------------------------------------------------------------ end card */
  SC.endCard = function (stage, a, b, L) {
    var sc = SC.scene(stage, a, b);
    var lk = SC.txt(sc.el, "lockup", L.lock.x, L.lock.y,
      '<img src="/assets/backtracking-icon.svg" width="' + L.lock.icon + '" height="' + L.lock.icon + '" alt=""><span class="wm">Backtracking</span>');
    lk.querySelector(".wm").style.fontSize = L.lock.font + "px";
    var tag = SC.txt(sc.el, "wide tc", L.tag.x, L.tag.y, esc(S.tag));
    tag.style.fontSize = L.tag.font + "px";
    if (L.tag.right != null) { tag.style.left = "auto"; tag.style.right = L.tag.right + "px"; }
    var lines = SC.mega(sc.el, L.mega.x, L.mega.y, S.mega, L.mega.size, L.mega.zhSize);
    var cta = SC.txt(sc.el, "cta", L.cta.x, L.cta.y, "");
    cta.textContent = S.cta; cta.style.fontSize = L.cta.font + "px";
    var url = SC.txt(sc.el, "url", L.url.x, L.url.y, esc(S.url)); url.style.fontSize = L.url.font + "px";
    var ft = L.footer.stack ? null : SC.footer(sc.el, L.footer.x, L.footer.y, L.footer.w, L.footer.font);
    if (L.footer.stack) {
      var r = SC.txt(sc.el, "rule", L.footer.x, L.footer.y, ""); r.style.width = L.footer.w + "px";
      var ed = SC.txt(sc.el, "editions wide", L.footer.x, L.footer.y + 28, S.editions.map(function (e) { return "<span>" + esc(e) + "</span>"; }).join(""));
      ed.style.fontSize = L.footer.font + "px";
      var li = SC.txt(sc.el, "wide tc", L.footer.x, L.footer.y + 28 + L.footer.font * 2, esc(S.licence));
      li.style.fontSize = L.footer.font + "px"; li.style.color = "var(--c-paper-2)";
      ft = { rule: r, ed: ed, li: li };
    }
    var bt = L.beats;
    sc.render = function (u) {
      lines.forEach(function (ln, i) { E.slam(ln, u, bt[i]); });
      E.slam(cta, u, bt[2], { dx: -60, dy: 0, s: 0 });
      E.slam(url, u, bt[2] + 0.2, { dy: 24 });
      [lk, tag, ft.rule, ft.ed, ft.li].forEach(function (x) { x.style.visibility = "visible"; });
    };
    return sc;
  };
})();
