// 中间会话区：会话头、对话流、权限条、输入框。
// 设计：wonder-ui/shell/{SessionHeader,SessionHeaderRunning}、chat/PermissionStrip。

import { useEffect, useState, type ReactNode } from "react";
import { AppWindow, Ellipsis, PanelBottom, SquareSplitHorizontal, X } from "lucide-react";
import { isRunning, pendingItems } from "dock-core";
import type { SessionSummary } from "../model";
import { store, useApp, useEnv, useSubagents, useThread } from "../dock/store";
import { Composer } from "./Composer";
import { GoalBar } from "./GoalBar";
import { ConnectionBanner } from "./ConnectionBanner";
import { formatElapsed, IconButton } from "./chrome";
import { Pending } from "./Pending";
import { DeleteConfirm, RenameInput, SessionMenu } from "./SessionMenu";
import { Queued } from "./Queued";
import { presetIcon } from "./presetIcon";
import { COMPOSER_WIDTH, Transcript } from "./Transcript";
import { WindowControls, type SidebarProps } from "./Sidebar";

export interface SessionPaneProps {
  session: SessionSummary | null;
  /** 侧栏收起时，红绿灯和折叠按钮固定在窗口左上角（App 画），这里只留透明占位。 */
  windowControls: Pick<SidebarProps, "onToggleSidebar" | "onBack" | "onForward" | "canGoBack" | "canGoForward"> | null;
  /** 右侧栏是否展开：收起时头栏右侧给「侧边面板」按钮让位。 */
  panelOpen?: boolean;
  /** 折叠右侧那一栏的按钮（收起时才画在这儿；展开时它在右侧那一列的头栏里）。 */
  sideToggle?: ReactNode;
  /** 壳内：把这个会话拆到工作台。不给就不画这个按钮（拆出来的窗口里就没有）。 */
  onDetach?: () => void;
  /** 工作台格子：开成独立窗口（不用拖）。 */
  onPopOut?: () => void;
  /** 工作台格子：收回壳内。给了就把「单独面板」换成「重新停靠」。 */
  onDock?: () => void;
  /** 工作台格子左上角的拖手（拖出窗口）。 */
  dragHandle?: ReactNode;
  /** 没选会话时占满会话区的内容（新对话页）。 */
  empty: ReactNode;
  /** 当前会话被删掉了（回到新对话页）。 */
  onDeleted: () => void;
  /** 分屏时每个面板的「关闭面板」。 */
  onClosePane?: () => void;
  /** 关闭按钮的文字；工作台格子里叫「关闭」。 */
  closeLabel?: string;
  /** 顶部的连接横条；分屏时只有左边那个画，免得两条一样的。 */
  showConnection?: boolean;
}

/** 自己订阅当前会话（store 的 `useThread` / `useEnv`）：流式增量只让这里重渲染。 */
export function SessionPane(props: SessionPaneProps) {
  const { session } = props;
  const id = session?.id ?? null;
  const thread = useThread(id);
  const env = useEnv(id);
  const app = useApp();
  const stopping = app.stopping.has(id ?? "");
  const running = isRunning(thread);
  const subagents = useSubagents(session?.id ?? null);
  const subagentsRunning = subagents.order.filter((id) => subagents.byId[id].info.status === "running").length;
  const startedAt = running ? thread.turns[thread.turns.length - 1].startedAt : null;
  const pending = pendingItems(thread)[0];
  const PresetIcon = presetIcon(session?.presetId);
  const [renaming, setRenaming] = useState(false);
  const [deleting, setDeleting] = useState(false);
  useEffect(() => setRenaming(false), [id]);

  return (
    <section className="glass-pane flex min-h-0 min-w-0 flex-1 flex-col bg-pane">
      <header
        className={`relative flex h-11 shrink-0 items-center justify-between gap-3 border-b border-line-soft pl-4 ${
          // 右侧栏收起时，右上角空出来给那个「侧边面板」按钮（展开时它在右侧那一列的头栏里）：
          // 收起 / 展开时按钮都在窗口右上角同一个位置（右边距都是 12px）。
          props.panelOpen ? "pr-4" : "pr-3"
        }`}
        data-tauri-drag-region
      >
        <div className="flex min-w-0 items-center gap-2.5" data-tauri-drag-region>
          {props.dragHandle}
          {props.windowControls && (
            // 侧栏收起时这组按钮由 App 固定画在左上角（FloatingWindowControls），盖着的遮罩也挡不住；
            // 这里只留一块同样大小的透明占位，让标题从它后面开始（左边缘和固定按钮对齐：x=20）。
            <div className="invisible ml-1 mr-1 shrink-0" aria-hidden>
              <WindowControls {...props.windowControls} collapsed />
            </div>
          )}
          {session && renaming ? (
            <RenameInput
              title={session.title}
              onCancel={() => setRenaming(false)}
              onSave={(title) => {
                setRenaming(false);
                store.rename(session.id, title);
              }}
            />
          ) : (
            <span
              key={session?.id ?? "new"}
              onDoubleClick={() => session && setRenaming(true)}
              className="enter-fade truncate text-sm font-semibold text-ink"
            >
              {session?.title || "新对话"}
            </span>
          )}
          {session && (
            <span
              title="Agent 预设（创建时选定）"
              className="flex h-[22px] shrink-0 items-center gap-1 rounded-full bg-fill px-2 text-[11px] font-medium text-ink-3"
            >
              <PresetIcon size={11} />
              {session.preset}
            </span>
          )}
          {session && renaming && <span className="enter-fade shrink-0 text-[11px] text-ink-4">按 Enter 保存，Esc 取消</span>}
          {session && running && !renaming && <Running startedAt={startedAt} />}
          {session && !running && !renaming && subagentsRunning > 0 && (
            <SubagentsRunning count={subagentsRunning} onClick={() => store.openSubagent(session.id, null)} />
          )}
        </div>
        <div className="flex shrink-0 items-center gap-1.5">
          {session ? (
            <SessionMenu
              sessionId={session.id}
              cwd={session.cwd}
              onRename={() => setRenaming(true)}
              onDelete={() => setDeleting(true)}
            />
          ) : (
            <IconButton icon={Ellipsis} label="更多" disabled className="size-7 text-ink-5" />
          )}
          {session && props.onPopOut && (
            <IconButton icon={AppWindow} label="在新窗口打开" size={14} onClick={props.onPopOut} className="size-7" />
          )}
          {session && props.onDock && (
            <button
              type="button"
              title="收回壳内"
              onClick={props.onDock}
              className="motion-colors flex h-7 items-center gap-1 rounded-lg border border-line-strong bg-white px-2.5 text-xs font-medium text-ink-2 hover:bg-hover"
            >
              <PanelBottom size={12} className="text-ink-3" />
              重新停靠
            </button>
          )}
          {session && !props.onDock && props.onDetach && (
            <button
              type="button"
              title="拆到工作台（可再拖出成独立窗口）"
              onClick={props.onDetach}
              className="motion-colors flex h-7 items-center gap-1 rounded-lg border border-line-strong bg-white px-2.5 text-xs font-medium text-ink-2 hover:bg-hover"
            >
              <SquareSplitHorizontal size={12} className="text-ink-3" />
              单独面板
            </button>
          )}
          {props.sideToggle}
          {props.onClosePane && (
            <button
              type="button"
              onClick={props.onClosePane}
              className="motion-colors flex h-7 items-center gap-1 rounded-lg border border-line-strong bg-white px-2.5 text-xs font-medium text-ink-2 hover:bg-hover"
            >
              <X size={12} className="text-ink-3" />
              {props.closeLabel ?? "关闭面板"}
            </button>
          )}
        </div>
      </header>

      {props.showConnection !== false && <ConnectionBanner app={app} onRestart={store.restartDock} />}

      {session ? (
        <Transcript
          thread={thread}
          sessionKey={session.id}
          loading={app.loadingIds.has(session.id)}
          onRetry={running ? undefined : (text) => store.send(session.id, text)}
        />
      ) : (
        props.empty
      )}

      {/* 输入区和对话流同一列宽：输入框两侧 mx-4，所以比阅读列宽 32px。 */}
      <div className={`mx-auto flex w-full shrink-0 flex-col ${COMPOSER_WIDTH}`}>
        {id && app.queued.get(id) && (
          <Queued
            threadId={id}
            items={app.queued.get(id)!}
            canSteer={store.canSteer()}
            onSteer={(q) => store.steerQueued(id, q)}
            onCancel={(q) => store.unqueue(id, q)}
          />
        )}

        {id && pending && (
          <Pending
            key={pending.id}
            item={pending}
            subagentRole={
              pending.kind === "permission" && pending.agentId ? subagents.byId[pending.agentId]?.info.role : null
            }
            onPermission={(allow, always) => store.resolvePermission(id, allow, always)}
            onAnswer={(answers) => store.answer(id, answers)}
            onPlan={(decision) => store.resolvePlan(id, decision)}
            onElicit={(action) => store.resolveElicit(id, action)}
            onElicitOpen={() => store.openElicit(id)}
          />
        )}

        {id && (
          <GoalBar
            goal={env.goal}
            onPause={() => store.pauseGoal(id)}
            onResume={() => store.resumeGoal(id)}
            onClear={() => store.clearGoal(id)}
            onEdit={(content) => store.editGoal(id, content)}
          />
        )}

        {id && (
          <Composer
            threadId={id}
            settings={env}
            running={running}
            stopping={stopping}
            onSend={(text, images, mode) => store.send(id, text, images, mode)}
            onStop={() => store.cancel(id)}
            onSetModel={(m) => store.setModel(id, m)}
            onSetApproval={(mode) => store.setApproval(id, mode)}
            onSetReasoning={(effort) => store.setReasoning(id, effort)}
            onSetPlan={(on) => store.setPlan(id, on)}
            loadSlash={store.slashCommands}
            onSlash={(text) => store.runSlash(id, text)}
            onStartGoal={(text, images) => store.startGoal(id, text, images)}
            loadContext={() => store.contextBreakdown(id)}
            onCompact={() => void store.compact(id)}
          />
        )}
      </div>
      {session && deleting && (
        <DeleteConfirm
          onCancel={() => setDeleting(false)}
          onConfirm={async () => {
            setDeleting(false);
            if (await store.remove(session.id)) props.onDeleted();
          }}
        />
      )}
    </section>
  );
}

/** 运行中：脉冲点 + 用时（每秒刷新）。设计：SessionHeaderRunning。 */
/** 主代理停了、子代理还在跑（设计 may）：点了开子代理面板。 */
function SubagentsRunning({ count, onClick }: { count: number; onClick: () => void }) {
  return (
    <button type="button" onClick={onClick} className="enter-fade flex shrink-0 items-center gap-1.5 rounded-full px-1 py-0.5 hover:bg-hover">
      <span className="relative size-1.5">
        <span className="pulse-halo absolute inset-0 rounded-full bg-accent" />
        <span className="absolute inset-0 rounded-full bg-accent" />
      </span>
      <span className="text-[11px] font-medium text-accent">{count} 个子代理在跑</span>
    </button>
  );
}

function Running({ startedAt }: { startedAt: number | null }) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(t);
  }, []);
  return (
    <span className="enter-fade flex shrink-0 items-center gap-1.5 rounded-full bg-[#34c7591a] px-2 py-0.5">
      <span className="relative size-1.5">
        <span className="pulse-halo absolute inset-0 rounded-full bg-[#34c759]" />
        <span className="absolute inset-0 rounded-full bg-[#34c759]" />
      </span>
      <span className="text-[11px] font-medium text-[#248a3d]">
        运行中{startedAt ? ` · ${formatElapsed(now - startedAt)}` : ""}
      </span>
    </span>
  );
}
