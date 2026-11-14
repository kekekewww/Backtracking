/* Backtracking site: feature loops, lazy video posters, missing-file fallbacks.
   Nothing here loads until the element is close to the viewport. */
(function () {
  "use strict";
  var doc = document.documentElement;
  doc.classList.add("js-media");
  var reduce = window.matchMedia && window.matchMedia("(prefers-reduced-motion: reduce)");
  var hasIO = "IntersectionObserver" in window;

  /* A loop that cannot be shown removes itself, so the card reads as a plain text card. */
  function drop(fig) {
    fig.classList.add("loop-missing");
    fig.innerHTML = "";
  }

  function still(fig, base) {
    var img = new Image();
    img.alt = "";
    img.decoding = "async";
    img.width = 960; img.height = 540;
    img.addEventListener("error", function () { drop(fig); });
    img.src = base + ".webp";
    fig.querySelector(".loop-box").appendChild(img);
    fig.classList.add("loop-still");
  }

  function motion(fig, base) {
    var v = document.createElement("video");
    v.muted = true; v.defaultMuted = true; v.loop = true; v.autoplay = true; v.playsInline = true;
    v.setAttribute("muted", ""); v.setAttribute("loop", ""); v.setAttribute("autoplay", "");
    v.setAttribute("playsinline", ""); v.setAttribute("preload", "none"); v.setAttribute("aria-hidden", "true");
    v.poster = base + ".webp";
    var types = [["webm", "video/webm"], ["mp4", "video/mp4"]];
    var failed = 0;
    types.forEach(function (t) {
      var s = document.createElement("source");
      s.src = base + "." + t[0];
      s.type = t[1];
      s.addEventListener("error", function () { if (++failed === types.length) drop(fig); });
      v.appendChild(s);
    });
    fig.querySelector(".loop-box").appendChild(v);
    fig._video = v;
    v.load();
    play(v);
  }

  function play(v) {
    var p = v.play();
    if (p && p.catch) p.catch(function () { /* autoplay refused: the poster frame stays */ });
  }

  function start(fig) {
    if (fig._started) return;
    fig._started = true;
    var base = fig.getAttribute("data-src");
    if (reduce && reduce.matches) still(fig, base); else motion(fig, base);
  }

  var loops = Array.prototype.slice.call(document.querySelectorAll("figure.loop[data-src]"));
  if (!hasIO) {
    loops.forEach(start);
  } else {
    /* load a little before the loop scrolls in */
    var loader = new IntersectionObserver(function (entries) {
      entries.forEach(function (e) {
        if (e.isIntersecting) { start(e.target); loader.unobserve(e.target); }
      });
    }, { rootMargin: "300px 0px" });
    /* play only while visible */
    var player = new IntersectionObserver(function (entries) {
      entries.forEach(function (e) {
        var v = e.target._video;
        if (!v) return;
        if (e.isIntersecting && !(reduce && reduce.matches)) play(v); else v.pause();
      });
    }, { threshold: 0 });
    loops.forEach(function (f) { loader.observe(f); player.observe(f); });
  }

  /* Switching to reduced motion while the page is open: stop the loops on their current frame. */
  if (reduce && reduce.addEventListener) {
    reduce.addEventListener("change", function () {
      loops.forEach(function (f) {
        if (!f._video) return;
        if (reduce.matches) f._video.pause(); else play(f._video);
      });
    });
  }

  /* Watch it: set the poster frame only when the player gets close; say so if the file is missing. */
  var players = Array.prototype.slice.call(document.querySelectorAll(".player video"));
  function arm(v) {
    var p = v.getAttribute("data-poster");
    if (p) { v.poster = p; v.removeAttribute("data-poster"); }
  }
  players.forEach(function (v) {
    var note = v.closest(".player").querySelector(".video-note");
    var src = v.querySelector("source");
    if (src && note) src.addEventListener("error", function () { note.hidden = false; });
  });
  if (hasIO) {
    var armer = new IntersectionObserver(function (entries) {
      entries.forEach(function (e) { if (e.isIntersecting) { arm(e.target); armer.unobserve(e.target); } });
    }, { rootMargin: "400px 0px" });
    players.forEach(function (v) { armer.observe(v); });
  } else {
    players.forEach(arm);
  }
})();
