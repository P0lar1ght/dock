// 右侧子代理面板。设计：wonder-ui/pages/Subagents（zat 列表 / how 详情 / who 空 / lbs 过程没有保留 /
// gpl 已结束）。外壳同别的面板（PanelChrome 的拖手、分屏、新窗口、关闭）。
//
// 列表是这个会话派出的全部子代理；点一个看它自己的对话：第一条是主代理派的任务，
// 父级中途发来的话、它回报给父级的话单独成卡。底部能插一句、能打断 / 结束。
// 子代理只活在 Dock 的内存里：Dock 重启过就只剩卡片，这里如实说。

import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { ArrowLeft, ArrowUp, Bot, ChevronDown, CornerUpRight, Hand, X } from "lucide-react";
import {
  currentActivity,
  elapsedMs,
  pendingItems,
  taskPrompt,
  toolCallCount,
  type Subagent,
  type SubagentApproval,
  type Turn,
  type TurnItem,
} from "dock-core";
import { store, useEnv, useSubagents, useThread } from "../dock/store";
import { ContextArc } from "./ContextRing";
import { IconButton } from "./chrome";
import { Markdown } from "./Markdown";
import { Reasoning } from "./Reasoning";
import { PathLinkContext } from "./markdown/pathLink";
import { usePathLinker } from "./markdown/usePathLinker";
import { activityText, clock, RolePill, SETTLED_LABEL, usageOf, useNow } from "./SubagentCard";
import { ToolCard } from "./ToolCard";
import { COMPOSER_WIDTH, READING_WIDTH } from "./Transcript";
import { MenuButton } from "./Popover";
import { Pending } from "./Pending";
import { imeComposing } from "../dock/ime";

/** 离底部这么近才自动跟随。 */
const STICK_PX = 60;

export function SubagentPanel({
  sessionId,
  target,
  onClose,
  lead,
  controls,
}: {
  sessionId: string | null;
  /** 卡片点进来要看的子代理；`seq` 变了就切过去（同一个也重新定位）。 */
  target: { agentId: string | null; seq: number } | null;
  onClose: () => void;
  lead?: ReactNode;
  controls?: ReactNode;
}) {
  const state = useSubagents(sessionId);
  const [agentId, setAgentId] = useState<string | null>(target?.agentId ?? null);
  useEffect(() => {
    if (target) setAgentId(target.agentId);
  }, [target]);
  // 换了会话：回到列表。
  useEffect(() => setAgentId(target?.agentId ?? null), [sessionId]); // eslint-disable-line react-hooks/exhaustive-deps

  const sub = agentId ? state.byId[agentId] ?? null : null;
  const close = (
    <>
      <IconButton icon={X} label="关闭子代理面板" size={14} onClick={onClose} className="size-7 text-ink-4" />
    </>
  );
  // 子代理和主会话同一个目录：回复里的文件路径照样能点开。
  const linker = usePathLinker(sessionId);

  return (
    <aside className="glass-pane enter-fade relative flex min-h-0 min-w-0 flex-1 flex-col bg-[#fafafa]">
      <PathLinkContext.Provider value={linker}>
        {sub && sessionId ? (
          <Detail key={sub.info.agentId} sub={sub} threadId={sessionId} onBack={() => setAgentId(null)} lead={lead} controls={controls} close={close} />
        ) : (
          <List subs={state.order.map((id) => state.byId[id])} onPick={setAgentId} lead={lead} controls={controls} close={close} />
        )}
      </PathLinkContext.Provider>
    </aside>
  );
}

// ---- 列表 ----

const SECTIONS: { label: string; match: (s: Subagent) => boolean }[] = [
  { label: "运行中", match: (s) => s.info.status === "running" },
  { label: "空闲", match: (s) => s.info.status === "idle" },
  { label: "已结束", match: (s) => s.info.status !== "running" && s.info.status !== "idle" },
];

function List({
  subs,
  onPick,
  lead,
  controls,
  close,
}: {
  subs: Subagent[];
  onPick: (id: string) => void;
  lead?: ReactNode;
  controls?: ReactNode;
  close: ReactNode;
}) {
  const now = useNow(subs.some((s) => s.info.status === "running"));
  return (
    <>
      <header className="flex h-11 shrink-0 items-center gap-2 border-b border-[#ececf0] px-3" data-tauri-drag-region>
        {lead}
        <Bot size={16} className="shrink-0 text-[#636366]" />
        <span className="text-[13px] font-semibold text-ink">子代理</span>
        <div className="min-w-2 flex-1" data-tauri-drag-region />
        {controls}
        <span className="mx-0.5 h-4 w-px bg-line" />
        {close}
      </header>
      {subs.length === 0 ? (
        <div className="flex flex-1 items-center justify-center text-[13px] text-[#8e8e93]">这个会话还没有派出子代理</div>
      ) : (
        <div className="thin-scroll flex min-h-0 flex-1 flex-col gap-4 overflow-auto p-3">
          {SECTIONS.map(({ label, match }) => {
            const rows = subs.filter(match);
            if (rows.length === 0) return null;
            return (
              <section key={label} className="flex flex-col gap-1.5">
                <div className="px-1 text-[11px] font-medium text-[#8e8e93]">
                  {label} · {rows.length}
                </div>
                {rows.map((s) => (
                  <ListRow key={s.info.agentId} sub={s} now={now} onPick={onPick} />
                ))}
              </section>
            );
          })}
        </div>
      )}
    </>
  );
}

function ListRow({ sub, now, onPick }: { sub: Subagent; now: number; onPick: (id: string) => void }) {
  const { info } = sub;
  const running = info.status === "running";
  const usage = usageOf(info);
  const over = usage !== null && usage.percent >= usage.triggerPercent;
  const meta = running
    ? `${clock(elapsedMs(sub, now))} · ${toolCallCount(sub)}`
    : info.status === "failed"
      ? "失败"
      : info.status === "cancelled"
        ? "已取消"
        : `${SETTLED_LABEL[info.status]} · ${clock(info.durationMs)}`;
  return (
    <button
      type="button"
      onClick={() => onPick(info.agentId)}
      className="enter-up motion-colors flex flex-col gap-1 rounded-[10px] border border-[#e5e5ea] bg-white px-2.5 py-2 text-left hover:bg-hover"
    >
      <span className="flex w-full items-center justify-between gap-2">
        <span className="flex min-w-0 flex-1 items-center gap-1.5">
          <RolePill text={info.role} small />
          <span className="min-w-0 truncate text-xs text-ink">{info.description}</span>
        </span>
        <span className="flex shrink-0 items-center gap-1.5">
          {usage && (
            <span
              className={`text-[10px] tabular-nums ${over ? "text-[#ff9f0a]" : "text-[#8e8e93]"}`}
              title={`已用 ${usage.used} / ${usage.total} tokens（${usage.percent}%）`}
            >
              {usage.percent}%
            </span>
          )}
          <span className={`text-[10px] ${info.status === "failed" ? "text-[#c0392b]" : "text-[#8e8e93]"}`}>{meta}</span>
        </span>
      </span>
      {running && <span className="truncate pl-0.5 text-[11px] text-[#8e8e93]">{activityText(currentActivity(sub))}</span>}
    </button>
  );
}

// ---- 详情 ----

export const STATUS_TEXT: Record<string, { text: string; dot: string; color: string }> = {
  running: { text: "运行中", dot: "bg-[#34c759]", color: "text-[#248a3d]" },
  idle: { text: "已回报", dot: "bg-[#8e8e93]", color: "text-[#636366]" },
  completed: { text: "完成", dot: "bg-[#8e8e93]", color: "text-[#636366]" },
  failed: { text: "失败", dot: "bg-[#ff3b30]", color: "text-[#c0392b]" },
  cancelled: { text: "已取消", dot: "bg-[#c7c7cc]", color: "text-[#8e8e93]" },
};

function Detail({
  sub,
  threadId,
  onBack,
  lead,
  controls,
  close,
}: {
  sub: Subagent;
  threadId: string;
  onBack: () => void;
  lead?: ReactNode;
  controls?: ReactNode;
  close: ReactNode;
}) {
  const { info } = sub;
  const running = info.status === "running";
  const ended = info.status === "completed";
  const now = useNow(running);
  const [loading, setLoading] = useState(!sub.complete);
  const status = STATUS_TEXT[info.status];

  // 中途接入的子代理对话不全：打开时拉一次完整的。
  useEffect(() => {
    if (sub.complete) return;
    setLoading(true);
    void store.loadSubagentHistory(threadId, info.agentId).finally(() => setLoading(false));
    // 只在打开这个子代理时拉一次。
  }, [threadId, info.agentId]); // eslint-disable-line react-hooks/exhaustive-deps

  const empty = sub.thread.turns.length === 0;
  const usage = usageOf(info);

  return (
    <>
      <header className="flex shrink-0 flex-col gap-2 border-b border-[#ececf0] px-3 py-2.5" data-tauri-drag-region>
        <div className="flex items-center gap-2">
          {lead}
          <IconButton icon={ArrowLeft} label="回到子代理列表" size={14} onClick={onBack} className="size-7 text-[#636366]" />
          <RolePill text={info.role} />
          <span className="min-w-0 flex-1 truncate text-[13px] font-medium text-ink" title={info.description}>
            {info.description}
          </span>
          {usage && <ContextArc usage={usage} />}
          {controls}
          <span className="mx-0.5 h-4 w-px bg-line" />
          {close}
        </div>
        <div className="flex items-center justify-between gap-2 pl-9">
          <span className="flex items-center gap-1.5">
            <span className={`size-1.5 rounded-full ${status.dot} ${running ? "breathe" : ""}`} />
            <span className={`text-[11px] font-medium ${status.color}`}>{status.text}</span>
            <span className="font-mono text-[11px] text-[#8e8e93]">{clock(elapsedMs(sub, now))}</span>
          </span>
          {!ended && (
            <span className="flex items-center gap-1.5">
              {running && <SmallButton onClick={() => void store.interruptSubagent(threadId, info.agentId)}>打断</SmallButton>}
              <SmallButton onClick={() => void store.stopSubagent(threadId, info.agentId)}>结束</SmallButton>
            </span>
          )}
        </div>
      </header>

      {empty && !running && !loading ? (
        <div className="flex flex-1 flex-col gap-3 p-3">
          <div className="flex flex-col items-center justify-center gap-2 rounded-[10px] border border-[#e5e5ea] bg-[#f5f5f7] px-4 py-10 text-center text-[13px] text-[#8e8e93]">
            过程没有保留（这个子代理在对话落盘之前跑的）
          </div>
          {info.output && (
            <div className="flex flex-col gap-1 rounded-[10px] border border-[#e5e5ea] bg-white px-3 py-2.5">
              <span className="text-[11px] font-medium text-[#8e8e93]">最后回报摘要</span>
              <span className="whitespace-pre-wrap break-words text-[13px] leading-[1.55] text-[#3a3a3c]">{info.output}</span>
            </div>
          )}
        </div>
      ) : (
        <Conversation sub={sub} running={running} />
      )}

      <Composer threadId={threadId} agentId={info.agentId} ended={ended || info.persisted} />
    </>
  );
}

function SmallButton({ onClick, children }: { onClick: () => void; children: ReactNode }) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="motion-colors flex h-[26px] items-center rounded-[7px] border border-[#e5e5ea] bg-white px-2.5 text-[11px] font-medium text-[#3a3a3c] hover:bg-hover active:scale-[0.97]"
    >
      {children}
    </button>
  );
}

/** 子代理自己的对话，跟着底部走（用户往上翻了就不打扰）。`wide`：主区域整页用，居中阅读列。 */
export function Conversation({ sub, running, wide = false }: { sub: Subagent; running: boolean; wide?: boolean }) {
  const scroller = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  useLayoutEffect(() => {
    const el = scroller.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [sub.thread]);
  const onScroll = () => {
    const el = scroller.current;
    if (el) stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < STICK_PX;
  };
  const turns = sub.thread.turns;
  const last = turns[turns.length - 1];

  return (
    <div
      ref={scroller}
      onScroll={onScroll}
      className={`thin-scroll flex min-h-0 flex-1 flex-col overflow-auto select-text ${wide ? "px-8 py-5" : "p-3"}`}
    >
      <div className={`flex flex-col gap-3 ${wide ? `mx-auto w-full ${READING_WIDTH}` : ""}`}>
        {turns.map((turn, t) => (
          <TurnBlock key={turn.id} turn={turn} sub={sub} first={t === 0} />
        ))}
        {running && last && waiting(last) && (
          <div className="enter-fade flex h-6 items-center text-[13px]">
            <span className="shimmer-text">正在思考</span>
          </div>
        )}
      </div>
    </div>
  );
}

/** 这一轮在跑、眼下没东西在动（刚开始、工具跑完等模型）。 */
function waiting(turn: Turn): boolean {
  const last = turn.items[turn.items.length - 1];
  if (!last || last.kind === "user") return true;
  if (last.kind === "tool") return last.status !== "running";
  return false;
}

function TurnBlock({ turn, sub, first }: { turn: Turn; sub: Subagent; first: boolean }) {
  return (
    <>
      {turn.items.map((item, i) => (
        <Entry key={item.id} item={item} sub={sub} task={first && i === 0 && item.kind === "user"} />
      ))}
      {turn.status === "failed" && (
        <div className="enter-up whitespace-pre-wrap break-words rounded-[10px] border border-[#fecaca] bg-[#fef2f2] px-3 py-2 text-xs leading-relaxed text-[#c0392b]">
          {turn.error || "这一轮出错了，Dock 没给原因。"}
        </div>
      )}
    </>
  );
}

function Entry({ item, sub, task }: { item: TurnItem; sub: Subagent; task: boolean }) {
  switch (item.kind) {
    case "user":
      if (task) return <TaskCard text={taskPrompt(sub, item.text)} />;
      if (item.origin !== "parent" && store.sentByUser(sub.info.agentId, item.text)) {
        return <Bubble label="你说" tone="gray" text={item.text} />;
      }
      return <Bubble label="主代理说" tone="blue" text={item.text} />;
    case "text":
      return (
        <div className="enter-up shrink-0 text-[13px]">
          <Markdown text={item.text} />
        </div>
      );
    case "reasoning":
      return <Reasoning item={item} compact />;
    case "tool":
      if (item.name === "send_message" && typeof item.arguments.message === "string") {
        return <ReportCard text={item.arguments.message} />;
      }
      return (
        <ToolCard
          tool={item.name}
          args={item.arguments}
          output={item.output}
          status={item.status}
          startedAt={item.startedAt}
          endedAt={item.endedAt}
        />
      );
    default:
      // 交互：子代理的面板不画（交互走主会话）。
      return null;
  }
}

/** 主代理派的任务：长的默认折到 4 行。 */
function TaskCard({ text }: { text: string }) {
  const [open, setOpen] = useState(false);
  const long = text.split("\n").length > 4 || text.length > 180;
  return (
    <div className="enter-up flex shrink-0 flex-col gap-1.5 rounded-[10px] border border-[#e5e5ea] bg-[#f5f5f7] px-3 py-2.5">
      <span className="text-[11px] font-medium text-[#8e8e93]">来自主代理</span>
      <span className={`whitespace-pre-wrap break-words text-[13px] leading-[1.55] text-ink ${long && !open ? "line-clamp-4" : ""}`}>
        {text}
      </span>
      {long && (
        <button type="button" onClick={() => setOpen((o) => !o)} className="w-fit text-[11px] font-medium text-accent hover:underline">
          {open ? "收起" : "展开"}
        </button>
      )}
    </div>
  );
}

function Bubble({ label, tone, text }: { label: string; tone: "blue" | "gray"; text: string }) {
  const box = tone === "blue" ? "border-[#0a84ff1a] bg-[#0a84ff14]" : "border-[#e5e5ea] bg-white";
  return (
    <div className={`enter-up flex shrink-0 flex-col gap-1 rounded-[10px] border px-3 py-2 ${box}`}>
      <span className={`text-[11px] font-medium ${tone === "blue" ? "text-accent" : "text-[#8e8e93]"}`}>{label}</span>
      <span className="whitespace-pre-wrap break-words text-[13px] leading-[1.55] text-ink">{text}</span>
    </div>
  );
}

function ReportCard({ text }: { text: string }) {
  return (
    <div className="enter-up flex shrink-0 flex-col gap-1 rounded-[10px] border border-[#e5e5ea] bg-[#f5f5f7] px-3 py-2.5">
      <span className="flex items-center justify-between">
        <span className="text-[11px] font-medium text-[#8e8e93]">回报给主代理</span>
        <CornerUpRight size={14} className="text-[#8e8e93]" />
      </span>
      {/* 回报通常是 markdown（清单、代码路径）。 */}
      <div className="min-w-0 text-[13px]">
        <Markdown text={text} />
      </div>
    </div>
  );
}

const APPROVAL: { id: SubagentApproval; label: string; hint: string }[] = [
  { id: "inherit", label: "跟随会话", hint: "" },
  { id: "ask", label: "请求批准", hint: "逐次确认" },
  { id: "auto", label: "自动批准", hint: "不再询问" },
];
const chip = "flex h-7 items-center gap-1.5 rounded-full bg-[#f5f5f7] px-2.5 text-xs font-medium text-ink-3 hover:bg-[#ececf0]";
const MAX_HEIGHT = 160;

/**
 * 给子代理补一句。样子同主会话的输入框（Composer.tsx）：多行输入、底栏审批胶囊 + 发送键。
 * 审批只管这个子代理（`subagent/approval/set`）：默认跟随会话；单独设成自动批准后，
 * 只读子代理会改东西的命令直接拒、不弹。它自己的权限请求也在这里批（同主会话那张卡）。
 * `ended`：已经结束 / 只在盘上的，发出去 Dock 用同一个 id 接着聊（会话关着会先开页）。
 * `wide`：主区域整页用，居中、和主会话输入框同宽。
 */
export function Composer({
  threadId,
  agentId,
  ended = false,
  wide = false,
}: {
  threadId: string;
  agentId: string;
  ended?: boolean;
  wide?: boolean;
}) {
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [focused, setFocused] = useState(false);
  const ref = useRef<HTMLTextAreaElement>(null);
  const sub = useSubagents(threadId).byId[agentId];
  const page = useEnv(threadId).approval === "auto" ? "自动批准" : "请求批准";
  const mode = sub?.info.approval ?? "inherit";
  const current = APPROVAL.find((a) => a.id === mode) ?? APPROVAL[0];
  const readOnly = sub?.info.readOnly ?? false;
  const options = APPROVAL.map((a) =>
    a.id === "inherit"
      ? { ...a, hint: `现在是${page}` }
      : a.id === "auto" && readOnly
        ? { ...a, hint: "改东西的命令直接拒绝" }
        : a,
  );

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, MAX_HEIGHT)}px`;
  }, [text]);

  const send = async () => {
    const message = text.trim();
    if (!message || busy) return;
    setBusy(true);
    if (await store.sendToSubagent(threadId, agentId, message)) setText("");
    setBusy(false);
  };

  return (
    <div className={`flex shrink-0 flex-col ${wide ? `mx-auto w-full ${COMPOSER_WIDTH}` : ""}`}>
      <SubagentPending threadId={threadId} agentId={agentId} role={sub?.info.role ?? null} />
      <div
        className={`glass-card @container relative flex shrink-0 flex-col gap-3 rounded-[22px] border bg-white px-4 pb-3 pt-3.5 transition-[box-shadow,border-color,transform] duration-[var(--dur-base)] ease-[var(--ease-out)] ${
          wide ? "mx-4 mb-3" : "mx-3 mb-3"
        } ${
          focused
            ? "-translate-y-0.5 border-[#d1d1d6] shadow-[0px_10px_32px_rgba(0,0,0,0.13)]"
            : "border-line-strong shadow-[0px_8px_28px_rgba(0,0,0,0.1)]"
        }`}
      >
        <textarea
          ref={ref}
          rows={1}
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey && !imeComposing(e)) {
              e.preventDefault();
              void send();
            }
          }}
          onFocus={() => setFocused(true)}
          onBlur={() => setFocused(false)}
          placeholder={ended ? "接着跟它说…（从它的对话接着来）" : "给子代理补充一句…"}
          className="thin-scroll min-w-0 resize-none bg-transparent text-[15px] leading-snug text-ink outline-none placeholder:text-ink-5 select-text"
        />
        <div className="flex items-center justify-between gap-2">
          <MenuButton
            title="审批模式（只改这个子代理）"
            align="left"
            header="这个子代理的审批"
            options={options}
            value={current.id}
            onPick={(id) => void store.setSubagentApproval(threadId, agentId, id as SubagentApproval)}
            className={chip}
            trigger={
              <>
                <Hand size={14} strokeWidth={1.5} />
                <span className="hidden @[16rem]:inline">{current.label}</span>
                <ChevronDown size={11} className="text-ink-4" />
              </>
            }
          />
          <button
            type="button"
            aria-label="发送"
            title="发送 Enter"
            disabled={!text.trim() || busy}
            onClick={() => void send()}
            className="flex size-8 shrink-0 items-center justify-center rounded-full bg-ink text-white transition-[background-color,transform] duration-[var(--dur-fast)] active:scale-90 disabled:bg-ink-5"
          >
            <ArrowUp size={16} strokeWidth={2.25} />
          </button>
        </div>
      </div>
    </div>
  );
}

/**
 * 这个子代理在等批准的那条（会话的权限队列队首是它的才有）。主会话底部同样有这张卡，
 * 哪边点都行。单独拆出来：它订阅主会话的对话，流式增量只让这里重渲染。
 */
function SubagentPending({ threadId, agentId, role }: { threadId: string; agentId: string; role: string | null }) {
  const pending = pendingItems(useThread(threadId))[0];
  if (!pending || pending.kind !== "permission" || pending.agentId !== agentId) return null;
  return (
    <Pending
      key={pending.id}
      item={pending}
      subagentRole={role}
      onPermission={(allow, always) => store.resolvePermission(threadId, allow, always)}
      onAnswer={() => {}}
      onPlan={() => {}}
      onElicit={() => {}}
      onElicitOpen={() => {}}
    />
  );
}
