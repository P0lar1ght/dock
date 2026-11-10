// 官方 server-debug（覆盖 SDK 全部能力）在 Dock 宿主里逐项走一遍：
// 内联出现 → 读它的宿主信息 → 依次点每个方法按钮 → 看它自己的事件日志、回调表，以及宿主这边的效果。
import { openGui } from "./harness.mjs";

const OUT = "/tmp/dock-e2e/shots";
const { browser, page, errors } = await openGui({ width: 1440, height: 1000 });
const logs = [];
page.on("console", (m) => {
  const t = m.text();
  if (/mcp-app/.test(t)) logs.push(t.slice(0, 160));
  if (m.type() === "error" && !/font|ERR_CONNECTION|Failed to load resource/.test(t)) errors.push(t.slice(0, 200));
});
async function say(text) {
  const box = page.locator('textarea[placeholder^="随心输入"]').first();
  await box.waitFor({ timeout: 20000 });
  await box.fill(text);
  await page.keyboard.press("Enter");
  await page.waitForTimeout(1500);
  for (let i = 0; i < 60 && (await page.$('button[aria-label="停止"]')); i++) await page.waitForTimeout(500);
  await page.waitForTimeout(1500);
}
const appFrames = () => page.frames().filter((f) => f !== page.mainFrame());

await page.waitForSelector('button:has-text("继续")', { timeout: 20000 });
await page.click('button:has-text("继续")');
await say('调用 mcp_debug__debug-tool {"contentType":"text","includeStructuredContent":true}');
await page.waitForTimeout(3000);
const f = appFrames()[0];
console.log("frames:", appFrames().length);
if (!f) {
  await page.screenshot({ path: `${OUT}/tp-debug-0.png` });
  console.log(await page.evaluate(() => document.body.innerText.slice(-600)));
  process.exit(1);
}
const text = async (sel) => (await f.locator(sel).first().innerText().catch(() => "")).trim();
console.log("== host context ==\n" + (await text("#host-context-info")).slice(0, 700));
console.log("== host capabilities ==\n" + (await text("#host-capabilities-info")).slice(0, 500));
await page.screenshot({ path: `${OUT}/tp-debug-1.png` });

const click = async (id, wait = 1200) => {
  await f.click(`#${id}`, { timeout: 5000 }).catch((e) => console.log(`  click ${id} failed: ${String(e).slice(0, 120)}`));
  await page.waitForTimeout(wait);
};
const steps = [
  ["log-info-btn"],
  ["update-context-text-btn"],
  ["update-context-structured-btn"],
  ["update-context-image-btn"],
  ["call-debug-tool-btn", 2500],
  ["call-debug-refresh-btn", 2000],
  ["resize-400x300-btn"],
  ["open-link-btn"],
  ["send-message-text-btn", 4000],
];
for (const [id, wait] of steps) {
  await click(id, wait);
  console.log(`clicked ${id}`);
}
console.log("== callback table ==\n" + (await text("#callback-table-body")).slice(0, 1200));
console.log("== event log (tail) ==\n" + (await text("#event-log")).split("\n").slice(-40).join("\n"));
const calls = await page.evaluate(() => (window.__tauriCalls ?? []).filter((c) => !c.cmd.startsWith("plugin:event")).map((c) => c.cmd + " " + JSON.stringify(c.args ?? {}).slice(0, 120)));
console.log("== tauri calls ==\n" + calls.join("\n"));
const frameEl = await f.frameElement();
console.log("iframe size:", JSON.stringify(await frameEl.boundingBox()));
await page.screenshot({ path: `${OUT}/tp-debug-2.png` });
console.log("== host logs ==\n" + logs.slice(0, 10).join("\n"));
console.log("errors:", errors.slice(0, 8));
await browser.close();
