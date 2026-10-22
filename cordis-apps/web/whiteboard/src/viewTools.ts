// 白板界面自己的工具（规范 appCapabilities.tools）：只有界面知道的事——用户选中了什么、正看着
// 哪一块、把视图挪给用户看、截一张图。宿主（Dock）在界面开着时交给模型，经 use_tool 调。
//
// 只在面板（fullscreen）里给：内联那份是只读快照，不该和面板抢「用户正看着的白板」。

import { exportToBlob } from "@excalidraw/excalidraw";
import type { App } from "@modelcontextprotocol/ext-apps";

/* eslint-disable @typescript-eslint/no-explicit-any */
type El = Record<string, any> & { id: string; type: string; isDeleted?: boolean };
export interface ViewApi {
  getSceneElementsIncludingDeleted: () => readonly El[];
  getAppState: () => Record<string, any>;
  getFiles: () => Record<string, any>;
  updateScene: (scene: Record<string, unknown>) => void;
  scrollToContent: (target?: unknown, opts?: Record<string, unknown>) => void;
}

/** 截图的长边上限（像素）：够看清，又不至于把一整张大图塞给模型。 */
const SNAPSHOT_MAX = 1600;

const TOOLS = [
  {
    name: "get_selection",
    description: "用户在白板上当前选中的图形：id、类型、标签文字、位置尺寸。用户说「这个」「选中的」时先调它。",
    inputSchema: { type: "object", properties: {} },
  },
  {
    name: "get_viewport",
    description: "用户现在在白板上看着的区域（画布坐标、缩放）和视野里的图形。",
    inputSchema: { type: "object", properties: {} },
  },
  {
    name: "focus",
    description: "把用户的视图移到这些图形上（动画缩放到刚好看全），可顺带选中它们。只动视图，不改内容。",
    inputSchema: {
      type: "object",
      properties: {
        ids: { type: "array", items: { type: "string" }, description: "图形 id（whiteboard_read 里看到的）" },
        select: { type: "boolean", description: "顺带选中它们（默认 false）" },
      },
      required: ["ids"],
    },
  },
  {
    name: "snapshot",
    description: "把白板（或其中几个图形）截成 PNG 图片，看用户画的草图、排版用。",
    inputSchema: {
      type: "object",
      properties: {
        ids: { type: "array", items: { type: "string" }, description: "只截这些图形；不给截全部" },
      },
    },
  },
];

function text(t: string, structured?: Record<string, unknown>) {
  return { content: [{ type: "text" as const, text: t }], ...(structured ? { structuredContent: structured } : {}) };
}

function fail(t: string) {
  return { content: [{ type: "text" as const, text: t }], isError: true };
}

function labelOf(e: El, all: readonly El[]): string {
  if (e.type === "text") return e.text ?? "";
  return all.find((x) => !x.isDeleted && x.containerId === e.id)?.text ?? "";
}

function brief(e: El, all: readonly El[]) {
  const r = Math.round;
  return {
    id: e.id,
    type: e.type,
    label: labelOf(e, all).slice(0, 80),
    x: r(e.x),
    y: r(e.y),
    width: r(Math.abs(e.width)),
    height: r(Math.abs(e.height)),
  };
}

function line(b: ReturnType<typeof brief>): string {
  return `${b.type} ${b.id}${b.label ? `「${b.label}」` : ""} (${b.x},${b.y}) ${b.width}×${b.height}`;
}

/** 顶层图形（不含容器里的标签文字、已删除的）。 */
const shapes = (all: readonly El[]) => all.filter((e) => !e.isDeleted && !e.containerId);

function ids(args: Record<string, unknown> | undefined): string[] {
  const v = args?.ids;
  return Array.isArray(v) ? v.filter((x): x is string => typeof x === "string" && x.length > 0) : [];
}

async function blobBase64(blob: Blob): Promise<string> {
  const bytes = new Uint8Array(await blob.arrayBuffer());
  let bin = "";
  for (let i = 0; i < bytes.length; i += 0x8000) bin += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(bin);
}

/**
 * 给 `app` 装上界面工具。`api()` 取当前画布（还没挂上回 null），`inPanel()` 判断这份界面是不是
 * 面板里那份。显示方式变了调返回的函数，告诉宿主工具表变了。
 */
export function installViewTools(app: App, api: () => ViewApi | null, inPanel: () => boolean): () => void {
  app.onlisttools = async () => ({ tools: inPanel() ? (TOOLS as never) : [] });
  app.oncalltool = async (params) => {
    const a = api();
    if (!inPanel() || !a) return fail("白板面板没开着，这个工具用不了") as never;
    const all = a.getSceneElementsIncludingDeleted();
    const args = (params.arguments ?? {}) as Record<string, unknown>;
    switch (params.name) {
      case "get_selection": {
        const selected = Object.keys(a.getAppState().selectedElementIds ?? {})
          .map((id) => all.find((e) => e.id === id))
          .filter((e): e is El => !!e && !e.isDeleted && !e.containerId)
          .map((e) => brief(e, all));
        return text(
          selected.length ? `用户选中了 ${selected.length} 个图形：\n${selected.map(line).join("\n")}` : "用户当前没有选中图形。",
          { selected },
        ) as never;
      }
      case "get_viewport": {
        const s = a.getAppState();
        const zoom = s.zoom?.value ?? 1;
        const view = { x: -s.scrollX, y: -s.scrollY, width: s.width / zoom, height: s.height / zoom };
        const r = Math.round;
        const inView = shapes(all)
          .filter((e) => {
            const x1 = Math.min(e.x, e.x + e.width);
            const y1 = Math.min(e.y, e.y + e.height);
            const x2 = Math.max(e.x, e.x + e.width);
            const y2 = Math.max(e.y, e.y + e.height);
            return x2 >= view.x && x1 <= view.x + view.width && y2 >= view.y && y1 <= view.y + view.height;
          })
          .map((e) => brief(e, all));
        const area = { x: r(view.x), y: r(view.y), width: r(view.width), height: r(view.height), zoom: Math.round(zoom * 100) / 100 };
        return text(
          `用户看着画布 (${area.x},${area.y}) 起 ${area.width}×${area.height} 的区域（缩放 ${area.zoom}），视野里 ${inView.length} 个图形` +
            (inView.length ? `：\n${inView.map(line).join("\n")}` : "。"),
          { viewport: area, elements: inView },
        ) as never;
      }
      case "focus": {
        const want = new Set(ids(args));
        const found = shapes(all).filter((e) => want.has(e.id));
        if (found.length === 0) return fail(`白板上没有这些图形：${[...want].join("、") || "（没给 ids）"}`) as never;
        a.scrollToContent(found, { fitToContent: true, animate: true });
        if (args.select === true) {
          a.updateScene({ appState: { selectedElementIds: Object.fromEntries(found.map((e) => [e.id, true])) } });
        }
        const missing = [...want].filter((id) => !found.some((e) => e.id === id));
        return text(
          `已把视图移到 ${found.length} 个图形${args.select === true ? "并选中" : ""}。` + (missing.length ? `没找到：${missing.join("、")}。` : ""),
        ) as never;
      }
      case "snapshot": {
        const want = new Set(ids(args));
        const live = all.filter((e) => !e.isDeleted);
        const picked = want.size
          ? live.filter((e) => want.has(e.id) || (e.containerId && want.has(e.containerId)))
          : live;
        if (picked.length === 0) return fail(want.size ? "白板上没有这些图形" : "白板是空的") as never;
        const blob = await exportToBlob({
          elements: picked as never,
          appState: { ...a.getAppState(), exportBackground: true, exportWithDarkMode: false } as never,
          files: a.getFiles() as never,
          mimeType: "image/png",
          getDimensions: (w: number, h: number) => {
            const scale = Math.min(2, SNAPSHOT_MAX / Math.max(w, h, 1));
            return { width: Math.round(w * scale), height: Math.round(h * scale), scale };
          },
        });
        return {
          content: [
            { type: "image", data: await blobBase64(blob), mimeType: "image/png" },
            { type: "text", text: `白板截图（${picked.filter((e) => !e.containerId).length} 个图形）。` },
          ],
        } as never;
      }
      default:
        return fail(`白板界面没有工具 ${params.name}`) as never;
    }
  };
  return () => void app.sendToolListChanged().catch(() => {});
}
