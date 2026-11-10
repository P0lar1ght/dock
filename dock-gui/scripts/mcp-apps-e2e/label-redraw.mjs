// 清空后用同样的 id 再画一遍：框里的标签不能丢（墓碑版本压过新标签的 bug）。
// 假模型「在项目白板上画一个登录流程」= clear + 同样的 open/login/home。画两遍、开面板，看 scene.json。
import { readFileSync, readdirSync } from "node:fs";
import { openGui } from "./harness.mjs";
const { browser, page } = await openGui({ width: 1440, height: 900 });
async function say(text) {
  const box = page.locator('textarea[placeholder^="随心输入"]').first();
  await box.fill(text);
  await page.keyboard.press("Enter");
  await page.waitForTimeout(7000);
}
await page.waitForSelector('button:has-text("继续")', { timeout: 20000 });
await page.click('button:has-text("继续")');
await say("在项目白板上画一个登录流程");
const f = page.frames().find((x) => x !== page.mainFrame());
await f.click("button.open", { timeout: 8000 });
await page.waitForTimeout(4000);
await say("在项目白板上画一个登录流程");
await page.waitForTimeout(4000);
const root = "/tmp/dock-e2e/home/apps/whiteboard";
const dir = readdirSync(root)[0];
const scene = JSON.parse(readFileSync(`${root}/${dir}/scene.json`, "utf8"));
const labels = scene.elements.filter((e) => e.containerId);
console.log("labels:", labels.map((e) => `${e.id} v${e.version}${e.isDeleted ? " DELETED" : ""} ${JSON.stringify(e.text)}`).join(" | "));
console.log(labels.length > 0 && labels.every((e) => !e.isDeleted) ? "PASS 标签都在" : "FAIL 标签丢了");
await page.screenshot({ path: "/tmp/dock-e2e/shots/label-redraw.png" });
await browser.close();
