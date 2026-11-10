// 定时任务（dock.1 `schedule/*`，见网关 handlers/schedule.rs）。
// 任务落盘在 $DOCK_HOME/schedules.json，属于某个会话，跨重启继续跑，7 天过期。
// 变动推 `schedule/changed`（连接级，不用订阅线程），收到后重拉列表。

import { dock } from "./client";

export interface ScheduleTask {
  id: string;
  /** 所属会话 id；老任务没有。 */
  threadId: string | null;
  threadTitle: string | null;
  cwd: string | null;
  prompt: string;
  everySecs: number;
  /** Dock 给的英文间隔说明（「every 10 minutes」）；界面用 `intervalText` 自己拼中文。 */
  intervalLabel: string;
  createdAtMs: number;
  nextAtMs: number;
  expiresAtMs: number;
  lastFiredAtMs: number | null;
  /** 上次没送到的原因；下次送到就清掉。 */
  lastError: string | null;
  /** 这个 Dock 进程持有它；false = 另一个 Dock 进程在跑它。 */
  heldHere: boolean;
}

export const scheduleList = () => dock.call<{ tasks: ScheduleTask[] }>("schedule/list").then((r) => r.tasks);

export const scheduleCreate = (p: { threadId: string; everySecs: number; prompt: string; fireImmediately?: boolean }) =>
  dock.call<{ task: ScheduleTask }>("schedule/create", p).then((r) => r.task);

export const scheduleUpdate = (p: { id: string; everySecs?: number; prompt?: string }) =>
  dock.call<{ task: ScheduleTask }>("schedule/update", p).then((r) => r.task);

export const scheduleDelete = (id: string) => dock.call<{ deleted: string }>("schedule/delete", { id });

/** 定时任务变了（任何一处：本页、模型的 scheduler_*、别的 Dock、到点触发）。返回取消函数。 */
export function onSchedulesChanged(fn: () => void): () => void {
  return dock.onNotification((n) => {
    if (n.method === "schedule/changed") fn();
  });
}

/** 「每 10 分钟」「每 2 小时」「每 1 天」。 */
export function intervalText(secs: number): string {
  if (secs % 86_400 === 0) return `每 ${secs / 86_400} 天`;
  if (secs % 3_600 === 0) return `每 ${secs / 3_600} 小时`;
  if (secs % 60 === 0) return `每 ${secs / 60} 分钟`;
  return `每 ${secs} 秒`;
}
