/* Renders img/og-en.png, img/og-zh-TW.png (1200x630) and img/apple-touch-icon.png (180x180)
 * from tools/og/og.html with a local Chrome.
 *
 *   npm install playwright-core      (in any scratch folder)
 *   NODE_PATH=<scratch>/node_modules node tools/render-og.cjs ["C:/Program Files/Google/Chrome/Application/chrome.exe"]
 *
 * Serves the site folder on a temporary local port so fonts load over http. */
const http = require("http");
const fs = require("fs");
const path = require("path");
const { chromium } = require("playwright-core");

const SITE = path.resolve(__dirname, "..");
const CHROME = process.argv[2] || "C:/Program Files/Google/Chrome/Application/chrome.exe";
const TYPES = { ".html": "text/html; charset=utf-8", ".css": "text/css", ".js": "text/javascript", ".svg": "image/svg+xml", ".woff2": "font/woff2", ".png": "image/png" };

const server = http.createServer((req, res) => {
  const p = path.join(SITE, decodeURIComponent(req.url.split("?")[0]));
  if (!p.startsWith(SITE) || !fs.existsSync(p) || fs.statSync(p).isDirectory()) { res.writeHead(404); res.end(); return; }
  res.writeHead(200, { "Content-Type": TYPES[path.extname(p)] || "application/octet-stream" });
  fs.createReadStream(p).pipe(res);
});

server.listen(0, "127.0.0.1", async () => {
  const base = "http://127.0.0.1:" + server.address().port;
  const browser = await chromium.launch({ executablePath: CHROME });
  try {
    const page = await browser.newPage({ viewport: { width: 1200, height: 630 }, deviceScaleFactor: 1 });
    for (const [lang, file] of [["en", "og-en.png"], ["zh", "og-zh-TW.png"]]) {
      await page.goto(base + "/tools/og/og.html?lang=" + lang);
      await page.evaluate(() => document.fonts.ready);
      await page.waitForTimeout(200);
      await page.screenshot({ path: path.join(SITE, "img", file) });
      console.log("wrote img/" + file);
    }
    await page.setViewportSize({ width: 180, height: 180 });
    await page.setContent('<html><body style="margin:0;background:#081311"><img src="' + base + '/img/icon.svg" style="display:block;width:180px;height:180px;transform:scale(1.07)"></body></html>');
    await page.waitForTimeout(200);
    await page.screenshot({ path: path.join(SITE, "img", "apple-touch-icon.png") });
    console.log("wrote img/apple-touch-icon.png");
  } finally {
    await browser.close();
    server.close();
  }
});
