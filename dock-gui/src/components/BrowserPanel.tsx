// 浏览器面板：看会话正在用的 Chromium 标签页，能接手操作。设计：wonder-ui/pages/BrowserPanel。
//
// 画面走 dock.1 的 `browser/view/*`（网关截屏流）：WKWebView 没有 CDP，远程 Dock 也连不到本机
// 的 Chromium，所以不直连浏览器。视图是连接级的：断线重连、换会话都要重开。
//
// agent 没在跑时像普通浏览器一样直接点、滚、打字；agent 跑的时候只读，点「接手操作」才发给页面。
// Dock 不锁页面：接手时 agent 这一轮还在跑的话两边会同时操作，界面要说清楚，不假装「暂停了 agent」。
//
// 网关支持 `browserViewport` 时：页面视口跟着面板大小走（按设备像素比推流，Retina 上清楚），
// 会话还没开网页时地址栏能直接开（经浏览器 MCP，和 agent 共用这一页）。
//
// 网关支持 `browserTabs` 时头栏是标签栏（BrowserTabs）：会话的全部标签页，能切、新开、关，
// 和 agent 共用当前页。状态和「接手 / 交还」挪到地址栏右边。

import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent, type PointerEvent, type ReactNode, type WheelEvent } from "react";
import {
  AppWindow,
  ArrowLeft,
  ArrowRight,
  Check,
  CircleAlert,
  CornerUpLeft,
  Globe,
  Lock,
  MoreVertical,
  MousePointer2,
  RefreshCw,
  RotateCw,
  TriangleAlert,
} from "lucide-react";
import type { ThreadState } from "dock-core";
import { dock, RpcError, type Notification } from "../dock/client";
import { store, useApp, useThread } from "../dock/store";
import { BrowserTabs, type BrowserTab } from "./BrowserTabs";
import { BrowserMenu } from "./BrowserMenu";
import { Popover } from "./Popover";
import { IconButton } from "./chrome";
import { calledTool } from "./toolCall";
import { imeComposing } from "../dock/ime";

const MCP_PREFIX = "mcp_browser__";
/** 没标签页 / 浏览器退出时多久再试一次（agent 打开网页后自己出画面）。 */
const RETRY_MS = 1500;
/** 面板大小停下来多久才改页面视口（拖分隔线时不跟着来回重排）。 */
const RESIZE_SETTLE_MS = 220;
/** 页面视口最窄这么宽（CSS 像素）：常见桌面站点（B 站、微博）的最小宽度都在这以内。 */
const MIN_VIEWPORT_WIDTH = 800;
const OPEN_DELAY_MS = 120;
const EXAMPLE = "打开 github.com/trending，总结今天的热门项目";

type Phase =
  | { kind: "idle" }
  | { kind: "connecting" }
  | { kind: "live"; viewId: string }
  | { kind: "no_tab" }
  | { kind: "exited" }
  | { kind: "unsupported" }
  | { kind: "error"; message: string };

interface FrameSize {
  width: number;
  height: number;
}

interface Viewport {
  width: number;
  height: number;
  deviceScaleFactor: number;
}

/** `browser/view/input` 的回包：按下落在 `<select>` 上回 `select`，⌘C / ⌘X 回 `copied`。 */
interface InputResult {
  ok?: boolean;
  select?: SelectPicker;
  copied?: string;
}

/** 网关回的下拉框：位置是页面 CSS 像素，选项照页面顺序（分组名在 `group`）。 */
interface SelectPicker {
  x: number;
  y: number;
  width: number;
  height: number;
  selectedIndex: number;
  options: { label: string; disabled: boolean; group: string | null }[];
}

interface Queued {
  event: Record<string, unknown>;
  done?: (result: InputResult | null) => void;
}

/** 一个标签页自己的画面设置：切回来还是这一套（缩放、自适应、页面尺寸）。 */
interface TabView {
  /** 画面缩放：1 = 铺满面板。 */
  zoom: number;
  /** 自适应：页面视口跟着面板大小走。 */
  fit: boolean;
  /** 记住的页面视口（关掉自适应、或选了固定尺寸时用它）。 */
  viewport: Viewport | null;
}

export function BrowserPanel({
  sessionId,
  onClose,
  lead,
  controls,
}: {
  sessionId: string | null;
  onClose: () => void;
  /** 头栏最左边（分屏 / 独立窗口时的拖手、红绿灯占位）。 */
  lead?: ReactNode;
  /** 关闭按钮左边（分屏菜单、在新窗口打开）。 */
  controls?: ReactNode;
}) {
  const app = useApp();
  const thread = useThread(sessionId);
  const running = !!sessionId && app.running.has(sessionId);
  /** Agent 请你接手（handoff_to_user 的提问还没答）：这一轮虽然在跑，但它在等你，不在动页面。 */
  const waitingForUser = !!sessionId && running && store.pendingHandoff(sessionId);
  /** Agent 真的在动这个页面：在跑，而且不是在等你。 */
  const agentBusy = running && !waitingForUser;
  const stopping = !!sessionId && app.stopping.has(sessionId);
  const connected = app.connection === "open";

  const [phase, setPhase] = useState<Phase>({ kind: "idle" });
  const [page, setPage] = useState({ url: "", title: "" });
  /** 会话的标签页（网关 `browser/view/tabs`）；老网关没有就一直是空的。 */
  const [tabs, setTabs] = useState<BrowserTab[]>([]);
  /** 点了「+」：新页开出来后把地址栏变成输入框（每次加一）。 */
  const [focusAddress, setFocusAddress] = useState(0);
  const [hasFrame, setHasFrame] = useState(false);
  /** 这个会话出过画面：浏览器退出后还能把最后一帧压暗留着。 */
  const [seenFrame, setSeenFrame] = useState(false);
  const [control, setControl] = useState(false);
  const [attempt, setAttempt] = useState(0);
  /** 点开的下拉框（网关回的选项）；选了或关了是 null。 */
  const [picker, setPicker] = useState<SelectPicker | null>(null);
  /** Dock 的浏览器是有头的（配置 `[browser] headed`）：真窗口可以单独操作。 */
  const [headed, setHeaded] = useState(false);
  const img = useRef<HTMLImageElement>(null);
  const size = useRef<FrameSize>({ width: 0, height: 0 });
  /** 收到的帧号：解码完只换最新的那张。 */
  const frameSeq = useRef(0);
  /** 当前视图；`browser/view/open` 还没回时是 null。 */
  const view = useRef<string | null>(null);
  const viewId = phase.kind === "live" ? phase.viewId : null;
  /** 网关能让页面视口跟着面板走、能替会话开页。 */
  const follow = connected && !!dock.capabilities.browserViewport;
  /** 画面区域（地址栏以下）；量它定页面视口。 */
  const area = useRef<HTMLDivElement>(null);
  /** 最近量到的视口：开视图时带上，开着时变了就发 resize。 */
  const viewport = useRef<Viewport | null>(null);
  /** 当前要的视口（渲染用）：画面按它的大小画，帧换了也不缩放。 */
  const [want, setWant] = useState<Viewport | null>(null);
  /** 画面区域的尺寸：算「铺满面板」的缩放比用。 */
  const [areaSize, setAreaSize] = useState({ width: 0, height: 0 });
  /** 画面缩放：1 = 铺满面板。 */
  const [zoom, setZoom] = useState(1);
  /** 自适应：页面视口跟着面板大小走（关掉就用这个标签页记住的尺寸）。 */
  const [fit, setFit] = useState(true);
  /** 每个标签页自己的设置：切回去还是这一套。 */
  const tabViews = useRef(new Map<string, TabView>());
  /** `fit` 的同步副本：量尺寸的回调里要用（那个 effect 只在连上 / 断开时重跑）。 */
  const fitRef = useRef(true);
  /** 当前活动的标签页：它的设置是画面现在这套。 */
  const activeTab = tabs.find((t) => t.active)?.targetId ?? null;

  /** 让页面按这个视口渲染（并记成当前标签页的尺寸）。 */
  const applyViewport = useCallback((next: Viewport) => {
    viewport.current = next;
    setWant(next);
    const id = view.current;
    if (id) dock.call("browser/view/resize", { viewId: id, ...next }).catch(() => {});
  }, []);

  /** 记住当前标签页的设置（缩放 / 自适应 / 尺寸）。 */
  const rememberTab = useCallback((targetId: string | null, patch: Partial<TabView>) => {
    if (!targetId) return;
    const before = tabViews.current.get(targetId) ?? { zoom: 1, fit: true, viewport: null };
    tabViews.current.set(targetId, { ...before, ...patch });
  }, []);

  useEffect(() => {
    setSeenFrame(false);
    setPage({ url: "", title: "" });
    setTabs([]);
    // 换会话：每个标签页的设置也重来（缩放、自适应、页面尺寸）。
    tabViews.current.clear();
    setZoom(1);
    fitRef.current = true;
    setFit(true);
    if (img.current) img.current.removeAttribute("src");
  }, [sessionId]);

  // 一轮结束就不用「接手」了：空闲时本来就能直接操作。
  useEffect(() => {
    if (!running) setControl(false);
  }, [running]);

  // 推送：帧直接写到 <img>（不进 React 状态，免得每帧重渲染）。
  // 按会话收：网关在回 open 之前就可能推出首帧，而静止的页面不会再推第二帧。
  useEffect(() => {
    if (!sessionId) return;
    return dock.onNotification((n: Notification) => {
      if (!n.method.startsWith("browser/view/")) return;
      const p = n.params as Record<string, unknown>;
      if (p.threadId !== sessionId || (view.current && p.viewId !== view.current)) return;
      // 被同一会话更新的 open 顶掉的旧视图（网关 #158 起会推）：不是当前这个就不管。
      if (n.method === "browser/view/closed" && p.reason === "replaced" && p.viewId !== view.current) return;
      if (n.method === "browser/view/frame") {
        // 先在后台解码好再换上去，主线程不卡在解码上；解码期间来了更新的帧，这张就丢掉。
        const seq = ++frameSeq.current;
        const url = `data:${p.mime ?? "image/jpeg"};base64,${p.data}`;
        const frameSize = { width: Number(p.width) || 0, height: Number(p.height) || 0 };
        const show = () => {
          if (seq !== frameSeq.current || !img.current) return;
          size.current = frameSize;
          img.current.src = url;
          setHasFrame(true);
          setSeenFrame(true);
        };
        const next = new Image();
        next.src = url;
        next.decode().then(show, show);
      } else if (n.method === "browser/view/status") {
        setPage({ url: String(p.url ?? ""), title: String(p.title ?? "") });
      } else if (n.method === "browser/view/tabs") {
        setTabs(Array.isArray(p.tabs) ? (p.tabs as BrowserTab[]) : []);
      } else if (n.method === "browser/view/closed") {
        view.current = null;
        setTabs([]);
        setControl(false);
        setHasFrame(false);
        setPhase(p.reason === "browser_exited" ? { kind: "exited" } : { kind: "no_tab" });
      }
    });
  }, [sessionId]);

  // 开视图：换会话、重连、重试都走这里。
  useEffect(() => {
    setControl(false);
    setHasFrame(false);
    setPicker(null);
    view.current = null;
    if (!sessionId || !connected) {
      setPhase({ kind: "idle" });
      return;
    }
    if (!dock.capabilities.browserView) {
      setPhase({ kind: "unsupported" });
      return;
    }
    let alive = true;
    setPhase((p) => (p.kind === "no_tab" || p.kind === "exited" ? p : { kind: "connecting" }));
    const openView = () =>
      dock
        .call<{ viewId: string; url: string; title: string; tabs?: BrowserTab[]; headed?: boolean }>(
          "browser/view/open",
          openParams(sessionId, follow ? viewport.current : null, pendingUrl.current),
        )
        .then(
          (r) => {
            if (!alive) {
              dock.call("browser/view/close", { viewId: r.viewId }).catch(() => {});
              return;
            }
            pendingUrl.current = null;
            view.current = r.viewId;
            setPage({ url: r.url, title: r.title });
            setTabs(r.tabs ?? []);
            setHeaded(r.headed === true);
            setPhase({ kind: "live", viewId: r.viewId });
          },
          (e) => {
            pendingUrl.current = null;
            if (!alive) return;
            const code = e instanceof RpcError ? e.code : undefined;
            // 网关说这次 open 已被更新的那次取代：等那次的结果。
            if (code === "superseded") return;
            if (code === "no_tab") setPhase({ kind: "no_tab" });
            else if (code === "browser_unavailable") setPhase({ kind: "exited" });
            else setPhase({ kind: "error", message: e instanceof Error ? e.message : String(e) });
          },
        );
    // 稍等再开：快速切会话（和开发模式 StrictMode 的双跑）只发最后一次。网关对同一会话只留
    // 一个视图（#158 之前，两次 open 并发时后完成的会把先开的那个悄悄关掉）。
    const timer = window.setTimeout(() => {
      if (alive) openView();
    }, OPEN_DELAY_MS);
    return () => {
      alive = false;
      window.clearTimeout(timer);
      const opened = view.current;
      view.current = null;
      if (opened) dock.call("browser/view/close", { viewId: opened }).catch(() => {});
    };
    // `follow` 只在连上 / 断开时变，跟 `connected` 一起重开；视口变了走 resize，不重开。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionId, connected, attempt]);

  // 量画面区域：停下来 RESIZE_SETTLE_MS 才定，拖分隔线（html.split-dragging）时等松手。
  useEffect(() => {
    const el = area.current;
    if (!el || !follow) return;
    let timer = 0;
    const settle = () => {
      window.clearTimeout(timer);
      timer = window.setTimeout(() => {
        if (document.documentElement.classList.contains("split-dragging")) return settle();
        setAreaSize({ width: el.clientWidth, height: el.clientHeight });
        // 关掉自适应：页面尺寸是这个标签页记住的那个，面板变了也不改。
        if (!fitRef.current) return;
        const next = measure(el);
        if (!next || sameViewport(next, viewport.current)) return;
        applyViewport(next);
      }, RESIZE_SETTLE_MS);
    };
    const first = measure(el);
    viewport.current = first;
    setWant(first);
    setAreaSize({ width: el.clientWidth, height: el.clientHeight });
    const ro = new ResizeObserver(settle);
    ro.observe(el);
    return () => {
      ro.disconnect();
      window.clearTimeout(timer);
    };
  }, [follow, applyViewport]);

  /** 地址栏在没有标签页时开页：带着网址重开视图，网关替会话开这一页。 */
  const pendingUrl = useRef<string | null>(null);
  const openUrl = (url: string) => {
    pendingUrl.current = url;
    setPhase({ kind: "connecting" });
    setAttempt((n) => n + 1);
  };

  // 没标签页 / 浏览器没了：隔一会儿再试，agent 一开网页画面就出来。
  useEffect(() => {
    if (phase.kind !== "no_tab" && phase.kind !== "exited") return;
    const t = window.setTimeout(() => setAttempt((n) => n + 1), RETRY_MS);
    return () => window.clearTimeout(t);
  }, [phase, attempt]);

  // 输入按顺序一个一个发：网关把每个请求放进各自的任务里跑，连发的按下 / 抬起可能乱序到 Chrome，
  // 点击就丢了。排队时连续的鼠标移动只留最新一个；连续的滚轮并成一个（位移相加）——触控板一秒
  // 六十来个滚轮事件，一个个等回包会越积越多，手停了画面还在追。
  const queue = useMemo(() => ({ busy: false, items: [] as Queued[] }), [viewId]);
  const send = useCallback(
    (event: Record<string, unknown>, done?: (result: InputResult | null) => void) => {
      if (!viewId) return done?.(null);
      const last = queue.items[queue.items.length - 1]?.event;
      if (isMove(event) && last && isMove(last)) queue.items[queue.items.length - 1] = { event };
      else if (last && sameWheel(event, last))
        queue.items[queue.items.length - 1] = {
          event: {
            ...event,
            deltaX: Number(last.deltaX) + Number(event.deltaX),
            deltaY: Number(last.deltaY) + Number(event.deltaY),
          },
        };
      else queue.items.push({ event, done });
      if (queue.busy) return;
      queue.busy = true;
      void (async () => {
        while (queue.items.length) {
          const next = queue.items.shift()!;
          const result = await dock.call<InputResult>("browser/view/input", { viewId, event: next.event }).then(
            (r) => r,
            (e) => {
              console.warn("browser/view/input", e);
              // 视图在网关那边已经没了（被顶掉、连接换过）：重开一个，别让用户对着死画面点。
              if (e instanceof RpcError && e.code === "not_found") {
                for (const item of queue.items) item.done?.(null);
                queue.items.length = 0;
                setAttempt((n) => n + 1);
              }
              return null;
            },
          );
          // 按下落在 <select> 上：网关回选项（原生弹层画不进画面），这里画菜单。
          if (result?.select) setPicker(result.select);
          next.done?.(result);
        }
        queue.busy = false;
      })();
    },
    [viewId, queue],
  );

  /** 下拉菜单选了（或关了）：选了就把那一项设回页面。 */
  const pick = useCallback(
    (index: number | null) => {
      setPicker(null);
      if (index !== null) send({ type: "select", index });
    },
    [send],
  );

  const navigate = (params: Record<string, unknown>) => {
    if (!viewId) return;
    dock.call("browser/view/navigate", { viewId, ...params }).catch(store.reportError);
  };

  /** 标签栏：切 / 关 / 新开都经网关调浏览器 MCP，画面和标签栏等推送跟上。 */
  const tabAction = (params: Record<string, unknown>) => {
    if (!viewId) return Promise.resolve();
    return dock.call("browser/view/tab", { viewId, ...params }).then(() => {}, store.reportError);
  };
  const supportsTabs = connected && !!dock.capabilities.browserTabs;

  /** 切到某个标签页：把那个页自己的设置拿回来（缩放、自适应、页面尺寸）。 */
  const applyTabView = useCallback(
    (targetId: string) => {
      const saved = tabViews.current.get(targetId);
      const nextZoom = saved?.zoom ?? 1;
      const nextFit = saved?.fit ?? true;
      setZoom(nextZoom);
      fitRef.current = nextFit;
      setFit(nextFit);
      // 自适应：跟着面板量；固定：用这个标签页记住的尺寸。
      const next = nextFit ? viewport.current : saved?.viewport ?? viewport.current;
      if (next && !sameViewport(next, viewport.current)) applyViewport(next);
    },
    [applyViewport],
  );

  // 活动标签页变了（用户切的，或 agent 切的）：换回那一套设置。
  useEffect(() => {
    if (!activeTab) return;
    applyTabView(activeTab);
  }, [activeTab, applyTabView]);

  /** 画面缩放：1 = 铺满面板。每个标签页各记各的。 */
  const changeZoom = (next: number) => {
    setZoom(next);
    rememberTab(activeTab, { zoom: next });
  };

  /** 自适应（页面视口跟着面板走）/ 固定（用记住的尺寸，面板变了也不改）。 */
  const changeFit = (nextFit: boolean) => {
    fitRef.current = nextFit;
    setFit(nextFit);
    rememberTab(activeTab, { fit: nextFit });
    if (nextFit) {
      const el = area.current;
      const measured = el ? measure(el) : null;
      if (measured) {
        applyViewport(measured);
        rememberTab(activeTab, { viewport: measured });
      }
    } else {
      // 关掉自适应：把现在这个尺寸记下来，之后面板变了也不动它。
      rememberTab(activeTab, { viewport: viewport.current });
    }
  };

  /** 固定尺寸：页面就按这个大小渲染（和面板大小无关）。 */
  const changeSize = (size: { width: number; height: number }) => {
    fitRef.current = false;
    setFit(false);
    const next: Viewport = {
      width: size.width,
      height: size.height,
      deviceScaleFactor: 1,
    };
    applyViewport(next);
    rememberTab(activeTab, { fit: false, viewport: next });
  };

  /** 强制重新加载：绕过缓存重新加载这一页。 */
  const hardReload = () => {
    if (!viewId) return;
    dock.call("browser/view/navigate", { viewId, action: "hard_reload" }).catch(store.reportError);
  };

  /** 画面在面板里的缩放比：铺满面板是 1（再乘用户选的缩放）。 */
  const scale = useMemo(() => fitScale(want, areaSize, zoom), [want, areaSize, zoom]);

  const agentAction = agentBusy && !control ? lastBrowserAction(thread) : null;
  const title = page.title || hostOf(page.url) || "浏览器";
  const live = phase.kind === "live";
  /** 鼠标键盘发给页面：agent 没在跑，或者你接手了。 */
  const interactive = live && hasFrame && (!agentBusy || control);
  /** 地址栏能用：开着画面时导航；没标签页 / 浏览器退出时直接开页（要网关支持）。 */
  const canOpen = follow && !!sessionId && (phase.kind === "no_tab" || phase.kind === "exited");

  return (
    <aside className="glass-pane enter-fade flex min-h-0 min-w-0 flex-1 flex-col bg-white">
      <BrowserTabs
        tabs={live ? tabs : []}
        locked={!interactive}
        agentHere={agentBusy && !control && hasFrame}
        fallback={live ? title : "浏览器"}
        lead={lead}
        controls={controls}
        onSwitch={(targetId) =>
          void tabAction({ action: "switch", targetId }).then(() => applyTabView(targetId))
        }
        onCloseTab={(targetId) => void tabAction({ action: "close", targetId })}
        onNew={supportsTabs ? () => void tabAction({ action: "new" }).then(() => setFocusAddress((n) => n + 1)) : undefined}
        onClosePanel={onClose}
      />

      <AddressBar
        url={live || phase.kind === "exited" ? page.url : ""}
        enabled={live}
        canOpen={canOpen}
        placeholder={
          canOpen
            ? "搜索或输入网址，在这个会话的浏览器里打开"
            : phase.kind === "no_tab"
              ? "Agent 打开网页后这里显示地址"
              : live
                ? "搜索或输入网址"
                : ""
        }
        onNavigate={(params) => (live ? navigate(params) : typeof params.url === "string" && openUrl(params.url))}
        focusSignal={focusAddress}
      >
        <StatusChip phase={phase} control={control} running={agentBusy} waiting={waitingForUser} hasFrame={hasFrame} />
        {live && headed && (
          <button
            type="button"
            title="Dock 的浏览器是有头的：切到它的窗口直接操作"
            onClick={() => viewId && dock.call("browser/view/focus", { viewId }).catch(store.reportError)}
            className="motion-colors flex h-7 shrink-0 items-center gap-1.5 rounded-lg border border-line-strong bg-white px-2.5 hover:bg-hover"
          >
            <AppWindow size={14} className="text-ink-2" />
            <span className="text-xs font-medium text-ink">在窗口中打开</span>
          </button>
        )}
        {live && hasFrame && agentBusy && !control && (
          <button
            type="button"
            onClick={() => setControl(true)}
            className="motion-colors flex h-7 shrink-0 items-center gap-1.5 rounded-lg border border-line-strong bg-white px-2.5 hover:bg-hover"
          >
            <MousePointer2 size={14} className="text-ink-2" />
            <span className="text-xs font-medium text-ink">接手操作</span>
          </button>
        )}
        {control && agentBusy && (
          <button
            type="button"
            onClick={() => setControl(false)}
            className="motion-colors flex h-7 shrink-0 items-center gap-1.5 rounded-lg bg-accent px-2.5 hover:bg-[#0077ed]"
          >
            <CornerUpLeft size={14} className="text-white" />
            <span className="text-xs font-medium text-white">交还 Agent</span>
          </button>
        )}
        {/* 更多：强制重新加载、缩放、窗口尺寸（每个标签页各记各的）。 */}
        <Popover
          title="浏览器菜单"
          side="bottom"
          align="right"
          className="motion-colors flex size-7 items-center justify-center rounded-md text-ink-3 hover:bg-hover hover:text-ink"
          panelClassName="w-[252px]"
          trigger={<MoreVertical size={16} />}
        >
          {() => (
            <BrowserMenu
              zoom={zoom}
              fit={fit}
              canHardReload={connected && !!dock.capabilities.browserHardReload}
              onZoom={changeZoom}
              onFit={changeFit}
              onPreset={changeSize}
              onHardReload={hardReload}
            />
          )}
        </Popover>
      </AddressBar>

      <div
        ref={area}
        className={`relative flex min-h-0 flex-1 flex-col ${
          phase.kind === "no_tab" ? "bg-sidebar" : follow ? "bg-white" : "bg-[#f2f2f4]"
        }`}
      >
        {live && control && agentBusy && (
          <div className="enter-fade absolute inset-x-3 top-3 z-10 flex items-center gap-2 rounded-lg border border-[#ffe0a8] bg-[#fff8eb] px-3 py-2 shadow-[0px_4px_14px_rgba(0,0,0,0.06)]">
            <TriangleAlert size={14} className="shrink-0 text-[#b25e00]" />
            <span className="flex-1 text-xs text-[#7a4100]">Agent 这一轮还在跑，可能和你同时操作这个页面。</span>
            <button
              type="button"
              disabled={stopping}
              onClick={() => sessionId && store.cancel(sessionId)}
              className="shrink-0 text-xs font-medium text-[#0a6fd6] hover:underline disabled:text-ink-4 disabled:no-underline"
            >
              {stopping ? "正在停止…" : "停止这一轮"}
            </button>
          </div>
        )}
        {(live || (phase.kind === "exited" && seenFrame)) && (
          <Screen
            img={img}
            size={size}
            box={want && scale > 0 ? { width: want.width * scale, height: want.height * scale } : null}
            control={interactive}
            flush={follow}
            dimmed={phase.kind === "exited"}
            send={send}
            waiting={live && !hasFrame}
            picker={interactive ? picker : null}
            onPick={pick}
          />
        )}
        {(phase.kind === "connecting" || (live && !hasFrame)) && <Skeleton />}
        {phase.kind === "no_tab" && <NoTab sessionId={sessionId} canOpen={canOpen} />}
        {phase.kind === "exited" && <Exited onRetry={() => setAttempt((n) => n + 1)} />}
        {phase.kind === "idle" && <Notice text={sessionId ? "等待连接 Dock…" : "先选一个会话"} />}
        {phase.kind === "unsupported" && <Notice text="当前 Dock 不支持浏览器画面（缺 browser/view），请更新 Dock 后重启。" />}
        {phase.kind === "error" && <Notice text={phase.message} onRetry={() => setAttempt((n) => n + 1)} />}
        {agentAction && live && hasFrame && (
          <div className="enter-up pointer-events-none absolute bottom-7 left-1/2 flex max-w-[90%] -translate-x-1/2 items-center gap-2 rounded-full bg-[#1c1c1ee6] py-1.5 pl-3 pr-3.5 shadow-[0px_6px_20px_rgba(0,0,0,0.18)]">
            <span className="relative size-2 shrink-0">
              <span className="pulse-halo absolute inset-0 rounded-full bg-accent" />
              <span className="absolute inset-0 rounded-full bg-accent" />
            </span>
            <span className="shrink-0 text-xs font-medium text-white">Agent 正在操作</span>
            <span className="min-w-0 truncate font-mono text-xs text-white/70">{agentAction}</span>
          </div>
        )}
      </div>

      <footer className="flex h-[30px] shrink-0 items-center gap-2 border-t border-line-soft px-4 text-[11px] text-ink-4">
        {footerText(phase, interactive, agentBusy, waitingForUser, hasFrame, size.current, zoom)}
      </footer>
    </aside>
  );
}

function StatusChip({
  phase,
  control,
  running,
  waiting,
  hasFrame,
}: {
  phase: Phase;
  control: boolean;
  running: boolean;
  waiting: boolean;
  hasFrame: boolean;
}) {
  let chip: { text: string; bg: string; fg: string; dot: string } | null = null;
  if (phase.kind === "live" && control) chip = { text: "你在操作", bg: "bg-ink", fg: "text-white", dot: "bg-white" };
  else if (phase.kind === "live" && hasFrame && waiting)
    chip = { text: "等你操作", bg: "bg-[#fff4dc]", fg: "text-[#8a5a00]", dot: "bg-[#ff9f0a]" };
  else if (phase.kind === "live" && hasFrame && running)
    chip = { text: "Agent 操作中", bg: "bg-[#eaf3ff]", fg: "text-[#0a6fd6]", dot: "bg-accent" };
  else if (phase.kind === "live" && hasFrame) chip = { text: "可操作", bg: "bg-fill", fg: "text-ink-3", dot: "bg-[#34c759]" };
  else if (phase.kind === "connecting" || (phase.kind === "live" && !hasFrame))
    chip = { text: "连接中", bg: "bg-fill", fg: "text-ink-3", dot: "bg-ink-5" };
  else if (phase.kind === "no_tab") chip = { text: "未打开网页", bg: "bg-fill", fg: "text-ink-3", dot: "bg-ink-5" };
  else if (phase.kind === "exited") chip = { text: "已断开", bg: "bg-[#fff0ef]", fg: "text-[#c9342a]", dot: "bg-[#ff453a]" };
  if (!chip) return null;
  return (
    <span className={`motion-colors flex h-6 shrink-0 items-center gap-1.5 rounded-full px-2.5 ${chip.bg}`}>
      <span className={`size-1.5 rounded-full ${chip.dot}`} />
      <span className={`text-xs font-medium ${chip.fg}`}>{chip.text}</span>
    </span>
  );
}

function AddressBar({
  url,
  enabled,
  canOpen,
  placeholder,
  onNavigate,
  focusSignal,
  children,
}: {
  url: string;
  enabled: boolean;
  /** 还没有页面：只能输网址开页（前进后退刷新不可用）。 */
  canOpen: boolean;
  placeholder: string;
  onNavigate: (params: Record<string, unknown>) => void;
  /** 变了（点了新标签页）就变成空的输入框等你输网址。 */
  focusSignal: number;
  /** 右边：状态、接手 / 交还。 */
  children?: ReactNode;
}) {
  const [draft, setDraft] = useState<string | null>(null);
  const input = useRef<HTMLInputElement>(null);
  // 空白页时输入框本来就在（autoFocus 不会再触发）：等它渲染出来再手动聚焦。
  useEffect(() => {
    if (focusSignal === 0) return;
    setDraft("");
    const frame = requestAnimationFrame(() => input.current?.focus());
    return () => cancelAnimationFrame(frame);
  }, [focusSignal]);
  // 空白页不显示 about:blank，当没有地址。
  if (url === "about:blank") url = "";
  const { host, rest, secure } = splitUrl(url);
  const nav = (action: string) => () => onNavigate({ action });
  return (
    <div className="flex h-11 shrink-0 items-center gap-1 border-b border-line-soft bg-white px-3">
      <IconButton icon={ArrowLeft} label="后退" size={16} onClick={nav("back")} disabled={!enabled} className="size-7" />
      <IconButton icon={ArrowRight} label="前进" size={16} onClick={nav("forward")} disabled={!enabled} className="size-7" />
      <IconButton icon={RotateCw} label="刷新" size={14} onClick={nav("reload")} disabled={!enabled} className="size-7" />
      <label className="ml-1 flex h-8 min-w-0 flex-1 cursor-text items-center gap-2 rounded-lg border border-line-strong bg-white px-3 focus-within:border-accent focus-within:ring-2 focus-within:ring-[#0a84ff26]">
        {secure && draft === null && <Lock size={12} className="shrink-0 text-ink-4" />}
        {draft === null && url && !canOpen ? (
          // 平时：主机名深色、路径浅色；点一下变成可编辑的整串地址。
          <button
            type="button"
            disabled={!enabled}
            onClick={() => setDraft(url)}
            className="flex min-w-0 flex-1 items-baseline text-left text-[13px]"
          >
            <span className="shrink-0 text-ink">{host}</span>
            <span className="min-w-0 truncate text-ink-4">{rest}</span>
          </button>
        ) : (
          <input
            ref={input}
            autoFocus={draft !== null}
            disabled={!enabled && !canOpen}
            value={draft ?? ""}
            placeholder={placeholder}
            spellCheck={false}
            onChange={(e) => setDraft(e.target.value)}
            onBlur={() => setDraft(null)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && draft?.trim()) {
                onNavigate({ url: draft.trim() });
                setDraft(null);
                e.currentTarget.blur();
              } else if (e.key === "Escape") {
                setDraft(null);
                e.currentTarget.blur();
              }
            }}
            className="min-w-0 flex-1 bg-transparent text-[13px] text-ink outline-none placeholder:text-ink-5"
          />
        )}
      </label>
      {children && <div className="ml-2 flex shrink-0 items-center gap-2">{children}</div>}
    </div>
  );
}

/**
 * 画面在面板里的缩放比：1 = 正好铺满面板（再乘用户选的缩放）。
 * 只看「要的视口」和面板大小：画面（帧）换了尺寸也不改这个比值，所以不会忽大忽小。
 */
export function fitScale(
  want: Viewport | null,
  area: { width: number; height: number },
  zoom: number,
): number {
  const base =
    want && area.width && area.height
      ? Math.min(area.width / want.width, area.height / want.height)
      : 1;
  return base * zoom;
}

/**
 * 画面按 contain 画在盒子里时，真正画出来的那块（客户端像素）。
 * 盒子定了大小、画面换了尺寸也不影响这里：只是画出来的位置变一点，比例不变。
 */
export function contentRect(el: HTMLImageElement | null, size: FrameSize) {
  if (!el || !size.width || !size.height) return null;
  const r = el.getBoundingClientRect();
  if (!r.width || !r.height) return null;
  const k = Math.min(r.width / size.width, r.height / size.height);
  if (!k) return null;
  const width = size.width * k;
  const height = size.height * k;
  return {
    left: r.left + (r.width - width) / 2,
    top: r.top + (r.height - height) / 2,
    width,
    height,
    /** 客户端像素 / 页面像素。 */
    k,
  };
}

/** 画面。接手时把鼠标 / 滚轮 / 键盘换算成页面视口的 CSS 像素发出去。 */
function Screen({
  img,
  size,
  box,
  control,
  flush,
  dimmed,
  send,
  waiting,
  picker,
  onPick,
}: {
  img: React.RefObject<HTMLImageElement | null>;
  size: React.RefObject<FrameSize>;
  /** 画面盒子（客户端像素）：页面视口按缩放比画出来的大小。定了它，帧换了也不缩放。 */
  box: { width: number; height: number } | null;
  control: boolean;
  /** 页面视口就是这块区域：铺满、不画卡片边框，像内嵌的浏览器。 */
  flush: boolean;
  dimmed: boolean;
  send: (event: Record<string, unknown>, done?: (result: InputResult | null) => void) => void;
  waiting: boolean;
  /** 点开的下拉框：在它下面画菜单。 */
  picker: SelectPicker | null;
  /** 选了第几项；`null` = 没选就关了。 */
  onPick: (index: number | null) => void;
}) {
  const keys = useRef<HTMLTextAreaElement>(null);
  const composing = useRef(false);
  const lastMove = useRef(0);

  // 不在「可操作」时自动抢焦点：空闲时一直可操作，抢了焦点会把对话输入框里打的字打断。
  // 点一下页面才把键盘交给它（见 `mouse`）。

  /** 画面坐标 → 页面 CSS 像素。`clamp`：拖到画面外时夹在边上（按着键拖出去还要跟手）。 */
  const point = (e: { clientX: number; clientY: number }, clamp = false) => {
    const area = contentRect(img.current, size.current);
    if (!area) return null;
    const { width, height } = size.current;
    let x = ((e.clientX - area.left) / area.width) * width;
    let y = ((e.clientY - area.top) / area.height) * height;
    if (x < 0 || y < 0 || x > width || y > height) {
      if (!clamp) return null;
      x = Math.min(Math.max(x, 0), width);
      y = Math.min(Math.max(y, 0), height);
    }
    return { x: Math.round(x), y: Math.round(y) };
  };

  const mouse = (action: string) => (e: PointerEvent<HTMLImageElement>) => {
    if (!control) return;
    // 按下时捕获指针：拖出画面再松手也收得到抬起，页面不会以为键一直按着。
    const captured = e.currentTarget.hasPointerCapture(e.pointerId);
    const p = point(e, captured || action === "up");
    if (!p) return;
    if (action === "down") e.currentTarget.setPointerCapture(e.pointerId);
    if (action === "move") {
      // 移动只在按住或悬停时有意义；限到 ~30 次/秒，别把连接塞满。
      const now = performance.now();
      if (now - lastMove.current < 33) return;
      lastMove.current = now;
    } else {
      e.preventDefault();
      keys.current?.focus();
    }
    send({
      type: "mouse",
      action,
      ...p,
      button: action === "move" ? "none" : buttonName(e.button),
      // 此刻按着的键：拖动时的移动要带上，不然页面当成悬停，拖选文字、拖滑块都不动。
      buttons: e.buttons,
      clickCount: action === "move" ? 0 : e.detail || 1,
      modifiers: modifiers(e),
    });
  };

  const wheel = (e: WheelEvent) => {
    if (!control) return;
    const p = point(e);
    if (!p) return;
    // 画面是缩着显示的：位移换成页面像素，滚动跟着手指走。
    const area = contentRect(img.current, size.current);
    const k = area?.k || 1;
    send({ type: "wheel", ...p, deltaX: e.deltaX / k, deltaY: e.deltaY / k, modifiers: modifiers(e) });
  };

  const keyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (composing.current || imeComposing(e)) return;
    const mod = e.metaKey || e.ctrlKey;
    // 留给 App 的快捷键（侧栏、浏览器面板、设置）和系统粘贴（走 input 事件插文字）。
    if (e.metaKey && ["b", "t", ",", "v"].includes(e.key.toLowerCase())) return;
    // 可打印字符交给 textarea 的 input 事件（输入法也走那条）；其余键直接发。
    if (e.key.length === 1 && !mod) return;
    e.preventDefault();
    const key = { type: "key", action: "press", key: e.key, modifiers: modifiers(e) };
    // ⌘C / ⌘X：页面的剪贴板在 Dock 那台机器上，网关把选中的字带回来，写进这边的剪贴板。
    // 写入要在这次按键里同步发起（WebKit 只认用户手势里的写入）：先交一个等回包的 Promise。
    const copying = mod && !e.altKey && ["c", "x"].includes(e.key.toLowerCase());
    if (copying && typeof ClipboardItem !== "undefined" && navigator.clipboard?.write) {
      const text = new Promise<Blob>((resolve, reject) =>
        send(key, (r) =>
          // 没选中东西就不动剪贴板：reject 让这次写入作废，原来的内容留着。
          r?.copied ? resolve(new Blob([r.copied], { type: "text/plain" })) : reject(new Error("nothing copied")),
        ),
      );
      navigator.clipboard.write([new ClipboardItem({ "text/plain": text })]).catch(() => {});
      return;
    }
    send(key);
  };

  const flushText = () => {
    const el = keys.current;
    if (!el || composing.current) return;
    if (el.value) send({ type: "text", text: el.value });
    el.value = "";
  };

  return (
    <div className={`flex min-h-0 flex-1 overflow-auto ${flush ? "" : "p-3"}`}>
      <div
        style={box ? { width: box.width, height: box.height } : undefined}
        className={`glass-keep motion-colors relative m-auto flex overflow-hidden bg-white ${
          box
            ? ""
            : `max-h-full max-w-full ${
                flush
                  ? ""
                  : control
                    ? "rounded-[10px] ring-2 ring-accent shadow-[0px_0px_0px_6px_rgba(10,132,255,0.12)]"
                    : "rounded-[10px] border border-line-strong shadow-[0px_1px_2px_rgba(0,0,0,0.04),0px_8px_24px_rgba(0,0,0,0.06)]"
              }`
        }`}
      >
        <img
          ref={img}
          alt=""
          draggable={false}
          onPointerDown={mouse("down")}
          onPointerUp={mouse("up")}
          onPointerMove={mouse("move")}
          onWheel={wheel}
          onContextMenu={(e) => control && e.preventDefault()}
          className={`block select-none object-contain ${box ? "h-full w-full" : "max-h-full max-w-full"} ${
            control ? "cursor-default" : "cursor-not-allowed"
          } ${dimmed ? "opacity-35 grayscale" : ""} ${waiting ? "invisible" : ""}`}
        />
        {picker && <SelectMenu picker={picker} img={img} size={size} onPick={onPick} />}
        {control && (
          <textarea
            ref={keys}
            aria-label="发给页面的键盘输入"
            onKeyDown={keyDown}
            onInput={flushText}
            onCompositionStart={() => (composing.current = true)}
            onCompositionEnd={() => {
              composing.current = false;
              flushText();
            }}
            className="pointer-events-none absolute left-0 top-0 h-px w-px resize-none opacity-0"
          />
        )}
      </div>
    </div>
  );
}

function Skeleton() {
  return (
    <div className="absolute inset-0 flex p-3">
      <div className="relative flex flex-1 flex-col gap-3 rounded-[10px] border border-line-strong bg-white p-5">
        <div className="h-4 w-[120px] rounded-md bg-[#f0f0f3]" />
        <div className="h-9 w-full rounded-lg bg-[#f5f5f7]" />
        <div className="h-12 w-full rounded-[10px] bg-[#f5f5f7]" />
        <div className="h-12 w-full rounded-[10px] bg-[#f5f5f7]" />
        <div className="h-12 w-4/5 rounded-[10px] bg-[#f5f5f7]" />
        <div className="absolute inset-0 flex items-center justify-center">
          <div className="enter-fade flex items-center gap-2.5 rounded-full border border-line-strong bg-white px-4 py-2 shadow-[0px_6px_20px_rgba(0,0,0,0.08)]">
            <span className="size-4 animate-spin rounded-full border-2 border-[#d6e8ff] border-t-accent" />
            <span className="text-[13px] font-medium text-ink-2">正在连接会话的浏览器…</span>
          </div>
        </div>
      </div>
    </div>
  );
}

function NoTab({ sessionId, canOpen }: { sessionId: string | null; canOpen: boolean }) {
  return (
    <div className="enter-fade absolute inset-0 flex flex-col items-center justify-center gap-3 px-10">
      <div className="flex size-12 items-center justify-center rounded-full border border-line-strong bg-white shadow-[0px_2px_8px_rgba(0,0,0,0.05)]">
        <Globe size={20} className="text-ink-3" />
      </div>
      <span className="text-[15px] font-semibold text-ink">这个会话还没打开网页</span>
      <span className="max-w-[360px] text-center text-[13px] leading-[1.55] text-ink-3">
        {canOpen
          ? "在上面的地址栏输入网址或搜索词直接打开，或者让 Agent 用浏览器工具打开。你和 Agent 用的是同一个浏览器，登录一次两边都能用。"
          : "Agent 用浏览器工具打开网页后，画面会实时出现在这里。你可以随时接手操作。"}
      </span>
      {sessionId && (
        <button
          type="button"
          onClick={() => store.fillComposer(sessionId, EXAMPLE)}
          className="motion-colors mt-1 flex items-center gap-2 rounded-[10px] border border-line-strong bg-white px-3.5 py-2.5 hover:bg-hover"
        >
          <span className="text-[13px] text-ink-2">「{EXAMPLE}」</span>
          <span className="shrink-0 text-[13px] font-medium text-[#0a6fd6]">填入输入框</span>
        </button>
      )}
    </div>
  );
}

function Exited({ onRetry }: { onRetry: () => void }) {
  return (
    <div className="enter-fade absolute inset-0 flex items-center justify-center bg-white/40">
      <div className="glass-dialog pop-in flex flex-col items-center gap-2.5 rounded-[14px] border border-line-strong bg-white px-8 py-6 shadow-[0px_12px_32px_rgba(0,0,0,0.10)]">
        <div className="flex size-10 items-center justify-center rounded-full bg-[#fff0ef]">
          <CircleAlert size={20} className="text-[#ff453a]" />
        </div>
        <span className="text-[15px] font-semibold text-ink">浏览器已退出</span>
        <span className="max-w-[300px] text-center text-[13px] leading-[1.55] text-ink-3">
          Chromium 被关掉或崩溃了。Agent 下次用浏览器工具时会重新启动它。
        </span>
        <button
          type="button"
          onClick={onRetry}
          className="motion-colors mt-1 flex h-8 items-center gap-1.5 rounded-lg bg-ink px-3.5 hover:bg-ink-2"
        >
          <RefreshCw size={14} className="text-white" />
          <span className="text-[13px] font-medium text-white">重新连接</span>
        </button>
      </div>
    </div>
  );
}

function Notice({ text, onRetry }: { text: string; onRetry?: () => void }) {
  return (
    <div className="enter-fade absolute inset-0 flex flex-col items-center justify-center gap-3 px-10">
      <span className="max-w-[380px] text-center text-[13px] leading-[1.55] text-ink-3">{text}</span>
      {onRetry && (
        <button type="button" onClick={onRetry} className="text-[13px] font-medium text-[#0a6fd6] hover:underline">
          重试
        </button>
      )}
    </div>
  );
}

function footerText(
  phase: Phase,
  interactive: boolean,
  running: boolean,
  waiting: boolean,
  hasFrame: boolean,
  size: FrameSize,
  zoom: number,
) {
  if (phase.kind === "live" && hasFrame)
    return (
      <>
        <span className="font-mono">
          {Math.round(size.width)} × {Math.round(size.height)}
        </span>
        <span>· 实时画面</span>
        {zoom !== 1 && <span className="font-mono">· 缩放 {Math.round(zoom * 100)}%</span>}
        <span className="flex-1" />
        <span>
          {waiting
            ? "Agent 在等你 · 直接在这里操作，做完在对话里选「已完成」"
            : interactive && running
            ? "接手中 · 点击、滚动、打字都发给这个页面"
            : interactive
              ? "点击、滚动、打字都发给这个页面"
              : "Agent 在操作 · 只读观看，接手后才会发给页面"}
        </span>
      </>
    );
  if (phase.kind === "exited") return <span>画面停在最后一帧</span>;
  if (phase.kind === "no_tab") return <span>浏览器工具来自内置 MCP「browser」</span>;
  if (phase.kind === "connecting" || phase.kind === "live") return <span>首帧到达前显示骨架</span>;
  return null;
}

/** 这一轮里 agent 最近一次浏览器工具：`browser_click 筛选「直飞」`。 */
function lastBrowserAction(thread: ThreadState): string | null {
  const turn = thread.turns[thread.turns.length - 1];
  if (!turn) return null;
  for (let i = turn.items.length - 1; i >= 0; i--) {
    const item = turn.items[i];
    if (item.kind !== "tool") continue;
    const call = calledTool(item);
    if (!call.name.startsWith(MCP_PREFIX)) continue;
    const name = call.name.slice(MCP_PREFIX.length);
    const args = call.args;
    const detail = ["url", "element", "text", "key", "ref", "selector"]
      .map((k) => args[k])
      .find((v) => typeof v === "string" && v.trim());
    return detail ? `${name} ${String(detail).slice(0, 60)}` : name;
  }
  return null;
}

function splitUrl(url: string): { host: string; rest: string; secure: boolean } {
  try {
    const u = new URL(url);
    if (u.protocol === "http:" || u.protocol === "https:")
      return { host: u.host, rest: `${u.pathname === "/" ? "" : u.pathname}${u.search}${u.hash}`, secure: u.protocol === "https:" };
  } catch {
    // about:blank、data: 之类：整串照原样显示。
  }
  return { host: url, rest: "", secure: false };
}

/**
 * 页面里点开的 `<select>`：原生弹层画不进画面，在画面上按它的位置画一个菜单。
 * 下面放不下、上面更宽敞时往上弹；↑↓ 跳过禁用项，Enter 选，Esc / 点外面关。
 */
function SelectMenu({
  picker,
  img,
  size,
  onPick,
}: {
  picker: SelectPicker;
  img: React.RefObject<HTMLImageElement | null>;
  size: React.RefObject<FrameSize>;
  onPick: (index: number | null) => void;
}) {
  const box = useRef<HTMLDivElement>(null);
  const [active, setActive] = useState(Math.max(0, picker.selectedIndex));
  const [place, setPlace] = useState<React.CSSProperties | null>(null);

  // 页面 CSS 像素 → 画面容器里的位置（画面可能缩放显示）。
  useLayoutEffect(() => {
    const el = img.current;
    const parent = box.current?.parentElement;
    if (!el || !parent || !size.current.width) return;
    const area = contentRect(el, size.current);
    if (!area) return;
    const pr = parent.getBoundingClientRect();
    const k = area.k;
    const left = Math.min(Math.max(area.left - pr.left + picker.x * k, 4), Math.max(4, pr.width - 124));
    const top = area.top - pr.top + (picker.y + picker.height) * k + 2;
    const above = area.top - pr.top + picker.y * k - 2;
    const below = pr.height - top - 4;
    const up = below < 160 && above - 4 > below;
    setPlace({
      left,
      minWidth: Math.max(picker.width * k, 120),
      maxWidth: pr.width - left - 4,
      ...(up ? { bottom: pr.height - above, maxHeight: above - 4 } : { top, maxHeight: below }),
    });
  }, [picker, img, size]);

  useEffect(() => {
    box.current?.focus();
    const outside = (e: MouseEvent) => {
      if (!box.current?.contains(e.target as Node)) onPick(null);
    };
    window.addEventListener("mousedown", outside, true);
    return () => window.removeEventListener("mousedown", outside, true);
  }, [onPick]);

  useEffect(() => {
    box.current?.querySelector(`[data-index="${active}"]`)?.scrollIntoView({ block: "nearest" });
  }, [active, place]);

  const enabled = (i: number) => !!picker.options[i] && !picker.options[i].disabled;
  const step = (from: number, dir: 1 | -1) => {
    for (let i = from + dir; i >= 0 && i < picker.options.length; i += dir) if (enabled(i)) return i;
    return from;
  };
  const onKey = (e: KeyboardEvent<HTMLDivElement>) => {
    const last = picker.options.length;
    if (e.key === "ArrowDown") setActive((a) => step(a, 1));
    else if (e.key === "ArrowUp") setActive((a) => step(a, -1));
    else if (e.key === "Home") setActive(step(-1, 1));
    else if (e.key === "End") setActive(step(last, -1));
    else if (e.key === "Enter" || e.key === " ") enabled(active) && onPick(active);
    else if (e.key === "Escape" || e.key === "Tab") onPick(null);
    else return;
    e.preventDefault();
    e.stopPropagation();
  };

  let group: string | null = null;
  return (
    <div
      ref={box}
      role="listbox"
      tabIndex={-1}
      onKeyDown={onKey}
      style={place ?? { visibility: "hidden" }}
      className="glass-popover pop-in thin-scroll absolute z-20 overflow-y-auto rounded-lg border border-line-strong bg-white py-1 shadow-[0px_8px_24px_rgba(0,0,0,0.14)] outline-none"
    >
      {picker.options.map((o, i) => {
        const header = o.group !== group ? o.group : null;
        group = o.group;
        return (
          <div key={i}>
            {header && <div className="px-3 pb-0.5 pt-1.5 text-[11px] font-medium text-ink-4">{header}</div>}
            <button
              type="button"
              role="option"
              data-index={i}
              aria-selected={i === picker.selectedIndex}
              disabled={o.disabled}
              onMouseEnter={() => !o.disabled && setActive(i)}
              onClick={() => onPick(i)}
              className={`flex w-full items-center gap-2 py-1 pr-3 text-left text-[13px] ${o.group ? "pl-5" : "pl-3"} ${
                o.disabled ? "text-ink-5" : i === active ? "bg-accent text-white" : "text-ink"
              }`}
            >
              <span className="min-w-0 flex-1 truncate">{o.label || "\u00a0"}</span>
              {i === picker.selectedIndex && <Check size={13} className="shrink-0" />}
            </button>
          </div>
        );
      })}
    </div>
  );
}

function hostOf(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return "";
  }
}

function isMove(event: Record<string, unknown>): boolean {
  return event.type === "mouse" && event.action === "move";
}

/** 两个滚轮能并成一个：修饰键一样（⌘ 滚轮是缩放，不能和普通滚动相加）。 */
function sameWheel(a: Record<string, unknown>, b: Record<string, unknown>): boolean {
  return a.type === "wheel" && b.type === "wheel" && String(a.modifiers) === String(b.modifiers);
}

function buttonName(button: number): string {
  return button === 1 ? "middle" : button === 2 ? "right" : "left";
}

function modifiers(e: { altKey: boolean; ctrlKey: boolean; metaKey: boolean; shiftKey: boolean }): string[] {
  const out: string[] = [];
  if (e.altKey) out.push("Alt");
  if (e.ctrlKey) out.push("Control");
  if (e.metaKey) out.push("Meta");
  if (e.shiftKey) out.push("Shift");
  return out;
}

/**
 * 画面区域的 CSS 尺寸 → 页面视口：跟着面板宽度排版，字是正常大小（像把真浏览器窗口拖窄）。
 * 宽度至少 MIN_VIEWPORT_WIDTH：再窄网站多半切成手机布局，那时宁可按这个宽度排版、
 * 整页缩小显示，不会右边被截掉；高度按面板比例跟着放大，画面正好铺满。
 * `deviceScaleFactor` 是补静止帧的倍数：缩小显示后按屏幕实际像素截（到 2 为止，再高帧太大）。
 * 太小（面板正收起）回 null。
 */
function measure(el: HTMLElement): Viewport | null {
  const boxWidth = el.clientWidth;
  const boxHeight = el.clientHeight;
  if (boxWidth < 64 || boxHeight < 64) return null;
  const zoom = Math.max(1, MIN_VIEWPORT_WIDTH / boxWidth);
  const dpr = Math.min(2, Math.max(1, window.devicePixelRatio || 1));
  return {
    width: Math.round(boxWidth * zoom),
    height: Math.round(boxHeight * zoom),
    deviceScaleFactor: Math.max(1, Math.round((dpr / zoom) * 100) / 100),
  };
}

function sameViewport(a: Viewport, b: Viewport | null): boolean {
  return !!b && a.width === b.width && a.height === b.height && a.deviceScaleFactor === b.deviceScaleFactor;
}

/**
 * `browser/view/open` 的参数：跟随面板时带视口（不限画面尺寸），老网关照旧限 1600。
 * 流里的帧质量 60：1280 宽的帧比 80 小约四成，滚动时 WebView 解码跟得上；停下来网关补的
 * 静止帧另按屏幕像素、高质量截，看得清。
 */
function openParams(threadId: string, viewport: Viewport | null, url: string | null): Record<string, unknown> {
  if (!viewport) return { threadId, maxWidth: 1600, maxHeight: 1600, ...(url ? { url } : {}) };
  return { threadId, viewport, quality: 60, ...(url ? { url } : {}) };
}
