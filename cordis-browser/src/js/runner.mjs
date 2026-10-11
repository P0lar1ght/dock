// browser_js 的执行器：常驻 Node 进程，stdin / stdout 一行一个 JSON。
// Rust 侧（cordis-browser 的 js.rs）每个 `dock mcp browser` 进程起一个。
//
// 请求：{ id, session, ws, target, code, timeoutMs, shotDir }
// 回复：{ id, ok, result, logs, images, dialog?, error? }
//
// 用 playwright-core 的 connectOverCDP 连 Dock 自己的 Chromium（和 chromiumoxide 同一个进程，
// CDP 允许多个客户端），按 target id 找到本会话的活动标签页，把模型写的代码当 async 函数体跑。
// 参考 Codex 的 node_repl（`js(title, code)`）：一次调用跑一段脚本、做好几步。

import { createInterface } from "node:readline";
import path from "node:path";
import { pathToFileURL } from "node:url";

const playwrightPath = process.argv[2];
const { chromium } = await import(pathToFileURL(playwrightPath).href);

const RESULT_MAX = 12_000;
const AsyncFunction = (async () => {}).constructor;

/** 每个会话一份，跨调用保留（模型把句柄、中间结果放这里）。 */
const states = new Map();

/**
 * 每个会话正在跑的脚本。调用因对话框提前返回后，脚本还卡在 CDP 上，对话框关掉后会接着
 * 跑完剩下的部分；这期间同一会话的新脚本直接报错，不让两段同时动一个标签页。
 */
const busy = new Set();

/** 超时：脚本取消不了，回完话整个执行器退出（Dock 下一次调用重起，`state` 随之清空）。 */
class ScriptTimeout extends Error {}

let connected = null; // { ws, browser }

async function browserFor(ws) {
  if (connected && connected.ws === ws && connected.browser.isConnected()) return connected.browser;
  const browser = await chromium.connectOverCDP(ws);
  connected = { ws, browser };
  for (const ctx of browser.contexts()) keepDialogsOpen(ctx);
  return browser;
}

/**
 * Playwright 在页面没有 dialog 监听时会自动关掉对话框。Dock 的规矩是对话框交给 agent
 * 处理（browser_handle_dialog），所以每个页挂一个空监听，让它留着。
 */
function keepDialogsOpen(ctx) {
  const hold = (p) => p.on("dialog", () => {});
  ctx.pages().forEach(hold);
  ctx.on("page", hold);
}

const targetIds = new WeakMap();

async function targetIdOf(page) {
  if (targetIds.has(page)) return targetIds.get(page);
  const cdp = await page.context().newCDPSession(page);
  try {
    const { targetInfo } = await cdp.send("Target.getTargetInfo");
    targetIds.set(page, targetInfo.targetId);
    return targetInfo.targetId;
  } finally {
    await cdp.detach().catch(() => {});
  }
}

async function pageFor(browser, target) {
  for (const ctx of browser.contexts()) {
    for (const page of ctx.pages()) {
      if ((await targetIdOf(page)) === target) return page;
    }
  }
  return null;
}

/** 只有一个表达式时当返回值（`page.title()`）；否则当函数体，用 return 给结果。 */
function compile(code) {
  const params = ["page", "context", "browser", "state", "log", "snapshot", "screenshot"];
  try {
    return new AsyncFunction(...params, `return (${code}\n);`);
  } catch {
    return new AsyncFunction(...params, code);
  }
}

function show(value) {
  if (value === undefined) return "";
  if (typeof value === "string") return value;
  const seen = new WeakSet();
  try {
    return JSON.stringify(
      value,
      (_, v) => {
        if (typeof v === "object" && v !== null) {
          if (seen.has(v)) return "[circular]";
          seen.add(v);
        }
        return v;
      },
      2,
    );
  } catch {
    return String(value);
  }
}

/** Playwright 的报错和 call log 带终端颜色码：模型和界面都只要文字。 */
function plain(text) {
  return text.replace(/\u001b\[[0-9;]*m/g, "");
}

function clip(text) {
  return text.length > RESULT_MAX ? `${text.slice(0, RESULT_MAX)}\n…(truncated, ${text.length} chars)` : text;
}

async function run(req) {
  const browser = await browserFor(req.ws);
  const page = await pageFor(browser, req.target);
  if (!page) throw new Error(`this session's tab (${req.target}) is not visible to Playwright yet`);
  const logs = [];
  const images = [];
  const state = states.get(req.session) ?? {};
  states.set(req.session, state);
  const log = (...args) => logs.push(plain(args.map((a) => (typeof a === "string" ? a : show(a))).join(" ")));
  const snapshot = (selector = "body") => page.locator(selector).ariaSnapshot();
  const screenshot = async (opts = {}) => {
    const file = path.join(req.shotDir, `js-${Date.now()}-${images.length}.png`);
    await page.screenshot({ ...opts, path: file });
    images.push(file);
    return file;
  };

  if (busy.has(req.session)) {
    return {
      ok: false,
      error:
        "the previous script on this tab is still running (it returned early because of a JS dialog; it resumes once the dialog is handled). Handle the dialog or wait, then retry.",
      logs: [],
      images: [],
    };
  }
  const fn = compile(req.code);
  let onDialog;
  const dialog = new Promise((resolve) => {
    onDialog = (d) => resolve({ type: d.type(), message: d.message() });
    page.on("dialog", onDialog);
  });
  let timer;
  const timeout = new Promise((_, reject) => {
    timer = setTimeout(() => reject(new ScriptTimeout(`script did not finish within ${Math.round(req.timeoutMs / 1000)}s`)), req.timeoutMs);
  });
  busy.add(req.session);
  const script = fn(page, page.context(), browser, state, log, snapshot, screenshot);
  script.then(
    () => busy.delete(req.session),
    () => busy.delete(req.session),
  );
  try {
    const out = await Promise.race([script.then((value) => ({ value })), dialog.then((d) => ({ dialog: d })), timeout]);
    return { ok: true, result: clip(show(out.value)), logs, images, dialog: out.dialog };
  } catch (e) {
    if (e instanceof ScriptTimeout) {
      return {
        ok: false,
        error: `${e.message}. The script runner was restarted to stop it; \`state\` was cleared for every session.`,
        logs,
        images,
        fatal: true,
      };
    }
    return { ok: false, error: clip(plain(e?.stack?.split("\n").slice(0, 6).join("\n") ?? String(e))), logs, images };
  } finally {
    clearTimeout(timer);
    page.off("dialog", onDialog);
  }
}

const rl = createInterface({ input: process.stdin });
rl.on("line", async (line) => {
  let req;
  try {
    req = JSON.parse(line);
  } catch {
    return;
  }
  let reply;
  try {
    reply = await run(req);
  } catch (e) {
    reply = { ok: false, error: String(e?.message ?? e), logs: [], images: [] };
  }
  const fatal = reply.fatal;
  delete reply.fatal;
  process.stdout.write(`${JSON.stringify({ id: req.id, ...reply })}\n`, () => {
    // 超时的脚本停不下来：退出整个执行器才算真的停了。
    if (fatal) process.exit(3);
  });
});
rl.on("close", async () => {
  // Dock 那边关了：只断开自己的 CDP 连接，不关浏览器（它归 chromiumoxide 管）。
  connected = null;
  process.exit(0);
});
process.stdout.write(`${JSON.stringify({ ready: true })}\n`);
