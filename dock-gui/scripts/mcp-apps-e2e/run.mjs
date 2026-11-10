// MCP App 端到端（配 fake-model.mjs）：
// 1. Agent 画图 → 工具卡下内联界面（Gen UI）
// 2. 点「在面板中编辑」→ 常驻面板（UI Plugin），内联处变成小条
// 3. 面板开着时 Agent 再改 → 送进同一个面板，不另起内联界面
// 4. 用户在面板里画一个框 → 存回服务器、界面上下文交给模型
// 5. 下一轮模型读到界面上下文（假模型原样念出来）
import { openGui } from "./harness.mjs";
import { readFileSync, readdirSync } from "node:fs";

const OUT = "/tmp/dock-e2e/shots";
const ok = (cond, msg) => {
  console.log(`${cond ? "✔" : "✘"} ${msg}`);
  if (!cond) process.exitCode = 1;
};
const { browser, page, errors } = await openGui({ width: 1440, height: 960 });
page.on("console", (m) => m.type() === "error" && !/font|ERR_CONNECTION|Failed to load resource/.test(m.text()) && errors.push(m.text().slice(0, 200)));

async function say(text) {
  const box = page.locator('textarea[placeholder^="随心输入"]').first();
  await box.waitFor({ timeout: 20000 });
  await box.fill(text);
  await page.keyboard.press("Enter");
  await page.waitForTimeout(1500);
  for (let i = 0; i < 60; i++) {
    if (!(await page.$('button[aria-label="停止"]'))) break;
    await page.waitForTimeout(500);
  }
  await page.waitForTimeout(1500);
}
const appFrames = () => page.frames().filter((f) => f !== page.mainFrame());
const panelFrame = async () => {
  for (const f of appFrames()) {
    const el = await f.frameElement();
    if (await el.evaluate((n) => !!n.closest("aside"))) return f;
  }
  return null;
};

await page.waitForSelector('button:has-text("继续")', { timeout: 20000 });
await page.click('button:has-text("继续")');

await say("在项目白板上画一个登录流程");
ok(appFrames().length === 1, "1. 内联界面出现");
await page.waitForTimeout(2500);
await page.screenshot({ path: `${OUT}/a-inline.png` });

await appFrames()[0].click("button.open", { timeout: 8000 });
await page.waitForTimeout(4000);
const panel = await panelFrame();
ok(!!panel, "2. 常驻面板打开");
ok(!!(await page.$('text=已在右侧面板中更新')), "2. 内联处变成「已在右侧面板中更新」");

await say("再加一个「找回密码」");
await page.waitForTimeout(3000);
ok(appFrames().length === 1, "3. 没有另起内联界面（只有面板里那一个）");
await page.screenshot({ path: `${OUT}/b-agent-in-panel.png` });

// 4. 用户在面板里画一个矩形：工具栏选矩形，在空白处拖一下。
const p = await panelFrame();
const canvas = await (await p.frameElement()).boundingBox();
await p.locator('[data-testid="toolbar-rectangle"]').click({ force: true });
const x0 = canvas.x + canvas.width * 0.25, y0 = canvas.y + canvas.height * 0.75;
await page.mouse.move(x0, y0);
await page.mouse.down();
await page.mouse.move(x0 + 160, y0 + 70, { steps: 8 });
await page.mouse.up();
await page.waitForTimeout(3500);
await page.screenshot({ path: `${OUT}/c-user-drew.png` });

await say("我刚在白板上改了什么？");
const reply = await page.evaluate(() => document.body.innerText);
ok(reply.includes("界面告诉我") && reply.includes("项目白板第"), "5. 模型收到界面交来的上下文");
ok(/用户刚在白板上改了：[^\n]*rectangle/.test(reply), "5. 上下文里有用户刚画的矩形");
console.log("   ", (reply.match(/界面告诉我[^\n]*\n?[^\n]*\n?[^\n]*/) ?? [""])[0].slice(0, 300));
await page.screenshot({ path: `${OUT}/d-context.png` });
// 6. 场景里没有多出来的元素：Agent 的 7 个 + 用户的 1 个，每个容器一个标签。
const root = "/tmp/dock-e2e/home/apps/whiteboard";
const scene = JSON.parse(readFileSync(`${root}/${readdirSync(root)[0]}/scene.json`, "utf8"));
const live = scene.elements.filter((e) => !e.isDeleted);
const shapes = live.filter((e) => !e.containerId);
const labels = live.filter((e) => e.containerId).map((e) => e.containerId);
ok(scene.pending.length === 0, "6. 骨架都画成了完整元素");
ok(shapes.length === 8, `6. 图形不多不少（${shapes.length}）`);
ok(new Set(labels).size === labels.length, "6. 每个容器只有一个标签");
console.log("errors:", errors.slice(0, 8));
ok(errors.length === 0, "没有页面错误");
await browser.close();
