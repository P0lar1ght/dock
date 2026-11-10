// 用 dock-core 当前的 appCsp 生成 map App 的 CSP，在真源（非不透明）iframe 里数 OSM 瓦片。
import { chromium } from "/Users/polar/.npm/_npx/705bc6b22212b352/node_modules/playwright/index.mjs";
import { readFileSync, writeFileSync } from "node:fs";
import { appCsp } from "/Users/polar/Desktop/AILab/dock/dock-core/src/mcpApps.ts";
const D = ["https://*.openstreetmap.org", "https://cesium.com", "https://*.cesium.com"];
const csp = appCsp({ resourceDomains: D, connectDomains: D });
console.log(csp.match(/worker-src[^;]*/)[0]);
const html = readFileSync("/tmp/mcp3p/serve/app.html", "utf8");
writeFileSync("/tmp/mcp3p/serve/v-merged.html", html.replace(/<head[^>]*>/i, (m) => m + `<meta http-equiv="Content-Security-Policy" content="${csp}">`));
const browser = await chromium.launch();
const page = await browser.newPage();
let tiles = 0;
page.on("request", (r) => /openstreetmap/.test(r.url()) && tiles++);
await page.setContent(`<iframe id=f style="width:800px;height:500px"></iframe>`);
await page.evaluate(() => { const f = document.getElementById("f"); f.sandbox = "allow-scripts allow-same-origin"; f.src = "http://127.0.0.1:8899/v-merged.html"; });
await page.waitForTimeout(12000);
console.log("merged appCsp tiles:", tiles);
await browser.close();
