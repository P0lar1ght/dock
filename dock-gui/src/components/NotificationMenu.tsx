// 铃铛通知菜单：「在等你」（权限 / 提问 / 计划 / elicitation）和「刚完成」（不在看
// 的时候跑完的轮次）。外观照 wonder-ui/shell/NotificationMenu（quag / wiss）。
// 点一条跳到那个会话；打开过的会话由 store.focus 移出「刚完成」。

import { Bell, CircleAlert, CircleCheck, ClipboardList, MessageCircleQuestion, Plug, Shield, type LucideIcon } from "lucide-react";
import { pendingItems, type PendingItem } from "dock-core";
import { store, type AppState, type FinishedTurn } from "../dock/store";
import { formatElapsed } from "./chrome";
import { permissionParts } from "../dock/permission";
import { Popover } from "./Popover";

interface Row {
  sessionId: string;
  title: string;
  detail: string;
  at: number;
  icon: LucideIcon;
  tone: "wait" | "done" | "fail";
}

const TONE = {
  wait: { badge: "bg-[#0a84ff1a]", icon: "text-accent", detail: "text-ink-4" },
  done: { badge: "bg-[#34c7591a]", icon: "text-[#34c759]", detail: "text-ink-4" },
  fail: { badge: "bg-[#ff3b301a]", icon: "text-[#ff3b30]", detail: "text-[#c0392b]" },
};

const PENDING_ICON: Record<PendingItem["kind"], LucideIcon> = {
  permission: Shield,
  question: MessageCircleQuestion,
  plan: ClipboardList,
  elicit: Plug,
};

function pendingDetail(item: PendingItem): string {
  switch (item.kind) {
    case "permission":
      return `允许 ${item.toolName || "工具"}？ ${permissionBrief(item.toolName, item.summary)}`.trim();
    case "question":
      return `Dock 在问你：${item.questions[0]?.question ?? ""}`;
    case "plan":
      return "计划待批准";
    case "elicit":
      return item.heading || `${item.server} 请求确认`;
  }
}

/**
 * 权限摘要是「工具名 + 参数 JSON」：一行里只放最能说明这次调用的那个参数
 * （命令、路径、网址），认不出就原样。
 */
function permissionBrief(tool: string, summary: string): string {
  // 命令工具：`$ 命令` + 换行 + 描述。通知里一句话，描述比命令好读。
  if (summary.startsWith("$ ")) {
    const { main, note } = permissionParts(summary);
    return note || main;
  }
  const rest = summary.startsWith(tool) ? summary.slice(tool.length).trim() : summary;
  try {
    const args = JSON.parse(rest) as Record<string, unknown>;
    const key = ["command", "path", "file_path", "url", "query"].find((k) => typeof args[k] === "string");
    if (key) return String(args[key]);
  } catch {
    // 不是 JSON：原样。
  }
  return rest;
}

function finishedDetail(f: FinishedTurn): string {
  return f.status === "failed"
    ? `出错：${f.error || "Dock 没给原因"}`
    : `已完成 · 用时 ${formatElapsed(f.elapsedMs)}`;
}

/** 「刚刚」「N 分钟前」「N 小时前」，再久写日期。 */
function ago(ms: number): string {
  const min = Math.floor((Date.now() - ms) / 60_000);
  if (min < 1) return "刚刚";
  if (min < 60) return `${min} 分钟前`;
  if (min < 24 * 60) return `${Math.floor(min / 60)} 小时前`;
  const d = new Date(ms);
  return `${d.getMonth() + 1}月${d.getDate()}日`;
}

function rows(app: AppState): { waiting: Row[]; finished: Row[] } {
  const title = (id: string) => app.sessions.find((s) => s.id === id)?.title ?? "会话";
  const waiting: Row[] = [];
  for (const id of app.waiting) {
    const item = pendingItems(store.getThread(id))[0];
    if (!item) continue;
    waiting.push({
      sessionId: id,
      title: title(id),
      detail: pendingDetail(item),
      at: store.waitingSince(id) ?? Date.now(),
      icon: PENDING_ICON[item.kind],
      tone: "wait",
    });
  }
  const finished: Row[] = [...app.finished].map(([id, f]) => ({
    sessionId: id,
    title: title(id),
    detail: finishedDetail(f),
    at: f.at,
    icon: f.status === "failed" ? CircleAlert : CircleCheck,
    tone: f.status === "failed" ? "fail" : "done",
  }));
  const newest = (a: Row, b: Row) => b.at - a.at;
  return { waiting: waiting.sort(newest), finished: finished.sort(newest) };
}

/** `rail`：紧凑铃铛（大一号，菜单从右侧弹出）。设计：SidebarRailBell（berg / byth）。 */
export function NotificationMenu({
  app,
  onOpen,
  rail = false,
}: {
  app: AppState;
  onOpen: (sessionId: string) => void;
  rail?: boolean;
}) {
  const count = app.waiting.size + app.finished.size;
  return (
    <Popover
      title={count > 0 ? `通知 · ${count}` : "通知"}
      side={rail ? "right" : "bottom"}
      align="left"
      panelClassName="w-[300px] max-h-[420px]"
      className={`relative flex items-center justify-center text-ink-3 hover:bg-hover hover:text-ink aria-expanded:bg-[#e8e8ec] aria-expanded:text-ink ${
        rail ? "size-8 rounded-lg" : "size-7 rounded-md"
      }`}
      trigger={
        <>
          <Bell size={16} strokeWidth={2} />
          {count > 0 && (
            <span
              className={`pop-in absolute size-1.5 rounded-full bg-accent ${rail ? "right-1 top-1 box-content border border-white" : "right-1.5 top-1.5"}`}
            />
          )}
        </>
      }
    >
      {(close) => {
        const { waiting, finished } = rows(app);
        const open = (id: string) => {
          close();
          onOpen(id);
        };
        if (waiting.length === 0 && finished.length === 0) {
          return (
            <div className="flex flex-col items-center gap-2 px-4 py-6">
              <div className="flex size-8 items-center justify-center rounded-full bg-[#f2f2f7]">
                <Bell size={16} className="text-ink-5" />
              </div>
              <div className="text-[13px] font-medium text-ink-2">没有要处理的</div>
              <div className="text-[11px] text-ink-4">会话等你确认或在后台跑完时，会出现在这里</div>
            </div>
          );
        }
        return (
          <>
            {waiting.length > 0 && <Section title={`在等你 · ${waiting.length}`} rows={waiting} onOpen={open} />}
            {waiting.length > 0 && finished.length > 0 && <div className="mx-2.5 my-1 h-px shrink-0 bg-line-soft" />}
            {finished.length > 0 && <Section title="刚完成" rows={finished} onOpen={open} />}
            <div className="mt-1 shrink-0 border-t border-line-soft px-2.5 pb-1 pt-1.5 text-[11px] text-ink-4">
              点一条跳到那个会话；看过的会移出
            </div>
          </>
        );
      }}
    </Popover>
  );
}

function Section({ title, rows, onOpen }: { title: string; rows: Row[]; onOpen: (id: string) => void }) {
  return (
    <>
      <div className="shrink-0 px-2.5 pb-1 pt-2 text-[11px] font-semibold tracking-[0.275px] text-ink-4">{title}</div>
      {rows.map((r) => {
        const tone = TONE[r.tone];
        return (
          <button
            key={r.sessionId}
            type="button"
            onClick={() => onOpen(r.sessionId)}
            className="motion-colors flex shrink-0 items-start gap-2.5 px-2.5 py-2 text-left hover:bg-[#0a84ff14]"
          >
            <span className={`flex size-6 shrink-0 items-center justify-center rounded-full ${tone.badge}`}>
              <r.icon size={14} className={tone.icon} />
            </span>
            <span className="flex min-w-0 flex-1 flex-col gap-0.5">
              <span className="flex items-center gap-2">
                <span className="min-w-0 flex-1 truncate text-[13px] font-medium leading-[18px] text-ink">{r.title}</span>
                <span className="shrink-0 text-[11px] leading-[18px] text-ink-5">{ago(r.at)}</span>
              </span>
              <span className={`truncate text-xs leading-4 ${tone.detail}`}>{r.detail}</span>
            </span>
          </button>
        );
      })}
    </>
  );
}
