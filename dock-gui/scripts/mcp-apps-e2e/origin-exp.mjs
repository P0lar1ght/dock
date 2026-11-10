// 对照实验：同一份 map 界面，(a) srcdoc + allow-scripts（不透明源，我们现在的做法）
// vs (b) 另一个源上的 src + allow-scripts allow-same-origin（官方参考宿主的沙箱代理那样）。数 OSM 瓦片请求。
import { chromium } from "/Users/polar/.npm/_npx/705bc6b22212b352/node_modules/playwright/index.mjs";
import { readFileSync } from "node:fs";
const html = readFileSync("/tmp/mcp3p/serve/app.html", "utf8");
const browser = await chromium.launch();
for (const mode of ["a-srcdoc-opaque", "b-other-origin"]) {
  const page = await browser.newPage();
  let tiles = 0, workers = 0;
  const errs = [];
  page.on("request", (r) => { if (/openstreetmap/.test(r.url())) tiles++; if (/Workers\//.test(r.url())) workers++; });
  page.on("pageerror", (e) => errs.push(String(e).slice(0, 120)));
  page.on("console", (m) => m.type() === "error" && errs.push(m.text().slice(0, 160)));
  await page.goto("http://localhost:1430/index.html").catch(() => {});
  await page.setContent(`<iframe id=f style="width:800px;height:500px"></iframe>`);
  if (mode.startsWith("a")) await page.evaluate((h) => { const f = document.getElementById("f"); f.sandbox = "allow-scripts"; f.srcdoc = h; }, html);
  else await page.evaluate(() => { const f = document.getElementById("f"); f.sandbox = "allow-scripts allow-same-origin"; f.src = "http://127.0.0.1:8899/app.html"; });
  await page.waitForTimeout(15000);
  console.log(mode, "tiles:", tiles, "workers:", workers, "errors:", errs.filter((e) => !/localStorage/.test(e)).slice(0, 3));
  await page.close();
}
await browser.close();
