// 界面工具用到的纯函数（不碰 Excalidraw，Node 自带 test 直接跑：npm test）。

/* eslint-disable @typescript-eslint/no-explicit-any */
export type El = Record<string, any> & { id: string; type: string; isDeleted?: boolean };

export interface Brief {
  id: string;
  type: string;
  label: string;
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface Area {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** 图形的标签：文字自己的内容，或框里那段文字。 */
export function labelOf(e: El, all: readonly El[]): string {
  if (e.type === "text") return e.text ?? "";
  return all.find((x) => !x.isDeleted && x.containerId === e.id)?.text ?? "";
}

export function brief(e: El, all: readonly El[]): Brief {
  const r = Math.round;
  return {
    id: e.id,
    type: e.type,
    label: labelOf(e, all).slice(0, 80),
    x: r(Math.min(e.x, e.x + e.width)),
    y: r(Math.min(e.y, e.y + e.height)),
    width: r(Math.abs(e.width)),
    height: r(Math.abs(e.height)),
  };
}

export function line(b: Brief): string {
  return `${b.type} ${b.id}${b.label ? `「${b.label}」` : ""} (${b.x},${b.y}) ${b.width}×${b.height}`;
}

/** 顶层图形（不含容器里的标签文字、已删除的）。 */
export const shapes = (all: readonly El[]) => all.filter((e) => !e.isDeleted && !e.containerId);

/** 参数里的 `ids`：只留非空字符串。 */
export function idsOf(args: Record<string, unknown> | undefined): string[] {
  const v = args?.ids;
  return Array.isArray(v) ? v.filter((x): x is string => typeof x === "string" && x.length > 0) : [];
}

/** Excalidraw 的 appState → 用户看着的画布区域（画布坐标）。 */
export function viewportOf(s: { scrollX: number; scrollY: number; width: number; height: number; zoom?: { value: number } }): Area & { zoom: number } {
  const zoom = s.zoom?.value ?? 1;
  return { x: -s.scrollX, y: -s.scrollY, width: s.width / zoom, height: s.height / zoom, zoom };
}

/** 包围盒和区域相交（大致：旋转、多点线按未旋转的包围盒算）。 */
export function intersects(e: El, view: Area): boolean {
  const x1 = Math.min(e.x, e.x + e.width);
  const y1 = Math.min(e.y, e.y + e.height);
  const x2 = Math.max(e.x, e.x + e.width);
  const y2 = Math.max(e.y, e.y + e.height);
  return x2 >= view.x && x1 <= view.x + view.width && y2 >= view.y && y1 <= view.y + view.height;
}

/** 截图要带上的元素：选中的图形、框里的文字，和两端都在选中图形上的箭头 / 线。不给 ids 截全部。 */
export function snapshotElements(all: readonly El[], ids: readonly string[]): El[] {
  const live = all.filter((e) => !e.isDeleted);
  if (ids.length === 0) return live;
  const want = new Set(ids);
  const picked = new Set(
    live
      .filter(
        (e) =>
          (want.has(e.id) && !e.containerId) ||
          (e.startBinding?.elementId && e.endBinding?.elementId && want.has(e.startBinding.elementId) && want.has(e.endBinding.elementId)),
      )
      .map((e) => e.id),
  );
  // 框里的文字、箭头上的字跟着容器走。
  return live.filter((e) => picked.has(e.id) || (e.containerId && picked.has(e.containerId)));
}
