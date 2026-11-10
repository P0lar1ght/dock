// 用法：node run.mjs <名字> <提示词文件>
// 开新对话 → 发提示 → 等这一轮跑完 → 展开所有工具卡 → 截图；再从网关拉这一轮的事件存成样本。
import { readFileSync, writeFileSync } from "node:fs";
import { openGui } from "./harness.mjs";

const [name, promptFile, preset] = process.argv.slice(2);
const prompt = readFileSync(promptFile, "utf8").trim();
const { browser, page, errors } = await openGui({ width: 1280, height: 1000 });
await page.waitForSelector('button:has-text("继续")', { timeout: 20000 });
if (preset) await page.click(`button:has-text("${preset}")`);
await page.click('button:has-text("继续")');
const box = page.locator('textarea[placeholder^="随心输入"]').first();
await box.waitFor({ timeout: 20000 });
await box.fill(prompt);
await page.keyboard.press("Enter");
const t0 = Date.now();
// 等跑完：停止键消失、没有「正在思考」，并稳定 3 秒。
await page.waitForSelector('button[aria-label="停止"]', { timeout: 60000 }).catch(() => {});
let stable = 0;
const handled = [];
while (Date.now() - t0 < 8 * 60_000) {
  await page.waitForTimeout(1000);
  // 等人的卡：提问选第一个选项提交；计划批准；权限允许本次。
  const btn = async (label) => (await page.$(`button:has-text("${label}")`)) ?? null;
  const submit = await btn("提交回答");
  if (submit) {
    // 每道题点第一个选项（选项按钮里有数字键帽「1」），再提交。
    await page.evaluate(() => {
      const card = [...document.querySelectorAll("button")].find((b) => b.textContent?.trim() === "提交回答")?.closest("[tabindex]");
      if (!card) return;
      for (const b of card.querySelectorAll("button")) {
        const badge = b.querySelector("kbd");
        if (badge && badge.textContent?.trim() === "1") b.click();
      }
    });
    await page.waitForTimeout(300);
    await submit.click().catch(() => {});
    handled.push("question");
    await page.waitForTimeout(1000);
    continue;
  }
  const approve = await btn("批准并执行");
  if (approve) {
    await approve.click();
    handled.push("plan");
    continue;
  }
  const allow = await btn("允许本次");
  if (allow) {
    await allow.click();
    handled.push("permission");
    continue;
  }
  const busy = await page.evaluate(() => !!document.querySelector('button[aria-label="停止"]'));
  stable = busy ? 0 : stable + 1;
  if (stable >= 3) break;
}
if (handled.length) console.log("answered:", handled.join(", "));
console.log(`turn done in ${Math.round((Date.now() - t0) / 1000)}s`);
// 展开过程区和每张工具卡。
await page.evaluate(() => {
  for (const b of document.querySelectorAll("button")) if (/^用时/.test(b.textContent?.trim() ?? "")) b.click();
});
await page.waitForTimeout(400);
await page.evaluate(() => {
  for (const b of document.querySelectorAll('.fold[data-open="false"]')) b.previousElementSibling?.click?.();
});
await page.waitForTimeout(800);
// 对话流整段截下来：把滚动区撑开。
await page.evaluate(() => {
  const s = [...document.querySelectorAll(".overflow-y-auto")].find((e) => e.querySelector(".gap-8"));
  if (s) {
    s.style.maxHeight = "none";
    s.style.overflow = "visible";
    s.style.maskImage = "none";
    let p = s.parentElement;
    while (p) {
      p.style.overflow = "visible";
      p.style.maxHeight = "none";
      p.style.height = "auto";
      p = p.parentElement;
    }
  }
});
await page.waitForTimeout(300);
const scroller = await page.$(".gap-8");
await scroller.screenshot({ path: `/private/tmp/scrollrepro/e2e/${name}.png` });
// 事件样本：threadId 从页面上的会话拿（dock.1 thread/history）。
const tools = await page.evaluate(() => [...document.querySelectorAll(".gap-8 button span.font-semibold")].map((e) => e.textContent));
console.log("tool rows:", JSON.stringify(tools));
console.log("errors:", errors.join(" | ") || "none");
await browser.close();
