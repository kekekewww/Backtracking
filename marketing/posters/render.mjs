#!/usr/bin/env node
// Renders every poster PNG with headless Chrome. No network: everything is loaded from disk.
//
//   node marketing/posters/render.mjs                 # all 28 PNGs
//   node marketing/posters/render.mjs edition-macos   # only designs whose name contains a filter
//   node marketing/posters/render.mjs --check         # also dump the DOM and report fonts / fit
//
// Env: CHROME=<path to chrome>, JOBS=<parallel renders, default 4>.
import { spawn } from "node:child_process";
import { readFileSync, writeFileSync, mkdirSync, mkdtempSync, rmSync, statSync, existsSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const require = createRequire(import.meta.url);
const { designs, langs } = require(join(here, "src", "manifest.js"));
const out = join(here, "out");
mkdirSync(out, { recursive: true });

const CHROME = process.env.CHROME || [
  "C:/Program Files/Google/Chrome/Application/chrome.exe",
  "C:/Program Files (x86)/Google/Chrome/Application/chrome.exe",
  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
  "/usr/bin/google-chrome",
].find((p) => existsSync(p));
if (!CHROME) { console.error("Chrome not found; set CHROME=<path>"); process.exit(1); }

const args = process.argv.slice(2);
const check = args.includes("--check");
const filters = args.filter((a) => !a.startsWith("--"));
const jobs = [];
for (const d of designs) for (const lang of langs) {
  if (filters.length && !filters.some((f) => `${d.name}-${lang}`.includes(f))) continue;
  jobs.push({ ...d, lang, file: `Backtracking-${d.name}-${lang}.png` });
}

function run(argv) {
  return new Promise((resolve) => {
    const p = spawn(CHROME, argv, { stdio: ["ignore", "pipe", "pipe"] });
    let stdout = "";
    p.stdout.on("data", (b) => (stdout += b));
    p.stderr.on("data", () => {});
    p.on("close", (code) => resolve({ code, stdout }));
  });
}

function pngSize(file) {
  const b = readFileSync(file);
  return { w: b.readUInt32BE(16), h: b.readUInt32BE(20), bytes: b.length };
}

async function render(j) {
  const url = pathToFileURL(join(here, "src", "poster.html")).href + `?d=${j.name}&lang=${j.lang}`;
  const profile = mkdtempSync(join(tmpdir(), "btposter-"));
  const base = ["--headless=new", "--disable-gpu", "--hide-scrollbars", "--force-device-scale-factor=1",
    "--allow-file-access-from-files", "--no-first-run", "--no-default-browser-check",
    `--user-data-dir=${profile}`, `--window-size=${j.w},${j.h}`, "--virtual-time-budget=8000"];
  const png = join(out, j.file);
  await run([...base, `--screenshot=${png}`, url]);
  let note = "";
  if (check) {
    const { stdout } = await run([...base, "--dump-dom", url]);
    const ready = /data-ready="1"/.test(stdout);
    const fonts = (stdout.match(/data-fonts-loaded="([^"]*)"/) || [])[1] || "";
    const sizes = [...stdout.matchAll(/data-sizes="([^"]*)"/g)].map((m) => m[1]).join(" ; ");
    note = ` ready=${ready} mega=[${sizes}] fonts=[${fonts}]`;
  }
  try { rmSync(profile, { recursive: true, force: true }); } catch {}
  const s = pngSize(png);
  const ok = s.w === j.w && s.h === j.h;
  console.log(`${ok ? "ok " : "BAD"} ${j.file} ${s.w}x${s.h} ${(s.bytes / 1024).toFixed(0)} KB${note}`);
  return { ...j, real: s, ok };
}

const results = [];
const queue = [...jobs];
const n = Math.max(1, Number(process.env.JOBS) || 4);
await Promise.all(Array.from({ length: n }, async () => {
  while (queue.length) results.push(await render(queue.shift()));
}));

// Gallery data for index.html: every PNG on disk with its measured size.
const all = [];
for (const d of designs) for (const lang of langs) {
  const file = `Backtracking-${d.name}-${lang}.png`;
  const p = join(out, file);
  if (!existsSync(p)) continue;
  const s = pngSize(p);
  all.push({ name: d.name, set: d.set, use: d.use, lang, file, w: s.w, h: s.h, bytes: s.bytes, mtime: statSync(p).mtime.toISOString() });
}
writeFileSync(join(out, "gallery-data.js"), "window.BT_GALLERY = " + JSON.stringify(all, null, 1) + ";\n");
const bad = results.filter((r) => !r.ok);
console.log(`${results.length} rendered, ${bad.length} with wrong size. Gallery data: out/gallery-data.js`);
process.exit(bad.length ? 1 : 0);
