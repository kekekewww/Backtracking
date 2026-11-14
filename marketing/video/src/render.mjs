#!/usr/bin/env node
/* Rebuilds every Backtracking campaign video from the HTML timelines in this folder.
 *
 *   node marketing/video/src/render.mjs                 # everything
 *   node marketing/video/src/render.mjs overview short  # only some jobs (prefix match)
 *   node marketing/video/src/render.mjs --stills overview-en 0,12.5,40   # PNG stills for checking
 *   node marketing/video/src/render.mjs --mux [overview short]  # re-mux the soundtrack only
 *
 * Environment (all optional):
 *   CHROME            Chrome/Chromium executable (default: the standard Windows install path)
 *   FFMPEG            ffmpeg executable (default: "ffmpeg" on PATH)
 *   BT_NODE_MODULES   folder that contains puppeteer-core (default: marketing/video/node_modules)
 *   BT_SCRATCH        folder for lossless intermediates and stills (default: OS temp dir)
 *   BT_MUSIC_DIR      folder holding overview-music.m4a / short-music.m4a (default: marketing/audio-src/out)
 *
 * Pipeline: a tiny static server serves the repository root (fonts, icon, pages); Chrome loads a
 * page, waits for fonts, then for every frame i calls window.seek(i / 30) and screenshots. Frames
 * go to a lossless intermediate, which is encoded to H.264 MP4 (yuv420p, +faststart) and, for the
 * loops, VP9 WebM. Posters are pulled from the intermediate. The overview and the short then get
 * their soundtrack from marketing/audio-src/out/ by stream copy; the loops stay silent.
 */
import http from "node:http";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawn, spawnSync } from "node:child_process";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const videoDir = path.resolve(here, "..");
const repo = path.resolve(videoDir, "..", "..");
const outDir = path.join(videoDir, "out");
const scratch = process.env.BT_SCRATCH || path.join(os.tmpdir(), "bt-video");
const FFMPEG = process.env.FFMPEG || "ffmpeg";
const CHROME = process.env.CHROME || "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe";
const req = createRequire(path.join(process.env.BT_NODE_MODULES || path.join(videoDir, "node_modules"), "_.js"));
const puppeteer = req("puppeteer-core");
const FPS = 30;

const LOOPS = ["select", "drag-out", "ab-sources", "waveform-only", "snapshot-shortcut"];
/* Soundtrack (overview and short only; the loops stay silent). The licensed music lives in
   marketing/audio-src/ (see its README for source and licence) and may only ship inside the videos:
   it is muxed by stream copy and never written as a separate file under marketing/video/. */
const musicDir = process.env.BT_MUSIC_DIR || path.join(repo, "marketing", "audio-src", "out");
const jobs = [];
for (const lang of ["en", "zh-TW"]) {
  jobs.push({ name: `overview-${lang}`, page: "overview.html", query: `lang=${lang}`, w: 1920, h: 1080, dur: 57.5,
    music: "overview-music.m4a",
    mp4: { crf: 22, maxrate: "2600k" }, poster: { t: 52.6, file: `overview-poster-${lang}.jpg` } });
  jobs.push({ name: `short-${lang}`, page: "short.html", query: `lang=${lang}`, w: 1080, h: 1920, dur: 21.0,
    music: "short-music.m4a",
    mp4: { crf: 22, maxrate: "3000k" }, poster: { t: 18.6, file: `short-poster-${lang}.jpg` } });
}
for (const id of LOOPS) {
  jobs.push({ name: `loop-${id}`, page: "loop.html", query: `id=${id}`, w: 1280, h: 720,
    mp4: { crf: 18, maxrate: "1500k" }, webm: { crf: 30, b: "1400k" }, loop: true });
}

const MIME = { ".html": "text/html; charset=utf-8", ".js": "text/javascript; charset=utf-8", ".mjs": "text/javascript",
  ".css": "text/css; charset=utf-8", ".svg": "image/svg+xml", ".png": "image/png", ".woff2": "font/woff2",
  ".ttf": "font/ttf", ".json": "application/json" };
function serve() {
  const srv = http.createServer((rq, rs) => {
    const p = decodeURIComponent(new URL(rq.url, "http://x").pathname);
    const f = path.join(repo, p);
    if (!f.startsWith(repo) || !fs.existsSync(f) || fs.statSync(f).isDirectory()) { rs.writeHead(404); rs.end(); return; }
    rs.writeHead(200, { "content-type": MIME[path.extname(f)] || "application/octet-stream", "cache-control": "max-age=3600" });
    fs.createReadStream(f).pipe(rs);
  });
  return new Promise((ok) => srv.listen(0, "127.0.0.1", () => ok(srv)));
}

function run(args) {
  const r = spawnSync(FFMPEG, args, { stdio: ["ignore", "inherit", "inherit"] });
  if (r.status !== 0) throw new Error("ffmpeg failed: " + args.join(" "));
}

async function openPage(browser, base, job) {
  const page = await browser.newPage();
  await page.setViewport({ width: job.w, height: job.h, deviceScaleFactor: 1 });
  page.on("pageerror", (e) => console.error(`[${job.name}] page error:`, e.message));
  await page.goto(`${base}/marketing/video/src/${job.page}?${job.query}`, { waitUntil: "networkidle0" });
  const fonts = await page.evaluate(async () => {
    const f = await window.__ready;
    await Promise.all([...document.images].map((i) => (i.complete ? 0 : new Promise((r) => (i.onload = i.onerror = r)))));
    return f;
  });
  const need = ["Archivo", "Open Sans", "Geist Mono"].concat(job.query.includes("zh-TW") ? ["Noto Sans TC"] : []);
  const missing = need.filter((n) => !fonts.includes(n));
  if (missing.length) throw new Error(`[${job.name}] fonts not loaded: ${missing.join(", ")}`);
  const duration = await page.evaluate(() => window.DURATION);
  return { page, duration };
}

async function capture(browser, base, job) {
  const { page, duration } = await openPage(browser, base, job);
  const frames = Math.round(duration * FPS);
  const inter = path.join(scratch, `${job.name}.mkv`);
  const ff = spawn(FFMPEG, ["-y", "-v", "error", "-f", "image2pipe", "-framerate", String(FPS), "-c:v", "png", "-i", "-",
    "-c:v", "libx264rgb", "-preset", "ultrafast", "-qp", "0", inter], { stdio: ["pipe", "inherit", "inherit"] });
  const done = new Promise((ok, no) => ff.on("close", (c) => (c === 0 ? ok() : no(new Error("ffmpeg intermediate failed")))));
  const t0 = Date.now();
  for (let i = 0; i < frames; i++) {
    await page.evaluate((t) => window.seek(t), i / FPS);
    const buf = await page.screenshot({ type: "png", optimizeForSpeed: true });
    if (!ff.stdin.write(buf)) await new Promise((r) => ff.stdin.once("drain", r));
    if (i % 150 === 0) process.stdout.write(`  ${job.name}: frame ${i}/${frames}\n`);
  }
  ff.stdin.end();
  await done;
  await page.close();
  console.log(`  ${job.name}: captured ${frames} frames in ${((Date.now() - t0) / 1000).toFixed(0)} s`);
  return { inter, duration };
}

/* Mux the job's music into out/<name>.mp4: video and AAC stream-copied, output as long as the
   video. Without the music file (e.g. a fresh clone), the silent video is written with a warning. */
function mux(job, silent) {
  const mp4 = path.join(outDir, `${job.name}.mp4`);
  const music = path.join(musicDir, job.music);
  if (!fs.existsSync(music)) {
    console.warn(`WARNING: ${path.relative(repo, music)} not found; ${job.name}.mp4 is written without a soundtrack.`);
    fs.copyFileSync(silent, mp4);
    fs.rmSync(silent, { force: true });
    return false;
  }
  run(["-y", "-v", "error", "-i", silent, "-i", music, "-map", "0:v:0", "-map", "1:a:0", "-c:v", "copy", "-c:a", "copy",
    "-t", String(job.dur), "-movflags", "+faststart", mp4]);
  fs.rmSync(silent, { force: true });
  return true;
}

function encode(job, inter) {
  const color = ["-vf", "scale=out_color_matrix=bt709:out_range=tv:flags=lanczos+accurate_rnd+full_chroma_int,format=yuv420p",
    "-color_primaries", "bt709", "-color_trc", "bt709", "-colorspace", "bt709", "-color_range", "tv"];
  const mp4 = path.join(outDir, `${job.name}.mp4`);
  const bufsize = String(parseInt(job.mp4.maxrate) * 2) + "k";
  // videos with a soundtrack are encoded silent to scratch first, then muxed into out/
  const vOut = job.music ? path.join(scratch, `${job.name}-silent.mp4`) : mp4;
  run(["-y", "-v", "error", "-i", inter, ...color, "-r", String(FPS), "-c:v", "libx264", "-preset", "veryslow", "-tune", "animation",
    "-crf", String(job.mp4.crf), "-maxrate", job.mp4.maxrate, "-bufsize", bufsize, "-g", job.loop ? "60" : "90",
    "-an", "-movflags", "+faststart", vOut]);
  if (job.music) mux(job, vOut);
  if (job.webm) {
    const webm = path.join(outDir, `${job.name}.webm`);
    run(["-y", "-v", "error", "-i", inter, ...color, "-r", String(FPS), "-c:v", "libvpx-vp9", "-b:v", job.webm.b, "-crf", String(job.webm.crf),
      "-row-mt", "1", "-deadline", "good", "-cpu-used", "1", "-an", webm]);
  }
  if (job.poster) {
    run(["-y", "-v", "error", "-ss", String(job.poster.t), "-i", inter, "-frames:v", "1", "-q:v", "2", path.join(outDir, job.poster.file)]);
  }
}

async function stills(browser, base, name, times) {
  const job = jobs.find((j) => j.name === name);
  if (!job) throw new Error("unknown job " + name);
  const { page } = await openPage(browser, base, job);
  const dir = path.join(scratch, "stills");
  fs.mkdirSync(dir, { recursive: true });
  for (const t of times) {
    await page.evaluate((x) => window.seek(x), t);
    const f = path.join(dir, `${name}-${t.toFixed(2)}.png`);
    await page.screenshot({ path: f, type: "png" });
    console.log(f);
  }
  await page.close();
}

const argv = process.argv.slice(2);
fs.mkdirSync(scratch, { recursive: true });
fs.mkdirSync(outDir, { recursive: true });
if (argv[0] === "--mux") {
  // re-mux the soundtrack into already rendered videos (their video stream is copied, not re-encoded)
  for (const job of jobs.filter((j) => j.music && (argv.length < 2 || argv.slice(1).some((a) => j.name.startsWith(a))))) {
    const silent = path.join(scratch, `${job.name}-silent.mp4`);
    run(["-y", "-v", "error", "-i", path.join(outDir, `${job.name}.mp4`), "-map", "0:v:0", "-c", "copy", "-an", silent]);
    console.log(mux(job, silent) ? `muxed ${job.name}` : `${job.name}: left silent`);
  }
  process.exit(0);
}
const srv = await serve();
const base = `http://127.0.0.1:${srv.address().port}`;
const browser = await puppeteer.launch({ executablePath: CHROME, headless: true,
  args: ["--hide-scrollbars", "--force-color-profile=srgb", "--font-render-hinting=none", "--disable-lcd-text"] });
try {
  if (argv[0] === "--stills") {
    await stills(browser, base, argv[1], argv[2].split(",").map(Number));
  } else {
    const pick = argv.length ? jobs.filter((j) => argv.some((a) => j.name.startsWith(a))) : jobs;
    for (const job of pick) {
      console.log(`render ${job.name}`);
      const { inter } = await capture(browser, base, job);
      encode(job, inter);
    }
  }
} finally {
  await browser.close();
  srv.close();
}
