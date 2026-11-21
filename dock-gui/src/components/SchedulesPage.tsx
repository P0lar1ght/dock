// 侧栏「定时任务」（dock.1 `schedule/*`）：按间隔、按日历时间（cron）、只一次、条件命令、
// 文件变化或只手动地向会话发提问；每次可以送进原会话，也可以在同一个项目里新开会话。
// 任务属于会话，落盘、跨重启；比一天还勤的默认 7 天过期。照 wonder-ui/pages/PageSchedules*：
// 按会话分组的列表（上次没发出去 / 由其他窗口运行两种特殊行；点开看最近的运行）、立即运行 /
// 暂停、新建 / 编辑弹窗、空状态、删除确认。变动跟着 `schedule/changed` 推送重拉。

import { useEffect, useMemo, useState, type ReactNode } from "react";
import {
  AlarmClock,
  CalendarClock,
  ChevronDown,
  ChevronRight,
  Clock,
  FolderSync,
  Hand,
  Pause,
  Pencil,
  Play,
  Plus,
  SquareTerminal,
  Trash2,
  Zap,
} from "lucide-react";
import {
  onSchedulesChanged,
  scheduleCreate,
  scheduleDelete,
  scheduleList,
  scheduleRun,
  scheduleUpdate,
  type FireReason,
  type ScheduleInput,
  type ScheduleTask,
  type WatchWhen,
} from "../dock/scheduleApi";
import { errorText } from "../dock/settingsApi";
import type { SessionSummary } from "../model";
import { Checkbox, dangerText, Modal, primaryButton, secondaryButton, Segmented, Select, TextInput } from "./settings/ui";
import { ago, baseName, EmptyCard, fullTime, NavPageShell, until } from "./navPage";

const UNITS = [
  { value: "60", label: "分钟" },
  { value: "3600", label: "小时" },
  { value: "86400", label: "天" },
] as const;

/** 剩不到 1 天就标橙。 */
const EXPIRY_WARN_MS = 86_400_000;

export function SchedulesPage({
  sessions,
  onOpenSession,
  initialSessionId,
  insetLeft,
}: {
  insetLeft?: boolean;
  sessions: SessionSummary[];
  onOpenSession: (id: string) => void;
  /** 新建时默认选这个会话（正在看的那个）。 */
  initialSessionId: string | null;
}) {
  const [tasks, setTasks] = useState<ScheduleTask[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [editing, setEditing] = useState<ScheduleTask | "new" | null>(null);
  const [deleting, setDeleting] = useState<ScheduleTask | null>(null);
  const [now, setNow] = useState(Date.now());

  const reload = () =>
    scheduleList()
      .then((t) => {
        setTasks(t);
        setError(null);
        // 相对时间按拉到数据这一刻算：不然新建的任务会显示成「8 天后过期」。
        setNow(Date.now());
      })
      .catch((e) => setError(errorText(e)));
  useEffect(() => {
    reload();
    const off = onSchedulesChanged(reload);
    // 相对时间（「下次 3 分钟后」）跟着走。
    const tick = window.setInterval(() => setNow(Date.now()), 30_000);
    return () => {
      off();
      window.clearInterval(tick);
    };
  }, []);

  const groups = useMemo(() => {
    const byThread = new Map<string, ScheduleTask[]>();
    for (const t of tasks ?? []) {
      const key = t.threadId ?? "";
      byThread.set(key, [...(byThread.get(key) ?? []), t]);
    }
    return [...byThread.entries()].map(([threadId, rows]) => {
      const session = sessions.find((s) => s.id === threadId);
      return {
        threadId,
        title: session?.title || rows[0].threadTitle || (threadId ? "（未命名会话）" : "第一页"),
        project: baseName(session?.cwd ?? rows[0].cwd),
        rows: rows.sort((a, b) => (a.nextAtMs ?? Infinity) - (b.nextAtMs ?? Infinity)),
      };
    });
  }, [tasks, sessions]);

  const newButton = (
    <button type="button" className={primaryButton} onClick={() => setEditing("new")}>
      <Plus size={13} />
      新建定时任务
    </button>
  );

  return (
    <NavPageShell title="定时任务" subtitle="按时间、条件命令或文件变化向会话发送提问 · 保存在 ~/.dock/schedules.json" actions={newButton} insetLeft={insetLeft}>
      {error && (
        <div className="px-8 pt-4">
          <div className="flex items-center gap-2 rounded-lg border border-[#ff3b3033] bg-[#ff3b3014] px-3 py-2 text-[12.5px] text-[#d70015]">
            <span className="min-w-0 flex-1">{error}</span>
            <button type="button" className="font-medium hover:underline" onClick={reload}>
              重试
            </button>
          </div>
        </div>
      )}
      {tasks === null ? (
        !error && (
          <div className="flex flex-col gap-2 px-8 pt-5">
            {[0, 1].map((i) => (
              <div key={i} className="skeleton h-14 rounded-lg" />
            ))}
          </div>
        )
      ) : tasks.length === 0 ? (
        <EmptyCard icon={Clock} title="还没有定时任务" body="在对话里输入 /loop 工作日 9 点 汇总昨天的提交、/loop CI 挂了就叫我，或点下面的按钮">
          <button type="button" className={`${primaryButton} mt-1`} onClick={() => setEditing("new")}>
            <Plus size={13} />
            新建定时任务
          </button>
        </EmptyCard>
      ) : (
        <div className="thin-scroll flex min-h-0 flex-1 flex-col gap-5 overflow-y-auto px-8 py-5">
          {groups.map((g) => (
            <section key={g.threadId} className="flex flex-col gap-1">
              <div className="flex items-center gap-2 px-1">
                <span className="max-w-[420px] truncate text-[13px] font-semibold text-ink">{g.title}</span>
                {g.project && <span className="text-xs text-ink-4">{g.project}</span>}
                {g.threadId && (
                  <button type="button" onClick={() => onOpenSession(g.threadId)} className="ml-auto text-xs font-medium text-accent hover:underline">
                    打开会话
                  </button>
                )}
              </div>
              {g.rows.map((t) => (
                <TaskLine
                  key={t.id}
                  task={t}
                  now={now}
                  onEdit={() => setEditing(t)}
                  onDelete={() => setDeleting(t)}
                  onOpenSession={onOpenSession}
                  onChanged={reload}
                />
              ))}
            </section>
          ))}
        </div>
      )}
      {editing && (
        <TaskDialog
          task={editing === "new" ? null : editing}
          sessions={sessions}
          defaultSessionId={initialSessionId}
          onClose={() => setEditing(null)}
          onDelete={editing === "new" ? undefined : () => setDeleting(editing)}
          onSaved={() => {
            setEditing(null);
            reload();
          }}
        />
      )}
      {deleting && (
        <Modal onClose={() => setDeleting(null)}>
          <DeleteConfirm
            task={deleting}
            onCancel={() => setDeleting(null)}
            onDeleted={() => {
              setDeleting(null);
              setEditing(null);
              reload();
            }}
          />
        </Modal>
      )}
    </NavPageShell>
  );
}

const KIND_ICON = {
  every: Clock,
  cron: CalendarClock,
  at: AlarmClock,
  watch: SquareTerminal,
  files: FolderSync,
  manual: Hand,
} as const;

const REASON_TEXT: Record<FireReason, string> = {
  schedule: "到点",
  catchUp: "补跑",
  immediate: "创建时",
  manual: "手动",
  watch: "条件满足",
  files: "文件改动",
};

const OUTCOME_TEXT = { sent: "已送达", coalesced: "上一次还没跑完，这次跳过", failed: "没送到" } as const;

/** 「下次 3 分钟后」「下次检查 …」「等文件改动」「已暂停」…… */
function nextText(task: ScheduleTask, now: number): ReactNode {
  if (task.paused) return "已暂停";
  if (task.finished) return "已跑完";
  if (task.pending) return "马上跑";
  const kind = task.schedule.kind;
  if (kind === "files") return "等文件改动";
  if (kind === "manual") return "点「立即运行」或 dock schedule run 时跑";
  if (task.nextAtMs === null) return "已跑完";
  return (
    <span title={fullTime(task.nextAtMs)}>
      {kind === "watch" ? "下次检查" : "下次"} {until(task.nextAtMs, now)}
    </span>
  );
}

function TaskLine({
  task,
  now,
  onEdit,
  onDelete,
  onOpenSession,
  onChanged,
}: {
  task: ScheduleTask;
  now: number;
  onEdit: () => void;
  onDelete: () => void;
  onOpenSession: (id: string) => void;
  onChanged: () => void;
}) {
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const Icon = KIND_ICON[task.schedule.kind];
  const left = task.expiresAtMs === null ? Infinity : task.expiresAtMs - now;
  const expiry =
    task.expiresAtMs === null
      ? task.schedule.kind === "at"
        ? null
        : "不过期"
      : left <= 0
        ? "已过期"
        : left < EXPIRY_WARN_MS
          ? `${Math.max(1, Math.round(left / 3_600_000))} 小时后过期`
          : `${Math.ceil(left / 86_400_000)} 天后过期`;
  const act = (run: () => Promise<unknown>) => {
    setBusy(true);
    setActionError(null);
    run()
      .then(onChanged)
      .catch((e) => setActionError(errorText(e)))
      .finally(() => setBusy(false));
  };
  const watch = task.schedule.kind === "watch" ? task.schedule : null;
  const files = task.schedule.kind === "files" ? task.schedule : null;
  return (
    <div className="flex flex-col">
      <div className={`motion-colors group flex h-14 items-center gap-3 rounded-lg px-2 hover:bg-[#f7f7f8] ${task.paused ? "opacity-60" : ""}`}>
        <Icon size={15} className="shrink-0 text-ink-4" />
        <button type="button" onClick={() => setOpen(!open)} className="flex min-w-0 flex-1 flex-col items-start gap-0.5 text-left" aria-expanded={open}>
          <span title={task.prompt} className="flex w-full min-w-0 items-center gap-1 text-[13px] font-medium text-ink">
            <ChevronRight size={12} className={`shrink-0 text-ink-4 transition-transform duration-150 ${open ? "rotate-90" : ""}`} />
            <span className="truncate">{task.prompt}</span>
          </span>
          <span className="w-full truncate pl-4 text-xs text-ink-4">
            {task.scheduleLabel} · {nextText(task, now)}
            {expiry && (
              <>
                {" · "}
                <span title={task.expiresAtMs === null ? undefined : fullTime(task.expiresAtMs)} className={left < EXPIRY_WARN_MS ? "text-[#c93400]" : ""}>
                  {expiry}
                </span>
              </>
            )}
            {task.maxRuns !== null && ` · 第 ${task.runs}/${task.maxRuns} 次`}
          </span>
        </button>
        {task.target === "newSession" && (
          <span title="每次在这个项目里新开一个会话" className="shrink-0 rounded-full bg-[#f2f2f7] px-2 py-0.5 text-[11px] font-medium text-[#636366]">
            新会话
          </span>
        )}
        {!task.heldHere && task.holderLive && (
          <span
            title="另一个 Dock 进程持有它；那个进程退出后，这里会接管。"
            className="shrink-0 rounded-full bg-[#f2f2f7] px-2 py-0.5 text-[11px] font-medium text-[#636366]"
          >
            由其他窗口运行
          </span>
        )}
        {task.lastFiredAtMs !== null && (
          <span title={fullTime(task.lastFiredAtMs)} className="shrink-0 text-xs text-ink-4 group-hover:hidden group-focus-within:hidden">
            上次 {ago(task.lastFiredAtMs, now)}
          </span>
        )}
        <div className="hidden shrink-0 items-center gap-1 group-hover:flex group-focus-within:flex">
          <LineButton label="立即运行" onClick={() => act(() => scheduleRun(task.id))} disabled={busy}>
            <Zap size={13} />
          </LineButton>
          {!task.finished && (
            <LineButton label={task.paused ? "继续" : "暂停"} onClick={() => act(() => scheduleUpdate({ id: task.id, paused: !task.paused }))} disabled={busy}>
              {task.paused ? <Play size={13} /> : <Pause size={13} />}
            </LineButton>
          )}
          <LineButton label="编辑" onClick={onEdit}>
            <Pencil size={13} />
          </LineButton>
          <LineButton label="删除" onClick={onDelete} danger>
            <Trash2 size={13} />
          </LineButton>
        </div>
      </div>
      {(task.lastError || actionError) && (
        <div className="enter-fade mb-1 ml-9 mr-2 rounded-md bg-[#ff3b300f] px-2.5 py-1.5 text-xs text-[#d70015]">
          {actionError ?? `上次没发出去：${task.lastError}`}
        </div>
      )}
      {open && (
        <div className="enter-fade mb-1.5 ml-9 mr-2 flex flex-col gap-1.5 rounded-md bg-[#f7f7f8] px-2.5 py-2 text-xs text-ink-3">
          {watch && (
            <div className="flex flex-col gap-0.5">
              <span className="break-all font-mono text-[11.5px] text-ink-2">$ {watch.command}</span>
              {task.watch && (
                <span className="text-ink-4">
                  上次检查 {ago(task.watch.checkedAtMs, now)} ·{" "}
                  {task.watch.error ? `没查成：${task.watch.error}` : `退出码 ${task.watch.exit ?? "（被信号停掉）"}`}
                  {task.watch.matched && " · 满足条件"}
                </span>
              )}
            </div>
          )}
          {files && <span className="break-all font-mono text-[11.5px] text-ink-2">{files.paths.join("   ")}</span>}
          {task.history.length === 0 ? (
            <span className="text-ink-4">还没跑过</span>
          ) : (
            task.history.map((run, i) => (
              <div key={`${run.atMs}-${i}`} className="flex min-w-0 items-center gap-2">
                <span title={fullTime(run.atMs)} className="w-[76px] shrink-0 text-ink-4">
                  {ago(run.atMs, now)}
                </span>
                <span className="w-[56px] shrink-0">{REASON_TEXT[run.reason]}</span>
                <span className={`min-w-0 truncate ${run.outcome === "failed" ? "text-[#d70015]" : run.outcome === "coalesced" ? "text-ink-4" : ""}`}>
                  {OUTCOME_TEXT[run.outcome]}
                  {run.error && `：${run.error}`}
                  {run.lateMs !== null && run.lateMs > 0 && `（晚了 ${Math.max(1, Math.round(run.lateMs / 60_000))} 分钟）`}
                </span>
                {run.threadId && (
                  <button type="button" className="ml-auto shrink-0 font-medium text-accent hover:underline" onClick={() => onOpenSession(run.threadId!)}>
                    打开会话
                  </button>
                )}
              </div>
            ))
          )}
        </div>
      )}
    </div>
  );
}

function LineButton({
  label,
  onClick,
  danger,
  disabled,
  children,
}: {
  label: string;
  onClick: () => void;
  danger?: boolean;
  disabled?: boolean;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      title={label}
      aria-label={label}
      onClick={onClick}
      disabled={disabled}
      className={`motion-colors flex size-7 items-center justify-center rounded-md border border-line-strong bg-white shadow-sm disabled:opacity-50 ${
        danger ? "text-[#ff3b30] hover:bg-[#ff3b3014]" : "text-ink-3 hover:text-ink"
      }`}
    >
      {children}
    </button>
  );
}

/** 秒数拆成「数 + 单位」：能整除天就按天，其次小时，否则分钟。 */
function splitInterval(secs: number): { amount: string; unit: string } {
  for (const u of [86_400, 3_600]) if (secs % u === 0) return { amount: String(secs / u), unit: String(u) };
  return { amount: String(Math.max(1, Math.round(secs / 60))), unit: "60" };
}

type Kind = ScheduleTask["schedule"]["kind"];
const KINDS: { value: Kind; label: string; title: string }[] = [
  { value: "every", label: "间隔", title: "每隔一段时间跑一次，适合盯着一件事直到它办完" },
  { value: "cron", label: "定时", title: "按日历时间跑，适合日报、周报这类例行事" },
  { value: "at", label: "一次", title: "到点跑一次" },
  { value: "watch", label: "命令", title: "定时跑一条命令，结果满足条件才发提问（只跑命令不花 token）" },
  { value: "files", label: "文件", title: "这些文件有改动就发提问" },
  { value: "manual", label: "手动", title: "只在点「立即运行」或执行 dock schedule run 时跑" },
];

const WHENS: { value: WatchWhen; label: string }[] = [
  { value: "changed", label: "结果变了" },
  { value: "fails", label: "失败时（退出码非 0）" },
  { value: "succeeds", label: "成功时（退出码 0）" },
  { value: "matches", label: "输出匹配正则" },
];

const TARGETS = [
  { value: "session" as const, label: "发到这个会话", title: "前几次的结果在上文里，适合盯着一件事" },
  { value: "newSession" as const, label: "每次新开会话", title: "在同一个项目里新开一个会话，上下文干净，适合例行报告" },
];

type Freq = "daily" | "weekdays" | "weekly" | "monthly" | "custom";
const FREQS: { value: Freq; label: string }[] = [
  { value: "daily", label: "每天" },
  { value: "weekdays", label: "工作日" },
  { value: "weekly", label: "每周" },
  { value: "monthly", label: "每月" },
  { value: "custom", label: "自定义 cron" },
];
const WEEKDAYS = ["1", "2", "3", "4", "5", "6", "0"].map((d) => ({ value: d, label: `周${"日一二三四五六"[Number(d)]}` }));
const MONTHDAYS = Array.from({ length: 31 }, (_, i) => ({ value: String(i + 1), label: `${i + 1} 日` }));

interface CronForm {
  freq: Freq;
  time: string;
  weekday: string;
  monthday: string;
  custom: string;
}

const pad = (n: number) => String(n).padStart(2, "0");

/** 常见的 cron 拆回表单；认不出就放进「自定义」。 */
function cronForm(expr: string | null): CronForm {
  const form: CronForm = { freq: "weekdays", time: "09:00", weekday: "1", monthday: "1", custom: expr ?? "" };
  if (!expr) return form;
  const m = expr.trim().match(/^(\d{1,2}) (\d{1,2}) (\*|\d{1,2}) \* (\*|1-5|[0-7])$/);
  if (!m) return { ...form, freq: "custom" };
  const [, min, hour, dom, dow] = m;
  const time = `${pad(Number(hour))}:${pad(Number(min))}`;
  if (dom === "*" && dow === "*") return { ...form, freq: "daily", time };
  if (dom === "*" && dow === "1-5") return { ...form, freq: "weekdays", time };
  if (dom === "*") return { ...form, freq: "weekly", time, weekday: dow === "7" ? "0" : dow };
  if (dow === "*") return { ...form, freq: "monthly", time, monthday: String(Number(dom)) };
  return { ...form, freq: "custom" };
}

function cronExpr(f: CronForm): string {
  if (f.freq === "custom") return f.custom.trim();
  const [h, m] = f.time.split(":").map(Number);
  const hm = `${m} ${h}`;
  if (f.freq === "daily") return `${hm} * * *`;
  if (f.freq === "weekdays") return `${hm} * * 1-5`;
  if (f.freq === "weekly") return `${hm} * * ${f.weekday}`;
  return `${hm} ${f.monthday} * *`;
}

/** `<input type="datetime-local">` 要的本机时间串。 */
function localInput(ms: number): string {
  const d = new Date(ms);
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/** 新建「只一次」的默认：下一个整点。 */
function nextHour(): number {
  const d = new Date();
  d.setMinutes(0, 0, 0);
  d.setHours(d.getHours() + 1);
  return d.getTime();
}

type ExpiryChoice = "keep" | "default" | "never" | "7d" | "30d";

const inputClass =
  "motion-colors h-8 rounded-lg border border-line-strong bg-white px-2.5 text-[12.5px] text-ink shadow-sm outline-none focus:border-accent";

function TaskDialog({
  task,
  sessions,
  defaultSessionId,
  onClose,
  onSaved,
  onDelete,
}: {
  task: ScheduleTask | null;
  sessions: SessionSummary[];
  defaultSessionId: string | null;
  onClose: () => void;
  onSaved: () => void;
  onDelete?: () => void;
}) {
  const spec = task?.schedule;
  const initial = splitInterval(spec?.kind === "every" ? spec.everySecs : 1800);
  const initialCheck = splitInterval(spec?.kind === "watch" ? spec.everySecs : 300);
  const [sessionId, setSessionId] = useState(task?.threadId ?? defaultSessionId ?? sessions[0]?.id ?? "");
  const [kind, setKind] = useState<Kind>(spec?.kind ?? "every");
  const [amount, setAmount] = useState(initial.amount);
  const [unit, setUnit] = useState(initial.unit);
  const [cron, setCron] = useState<CronForm>(() => cronForm(spec?.kind === "cron" ? spec.cron : null));
  const [at, setAt] = useState(() => localInput(spec?.kind === "at" ? spec.atMs : nextHour()));
  const [command, setCommand] = useState(spec?.kind === "watch" ? spec.command : "");
  const [checkAmount, setCheckAmount] = useState(initialCheck.amount);
  const [checkUnit, setCheckUnit] = useState(initialCheck.unit);
  const [when, setWhen] = useState<WatchWhen>(spec?.kind === "watch" ? spec.when : "changed");
  const [pattern, setPattern] = useState(spec?.kind === "watch" ? (spec.pattern ?? "") : "");
  const [paths, setPaths] = useState(spec?.kind === "files" ? spec.paths.join("\n") : "");
  const [debounce, setDebounce] = useState(String(spec?.kind === "files" ? spec.debounceSecs : 10));
  const [target, setTarget] = useState<"session" | "newSession">(task?.target ?? "session");
  const [expiry, setExpiry] = useState<ExpiryChoice>(task ? "keep" : "default");
  /** null = 跟着种类的默认（按时间 / 只一次补，按间隔不补）。 */
  const [catchUp, setCatchUp] = useState<boolean | null>(task ? task.catchUp : null);
  const [prompt, setPrompt] = useState(task?.prompt ?? "");
  const [runNow, setRunNow] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const everySecs = Math.round(Number(amount) * Number(unit));
  const tooShort = kind === "every" && (!Number.isFinite(everySecs) || everySecs < 60);
  const atMs = new Date(at).getTime();
  const atPast = kind === "at" && (!Number.isFinite(atMs) || atMs <= Date.now());
  const expr = cronExpr(cron);
  const cronMissing = kind === "cron" && expr === "";
  const checkSecs = Math.round(Number(checkAmount) * Number(checkUnit));
  const checkTooShort = kind === "watch" && (!Number.isFinite(checkSecs) || checkSecs < 60);
  const watchMissing = kind === "watch" && (command.trim() === "" || (when === "matches" && pattern.trim() === ""));
  const pathList = paths
    .split("\n")
    .map((p) => p.trim())
    .filter(Boolean);
  const debounceSecs = Math.max(1, Math.round(Number(debounce) || 10));
  const filesMissing = kind === "files" && pathList.length === 0;
  const invalid = tooShort || atPast || cronMissing || checkTooShort || watchMissing || filesMissing;
  const canSave = !busy && prompt.trim() !== "" && !invalid && (task !== null || sessionId !== "");
  const timeBased = kind === "every" || kind === "cron" || kind === "at";
  const catchUpOn = catchUp ?? (kind === "cron" || kind === "at");

  // 会话下拉按项目分组（最近用过的在前）。
  const byProject = useMemo(() => {
    const groups = new Map<string, SessionSummary[]>();
    for (const s of [...sessions].sort((a, b) => b.updatedAt - a.updatedAt)) {
      const key = baseName(s.cwd) || s.cwd;
      groups.set(key, [...(groups.get(key) ?? []), s]);
    }
    return [...groups.entries()];
  }, [sessions]);
  const sessionTitle = (id: string | null) => sessions.find((s) => s.id === id)?.title || task?.threadTitle || "（未命名会话）";

  const expiryOptions: { value: ExpiryChoice; label: string }[] = [
    task
      ? { value: "keep", label: task.expiresAtMs === null ? "不改（现在不过期）" : `不改（${fullTime(task.expiresAtMs)}）` }
      : { value: "default", label: "默认（比一天还勤的 7 天后）" },
    { value: "never", label: "不过期" },
    { value: "7d", label: "7 天后" },
    { value: "30d", label: "30 天后" },
  ];

  const save = async () => {
    setBusy(true);
    setError(null);
    const schedule: ScheduleInput =
      kind === "every"
        ? { everySecs }
        : kind === "cron"
          ? { cron: expr }
          : kind === "at"
            ? { atMs }
            : kind === "watch"
              ? { watch: { command: command.trim(), everySecs: checkSecs, when, ...(when === "matches" && { pattern: pattern.trim() }) } }
              : kind === "files"
                ? { files: { paths: pathList, debounceSecs } }
                : { manual: true };
    const expiresAtMs =
      kind === "at" || expiry === "keep" || expiry === "default"
        ? undefined
        : expiry === "never"
          ? null
          : Date.now() + (expiry === "7d" ? 7 : 30) * 86_400_000;
    const options = {
      ...(expiresAtMs !== undefined && { expiresAtMs }),
      ...(timeBased && catchUp !== null && catchUp !== task?.catchUp && { catchUp }),
      ...(target !== (task?.target ?? "session") && { target }),
    };
    try {
      if (task) await scheduleUpdate({ id: task.id, prompt: prompt.trim(), ...schedule, ...options });
      else await scheduleCreate({ threadId: sessionId, prompt: prompt.trim(), fireImmediately: runNow, ...schedule, ...options });
      onSaved();
    } catch (e) {
      setError(errorText(e));
      setBusy(false);
    }
  };

  return (
    <Modal onClose={onClose} width={500}>
      <div className="flex w-full flex-col gap-4">
        <div className="text-base font-semibold text-ink">{task ? "编辑定时任务" : "新建定时任务"}</div>
        <label className="flex flex-col gap-1.5">
          <span className="text-[12.5px] font-medium text-ink">发到哪个会话</span>
          {task ? (
            <div className="flex h-8 items-center rounded-lg bg-[#f5f5f7] px-2.5 text-[12.5px] text-ink-3">{sessionTitle(task.threadId)}</div>
          ) : sessions.length === 0 ? (
            <div className="text-[12.5px] text-ink-4">还没有会话：先开一个对话再来建定时任务。</div>
          ) : (
            <span className="motion-colors relative flex h-8 items-center gap-2 rounded-lg border border-line-strong bg-white px-2.5 shadow-sm focus-within:border-accent">
              <span className="min-w-0 flex-1 truncate text-[12.5px] text-ink">{sessionTitle(sessionId)}</span>
              <ChevronDown size={13} className="shrink-0 text-ink-4" />
              <select value={sessionId} onChange={(e) => setSessionId(e.target.value)} className="absolute inset-0 cursor-pointer opacity-0">
                {byProject.map(([project, rows]) => (
                  <optgroup key={project} label={project}>
                    {rows.map((s) => (
                      <option key={s.id} value={s.id}>
                        {s.title || "（未命名会话）"}
                      </option>
                    ))}
                  </optgroup>
                ))}
              </select>
            </span>
          )}
          {!task && sessions.length > 0 && <span className="text-[11px] text-ink-4">按项目分组；到点时会话关着也会在后台打开</span>}
        </label>
        <div className="flex flex-col gap-1.5">
          <span className="text-[12.5px] font-medium text-ink">什么时候</span>
          <div className="self-start">
            <Segmented value={kind} options={KINDS} onChange={setKind} />
          </div>
          {kind === "watch" && (
            <div key="watch" className="enter-fade flex flex-col gap-1.5">
              <TextInput value={command} onChange={setCommand} placeholder="gh pr checks 123" width="100%" mono invalid={watchMissing && command.trim() === ""} />
              <div className="flex flex-wrap items-center gap-2">
                <span className="text-[12.5px] text-ink-3">每</span>
                <input
                  type="number"
                  min={1}
                  value={checkAmount}
                  onChange={(e) => setCheckAmount(e.target.value)}
                  className={`${inputClass} w-[72px] ${checkTooShort ? "border-[#ff3b30]" : ""} [appearance:textfield] [&::-webkit-inner-spin-button]:appearance-none`}
                />
                <Select value={checkUnit} options={UNITS.map((u) => ({ ...u }))} onChange={setCheckUnit} width={80} />
                <span className="text-[12.5px] text-ink-3">查一次，</span>
                <Select value={when} options={WHENS} onChange={(v) => setWhen(v as WatchWhen)} width={170} />
              </div>
              {when === "matches" && <TextInput value={pattern} onChange={setPattern} placeholder="正则，例如 FAILED|error" width="100%" mono invalid={pattern.trim() === ""} />}
              <span className={`text-[11px] ${checkTooShort ? "text-[#d70015]" : "text-ink-4"}`}>
                在会话的项目目录里跑（bash）。只跑命令不花 token，满足条件才把提问连同命令输出发过去；除了「结果变了」，都是从不满足变成满足时触发一次。
              </span>
            </div>
          )}
          {kind === "files" && (
            <div key="files" className="enter-fade flex flex-col gap-1.5">
              <textarea
                value={paths}
                onChange={(e) => setPaths(e.target.value)}
                placeholder={"docs/\nsrc/**/*.rs\n~/Downloads"}
                rows={3}
                spellCheck={false}
                className={`motion-colors resize-none rounded-lg border bg-white px-2.5 py-2 font-mono text-xs leading-relaxed text-ink shadow-sm outline-none placeholder:text-ink-4 focus:border-accent ${
                  filesMissing ? "border-[#ff3b30]" : "border-line-strong"
                }`}
              />
              <div className="flex items-center gap-2">
                <span className="text-[12.5px] text-ink-3">改动停下</span>
                <input
                  type="number"
                  min={1}
                  value={debounce}
                  onChange={(e) => setDebounce(e.target.value)}
                  className={`${inputClass} w-[72px] [appearance:textfield] [&::-webkit-inner-spin-button]:appearance-none`}
                />
                <span className="text-[12.5px] text-ink-3">秒后发提问</span>
              </div>
              <span className="text-[11px] text-ink-4">
                一行一个，相对会话的项目目录（可以写 glob）。.git、node_modules、target 里的改动，以及这个任务自己那一轮跑着时的改动不算。
              </span>
            </div>
          )}
          {kind === "manual" && (
            <span key="manual" className="enter-fade text-[11px] leading-relaxed text-ink-4">
              不会自己跑：点列表里的「立即运行」，或者在脚本、git hook、CI 里执行 <code className="font-mono">dock schedule run &lt;id&gt;</code>（可以带 <code className="font-mono">--input</code> 附一段话）。
            </span>
          )}
          {kind === "every" && (
            <div key="every" className="enter-fade flex flex-col gap-1.5">
              <div className="flex items-center gap-2">
                <span className="text-[12.5px] text-ink-3">每</span>
                <input
                  type="number"
                  min={1}
                  value={amount}
                  onChange={(e) => setAmount(e.target.value)}
                  className={`${inputClass} w-[88px] ${tooShort ? "border-[#ff3b30]" : ""} [appearance:textfield] [&::-webkit-inner-spin-button]:appearance-none`}
                />
                <Select value={unit} options={UNITS.map((u) => ({ ...u }))} onChange={setUnit} width={88} />
              </div>
              <span className={`text-[11px] ${tooShort ? "text-[#d70015]" : "text-ink-4"}`}>最短 1 分钟；适合盯着一件事直到它办完</span>
            </div>
          )}
          {kind === "cron" && (
            <div key="cron" className="enter-fade flex flex-col gap-1.5">
              <div className="flex flex-wrap items-center gap-2">
                <Select value={cron.freq} options={FREQS} onChange={(v) => setCron({ ...cron, freq: v as Freq })} width={124} />
                {cron.freq === "weekly" && <Select value={cron.weekday} options={WEEKDAYS} onChange={(v) => setCron({ ...cron, weekday: v })} width={84} />}
                {cron.freq === "monthly" && <Select value={cron.monthday} options={MONTHDAYS} onChange={(v) => setCron({ ...cron, monthday: v })} width={84} />}
                {cron.freq === "custom" ? (
                  <TextInput value={cron.custom} onChange={(v) => setCron({ ...cron, custom: v })} placeholder="0 9 * * 1-5" width={180} mono invalid={cronMissing} />
                ) : (
                  <input type="time" value={cron.time} onChange={(e) => e.target.value && setCron({ ...cron, time: e.target.value })} className={`${inputClass} w-[104px]`} />
                )}
              </div>
              <span className="text-[11px] text-ink-4">
                {cron.freq === "custom" ? "5 段：分 时 日 月 周，按本机时区。" : `cron ${expr} · `}
                按本机时区；适合日报、周报这类例行事
              </span>
            </div>
          )}
          {kind === "at" && (
            <div key="at" className="enter-fade flex flex-col gap-1.5">
              <input
                type="datetime-local"
                value={at}
                onChange={(e) => setAt(e.target.value)}
                className={`${inputClass} w-[200px] ${atPast ? "border-[#ff3b30]" : ""}`}
              />
              <span className={`text-[11px] ${atPast ? "text-[#d70015]" : "text-ink-4"}`}>
                {atPast ? "这个时间已经过了" : "到点跑一次，跑完就从列表里消失"}
              </span>
            </div>
          )}
        </div>
        <div className="flex flex-col gap-1.5">
          <span className="text-[12.5px] font-medium text-ink">每次发到</span>
          <div className="self-start">
            <Segmented value={target} options={TARGETS} onChange={setTarget} />
          </div>
        </div>
        <label className="flex flex-col gap-1.5">
          <span className="text-[12.5px] font-medium text-ink">提问</span>
          <textarea
            value={prompt}
            onChange={(e) => setPrompt(e.target.value)}
            placeholder="每次到点发给 Agent 的话"
            rows={4}
            className="motion-colors min-h-[88px] resize-none rounded-lg border border-line-strong bg-white px-2.5 py-2 text-[12.5px] leading-relaxed text-ink shadow-sm outline-none placeholder:text-ink-4 focus:border-accent"
          />
        </label>
        {kind !== "at" && (
          <div className="flex items-center gap-2">
            <span className="text-[12.5px] text-ink">自动过期</span>
            <Select value={expiry} options={expiryOptions} onChange={(v) => setExpiry(v as ExpiryChoice)} width={220} />
          </div>
        )}
        {timeBased && (
          <label className="flex cursor-pointer items-center gap-2 text-[12.5px] text-ink">
            <Checkbox checked={catchUpOn} onChange={setCatchUp} label="错过了回来后补跑一次" />
            错过了（Dock 没开 / 电脑睡着）回来后补跑一次
          </label>
        )}
        {!task && kind !== "at" && (
          <label className="flex cursor-pointer items-center gap-2 text-[12.5px] text-ink">
            <Checkbox checked={runNow} onChange={setRunNow} label="创建后立即运行一次" />
            创建后立即运行一次
          </label>
        )}
        <p className="text-xs leading-relaxed text-ink-4">
          Dock 没开着的时候不运行；错过了最多补一次，不会一口气补很多次。会话忙着时到点会排队，上一次还在排就不再叠。跑起来以后照常过权限确认。
        </p>
        {error && <p className="text-[12.5px] text-[#d70015]">{error}</p>}
        <div className="flex items-center gap-2">
          {onDelete && (
            <button type="button" className={dangerText} onClick={onDelete}>
              删除
            </button>
          )}
          <div className="ml-auto flex items-center gap-2">
            <button type="button" className={secondaryButton} onClick={onClose}>
              取消
            </button>
            <button type="button" className={primaryButton} disabled={!canSave} onClick={save}>
              {task ? "保存" : "创建"}
            </button>
          </div>
        </div>
      </div>
    </Modal>
  );
}

function DeleteConfirm({ task, onCancel, onDeleted }: { task: ScheduleTask; onCancel: () => void; onDeleted: () => void }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  return (
    <div className="flex w-full flex-col gap-3">
      <div className="text-[15px] font-semibold text-ink">删除这个定时任务？</div>
      <p className="break-words text-[12.5px] text-ink-4">{task.prompt}</p>
      {error && <p className="text-[12.5px] text-[#d70015]">{error}</p>}
      <div className="flex justify-end gap-2">
        <button type="button" className={secondaryButton} onClick={onCancel}>
          取消
        </button>
        <button
          type="button"
          disabled={busy}
          className="motion-colors flex h-8 items-center rounded-lg bg-[#ff3b30] px-3 text-[12.5px] font-medium text-white shadow-sm hover:bg-[#e0352b] disabled:opacity-50"
          onClick={() => {
            setBusy(true);
            scheduleDelete(task.id)
              .then(onDeleted)
              .catch((e) => {
                setError(errorText(e));
                setBusy(false);
              });
          }}
        >
          删除
        </button>
      </div>
    </div>
  );
}
