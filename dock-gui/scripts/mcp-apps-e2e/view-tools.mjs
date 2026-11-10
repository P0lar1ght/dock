// 第五步：界面工具（appCapabilities.tools）。真模型：画图 → 进面板 → 用户点选一个框 →
// 问模型选中的是哪个（应调 app_whiteboard__get_selection）→ 让它把视图挪到另一个框（focus）。
// 断言不了措辞：打印回答、会话里的工具调用，人看。
import { readFileSync, readdirSync, statSync } from "node:fs";
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
/** 最近写过的会话目录（会话按 cwd 分目录，里面一层是会话 id）。 */
function latestSession() {
  const root = "/tmp/dock-e2e/home/sessions";
  const isDir = (p) => statSync(p).isDirectory();
  const dirs = readdirSync(root)
    .map((d) => `${root}/${d}`)
    .filter(isDir)
    .flatMap((d) => readdirSync(d).map((s) => `${d}/${s}`).filter(isDir));
  return dirs.sort((a, b) => statSync(b).mtimeMs - statSync(a).mtimeMs)[0];
}
const toolRows = () =>
  readFileSync(`${latestSession()}/chat_history.jsonl`, "utf8")
    .split("\n")
    .filter(Boolean)
    .map((l) => JSON.parse(l));
await page.waitForSelector('button:has-text("继续")', { timeout: 20000 });
await page.click('button:has-text("继续")');
// FAKE=1：本地假模型（start-env.sh fake），按「调用 <名字> <JSON>」直接调界面工具，测通路。
const FAKE = !!process.env.FAKE;
await say(FAKE ? "在项目白板上画一个登录流程" : "在项目白板上画一个登录流程：「打开 App」→「输入账号」→「进入首页」，从左到右横着排，用箭头连起来。用白板工具 whiteboard_edit（按需工具，先 search_tool 找）。画完一句话说明。");
if (!frames().length) {
  console.log("没有界面：", await page.evaluate(() => document.body.innerText.slice(-800)));
  await browser.close();
  process.exit(1);
}
await frames()[0].click("button.open", { timeout: 8000 });
await page.waitForTimeout(5000);
const calls = await page.evaluate(() => (window.__dockViewExposed ?? null));
const panel = frames().at(-1);
const box = await (await panel.frameElement()).boundingBox();
// 适配内容后三个框横排在画布中间：点正中间选中「输入账号」。
await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2);
await page.waitForTimeout(1500);
await page.screenshot({ path: `${OUT}/v-select.png` });
if (FAKE) {
  await say("调用 app_whiteboard__get_selection {}");
  await say("调用 app_whiteboard__get_viewport {}");
  // 从会话里 get_viewport 的结果挑最后一个方框 id 去 focus。
  const vp = toolRows().filter((r) => r.kind === "tool" && /get_viewport/.test(r.arguments ?? "")).at(-1)?.content ?? "";
  const id = [...vp.matchAll(/rectangle (\S+?)「/g)].map((m) => m[1]).at(-1);
  await say(`调用 app_whiteboard__focus {"ids":["${id}"],"select":true}`);
  await say("调用 app_whiteboard__snapshot {}");
  await say("调用 app_whiteboard__no_such_tool {}");
} else {
  await say("我在白板上选中的是哪个框？查一下再回答，一句话。");
  await say("把我的视图移到「进入首页」那个框上，并选中它。一句话说明。");
}
await page.screenshot({ path: `${OUT}/v-focus.png` });
const text = await page.evaluate(() => document.body.innerText);
console.log("---- 回答 ----");
console.log(text.slice(Math.max(0, text.indexOf(FAKE ? "调用 app_whiteboard__get_selection" : "我在白板上选中的是哪个框"))).slice(0, 1500));
console.log("---- 工具调用 ----");
for (const r of toolRows()) {
  if (r.kind === "tool") console.log(r.name, (r.arguments ?? "").slice(0, 160), "=>", (r.content ?? "").slice(0, 200).replace(/\n/g, " | "));
  if (r.kind === "system-reminder" && /界面提供的工具/.test(r.text ?? "")) console.log("[reminder]", r.text.slice(0, 300).replace(/\n/g, " | "));
}
console.log("errors:", errors.filter((e) => !/font|ERR_CONNECTION/.test(e)).slice(0, 5), calls);
await browser.close();
