// 对话流：按 dock-core 的轮次（`Turn`）渲染。形态参考用户给的截图（Codex 式）：
// 不标「用户 / 助手」；用户消息右侧灰气泡，隔久了中间插日期；一轮结束后过程折进
// 「用时 X ›」，只露最后的回复。一轮的结果、用时、错误原文都来自 Dock，不在这里推导。
// 工具卡：wonder-ui/tools/*。动效见 docs/MOTION.md。

import {
  createContext,
  memo,
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ComponentType,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import {
  AlertCircle,
  ArrowDown,
  Check,
  ChevronRight,
  CircleCheck,
  CircleSlash,
  Copy,
  CornerDownRight,
  Goal,
  Image as ImageIcon,
  LoaderCircle,
  Undo2,
} from "lucide-react";
import type { ThreadState, Turn, TurnItem } from "dock-core";
import { visibleReasoningText, turnHasActivity } from "dock-core";
import { prefs, usePrefs } from "../dock/prefs";
import { store } from "../dock/store";
import { stripImageChips } from "../dock/chips";
import { fetchUserImage } from "../dock/userImages";
import { formatElapsed, formatTokens } from "./chrome";
import { ImageViewer } from "./ImageViewer";
import { describeImage } from "./imageInput";
import { Markdown } from "./Markdown";
import { permissionParts } from "../dock/permission";
import { ToolCard, formatSeconds } from "./ToolCard";
import { toolCardFeatures, useToolCardRevision, useTurnFooters } from "../features/registry";
import { CanvasChip, CanvasIndexContext } from "./CanvasChip";
import { CompactionCard } from "./CompactionCard";
import { SideNoteCard } from "./SideNoteCard";
import { canvasEvent, canvasEvents, canvasIndex } from "./canvasDoc";
import { isSubagentCall, SubagentCard, SubagentGroup } from "./SubagentCard";
import { RewindConfirm } from "./SessionMenu";
import { PathLinkContext } from "./markdown/pathLink";
import { usePathLinker } from "./markdown/usePathLinker";
import { NAV_ATTR, navEntries, TurnNav } from "./TurnNav";

/** 阅读列宽：宽窗口里对话和输入框居中、行不拉太长（SessionPane 的输入区用同一个值）。 */
export const READING_WIDTH = "max-w-[780px]";
export const COMPOSER_WIDTH = "max-w-[812px]";
/** 你发的消息超过这么多行先折起来。 */
const BUBBLE_LINES = 10;

/** 离底部这么近才自动跟随；用户往上翻了就不打扰。 */
const STICK_PX = 80;
/** 「回到底部」/ 跳到某条消息的动画时长。 */
const GLIDE_MS = 280;
/** 跳到某条消息时，它的顶边离视口上沿留这么多。 */
const JUMP_GAP = 16;
/** 两轮用户消息隔这么久，中间插一行日期。 */
const DATE_GAP_MS = 30 * 60 * 1000;

type UserItem = Extract<TurnItem, { kind: "user" }>;

/** 当前对话流是哪个线程：气泡按它找本窗口发出去的图的预览。 */
const ThreadIdContext = createContext("");
/** 正在流的那段回复（一轮在跑、最后一条是正文）：末尾挂光标。 */
const LiveTextContext = createContext<string | null>(null);

/**
 * 嵌在别处用的对话流（侧边聊天面板）的开关；会话区用默认值。
 * - `compact`：窄面板里左右留白小一点。
 * - `rewind`：用户气泡能不能撤回（侧边聊天不行：撤回会按整份日志重建，把主线快照带出来）。
 * - `ReplyActions`：每轮结束后、回复下面的一行小操作（悬停出现）。
 * - `filesThreadId`：回复里的文件路径按哪个会话的目录打开（侧边聊天用主会话的；默认就是这个线程）。
 */
export interface TranscriptOptions {
  compact: boolean;
  rewind: boolean;
  ReplyActions: ComponentType<{ text: string }> | null;
  filesThreadId?: string;
}

export const TranscriptOptionsContext = createContext<TranscriptOptions>({
  compact: false,
  rewind: true,
  ReplyActions: null,
});

export function Transcript({
  thread,
  sessionKey,
  loading = false,
  onRetry,
}: {
  thread: ThreadState;
  sessionKey: string;
  /** 正在拉历史：还没内容时画骨架，不显示「新对话」。 */
  loading?: boolean;
  /** 最后一轮出错时的「重试」：把那一轮的用户消息再发一次。 */
  onRetry?: (text: string) => void;
}) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  /** 上一次滚动位置：分得清是往上翻（不再跟随）还是往下回（近底部重新跟随）。 */
  const lastTop = useRef(0);
  const follow = useRef(0);
  /** 「回到底部」的滚动动画；在滚的时候 onScroll 不判跟随（中途离底部远，会被当成用户往上翻）。 */
  const glide = useRef(0);
  const [atBottom, setAtBottom] = useState(true);
  const options = useContext(TranscriptOptionsContext);
  const linker = usePathLinker(options.filesThreadId ?? sessionKey);

  const unstick = () => {
    cancelAnimationFrame(glide.current);
    glide.current = 0;
    stick.current = false;
    setAtBottom(false);
  };

  // 换会话：直接到底，不做平滑滚动。
  useLayoutEffect(() => {
    stick.current = true;
    setAtBottom(true);
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [sessionKey]);

  // 新内容（含流式增量）：贴着底部时跟随。`thread` 每个增量都是新引用，一帧最多跟一次，
  // 直接定位不做平滑滚动：每个增量起一段平滑动画会和用户的滚动抢位置，往上翻不动。
  useEffect(() => {
    if (!stick.current || follow.current) return;
    follow.current = requestAnimationFrame(() => {
      follow.current = 0;
      const el = scrollRef.current;
      if (el && stick.current) el.scrollTop = el.scrollHeight;
    });
  }, [thread]);
  useEffect(
    () => () => {
      cancelAnimationFrame(follow.current);
      cancelAnimationFrame(glide.current);
    },
    [],
  );

  // 画布卡要知道每块画布的标题（edit / data 的参数里没有）和谁是最新一条。
  const canvases = useMemo(() => canvasIndex(canvasEvents(thread)), [thread]);
  const nav = useMemo(() => navEntries(thread.turns), [thread.turns]);

  if (thread.turns.length === 0 && loading) return <LoadingSkeleton key={sessionKey} />;
  if (thread.turns.length === 0) {
    return (
      <div key={sessionKey} className="enter-fade flex flex-1 flex-col items-center justify-center gap-2 text-ink-4">
        <div className="text-[15px] font-semibold text-ink-2">新对话</div>
        <div className="text-xs">在下面输入任务，Dock 会在这个项目的目录里开始工作。</div>
      </div>
    );
  }

  /**
   * 逐帧滚到 `target()`（每帧重算：内容还在长也能追上）。不用 scrollTo smooth：实测 WKWebView 里
   * 原生平滑滚动会被打断（页里有 MCP App 界面时点一下只挪几像素）。
   */
  const glideTo = (target: (el: HTMLDivElement) => number) => {
    const el = scrollRef.current;
    if (!el) return;
    cancelAnimationFrame(glide.current);
    const from = el.scrollTop;
    const t0 = performance.now();
    const step = (now: number) => {
      const k = Math.min(1, (now - t0) / GLIDE_MS);
      el.scrollTop = from + (target(el) - from) * (1 - (1 - k) ** 3);
      if (k < 1) {
        glide.current = requestAnimationFrame(step);
      } else {
        glide.current = 0;
        const gap = el.scrollHeight - el.scrollTop - el.clientHeight;
        stick.current = gap <= 1;
        setAtBottom(stick.current);
      }
    };
    glide.current = requestAnimationFrame(step);
  };

  const toBottom = () => {
    stick.current = true;
    glideTo((el) => el.scrollHeight - el.clientHeight);
  };

  // 消息导航：滚到那一轮的顶上，不再跟随底部。
  const toTurn = (turnId: string) => {
    const node = scrollRef.current?.querySelector<HTMLElement>(`[${NAV_ATTR}="${CSS.escape(turnId)}"]`);
    if (!node) return;
    stick.current = false;
    setAtBottom(false);
    glideTo((el) => {
      const top = el.scrollTop + node.getBoundingClientRect().top - el.getBoundingClientRect().top - JUMP_GAP;
      return Math.max(0, Math.min(top, el.scrollHeight - el.clientHeight));
    });
  };

  let prevAt: number | undefined;
  return (
    // 不写 flex-1：内容短时按自身高度收，输入框跟着浮起来（wonder-ui/chat/ComposerFloating）；
    // 内容长了再被 flex 压回可滚动的高度。
    <div className="relative flex min-h-0 shrink flex-col">
      <div
        ref={scrollRef}
        // 换会话重挂滚动区（进场动效、滚动位置）。和下面的 TurnNav 是兄弟：key 要带前缀，不能都用
        // sessionKey——兄弟 key 重复时 React 换会话删不掉旧节点，旧对话一层层叠在新对话上（残影）。
        key={`scroll:${sessionKey}`}
        // 往上滚一下就不再跟随（触控板一次只滚几像素，等离底部 STICK_PX 才放手会被下个增量拽回去）；
        // 到底了、或往下滚回离底部 STICK_PX 以内，重新跟随。
        onWheel={(e) => {
          if (e.deltaY < 0 && e.currentTarget.scrollTop > 0) unstick();
        }}
        onScroll={(e) => {
          const el = e.currentTarget;
          const gap = el.scrollHeight - el.scrollTop - el.clientHeight;
          const up = el.scrollTop < lastTop.current;
          lastTop.current = el.scrollTop;
          if (glide.current) return;
          stick.current = gap <= 1 || (!up && gap < STICK_PX);
          setAtBottom(stick.current);
        }}
        // 底边渐隐：内容滚到输入框边上时淡出，「回到底部」不压在字上（贴底时那一截是 pb-6 的留白）。
        className={`thin-scroll enter-fade flex min-h-0 shrink flex-col overflow-y-auto pb-6 pt-4 select-text [mask-image:linear-gradient(to_bottom,black_calc(100%-24px),transparent)] ${
          options.compact ? "px-4 [--chat-text:14px]" : "px-8"
        }`}
      >
        <div className={`mx-auto flex w-full flex-col gap-8 ${READING_WIDTH}`}>
          <ThreadIdContext.Provider value={sessionKey}>
            <PathLinkContext.Provider value={linker}>
              <CanvasIndexContext.Provider value={canvases}>
                {thread.turns.map((turn, i) => {
                  const at = firstUser(turn)?.at;
                  const showDate = !!at && (prevAt === undefined || at - prevAt > DATE_GAP_MS);
                  if (at) prevAt = at;
                  const last = i === thread.turns.length - 1;
                  return (
                    <TurnView
                      key={turn.id}
                      turn={turn}
                      showDate={showDate}
                      last={last}
                      onRetry={last ? onRetry : undefined}
                      compacting={last && thread.compaction?.status === "running"}
                    />
                  );
                })}
                {thread.compaction && thread.compaction.status !== "completed" && (
                  <CompactionCard progress={thread.compaction} onRetry={() => store.compact(sessionKey)} />
                )}
              </CanvasIndexContext.Provider>
            </PathLinkContext.Provider>
          </ThreadIdContext.Provider>
        </div>
      </div>
      <button
        type="button"
        title="回到底部"
        aria-label="回到底部"
        onClick={toBottom}
        tabIndex={atBottom ? -1 : 0}
        className={`glass-popover absolute bottom-3 left-1/2 flex size-9 -translate-x-1/2 items-center justify-center rounded-full border border-line-strong bg-white text-ink-2 shadow-[0px_4px_14px_rgba(0,0,0,0.1)] transition-[opacity,transform] duration-[var(--dur-base)] ease-[var(--ease-out)] hover:bg-hover ${
          atBottom ? "pointer-events-none translate-y-2 opacity-0" : "translate-y-0 opacity-100"
        }`}
      >
        <ArrowDown size={17} />
      </button>
      {!options.compact && (
        <TurnNav
          key={`nav:${sessionKey}`}
          entries={nav}
          scrollRef={scrollRef}
          onJump={toTurn}
          formatTime={(ms) => formatDate(ms, false)}
        />
      )}
    </div>
  );
}

function firstUser(turn: Turn): UserItem | undefined {
  return turn.items.find((i): i is UserItem => i.kind === "user");
}

const END_NOTE: Record<string, string> = { cancelled: " · 已停止", failed: " · 出错" };

/** 轮次没变就不重渲染（dock-core 保证没变的轮次是同一个引用）。 */
const TurnView = memo(function TurnView({
  turn,
  showDate,
  last,
  onRetry,
  compacting = false,
}: {
  turn: Turn;
  showDate: boolean;
  /** 最后一轮：还没有输出时撤回不用确认（像刚发出去就停了那种）。 */
  last: boolean;
  onRetry?: (text: string) => void;
  /** 这一轮在等压缩跑完：压缩卡在对话流末尾说进展，这里不再挂「正在思考」。 */
  compacting?: boolean;
}) {
  const [open, setOpen] = useState(false);
  // 功能认领产物（isArtifact）要等它的数据到：数据变了重算哪些不折进过程。
  useToolCardRevision();
  const done = turn.status !== "running";
  const users = turn.items.filter((i): i is UserItem => i.kind === "user");
  // 侧边聊天写进来的笔记不是这一轮的工作：单独排在回复后面，不参与「哪段是回复」的判断
  // （一轮结束后才写进来的笔记落在末尾，算进去会把回复整段折进过程里）。
  const all = turn.items.filter((i) => i.kind !== "user" && !isPending(i));
  const notes = all.filter((i) => i.kind === "sideNote");
  const work = all.filter((i) => i.kind !== "sideNote");
  // 过程 = 到最后一条非文字为止；之后的助手文字是这轮的回复，始终露出来。
  let cut = work.length;
  while (cut > 0 && work[cut - 1].kind === "text") cut--;
  // 画布卡是这一轮的产物、压缩标记是上下文的分界、子代理可能还在后台跑，都不随过程折起来：
  // 轮次结束后挪到折叠区外面。功能也能认领自己的产物（MCP App 的界面）。
  // 别的产物（工作流卡、预设草稿卡）跑着时留在调用发生的位置，结束后才挪出折叠区。
  // 只有声明了 keepMounted 的（MCP App 的界面，里面是 iframe）跑着的时候也在折叠区外面：
  // 一轮结束时从折叠区挪出来会换一棵 React 子树，iframe 卸了重挂——界面状态全丢，加载时
  // 发消息的 App 还会再发一遍。
  const isFeatureArtifact = (i: TurnItem) =>
    i.kind === "tool" && toolCardFeatures().some((f) => f.isArtifact?.(i.name, i.arguments));
  const isPinned = (i: TurnItem) =>
    i.kind === "tool" && toolCardFeatures().some((f) => f.keepMounted && f.isArtifact?.(i.name, i.arguments));
  const isCanvas = (i: TurnItem) =>
    (i.kind === "tool" && canvasEvent(i) !== null) || isFeatureArtifact(i) || i.kind === "compaction" || isSubagentCall(i);
  const process = work.slice(0, cut).filter((i) => !isPinned(i) && (!done || !isCanvas(i)));
  const artifacts = work.slice(0, cut).filter((i) => (done ? isCanvas(i) : isPinned(i)));
  // 空闲时手动压缩单独成一轮，里面只有压缩标记（或一张侧边聊天笔记）：不配「用时」行。
  const onlyCompaction = users.length === 0 && all.length > 0 && work.every((i) => i.kind === "compaction");
  const reply = work.slice(cut);
  const { ReplyActions } = useContext(TranscriptOptionsContext);
  const turnFooters = useTurnFooters();
  const threadId = useContext(ThreadIdContext);
  const replyText = reply
    .map((i) => (i.kind === "text" ? i.text : ""))
    .join("")
    .trim();
  const at = users[0]?.at;
  const retryText = users
    .map((u) => u.text)
    .join("\n\n")
    .trim();
  const tail = turn.items[turn.items.length - 1];
  const liveText = turn.status === "running" && tail?.kind === "text" ? tail.id : null;

  return (
    <LiveTextContext.Provider value={liveText}>
      <div className="flex shrink-0 flex-col gap-3" {...(users.length > 0 ? { [NAV_ATTR]: turn.id } : {})}>
        {showDate && at && (
          <div className="enter-fade pb-1 pt-3 text-center text-[13px] text-ink-4">{formatDate(at, true)}</div>
        )}
        {users.map((u) => (
          <UserBubble key={u.id} item={u} turnId={turn.id} confirmRewind={!last || work.length > 0} />
        ))}
        {done && turn.endedAt && !onlyCompaction && (
          <button
            type="button"
            disabled={process.length === 0}
            onClick={() => setOpen((o) => !o)}
            className="enter-fade group flex items-center gap-1 border-b border-line pb-2 pt-1 text-left text-[14px] text-ink-4 enabled:hover:text-ink-2"
          >
            用时 {formatElapsed(turn.endedAt - turn.startedAt)}
            {turn.steered ? <span> · 收到插话，接着往下</span> : END_NOTE[turn.status] && <span>{END_NOTE[turn.status]}</span>}
            {process.length > 0 && <ChevronRight size={15} className="rotate-open" data-open={open} />}
          </button>
        )}
        {process.length > 0 && (
          <div className="fold" data-open={!done || open} aria-hidden={done && !open}>
            <div>
              <div className="flex flex-col gap-2 pb-1">
                <Items items={process} />
              </div>
            </div>
          </div>
        )}
        <Items items={artifacts} />
        <Items items={reply} />
        {/* 功能登记的轮次脚注（这一轮改了哪些文件），见 features/registry.ts。 */}
        {done && turnFooters.map((Footer, i) => <Footer key={i} threadId={threadId} turnId={turn.id} />)}
        {done && ReplyActions && replyText && <ReplyActions text={replyText} />}
        <Items items={notes} />
        {/* 只有交互在等用户的那一轮不算「在跑」：模型停着等回答，不该显示成正在思考。 */}
        {turn.status === "running" && turnHasActivity(turn) && !compacting && waitingForModel(turn) && (
          <div className="enter-fade flex h-7 items-center text-[14px]">
            <span className="shimmer-text">正在思考</span>
          </div>
        )}
        {turn.status === "failed" && (
          // 设计：doxa 的 Turn Error Box。「重试」只给最后一轮，重发这一轮的用户消息。
          <div className="enter-up flex flex-col gap-2 rounded-[10px] border border-[#fecaca] bg-[#fef2f2] p-3.5 shadow-sm">
            <div className="flex items-center justify-between gap-2">
              <span className="flex items-center gap-2 text-xs font-semibold text-[#c0392b]">
                <AlertCircle size={16} className="shrink-0 text-[#ff3b30]" />
                这一轮出错了
              </span>
              {onRetry && retryText && (
                <button
                  type="button"
                  title="把这一轮的消息再发一次"
                  onClick={() => onRetry(retryText)}
                  className="motion-colors flex h-6 shrink-0 items-center rounded-md border border-[#fecaca] bg-white px-2.5 text-[11px] font-medium text-[#c0392b] shadow-sm hover:bg-[#fff5f5] active:scale-[0.97]"
                >
                  重试
                </button>
              )}
            </div>
            <div className="min-w-0 whitespace-pre-wrap break-words text-xs leading-relaxed text-[#c0392b]">
              {turn.error || "Dock 没给原因。"}
            </div>
          </div>
        )}
        {turn.status === "completed" && work.length === 0 && notes.length === 0 && (
          <div className="enter-fade text-[13px] text-ink-4">这一轮没有回复。</div>
        )}
      </div>
    </LiveTextContext.Provider>
  );
});

/**
 * 这一轮在跑、眼下却没东西在动：刚发出去还没回、工具跑完了等模型下一步、交互答完了。
 * 这时候在底部挂一行「正在思考」。正文在流、思考在流、工具在跑、在等你回答时不挂。
 */
function waitingForModel(turn: Turn): boolean {
  const last = turn.items[turn.items.length - 1];
  if (!last || last.kind === "user") return true;
  if (last.kind === "tool") return last.status !== "running";
  if (last.kind === "text" || last.kind === "reasoning") return false;
  return !isPending(last);
}

/** 还在等用户的交互不进时间线，由会话区底部的交互卡处理（见 Pending.tsx）。 */
function isPending(item: TurnItem): boolean {
  switch (item.kind) {
    case "permission":
    case "plan":
      return item.decision === null;
    case "question":
    case "elicit":
      return !item.resolved;
    default:
      return false;
  }
}

/** 网关给带图的消息正文前面加了「[Image #1] [Image #2] 」，气泡里有图了就不再显示这串。 */
function bubbleText(item: UserItem): string {
  if (item.attachments.length === 0) return item.text;
  return stripImageChips(item.text);
}

function UserBubble({ item, turnId, confirmRewind }: { item: UserItem; turnId: string; confirmRewind: boolean }) {
  const threadId = useContext(ThreadIdContext);
  const [copied, setCopied] = useState(false);
  // 撤回：确认框开着 / 正在撤 / 撤失败的原因（几秒后消失）。
  const [asking, setAsking] = useState(false);
  const [rewinding, setRewinding] = useState(false);
  const [rewindError, setRewindError] = useState<string | null>(null);
  // 只有 Dock 投影出来的轮次能撤（`t1`、`t2`…）；本地刚发、还没回执的不行。
  const { rewind: rewindable } = useContext(TranscriptOptionsContext);
  const canRewind = rewindable && /^t\d+$/.test(turnId);
  const rewind = async () => {
    setAsking(false);
    setRewinding(true);
    const error = await store.rewind(threadId, turnId);
    setRewinding(false);
    if (error) {
      setRewindError(error);
      window.setTimeout(() => setRewindError(null), 4000);
    }
  };
  const [viewing, setViewing] = useState<number | null>(null);
  const text = bubbleText(item);
  const urls = item.attachments.length ? store.imagePreviews(threadId, item.id) : null;
  // 本地预览没有的（刷新 / 重开 / 换机器），按内容摘要向 Dock 要像素。
  const [fetched, setFetched] = useState<Record<number, string>>({});
  useEffect(() => {
    if (!item.attachments.length) return;
    let alive = true;
    item.attachments.forEach((a, i) => {
      if (urls?.[i] || !a.digest) return;
      void fetchUserImage(threadId, a.digest).then((url) => {
        if (alive && url) setFetched((prev) => (prev[i] === url ? prev : { ...prev, [i]: url }));
      });
    });
    return () => {
      alive = false;
    };
  }, [threadId, item.id, item.attachments, urls]);
  // 设置 › 外观 › 显示消息时间戳：时间常驻；关着时悬停才出现。
  const showTime = usePrefs().timestamps;
  const copy = () => {
    void navigator.clipboard.writeText(text).then(() => {
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1200);
    });
  };
  return (
    <div className="enter-up group flex flex-col items-end gap-1.5 pl-[18%]">
      {/* 靠右的列里气泡按内容收缩：长串无断点（base64、URL）要能在任意处折行，否则气泡撑出左边界。 */}
      <div className="glass-bubble flex max-w-full flex-col gap-2.5 rounded-[20px] bg-[#f3f3f5] px-4 py-2.5">
        {item.attachments.length > 0 && (
          // 设计：wonder-ui/chat/Images（boor）。本窗口发的有预览；历史里的图 Dock 不回像素，只画尺寸。
          <div className="flex flex-wrap justify-end gap-2 pt-1">
            {item.attachments.map((a, i) => {
              const url = urls?.[i] ?? fetched[i];
              return url ? (
                <button
                  key={i}
                  type="button"
                  title={describeImage(a)}
                  onClick={() => setViewing(i)}
                  className="h-[86px] w-[140px] overflow-hidden rounded-md border border-line-strong bg-fill"
                >
                  <img src={url} alt="" className="size-full object-cover" />
                </button>
              ) : (
                <div
                  key={i}
                  title={`${describeImage(a)}（原图取不到了）`}
                  className="flex h-[86px] w-[140px] flex-col items-center justify-center gap-1 rounded-md border border-line-strong bg-[#e8e8ec] text-ink-4"
                >
                  <ImageIcon size={20} strokeWidth={1.5} />
                  <span className="text-[10.5px]">
                    {a.width}×{a.height}
                  </span>
                </div>
              );
            })}
          </div>
        )}
        {text && <BubbleText text={text} />}
      </div>
      {viewing !== null && (urls?.[viewing] ?? fetched[viewing]) && (
        <ImageViewer
          url={urls?.[viewing] ?? fetched[viewing]}
          caption={describeImage(item.attachments[viewing])}
          onClose={() => setViewing(null)}
        />
      )}
      <div className="flex h-6 items-center gap-1 pr-1 text-[12.5px] text-ink-4">
        {item.steered && (
          <span
            title="一轮进行中发的，在步骤边界并进了那一轮"
            className="mr-1 flex items-center gap-0.5 text-[11.5px] text-accent"
          >
            <CornerDownRight size={12} />
            插话
          </span>
        )}
        {item.at > 0 && (
          <span
            className={`mr-1 transition-opacity duration-[var(--dur-base)] ${showTime ? "" : "opacity-0 group-hover:opacity-100"}`}
          >
            {formatDate(item.at, false)}
          </span>
        )}
        <button
          type="button"
          title="复制"
          onClick={copy}
          className="motion-colors flex size-6 items-center justify-center rounded-md opacity-0 transition-opacity duration-[var(--dur-base)] hover:bg-hover hover:text-ink-2 group-hover:opacity-100"
        >
          {copied ? <Check size={14} className="pop-in text-[#34c759]" /> : <Copy size={14} />}
        </button>
        {text.trim() && (
          <button
            type="button"
            title="把这句话立成目标，一直做到完成"
            onClick={() => void store.setGoalFrom(threadId, text.trim())}
            className="motion-colors flex h-6 items-center gap-1 rounded-md px-1.5 opacity-0 transition-opacity duration-[var(--dur-base)] hover:bg-hover hover:text-ink-2 group-hover:opacity-100"
          >
            <Goal size={14} />
            设为目标
          </button>
        )}
        {canRewind && (
          <button
            type="button"
            title="撤回这条：它和之后的对话删掉，正文放回输入框改了再发"
            disabled={rewinding}
            onClick={() => (confirmRewind ? setAsking(true) : void rewind())}
            className={`motion-colors flex h-6 items-center gap-1 rounded-md px-1.5 transition-opacity duration-[var(--dur-base)] hover:bg-hover hover:text-ink-2 group-hover:opacity-100 ${
              rewinding ? "opacity-100" : "opacity-0"
            }`}
          >
            {rewinding ? <LoaderCircle size={14} className="animate-spin" /> : <Undo2 size={14} />}
            撤回
          </button>
        )}
      </div>
      {rewindError && <div className="enter-fade pr-1 text-[12px] text-[#ff3b30]">撤回失败：{rewindError}</div>}
      {/* 挂到 body 上：气泡带进场动画（transform），fixed 遮罩会被它圈在气泡里。 */}
      {asking &&
        createPortal(
          <RewindConfirm onConfirm={() => void rewind()} onCancel={() => setAsking(false)} />,
          document.body,
        )}
    </div>
  );
}

/** 气泡正文：超过 BUBBLE_LINES 行先折起，底下「展开全部」。 */
function BubbleText({ text }: { text: string }) {
  const ref = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(false);
  const [long, setLong] = useState(false);
  useLayoutEffect(() => {
    const el = ref.current;
    if (el && !open) setLong(el.scrollHeight > el.clientHeight + 1);
  }, [text, open]);
  return (
    <div className="flex flex-col items-start gap-1">
      <div
        ref={ref}
        style={open ? undefined : { WebkitLineClamp: BUBBLE_LINES }}
        className={`font-chat whitespace-pre-wrap wrap-anywhere text-[length:var(--chat-text,15px)] leading-[1.65] text-ink ${
          open ? "" : "line-clamp-(--clamp) overflow-hidden [display:-webkit-box] [-webkit-box-orient:vertical]"
        }`}
      >
        {text}
      </div>
      {(long || open) && (
        <button
          type="button"
          onClick={() => setOpen((o) => !o)}
          className="motion-colors text-[13px] text-accent hover:underline"
        >
          {open ? "收起" : "展开全部"}
        </button>
      )}
    </div>
  );
}

const WEEK = "日一二三四五六";

/** 分隔行 `8月20日周四 21:52`；气泡下 `8月20日 21:52`，今天只写时间。 */
function formatDate(ms: number, divider: boolean): string {
  const d = new Date(ms);
  const time = `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
  const now = new Date();
  const sameDay = d.toDateString() === now.toDateString();
  if (sameDay && !divider) return time;
  const year = d.getFullYear() === now.getFullYear() ? "" : `${d.getFullYear()}年`;
  const day = `${year}${d.getMonth() + 1}月${d.getDate()}日`;
  return divider ? `${sameDay ? "今天" : day}${sameDay ? "" : `周${WEEK[d.getDay()]}`} ${time}` : `${day} ${time}`;
}

const PERMISSION_TEXT: Record<string, string> = { approve: "已允许", deny: "已拒绝", cancelled: "未处理，本轮已结束" };
const PLAN_TEXT: Record<string, string> = {
  approve: "计划已批准",
  revise: "计划要求修改",
  quit: "计划已退出",
  cancelled: "计划未处理，本轮已结束",
};

/**
 * 模型的思考。想的时候：流光「正在思考 · Ns」，下面滚动露最新几行；想完：折成「思考了 Ns ›」，
 * 点开看全文。思考是给人看的旁白，字比正文小一号、灰一档。
 */
function Reasoning({ item }: { item: Extract<TurnItem, { kind: "reasoning" }> }) {
  const live = item.endedAt === null;
  // 设置 › 外观 › 默认展开思考过程。
  const [open, setOpen] = useState(() => prefs.get().expandReasoning);
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!live) return;
    const t = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(t);
  }, [live]);
  const elapsed = formatElapsed((item.endedAt ?? now) - item.startedAt);
  // 旧会话 / 跨 chunk 残片：item.text 里可能还带着 AQEA…</signature>，卡片只画摘要。
  const text = visibleReasoningText(item.text).trim();
  return (
    <div className="enter-fade flex shrink-0 flex-col gap-1.5">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className="group flex h-7 w-fit items-center gap-1 text-left text-[14px] text-ink-4 hover:text-ink-2"
      >
        {live ? (
          <>
            <span className="shimmer-text">正在思考</span>
            <span className="text-ink-5">· {elapsed}</span>
          </>
        ) : (
          <>思考了 {elapsed}</>
        )}
        <ChevronRight size={15} className="rotate-open" data-open={open} />
      </button>
      {open ? (
        <div className="enter-fade whitespace-pre-wrap break-words border-l-2 border-line pl-3 text-[13px] leading-[1.6] text-ink-4">
          {text}
        </div>
      ) : (
        live &&
        text && (
          // 只露最新三行，上沿渐隐；新字进来底部对齐，像在往上滚。
          <div className="flex max-h-[4.8em] flex-col justify-end overflow-hidden border-l-2 border-line pl-3 text-[13px] leading-[1.6] text-ink-4 [mask-image:linear-gradient(to_bottom,transparent,black_45%)]">
            <div className="whitespace-pre-wrap break-words">{text}</div>
          </div>
        )
      )}
    </div>
  );
}

/** 按顺序画条目；挨着的几个子代理调用（一步里并行派的）合成一张组卡。 */
function Items({ items }: { items: TurnItem[] }) {
  const threadId = useContext(ThreadIdContext);
  const out: ReactNode[] = [];
  for (let i = 0; i < items.length; i++) {
    const item = items[i];
    if (!isSubagentCall(item)) {
      out.push(<Item key={item.id} item={item} />);
      continue;
    }
    const run = [item];
    while (i + 1 < items.length && isSubagentCall(items[i + 1])) run.push(items[++i] as typeof item);
    out.push(
      run.length > 1 ? (
        <SubagentGroup key={item.id} items={run} threadId={threadId} />
      ) : (
        <SubagentCard key={item.id} item={item} threadId={threadId} />
      ),
    );
  }
  return <>{out}</>;
}

function Item({ item }: { item: TurnItem }) {
  const threadId = useContext(ThreadIdContext);
  const liveText = useContext(LiveTextContext);
  switch (item.kind) {
    case "user":
      return null;
    case "text":
      return (
        <div className="enter-up shrink-0">
          <Markdown text={item.text} live={item.id === liveText} />
        </div>
      );
    case "reasoning":
      return <Reasoning item={item} />;
    case "tool": {
      const ev = canvasEvent(item);
      if (ev) return <CanvasItem ev={ev} />;
      return (
        <ToolCard
          tool={item.name}
          args={item.arguments}
          output={item.output}
          status={item.status}
          startedAt={item.startedAt}
          endedAt={item.endedAt}
          initialOpen={prefs.get().expandTools}
          threadId={threadId}
          itemId={item.id}
          images={item.images}
        />
      );
    }
    case "permission":
      return (
        <Note icon={item.decision === "approve" ? CircleCheck : CircleSlash}>
          {PERMISSION_TEXT[item.decision ?? ""]}
          {item.always && "（以后都是）"} {item.toolName}
          {item.summary && <PermissionRecord summary={item.summary} />}
        </Note>
      );
    case "question":
      return <Note icon={CircleCheck}>已回答提问：{item.questions.map((q) => q.question).join(" / ")}</Note>;
    case "plan":
      return <Note icon={item.decision === "approve" ? CircleCheck : CircleSlash}>{PLAN_TEXT[item.decision ?? ""]}</Note>;
    case "elicit":
      return <Note icon={CircleCheck}>已处理 {item.server} 的请求</Note>;
    case "sideNote":
      return <SideNoteCard text={item.text} at={item.at} />;
    case "compaction":
      // 设计：wonder-ui/chat/FeedCompressedNote（his.grow）+ FeedCompaction 的完成格。
      // 前后占用、用时只有实时看着压完才有。
      return (
        <div className="enter-fade flex min-w-0 shrink-0 items-center gap-1.5 text-[11.5px] text-[#8e8e93]">
          <CircleCheck size={12} className="shrink-0" />
          <span className="min-w-0 truncate">
            {item.trigger === "auto" ? "上下文已自动压缩" : "上下文已压缩"}
            {item.beforeTokens !== null && item.afterTokens !== null && (
              <span className="tabular-nums">
                {" · "}
                {formatTokens(item.beforeTokens)} → {formatTokens(item.afterTokens)}
                {item.elapsedMs !== null && ` · ${formatSeconds(item.elapsedMs)}`}
              </span>
            )}
          </span>
        </div>
      );
  }
}

function CanvasItem({ ev }: { ev: NonNullable<ReturnType<typeof canvasEvent>> }) {
  const threadId = useContext(ThreadIdContext);
  return <CanvasChip ev={ev} threadId={threadId} />;
}

function Note({ icon: Icon, children }: { icon: typeof CircleCheck; children: ReactNode }) {
  return (
    <div className="enter-fade flex min-w-0 shrink-0 items-center gap-1.5 text-[12px] text-ink-4">
      <Icon size={12} className="shrink-0" />
      <span className="min-w-0 truncate">{children}</span>
    </div>
  );
}

/**
 * 载入历史时的占位：一个用户气泡 + 三行回复的骨架，中间浮一个「正在载入」胶囊。
 * 设计：wonder-ui/chat/TranscriptLoading（bilo）。晚 150ms 才淡入，读得快就不闪。
 */
function LoadingSkeleton() {
  const bone = "h-3 rounded-sm animate-pulse";
  return (
    <div className="enter-fade relative flex flex-1 flex-col [animation-delay:150ms]">
      <div className="mx-auto flex w-full max-w-[868px] flex-col gap-7 px-4 pt-6">
        <div className="flex justify-end">
          <div className="flex w-[320px] flex-col gap-2 rounded-[14px] rounded-br-sm bg-fill p-3.5">
            <div className={`${bone} w-full bg-line-strong`} />
            <div className={`${bone} w-[180px] bg-line-strong`} />
          </div>
        </div>
        <div className="flex flex-col gap-2.5">
          <div className={`${bone} w-[75%] bg-line`} />
          <div className={`${bone} w-[60%] bg-line`} />
          <div className={`${bone} w-[45%] bg-line`} />
        </div>
      </div>
      <div className="absolute inset-x-0 top-1/2 flex justify-center">
        <div className="flex h-10 items-center gap-2 rounded-full border border-line-strong bg-white px-5 shadow-md">
          <LoaderCircle size={14} className="animate-spin text-accent" />
          <span className="text-xs font-medium text-ink">正在载入会话记录…</span>
        </div>
      </div>
    </div>
  );
}

/** 时间线里一条权限记录的尾巴：有描述说描述（悬停看命令），没有就是摘要首行。 */
function PermissionRecord({ summary }: { summary: string }) {
  const { main, note } = permissionParts(summary);
  return note ? (
    <span className="text-ink-4" title={main}>
      {" "}
      · {note}
    </span>
  ) : (
    <span className="font-mono text-ink-5"> · {main}</span>
  );
}
