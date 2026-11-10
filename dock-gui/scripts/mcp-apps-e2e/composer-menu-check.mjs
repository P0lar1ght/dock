// 输入框的模型下拉（向上弹）：面板在按钮上方、整个可见、点外面会收。
import { openGui } from "./harness.mjs";
const { browser, page } = await openGui({ width: 1440, height: 900 });
await page.waitForSelector('button:has-text("继续")', { timeout: 20000 });
await page.click('button:has-text("继续")');
await page.waitForTimeout(1500);
const trigger = page.locator('button:has-text("fake")').first();
const t = await trigger.boundingBox();
await trigger.click();
await page.waitForTimeout(400);
const panel = page.locator(".glass-popover").first();
const b = await panel.boundingBox();
const top = await page.evaluate(({ x, y }) => document.querySelector(".glass-popover").contains(document.elementFromPoint(x, y)), { x: b.x + b.width / 2, y: b.y + b.height / 2 });
console.log("trigger", t.y, "panel bottom", b.y + b.height, "above:", b.y + b.height <= t.y, "visible:", top, "inViewport:", b.y >= 0);
await page.mouse.click(200, 300);
await page.waitForTimeout(300);
console.log("closed after outside click:", (await page.locator(".glass-popover").count()) === 0);
await browser.close();
