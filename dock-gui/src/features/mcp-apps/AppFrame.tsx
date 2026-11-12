// 一个 MCP App 界面：沙箱 iframe + 官方 AppBridge（@modelcontextprotocol/ext-apps，规范 2026-01-26）。
//
// 安全：
// - iframe 只给 `allow-scripts`（不同源：碰不到本窗口的 DOM、存储、Tauri IPC）；
// - 文档最前面注入 CSP（dock-core appCsp）：默认不联网，资源 `_meta.ui.csp` 声明的来源才放开；
// - 界面只经 AppBridge 说话：调工具走 `mcp/app/call`（Dock 只认同一服务器、visibility 含 app 的工具），
//   读资源只认同一服务器的 `ui://`，发消息走输入框同一条路（在跑就排队）。
//
// 调用（工具输入 / 结果）是声明式的：`call` 换了就先发 tool-input，结果到了再发 tool-result。
// 面板里一个界面会陆续收到多次调用。

import { useEffect, useMemo, useRef, useState } from "react";
import { AlertTriangle } from "lucide-react";
import { AppBridge, PostMessageTransport, type McpUiDisplayMode, type McpUiHostContext } from "@modelcontextprotocol/ext-apps/app-bridge";
import { appAllow, appCsp, cleanSources, viewToolsForExpose, type McpAppResource, type McpAppTool } from "dock-core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { invoke } from "@tauri-apps/api/core";
import { IN_TAURI } from "../../components/chrome";
import { store } from "../../dock/store";
import { currentClient, dock } from "../../dock/client";
import { appCall, appContext, appRead, appResource, appViewExpose, handleViewCalls, viewToolsSupported } from "./api";

/** 交给界面的一次调用。`result` 到了才发 tool-result；`cancelled` 发 tool-cancelled。 */
export interface FrameCall {
  /** 换了就是新的一次调用（工具调用 id）。 */
  key: string;
  args: Record<string, unknown>;
  result?: Record<string, unknown> | null;
  cancelled?: string | null;
}

export interface AppFrameProps {
  server: string;
  uri: string;
  /** 界面里的调用、上下文记在哪个会话上；没有会话时界面只能看。 */
  threadId: string | null;
  /** 创建这个界面的那颗工具（`hostContext.toolInfo`）；面板从菜单打开时没有。 */
  tool: McpAppTool | null;
  call: FrameCall | null;
  displayMode: McpUiDisplayMode;
  /** 界面请求换显示方式：回实际给的。 */
  onRequestMode: (mode: McpUiDisplayMode) => McpUiDisplayMode;
  /** 内联时跟着界面报的高度走；面板里铺满。 */
  fill?: boolean;
  className?: string;
}

const HOST_INFO = { name: "Dock", version: "0.1.0" };

/** 规范的主题变量（和 GUI 同一套颜色）。界面可以不用。 */
const STYLE_VARIABLES = {
  "--color-background-primary": "#ffffff",
  "--color-background-secondary": "#fafafa",
  "--color-background-tertiary": "#f0f0f2",
  "--color-background-inverse": "#1c1c1e",
  "--color-background-info": "#e8f2ff",
  "--color-background-danger": "#fef2f2",
  "--color-background-success": "#ecfdf3",
  "--color-background-warning": "#fff8e6",
  "--color-text-primary": "#1c1c1e",
  "--color-text-secondary": "#3a3a3c",
  "--color-text-tertiary": "#636366",
  "--color-text-inverse": "#ffffff",
  "--color-text-info": "#0a84ff",
  "--color-text-danger": "#d70015",
  "--color-text-success": "#248a3d",
  "--color-text-warning": "#c93400",
  "--color-border-primary": "#e5e5ea",
  "--color-border-secondary": "#ececf0",
  "--color-ring-primary": "#0a84ff",
  "--font-sans": '-apple-system, BlinkMacSystemFont, "SF Pro Text", "PingFang SC", "Helvetica Neue", sans-serif',
  "--font-mono": 'ui-monospace, "SF Mono", SFMono-Regular, Menlo, monospace',
  "--border-radius-sm": "6px",
  "--border-radius-md": "8px",
  "--border-radius-lg": "12px",
} as Record<string, string>;

/**
 * 沙箱里（只有 allow-scripts，不同源）碰 localStorage / sessionStorage 会抛 SecurityError。
 * 第三方界面常拿它存视图状态（官方 map 示例就是）：换成只在内存里的一份，重载后没了，但不报错。
 */
const STORAGE_SHIM = `<script>(()=>{const m=()=>{const d=new Map();return{get length(){return d.size},clear:()=>d.clear(),getItem:k=>d.has(String(k))?d.get(String(k)):null,key:i=>[...d.keys()][i]??null,removeItem:k=>{d.delete(String(k))},setItem:(k,v)=>{d.set(String(k),String(v))}}};for(const n of["localStorage","sessionStorage"]){try{void window[n].length}catch{Object.defineProperty(window,n,{value:m(),configurable:true})}}})();</script>`;

/** 把 CSP 和存储垫片塞进界面文档 `<head>` 的最前面（没有 `<head>` 就补一个）。 */
function wrap(html: string, csp: string): string {
  const meta = `<meta http-equiv="Content-Security-Policy" content="${csp.replace(/"/g, "&quot;")}"><meta charset="utf-8">${STORAGE_SHIM}`;
  const m = /<head[^>]*>/i.exec(html);
  if (m) return html.slice(0, m.index + m[0].length) + meta + html.slice(m.index + m[0].length);
  const doctype = /^\s*<!doctype[^>]*>/i.exec(html);
  if (doctype) return doctype[0] + `<head>${meta}</head>` + html.slice(doctype[0].length);
  return `<!doctype html><html><head>${meta}</head><body>${html}</body></html>`;
}

function containerOf(el: HTMLIFrameElement | null, mode: McpUiDisplayMode): McpUiHostContext["containerDimensions"] {
  const box = el?.parentElement?.getBoundingClientRect();
  const width = Math.round(box?.width ?? 0);
  if (mode === "inline") return width > 0 ? { width, maxHeight: 720 } : { maxHeight: 720 };
  const height = Math.round(box?.height ?? 0);
  return width > 0 && height > 0 ? { width, height } : { maxHeight: 720 };
}

/**
 * 界面跑在哪：Tauri 里交给 src-tauri 的 mcpapp:// 协议，每台服务器一个真实源（能给
 * allow-same-origin，Worker、存储都正常）；浏览器里（无头测试）退回 srcdoc（不透明源）。
 * localStorage 里 `dock-gui.mcpAppSrcdoc = 1` 强制走 srcdoc（对照测试用）。
 */
async function hostedSrc(server: string, html: string, csp: string): Promise<string | null> {
  if (!IN_TAURI || localStorage.getItem("dock-gui.mcpAppSrcdoc") === "1") return null;
  return invoke<string>("mcp_app_doc", { origin: originKey(server), html, csp });
}

/** 源键：服务器名的安全字符 + 8 位哈希（同名不同写法也分得开），能当主机名的一段。 */
function originKey(server: string): string {
  let h = 0x811c9dc5;
  for (let i = 0; i < server.length; i++) h = Math.imul(h ^ server.charCodeAt(i), 0x01000193) >>> 0;
  const slug = server.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 40) || "app";
  return `${slug}-${h.toString(16).padStart(8, "0")}`;
}

const errText = (e: unknown) => (e instanceof Error ? e.message.replace(/^[\w/]+: /, "") : String(e));

function textOf(content: unknown): string {
  if (!Array.isArray(content)) return "";
  return content
    .map((b) => (b && typeof b === "object" && (b as { type?: unknown }).type === "text" ? String((b as { text?: unknown }).text ?? "") : ""))
    .filter(Boolean)
    .join("\n");
}

export function AppFrame({ server, uri, threadId, tool, call, displayMode, onRequestMode, fill = false, className = "" }: AppFrameProps) {
  const [resource, setResource] = useState<McpAppResource | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const [doc, setDoc] = useState<string | null>(null);
  const [src, setSrc] = useState<string | null>(null);
  const [ready, setReady] = useState(false);
  const [height, setHeight] = useState(320);
  const frame = useRef<HTMLIFrameElement>(null);
  const bridge = useRef<AppBridge | null>(null);
  // 处理函数里读最新值，不为它们重建桥。
  const live = useRef({ threadId, onRequestMode, displayMode });
  live.current = { threadId, onRequestMode, displayMode };

  useEffect(() => {
    let alive = true;
    setResource(null);
    setError(null);
    appResource(server, uri).then(
      (r) => alive && setResource(r),
      (e) => alive && setError(errText(e)),
    );
    return () => {
      alive = false;
    };
  }, [server, uri, attempt]);

  const csp = useMemo(() => (resource ? appCsp(resource.csp) : ""), [resource]);

  // 桥先连上，再给 iframe 文档：界面一加载就会发 `ui/initialize`。
  useEffect(() => {
    const win = frame.current?.contentWindow;
    if (!resource || !win) return;
    const hostContext: McpUiHostContext = {
      theme: "light",
      platform: "desktop",
      locale: navigator.language,
      timeZone: Intl.DateTimeFormat().resolvedOptions().timeZone,
      displayMode: live.current.displayMode,
      availableDisplayModes: ["inline", "fullscreen"],
      // 内联：宽度跟对话流、高度最多 720（界面报尺寸，宿主照着撑开）；面板：铺满，量出来的实际尺寸。
      containerDimensions: containerOf(frame.current, live.current.displayMode),
      styles: { variables: STYLE_VARIABLES as McpUiHostContext["styles"] extends { variables?: infer V } ? V : never },
      ...(tool
        ? {
            toolInfo: {
              tool: { name: tool.name, description: tool.description, inputSchema: { type: "object", ...tool.inputSchema } },
            },
          }
        : {}),
    } as McpUiHostContext;
    const b = new AppBridge(
      null,
      HOST_INFO,
      {
        openLinks: {},
        serverTools: {},
        serverResources: {},
        logging: {},
        updateModelContext: { text: {}, structuredContent: {} },
        message: { text: {} },
        sandbox: {
          csp: resource.csp
            ? {
                connectDomains: cleanSources(resource.csp.connectDomains),
                resourceDomains: cleanSources(resource.csp.resourceDomains),
                frameDomains: cleanSources(resource.csp.frameDomains),
                baseUriDomains: cleanSources(resource.csp.baseUriDomains),
              }
            : {},
        },
      },
      { hostContext },
    );
    const needThread = () => {
      const id = live.current.threadId;
      if (!id) throw new Error("先打开一个会话再用这个界面");
      return id;
    };
    b.oncalltool = async (params) =>
      (await appCall(needThread(), server, params.name, (params.arguments ?? {}) as Record<string, unknown>)) as never;
    // 界面读自己那台服务器的资源（server 按实例定死，不用 iframe 传来的）。
    b.onreadresource = async (params) => {
      const raw = await appRead(server, params.uri);
      if (raw) return raw as never;
      // 旧版 Dock 没有 mcp/app/read：只能读 ui:// 界面。
      if (!params.uri.startsWith("ui://")) throw new Error("这个版本的 Dock 只能给界面读 ui:// 资源");
      const r = await appResource(server, params.uri);
      return { contents: [{ uri: params.uri, mimeType: r.mimeType ?? "text/html;profile=mcp-app", text: r.html }] };
    };
    b.onmessage = async (params) => {
      const text = textOf(params.content).trim();
      const id = live.current.threadId;
      if (!text || !id) return { isError: true };
      return (await store.send(id, text)) ? {} : { isError: true };
    };
    b.onopenlink = async (params) => {
      if (!/^https?:\/\//i.test(params.url)) return { isError: true };
      await openUrl(params.url).catch(() => window.open(params.url, "_blank", "noopener"));
      return {};
    };
    b.onupdatemodelcontext = async (params) => {
      await appContext(
        needThread(),
        server,
        uri,
        params.content as unknown[] | undefined,
        params.structuredContent as Record<string, unknown> | undefined,
      );
      return {};
    };
    b.onrequestdisplaymode = async (params) => ({ mode: live.current.onRequestMode(params.mode) });
    b.onsizechange = (params) => {
      if (typeof params.height === "number" && params.height > 0) setHeight(Math.min(720, Math.max(48, Math.ceil(params.height))));
    };
    b.onloggingmessage = (params) => console.debug(`[mcp-app ${server}]`, params.level, params.data);
    b.oninitialized = () => setReady(true);
    let alive = true;
    b.connect(new PostMessageTransport(win, win)).then(
      async () => {
        const html = wrap(resource.html, csp);
        const hosted = await hostedSrc(server, html, csp).catch(() => null);
        if (!alive) return;
        if (hosted) setSrc(hosted);
        else setDoc(html);
      },
      (e) => alive && setError(errText(e)),
    );
    bridge.current = b;
    const client = currentClient();
    return () => {
      alive = false;
      setReady(false);
      setDoc(null);
      setSrc(null);
      bridge.current = null;
      // 切走了就别收尾：桥上的调用会打到现在的前台。界面卸下，这条连接上的租约留着。
      if (currentClient() !== client) return;
      void b.teardownResource({}).catch(() => {});
      void b.close().catch(() => {});
    };
    // `tool` 只用来填初始上下文，换调用不重建界面。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [resource, csp, server, uri]);

  // 界面自己登记了工具（appCapabilities.tools）：报给 Dock，这个会话的模型就能调；工具表变了
  // 再报，重连后（租约换了）再报，卸下时收回。报和收回排成一队，晚到的报不会盖掉收回。
  useEffect(() => {
    const b = bridge.current;
    if (!ready || !b || !threadId || !viewToolsSupported() || !b.getAppCapabilities()?.tools) return;
    const viewId = crypto.randomUUID();
    let alive = true;
    let queue: Promise<unknown> = Promise.resolve();
    const report = (list: () => Promise<unknown>) => {
      queue = queue
        .then(async () => {
          const listed = await list();
          await appViewExpose({ threadId, viewId, server, resourceUri: uri, tools: viewToolsForExpose(listed) });
        })
        .catch((e) => console.warn(`[mcp-app ${server}] 界面工具报不上去`, e));
    };
    const relist = () => {
      if (alive) report(() => b.listTools({}).catch(() => null));
    };
    relist();
    b.setNotificationHandler("notifications/tools/list_changed", relist);
    const offCalls = handleViewCalls(viewId, async (call) =>
      (await b.callTool({ name: call.name, arguments: call.arguments })) as Record<string, unknown>,
    );
    const client = currentClient();
    const offState = client.onState((s) => s === "open" && relist());
    return () => {
      alive = false;
      offCalls();
      offState();
      if (currentClient() === client) report(async () => null);
    };
  }, [ready, threadId, server, uri]);

  // 显示方式变了（内联 → 面板）告诉界面。
  useEffect(() => {
    if (ready) void bridge.current?.sendHostContextChange({ displayMode, containerDimensions: containerOf(frame.current, displayMode) });
  }, [ready, displayMode]);

  // 调用：先发输入，结果到了再发结果。
  const sentInput = useRef<string | null>(null);
  const sentResult = useRef<string | null>(null);
  useEffect(() => {
    const b = bridge.current;
    if (!ready || !b || !call) return;
    if (sentInput.current !== call.key) {
      sentInput.current = call.key;
      sentResult.current = null;
      void b.sendToolInput({ arguments: call.args });
    }
    if (call.result && sentResult.current !== call.key) {
      sentResult.current = call.key;
      void b.sendToolResult(call.result as never);
    } else if (call.cancelled && sentResult.current !== call.key) {
      sentResult.current = call.key;
      void b.sendToolCancelled({ reason: call.cancelled });
    }
  }, [ready, call]);
  // 重连桥（换会话 / 重试）后要重新发。
  useEffect(() => {
    if (!ready) {
      sentInput.current = null;
      sentResult.current = null;
    }
  }, [ready]);

  if (error) {
    return (
      <div className={`enter-fade flex flex-col items-start gap-1.5 rounded-lg border border-[#fecaca] bg-[#fef6f6] px-3 py-2.5 ${className}`}>
        <span className="flex items-center gap-1.5 text-[12px] font-medium text-[#c0392b]">
          <AlertTriangle size={13} />
          界面打不开
        </span>
        <span className="text-[11.5px] text-[#b91c1c]">{error}</span>
        <button type="button" onClick={() => setAttempt((n) => n + 1)} className="motion-colors text-[11.5px] font-medium text-accent hover:underline">
          重试
        </button>
      </div>
    );
  }
  const bordered = resource?.prefersBorder !== false;
  return (
    <div
      className={`relative overflow-hidden ${fill ? "flex min-h-0 flex-1" : "rounded-lg"} ${bordered && !fill ? "border border-line-strong bg-white" : ""} ${className}`}
      style={fill ? undefined : { height }}
    >
      {!ready && <div className="absolute inset-0 animate-pulse bg-fill" />}
      <iframe
        ref={frame}
        title={`${server} 界面`}
        src={src ?? undefined}
        srcDoc={src ? undefined : (doc ?? undefined)}
        // 独立源（mcpapp://）才给 allow-same-origin：源和 GUI 不同，碰不到 GUI；srcdoc 是不透明源，不能给。
        sandbox={src ? "allow-scripts allow-same-origin" : "allow-scripts"}
        allow={appAllow(resource?.permissions ?? null) || undefined}
        className={`h-full w-full border-0 ${ready ? "enter-fade" : "opacity-0"}`}
      />
    </div>
  );
}

/** 连着的 Dock 支不支持 MCP Apps（旧版没有能力位）。 */
export const mcpAppsSupported = () => !!dock.capabilities.mcpApps;
