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
/**
 * 动作 / 等待的默认超时。Playwright 默认 30 秒：定位不到、按钮一直灰着时每次白等 30 秒
 * （实测 21 次）。参考 Codex 的浏览器插件：日常动作不手写超时，慢的那一步再显式给。
 */
const ACTION_TIMEOUT_MS = 10_000;
const NAVIGATION_TIMEOUT_MS = 30_000;
/**
 * `page.goto` 没写 `waitUntil` 时：到 DOMContentLoaded 就返回，`load` 最多再等这么久（和
 * browser_navigate 一致）。Playwright 默认等 `load`，图片、广告、统计脚本常把它拖上好几秒，
 * 页面早就能读能点了；之后的定位器本来就会自己等元素出现。写了 `waitUntil` 照写的来。
 */
const LOAD_GRACE_MS = 1_000;
/** 定位超时时附上的页面结构（ARIA 快照）最多这么多字。 */
const SNAPSHOT_ON_ERROR_MAX = 3_000;
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
  for (const ctx of browser.contexts()) {
    keepDialogsOpen(ctx);
    ctx.setDefaultTimeout(ACTION_TIMEOUT_MS);
    ctx.setDefaultNavigationTimeout(NAVIGATION_TIMEOUT_MS);
  }
  return browser;
}

/**
 * Playwright 在页面没有 dialog 监听时会自动关掉对话框。Dock 的规矩是对话框交给 agent
 * 处理（browser_handle_dialog），所以每个页挂一个空监听，让它留着。
 */
function fastGoto(page) {
  if (page.__dockFastGoto) return;
  page.__dockFastGoto = true;
  const goto = page.goto.bind(page);
  page.goto = async (url, opts = {}) => {
    if (opts.waitUntil) return goto(url, opts);
    const response = await goto(url, { ...opts, waitUntil: "domcontentloaded" });
    await page.waitForLoadState("load", { timeout: LOAD_GRACE_MS }).catch(() => {});
    return response;
  };
}

function keepDialogsOpen(ctx) {
  const hold = (p) => {
    p.on("dialog", () => {});
    fastGoto(p);
  };
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

/** 一页里最多带几个 iframe 的内容进快照。 */
const SNAPSHOT_FRAMES_MAX = 10;

/**
 * ARIA 快照。看整页（`selector` 是 `body`）时把 iframe 里面也带上：`locator.ariaSnapshot()`
 * 不进 iframe，只给一行 `- iframe`，iframe 里的表单对模型是看不见的。每个 iframe 一段，
 * 标出它的网址 / name，好用 `page.frameLocator(…)` 或 `page.frame({ url })` 去动它。
 */
async function ariaTree(page, selector = "body", opts = {}) {
  const main = await page.locator(selector).ariaSnapshot(opts);
  if (selector !== "body") return main;
  const frames = page
    .frames()
    // `about:blank` / `about:srcdoc` 的 iframe 也看：srcdoc 里的表单、脚本填出来的编辑器都在这种
    // iframe 里。跨域进不去的、里面是空的，下面跳过。
    .filter((f) => f !== page.mainFrame())
    .slice(0, SNAPSHOT_FRAMES_MAX);
  const parts = [main];
  for (const f of frames) {
    const tree = await f
      .locator("body")
      .ariaSnapshot({ timeout: Math.min(opts.timeout ?? 2000, 2000) })
      .catch(() => null);
    if (!tree || !tree.trim()) continue;
    const name = f.name() ? ` name=${JSON.stringify(f.name())}` : "";
    const body = tree
      .split("\n")
      .map((l) => `  ${l}`)
      .join("\n");
    parts.push(`- frame ${JSON.stringify(f.url())}${name}:\n${body}`);
  }
  return parts.join("\n");
}

/** 这些请求可能一直不结束（推送流、媒体），不算「还在加载」。 */
const LONG_LIVED = new Set(["eventsource", "websocket", "media"]);

/**
 * 等页面安静下来：本页没有在飞的请求，DOM 内容（增删节点、改文字）也有 `quietMs` 没变；
 * 最多 `timeoutMs`，到点照常返回（`settled: false`），不抛错。比固定 sleep 快：页面一停就走。
 * `inflight` 是本页在飞的请求（脚本开始后才开始的那些）。
 */
async function settleOn(p, inflight, { quietMs = 500, timeoutMs = 5000 } = {}) {
  const start = Date.now();
  // 页面里挂一个计数的 MutationObserver。几个 settle 同时等同一页时共用一个（`users` 计数），
  // 最后一个等完就拆掉，不在页面里一直挂着。
  const mutations = (join) =>
    p
      .evaluate((join) => {
        let box = window.__dockMutations;
        if (!box) {
          box = { n: 0, users: 0 };
          box.obs = new MutationObserver(() => box.n++);
          box.obs.observe(document, { subtree: true, childList: true, characterData: true });
          window.__dockMutations = box;
        }
        // 跳转换了文档：新文档里的这一个也算这次 settle 的。
        if (join || box.users === 0) box.users++;
        return box.n;
      }, join)
      // 正在跳转（执行上下文没了）：当成还在变。
      .catch(() => -1);
  const leave = () =>
    p
      .evaluate(() => {
        const box = window.__dockMutations;
        if (!box) return;
        box.users--;
        if (box.users <= 0) {
          box.obs.disconnect();
          delete window.__dockMutations;
        }
      })
      .catch(() => {});
  let last = await mutations(true);
  let quietSince = Date.now();
  try {
    while (Date.now() - start < timeoutMs) {
      await new Promise((resolve) => setTimeout(resolve, 100));
      const n = await mutations(false);
      if (n === -1 || n !== last || (inflight?.size ?? 0) > 0) {
        last = n;
        quietSince = Date.now();
        continue;
      }
      if (Date.now() - quietSince >= quietMs) return { settled: true, ms: Date.now() - start };
    }
    return { settled: false, ms: Date.now() - start, pendingRequests: inflight?.size ?? 0 };
  } finally {
    await leave();
  }
}

/** 定位超时、而且根本没找到元素（call log 里没有 `resolved to`）：那时页面上有什么。 */
async function pageNowFor(page, error) {
  if (!/Timeout \d+ms exceeded/.test(error) || /resolved to/.test(error)) return null;
  try {
    const tree = await ariaTree(page, "body", { timeout: 2000 });
    const chars = Array.from(tree);
    return chars.length > SNAPSHOT_ON_ERROR_MAX
      ? `${chars.slice(0, SNAPSHOT_ON_ERROR_MAX).join("")}\n…(truncated, ${chars.length} chars)`
      : tree;
  } catch {
    return null;
  }
}

/** 脚本跑完时活动页的网址和标题（给模型确认跳到了哪儿，省一次截图）。页卡住时不等。 */
async function pageState(page) {
  if (!page || page.isClosed()) return null;
  const title = await Promise.race([
    page.title().catch(() => ""),
    new Promise((resolve) => setTimeout(() => resolve(""), 1000)),
  ]);
  return { url: page.url(), title };
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
  const params = ["page", "context", "browser", "state", "log", "snapshot", "screenshot", "docs", "onDialog", "tabs", "readPages", "settle"];
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
  const snapshot = (selector = "body", target = page) => ariaTree(target, selector);
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
  /** tabs.new 开的页（按 target id）：回话时只报还开着的，关掉的不用 Rust 侧去收。 */
  const openedPages = new Map();
  const stillOpen = () => opened.filter((id) => !openedPages.get(id)?.isClosed());
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
      openedPages.set(id, p);
      watch(p);
      if (url) await p.goto(url);
      return p;
    },
    async close(which) {
      const p = typeof which === "number" ? await pageAt(which) : which;
      const id = await targetIdOf(p);
      if (!order.includes(id)) throw new Error("tabs.close: not one of this session's tabs");
      if (order.length === 1) throw new Error("tabs.close: this is the session's last tab (browser_close closes the session)");
      await p.close();
      order.splice(order.indexOf(id), 1);
      if (activate === id) activate = null;
      return order.length;
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
  /** 每页在飞的请求（settle 用）。 */
  const inflight = new Map();
  // Service Worker 发的请求没有 frame（`frame()` 会抛错）：不归哪一页，不算。
  const pageOfRequest = (r) => {
    try {
      return r.frame().page();
    } catch {
      return null;
    }
  };
  const onRequest = (r) => {
    if (LONG_LIVED.has(r.resourceType())) return;
    inflight.get(pageOfRequest(r))?.add(r);
  };
  const onRequestDone = (r) => inflight.get(pageOfRequest(r))?.delete(r);
  watch = (p) => {
    if (watched.has(p)) return;
    watched.add(p);
    inflight.set(p, new Set());
    p.on("dialog", dialogListener);
    p.on("request", onRequest);
    p.on("requestfinished", onRequestDone);
    p.on("requestfailed", onRequestDone);
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
  const settle = (opts = {}) => {
    const p = opts.page ?? page;
    watch(p);
    return settleOn(p, inflight.get(p), opts);
  };
  const script = fn(page, page.context(), browser, state, log, snapshot, screenshot, docs, onDialog, tabs, readPages, settle);
  script.then(
    () => busy.delete(req.session),
    () => busy.delete(req.session),
  );
  // 跑完之后的活动页：tabs.select 指定的那页，否则还是开始时那页。
  const activePage = async () => (activate ? await pageFor(browser, activate).catch(() => null) : page);
  try {
    const out = await Promise.race([script.then((value) => ({ value })), dialog.then((d) => ({ dialog: d })), timeout]);
    // 对话框开着时页面不响应，不去读标题。
    const state = out.dialog ? null : await pageState(await activePage());
    return { ok: true, result: clip(show(out.value)), logs, images, dialog: out.dialog, opened: stillOpen(), activate, page: state, hints: hintsFor(req.code) };
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
    const error = clip(plain(e?.stack?.split("\n").slice(0, 6).join("\n") ?? String(e)));
    const current = await activePage();
    return {
      ok: false,
      error,
      logs,
      images,
      opened: stillOpen(),
      activate,
      page: await pageState(current),
      pageNow: current ? await pageNowFor(current, error) : null,
      hints: hintsFor(req.code),
    };
  } finally {
    clearTimeout(timer);
    for (const p of watched) {
      p.off("dialog", dialogListener);
      p.off("request", onRequest);
      p.off("requestfinished", onRequestDone);
      p.off("requestfailed", onRequestDone);
    }
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
