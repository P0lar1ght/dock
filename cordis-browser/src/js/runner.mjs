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
import { readFileSync } from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const playwrightPath = process.argv[2];
/** 用法指南（`docs()`），Rust 侧和执行器一起写到同一个目录。 */
const guidePath = process.argv[3];
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

/**
 * 看一眼脚本写法，给几句提示（只附在结果里，不拦）：固定等待、整页抠文本。工具描述里
 * 写过这两条，实测模型还是常这么写，在结果里就近提醒更管用。
 */
function hintsFor(code) {
  const hints = [];
  if (/waitForTimeout\s*\(|new Promise\s*\(\s*\(?\s*\w*\s*\)?\s*=>\s*setTimeout/.test(code)) {
    hints.push(
      "fixed sleep (waitForTimeout / setTimeout): wait for a condition instead — locator.waitFor(), page.waitForURL(), expect the element you need.",
    );
  }
  if (/body['"`]?\s*\)?\s*\.\s*(innerText|textContent)|document\.body\.(innerText|textContent)|textContent\s*\(\s*['"`]body/.test(code)) {
    hints.push(
      "reading the whole page's text: locate the region you need (getByRole / a heading's container) and read inside it, or use snapshot(selector).",
    );
  }
  return hints;
}

/**
 * 在临时标签页里同时打开几个网址、读出内容、关掉（参考 Codex 的 `tabs.content`）。
 * 临时页共用登录态，不登记进会话（读完就关）；页上的对话框直接关掉，别让读取卡住。
 * 按输入顺序回 `{ url, title, text }`，打不开的那项回 `{ url, error }`。
 */
async function readPagesIn(context, urls, opts = {}, scriptDeadline = Infinity) {
  const { selector = "body", mode = "text", maxChars = 4000, concurrency = 4, timeoutMs = 20_000 } = opts;
  if (mode !== "text" && mode !== "snapshot") throw new Error(`readPages: mode must be 'text' or 'snapshot', got ${JSON.stringify(mode)}`);
  // 单个网址也行；别的类型说清楚（`[...'https://a']` 会拆成一个个字符）。
  if (typeof urls !== "string" && !Array.isArray(urls)) throw new Error("readPages: urls must be a URL string or an array of URLs");
  const list = typeof urls === "string" ? [urls] : [...urls];
  // 总截止时间：默认按脚本时限留余量，网址多、页又慢时到点收手，已读的照常返回——不然一路
  // 读到脚本超时、执行器重启，结果全丢。
  const deadline = Math.min(scriptDeadline, opts.deadlineMs ? Date.now() + opts.deadlineMs : Infinity);
  const out = new Array(list.length);
  let next = 0;
  const worker = async () => {
    while (next < list.length) {
      const i = next++;
      const url = list[i];
      const left = deadline - Date.now();
      if (left <= 500) {
        out[i] = { url, error: "skipped: deadline reached (raise timeout_ms or read fewer URLs per call)" };
        continue;
      }
      const budget = Math.min(timeoutMs, left);
      const p = await context.newPage();
      p.on("dialog", (d) => d.dismiss().catch(() => {}));
      // 临时页里弹出来的新页没人管：也关掉，免得留下不归任何会话的页。
      p.on("popup", (pp) => pp.close().catch(() => {}));
      try {
        await p.goto(url, { timeout: budget, waitUntil: "domcontentloaded" });
        await p.waitForLoadState("load", { timeout: Math.min(budget, 5000) }).catch(() => {});
        const region = p.locator(selector).first();
        const raw = mode === "snapshot" ? await region.ariaSnapshot({ timeout: budget }) : await region.innerText({ timeout: budget });
        // 按字符截（别把 emoji 这类代理对切成两半）。
        const chars = Array.from(raw);
        const text = chars.length > maxChars ? `${chars.slice(0, maxChars).join("")}…(truncated, ${chars.length} chars)` : raw;
        out[i] = { url, title: await p.title().catch(() => ""), text };
      } catch (e) {
        out[i] = { url, error: plain(String(e?.message ?? e)).split("\n")[0] };
      } finally {
        await p.close().catch(() => {});
      }
    }
  };
  await Promise.all(Array.from({ length: Math.max(1, Math.min(concurrency, list.length)) }, worker));
  return out;
}

/** 只有一个表达式时当返回值（`page.title()`）；否则当函数体，用 return 给结果。 */
function compile(code) {
  const params = ["page", "context", "browser", "state", "log", "snapshot", "screenshot", "docs", "onDialog", "tabs", "readPages"];
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
  const docs = () => (guidePath ? readFileSync(guidePath, "utf8") : "(guide not available)");

  // 本会话的标签页（Rust 侧按顺序给 target id）。脚本里新开的页记进 `opened`，
  // `select` 记下跑完之后哪页是活动页；回给 Rust 去登记，GUI 画面才跟得过去。
  // 只认 tabs.new 开的页：直接 context.newPage() 的页分不清是哪个会话的，不收。
  const order = [...(req.tabs ?? [req.target])];
  const opened = [];
  let activate = null;
  // 本会话的页都要盯对话框（watch 在下面定义好 dialogListener 之后才可用）。
  let watch = () => {};
  const pageAt = async (i) => {
    const id = order[i];
    const p = id ? await pageFor(browser, id) : null;
    if (!p) throw new Error(`no tab at index ${i} (this session has ${order.length})`);
    watch(p);
    return p;
  };
  const tabs = {
    async list() {
      const current = activate ?? req.target;
      const out = [];
      for (const [index, id] of order.entries()) {
        const p = await pageFor(browser, id);
        out.push({ index, url: p?.url() ?? "", title: p ? await p.title().catch(() => "") : "", active: id === current });
      }
      return out;
    },
    get: pageAt,
    async new(url) {
      const p = await page.context().newPage();
      // 先登记进本会话再打开网址：goto 失败也不会留下不归任何会话的页。
      const id = await targetIdOf(p);
      order.push(id);
      opened.push(id);
      watch(p);
      if (url) await p.goto(url);
      return p;
    },
    async select(which) {
      const p = typeof which === "number" ? await pageAt(which) : which;
      const id = await targetIdOf(p);
      if (!order.includes(id)) throw new Error("tabs.select: not one of this session's tabs (open it with tabs.new)");
      watch(p);
      activate = id;
      await p.bringToFront().catch(() => {});
      return p;
    },
  };

  // 脚本里先声明「接下来的对话框怎么处理」：'accept' / 'dismiss' / { accept, text } /
  // async (dialog) => {…}。声明过的就地处理、记进 log，脚本接着跑；没声明的照旧让调用
  // 提前返回，交给 browser_handle_dialog。
  let policy = null;
  const onDialog = (action = "accept") => {
    policy = action;
  };
  const fn = compile(req.code);
  let dialogListener;
  const dialog = new Promise((resolve) => {
    dialogListener = async (d) => {
      if (!policy) {
        const target = await targetIdOf(d.page()).catch(() => null);
        resolve({ type: d.type(), message: d.message(), target });
        return;
      }
      const what = `${d.type()} dialog ${JSON.stringify(d.message())}`;
      try {
        if (typeof policy === "function") {
          await policy(d);
          log(`${what}: handled by onDialog`);
        } else if (policy === "dismiss" || (typeof policy === "object" && policy.accept === false)) {
          await d.dismiss();
          log(`${what}: dismissed`);
        } else {
          const text = typeof policy === "object" ? policy.text : undefined;
          await d.accept(text);
          log(`${what}: accepted${text !== undefined ? ` with ${JSON.stringify(text)}` : ""}`);
        }
      } catch (e) {
        log(`${what}: could not handle it (${e?.message ?? e})`);
      }
    };
  });
  // 盯本会话的每一页：开始时的活动页、tabs.get / new / select 拿到的页、这些页点出来的弹出页。
  // 以前只盯活动页：别的页上弹框时 onDialog 不生效、也不提前返回，脚本卡到超时。
  const watched = new Set();
  watch = (p) => {
    if (watched.has(p)) return;
    watched.add(p);
    p.on("dialog", dialogListener);
  };
  watch(page);
  const onPopup = async (p) => {
    const opener = await p.opener().catch(() => null);
    if (opener && watched.has(opener)) watch(p);
  };
  page.context().on("page", onPopup);
  let timer;
  const timeout = new Promise((_, reject) => {
    timer = setTimeout(() => reject(new ScriptTimeout(`script did not finish within ${Math.round(req.timeoutMs / 1000)}s`)), req.timeoutMs);
  });
  busy.add(req.session);
  // 留 3 秒给脚本收尾、回话：readPages 到点收手，不拖到执行器超时重启。
  const scriptDeadline = Date.now() + Math.max(1000, req.timeoutMs - 3000);
  const readPages = (urls, opts) => readPagesIn(page.context(), urls, opts, scriptDeadline);
  const script = fn(page, page.context(), browser, state, log, snapshot, screenshot, docs, onDialog, tabs, readPages);
  script.then(
    () => busy.delete(req.session),
    () => busy.delete(req.session),
  );
  try {
    const out = await Promise.race([script.then((value) => ({ value })), dialog.then((d) => ({ dialog: d })), timeout]);
    return { ok: true, result: clip(show(out.value)), logs, images, dialog: out.dialog, opened, activate, hints: hintsFor(req.code) };
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
    return {
      ok: false,
      error: clip(plain(e?.stack?.split("\n").slice(0, 6).join("\n") ?? String(e))),
      logs,
      images,
      opened,
      activate,
      hints: hintsFor(req.code),
    };
  } finally {
    clearTimeout(timer);
    for (const p of watched) p.off("dialog", dialogListener);
    page.context().off("page", onPopup);
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
