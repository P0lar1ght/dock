// (c) 真实源 + Dock 生成的 CSP（meta）：看 Chrome 报哪些 CSP 违规。
import { chromium } from "/Users/polar/.npm/_npx/705bc6b22212b352/node_modules/playwright/index.mjs";
import { readFileSync, writeFileSync } from "node:fs";
import { appCsp } from "../../../dock-core/src/mcpApps.ts";
const html = readFileSync("/tmp/mcp3p/serve/app.html", "utf8");
const csp = appCsp({
  connectDomains: ["https://*.openstreetmap.org", "https://cesium.com", "https://*.cesium.com"],
  resourceDomains: ["https://*.openstreetmap.org", "https://cesium.com", "https://*.cesium.com"],
  frameDomains: [], baseUriDomains: [],
});
console.log("CSP:", csp);
const meta = `<meta http-equiv="Content-Security-Policy" content="${csp.replace(/"/g, "&quot;")}">`;
writeFileSync("/tmp/mcp3p/serve/app-csp.html", html.replace(/<head[^>]*>/i, (m) => m + meta));
const browser = await chromium.launch();
const page = await browser.newPage();
let tiles = 0;
const viol = [];
page.on("request", (r) => /openstreetmap/.test(r.url()) && tiles++);
page.on("console", (m) => /Content Security Policy|Refused/.test(m.text()) && viol.push(m.text().slice(0, 220)));
await page.setContent(`<iframe id=f style="width:800px;height:500px"></iframe>`);
await page.evaluate(() => { const f = document.getElementById("f"); f.sandbox = "allow-scripts allow-same-origin"; f.src = "http://127.0.0.1:8899/app-csp.html"; });
await page.waitForTimeout(15000);
console.log("tiles:", tiles);
console.log([...new Set(viol)].slice(0, 8).join("\n"));
await browser.close();
