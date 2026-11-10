// 设置 › MCP 服务：卡片「⋯」菜单要整个露出来、不被下一张卡片盖住。
import { openGui } from "./harness.mjs";
const { browser, page } = await openGui({ width: 1440, height: 900 });
await page.waitForSelector('button:has-text("继续")', { timeout: 20000 });
await page.click('button[aria-label^="设置"]');
await page.waitForTimeout(1000);
await page.getByText("MCP 服务", { exact: true }).first().click();
await page.waitForTimeout(1500);
const more = page.locator('button[aria-label="更多"]').first();
await more.hover();
await more.click({ force: true });
await page.waitForTimeout(500);
const panel = page.locator(".glass-popover").first();
const box = await panel.boundingBox();
// 面板上几个点，最上面的元素都得在面板里（没被别的卡片盖住、没被裁掉）。
const ok = await page.evaluate(({ x, y, w, h }) => {
  const el = document.querySelector(".glass-popover");
  return [[x + w / 2, y + 8], [x + w / 2, y + h / 2], [x + w / 2, y + h - 8]].map(([px, py]) => el.contains(document.elementFromPoint(px, py)));
}, { x: box.x, y: box.y, w: box.width, h: box.height });
console.log("panel", box, "top-most at probes:", ok, await panel.innerText());
await page.screenshot({ path: "/tmp/dock-e2e/shots/popover.png" });
await browser.close();
