// 会话头「更多」菜单、侧栏右键菜单、标题内联改名、删除确认。
// 设计：wonder-ui/shell/SessionMoreMenu（dubs / dalk / gelt）、SidebarSessionMenu（ber）。
// 「归档」没做：Dock 的 archive 是把当前会话存进历史，不是从列表里藏起来。

import { useEffect, useLayoutEffect, useRef, useState } from "react";
import {
  AppWindow,
  Check,
  Columns2,
  Copy,
  Ellipsis,
  FolderOpen,
  MessageSquare,
  PenLine,
  Pin,
  PinOff,
  SquarePen,
  SquareSplitHorizontal,
  Trash2,
  Undo2,
  Unplug,
  X,
} from "lucide-react";
import { openPath } from "@tauri-apps/plugin-opener";
import { Popover } from "./Popover";
import { imeComposing } from "../dock/ime";
import { useApp } from "../dock/store";

const item = "motion-colors flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-xs";
const normal = `${item} text-ink hover:bg-[#0a84ff14]`;

export function SessionMenu({
  sessionId,
  cwd,
  onRename,
  onDelete,
}: {
  sessionId: string;
  cwd: string;
  onRename: () => void;
  onDelete: () => void;
}) {
  // 连远程 Dock 时路径在远端，本机访达打不开。
  const remote = useApp().remote;
  return (
    <Popover
      title="更多"
      side="bottom"
      align="right"
      panelClassName="w-[220px] px-1.5 py-1.5 gap-0.5"
      className="flex size-7 items-center justify-center rounded-lg text-ink-4 hover:bg-hover hover:text-ink aria-expanded:bg-hover aria-expanded:text-ink"
      trigger={<Ellipsis size={16} />}
    >
      {(close) => {
        const run = (fn: () => void) => () => {
          close();
          fn();
        };
        return (
          <>
            <button type="button" className={normal} onClick={run(onRename)}>
              <PenLine size={14} className="text-ink-3" />
              改名
            </button>
            <button type="button" className={normal} onClick={run(() => navigator.clipboard.writeText(sessionId))}>
              <Copy size={14} className="text-ink-3" />
              复制会话 ID
            </button>
            {!remote && (
              <button type="button" className={normal} onClick={run(() => openPath(cwd).catch(() => {}))}>
                <FolderOpen size={14} className="text-ink-3" />
                在访达中打开项目目录
              </button>
            )}
            <div className="mx-0 my-1 h-px bg-line-strong" />
            <button type="button" className={`${item} font-medium text-[#ff3b30] hover:bg-[#ff3b3014]`} onClick={run(onDelete)}>
              <Trash2 size={14} />
              删除
            </button>
          </>
        );
      }}
    </Popover>
  );
}

/** 右键菜单的一项；`null` = 分隔线。 */
export type MenuEntry<A extends string> =
  | { action: A; icon: typeof Copy; label: string; hint?: string; danger?: boolean }
  | null;

/**
 * 右键菜单外壳：钉在鼠标位置，出屏就往回挪。点外面、Esc、滚动、窗口失焦都收起。
 * 设计：wonder-ui/shell/SidebarSessionMenu（ber）、SidebarProjectMenu（aru）。
 */
export function ContextMenu<A extends string>({
  x,
  y,
  entries,
  onAction,
  onClose,
}: {
  x: number;
  y: number;
  entries: MenuEntry<A>[];
  onAction: (action: A) => void;
  onClose: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ left: x, top: y });
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const { width, height } = el.getBoundingClientRect();
    setPos({
      left: Math.max(8, Math.min(x, window.innerWidth - width - 8)),
      top: Math.max(8, Math.min(y, window.innerHeight - height - 8)),
    });
  }, [x, y]);
  useEffect(() => {
    const onDown = (e: MouseEvent) => !ref.current?.contains(e.target as Node) && onClose();
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    // 捕获阶段：侧栏树自己滚动也要收起。
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    window.addEventListener("scroll", onClose, true);
    window.addEventListener("blur", onClose);
    window.addEventListener("resize", onClose);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("scroll", onClose, true);
      window.removeEventListener("blur", onClose);
      window.removeEventListener("resize", onClose);
    };
  }, [onClose]);

  // 相邻 / 首尾的分隔线去掉（某组的项全被条件藏掉时）。
  const shown = entries.filter((e, i, all) => e || (i > 0 && i < all.length - 1 && all[i - 1] && all.slice(i + 1).some(Boolean)));
  return (
    <div
      ref={ref}
      role="menu"
      onContextMenu={(e) => e.preventDefault()}
      style={{ left: pos.left, top: pos.top, transformOrigin: "left top" }}
      className="glass-popover pop-in fixed z-50 flex w-[232px] flex-col gap-0.5 rounded-lg border border-line-strong bg-white p-1.5 shadow-xl"
    >
      {shown.map((e, i) =>
        e ? (
          <button
            key={e.action}
            type="button"
            role="menuitem"
            className={e.danger ? `${item} font-medium text-[#ff3b30] hover:bg-[#ff3b3014]` : normal}
            onClick={() => {
              onClose();
              onAction(e.action);
            }}
          >
            <e.icon size={14} className={`shrink-0 ${e.danger ? "" : "text-ink-3"}`} />
            {e.label}
            {e.hint && <span className="ml-auto text-[11px] text-ink-4">{e.hint}</span>}
          </button>
        ) : (
          <div key={`d${i}`} className="my-1 h-px shrink-0 bg-line-strong" />
        ),
      )}
    </div>
  );
}

export type SessionAction =
  | "open"
  | "split"
  | "detach"
  | "window"
  | "rename"
  | "copy-id"
  | "finder"
  | "disconnect"
  | "delete";

/** 侧栏会话行的右键菜单。「断开」只给开着且有对话的会话（空会话 Dock 关掉不落盘，等于删了）。 */
export function SessionContextMenu({
  x,
  y,
  canDisconnect,
  onAction,
  onClose,
}: {
  x: number;
  y: number;
  canDisconnect: boolean;
  onAction: (action: SessionAction) => void;
  onClose: () => void;
}) {
  const remote = useApp().remote;
  const entries: MenuEntry<SessionAction>[] = [
    { action: "open", icon: MessageSquare, label: "打开" },
    { action: "split", icon: Columns2, label: "在右侧分屏打开", hint: "⌘ 点击" },
    { action: "detach", icon: SquareSplitHorizontal, label: "单独面板" },
    { action: "window", icon: AppWindow, label: "在新窗口打开" },
    null,
    { action: "rename", icon: PenLine, label: "改名" },
    { action: "copy-id", icon: Copy, label: "复制会话 ID" },
    remote ? null : { action: "finder", icon: FolderOpen, label: "在访达中打开项目目录" },
    null,
    canDisconnect ? { action: "disconnect", icon: Unplug, label: "断开", hint: "记录保留" } : null,
    { action: "delete", icon: Trash2, label: "删除…", danger: true },
  ];
  return <ContextMenu x={x} y={y} entries={entries} onAction={onAction} onClose={onClose} />;
}

export type ProjectAction = "new-chat" | "pin" | "unpin" | "rename" | "copy-path" | "finder" | "disconnect-all" | "remove";

/** 侧栏项目行的右键菜单（设计 aru）。`openCount` = 这个项目下开着的会话数，0 就不给「断开全部」。 */
export function ProjectContextMenu({
  x,
  y,
  pinned,
  openCount,
  onAction,
  onClose,
}: {
  x: number;
  y: number;
  pinned: boolean;
  openCount: number;
  onAction: (action: ProjectAction) => void;
  onClose: () => void;
}) {
  const remote = useApp().remote;
  const entries: MenuEntry<ProjectAction>[] = [
    { action: "new-chat", icon: SquarePen, label: "新对话" },
    pinned ? { action: "unpin", icon: PinOff, label: "取消置顶" } : { action: "pin", icon: Pin, label: "置顶" },
    null,
    { action: "rename", icon: PenLine, label: "改名" },
    { action: "copy-path", icon: Copy, label: "复制路径" },
    remote ? null : { action: "finder", icon: FolderOpen, label: "在访达中显示" },
    null,
    openCount > 0
      ? { action: "disconnect-all", icon: Unplug, label: "断开全部会话", hint: `${openCount} 个开着` }
      : null,
    { action: "remove", icon: X, label: "移除项目…", danger: true },
  ];
  return <ContextMenu x={x} y={y} entries={entries} onAction={onAction} onClose={onClose} />;
}

/** 会话头标题的改名输入框：Enter 保存、Esc 取消；点外面也算保存。 */
export function RenameInput({
  title,
  onSave,
  onCancel,
}: {
  title: string;
  onSave: (title: string) => void;
  onCancel: () => void;
}) {
  const [value, setValue] = useState(title);
  const ref = useRef<HTMLInputElement>(null);
  useEffect(() => ref.current?.select(), []);
  const save = () => {
    const t = value.trim();
    if (t && t !== title) onSave(t);
    else onCancel();
  };
  return (
    <div className="pop-in flex h-7 min-w-0 items-center gap-1.5 rounded-md border border-accent bg-white px-2 shadow-sm">
      <input
        ref={ref}
        value={value}
        onChange={(e) => setValue(e.target.value)}
        onBlur={save}
        onKeyDown={(e) => {
          if (imeComposing(e)) return;
          if (e.key === "Enter") save();
          if (e.key === "Escape") onCancel();
        }}
        style={{ width: `${Math.min(40, Math.max(8, textWidth(value) + 1))}ch` }}
        className="min-w-0 bg-transparent text-[13px] font-medium text-ink outline-none selection:bg-[#0a84ff33] select-text"
      />
      <button
        type="button"
        title="保存"
        onMouseDown={(e) => e.preventDefault()}
        onClick={save}
        className="flex size-[18px] shrink-0 items-center justify-center rounded-sm bg-ink text-white"
      >
        <Check size={12} />
      </button>
      <button
        type="button"
        title="取消"
        onMouseDown={(e) => e.preventDefault()}
        onClick={onCancel}
        className="flex size-[18px] shrink-0 items-center justify-center rounded-sm bg-[#e8e8ec] text-ink-3"
      >
        <X size={12} />
      </button>
    </div>
  );
}

/** 输入框宽度（ch）：中日韩字符按两个算。 */
function textWidth(s: string): number {
  let n = 0;
  for (const c of s) n += /[\u2e80-\uffef]/.test(c) ? 2 : 1;
  return n;
}

/** 删除会话确认。 */
export function DeleteConfirm({ onConfirm, onCancel }: { onConfirm: () => void; onCancel: () => void }) {
  return (
    <Confirm
      icon={Trash2}
      title="删除这个会话？"
      body="记录会从磁盘移除，此操作无法撤销。"
      confirmLabel="删除"
      onConfirm={onConfirm}
      onCancel={onCancel}
    />
  );
}

/** 撤回消息确认：这条和之后的对话都删掉；工具做过的事不撤销。 */
export function RewindConfirm({ onConfirm, onCancel }: { onConfirm: () => void; onCancel: () => void }) {
  return (
    <Confirm
      icon={Undo2}
      title="撤回这条消息？"
      body="这条和它之后的对话都会删掉，正文放回输入框。已经改过的文件、跑过的命令不会撤销。"
      confirmLabel="撤回"
      onConfirm={onConfirm}
      onCancel={onCancel}
    />
  );
}

/** 从侧栏移除项目确认：只是不再显示，会话记录和目录都不动。 */
export function RemoveProjectConfirm({
  name,
  sessions,
  onConfirm,
  onCancel,
}: {
  name: string;
  sessions: number;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  return (
    <Confirm
      icon={X}
      title={`从侧栏移除「${name}」？`}
      body={`${
        sessions > 0 ? `它下面 ${sessions} 个会话的记录留在磁盘上，搜索仍找得到；` : ""
      }项目目录不动。用「+」再建同一个项目就会回来。`}
      confirmLabel="移除"
      onConfirm={onConfirm}
      onCancel={onCancel}
    />
  );
}

/** 危险操作确认：盖一层半透明遮罩，居中小卡。Esc / 点遮罩取消。 */
function Confirm({
  icon: Icon,
  title,
  body,
  confirmLabel,
  onConfirm,
  onCancel,
}: {
  icon: typeof Trash2;
  title: string;
  body: string;
  confirmLabel: string;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onCancel();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onCancel]);
  return (
    <div className="enter-fade fixed inset-0 z-50 flex items-center justify-center bg-black/15" onMouseDown={onCancel}>
      <div
        role="alertdialog"
        aria-label={title}
        onMouseDown={(e) => e.stopPropagation()}
        className="glass-dialog pop-in flex w-[360px] flex-col gap-5 rounded-[10px] border border-line-strong bg-white p-4 shadow-xl"
      >
        <div className="flex items-start gap-3">
          <span className="mt-0.5 flex size-8 shrink-0 items-center justify-center rounded-full bg-[#fef2f2] text-[#ff3b30]">
            <Icon size={16} />
          </span>
          <div className="flex flex-col gap-1">
            <span className="text-sm font-semibold text-ink">{title}</span>
            <span className="text-xs leading-[1.4] text-ink-3">{body}</span>
          </div>
        </div>
        <div className="flex justify-end gap-2">
          <button
            type="button"
            onClick={onCancel}
            className="motion-colors flex h-[26px] items-center rounded-[7px] border border-line-strong bg-white px-3 text-[11px] font-medium text-ink hover:bg-hover"
          >
            取消
          </button>
          <button
            type="button"
            autoFocus
            onClick={onConfirm}
            className="motion-colors flex h-[26px] items-center rounded-[7px] bg-[#ff3b30] px-3 text-[11px] font-medium text-white shadow-sm hover:bg-[#e0352b] active:scale-[0.97]"
          >
            {confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}
