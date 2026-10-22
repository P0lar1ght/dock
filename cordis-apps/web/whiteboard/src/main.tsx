// Dock 白板 MCP App 的界面（规范 io.modelcontextprotocol/ui 2026-01-26）。
//
// - 场景在服务器上（按项目），这里经只给界面用的工具 `whiteboard_load` / `whiteboard_save` 读写；
// - Agent 加的图形先是骨架（pending）：这里用 Excalidraw 的 `convertToExcalidrawElements` 画成完整
//   元素（算箭头路径、排标签），再存回去（`consumed`）；
// - 用户改的：按元素 `version` 只存变了的，同时把「改了什么、选中了什么」交给模型（updateModelContext）；
// - Agent 改的：收到 tool-result（或轮询发现 `rev` 变了）就重载，用 `reconcileElements` 合并；
// - 内联（Gen UI）只看不改，点「在面板中编辑」请求 fullscreen，宿主把它放进常驻面板。
// - 面板里那份还给宿主几颗界面工具（viewTools.ts：选中、视野、挪视图、截图），模型经宿主调。

import "./shim";
import { StrictMode, useCallback, useEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import {
  CaptureUpdateAction,
  Excalidraw,
  convertToExcalidrawElements,
  reconcileElements,
  restoreElements,
} from "@excalidraw/excalidraw";
import "@excalidraw/excalidraw/index.css";
import { App, applyHostStyleVariables, type McpUiHostContext } from "@modelcontextprotocol/ext-apps";
import { installViewTools, type ViewApi } from "./viewTools";

/* eslint-disable @typescript-eslint/no-explicit-any */
type El = Record<string, any> & { id: string; type: string; version: number; isDeleted?: boolean };
type Api = {
  getSceneElementsIncludingDeleted: () => readonly El[];
  getAppState: () => Record<string, any>;
  getFiles: () => Record<string, any>;
  updateScene: (scene: Record<string, unknown>) => void;
  addFiles: (files: unknown[]) => void;
  scrollToContent: (target?: unknown, opts?: Record<string, unknown>) => void;
};
interface Scene {
  rev: number;
  elements: El[];
  pending: Record<string, any>[];
  appState: Record<string, any>;
  files: Record<string, any>;
}

const INLINE_HEIGHT = 420;
const SAVE_DELAY = 700;
const POLL_MS = 2500;

const app = new App({ name: "dock-whiteboard", version: "0.1.0" }, { tools: { listChanged: true } }, { autoResize: false });
/** 当前画布（界面工具用）。 */
let currentApi: ViewApi | null = null;
const toolsChanged = installViewTools(
  app,
  () => currentApi,
  () => (app.getHostContext()?.displayMode ?? "inline") === "fullscreen",
);
/** 只连一次（开发时 StrictMode 会把 effect 跑两遍）。 */
let connecting: Promise<void> | null = null;
/** 第一次载入也只做一次（同上）：两遍各画一份骨架会多出元素。 */
let firstLoad: Promise<Record<string, any>> | null = null;
let initialized = false;

async function tool(name: string, args: Record<string, unknown> = {}): Promise<Record<string, any>> {
  const r = await app.callServerTool({ name, arguments: args });
  if (r.isError) {
    const text = (r.content ?? []).map((b: any) => (b.type === "text" ? b.text : "")).join("\n");
    throw new Error(text || `${name} 失败`);
  }
  return (r.structuredContent ?? {}) as Record<string, any>;
}

/**
 * 骨架 → 完整元素。箭头连到已有的完整元素时，那一端先从骨架里拿掉（Excalidraw 会把引用的
 * 图形另画一份），画完再自己补绑定；箭头的位置服务器已经按两端算好了。
 */
function materialize(pending: Record<string, any>[], existing: readonly El[]): { made: El[]; touched: El[] } {
  if (pending.length === 0) return { made: [], touched: [] };
  const pendingIds = new Set(pending.map((p) => p.id));
  const byId = new Map(existing.filter((e) => !e.isDeleted).map((e) => [e.id, e]));
  const outside: { arrow: string; end: "start" | "end"; target: string }[] = [];
  const skeletons = pending.map((p) => {
    const copy = { ...p };
    for (const end of ["start", "end"] as const) {
      const target = p[end]?.id;
      if (target && !pendingIds.has(target)) {
        delete copy[end];
        if (byId.has(target)) outside.push({ arrow: p.id, end, target });
      }
    }
    return copy;
  });
  const converted = convertToExcalidrawElements(skeletons as never, { regenerateIds: false }) as unknown as El[];
  // 重用了删掉的 id：版本要压过墓碑，不然合并时墓碑赢（服务器在 customData 里记了下限）。
  // 框里的标签是 convert 另生的文字元素，不带 customData：用它那个框的下限（服务器已把标签墓碑算进去）。
  const mins = new Map<string, number>();
  for (const e of converted) if (typeof e.customData?.dockMinVersion === "number") mins.set(e.id, e.customData.dockMinVersion);
  const made = converted.map((e) => {
    const min = e.customData?.dockMinVersion ?? (e.containerId ? mins.get(e.containerId) : undefined);
    if (typeof min !== "number") return e;
    const { dockMinVersion: _, ...rest } = e.customData ?? {};
    return { ...e, version: Math.max(e.version, min), customData: Object.keys(rest).length ? rest : undefined };
  });
  // 标签文字的 id 定死成「<容器>-label」：两个界面各画一遍也合并成同一个元素，不会多出一份标签。
  const rename = new Map<string, string>();
  for (const e of made) if (e.type === "text" && e.containerId && pendingIds.has(e.containerId)) rename.set(e.id, `${e.containerId}-label`);
  for (const e of made) {
    if (rename.has(e.id)) e.id = rename.get(e.id)!;
    if (Array.isArray(e.boundElements)) e.boundElements = e.boundElements.map((b: any) => (rename.has(b.id) ? { ...b, id: rename.get(b.id) } : b));
  }
  const madeById = new Map(made.map((e) => [e.id, e]));
  const touchedById = new Map<string, El>();
  for (const { arrow, end, target } of outside) {
    const a = madeById.get(arrow);
    if (!a) continue;
    a[end === "start" ? "startBinding" : "endBinding"] = { elementId: target, focus: 0, gap: 8 };
    const t = touchedById.get(target) ?? { ...byId.get(target)! };
    const bound = [...(t.boundElements ?? [])];
    if (!bound.some((b: any) => b.id === arrow)) bound.push({ type: "arrow", id: arrow });
    touchedById.set(target, { ...t, boundElements: bound });
  }
  const touched = [...touchedById.values()].map((e) => ({
    ...e,
    version: e.version + 1,
    versionNonce: Math.floor(Math.random() * 2 ** 31),
    updated: Date.now(),
  }));
  return { made, touched };
}

/** 按 id 合成一份：后面的盖前面的。同一个数组里有重复 id 时 restoreElements 会给后一个换新 id。 */
function byIdLast(...lists: readonly El[][]): El[] {
  const map = new Map<string, El>();
  for (const list of lists) for (const e of list) map.set(e.id, e);
  return [...map.values()];
}

function label(e: El, all: readonly El[]): string {
  if (e.type === "text") return e.text ?? "";
  const t = all.find((x) => !x.isDeleted && x.containerId === e.id);
  return t?.text ?? "";
}

/** 给模型看的一句：类型、id、标签，外加位置尺寸（模型据此知道用户画在哪、多大）。 */
function describe(e: El, all: readonly El[]): string {
  const l = label(e, all);
  const r = Math.round;
  return `${e.type} ${e.id}${l ? `「${l.slice(0, 40)}」` : ""} (${r(e.x)},${r(e.y)}) ${r(Math.abs(e.width))}×${r(Math.abs(e.height))}`;
}

function Board() {
  const api = useRef<Api | null>(null);
  const [ctx, setCtx] = useState<McpUiHostContext | undefined>(undefined);
  const [connected, setConnected] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [initial, setInitial] = useState<{ elements: El[]; appState: Record<string, any>; files: Record<string, any> } | null>(null);
  const rev = useRef(0);
  const synced = useRef(new Map<string, number>());
  const syncedFiles = useRef(new Set<string>());
  const applying = useRef(false);
  const saveTimer = useRef<number | null>(null);
  const contextTimer = useRef<number | null>(null);
  const lastSelection = useRef("");
  const recent = useRef<string[]>([]);
  const inline = (ctx?.displayMode ?? "inline") === "inline";
  const isInline = () => (app.getHostContext()?.displayMode ?? "inline") === "inline";
  const refreshing = useRef<Promise<void>>(Promise.resolve());

  const remember = (elements: readonly El[]) => {
    for (const e of elements) synced.current.set(e.id, e.version);
  };

  /** 把用户改的、变了版本的元素存回去；带上刚画好的骨架 id。 */
  const save = useCallback(async (consumed: string[] = [], extra: El[] = []) => {
    const a = api.current;
    // 内联（Gen UI）只看不改：存回去的只有面板里那一份，免得两个界面各存一遍。
    if (!a || isInline()) return;
    const all = a.getSceneElementsIncludingDeleted();
    const changed = [...all.filter((e) => synced.current.get(e.id) !== e.version), ...extra];
    const files = a.getFiles();
    const newFiles = Object.fromEntries(Object.entries(files).filter(([id]) => !syncedFiles.current.has(id)));
    if (changed.length === 0 && consumed.length === 0 && Object.keys(newFiles).length === 0) return;
    const r = await tool("whiteboard_save", {
      elements: changed,
      consumed,
      appState: { viewBackgroundColor: a.getAppState().viewBackgroundColor },
      files: newFiles,
    });
    remember(changed);
    for (const id of Object.keys(newFiles)) syncedFiles.current.add(id);
    if (typeof r.rev === "number") rev.current = r.rev;
    if (consumed.length === 0) {
      recent.current = [...new Set([...changed.filter((e) => !e.containerId).map((e) => (e.isDeleted ? `删除 ${e.type} ${e.id}` : describe(e, all))), ...recent.current])].slice(0, 12);
      scheduleContext();
    }
  }, []);

  /** 交给模型：现在有什么、用户刚改了什么、选中了什么。只留最新一份（宿主覆盖）。 */
  const scheduleContext = useCallback(() => {
    if (contextTimer.current) window.clearTimeout(contextTimer.current);
    contextTimer.current = window.setTimeout(() => {
      const a = api.current;
      if (!a) return;
      const all = a.getSceneElementsIncludingDeleted();
      const live = all.filter((e) => !e.isDeleted && !e.containerId);
      const selected = Object.keys(a.getAppState().selectedElementIds ?? {})
        .map((id) => all.find((e) => e.id === id))
        .filter((e): e is El => !!e && !e.isDeleted && !e.containerId);
      const lines = [`项目白板第 ${rev.current} 版，${live.length} 个图形。`];
      if (recent.current.length) lines.push(`用户刚在白板上改了：${recent.current.join("；")}。`);
      lines.push(selected.length ? `用户当前选中：${selected.map((e) => describe(e, all)).join("；")}。` : "用户当前没有选中图形。");
      void app.updateModelContext({
        content: [{ type: "text", text: lines.join("\n") }],
        structuredContent: { rev: rev.current, selected: selected.map((e) => e.id), recent: recent.current },
      });
    }, 1200);
  }, []);

  /** 重载服务器上的场景并合并进画布（Agent 改过、别的会话改过）。一个接一个跑：轮询和 tool-result 会撞。 */
  const refresh = useCallback((force = false) => {
    const next = refreshing.current.then(() => reload(force));
    refreshing.current = next.catch(() => {});
    return next;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const reload = async (force: boolean) => {
    const a = api.current;
    if (!a) return;
    const r = (await tool("whiteboard_load", force ? {} : { sinceRev: rev.current })) as Scene & { unchanged?: boolean };
    if (r.unchanged) return;
    const local = a.getSceneElementsIncludingDeleted();
    const remote = r.elements ?? [];
    const { made, touched } = materialize(r.pending ?? [], [...local, ...remote]);
    const incoming = restoreElements(byIdLast(remote, touched, made) as never, null, { refreshDimensions: true, repairBindings: true }) as unknown as El[];
    const merged = reconcileElements(local as never, incoming as never, a.getAppState() as never) as unknown as El[];
    applying.current = true;
    a.updateScene({ elements: merged, captureUpdate: CaptureUpdateAction.NEVER });
    if (r.files) a.addFiles(Object.values(r.files));
    applying.current = false;
    rev.current = r.rev;
    remember(remote);
    for (const id of Object.keys(r.files ?? {})) syncedFiles.current.add(id);
    if (made.length || touched.length) {
      await save(r.pending.map((p) => p.id));
      if (made.length) a.scrollToContent(undefined, { fitToContent: true, animate: true });
    }
  };

  // 连宿主。
  useEffect(() => {
    app.onhostcontextchanged = (p) => {
      setCtx((c) => ({ ...c, ...p }));
      // 进 / 出面板：界面工具只在面板里给。
      if (p.displayMode) toolsChanged();
      if (p.styles?.variables) applyHostStyleVariables(p.styles.variables);
    };
    app.ontoolinput = () => setBusy("Agent 正在改白板…");
    app.ontoolresult = () => {
      setBusy(null);
      void refresh().catch((e) => setError(String(e?.message ?? e)));
    };
    app.ontoolcancelled = () => setBusy(null);
    app.onteardown = async () => {
      if (saveTimer.current) window.clearTimeout(saveTimer.current);
      await save().catch(() => {});
      return {};
    };
    (connecting ??= app.connect())
      .then(() => (firstLoad ??= tool("whiteboard_load")))
      .then(async (loaded) => {
        const c = app.getHostContext();
        setCtx(c);
        if (c?.styles?.variables) applyHostStyleVariables(c.styles.variables);
        if ((c?.displayMode ?? "inline") === "inline") void app.sendSizeChanged({ height: INLINE_HEIGHT });
        setConnected(true);
        const scene = loaded as Scene;
        rev.current = scene.rev;
        remember(scene.elements);
        for (const id of Object.keys(scene.files ?? {})) syncedFiles.current.add(id);
        const { made, touched } = materialize(scene.pending ?? [], scene.elements);
        const elements = restoreElements(byIdLast(scene.elements, touched, made) as never, null, {
          refreshDimensions: true,
          repairBindings: true,
        }) as unknown as El[];
        setInitial({ elements, appState: scene.appState ?? {}, files: scene.files ?? {} });
        // 刚画好的骨架等画布挂上后存回去。
        if ((made.length || touched.length) && !initialized) {
          initialized = true;
          window.setTimeout(() => void save(scene.pending.map((p) => p.id)).catch(() => {}), 300);
        }
      })
      .catch((e) => setError(String(e?.message ?? e)));
  }, [refresh, save]);

  // 轮询：别的会话、终端里的 Agent 改了也看得到。页面藏起来时不拉。
  useEffect(() => {
    if (!connected) return;
    const t = window.setInterval(() => {
      if (document.visibilityState === "visible") void refresh().catch(() => {});
    }, POLL_MS);
    return () => window.clearInterval(t);
  }, [connected, refresh]);

  const onChange = (elements: readonly El[], appState: Record<string, any>) => {
    if (applying.current || inline) return;
    if (elements.some((e) => synced.current.get(e.id) !== e.version)) {
      if (saveTimer.current) window.clearTimeout(saveTimer.current);
      saveTimer.current = window.setTimeout(() => void save().catch((e) => setError(String(e?.message ?? e))), SAVE_DELAY);
    }
    const sel = Object.keys(appState.selectedElementIds ?? {}).sort().join(",");
    if (sel !== lastSelection.current) {
      lastSelection.current = sel;
      scheduleContext();
    }
  };

  if (error && !initial) return <div className="note error">白板打不开：{error}</div>;
  if (!initial) return <div className="note">正在打开白板…</div>;
  return (
    <div style={{ position: "relative", height: inline ? INLINE_HEIGHT : "100vh" }}>
      <Excalidraw
        excalidrawAPI={(a) => {
          api.current = a as unknown as Api;
          currentApi = a as unknown as ViewApi;
          // 载入后缩放到能看全所有图形。
          window.setTimeout(() => api.current?.scrollToContent(undefined, { fitToContent: true }), 50);
        }}
        initialData={{ elements: initial.elements as never, appState: { ...initial.appState, zenModeEnabled: inline }, files: initial.files as never, scrollToContent: true }}
        onChange={onChange as never}
        viewModeEnabled={inline}
        theme={ctx?.theme === "dark" ? "dark" : "light"}
        langCode="zh-CN"
        UIOptions={{ canvasActions: { loadScene: false, saveToActiveFile: false, toggleTheme: false } }}
      />
      {inline && (
        <button className="open" onClick={() => void app.requestDisplayMode({ mode: "fullscreen" })}>
          在面板中编辑
        </button>
      )}
      {busy && <div className="busy">{busy}</div>}
      {error && <div className="toast">{error}</div>}
    </div>
  );
}

const style = document.createElement("style");
style.textContent = `
.note { padding: 16px; color: var(--color-text-tertiary, #636366); }
.note.error { color: var(--color-text-danger, #d70015); }
.open { position: absolute; right: 12px; bottom: 12px; z-index: 10; height: 30px; padding: 0 12px; border: 0; border-radius: 8px;
  background: #1c1c1e; color: #fff; font: 500 12px/30px var(--font-sans, -apple-system, sans-serif); cursor: pointer; }
.open:hover { background: #3a3a3c; }
.busy { position: absolute; left: 50%; top: 10px; transform: translateX(-50%); z-index: 10; padding: 4px 10px; border-radius: 999px;
  background: #e8f2ff; color: #0a84ff; font: 500 12px/1.5 var(--font-sans, sans-serif); }
.toast { position: absolute; left: 12px; bottom: 12px; z-index: 10; max-width: 60%; padding: 6px 10px; border-radius: 8px;
  background: #fef2f2; color: #d70015; font: 12px/1.5 var(--font-sans, sans-serif); }
`;
document.head.appendChild(style);

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <Board />
  </StrictMode>,
);
