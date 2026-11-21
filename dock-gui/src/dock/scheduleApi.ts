// 定时任务（dock.1 `schedule/*`，见网关 handlers/schedule.rs）。
// 任务落盘在 $DOCK_HOME/schedules.json，属于某个会话，跨重启继续跑。
// 触发方式六选一：固定间隔、cron（本机时区）、只跑一次、条件命令、文件变化、只手动。
// 比一天还勤的默认 7 天过期。每次可以送进原会话，也可以在同一个项目里新开会话。
// 变动推 `schedule/changed`（连接级，不用订阅线程），收到后重拉列表。

import { dock } from "./client";

export type WatchWhen = "changed" | "succeeds" | "fails" | "matches";

export type ScheduleSpec =
  | { kind: "every"; everySecs: number }
  | { kind: "cron"; cron: string }
  | { kind: "at"; atMs: number }
  | { kind: "watch"; command: string; everySecs: number; when: WatchWhen; pattern: string | null }
  | { kind: "files"; paths: string[]; debounceSecs: number }
  | { kind: "manual" };

export type FireReason = "schedule" | "catchUp" | "immediate" | "manual" | "watch" | "files";

export interface ScheduleRun {
  atMs: number;
  reason: FireReason;
  /** sent = 送进了会话；coalesced = 上一次还没跑完，这次跳过；failed = 没送到。 */
  outcome: "sent" | "coalesced" | "failed";
  /** 送进了哪个会话（新开会话时是新的那个）。 */
  threadId: string | null;
  lateMs: number | null;
  error: string | null;
}

export interface ScheduleTask {
  id: string;
  /** 所属会话 id；老任务没有。 */
  threadId: string | null;
  threadTitle: string | null;
  cwd: string | null;
  prompt: string;
  schedule: ScheduleSpec;
  /** Dock 给的中文说明：「每 30 分钟」「工作日 09:00」「一次 · 11月23日 09:00」。 */
  scheduleLabel: string;
  /** 固定间隔才有；cron / 一次性是 null。 */
  everySecs: number | null;
  /** 给模型看的英文说明。 */
  intervalLabel: string;
  createdAtMs: number;
  /** null = 跑完了（一次性 / 跑满次数）；还留在表上就是最后一次没送到。 */
  nextAtMs: number | null;
  /** null = 不过期。 */
  expiresAtMs: number | null;
  lastFiredAtMs: number | null;
  /** 上次没送到的原因；下次送到就清掉。 */
  lastError: string | null;
  /** 自动触发过几次（手动运行不算）。 */
  runs: number;
  maxRuns: number | null;
  /** 错过了（Dock 没开 / 电脑睡着）回来后补跑一次。 */
  catchUp: boolean;
  /** session = 送进这个会话；newSession = 每次在同一个项目里新开一个会话。 */
  target: "session" | "newSession";
  /** 新开会话时用的预设。 */
  preset: string | null;
  paused: boolean;
  /** 有一次等着马上跑（手动运行 / 条件满足 / 文件改了）。 */
  pending: boolean;
  finished: boolean;
  /** 最近的运行，新的在前。 */
  history: ScheduleRun[];
  /** 条件命令上一次检查的结果。 */
  watch: { checkedAtMs: number; exit: number | null; matched: boolean; error: string | null } | null;
  /** 这个 Dock 进程持有它；false = 另一个 Dock 进程在跑它。 */
  heldHere: boolean;
  /** 有活着的 Dock 持有它（本进程或别的）；false = 现在没有 Dock 会去跑它。 */
  holderLive: boolean;
}

/** 触发方式，只给一个。 */
export type ScheduleInput =
  | { everySecs: number }
  | { cron: string }
  | { atMs: number }
  | { watch: { command: string; everySecs: number; when: WatchWhen; pattern?: string } }
  | { files: { paths: string[]; debounceSecs: number } }
  | { manual: true };

type TaskOptions = {
  /** 省略 = 默认（比一天还勤的 7 天后过期）；null = 不过期。 */
  expiresAtMs?: number | null;
  /** null = 不限次数。 */
  maxRuns?: number | null;
  catchUp?: boolean;
  target?: "session" | "newSession";
  paused?: boolean;
};

export const scheduleList = () => dock.call<{ tasks: ScheduleTask[] }>("schedule/list").then((r) => r.tasks);

export const scheduleCreate = (p: { threadId: string; prompt: string; fireImmediately?: boolean } & TaskOptions & ScheduleInput) =>
  dock.call<{ task: ScheduleTask }>("schedule/create", p).then((r) => r.task);

export const scheduleUpdate = (p: { id: string; prompt?: string } & TaskOptions & (ScheduleInput | Record<never, never>)) =>
  dock.call<{ task: ScheduleTask }>("schedule/update", p).then((r) => r.task);

export const scheduleDelete = (id: string) => dock.call<{ deleted: string }>("schedule/delete", { id });

/** 马上跑一次（不占次数、不动排期）；持有它的 Dock 一秒内送出。 */
export const scheduleRun = (id: string, input?: string) => dock.call<{ ok: boolean }>("schedule/run", input ? { id, input } : { id });

/** 定时任务变了（任何一处：本页、模型的 scheduler_*、别的 Dock、到点触发）。返回取消函数。 */
export function onSchedulesChanged(fn: () => void): () => void {
  return dock.onNotification((n) => {
    if (n.method === "schedule/changed") fn();
  });
}
