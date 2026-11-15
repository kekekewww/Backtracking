/* Backtracking user guide: contents panel, copy buttons, current-section highlight. */
(function () {
  "use strict";
  var body = document.body;
  var wide = window.matchMedia("(min-width: 1100px)");

  /* contents: always open beside the text on wide screens, collapsed above it on phones */
  var toc = document.querySelector("details.toc");
  if (toc) {
    var sync = function () { toc.open = wide.matches; };
    sync();
    if (wide.addEventListener) wide.addEventListener("change", sync);
    toc.querySelector("summary").addEventListener("click", function (e) { if (wide.matches) e.preventDefault(); });
    toc.addEventListener("click", function (e) {
      if (!wide.matches && e.target.closest && e.target.closest("a")) toc.open = false;
    });
  }

  /* copy buttons */
  var L = { copy: body.getAttribute("data-copy") || "Copy", done: body.getAttribute("data-copied") || "Copied",
            fail: body.getAttribute("data-copyfail") || "Select the text to copy" };
  function fallback(code) {
    var r = document.createRange();
    r.selectNodeContents(code);
    var sel = window.getSelection();
    sel.removeAllRanges();
    sel.addRange(r);
    var ok = false;
    try { ok = document.execCommand("copy"); } catch (e) { ok = false; }
    if (ok) sel.removeAllRanges();
    return ok;
  }
  Array.prototype.forEach.call(document.querySelectorAll(".code"), function (box) {
    var code = box.querySelector("pre code");
    var bar = box.querySelector(".code-bar");
    if (!code || !bar) return;
    var b = document.createElement("button");
    b.type = "button";
    b.className = "copy";
    b.textContent = L.copy;
    var live = document.createElement("span");
    live.className = "sr-only";
    live.setAttribute("aria-live", "polite");
    var timer = 0;
    function show(text, ok) {
      b.textContent = text;
      b.classList.toggle("done", ok);
      live.textContent = text;
      clearTimeout(timer);
      timer = setTimeout(function () { b.textContent = L.copy; b.classList.remove("done"); live.textContent = ""; }, 1800);
    }
    b.addEventListener("click", function () {
      var text = code.textContent;
      if (navigator.clipboard && window.isSecureContext) {
        navigator.clipboard.writeText(text).then(function () { show(L.done, true); },
          function () { show(fallback(code) ? L.done : L.fail, true); });
      } else {
        var ok = fallback(code);
        show(ok ? L.done : L.fail, ok);
      }
    });
    bar.appendChild(b);
    bar.appendChild(live);
  });

  /* tables: show the right-edge cue while more columns are hidden to the right */
  var boxes = Array.prototype.slice.call(document.querySelectorAll(".table-box"));
  function cue(box) {
    var w = box.firstElementChild;
    box.classList.toggle("more", w.scrollLeft + w.clientWidth < w.scrollWidth - 2);
  }
  boxes.forEach(function (box) {
    box.firstElementChild.addEventListener("scroll", function () { cue(box); }, { passive: true });
    cue(box);
  });
  window.addEventListener("resize", function () { boxes.forEach(cue); });

  /* highlight the section being read */
  if (!("IntersectionObserver" in window) || !toc) return;
  var links = {};
  Array.prototype.forEach.call(toc.querySelectorAll("a[href^='#']"), function (a) {
    links[decodeURIComponent(a.getAttribute("href").slice(1))] = a;
  });
  var heads = Array.prototype.filter.call(document.querySelectorAll(".doc-body h2[id], .doc-body h3[id]"), function (h) { return links[h.id]; });
  var current = null;
  function mark() {
    var line = window.innerHeight * 0.3, pick = null;
    for (var i = 0; i < heads.length; i++) {
      if (heads[i].getBoundingClientRect().top <= line) pick = heads[i]; else break;
    }
    var a = pick ? links[pick.id] : null;
    if (a === current) return;
    if (current) current.removeAttribute("aria-current");
    if (a) a.setAttribute("aria-current", "true");
    current = a;
  }
  var io = new IntersectionObserver(mark, { rootMargin: "0px 0px -70% 0px" });
  heads.forEach(function (h) { io.observe(h); });
  window.addEventListener("scroll", function () { window.requestAnimationFrame(mark); }, { passive: true });
  mark();
})();
