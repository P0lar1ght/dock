// MCP App 面板拖成独立窗（无头）：两个页面当两扇窗，假桥带一条事件总线（emit 转给所有页面）。
// 主窗口「在新窗口打开」→ 按它请求的 URL 开第二页（独立面板窗）→ 主窗口内联卡改成「面板窗口」；
// Agent 再调一次 → 调用经事件送进独立窗；点内联卡 → 聚焦那扇窗；关掉独立窗 → 主窗口恢复。
import { chromium } from "/Users/polar/.npm/_npx/705bc6b22212b352/node_modules/playwright/index.mjs";
import { readFileSync } from "node:fs";

const TOKEN = readFileSync("/tmp/dock-e2e/token", "utf8").trim();
const PROJ = "/tmp/dock-e2e/proj";
const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 }, deviceScaleFactor: 2 });
const pages = [];
await ctx.exposeBinding("__busEmit", async (_src, event, payload) => {
  for (const p of pages) if (!p.isClosed()) await p.evaluate(([e, d]) => window.__busDeliver?.(e, d), [event, payload]).catch(() => {});
});
async function open(label, query = "") {
  const page = await ctx.newPage();
  await page.addInitScript(({ token, proj, label }) => {
    if (window.top !== window) return;
    let cb = 0;
    const fns = new Map();
    const listeners = [];
    window.__busLog = [];
    window.__busDeliver = (event, payload) => {
      window.__busLog.push({ event, payload });
      for (const l of listeners) if (l.event === event) fns.get(l.handler)?.({ event, id: l.handler, payload });
    };
    window.__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label }, currentWebview: { windowLabel: label, label } },
      transformCallback: (fn) => {
        const id = ++cb;
        fns.set(id, fn);
        return id;
      },
      unregisterCallback: (id) => fns.delete(id),
      convertFileSrc: (p) => p,
      invoke: async (cmd, args) => {
        (window.__tauriCalls ??= []).push({ cmd, args });
        if (cmd === "dock_connection") return { ws: "ws://127.0.0.1:18997/api/ws", token };
        if (cmd === "remote_list") return { active: "e2e", remotes: [{ id: "e2e", name: "e2e", url: "http://127.0.0.1:18997" }] };
        if (cmd === "plugin:dialog|open") return proj;
        if (cmd === "plugin:event|listen") {
          listeners.push({ event: args.event, handler: args.handler });
          return args.handler;
        }
        if (cmd === "plugin:event|emit") {
          window.__busEmit(args.event, args.payload);
          return null;
        }
        if (cmd.startsWith("plugin:event|")) return 1;
        return null;
      },
    };
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
    localStorage.setItem("dock-gui.prefs", JSON.stringify({ approval: "auto", projectDir: proj, restoreSession: false, notifyTurnDone: false, notifyNeedsInput: false }));
  }, { token: TOKEN, proj: PROJ, label });
  await page.goto(`http://localhost:1430/${query}`);
  pages.push(page);
  return page;
}
const calls = (page, cmd) => page.evaluate((c) => (window.__tauriCalls ?? []).filter((x) => x.cmd === c), cmd);
async function say(page, text) {
  const box = page.locator('textarea[placeholder^="随心输入"]').first();
  await box.fill(text);
  await page.keyboard.press("Enter");
  await page.waitForTimeout(6000);
}
const check = (name, ok, extra = "") => console.log(`${ok ? "PASS" : "FAIL"} ${name}${extra ? "  " + extra : ""}`);

const main = await open("main");
await main.waitForSelector('button:has-text("继续")', { timeout: 20000 });
await main.click('button:has-text("继续")');
await say(main, "在项目白板上画一个登录流程");
const inline = main.frames().find((f) => f !== main.mainFrame());
await inline.click("button.open", { timeout: 8000 });
await main.waitForTimeout(3000);
check("主窗口开了面板", (await main.locator('button[aria-label="在新窗口打开"]').count()) > 0);

// 「在新窗口打开」：主窗口请求 Rust 开窗；照它请求的参数开第二页。
await main.locator('button[aria-label="在新窗口打开"]').first().click();
await main.waitForTimeout(1500);
const req = (await calls(main, "open_panel_window")).at(-1)?.args;
check("请求开独立窗", req?.panel?.startsWith("mcp-app:"), JSON.stringify(req));
const detached = await open(`panel-mcpapp-x-${req.sessionId}`, `?session=${req.sessionId}&panel=${encodeURIComponent(req.panel)}`);
await detached.waitForTimeout(8000);
check("独立窗画出白板", detached.frames().length > 1);
check("主窗口面板关了", (await main.locator('button[aria-label="在新窗口打开"]').count()) === 0);
const mainText = () => main.evaluate(() => document.body.innerText);
check("内联卡指向面板窗口", /面板窗口中/.test(await mainText()));

// Agent 再调一次：调用送进独立窗。
const before = await detached.evaluate(() => window.__busLog.filter((e) => e.event === "mcp-app/route").length);
await say(main, "在项目白板上画一个登录流程");
const after = await detached.evaluate(() => window.__busLog.filter((e) => e.event === "mcp-app/route" && e.payload.to.startsWith("panel-mcpapp")).length);
check("新调用送进独立窗", after > before, `${before} → ${after}`);
check("主窗口没再开内联界面", main.frames().length === 1 + 0 || !(await mainText()).includes("在面板中编辑"));

// 点内联卡：聚焦那扇窗（请求同一 kind + 会话），不在主窗口开面板。
const n = (await calls(main, "open_panel_window")).length;
await main.locator('button:has-text("面板窗口中")').last().click();
await main.waitForTimeout(1000);
const focus = (await calls(main, "open_panel_window")).at(-1)?.args;
check("点内联卡 = 聚焦独立窗", (await calls(main, "open_panel_window")).length === n + 1 && focus.panel === req.panel && focus.sessionId === req.sessionId);
check("主窗口没开面板", (await main.locator('button[aria-label="在新窗口打开"]').count()) === 0);

// 界面工具：独立窗的界面经它自己那条连接报给 Dock，模型调用推过去、它跑完回结果。
await say(main, "调用 app_whiteboard__get_selection {}");
const viewCalls = await detached.evaluate(() => (window.__tauriCalls ?? []).length);
const rows = readFileSync(`${(await import("node:child_process")).execSync("ls -td /tmp/dock-e2e/home/sessions/*/*/ | head -1").toString().trim()}chat_history.jsonl`, "utf8")
  .split("\n").filter(Boolean).map((l) => JSON.parse(l));
const sel = rows.filter((r) => r.kind === "tool" && /get_selection/.test(r.arguments ?? "")).at(-1);
check("独立窗里的界面工具调得通", !!sel && !sel.is_error && /选中/.test(sel.content), (sel?.content ?? "").slice(0, 60));

// 关掉独立窗：主窗口不再认为面板开着。
await detached.close({ runBeforeUnload: true });
await main.waitForTimeout(1500);
check("关窗后内联卡恢复", !/面板窗口中/.test(await mainText()));
await main.screenshot({ path: "/tmp/dock-e2e/shots/popout-main.png" });
await browser.close();
