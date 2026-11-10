// 真模型（isolated config 默认 deepseek-flash）走一遍：画图 → 进面板 → 用户画一个框 → 问模型改了什么。
// 断言不了措辞，只打印模型回答和工具调用，人看。
import { openGui } from "./harness.mjs";

const OUT = "/tmp/dock-e2e/shots";
const { browser, page, errors } = await openGui({ width: 1440, height: 960 });
async function say(text, wait = 180) {
  const box = page.locator('textarea[placeholder^="随心输入"]').first();
  await box.waitFor({ timeout: 20000 });
  await box.fill(text);
  await page.keyboard.press("Enter");
  await page.waitForTimeout(2000);
  for (let i = 0; i < wait * 2; i++) {
    if (!(await page.$('button[aria-label="停止"]'))) break;
    await page.waitForTimeout(500);
  }
  await page.waitForTimeout(2000);
}
const frames = () => page.frames().filter((f) => f !== page.mainFrame());
await page.waitForSelector('button:has-text("继续")', { timeout: 20000 });
await page.click('button:has-text("继续")');
await say("在项目白板上画一个登录流程：「打开 App」→「输入账号」→「进入首页」，用箭头连起来。用白板工具 whiteboard_edit（按需工具，先 search_tool 找）。画完一句话说明。");
console.log("frames after draw:", frames().length);
await page.screenshot({ path: `${OUT}/r1.png` });
if (!frames().length) {
  console.log(await page.evaluate(() => document.body.innerText.slice(-800)));
  await browser.close();
  process.exit(1);
}
await frames()[0].click("button.open", { timeout: 8000 });
await page.waitForTimeout(4000);
const p = frames()[0];
await p.locator('[data-testid="toolbar-rectangle"]').click({ force: true });
const box = await (await p.frameElement()).boundingBox();
const x0 = box.x + box.width * 0.3, y0 = box.y + box.height * 0.78;
await page.mouse.move(x0, y0);
await page.mouse.down();
await page.mouse.move(x0 + 160, y0 + 70, { steps: 8 });
await page.mouse.up();
await page.waitForTimeout(3500);
await say("我刚在白板上改了什么？只根据你掌握的信息用一两句话回答，不要调用工具。");
const text = await page.evaluate(() => document.body.innerText);
console.log("---- 最后的回答 ----");
console.log(text.slice(text.lastIndexOf("我刚在白板上改了什么")).slice(0, 600));
await page.screenshot({ path: `${OUT}/r2.png` });
console.log("errors:", errors.filter((e) => !/font|ERR_CONNECTION/.test(e)).slice(0, 5));
await browser.close();
