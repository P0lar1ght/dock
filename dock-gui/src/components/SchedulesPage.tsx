// 侧栏「定时任务」：按间隔向会话发提问（dock.1 `schedule/*`）。任务属于会话，落盘、跨重启，
// 7 天过期。照 wonder-ui/pages/PageSchedules*：按会话分组的列表（上次没发出去 / 由其他窗口运行
// 两种特殊行）、新建 / 编辑弹窗、空状态、删除确认。变动跟着 `schedule/changed` 推送重拉。

import { useEffect, useMemo, useState } from "react";
import { ChevronDown, Clock, Pencil, Plus, Trash2 } from "lucide-react";
import {
  intervalText,
  onSchedulesChanged,
  scheduleCreate,
  scheduleDelete,
  scheduleList,
  scheduleUpdate,
  type ScheduleTask,
} from "../dock/scheduleApi";
import { errorText } from "../dock/settingsApi";
import type { SessionSummary } from "../model";
import { Checkbox, dangerText, Modal, primaryButton, secondaryButton } from "./settings/ui";
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
        rows: rows.sort((a, b) => a.nextAtMs - b.nextAtMs),
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
    <NavPageShell title="定时任务" subtitle="按间隔向会话发送提问 · 保存在 ~/.dock/schedules.json" actions={newButton} insetLeft={insetLeft}>
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
        <EmptyCard icon={Clock} title="还没有定时任务" body="在对话里输入 /loop 10m 检查 CI，或点下面的按钮">
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
                <TaskLine key={t.id} task={t} now={now} onEdit={() => setEditing(t)} onDelete={() => setDeleting(t)} />
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

function TaskLine({ task, now, onEdit, onDelete }: { task: ScheduleTask; now: number; onEdit: () => void; onDelete: () => void }) {
  const left = task.expiresAtMs - now;
  const expiry = left <= 0 ? "已过期" : left < EXPIRY_WARN_MS ? `${Math.max(1, Math.round(left / 3_600_000))} 小时后过期` : `${Math.ceil(left / 86_400_000)} 天后过期`;
  return (
    <div className="flex flex-col">
      <div className="motion-colors group flex h-14 items-center gap-3 rounded-lg px-2 hover:bg-[#f7f7f8]">
        <Clock size={15} className="shrink-0 text-ink-4" />
        <div className="flex min-w-0 flex-1 flex-col gap-0.5">
          <span title={task.prompt} className="truncate text-[13px] font-medium text-ink">
            {task.prompt}
          </span>
          <span className="truncate text-xs text-ink-4">
            {intervalText(task.everySecs)} ·{" "}
            <span title={fullTime(task.nextAtMs)}>下次 {until(task.nextAtMs, now)}</span> ·{" "}
            <span title={fullTime(task.expiresAtMs)} className={left < EXPIRY_WARN_MS ? "text-[#c93400]" : ""}>
              {expiry}
            </span>
          </span>
        </div>
        {!task.heldHere && (
          <span
            title="另一个 Dock 进程持有它；那个进程退出后，这里会接管。"
            className="shrink-0 rounded-full bg-[#f2f2f7] px-2 py-0.5 text-[11px] font-medium text-[#636366]"
          >
            由其他窗口运行
          </span>
        )}
        {task.lastFiredAtMs !== null && (
          <span title={fullTime(task.lastFiredAtMs)} className="shrink-0 text-xs text-ink-4">
            上次 {ago(task.lastFiredAtMs, now)}
          </span>
        )}
        <div className="hidden shrink-0 items-center gap-1 group-hover:flex group-focus-within:flex">
          <LineButton label="编辑" onClick={onEdit}>
            <Pencil size={13} />
          </LineButton>
          <LineButton label="删除" onClick={onDelete} danger>
            <Trash2 size={13} />
          </LineButton>
        </div>
      </div>
      {task.lastError && (
        <div className="enter-fade mb-1 ml-9 mr-2 rounded-md bg-[#ff3b300f] px-2.5 py-1.5 text-xs text-[#d70015]">
          上次没发出去：{task.lastError}
        </div>
      )}
    </div>
  );
}

function LineButton({ label, onClick, danger, children }: { label: string; onClick: () => void; danger?: boolean; children: React.ReactNode }) {
  return (
    <button
      type="button"
      title={label}
      aria-label={label}
      onClick={onClick}
      className={`motion-colors flex size-7 items-center justify-center rounded-md border border-line-strong bg-white shadow-sm ${
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
  const initial = splitInterval(task?.everySecs ?? 1800);
  const [sessionId, setSessionId] = useState(task?.threadId ?? defaultSessionId ?? sessions[0]?.id ?? "");
  const [amount, setAmount] = useState(initial.amount);
  const [unit, setUnit] = useState(initial.unit);
  const [prompt, setPrompt] = useState(task?.prompt ?? "");
  const [runNow, setRunNow] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const everySecs = Math.round(Number(amount) * Number(unit));
  const tooShort = !Number.isFinite(everySecs) || everySecs < 60;
  const canSave = !busy && prompt.trim() !== "" && !tooShort && (task !== null || sessionId !== "");

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

  const save = async () => {
    setBusy(true);
    setError(null);
    try {
      if (task) await scheduleUpdate({ id: task.id, everySecs, prompt: prompt.trim() });
      else await scheduleCreate({ threadId: sessionId, everySecs, prompt: prompt.trim(), fireImmediately: runNow });
      onSaved();
    } catch (e) {
      setError(errorText(e));
      setBusy(false);
    }
  };

  return (
    <Modal onClose={onClose}>
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
          <span className="text-[12.5px] font-medium text-ink">间隔</span>
          <div className="flex items-center gap-2">
            <input
              type="number"
              min={1}
              value={amount}
              onChange={(e) => setAmount(e.target.value)}
              className={`motion-colors h-8 w-[88px] rounded-lg border bg-white px-2.5 text-[12.5px] text-ink shadow-sm outline-none focus:border-accent ${
                tooShort ? "border-[#ff3b30]" : "border-line-strong"
              } [appearance:textfield] [&::-webkit-inner-spin-button]:appearance-none`}
            />
            <span className="motion-colors relative flex h-8 items-center gap-2 rounded-lg border border-line-strong bg-white px-2.5 shadow-sm">
              <span className="text-[12.5px] text-ink">{UNITS.find((u) => u.value === unit)?.label}</span>
              <ChevronDown size={13} className="text-ink-4" />
              <select value={unit} onChange={(e) => setUnit(e.target.value)} className="absolute inset-0 cursor-pointer opacity-0">
                {UNITS.map((u) => (
                  <option key={u.value} value={u.value}>
                    {u.label}
                  </option>
                ))}
              </select>
            </span>
          </div>
          <span className={`text-[11px] ${tooShort ? "text-[#d70015]" : "text-ink-4"}`}>最短 1 分钟</span>
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
        {!task && (
          <label className="flex cursor-pointer items-center gap-2 text-[12.5px] text-ink">
            <Checkbox checked={runNow} onChange={setRunNow} label="创建后立即运行一次" />
            创建后立即运行一次
          </label>
        )}
        <p className="text-xs leading-relaxed text-ink-4">7 天后自动过期。Dock 没开着的时候不运行，错过的不补。</p>
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
