// 用法：node fixture-shot.mjs <前缀> <样本...>：预览里回放样本、展开全部工具卡、整段截图。
import { chromium } from "/Users/polar/.npm/_npx/705bc6b22212b352/node_modules/playwright/index.mjs";
const [prefix, ...names] = process.argv.slice(2);
const b = await chromium.launch();
for (const name of names) {
  const p = await b.newPage({ viewport: { width: 1000, height: 3000 }, deviceScaleFactor: 2 });
  const errs = [];
  p.on("pageerror", (e) => errs.push(String(e)));
  await p.addInitScript(() => localStorage.setItem("dock-gui.prefs", JSON.stringify({ expandTools: true })));
  await p.goto(`http://localhost:1430/preview.html#tool-fixtures-${name}`);
  await p.waitForTimeout(1500);
  await p.evaluate(() => {
    for (const b of document.querySelectorAll("button")) if (/^用时/.test(b.textContent?.trim() ?? "")) b.click();
  });
  await p.waitForTimeout(1800);
  await p.evaluate(() => {
    for (const el of document.querySelectorAll("*")) {
      const cs = getComputedStyle(el);
      if (cs.overflowY === "auto" || cs.overflowY === "hidden") {
        if (el.closest("pre") || el.tagName === "PRE") continue;
      }
    }
    const s = [...document.querySelectorAll(".overflow-y-auto")].find((e) => e.querySelector(".gap-8"));
    if (s) Object.assign(s.style, { maxHeight: "none", overflow: "visible", maskImage: "none" });
    let q = s?.parentElement;
    while (q) {
      Object.assign(q.style, { overflow: "visible", maxHeight: "none", height: "auto" });
      q = q.parentElement;
    }
  });
  await p.waitForTimeout(300);
  await (await p.$(".gap-8")).screenshot({ path: `/private/tmp/scrollrepro/e2e/${prefix}-${name}.png` });
  console.log(name, errs.length ? "ERR " + errs.join(" | ") : "ok");
  await p.close();
}
await b.close();
