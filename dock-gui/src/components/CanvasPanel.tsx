// 画布面板：会话里 agent 用 `canvas_*` 写的 HTML，放进沙箱 iframe 跑。
// 设计：wonder-ui 的 sert（并排面板）、onca（生成中）、khan（运行出错）、mira（交回 agent / 热更新）、
// orby（版本历史回退、源码 / 数据双页签、导出菜单、顶栏标题下拉切画布）。
//
// 数据全走 dock.1 的 `canvas/*`（远程 Dock 也一样）。刷新时机看对话流里 `canvas_*` 工具项：
// 一条跑完就重新拉；只改数据（同一版、HTML 没变）不重载页面，经桥推给 `dock.onData`。
// iframe 只开 allow-scripts（不同源）：页面碰不到 Dock、Tauri 和用户文件，联网照常。

import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import {
  Check,
  ChevronDown,
  CircleAlert,
  Code2,
  Copy,
  Download,
  FileJson,
  FolderOpen,
  History,
  Layers,
  LayoutDashboard,
  RefreshCw,
  RotateCcw,
  Sparkles,
  X,
  type LucideIcon,
} from "lucide-react";
import { dock } from "../dock/client";
import { currentStore, store, useApp, useCanvas, useThread } from "../dock/store";
import type { SessionSummary } from "../model";
import { IconButton } from "./chrome";
import { Popover } from "./Popover";
import { buildDoc, canvasEvents, fixPrompt, sendPrompt, unknownLibs, type FromCanvas } from "./canvasDoc";

interface Version {
  n: number;
  note: string;
  createdMs: number;
  bytes: number;
}

interface Meta {
  id: string;
  title: string;
  createdMs: number;
  updatedMs: number;
  dataUpdatedMs: number;
  latest: number;
  versions: Version[];
}

interface Loaded {
  meta: Meta;
  version: number;
  html: string;
  data: unknown;
  path: string;
}

const EXAMPLE = "把这个项目的目录结构和各语言代码行数做成一块画布，用图表展示";
const SET_DATA_DELAY_MS = 400;

export function CanvasPanel({
  sessionId,
  target,
  onClose,
  lead,
  controls,
}: {
  sessionId: string | null;
  /** 从对话流里的卡片点进来：要看哪块（`seq` 变了才算一次新请求）。 */
  target: { canvasId: string | null; seq: number } | null;
  onClose: () => void;
  /** 头栏最左边（分屏 / 独立窗口时的拖手、红绿灯占位）。 */
  lead?: ReactNode;
  /** 关闭按钮左边（分屏菜单、在新窗口打开）。 */
  controls?: ReactNode;
}) {
  const app = useApp();
  const thread = useThread(sessionId);
  const canvas = useCanvas();
  const connected = app.connection === "open";
  const [list, setList] = useState<Meta[] | null>(null);
  const [currentId, setCurrentId] = useState<string | null>(null);
  /** 在看的版本；`null` = 跟着最新。 */
  const [pinned, setPinned] = useState<number | null>(null);
  const [loaded, setLoaded] = useState<Loaded | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const [view, setView] = useState<"canvas" | "source">("canvas");
  const [reloads, setReloads] = useState(0);
  const [busy, setBusy] = useState(false);

  const events = useMemo(() => canvasEvents(thread), [thread]);
  const doneSig = events
    .filter((e) => e.status === "completed")
    .map((e) => e.itemId)
    .join(",");
  const running = events.filter((e) => e.status === "running");
  const generating = running.find((e) => e.tool === "canvas_create") ?? null;

  // 换会话：从头来。
  useEffect(() => {
    setList(null);
    setCurrentId(null);
    setPinned(null);
    setLoaded(null);
  }, [sessionId]);

  // 卡片点进来：切到那块，看最新版。
  const targetSeq = target?.seq;
  useEffect(() => {
    if (!target) return;
    if (target.canvasId) setCurrentId(target.canvasId);
    setPinned(null);
    setView("canvas");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [targetSeq]);

  // agent 新写完一块：跟过去（和浏览器面板跟着 agent 换标签页一样）。
  const lastDone = events.filter((e) => e.status === "completed" && e.canvasId).slice(-1)[0];
  const lastDoneId = lastDone?.itemId;
  const seen = useRef<string | undefined>(undefined);
  useEffect(() => {
    // 对话流重载时会短暂变空再回来：只有真换了一条才跟。
    if (!lastDone?.canvasId || seen.current === lastDoneId) return;
    const first = seen.current === undefined;
    seen.current = lastDoneId;
    if (first) return;
    setCurrentId(lastDone.canvasId);
    setPinned(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [lastDoneId]);

  const refreshList = useCallback(async () => {
    if (!sessionId) return;
    try {
      const r = await dock.call<{ canvases: Meta[] }>("canvas/list", { threadId: sessionId });
      setList(r.canvases);
      setCurrentId((id) => (id && r.canvases.some((c) => c.id === id) ? id : (r.canvases[0]?.id ?? null)));
      setFailure(null);
    } catch (e) {
      setFailure(e instanceof Error ? e.message : String(e));
    }
  }, [sessionId]);

  useEffect(() => {
    if (connected) void refreshList();
  }, [connected, refreshList, doneSig, reloads]);

  const current = list?.find((c) => c.id === currentId) ?? null;
  const version = pinned ?? current?.latest ?? null;
  const stamp = current ? `${current.latest}:${current.dataUpdatedMs}` : "";

  useEffect(() => {
    if (!sessionId || !currentId || version === null) return;
    let live = true;
    dock
      .call<Omit<Loaded, "meta"> & { canvas: Meta }>("canvas/get", { threadId: sessionId, canvasId: currentId, version })
      .then(({ canvas: meta, ...r }) => live && setLoaded({ ...r, meta }))
      .catch((e) => live && setFailure(e instanceof Error ? e.message : String(e)));
    return () => {
      live = false;
    };
  }, [sessionId, currentId, version, stamp, reloads]);

  const key = sessionId && currentId ? `${sessionId}#${currentId}` : null;
  useEffect(() => {
    const owner = currentStore();
    owner.setCanvasShown(key);
    return () => owner.setCanvasShown(null);
  }, [key]);

  const shown = loaded && loaded.meta.id === currentId ? loaded : null;
  const isLatest = !!shown && shown.version === shown.meta.latest;
  const runtime = key && isLatest ? (canvas.errors.get(key) ?? null) : null;
  const [oldError, setOldError] = useState<string | null>(null);
  const error = isLatest ? runtime : oldError ? { version: shown?.version ?? 0, message: oldError, line: null } : null;
  const updating = running.find((e) => e.tool !== "canvas_create" && e.canvasId === currentId) ?? null;

  const onMessage = useCallback(
    (m: FromCanvas) => {
      if (!sessionId || !shown) return;
      const k = `${sessionId}#${shown.meta.id}`;
      if (m.type === "key") {
        const { type: _type, ...init } = m;
        window.dispatchEvent(new KeyboardEvent("keydown", { ...init, bubbles: true, cancelable: true }));
        return;
      }
      if (m.type === "error") {
        if (shown.version === shown.meta.latest) {
          store.setCanvasError(k, { version: shown.version, message: m.message, line: m.line });
        } else setOldError(m.message);
      } else if (m.type === "send") {
        store.fillComposer(sessionId, sendPrompt(shown.meta.title, shown.meta.id, m.text), "append");
      } else if (m.type === "setData") {
        queueSetData(sessionId, shown.meta.id, m.data);
      }
    },
    [sessionId, shown],
  );

  // 换了一版（或重载）：旧报错作废。
  const docId = shown ? `${shown.meta.id}@${shown.version}` : "";
  useEffect(() => {
    setOldError(null);
    if (key && isLatest) store.setCanvasError(key, null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [docId, reloads]);

  const rollback = async (n: number) => {
    if (!sessionId || !current) return;
    setBusy(true);
    try {
      await dock.call("canvas/rollback", { threadId: sessionId, canvasId: current.id, version: n });
      setPinned(null);
      await refreshList();
    } catch (e) {
      store.reportError(e);
    }
    setBusy(false);
  };

  const askFix = () => {
    if (!sessionId || !shown || !error) return;
    store.fillComposer(sessionId, fixPrompt(shown.meta.title, shown.meta.id, error), "append");
  };

  const dot = error ? "bg-[#ff3b30]" : updating || generating ? "bg-[#af52de]" : shown ? "bg-[#34c759]" : "bg-ink-5";

  return (
    // 容器查询：面板窄时顶栏先收「导出」字样和刷新，标题让出宽度（关闭按钮总在）。
    <aside className="glass-pane @container enter-fade relative flex min-h-0 min-w-0 flex-1 flex-col bg-white">
      <header className="flex h-11 shrink-0 items-center gap-1.5 border-b border-line-soft px-3" data-tauri-drag-region>
        {lead}
        <span className={`ml-1 size-2 shrink-0 rounded-full ${dot}`} />
        {list && list.length > 0 ? (
          <Switcher list={list} currentId={currentId} onPick={(id) => (setCurrentId(id), setPinned(null))} />
        ) : (
          <span className="shrink-0 text-[13px] font-semibold text-ink">画布</span>
        )}
        {current && shown && (
          <VersionMenu
            meta={current}
            shown={shown.version}
            error={!!error}
            updating={!!updating}
            busy={busy}
            onView={(n) => setPinned(n === current.latest ? null : n)}
            onRollback={(n) => void rollback(n)}
          />
        )}
        <div className="min-w-2 flex-1" data-tauri-drag-region />
        <CanvasOpen
          sessionId={sessionId}
          list={list}
          className="flex size-7 shrink-0 items-center justify-center rounded-md text-ink-3 hover:bg-hover hover:text-ink"
          trigger={<FolderOpen size={14} />}
        />
        {shown && (
          <>
            <div className="hidden @[440px]:flex">
              <Segmented value={view} onChange={setView} />
            </div>
            <IconButton
              icon={view === "canvas" ? Code2 : LayoutDashboard}
              label={view === "canvas" ? "看源码" : "看画布"}
              size={14}
              onClick={() => setView(view === "canvas" ? "source" : "canvas")}
              className="size-7 @[440px]:hidden"
            />
            <IconButton
              icon={RefreshCw}
              label="重新加载"
              size={14}
              onClick={() => setReloads((n) => n + 1)}
              className="hidden size-7 @[460px]:flex"
            />
            <ExportMenu loaded={shown} />
          </>
        )}
        {controls}
        <span className="mx-0.5 h-4 w-px bg-line" />
        <IconButton icon={X} label="关闭画布面板" size={14} onClick={onClose} className="size-7 text-ink-4" />
      </header>

      {error && view === "canvas" && (
        <Banner tone="red" icon={CircleAlert} title={error.message} hint={error.line ? `v${error.version}.html 第 ${error.line} 行` : null}>
          <BannerButton icon={Code2} label="查看源码" onClick={() => setView("source")} />
          <BannerButton icon={Sparkles} label="让 Agent 修复" strong onClick={askFix} />
        </Banner>
      )}
      {updating && !error && (
        <Banner
          tone="purple"
          icon={Sparkles}
          title={updating.tool === "canvas_data" ? "Agent 正在更新数据" : "Agent 正在改这块画布"}
          hint={updating.note ?? "改完会自动换上新版"}
        />
      )}
      {shown && !isLatest && (
        <Banner tone="gray" icon={History} title={`正在看 v${shown.version}（旧版）`} hint={shown.meta.versions.find((v) => v.n === shown.version)?.note ?? null}>
          <BannerButton icon={LayoutDashboard} label="回到最新" onClick={() => setPinned(null)} />
          <BannerButton icon={RotateCcw} label="回退到这一版" strong onClick={() => void rollback(shown.version)} />
        </Banner>
      )}

      {!sessionId ? (
        <Empty icon={Layers} title="先选一个会话" text="画布面板显示当前会话里 agent 画出来的看板、图表和报告。" />
      ) : failure && !list ? (
        <Empty icon={CircleAlert} title="读不到画布" text={failure}>
          <DarkButton icon={RefreshCw} label="重试" onClick={() => setReloads((n) => n + 1)} />
        </Empty>
      ) : list === null ? (
        <Skeleton />
      ) : generating && (!shown || list.length === 0) ? (
        <Generating title={generating.title} />
      ) : list.length === 0 ? (
        <Empty
          icon={LayoutDashboard}
          title="还没有画布"
          text="让 agent 把结果画出来：看板、图表、对比表、小工具都行。页面在这里实时渲染，数据变了就地重绘。"
        >
          <CanvasOpen
            sessionId={sessionId}
            list={list}
            className="motion-colors flex items-center gap-1.5 rounded-lg border border-line-strong bg-white px-3 py-2 text-[13px] text-ink-2 hover:bg-hover"
            trigger={
              <>
                <FolderOpen size={14} />
                打开历史画布
              </>
            }
          />
          <button
            type="button"
            onClick={() => store.fillComposer(sessionId, EXAMPLE, "replace")}
            className="motion-colors mt-1 rounded-lg border border-line-strong bg-white px-3 py-2 text-left text-xs text-ink-2 hover:bg-hover"
          >
            {EXAMPLE}
            <span className="ml-2 text-accent">填入输入框</span>
          </button>
        </Empty>
      ) : !shown ? (
        <Skeleton />
      ) : view === "source" ? (
        <Source loaded={shown} errorLine={error?.line ?? null} />
      ) : (
        <Frame key={`${docId}#${reloads}`} loaded={shown} onMessage={onMessage} />
      )}
    </aside>
  );
}

// ---- 改数据：用户在页面里连着改，合并成一次写 ----

let pending: { timer: number; flush: () => void } | null = null;

function queueSetData(threadId: string, canvasId: string, data: unknown) {
  if (pending) window.clearTimeout(pending.timer);
  const flush = () => {
    pending = null;
    dock.call("canvas/setData", { threadId, canvasId, data }).catch((e) => store.reportError(e));
  };
  pending = { timer: window.setTimeout(flush, SET_DATA_DELAY_MS), flush };
}

// ---- iframe ----

function Frame({ loaded, onMessage }: { loaded: Loaded; onMessage: (m: FromCanvas) => void }) {
  const ref = useRef<HTMLIFrameElement>(null);
  const [doc, setDoc] = useState<string | null>(null);
  const handler = useRef(onMessage);
  handler.current = onMessage;

  // HTML 只在挂载时装一次（换版本 / 重载由外层 key 换掉整个 Frame）；之后只推数据。
  const first = useRef(loaded.data);
  useEffect(() => {
    let live = true;
    void buildDoc(loaded.html, first.current).then((d) => live && setDoc(d));
    return () => {
      live = false;
    };
  }, [loaded.html]);

  const sent = useRef(loaded.data);
  useEffect(() => {
    if (Object.is(sent.current, loaded.data) || JSON.stringify(sent.current) === JSON.stringify(loaded.data)) return;
    sent.current = loaded.data;
    ref.current?.contentWindow?.postMessage({ __dock: 1, type: "data", data: loaded.data }, "*");
  }, [loaded.data]);

  useEffect(() => {
    const onMsg = (e: MessageEvent) => {
      if (!ref.current || e.source !== ref.current.contentWindow) return;
      const m = e.data as (FromCanvas & { __dock?: number }) | null;
      if (m && m.__dock === 1) handler.current(m);
    };
    window.addEventListener("message", onMsg);
    return () => window.removeEventListener("message", onMsg);
  }, []);

  const missing = unknownLibs(loaded.html);
  return (
    <div className="glass-keep relative flex min-h-0 flex-1 flex-col bg-white">
      {missing.length > 0 && (
        <div className="shrink-0 border-b border-[#ffcc00]/40 bg-[#fffbea] px-4 py-1.5 text-xs text-[#8a6d00]">
          没有内置这些库：{missing.map((n) => `dock:lib/${n}`).join("、")}。目前内置 chart.js；别的库请用 CDN 地址。
        </div>
      )}
      {doc === null ? (
        <Skeleton />
      ) : (
        <iframe
          ref={ref}
          title={loaded.meta.title}
          srcDoc={doc}
          sandbox="allow-scripts allow-forms allow-popups allow-modals"
          className="glass-keep enter-fade min-h-0 w-full flex-1 border-0 bg-white"
        />
      )}
    </div>
  );
}

// ---- 顶栏 ----

function Switcher({ list, currentId, onPick }: { list: Meta[]; currentId: string | null; onPick: (id: string) => void }) {
  const current = list.find((c) => c.id === currentId);
  return (
    <Popover
      title="切换画布"
      side="bottom"
      align="left"
      panelClassName="w-[300px] max-h-80"
      className="flex min-w-0 max-w-[110px] items-center gap-1 rounded-md px-1.5 py-1 hover:bg-hover @[460px]:max-w-[180px] @[620px]:max-w-[260px]"
      trigger={
        <>
          <span className="truncate text-[13px] font-semibold text-ink">{current?.title ?? "画布"}</span>
          {list.length > 1 && <ChevronDown size={13} className="shrink-0 text-ink-4" />}
        </>
      }
    >
      {(close) => (
        <div className="flex flex-col p-1">
          <div className="px-2.5 pb-1 pt-1.5 text-[11px] text-ink-4">这个会话的画布（{list.length}）</div>
          {list.map((c, i) => (
            <button
              key={c.id}
              type="button"
              onClick={() => {
                onPick(c.id);
                close();
              }}
              className={`motion-colors flex items-center gap-2 rounded-md px-2.5 py-2 text-left hover:bg-hover ${c.id === currentId ? "bg-fill" : ""}`}
            >
              <span className={`size-1.5 shrink-0 rounded-full ${c.id === currentId ? "bg-[#34c759]" : "bg-ink-5"}`} />
              <span className="min-w-0 flex-1 truncate text-[13px] text-ink">
                {i + 1}. {c.title}
              </span>
              <span className="shrink-0 font-mono text-[11px] text-ink-4">v{c.latest}</span>
              {c.id === currentId && <Check size={13} className="shrink-0 text-accent" />}
            </button>
          ))}
        </div>
      )}
    </Popover>
  );
}

// ---- 打开历史画布 ----
//
// 画布跟着会话目录走（`<会话目录>/canvas/<canvas-N>/`），所以历史画布在别的会话里。
// 网关的 `canvas/list` 按 threadId 找会话目录：开着的那页用它正在写的会话，关着的
// 会话从名册找 —— 历史会话的画布不用开页就能列出来，也不依赖本机磁盘（远程 Dock 一样）。

/** 一次最多扫这么多个会话：扫太多会把网关打满，也没人翻那么久。 */
const SCAN_LIMIT = 60;
/** 同时发几个 `canvas/list`。 */
const SCAN_CONCURRENCY = 8;
/** 扫完的结果留一小会儿：顶栏和空状态两个入口共用一次扫描。 */
const SCAN_CACHE_MS = 10_000;

interface CanvasGroup {
  threadId: string;
  title: string;
  cwd: string;
  updatedAt: number;
  canvases: Meta[];
}

let scanCache: { at: number; groups: CanvasGroup[] } | null = null;

/** 扫历史会话的画布。读不到的会话（刚删了、没落盘）跳过，不挡着别的。 */
async function scanCanvases(sessions: SessionSummary[], currentId: string | null): Promise<CanvasGroup[]> {
  const targets = sessions
    .filter((s) => s.id !== currentId)
    .sort((a, b) => b.updatedAt - a.updatedAt)
    .slice(0, SCAN_LIMIT);
  const groups: CanvasGroup[] = [];
  let next = 0;
  const worker = async () => {
    while (next < targets.length) {
      const s = targets[next++];
      try {
        const r = await dock.call<{ canvases: Meta[] }>("canvas/list", { threadId: s.id });
        if (r.canvases.length > 0) {
          groups.push({ threadId: s.id, title: s.title, cwd: s.cwd, updatedAt: s.updatedAt, canvases: r.canvases });
        }
      } catch {
        // 这个会话读不到（删了 / 没落盘）：跳过。
      }
    }
  };
  await Promise.all(Array.from({ length: Math.min(SCAN_CONCURRENCY, targets.length) }, worker));
  return groups.sort((a, b) => b.updatedAt - a.updatedAt);
}

/** 顶栏和空状态两处入口：同一个下拉，两种触发外观。 */
function CanvasOpen({
  sessionId,
  list,
  className,
  trigger,
}: {
  sessionId: string | null;
  /** 当前会话的画布（面板里已经列好了）。 */
  list: Meta[] | null;
  className: string;
  trigger: ReactNode;
}) {
  return (
    <Popover
      title="打开画布"
      side="bottom"
      align="right"
      panelClassName="w-[380px] max-h-[420px]"
      className={className}
      trigger={trigger}
    >
      {(close) => <CanvasOpenBody sessionId={sessionId} list={list} close={close} />}
    </Popover>
  );
}

function CanvasOpenBody({
  sessionId,
  list,
  close,
}: {
  sessionId: string | null;
  list: Meta[] | null;
  close: () => void;
}) {
  const sessions = useApp().sessions;
  const [groups, setGroups] = useState<CanvasGroup[] | null>(null);
  const [scanning, setScanning] = useState(false);

  useEffect(() => {
    let live = true;
    const cached = scanCache && Date.now() - scanCache.at < SCAN_CACHE_MS ? scanCache : null;
    if (cached) setGroups(cached.groups);
    setScanning(true);
    void scanCanvases(sessions, sessionId).then((g) => {
      scanCache = { at: Date.now(), groups: g };
      if (!live) return;
      setGroups(g);
      setScanning(false);
    });
    return () => {
      live = false;
    };
  }, [sessions, sessionId]);

  // 打开别人的画布 = 切到那个会话再打开（对话流里的画布卡也是这条路）。
  const open = (threadId: string, canvasId: string) => {
    store.openCanvas(threadId, canvasId);
    close();
  };

  const row = (c: Meta, threadId: string) => (
    <button
      key={`${threadId}#${c.id}`}
      type="button"
      onClick={() => open(threadId, c.id)}
      className="motion-colors flex w-full items-center gap-2 rounded-md px-2.5 py-2 text-left hover:bg-hover"
    >
      <span className="size-1.5 shrink-0 rounded-full bg-ink-5" />
      <span className="min-w-0 flex-1 truncate text-[13px] text-ink">{c.title}</span>
      <span className="shrink-0 font-mono text-[11px] text-ink-4">v{c.latest}</span>
      <span className="shrink-0 text-[11px] text-ink-4">{clock(c.updatedMs)}</span>
    </button>
  );

  return (
    <div className="flex flex-col p-1">
      {list && list.length > 0 && sessionId && (
        <>
          <div className="px-2.5 pb-1 pt-1.5 text-[11px] text-ink-4">当前会话（{list.length}）</div>
          {list.map((c) => row(c, sessionId))}
        </>
      )}
      <div className="px-2.5 pb-1 pt-2 text-[11px] text-ink-4">历史会话</div>
      {groups === null && <div className="px-2.5 py-2 text-xs text-ink-4">正在找历史画布…</div>}
      {groups !== null && groups.length === 0 && !scanning && (
        <div className="px-2.5 py-2 text-xs leading-[1.5] text-ink-4">
          别的会话里没画过画布。画布存在各自会话目录的 canvas/ 下，跟着会话走。
        </div>
      )}
      {groups?.map((g) => (
        <div key={g.threadId}>
          <div className="flex items-baseline gap-2 px-2.5 pb-1 pt-2 text-[11px]">
            <span className="shrink-0 text-ink-3">{g.title}</span>
            <span className="truncate text-ink-4" title={g.cwd}>
              {g.cwd}
            </span>
          </div>
          {g.canvases.map((c) => row(c, g.threadId))}
        </div>
      ))}
    </div>
  );
}

function VersionMenu({
  meta,
  shown,
  error,
  updating,
  busy,
  onView,
  onRollback,
}: {
  meta: Meta;
  shown: number;
  error: boolean;
  updating: boolean;
  busy: boolean;
  onView: (n: number) => void;
  onRollback: (n: number) => void;
}) {
  const tone = error
    ? "border-[#ff3b30]/30 bg-[#fff5f5] text-[#d70015]"
    : updating
      ? "border-[#af52de]/30 bg-[#faf5ff] text-[#af52de]"
      : "border-line-strong bg-fill text-ink-2";
  const label = error ? `v${shown}（异常）` : updating ? `v${shown} › 更新中` : `v${shown}`;
  return (
    <Popover
      title="版本历史"
      side="bottom"
      align="left"
      panelClassName="w-[340px] max-h-96"
      className={`flex h-[22px] shrink-0 items-center gap-1 rounded-md border px-2 font-mono text-[11px] font-semibold ${tone}`}
      trigger={
        <>
          {label}
          <ChevronDown size={11} />
        </>
      }
    >
      {(close) => (
        <div className="flex flex-col gap-1 p-1.5">
          <div className="flex items-center gap-1.5 px-1.5 pb-1 pt-1 text-[11px] text-ink-4">
            <History size={12} />
            版本历史 · 共 {meta.versions.length} 个版本
          </div>
          {[...meta.versions].reverse().map((v) => {
            const latest = v.n === meta.latest;
            const viewing = v.n === shown;
            return (
              <div
                key={v.n}
                className={`flex items-center gap-2.5 rounded-lg border px-2.5 py-2 ${
                  latest ? "border-[#af52de]/30 bg-[#faf5ff]" : "border-line-soft bg-white"
                }`}
              >
                <span
                  className={`flex h-5 min-w-7 shrink-0 items-center justify-center rounded px-1 font-mono text-[11px] font-semibold ${
                    latest ? "bg-[#af52de] text-white" : "bg-fill text-ink-3"
                  }`}
                >
                  v{v.n}
                </span>
                <button
                  type="button"
                  onClick={() => {
                    onView(v.n);
                    close();
                  }}
                  className="flex min-w-0 flex-1 flex-col items-start text-left"
                >
                  <span className="w-full truncate text-[13px] text-ink">{v.note || "（没有说明）"}</span>
                  <span className="text-[11px] text-ink-4">
                    {clock(v.createdMs)}
                    {viewing ? " · 正在看" : ""}
                  </span>
                </button>
                {latest ? (
                  <span className="shrink-0 text-[11px] font-medium text-[#34c759]">当前版本</span>
                ) : (
                  <button
                    type="button"
                    disabled={busy}
                    onClick={() => {
                      onRollback(v.n);
                      close();
                    }}
                    className="motion-colors flex h-6 shrink-0 items-center gap-1 rounded-md border border-line-strong bg-white px-2 text-[11px] text-ink-2 hover:bg-hover disabled:opacity-50"
                  >
                    <RotateCcw size={11} />
                    回退
                  </button>
                )}
              </div>
            );
          })}
        </div>
      )}
    </Popover>
  );
}

function Segmented({ value, onChange }: { value: "canvas" | "source"; onChange: (v: "canvas" | "source") => void }) {
  const item = (v: "canvas" | "source", label: string) => (
    <button
      type="button"
      onClick={() => onChange(v)}
      className={`motion-colors h-[22px] rounded-[5px] px-2 text-xs ${
        value === v ? "bg-white font-medium text-accent shadow-sm" : "text-ink-3 hover:text-ink"
      }`}
    >
      {label}
    </button>
  );
  return (
    <div className="flex shrink-0 items-center rounded-md bg-fill p-0.5">
      {item("canvas", "画布")}
      {item("source", "源码")}
    </div>
  );
}

function ExportMenu({ loaded }: { loaded: Loaded }) {
  const remote = useApp().remote;
  const [done, setDone] = useState<string | null>(null);
  const copy = async (what: string, text: string | Promise<string>) => {
    try {
      await navigator.clipboard.writeText(await text);
      setDone(what);
      window.setTimeout(() => setDone(null), 1400);
    } catch (e) {
      store.reportError(e);
    }
  };
  // 存盘走 Tauri 的「存储为」对话框（写到哪儿由用户在对话框里选）。
  const save = async (what: string, name: string, text: string | Promise<string>) => {
    try {
      const path = await invoke<string | null>("save_text_file", { name, contents: await text });
      if (!path) return;
      setDone(what);
      window.setTimeout(() => setDone(null), 1400);
    } catch (e) {
      store.reportError(e);
    }
  };
  const base = fileName(loaded.meta.title);
  const row = (icon: LucideIcon, title: string, hint: string, what: string, run: () => void, tag?: string) => {
    const Icon = icon;
    return (
      <button
        type="button"
        onClick={run}
        className="motion-colors flex items-center gap-3 rounded-lg px-2.5 py-2 text-left hover:bg-hover"
      >
        <span className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-fill">
          <Icon size={15} className="text-ink-3" />
        </span>
        <span className="flex min-w-0 flex-1 flex-col">
          <span className="text-[13px] font-medium text-ink">{title}</span>
          <span className="truncate text-[11px] text-ink-4">{hint}</span>
        </span>
        <span className={`shrink-0 text-[11px] ${done === what ? "text-[#34c759]" : "text-accent"}`}>
          {done === what ? (tag === "复制" || !tag ? "已复制" : "已保存") : (tag ?? "复制")}
        </span>
      </button>
    );
  };
  return (
    <Popover
      title="导出"
      side="bottom"
      panelClassName="w-[320px]"
      className="flex h-7 shrink-0 items-center gap-1 rounded-md border border-line-strong bg-white px-1.5 text-xs text-ink-2 hover:bg-hover @[520px]:px-2"
      trigger={
        <>
          <Download size={13} />
          <span className="hidden @[520px]:inline">导出</span>
        </>
      }
    >
      {() => (
        <div className="flex flex-col p-1">
          {row(
            Layers,
            "导出自包含 HTML",
            "库和数据都内联，离线双击能开",
            "full",
            () => void save("full", `${base}.html`, buildDoc(loaded.html, loaded.data)),
            "存储",
          )}
          {row(
            FileJson,
            "导出数据 JSON",
            "data.json",
            "data",
            () => void save("data", `${base}.json`, JSON.stringify(loaded.data, null, 2)),
            "存储",
          )}
          {row(Code2, "复制 HTML 源码", `v${loaded.version}.html，带 window.dock 调用`, "html", () => void copy("html", loaded.html))}
          {/* 连远程 Dock 时画布目录在远端，本机访达打不开。 */}
          {!remote && (
            <>
              <div className="mx-2.5 my-1 h-px bg-line-soft" />
              {row(
                FolderOpen,
                "在访达中显示",
                "画布目录（meta.json、各版 HTML、data.json）",
                "reveal",
                () => void revealItemInDir(`${loaded.path}/v${loaded.version}.html`).catch((e) => store.reportError(e)),
                "打开",
              )}
            </>
          )}
        </div>
      )}
    </Popover>
  );
}

/** 标题当文件名：去掉路径和系统不认的字符。 */
function fileName(title: string): string {
  return title.replace(/[\\/:*?"<>|\n\r]+/g, " ").trim().slice(0, 60) || "canvas";
}

// ---- 源码 ----

function Source({ loaded, errorLine }: { loaded: Loaded; errorLine: number | null }) {
  const [tab, setTab] = useState<"html" | "data">("html");
  const [copied, setCopied] = useState(false);
  const text = tab === "html" ? loaded.html : JSON.stringify(loaded.data, null, 2);
  const lines = text.split("\n");
  const errRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (tab === "html") errRef.current?.scrollIntoView({ block: "center" });
  }, [tab, errorLine]);
  const tabBtn = (v: "html" | "data", icon: LucideIcon, label: string) => {
    const Icon = icon;
    return (
      <button
        type="button"
        onClick={() => setTab(v)}
        className={`motion-colors flex h-7 items-center gap-1.5 rounded-md px-2.5 text-[13px] ${
          tab === v ? "bg-white font-medium text-accent shadow-sm" : "text-ink-3 hover:text-ink"
        }`}
      >
        <Icon size={14} />
        {label}
      </button>
    );
  };
  return (
    <div className="enter-fade flex min-h-0 flex-1 flex-col gap-3 bg-pane p-4">
      <div className="flex items-center gap-2">
        <div className="flex items-center rounded-lg bg-fill p-0.5">
          {tabBtn("html", Code2, "HTML")}
          {tabBtn("data", FileJson, "数据 (JSON)")}
        </div>
        <div className="flex-1" />
        <span className="rounded bg-fill px-1.5 py-0.5 text-[11px] text-ink-4">只读</span>
        <button
          type="button"
          onClick={() => {
            void navigator.clipboard.writeText(text).then(() => {
              setCopied(true);
              window.setTimeout(() => setCopied(false), 1400);
            });
          }}
          className="flex items-center gap-1 text-xs text-accent hover:underline"
        >
          {copied ? <Check size={13} /> : <Copy size={13} />}
          {copied ? "已复制" : "复制"}
        </button>
      </div>
      <div className="flex min-h-0 flex-1 flex-col overflow-hidden rounded-xl bg-[#1c1c1e]">
        <div className="shrink-0 border-b border-white/10 px-4 py-2 font-mono text-[11px] text-[#64d2ff]">
          {loaded.meta.id}/{tab === "html" ? `v${loaded.version}.html` : "data.json"}
        </div>
        <div className="thin-scroll min-h-0 flex-1 overflow-auto py-2 font-mono text-[12px] leading-[1.6] select-text">
          {lines.map((l, i) => {
            const bad = tab === "html" && errorLine === i + 1;
            return (
              <div key={i} ref={bad ? errRef : undefined} className={`flex px-2 ${bad ? "bg-[#ff3b30]/20" : ""}`}>
                <span className="w-10 shrink-0 select-none pr-3 text-right text-white/30">{i + 1}</span>
                <span className="whitespace-pre text-white/85">{l || " "}</span>
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}

// ---- 状态 ----

function Banner({
  tone,
  icon: Icon,
  title,
  hint,
  children,
}: {
  tone: "red" | "purple" | "gray";
  icon: LucideIcon;
  title: string;
  hint: string | null;
  children?: React.ReactNode;
}) {
  const box = {
    red: "border-[#ff3b30]/20 bg-[#fff5f5]",
    purple: "border-[#af52de]/20 bg-[#faf5ff]",
    gray: "border-line-soft bg-pane",
  }[tone];
  const ink = { red: "text-[#d70015]", purple: "text-[#8e44ad]", gray: "text-ink" }[tone];
  const badge = { red: "bg-[#ff3b30]/10 text-[#ff3b30]", purple: "bg-[#af52de]/10 text-[#af52de]", gray: "bg-fill text-ink-3" }[tone];
  return (
    <div className={`enter-fade flex shrink-0 items-center gap-3 border-b px-4 py-2.5 ${box}`}>
      <span className={`flex size-8 shrink-0 items-center justify-center rounded-lg ${badge}`}>
        <Icon size={15} className={tone === "purple" ? "animate-pulse" : ""} />
      </span>
      <div className="flex min-w-0 flex-1 flex-col">
        <span className={`truncate text-[13px] font-semibold ${ink}`} title={title}>
          {title}
        </span>
        {hint && <span className="truncate text-xs text-ink-3">{hint}</span>}
      </div>
      {children}
    </div>
  );
}

function BannerButton({ icon: Icon, label, strong, onClick }: { icon: LucideIcon; label: string; strong?: boolean; onClick: () => void }) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={`motion-colors flex h-7 shrink-0 items-center gap-1.5 rounded-md px-2.5 text-xs font-medium ${
        strong ? "bg-[#ff3b30] text-white hover:bg-[#e0352b]" : "border border-line-strong bg-white text-ink-2 hover:bg-hover"
      }`}
    >
      <Icon size={13} />
      {label}
    </button>
  );
}

function Generating({ title }: { title: string | null }) {
  return (
    <div className="enter-fade flex min-h-0 flex-1 flex-col gap-4 bg-pane p-5">
      <div className="flex items-center gap-2">
        <span className="relative flex size-2.5">
          <span className="absolute size-2.5 animate-ping rounded-full bg-accent opacity-60" />
          <span className="size-2.5 rounded-full bg-accent" />
        </span>
        <span className="text-[13px] font-semibold text-ink">{title ? `正在生成「${title}」` : "正在生成画布"}</span>
        <span className="text-xs text-ink-4">模型写完整页 HTML 后在这里渲染</span>
      </div>
      <div className="grid grid-cols-4 gap-3">
        {[0, 1, 2, 3].map((i) => (
          <div key={i} className="animate-pulse h-20 rounded-xl border border-line-soft bg-white" />
        ))}
      </div>
      <div className="animate-pulse min-h-0 flex-1 rounded-xl border border-line-soft bg-white" />
    </div>
  );
}

function Skeleton() {
  return (
    <div className="flex min-h-0 flex-1 flex-col gap-3 bg-pane p-5">
      <div className="animate-pulse h-6 w-1/3 rounded-md bg-fill" />
      <div className="animate-pulse min-h-0 flex-1 rounded-xl bg-fill" />
    </div>
  );
}

function Empty({ icon: Icon, title, text, children }: { icon: LucideIcon; title: string; text: string; children?: React.ReactNode }) {
  return (
    <div className="enter-fade flex min-h-0 flex-1 flex-col items-center justify-center gap-3 bg-pane px-10">
      <div className="flex size-12 items-center justify-center rounded-full border border-line-strong bg-white">
        <Icon size={20} className="text-ink-3" />
      </div>
      <span className="text-[15px] font-semibold text-ink">{title}</span>
      <span className="max-w-[380px] text-center text-[13px] leading-[1.55] text-ink-3">{text}</span>
      {children}
    </div>
  );
}

function DarkButton({ icon: Icon, label, onClick }: { icon: LucideIcon; label: string; onClick: () => void }) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="motion-colors flex h-8 items-center gap-1.5 rounded-lg bg-ink px-3.5 hover:bg-ink-2"
    >
      <Icon size={14} className="text-white" />
      <span className="text-[13px] font-medium text-white">{label}</span>
    </button>
  );
}

function clock(ms: number): string {
  const d = new Date(ms);
  const pad = (n: number) => String(n).padStart(2, "0");
  const today = new Date().toDateString() === d.toDateString();
  return today ? `${pad(d.getHours())}:${pad(d.getMinutes())}` : `${d.getMonth() + 1}/${d.getDate()} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}
