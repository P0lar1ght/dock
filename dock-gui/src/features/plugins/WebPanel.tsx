// 插件自带的 web 界面（`surface/web`）：沙箱 iframe + 窄桥。契约见 docs/PLUGIN-VIEWS.md「web 面板」。
//
// 安全：
// - iframe 只给 `allow-scripts`（不同源：碰不到本窗口的 DOM、存储、Tauri IPC）；
// - 文档里注入 CSP：不许联网（connect / img / font 只认 data: blob:），脚本样式只能内联；
// - 和 Dock 只经 postMessage 桥说话，桥只放行下面白名单里的几个读方法 + 本面板的动作。
//
// 桥（页面里是 `window.dock`）：
// - `dock.call("threads.list")` → `thread/list` 的结果；
// - `dock.call("threads.history", { threadId })` → `thread/history` 的结果（完整事件流）；
// - `dock.call("threads.watch", { threadId })` → `{ live }`：之后这个会话的实时事件经 `dock.on("event")` 推来；
//   `live: false` 表示收不到实时推送（主窗口里只转发已打开的会话），插件自己隔一会儿重拉历史；
// - `dock.call("panel.action", { action })` → 本面板的 `surface/action`；
// - `dock.activeThread`：主窗口当前会话 id（独立窗里是 null），变了推 `dock.on("activeThread")`。

import { useEffect, useMemo, useRef, useState } from "react";
import { dock } from "../../dock/client";
import { surfaceAction, surfaceWeb } from "./api";

/** 注入进插件文档的 CSP：不许联网，只许内联脚本 / 样式。 */
const CSP =
  "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data: blob:; font-src data:; media-src data: blob:";

/** 给插件用的设计 token（和 GUI 同一套），插件可以直接用这些变量。 */
const BASE_CSS = `
:root {
  --dock-bg: #fafafa; --dock-surface: #ffffff; --dock-fill: #f0f0f2; --dock-hover: #ededf0; --dock-selected: #e8e8ec;
  --dock-line: #e5e5ea; --dock-line-soft: #ececf0;
  --dock-ink: #1c1c1e; --dock-ink-2: #3a3a3c; --dock-ink-3: #636366; --dock-ink-4: #8e8e93; --dock-ink-5: #c7c7cc;
  --dock-accent: #0a84ff; --dock-success: #248a3d; --dock-warning: #c93400; --dock-danger: #d70015;
  --dock-font: -apple-system, BlinkMacSystemFont, "SF Pro Text", "PingFang SC", "Helvetica Neue", sans-serif;
  --dock-mono: ui-monospace, "SF Mono", SFMono-Regular, Menlo, monospace;
}
html, body { margin: 0; background: var(--dock-bg); color: var(--dock-ink); font: 13px/1.5 var(--dock-font); }
`;

/** 页面里的桥客户端：`window.dock`。 */
const BRIDGE_JS = `
(() => {
  let seq = 0;
  const pending = new Map();
  const listeners = { event: new Set(), activeThread: new Set() };
  const dock = {
    activeThread: null,
    call(method, params) {
      const id = ++seq;
      parent.postMessage({ dockBridge: 1, id, method, params: params || {} }, "*");
      return new Promise((resolve, reject) => pending.set(id, { resolve, reject }));
    },
    on(name, fn) {
      (listeners[name] || (listeners[name] = new Set())).add(fn);
      return () => listeners[name].delete(fn);
    },
  };
  addEventListener("message", (e) => {
    const m = e.data;
    if (!m || m.dockBridge !== 1 || e.source !== parent) return;
    if (m.id && pending.has(m.id)) {
      const p = pending.get(m.id);
      pending.delete(m.id);
      m.error ? p.reject(new Error(m.error)) : p.resolve(m.result);
    } else if (m.push === "event") {
      listeners.event.forEach((fn) => fn(m.payload));
    } else if (m.push === "activeThread") {
      dock.activeThread = m.payload;
      listeners.activeThread.forEach((fn) => fn(m.payload));
    }
  });
  window.dock = dock;
})();
`;

/** 把 CSP、基础样式、桥塞进插件文档的 <head>（没有 <head> 就放最前面）。 */
function wrap(html: string, activeThread: string | null): string {
  const head = `<meta http-equiv="Content-Security-Policy" content="${CSP}"><meta charset="utf-8"><style>${BASE_CSS}</style><script>${BRIDGE_JS}window.dock.activeThread=${JSON.stringify(activeThread)};</script>`;
  const m = /<head[^>]*>/i.exec(html);
  if (m) return html.slice(0, m.index + m[0].length) + head + html.slice(m.index + m[0].length);
  return `<!doctype html><html><head>${head}</head><body>${html}</body></html>`;
}

export function WebPanel({
  surfaceId,
  activeThread,
  detached,
  onClosed,
}: {
  surfaceId: string;
  /** 主窗口当前会话；独立窗里是 null。 */
  activeThread: string | null;
  /** 在独立窗里：自己有一条连接，可以真订阅会话。 */
  detached: boolean;
  /** 插件在动作里关了面板。 */
  onClosed: () => void;
}) {
  const [html, setHtml] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const frame = useRef<HTMLIFrameElement>(null);
  const watched = useRef(new Set<string>());
  const active = useRef(activeThread);
  active.current = activeThread;
  const closed = useRef(onClosed);
  closed.current = onClosed;

  useEffect(() => {
    let alive = true;
    surfaceWeb(surfaceId).then(
      (h) => alive && (h === null ? setError("插件没有给出 web 界面") : setHtml(h)),
      (e) => alive && setError(e instanceof Error ? e.message.replace(/^[\w/]+: /, "") : String(e)),
    );
    return () => {
      alive = false;
    };
  }, [surfaceId]);

  // 文档只在 HTML 变了时重建；当前会话变化走推送，不重载页面。
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const doc = useMemo(() => (html === null ? null : wrap(html, active.current)), [html]);

  const post = (msg: Record<string, unknown>) => frame.current?.contentWindow?.postMessage({ dockBridge: 1, ...msg }, "*");

  useEffect(() => {
    post({ push: "activeThread", payload: activeThread });
  }, [activeThread]);

  useEffect(() => {
    const onMessage = async (e: MessageEvent) => {
      const win = frame.current?.contentWindow;
      const m = e.data as { dockBridge?: number; id?: number; method?: string; params?: Record<string, unknown> };
      if (!win || e.source !== win || m?.dockBridge !== 1 || typeof m.id !== "number") return;
      const reply = (result: unknown, error?: string) => win.postMessage({ dockBridge: 1, id: m.id, result, error }, "*");
      const params = m.params ?? {};
      const threadId = typeof params.threadId === "string" ? params.threadId : "";
      try {
        switch (m.method) {
          case "threads.list":
            // 同 GUI 侧栏：所有项目的会话（不带 scope 只回当前页）。
            return reply(await dock.call("thread/list", { scope: "all" }));
          case "threads.history":
            if (!threadId) return reply(null, "threadId 不能为空");
            return reply(await dock.call("thread/history", { threadId }));
          case "threads.watch": {
            if (!threadId) return reply(null, "threadId 不能为空");
            watched.current.add(threadId);
            // 独立窗自己一条连接，真订阅；主窗口只转发已打开的会话（不往 GUI 的订阅里塞别的）。
            if (detached) {
              await dock.call("thread/subscribe", { threadId });
              return reply({ live: true });
            }
            return reply({ live: threadId === active.current });
          }
          case "panel.action": {
            const action = typeof params.action === "string" ? params.action : "";
            if (!action) return reply(null, "action 不能为空");
            const r = await surfaceAction(surfaceId, action);
            if (r.closed) closed.current();
            return reply({ closed: r.closed });
          }
          default:
            return reply(null, `桥不支持 ${String(m.method)}`);
        }
      } catch (err) {
        reply(null, err instanceof Error ? err.message.replace(/^[\w/]+: /, "") : String(err));
      }
    };
    window.addEventListener("message", onMessage);
    const off = dock.onNotification((n) => {
      const id = typeof n.params.threadId === "string" ? n.params.threadId : "";
      if (id && watched.current.has(id)) post({ push: "event", payload: { threadId: id, method: n.method, params: n.params } });
    });
    return () => {
      window.removeEventListener("message", onMessage);
      off();
    };
  }, [surfaceId, detached]);

  if (error) return <div className="p-4 text-xs text-[#d70015]">web 界面打不开：{error}</div>;
  if (doc === null) return <div className="m-4 h-24 animate-pulse rounded-lg bg-fill" />;
  return (
    <iframe
      ref={frame}
      title="插件 web 面板"
      srcDoc={doc}
      sandbox="allow-scripts"
      className="enter-fade h-full w-full flex-1 border-0 bg-pane"
    />
  );
}
