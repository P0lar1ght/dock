// 对话流里的画布卡：`canvas_create` / `canvas_edit` / `canvas_data` 的工具项。
// 设计：wonder-ui/chat/FeedCanvasChip（34px 基底）+ coin 画板的四态（生成中 / 已生成 / 已更新 / 运行出错）。

import { createContext, useContext } from "react";
import { CircleAlert, FileText, Layers, RefreshCw, Sparkles, type LucideIcon } from "lucide-react";
import { store, useCanvas } from "../dock/store";
import { fixPrompt, type CanvasEvent } from "./canvasDoc";

/** 线程里每块画布的标题，和每块画布最新那一条工具项（只有它显示运行时报错）。 */
export interface CanvasIndex {
  titles: ReadonlyMap<string, string>;
  latest: ReadonlyMap<string, string>;
}

export const CanvasIndexContext = createContext<CanvasIndex>({ titles: new Map(), latest: new Map() });

type Tone = "blue" | "purple" | "red" | "gray";

const TONE: Record<Tone, { box: string; icon: string; link: string }> = {
  blue: { box: "border-[#0a84ff]/35 bg-white", icon: "text-accent", link: "text-accent" },
  purple: { box: "border-[#af52de]/35 bg-white", icon: "text-[#af52de]", link: "text-[#af52de]" },
  red: { box: "border-[#ff3b30]/35 bg-[#fff5f5]", icon: "text-[#ff3b30]", link: "text-[#ff3b30]" },
  gray: { box: "border-line-strong bg-white", icon: "text-ink-4", link: "text-ink-3" },
};

export function CanvasChip({ ev, threadId }: { ev: CanvasEvent; threadId: string }) {
  const index = useContext(CanvasIndexContext);
  const canvas = useCanvas();
  const id = ev.canvasId;
  const key = id ? `${threadId}#${id}` : null;
  const title = ev.title ?? (id ? index.titles.get(id) : null) ?? "画布";
  const shown = !!key && canvas.shown === key;
  const isLatest = !!id && index.latest.get(id) === ev.itemId;
  const runtime = key && isLatest ? canvas.errors.get(key) : undefined;
  const open = () => store.openCanvas(threadId, id);

  let tone: Tone;
  let icon: LucideIcon;
  let text: string;
  let link: string | null = shown ? "右侧已展开 ✓" : "打开";
  let action = open;
  let pulse = false;

  if (runtime) {
    tone = "red";
    icon = CircleAlert;
    const kind = runtime.message.match(/^(\w*Error)\b/)?.[1];
    text = `${title} · 运行时报错${kind ? `（${kind}）` : ""}`;
    link = "让 Agent 修复";
    action = () => store.fillComposer(threadId, fixPrompt(title, id!, runtime), "append");
  } else if (ev.status === "running") {
    pulse = true;
    if (ev.tool === "canvas_create") {
      tone = "blue";
      icon = Layers;
      text = `${title} · 正在生成…`;
      link = "查看进度";
    } else {
      tone = "purple";
      icon = RefreshCw;
      text = `${title} · 正在更新…`;
      link = shown ? "右侧已展开 ✓" : "查看";
    }
  } else if (ev.status === "failed") {
    tone = "red";
    icon = CircleAlert;
    text = `${title} · 没改成：${ev.error ?? "出错了"}`;
    link = id ? link : null;
  } else if (ev.status !== "completed") {
    tone = "gray";
    icon = Layers;
    text = `${title} · ${ev.status === "denied" ? "被拒绝" : "已中断"}`;
    link = null;
  } else if (ev.tool === "canvas_create") {
    tone = "blue";
    icon = FileText;
    text = `${title} · 已生成 v${ev.version ?? 1}`;
  } else if (ev.tool === "canvas_edit") {
    tone = "purple";
    icon = RefreshCw;
    text = `${title} · 已更新至 v${ev.version ?? "?"}${ev.note ? `（${ev.note}）` : ""}`;
    if (!shown) link = "打开最新";
  } else {
    tone = "blue";
    icon = RefreshCw;
    text = `${title} · 数据已更新`;
  }

  const t = TONE[tone];
  const Icon = icon;
  return (
    <div
      className={`enter-up flex h-[34px] max-w-[560px] shrink-0 items-center gap-2 rounded-[9px] border px-2.5 shadow-sm ${t.box}`}
    >
      {pulse ? (
        <span className="relative flex size-[13px] shrink-0 items-center justify-center">
          <span className={`absolute size-2 animate-ping rounded-full opacity-60 ${tone === "blue" ? "bg-accent" : "bg-[#af52de]"}`} />
          <span className={`size-2 rounded-full ${tone === "blue" ? "bg-accent" : "bg-[#af52de]"}`} />
        </span>
      ) : (
        <Icon size={13} className={`shrink-0 ${t.icon}`} />
      )}
      <span
        className={`min-w-0 flex-1 truncate text-xs font-medium ${tone === "red" ? "text-[#d70015]" : "text-ink"}`}
        title={text}
      >
        {text}
      </span>
      {link && (
        <button
          type="button"
          onClick={action}
          className={`flex shrink-0 items-center gap-1 text-[11px] font-medium hover:underline ${t.link}`}
        >
          {runtime && <Sparkles size={11} />}
          {link}
        </button>
      )}
    </div>
  );
}
