// 逐条放宽 CSP，看哪一条挡了 Cesium（数 OSM 瓦片请求）。
import { chromium } from "/Users/polar/.npm/_npx/705bc6b22212b352/node_modules/playwright/index.mjs";
import { readFileSync, writeFileSync } from "node:fs";
const html = readFileSync("/tmp/mcp3p/serve/app.html", "utf8");
const D = "https://*.openstreetmap.org https://cesium.com https://*.cesium.com";
const base = {
  "default-src": "'none'",
  "script-src": `'unsafe-inline' 'unsafe-eval' data: blob: ${D}`,
  "style-src": `'unsafe-inline' data: blob: ${D}`,
  "img-src": `data: blob: ${D}`,
  "font-src": `data: blob: ${D}`,
  "media-src": `data: blob: ${D}`,
  "connect-src": D,
  "worker-src": "blob:",
  "frame-src": "'none'",
  "object-src": "'none'",
  "base-uri": "'none'",
  "form-action": "'none'",
};
const variants = {
  ours: {},
  "worker+domains": { "worker-src": `blob: ${D}` },
  "connect+data/blob": { "connect-src": `data: blob: ${D}` },
  "both": { "worker-src": `blob: ${D}`, "connect-src": `data: blob: ${D}` },
};
const browser = await chromium.launch();
for (const [name, patch] of Object.entries(variants)) {
  const csp = Object.entries({ ...base, ...patch }).map(([k, v]) => `${k} ${v}`).join("; ");
  writeFileSync(`/tmp/mcp3p/serve/v-${name.replace(/\W/g, "")}.html`, html.replace(/<head[^>]*>/i, (m) => m + `<meta http-equiv="Content-Security-Policy" content="${csp}">`));
  const page = await browser.newPage();
  let tiles = 0;
  page.on("request", (r) => /openstreetmap/.test(r.url()) && tiles++);
  await page.setContent(`<iframe id=f style="width:800px;height:500px"></iframe>`);
  await page.evaluate((u) => { const f = document.getElementById("f"); f.sandbox = "allow-scripts allow-same-origin"; f.src = u; }, `http://127.0.0.1:8899/v-${name.replace(/\W/g, "")}.html`);
  await page.waitForTimeout(12000);
  console.log(name.padEnd(20), "tiles:", tiles);
  await page.close();
}
await browser.close();
