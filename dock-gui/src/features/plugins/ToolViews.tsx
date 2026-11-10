// 插件给工具登记的卡片视图。设计：wonder-ui/plugins/ToolPluginExpanded（gan）、
// ToolPluginCollapsed（oxy）、ToolPluginFallback（pci）。
// - 有视图的工具：头部图标换成 Puzzle（契约里插件不能给图标）。
// - 展开时按需拉 `tool/view`（只给受信 ticket，会跑插件脚本）；右上角「视图 | 原始」切换。
// - 插件这次不给视图（`view: null`）或拉失败：照常画原始输出。
// 按需工具经 `use_tool` 调用，按里面那颗的名字找（dock-core `viewToolName`）。

import { useEffect, useState, useSyncExternalStore } from "react";
import { Puzzle } from "lucide-react";
import { viewToolName, type ViewNode } from "dock-core";
import type { ToolBodyProps } from "../registry";
import { onPluginPush, toolView, toolViewNames } from "./api";
import { ViewTree } from "./ViewTree";

// 有视图的工具名：全局一份，`tool/views/changed` 时重拉。
let names = new Set<string>();
const listeners = new Set<() => void>();
let started = false;

function start() {
  if (started) return;
  started = true;
  const load = () =>
    toolViewNames().then(
      (list) => {
        names = new Set(list);
        listeners.forEach((fn) => fn());
      },
      () => {},
    );
  void load();
  onPluginPush((p) => p.kind === "toolViews" && void load());
}

function subscribe(fn: () => void) {
  start();
  listeners.add(fn);
  return () => listeners.delete(fn);
}

function useHasView(toolName: string, args: Record<string, unknown>): boolean {
  const set = useSyncExternalStore(subscribe, () => names);
  return set.has(viewToolName(toolName, args));
}

export function useToolIcon(toolName: string, args: Record<string, unknown>) {
  return useHasView(toolName, args) ? Puzzle : null;
}

export function ToolViewBody({ threadId, itemId, toolName, args, raw }: ToolBodyProps) {
  const has = useHasView(toolName, args);
  const [mode, setMode] = useState<"view" | "raw">("view");
  // undefined = 还在拉；null = 这次没有视图。
  const [view, setView] = useState<ViewNode | null | undefined>(undefined);

  useEffect(() => {
    if (!has) return;
    let alive = true;
    toolView(threadId, itemId).then(
      (v) => alive && setView(v),
      () => alive && setView(null),
    );
    return () => {
      alive = false;
    };
  }, [has, threadId, itemId]);

  if (!has || view === null) return <>{raw}</>;
  return (
    <div className="flex flex-col gap-2">
      <div className="flex justify-end">
        <div className="flex overflow-hidden rounded-md" role="tablist" aria-label="工具结果">
          {(["view", "raw"] as const).map((m) => (
            <button
              key={m}
              type="button"
              role="tab"
              aria-selected={mode === m}
              onClick={() => setMode(m)}
              className={`motion-colors flex h-6 items-center px-2 text-[11px] font-medium leading-none ${
                mode === m ? "bg-ink text-white" : "bg-[#f2f2f7] text-ink-3 hover:bg-[#e8e8ec]"
              }`}
            >
              {m === "view" ? "视图" : "原始"}
            </button>
          ))}
        </div>
      </div>
      {mode === "raw" ? (
        raw
      ) : view === undefined ? (
        <div className="h-16 animate-pulse rounded-lg bg-fill" />
      ) : (
        <div key="view" className="enter-fade">
          <ViewTree node={view} />
        </div>
      )}
    </div>
  );
}
