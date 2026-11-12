// 桌面（CUA）面板：agent 经 cua-driver 操作桌面的每一步，和它当时截到的画面。
// 设计：wonder-ui/pages/DesktopPanel。
//
// 数据全来自 dock.1：驱动状态看 `mcp/list` 的 `cua-driver` 行；步骤是对话流里
// `mcp_cua-driver__*` 的工具项。
//
// 画面默认是实时的（网关 `desktop/view/*`，能力 `desktopView`）：agent 正在操作的窗口，约 4 帧/秒，
// agent 还没碰过窗口时看最前面的窗口；最近的点击叠一个圈。点操作记录里某一步回看它当时的截图
// （`item/image` 按需取）。老网关没有实时画面，只有 agent 自己截的图，面板上要说清楚。
//
// 网关有 `desktopControl` 时：可以钉住一个窗口（空闲不再跟着前台截），并在画面上点击、滚动、打字。
// agent 正在跑时先点「接手」，避免和它抢同一个桌面。

import { useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent, type PointerEvent, type ReactNode, type WheelEvent } from "react";
import {
  AppWindow,
  Camera,
  Check,
  CircleAlert,
  Command,
  Copy,
  Download,
  EyeOff,
  Hand,
  ListTree,
  Loader2,
  Monitor,
  MousePointer2,
  Move,
  RefreshCw,
  ScrollText,
  Type,
  X,
  type LucideIcon,
} from "lucide-react";
import type { ThreadState } from "dock-core";
import { currentClient, dock, RpcError } from "../dock/client";
import { matches, SHORTCUTS } from "../dock/shortcuts";
import { store, useApp, useThread, type McpServerInfo } from "../dock/store";
import { usePrefs } from "../dock/prefs";
import { AgentCursor, type CursorEvent } from "./AgentCursor";
import { IconButton } from "./chrome";
import { calledTool } from "./toolCall";

const SERVER = "cua-driver";
const PREFIX = `mcp_${SERVER}__`;
const INSTALL = '/bin/bash -c "$(curl -fsSL https://cua.ai/driver/install.sh)"';
const EXAMPLE = "打开备忘录，新建一条：明天 10 点开会";

interface Step {
  id: string;
  icon: LucideIcon;
  verb: string;
  detail: string;
  at: number;
  status: "running" | "completed" | "failed" | "cancelled" | "denied";
  /** 结果里第一张图的 index（没有图是 null）。 */
  image: number | null;
  /** 截图时的窗口标题（从界面树第一行读）。 */
  window: string | null;
  /** 点击类操作的坐标（agent 那张截图的像素）。 */
  point: { x: number; y: number } | null;
  /** 参数里的窗口 id。 */
  windowId: number | null;
  /** `get_window_state` 截到的图多大（输出里的 `size=宽x高`）：换算点击坐标用。 */
  shotSize: { width: number; height: number } | null;
}

/** 实时画面一帧的元数据（像素走 <img>，不进状态）。 */
interface LiveFrame {
  width: number;
  height: number;
  source: "agent" | "front" | "pin";
  pid: number | null;
  windowId: number | null;
  label: string;
}

interface DeskWin {
  pid: number;
  windowId: number;
  app: string | null;
  title: string | null;
}

type Driver = { kind: "loading" } | { kind: "missing" } | { kind: "server"; info: McpServerInfo };

export function DesktopPanel({
  sessionId,
  onClose,
  lead,
  controls,
}: {
  sessionId: string | null;
  onClose: () => void;
  /** 头栏最左边（分屏 / 独立窗口时的拖手、红绿灯占位）。 */
  lead?: ReactNode;
  /** 关闭按钮左边（分屏菜单、在新窗口打开）。 */
  controls?: ReactNode;
}) {
  const app = useApp();
  const thread = useThread(sessionId);
  const running = !!sessionId && app.running.has(sessionId);
  const connected = app.connection === "open";
  const [driver, setDriver] = useState<Driver>({ kind: "loading" });
  const [busy, setBusy] = useState(false);
  const [picked, setPicked] = useState<string | null>(null);

  const steps = useMemo(() => desktopSteps(thread), [thread]);
  const shots = steps.filter((s) => s.image !== null);
  // 有实时画面时默认看实时，点某一步回看；没有就看最新一张截图。有新截图都回到默认。
  const latest = shots[shots.length - 1] ?? null;
  const shown = shots.find((s) => s.id === picked) ?? latest;
  const latestId = latest?.id;
  useEffect(() => setPicked(null), [latestId, sessionId]);

  const probe = useCallback(async () => {
    try {
      const servers = await store.mcpList();
      const info = servers.find((s) => s.name === SERVER);
      setDriver(info ? { kind: "server", info } : { kind: "missing" });
    } catch {
      setDriver({ kind: "missing" });
    }
  }, []);
  useEffect(() => {
    if (connected) void probe();
  }, [connected, probe]);

  const retry = async (reconnect: boolean) => {
    setBusy(true);
    try {
      if (reconnect) await store.mcpReconnect(SERVER);
      else await store.mcpReload();
    } catch (e) {
      store.reportError(e);
    }
    await probe();
    setBusy(false);
  };

  const ok = driver.kind === "server" && driver.info.status === "connected";
  const last = steps[steps.length - 1];
  const acting = running && last?.status === "running" ? last : null;
  /** 网关能推实时画面。 */
  const live = ok && connected && !!dock.capabilities.desktopView;
  const replaying = live && picked !== null && !!shots.find((s) => s.id === picked);

  return (
    <aside className="glass-pane enter-fade relative flex min-h-0 min-w-0 flex-1 flex-col bg-white">
      <header className="flex h-11 shrink-0 items-center gap-2.5 border-b border-line-soft px-4" data-tauri-drag-region>
        {lead}
        <Monitor size={16} className="shrink-0 text-ink-3" />
        <span className="shrink-0 text-[13px] font-semibold text-ink">桌面</span>
        <StatusChip driver={driver} running={running && steps.length > 0} />
        {ok && <span className="truncate text-xs text-ink-4">cua-driver 已连接</span>}
        <div className="flex-1" data-tauri-drag-region />
        {controls && (
          <>
            {controls}
            <div className="h-4 w-px shrink-0 bg-line-strong" />
          </>
        )}
        <IconButton icon={X} label="关闭桌面面板" size={14} onClick={onClose} className="size-7 text-ink-4" />
      </header>

      {!sessionId ? (
        <Empty icon={Monitor} title="先选一个会话" text="桌面面板列出当前会话里 agent 操作桌面的每一步。" />
      ) : driver.kind === "missing" && steps.length === 0 ? (
        <Empty
          icon={Download}
          title="没找到 cua-driver"
          text="Dock 所在的机器上还没装桌面驱动。装好后 Dock 会自动接上，不用改配置。"
        >
          <button
            type="button"
            title="复制安装命令"
            onClick={() => void navigator.clipboard.writeText(INSTALL)}
            className="mt-1 flex w-[min(420px,100%)] items-center gap-2 rounded-lg bg-ink px-3 py-2.5 text-left"
          >
            <span className="min-w-0 flex-1 truncate font-mono text-xs text-white/90">{INSTALL}</span>
            <Copy size={14} className="shrink-0 text-white/70" />
          </button>
          <span className="text-center text-xs text-ink-4">macOS 还要在系统设置里给它「辅助功能」和「屏幕录制」权限</span>
          <DarkButton icon={RefreshCw} label="重新检测" busy={busy} onClick={() => void retry(false)} />
        </Empty>
      ) : driver.kind === "server" && driver.info.status !== "connected" && steps.length === 0 ? (
        <Empty
          icon={CircleAlert}
          title={driver.info.status === "disabled" ? "cua-driver 已停用" : "cua-driver 没连上"}
          text={
            driver.info.status === "disabled"
              ? "在设置 › MCP 里把它重新打开。"
              : "驱动装了，但 MCP 连接失败。下面是 Dock 看到的原因。"
          }
        >
          {driver.info.detail && (
            <div className="w-[min(420px,100%)] break-all rounded-lg border border-[#ffd6d2] bg-[#fff6f5] px-3 py-2.5 font-mono text-xs leading-[1.5] text-[#8a2a22]">
              {driver.info.detail}
            </div>
          )}
          {driver.info.status !== "disabled" && (
            <DarkButton icon={RefreshCw} label="重新连接" busy={busy} onClick={() => void retry(true)} />
          )}
        </Empty>
      ) : steps.length === 0 && !live ? (
        <Empty
          icon={Monitor}
          title="这个会话还没操作过桌面"
          text="让 agent 打开应用、点按钮、打字时，这里会列出每一步和它截到的画面。"
        >
          <button
            type="button"
            onClick={() => store.fillComposer(sessionId, EXAMPLE)}
            className="motion-colors mt-1 flex items-center gap-2 rounded-[10px] border border-line-strong bg-white px-3.5 py-2.5 hover:bg-hover"
          >
            <span className="text-[13px] text-ink-2">「{EXAMPLE}」</span>
            <span className="shrink-0 text-[13px] font-medium text-[#0a6fd6]">填入输入框</span>
          </button>
        </Empty>
      ) : (
        <>
          {live && !replaying ? (
            <LiveView sessionId={sessionId} steps={steps} acting={acting} running={running} />
          ) : (
            <Viewer
              sessionId={sessionId}
              step={shown}
              acting={acting}
              onBackToLive={live ? () => setPicked(null) : undefined}
            />
          )}
          {steps.length > 0 ? (
            <Timeline steps={steps} shownId={live && !replaying ? null : (shown?.id ?? null)} onPick={setPicked} live={live} />
          ) : (
            <div className="flex h-[30px] shrink-0 items-center gap-2 border-t border-line-soft px-4">
              <span className="min-w-0 flex-1 truncate text-[11px] text-ink-4">
                {dock.capabilities.desktopControl
                  ? "还没操作过桌面 · 可以在上面钉住一个窗口，或等 agent 动手后跟上"
                  : "还没操作过桌面 · 现在看的是最前面的窗口，agent 一动手就跟到它操作的窗口"}
              </span>
              <button
                type="button"
                onClick={() => store.fillComposer(sessionId, EXAMPLE)}
                className="shrink-0 text-[11px] font-medium text-[#0a6fd6] hover:underline"
              >
                试试「{EXAMPLE}」
              </button>
            </div>
          )}
        </>
      )}
    </aside>
  );
}

function StatusChip({ driver, running }: { driver: Driver; running: boolean }) {
  let chip: { text: string; bg: string; fg: string; dot: string } | null = null;
  if (driver.kind === "missing") chip = { text: "未安装", bg: "bg-fill", fg: "text-ink-3", dot: "bg-ink-5" };
  else if (driver.kind === "server" && driver.info.status === "disabled")
    chip = { text: "已停用", bg: "bg-fill", fg: "text-ink-3", dot: "bg-ink-5" };
  else if (driver.kind === "server" && driver.info.status !== "connected")
    chip = { text: "连不上", bg: "bg-[#fff0ef]", fg: "text-[#c9342a]", dot: "bg-[#ff453a]" };
  else if (driver.kind === "server" && running)
    chip = { text: "Agent 操作中", bg: "bg-[#eaf3ff]", fg: "text-[#0a6fd6]", dot: "bg-accent" };
  else if (driver.kind === "server") chip = { text: "空闲", bg: "bg-fill", fg: "text-ink-3", dot: "bg-[#34c759]" };
  if (!chip) return null;
  return (
    <span className={`motion-colors flex h-6 shrink-0 items-center gap-1.5 rounded-full px-2.5 ${chip.bg}`}>
      <span className={`size-1.5 rounded-full ${chip.dot}`} />
      <span className={`text-xs font-medium ${chip.fg}`}>{chip.text}</span>
    </span>
  );
}

/** 画面：选中那一步（默认最新）的截图。像素按需取，取过的缓存着。 */
function Viewer({
  sessionId,
  step,
  acting,
  onBackToLive,
}: {
  sessionId: string;
  step: Step | null;
  acting: Step | null;
  /** 有实时画面时：回看结束、回到实时。 */
  onBackToLive?: () => void;
}) {
  const cache = useRef(new Map<string, string>());
  const [src, setSrc] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  const key = step && step.image !== null ? `${sessionId}#${step.id}#${step.image}` : null;

  useEffect(() => {
    setFailed(false);
    if (!key || !step || step.image === null) {
      setSrc(null);
      return;
    }
    const hit = cache.current.get(key);
    if (hit) {
      setSrc(hit);
      return;
    }
    let alive = true;
    dock
      .call<{ mimeType: string; data: string }>("item/image", { threadId: sessionId, itemId: step.id, index: step.image })
      .then(
        (r) => {
          const url = `data:${r.mimeType};base64,${r.data}`;
          cache.current.set(key, url);
          if (alive) setSrc(url);
        },
        () => alive && setFailed(true),
      );
    return () => {
      alive = false;
    };
  }, [key, sessionId, step]);

  return (
    <div className="relative flex min-h-0 flex-1 flex-col gap-2.5 bg-[#f2f2f4] px-4 pb-4 pt-3">
      {step && (
        <div className="flex items-center gap-2">
          <span className="text-xs font-medium text-ink-2">{onBackToLive ? "回看 · Agent 当时看到的画面" : "Agent 看到的画面"}</span>
          <span className="min-w-0 flex-1 truncate text-xs text-ink-4">
            {[step.window, `${ago(step.at)}截的`].filter(Boolean).join(" · ")}
          </span>
          {onBackToLive && (
            <button
              type="button"
              onClick={onBackToLive}
              className="motion-colors flex h-6 shrink-0 items-center gap-1.5 rounded-full bg-ink px-2.5 hover:bg-ink-2"
            >
              <span className="size-1.5 rounded-full bg-[#34c759]" />
              <span className="text-[11px] font-medium text-white">回到实时画面</span>
            </button>
          )}
        </div>
      )}
      <div className="relative flex min-h-0 flex-1 items-center justify-center overflow-hidden rounded-[10px] border border-line-strong bg-[#e4e6ea]">
        {src ? (
          <img key={src} src={src} alt="" className="enter-fade max-h-full max-w-full object-contain" />
        ) : step && !failed ? (
          <Loader2 size={18} className="animate-spin text-ink-4" />
        ) : acting && !step ? (
          // 还没有任何截图、agent 正在做第一步（可能在等你批准）：别说成「没截图」。
          <div className="flex flex-col items-center gap-2 px-6 text-center">
            <Loader2 size={18} className="animate-spin text-ink-4" />
            <span className="text-xs text-ink-4">等 agent 截第一张图</span>
          </div>
        ) : (
          <div className="flex flex-col items-center gap-2 px-6 text-center">
            <EyeOff size={20} className="text-ink-4" />
            <span className="text-[13px] font-medium text-ink-2">{failed ? "取不到这张截图" : "这几步没有截图"}</span>
            <span className="text-xs text-ink-4">
              {failed ? "可能是旧会话没存下来。" : "agent 只读了界面结构（省图片额度），没拍画面"}
            </span>
          </div>
        )}
        {acting && (
          <div className="enter-up pointer-events-none absolute bottom-4 left-1/2 flex max-w-[90%] -translate-x-1/2 items-center gap-2 rounded-full bg-[#1c1c1ee6] py-1.5 pl-3 pr-3.5 shadow-[0px_6px_20px_rgba(0,0,0,0.18)]">
            <span className="relative size-2 shrink-0">
              <span className="pulse-halo absolute inset-0 rounded-full bg-accent" />
              <span className="absolute inset-0 rounded-full bg-accent" />
            </span>
            <span className="shrink-0 text-xs font-medium text-white">Agent 正在操作</span>
            <span className="min-w-0 truncate text-xs text-white/70">
              {acting.verb} {acting.detail}
            </span>
          </div>
        )}
      </div>
    </div>
  );
}

function Timeline({
  steps,
  shownId,
  onPick,
  live,
}: {
  steps: Step[];
  shownId: string | null;
  onPick: (id: string) => void;
  live: boolean;
}) {
  const list = useRef<HTMLDivElement>(null);
  // 新步骤进来滚到底。
  useEffect(() => {
    const el = list.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [steps.length]);
  return (
    <div className="flex max-h-[42%] shrink-0 flex-col border-t border-line-soft bg-white">
      <div className="flex h-9 shrink-0 items-center gap-2 px-4">
        <span className="text-xs font-semibold text-ink">操作记录</span>
        <span className="truncate text-xs text-ink-4">{steps.length} 步 · 点带缩略图的步骤看当时的画面</span>
      </div>
      <div ref={list} className="thin-scroll flex min-h-0 flex-col gap-px overflow-y-auto px-2 pb-2">
        {steps.map((s) => {
          const Icon = s.icon;
          const has = s.image !== null;
          return (
            <button
              key={s.id}
              type="button"
              disabled={!has}
              onClick={() => onPick(s.id)}
              className={`motion-colors flex h-9 shrink-0 items-center gap-2.5 rounded-lg px-2.5 text-left ${
                s.id === shownId ? "bg-fill" : has ? "hover:bg-hover" : ""
              }`}
            >
              <Icon size={14} className="shrink-0 text-ink-3" />
              <span className="w-14 shrink-0 text-[13px] font-medium text-ink">{s.verb}</span>
              <span className="min-w-0 flex-1 truncate text-[13px] text-ink-2">{s.detail}</span>
              {has && <Camera size={13} className="shrink-0 text-ink-4" />}
              <span className="w-16 shrink-0 text-right font-mono text-[11px] text-ink-4">{clock(s.at)}</span>
              <StepStatus status={s.status} />
            </button>
          );
        })}
      </div>
      <div className="flex h-[30px] shrink-0 items-center border-t border-line-soft px-4">
        <span className="text-[11px] text-ink-4">
          {live
            ? dock.capabilities.desktopControl
              ? "点画面操作。打字写在画面下面的输入条里回车送出。钉住后空闲不跟着前台刷新；agent 在跑时先接手"
              : "只读 · 上面是实时画面，点带缩略图的步骤回看当时的截图"
            : "只读 · 画面是 agent 自己截的图，不是实时画面"}
        </span>
      </div>
    </div>
  );
}

function StepStatus({ status }: { status: Step["status"] }) {
  if (status === "running") return <Loader2 size={13} className="shrink-0 animate-spin text-accent" />;
  if (status === "completed") return <Check size={14} className="shrink-0 text-[#34c759]" />;
  return (
    <span className="shrink-0 text-[11px] text-[#c9342a]">
      {status === "denied" ? "已拒绝" : status === "cancelled" ? "已中断" : "失败"}
    </span>
  );
}

function Empty({
  icon: Icon,
  title,
  text,
  children,
}: {
  icon: LucideIcon;
  title: string;
  text: string;
  children?: React.ReactNode;
}) {
  return (
    <div className="enter-fade flex min-h-0 flex-1 flex-col items-center justify-center gap-3 bg-pane px-10">
      <div className="flex size-12 items-center justify-center rounded-full border border-line-strong bg-white">
        <Icon size={20} className="text-ink-3" />
      </div>
      <span className="text-[15px] font-semibold text-ink">{title}</span>
      <span className="max-w-[380px] text-center text-[13px] leading-[1.55] text-ink-3">{text}</span>
      {children}
    </div>
  );
}

function DarkButton({ icon: Icon, label, busy, onClick }: { icon: LucideIcon; label: string; busy: boolean; onClick: () => void }) {
  return (
    <button
      type="button"
      disabled={busy}
      onClick={onClick}
      className="motion-colors flex h-8 items-center gap-1.5 rounded-lg bg-ink px-3.5 hover:bg-ink-2 disabled:opacity-60"
    >
      <Icon size={14} className={`text-white ${busy ? "animate-spin" : ""}`} />
      <span className="text-[13px] font-medium text-white">{label}</span>
    </button>
  );
}

/** 对话流里 agent 经 cua-driver 做的每一步（全部轮次，按时间）。 */
function desktopSteps(thread: ThreadState): Step[] {
  const out: Step[] = [];
  for (const turn of thread.turns) {
    for (const item of turn.items) {
      if (item.kind !== "tool") continue;
      const call = calledTool(item);
      if (!call.name.startsWith(PREFIX)) continue;
      const tool = call.name.slice(PREFIX.length);
      const { icon, verb, detail } = describe(tool, call.args, item.images.length > 0);
      const window = /AXWindow "([^"]+)"/.exec(item.output)?.[1] ?? null;
      const size = /size=(\d+)x(\d+)/.exec(item.output);
      out.push({
        id: item.id,
        icon,
        verb,
        // 截图 / 读界面那一步：有窗口标题就显示标题，比「窗口 7」好认。
        detail: tool === "get_window_state" && window ? window : detail,
        at: item.startedAt,
        status: item.status,
        image: item.images[0]?.index ?? null,
        window,
        point: num(call.args.x) !== null && num(call.args.y) !== null ? { x: num(call.args.x)!, y: num(call.args.y)! } : null,
        windowId: num(call.args.window_id),
        shotSize: size ? { width: Number(size[1]), height: Number(size[2]) } : null,
      });
    }
  }
  return out;
}

function describe(tool: string, a: Record<string, unknown>, shot: boolean): { icon: LucideIcon; verb: string; detail: string } {
  const s = (k: string) => (typeof a[k] === "string" ? (a[k] as string) : typeof a[k] === "number" ? String(a[k]) : "");
  const at = a.x !== undefined && a.y !== undefined ? `(${s("x")}, ${s("y")})` : a.element_token ? "界面元素" : "";
  switch (tool) {
    case "launch_app":
      return { icon: AppWindow, verb: "打开应用", detail: s("name") || s("bundle_id") || s("launch_path") };
    case "get_window_state":
      return { icon: shot ? Camera : ListTree, verb: shot ? "截图" : "读界面", detail: s("window_id") ? `窗口 ${s("window_id")}` : "" };
    case "get_desktop_state":
      return { icon: Camera, verb: "截桌面", detail: "" };
    case "click":
    case "double_click":
    case "right_click":
      return { icon: MousePointer2, verb: tool === "double_click" ? "双击" : tool === "right_click" ? "右键" : "点击", detail: at };
    case "type_text":
      return { icon: Type, verb: "输入", detail: s("text") };
    case "set_value":
      return { icon: Type, verb: "填值", detail: s("value") };
    case "press_key":
      return { icon: Command, verb: "按键", detail: keys([...list(a.modifiers), s("key")]) };
    case "hotkey":
      return { icon: Command, verb: "按键", detail: keys(list(a.keys)) };
    case "scroll":
      return { icon: ScrollText, verb: "滚动", detail: [s("direction"), s("amount")].filter(Boolean).join(" ") };
    case "drag":
      return { icon: Move, verb: "拖动", detail: "" };
    case "invoke_menu":
      return { icon: ListTree, verb: "菜单", detail: list(a.path).join(" › ") };
    case "bring_to_front":
      return { icon: AppWindow, verb: "切到前台", detail: s("name") || s("pid") };
    case "list_apps":
      return { icon: ListTree, verb: "列应用", detail: "" };
    case "list_windows":
      return { icon: ListTree, verb: "列窗口", detail: "" };
    default:
      return { icon: Hand, verb: "操作", detail: tool };
  }
}

/** `["cmd","shift","n"]` → `⌘⇧N`。 */
function keys(names: string[]): string {
  const sym: Record<string, string> = { cmd: "⌘", command: "⌘", meta: "⌘", shift: "⇧", option: "⌥", alt: "⌥", ctrl: "⌃", control: "⌃" };
  return names
    .filter(Boolean)
    .map((k) => sym[k.toLowerCase()] ?? (k.length === 1 ? k.toUpperCase() : k))
    .join("");
}

function list(v: unknown): string[] {
  return Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : [];
}

function clock(ms: number): string {
  return new Date(ms).toLocaleTimeString("zh-CN", { hour12: false });
}

function ago(ms: number): string {
  const s = Math.max(0, (Date.now() - ms) / 1000);
  if (s < 60) return `${Math.floor(s)} 秒前`;
  if (s < 3600) return `${Math.floor(s / 60)} 分钟前`;
  return clock(ms);
}

function num(v: unknown): number | null {
  const n = typeof v === "number" ? v : typeof v === "string" && v.trim() ? Number(v) : NaN;
  return Number.isFinite(n) ? n : null;
}

/** 画面在 <img> 里实际占的矩形（CSS 像素，相对 img 的父元素）。 */
interface ContentRect {
  left: number;
  top: number;
  width: number;
  height: number;
}

/** 画面状态：网关的 `desktop/view/status`，加上连接中 / 断开。 */
type ViewState =
  | { kind: "connecting" }
  | { kind: "live" }
  | { kind: "no_window"; message: string }
  | { kind: "capture_failed"; message: string }
  | { kind: "error"; message: string };

/** 标题行圆点和文字的颜色（设计稿 rfc 的三种状态）。 */
const STATE_TONE: Record<ViewState["kind"], { dot: string; text: string }> = {
  connecting: { dot: "bg-ink-5", text: "text-ink-4" },
  live: { dot: "bg-[#34c759]", text: "text-ink-4" },
  no_window: { dot: "bg-[#aeaeb2]", text: "text-ink-4" },
  capture_failed: { dot: "bg-[#ff9f0a]", text: "text-[#b25e00]" },
  error: { dot: "bg-[#ff3b30]", text: "text-[#d70015]" },
};

/**
 * 实时画面：`desktop/view/open` 推来的帧直接写进 <img>（不进 React 状态）。断了隔一会儿重开。
 * agent 的光标叠在画面上（AgentCursor）：位置来自网关的 `desktop/view/cursor`；agent 正在做的
 * 像素点击先按参数滑过去，做完后网关给的真实位置再校准、播点击效果。
 */
function LiveView({
  sessionId,
  steps,
  acting,
  running,
}: {
  sessionId: string;
  steps: Step[];
  acting: Step | null;
  running: boolean;
}) {
  const img = useRef<HTMLImageElement>(null);
  const box = useRef<HTMLDivElement>(null);
  const [frame, setFrame] = useState<LiveFrame | null>(null);
  const [hasFrame, setHasFrame] = useState(false);
  // 设置 › 电脑操控 › 在画面上显示 agent 光标。
  const showCursor = usePrefs().agentCursor;
  const [state, setState] = useState<ViewState>({ kind: "connecting" });
  const [attempt, setAttempt] = useState(0);
  const [cursor, setCursor] = useState<CursorEvent | null>(null);
  const seq = useRef(0);
  const canControl = !!dock.capabilities.desktopControl;
  const [taken, setTaken] = useState(false);
  const [menu, setMenu] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);
  const menuBtn = useRef<HTMLButtonElement>(null);
  const [wins, setWins] = useState<DeskWin[] | null>(null);
  const [pinned, setPinned] = useState(false);
  const [hint, setHint] = useState<string | null>(null);
  const pin = useRef<{ pid: number; windowId: number } | null>(null);
  const viewRef = useRef<string | null>(null);
  const down = useRef<{ x: number; y: number } | null>(null);
  const clickTimer = useRef<(() => void) | null>(null);
  const scrollAcc = useRef<{ x: number; y: number; dx: number; dy: number; shot: Record<string, unknown> } | null>(null);
  const scrollTimer = useRef(0);
  const scrollBusy = useRef(false);
  const frameRef = useRef<LiveFrame | null>(null);
  const controlRef = useRef(false);
  const tail = useRef(Promise.resolve());
  const draftRef = useRef<HTMLInputElement>(null);
  const [draft, setDraft] = useState("");
  const lastPoint = useRef<{ x: number; y: number } | null>(null);
  const asked = useRef(0);
  useEffect(() => {
    if (!running) setTaken(false);
  }, [running]);
  useEffect(
    () => () => {
      clickTimer.current?.();
      clickTimer.current = null;
      window.clearTimeout(scrollTimer.current);
      scrollAcc.current = null;
    },
    [],
  );
  useEffect(() => {
    pin.current = null;
    setPinned(false);
    setMenu(false);
    setHint(null);
    store.clearError();
  }, [sessionId]);
  useEffect(() => {
    if (!menu) return;
    const close = (e: globalThis.PointerEvent) => {
      const t = e.target;
      if (!(t instanceof Node)) return;
      if (menuRef.current?.contains(t) || menuBtn.current?.contains(t)) return;
      setMenu(false);
    };
    window.addEventListener("pointerdown", close);
    return () => window.removeEventListener("pointerdown", close);
  }, [menu]);
  // 画面在 <img> 里实际占的矩形：object-contain 会在窄高的窗口两边留白，光标层只能铺在
  // 画面本身上，否则相对坐标会被留白拉偏。每帧加载、显示区变大小时重算（没变不触发渲染）。
  const [content, setContent] = useState<ContentRect | null>(null);
  const measure = useCallback(() => {
    const el = img.current;
    if (!el || !el.naturalWidth || !el.naturalHeight) return;
    const scale = Math.min(el.clientWidth / el.naturalWidth, el.clientHeight / el.naturalHeight);
    const width = el.naturalWidth * scale;
    const height = el.naturalHeight * scale;
    const next = {
      left: el.offsetLeft + (el.clientWidth - width) / 2,
      top: el.offsetTop + (el.clientHeight - height) / 2,
      width,
      height,
    };
    setContent((prev) =>
      prev && Object.keys(next).every((k) => Math.abs(prev[k as keyof ContentRect] - next[k as keyof ContentRect]) < 0.5)
        ? prev
        : next,
    );
    const need = pixelBudget(box.current);
    if (asked.current > 0 && need > asked.current * 1.25) {
      asked.current = need;
      setAttempt((n) => n + 1);
    }
  }, []);
  useEffect(() => {
    const el = box.current;
    if (!el) return;
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [measure]);

  useEffect(() => {
    const client = currentClient();
    let alive = true;
    let viewId: string | null = null;
    let last = "";
    setState({ kind: "connecting" });
    setHasFrame(false);
    setCursor(null);
    const off = client.onNotification((n) => {
      if (!n.method.startsWith("desktop/view/")) return;
      const p = n.params as Record<string, unknown>;
      // 首帧可能早于 open 的回包：先按会话收。
      if (p.threadId !== sessionId || (viewId && p.viewId !== viewId)) return;
      if (n.method === "desktop/view/frame") {
        if (img.current) img.current.src = `data:${p.mime ?? "image/png"};base64,${p.data}`;
        const w = (p.window ?? {}) as Record<string, unknown>;
        const next: LiveFrame = {
          width: Number(p.width) || 0,
          height: Number(p.height) || 0,
          source: p.source === "agent" ? "agent" : p.source === "pin" ? "pin" : "front",
          pid: num(w.pid),
          windowId: num(w.windowId),
          // 应用名和窗口标题一样（「计算器 · 计算器」）只留一个。
          label: [...new Set([w.app, w.title].filter((x): x is string => typeof x === "string" && !!x))].join(" · "),
        };
        // 每帧都 setState 会 4 次 / 秒重渲染：只在窗口 / 尺寸变了时换。
        const key = JSON.stringify(next);
        if (key !== last) {
          last = key;
          setFrame(next);
        }
        setHasFrame(true);
        setState((s) => (s.kind === "live" ? s : { kind: "live" }));
      } else if (n.method === "desktop/view/status") {
        const message = typeof p.message === "string" ? p.message : "";
        if (p.state === "no_window") setState({ kind: "no_window", message });
        else if (p.state === "capture_failed") setState({ kind: "capture_failed", message });
      } else if (n.method === "desktop/view/cursor") {
        setCursor({
          seq: ++seq.current,
          x: num(p.x),
          y: num(p.y),
          action: typeof p.action === "string" ? p.action : "click",
          label: typeof p.label === "string" ? p.label : null,
          keys: Array.isArray(p.keys) ? p.keys.map(String) : [],
        });
      } else if (n.method === "desktop/view/closed" && p.reason !== "replaced") {
        viewId = null;
        setCursor(null);
        setState({ kind: "error", message: "cua-driver 断开了，正在重连…" });
        window.setTimeout(() => alive && setAttempt((a) => a + 1), 2000);
      }
    });
    // 按面板铺满后的实际像素要图（Retina 上最多 2 倍），避免小窗口被放大后发糊。
    const maxDimension = pixelBudget(box.current);
    asked.current = maxDimension;
    const openArgs: Record<string, unknown> = { threadId: sessionId, maxDimension };
    if (pin.current) {
      openArgs.pid = pin.current.pid;
      openArgs.windowId = pin.current.windowId;
    }
    client.call<{ viewId: string }>("desktop/view/open", openArgs).then(
      (r) => {
        if (!alive) {
          if (currentClient() === client) client.call("desktop/view/close", { viewId: r.viewId }).catch(() => {});
          return;
        }
        viewId = r.viewId;
        viewRef.current = r.viewId;
      },
      (e) => {
        if (!alive) return;
        // 网关说这次 open 已被同一会话更新的那次取代：等那次的结果。
        if (e instanceof RpcError && e.code === "superseded") return;
        setState({ kind: "error", message: e instanceof Error ? e.message : String(e) });
        window.setTimeout(() => alive && setAttempt((a) => a + 1), 3000);
      },
    );
    return () => {
      alive = false;
      off();
      viewRef.current = null;
      if (viewId && currentClient() === client) client.call("desktop/view/close", { viewId }).catch(() => {});
    };
  }, [sessionId, attempt]);

  // agent 正在做的像素点击：先把光标滑到那里（按参数换算），动作做完再由网关校准。
  const intent = acting && frame ? intentFor(steps, frame) : null;
  const intentKey = intent ? `${acting?.id}:${intent.left.toFixed(3)}:${intent.top.toFixed(3)}` : null;
  useEffect(() => {
    if (!intent || !acting) return;
    setCursor({ seq: ++seq.current, x: intent.left, y: intent.top, action: "move", label: acting.verb, keys: [] });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [intentKey]);

  const live = state.kind === "live";
  const tone = STATE_TONE[state.kind];
  const where = pinned || frame?.source === "pin" ? "钉住的窗口" : frame?.source === "agent" ? "Agent 操作的窗口" : "最前面的窗口";
  const note =
    state.kind === "live"
      ? [frame?.label, where].filter(Boolean).join(" · ")
      : state.kind === "connecting"
        ? "正在连接…"
        : state.message;
  const control = canControl && live && hasFrame && (!running || taken);
  frameRef.current = frame;
  controlRef.current = control;

  const enqueue = useCallback((job: () => Promise<void>) => {
    tail.current = tail.current.then(job);
  }, []);
  const deliver = useCallback((event: Record<string, unknown>) => {
    const id = viewRef.current;
    if (!id) return Promise.resolve();
    return currentClient()
      .call("desktop/view/input", { viewId: id, event })
      .then(
        () => setHint(null),
        (e) => setHint(driverHint(e)),
      );
  }, []);
  const send = useCallback((event: Record<string, unknown>) => enqueue(() => deliver(event)), [deliver, enqueue]);

  const at = (e: { clientX: number; clientY: number }) => {
    const el = img.current;
    const shot = frameRef.current;
    if (!el || !shot?.width || !shot.height) return null;
    return pointOnFrame(e, el, shot.width, shot.height);
  };

  // 一次滑动里的滚轮事件先加在一起。上一轮还没回来时继续累加，回来后再发一轮，避免一点一点地等。
  const kickScroll = () => {
    if (scrollBusy.current) return;
    const acc = scrollAcc.current;
    if (!acc || (!acc.dx && !acc.dy) || !controlRef.current) return;
    scrollAcc.current = null;
    scrollBusy.current = true;
    void deliver({ type: "scroll", x: acc.x, y: acc.y, dx: acc.dx, dy: acc.dy, ...acc.shot }).then(() => {
      scrollBusy.current = false;
      kickScroll();
    });
  };
  const flushScroll = () => {
    window.clearTimeout(scrollTimer.current);
    kickScroll();
  };

  const pointerDown = (e: PointerEvent<HTMLImageElement>) => {
    if (!control) return;
    clickTimer.current?.();
    clickTimer.current = null;
    const p = at(e);
    if (!p) return;
    down.current = p;
    e.currentTarget.setPointerCapture(e.pointerId);
    if (e.button === 0 && !(e.ctrlKey && !e.metaKey)) draftRef.current?.focus();
  };

  const pointerUp = (e: PointerEvent<HTMLImageElement>) => {
    if (!control) return;
    flushScroll();
    const start = down.current;
    down.current = null;
    const p = at(e) ?? start;
    if (!start || !p) return;
    const shot = frameFields(frameRef.current);
    lastPoint.current = p;
    const right = e.button === 2 || (e.button === 0 && e.ctrlKey && !e.metaKey);
    if (right) {
      send({ type: "click", ...p, button: "right", count: 1, ...shot });
      return;
    }
    if (e.button !== 0) return;
    if (Math.hypot(p.x - start.x, p.y - start.y) >= 8) {
      clickTimer.current?.();
      clickTimer.current = null;
      send({ type: "drag", x: start.x, y: start.y, toX: p.x, toY: p.y, ...shot });
      return;
    }
    if (e.detail >= 2) {
      clickTimer.current?.();
      clickTimer.current = null;
      send({ type: "click", ...p, button: "left", count: 2, ...shot });
      return;
    }
    // 单击先占住发送队列等 220ms：这段里的打字会排在点击后面，双击则改发双击。
    enqueue(
      () =>
        new Promise<void>((resolve) => {
          const timer = window.setTimeout(() => {
            clickTimer.current = null;
            if (!controlRef.current) {
              resolve();
              return;
            }
            void deliver({ type: "click", ...p, button: "left", count: 1, ...shot }).then(resolve);
          }, 220);
          clickTimer.current = () => {
            window.clearTimeout(timer);
            resolve();
          };
        }),
    );
  };

  const wheel = (e: WheelEvent<HTMLImageElement>) => {
    if (!control) return;
    const p = at(e);
    if (!p) return;
    e.preventDefault();
    // 行模式、页模式的 delta 比像素小很多，先换成大约相当的像素再累加。
    const unit = e.deltaMode === 1 ? 32 : e.deltaMode === 2 ? 400 : 1;
    const dx = e.deltaX * unit;
    const dy = e.deltaY * unit;
    if (!dx && !dy) return;
    const acc = scrollAcc.current;
    if (acc) {
      acc.dx += dx;
      acc.dy += dy;
      acc.x = p.x;
      acc.y = p.y;
    } else {
      scrollAcc.current = { x: p.x, y: p.y, dx, dy, shot: frameFields(frameRef.current) };
    }
    window.clearTimeout(scrollTimer.current);
    scrollTimer.current = window.setTimeout(kickScroll, scrollBusy.current ? 0 : 40);
  };

  const submitText = () => {
    const text = draft;
    if (!text || !controlRef.current) return;
    setDraft("");
    const point = lastPoint.current;
    flushScroll();
    send({
      type: "text",
      text,
      ...frameFields(frameRef.current),
      ...(point ? { x: point.x, y: point.y } : {}),
    });
  };

  const sendKey = (e: KeyboardEvent<HTMLInputElement>) => {
    flushScroll();
    send({ type: "key", key: e.key, modifiers: keyMods(e), ...frameFields(frameRef.current) });
  };

  const onDraftKey = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.nativeEvent.isComposing || e.key === "Process") return;
    if (["Meta", "Control", "Alt", "Shift"].includes(e.key)) return;
    // 侧栏、设置这些留给应用。复制粘贴留在输入条里。
    if (SHORTCUTS.some((s) => !s.fixed && matches(e.nativeEvent, s.id))) return;
    const command = e.metaKey || e.ctrlKey;
    if (command && !e.altKey && "acvxz".includes(e.key.toLowerCase())) return;
    const combo = command || e.altKey || e.shiftKey;
    if (e.key === "Enter") {
      e.preventDefault();
      if (!combo && draft !== "") submitText();
      else sendKey(e);
      return;
    }
    if ((command || e.altKey) && e.key.length > 0) {
      e.preventDefault();
      sendKey(e);
      return;
    }
    // 输入条空着时，回车、方向键、功能键直接送给窗口。有字时留给输入条编辑。
    if (draft !== "" || !windowKey(e.key)) return;
    e.preventDefault();
    sendKey(e);
  };

  const openMenu = () => {
    setMenu(true);
    setWins(null);
    currentClient()
      .call<{ windows: DeskWin[] }>("desktop/view/windows", { threadId: sessionId })
      .then(
        (r) => setWins(r.windows),
        (e) => {
          setHint(driverHint(e));
          setWins([]);
        },
      );
  };

  const choose = (next: { pid: number; windowId: number } | null) => {
    pin.current = next;
    setPinned(!!next);
    setMenu(false);
    const id = viewRef.current;
    if (!id) return;
    const args: Record<string, unknown> = { viewId: id };
    if (next) {
      args.pid = next.pid;
      args.windowId = next.windowId;
    }
    currentClient()
      .call("desktop/view/pin", args)
      .catch((e) => setHint(driverHint(e)));
  };

  return (
    <div className="relative flex min-h-0 flex-1 flex-col gap-2.5 bg-[#f2f2f4] px-4 pb-4 pt-3">
      <div className="flex items-center gap-2">
        <span className="relative size-2 shrink-0">
          {live && <span className="pulse-halo absolute inset-0 rounded-full bg-[#34c759]" />}
          <span className={`motion-colors absolute inset-0 rounded-full ${tone.dot}`} />
        </span>
        <span className="shrink-0 whitespace-nowrap text-xs font-medium text-ink-2">{pinned || frame?.source === "pin" ? "钉住的画面" : "实时画面"}</span>
        <span className={`min-w-0 flex-1 truncate text-xs ${hint ? "text-[#b25e00]" : tone.text}`}>{hint ?? note}</span>
        {canControl && live && running && (
          <button
            type="button"
            onClick={() => setTaken((v) => !v)}
            className={`motion-colors flex h-6 shrink-0 items-center gap-1 rounded-full px-2 text-[11px] font-medium ${
              taken ? "bg-accent text-white" : "bg-white text-ink-2 ring-1 ring-line-strong hover:bg-hover"
            }`}
          >
            <Hand size={12} />
            {taken ? "交还" : "接手"}
          </button>
        )}
        {canControl && (
          <button
            ref={menuBtn}
            type="button"
            onClick={() => (menu ? setMenu(false) : openMenu())}
            className="motion-colors flex h-6 shrink-0 items-center gap-1 rounded-full bg-white px-2 text-[11px] font-medium text-ink-2 ring-1 ring-line-strong hover:bg-hover"
          >
            <AppWindow size={12} />
            {pinned ? "已钉住" : "钉住窗口"}
          </button>
        )}
      </div>
      {menu && (
        <div
          ref={menuRef}
          className="absolute right-4 top-10 z-20 flex max-h-80 w-72 flex-col overflow-hidden rounded-lg border border-line-strong bg-white py-1 shadow-[0px_8px_24px_rgba(0,0,0,0.12)]"
        >
          <button
            type="button"
            onClick={() => choose(null)}
            className="px-3 py-1.5 text-left text-[12px] text-ink-2 hover:bg-hover"
          >
            跟随最前面的窗口
          </button>
          <div className="mx-2 my-1 h-px bg-line-soft" />
          <div className="thin-scroll flex min-h-0 flex-col overflow-y-auto">
            {wins === null ? (
              <span className="px-3 py-2 text-[12px] text-ink-4">正在列出窗口…</span>
            ) : wins.length === 0 ? (
              <span className="px-3 py-2 text-[12px] text-ink-4">没有可钉的窗口</span>
            ) : (
              wins.map((w) => {
                const on = pin.current?.pid === w.pid && pin.current.windowId === w.windowId;
                const { app, title } = winParts(w);
                return (
                  <button
                    key={`${w.pid}:${w.windowId}`}
                    type="button"
                    onClick={() => choose(w)}
                    className={`flex w-full shrink-0 flex-col items-start gap-0.5 px-3 py-1.5 text-left hover:bg-hover ${
                      on ? "bg-[#eef3ff]" : ""
                    }`}
                  >
                    <span className={`w-full truncate text-[13px] font-medium leading-5 ${on ? "text-accent" : "text-[#1c1c1e]"}`}>
                      {app}
                    </span>
                    {title && (
                      <span className="w-full truncate text-[11px] leading-4 text-[#6e6e73]">{title}</span>
                    )}
                  </button>
                );
              })
            )}
          </div>
        </div>
      )}
      <div
        ref={box}
        className="relative min-h-0 flex-1 overflow-hidden rounded-[10px] border border-line-strong bg-[#e4e6ea]"
      >
        <div className="absolute inset-0">
          <img
            ref={img}
            alt=""
            draggable={false}
            onLoad={measure}
            onPointerDown={pointerDown}
            onPointerUp={pointerUp}
            onWheel={wheel}
            onContextMenu={(e) => control && e.preventDefault()}
            className={`absolute inset-0 h-full w-full select-none object-contain transition-[opacity,filter] duration-200 ${
              hasFrame ? "" : "invisible"
            } ${hasFrame && !live ? "opacity-50 grayscale" : ""} ${control ? "cursor-default" : ""}`}
          />
          {showCursor && hasFrame && live && content && <AgentCursor event={cursor} rect={content} />}
        </div>
        {!hasFrame && state.kind === "no_window" && (
          <div className="absolute inset-0 flex items-center justify-center px-6 text-center text-xs text-ink-4">{state.message}</div>
        )}
        {!hasFrame && state.kind !== "no_window" && (
          <div className="absolute inset-0 flex items-center justify-center">
            <Loader2 size={18} className="animate-spin text-ink-4" />
          </div>
        )}
        {acting && live && (
          <div className="enter-up pointer-events-none absolute bottom-4 left-1/2 flex max-w-[90%] -translate-x-1/2 items-center gap-2 rounded-full bg-[#1c1c1ee6] py-1.5 pl-3 pr-3.5 shadow-[0px_6px_20px_rgba(0,0,0,0.18)]">
            <span className="relative size-2 shrink-0">
              <span className="pulse-halo absolute inset-0 rounded-full bg-accent" />
              <span className="absolute inset-0 rounded-full bg-accent" />
            </span>
            <span className="shrink-0 text-xs font-medium text-white">Agent 正在操作</span>
            <span className="min-w-0 truncate text-xs text-white/70">
              {acting.verb} {acting.detail}
            </span>
          </div>
        )}
      </div>
      <form
        className="flex shrink-0 items-center gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          submitText();
        }}
      >
        <input
          ref={draftRef}
          value={draft}
          disabled={!control}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={onDraftKey}
          placeholder={
            control ? "打字后回车送出。空着按回车或组合键，会直接送给窗口" : running ? "先点「接手」再打字" : "等画面出来后再打字"
          }
          aria-label="送到这个窗口的文字"
          className="h-8 min-w-0 flex-1 rounded-lg border border-line-strong bg-white px-3 text-[13px] text-ink outline-none placeholder:text-ink-4 focus:border-accent disabled:bg-[#f4f4f5]"
        />
        <button
          type="submit"
          disabled={!control || draft.length === 0}
          className="h-8 shrink-0 rounded-lg bg-ink px-3 text-[12px] font-medium text-white disabled:opacity-40"
        >
          发送
        </button>
      </form>
    </div>
  );
}

/** 空输入条上这些键送给窗口，不留在输入框里。 */
function windowKey(key: string): boolean {
  return (
    key === "Enter" ||
    key === "Escape" ||
    key === "Tab" ||
    key === "Backspace" ||
    key === "Delete" ||
    key === "Home" ||
    key === "End" ||
    key === "PageUp" ||
    key === "PageDown" ||
    key.startsWith("Arrow") ||
    /^F\d{1,2}$/.test(key)
  );
}

/** 面板铺满后，按较长边的设备像素向驱动要图。上限 2048，下限 640。 */
function pixelBudget(el: HTMLElement | null): number {
  const w = el?.clientWidth ?? 800;
  const h = el?.clientHeight ?? 600;
  const px = Math.max(w, h) * Math.min(2, window.devicePixelRatio || 1);
  return Math.round(Math.min(2048, Math.max(640, px)));
}

/** 操作失败留在画面上，不写进侧栏的项目列表。驱动的长 JSON 收成一句。 */
function driverHint(e: unknown): string {
  let raw = e instanceof Error ? e.message : String(e);
  raw = raw.replace(/^desktop\/view\/[a-z]+:\s*/i, "");
  if (raw.includes("minimized_or_hidden_window")) return "这个窗口在后台或被最小化，后台点不到";
  if (raw.includes("画面已经换了") || raw.includes("还没有画面")) return "画面刚换过，请再点一次";
  const line = raw
    .split("\n")
    .map((s) => s.trim())
    .find((s) => s && !s.startsWith("{"));
  const text = line ?? "操作没送出去";
  return text.length > 72 ? `${text.slice(0, 72)}…` : text;
}

/** 点下去时那一帧的身份。网关拿它和当前帧对，对不上就不点。 */
function frameFields(frame: LiveFrame | null): Record<string, unknown> {
  if (!frame) return {};
  const out: Record<string, unknown> = { width: frame.width, height: frame.height };
  if (frame.pid !== null) out.pid = frame.pid;
  if (frame.windowId !== null) out.windowId = frame.windowId;
  return out;
}

function cleanLabel(value: string | null): string {
  if (!value) return "";
  return value.replace(/\s+/g, " ").trim();
}

/** 应用名一行、窗口标题一行。标题和应用名一样就不重复。 */
function winParts(w: { app: string | null; title: string | null }): { app: string; title: string } {
  const app = cleanLabel(w.app);
  const title = cleanLabel(w.title);
  return {
    app: app || title || "未命名窗口",
    title: title && title !== app ? title : "",
  };
}

/** 指针在画面上的位置，换成这一帧的像素。点在留白上就没有。 */
function pointOnFrame(
  e: { clientX: number; clientY: number },
  el: HTMLImageElement,
  width: number,
  height: number,
): { x: number; y: number } | null {
  if (!el.naturalWidth || !el.naturalHeight) return null;
  const rect = el.getBoundingClientRect();
  const scale = Math.min(rect.width / el.naturalWidth, rect.height / el.naturalHeight);
  const shownW = el.naturalWidth * scale;
  const shownH = el.naturalHeight * scale;
  const left = rect.left + (rect.width - shownW) / 2;
  const top = rect.top + (rect.height - shownH) / 2;
  const x = ((e.clientX - left) / shownW) * width;
  const y = ((e.clientY - top) / shownH) * height;
  if (x < 0 || y < 0 || x > width || y > height) return null;
  return { x: Math.round(x), y: Math.round(y) };
}

function keyMods(e: { metaKey: boolean; ctrlKey: boolean; altKey: boolean; shiftKey: boolean }): string[] {
  const mods: string[] = [];
  if (e.metaKey) mods.push("cmd");
  if (e.ctrlKey) mods.push("ctrl");
  if (e.altKey) mods.push("alt");
  if (e.shiftKey) mods.push("shift");
  return mods;
}

/**
 * agent 正在做的像素点击落在画面上的哪里（0–1 的比例）：只认同一个窗口、还在跑的那一步。
 * 坐标是 agent 那张截图的像素，按它之前最近一次截同一窗口的 `size=` 换算。换算不了就没有。
 */
function intentFor(steps: Step[], frame: LiveFrame): { left: number; top: number } | null {
  const i = steps.length - 1;
  const step = steps[i];
  if (!step || step.status !== "running" || !step.point) return null;
  if (frame.windowId === null || step.windowId !== frame.windowId) return null;
  for (let j = i - 1; j >= 0; j--) {
    const shot = steps[j];
    if (shot.windowId !== step.windowId || !shot.shotSize) continue;
    const left = step.point.x / shot.shotSize.width;
    const top = step.point.y / shot.shotSize.height;
    return left >= 0 && left <= 1 && top >= 0 && top <= 1 ? { left, top } : null;
  }
  return null;
}
