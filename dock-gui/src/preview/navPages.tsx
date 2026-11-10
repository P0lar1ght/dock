// 预览：侧栏整页（#page-<状态>）。dock.call 换成假数据，不连 Dock，
// 给无头浏览器截图对照 wonder-ui/pages/PagePR* 与 PageSchedules*。
// 弹窗 / 抽屉这类要点一下才出来的状态，渲染后按 DOM 点开。

import { useEffect } from "react";
import { dock } from "../dock/client";
import { PullRequestsPage } from "../components/PullRequestsPage";
import { SchedulesPage } from "../components/SchedulesPage";
import type { SessionSummary } from "../model";

const NOW = Date.now();
const MIN = 60_000;
const HOUR = 60 * MIN;
const DAY = 24 * HOUR;

const PRS = [
  {
    number: 168,
    title: "feat(gateway): 设置页写配置的方法与受信 ticket 门禁",
    url: "https://github.com/P0lar1ght/dock/pull/168",
    author: "bob",
    isDraft: false,
    headRefName: "feat/settings-config",
    baseRefName: "main",
    updatedAt: new Date(NOW - 25 * MIN).toISOString(),
    reviewDecision: "REVIEW_REQUIRED",
    mergeable: "MERGEABLE",
    checks: { state: "pending", total: 2, failed: 0, pending: 1 },
    mine: false,
    reviewRequested: true,
  },
  {
    number: 172,
    title: "feat(spine): 定时任务落盘、属于发起它的会话，跨重启继续跑",
    url: "https://github.com/P0lar1ght/dock/pull/172",
    author: "P0lar1ght",
    isDraft: false,
    headRefName: "feat/durable-schedules",
    baseRefName: "main",
    updatedAt: new Date(NOW - 2 * HOUR).toISOString(),
    reviewDecision: null,
    mergeable: "MERGEABLE",
    checks: { state: "success", total: 2, failed: 0, pending: 0 },
    mine: true,
    reviewRequested: false,
  },
  {
    number: 173,
    title: "feat(gateway): vcs/pr/list | get 经本机 gh 列出项目的 GitHub PR",
    url: "https://github.com/P0lar1ght/dock/pull/173",
    author: "P0lar1ght",
    isDraft: true,
    headRefName: "feat/gateway-pr-list",
    baseRefName: "main",
    updatedAt: new Date(NOW - DAY).toISOString(),
    reviewDecision: null,
    mergeable: "UNKNOWN",
    checks: { state: "none", total: 0, failed: 0, pending: 0 },
    mine: true,
    reviewRequested: false,
  },
  {
    number: 160,
    title: "fix(tui): 终端光标回滚后错位",
    url: "https://github.com/P0lar1ght/dock/pull/160",
    author: "alice",
    isDraft: false,
    headRefName: "fix/tui-cursor",
    baseRefName: "main",
    updatedAt: new Date(NOW - 3 * DAY).toISOString(),
    reviewDecision: "CHANGES_REQUESTED",
    mergeable: "MERGEABLE",
    checks: { state: "failure", total: 3, failed: 2, pending: 0 },
    mine: false,
    reviewRequested: false,
  },
];

const DETAIL = {
  ...PRS[1],
  body: "定时任务写入 ~/.dock/schedules.json。重启后继续跑，7 天后过期。到点时会话关着，会在后台打开再发。\n\n- 多个 Dock 共用时只有持有者触发\n- 停机期间错过的不补跑",
  state: "OPEN",
  mergeStateStatus: "CLEAN",
  additions: 1368,
  deletions: 156,
  changedFiles: 22,
  createdAt: new Date(NOW - 5 * HOUR).toISOString(),
  comments: 4,
  reviews: [],
  checksSummary: { state: "success", total: 2, failed: 0, pending: 0 },
  checks: [
    { name: "dock / test", state: "success", url: "https://github.com" },
    { name: "lint", state: "success", url: "https://github.com" },
  ],
};

const SESSIONS: SessionSummary[] = [
  { id: "s-review", projectId: "code:/Users/polar/Desktop/AILab/dock", title: "review代码并提交pr", preset: "编码", presetId: "code", cwd: "/Users/polar/Desktop/AILab/dock", open: true, subagentCount: 0, subagentsRunning: 0, updatedAt: NOW - HOUR },
  { id: "s-bili", projectId: "code:/Users/polar/Desktop/AILab/dock", title: "打开浏览器，搜下哔哩哔哩最近的热门视频并做成画布", preset: "编码", presetId: "code", cwd: "/Users/polar/Desktop/AILab/dock", open: false, subagentCount: 0, subagentsRunning: 0, updatedAt: NOW - 3 * HOUR },
];

const task = (id: string, threadId: string, prompt: string, everySecs: number, next: number, last: number | null, extra = {}) => ({
  id,
  threadId,
  threadTitle: null,
  cwd: "/Users/polar/Desktop/AILab/dock",
  prompt,
  everySecs,
  intervalLabel: "",
  createdAtMs: NOW - HOUR,
  nextAtMs: NOW + next,
  expiresAtMs: NOW + 7 * DAY - HOUR,
  lastFiredAtMs: last === null ? null : NOW - last,
  lastError: null,
  heldHere: true,
  ...extra,
});
const TASKS = [
  task("cron-1", "s-review", "检查 CI 是否全绿，有失败就总结原因", 1800, 12 * MIN, 7 * MIN),
  task("cron-2", "s-review", "看一下 main 有没有新提交需要 rebase", 7200, 40 * MIN, 2 * HOUR, { lastError: "会话打不开（已删除？）" }),
  task("cron-3", "s-bili", "刷新排行榜并更新画布", 86400, 20 * HOUR, null, { heldHere: false, expiresAtMs: NOW + 6 * DAY }),
];

export function isNavPagePreview() {
  return location.hash.startsWith("#page-");
}

export function NavPagePreview() {
  const state = location.hash.slice("#page-".length);
  (dock as unknown as { call: (m: string, p?: Record<string, unknown>) => Promise<unknown> }).call = async (method) => {
    const ok = (v: unknown) => new Promise((r) => setTimeout(() => r(v), 20));
    if (method === "vcs/pr/list") {
      if (state === "pr-loading") return new Promise(() => {});
      if (state === "pr-nogh") return ok({ available: false, reason: "gh_missing", hint: "" });
      if (state === "pr-unauth") return ok({ available: false, reason: "gh_unauthenticated", hint: "" });
      if (state === "pr-notgithub") return ok({ available: false, reason: "not_github", hint: "" });
      if (state === "pr-error") return ok({ available: false, reason: "gh_failed", hint: "gh 执行失败：HTTP 502" });
      return ok({
        available: true,
        repo: "P0lar1ght/dock",
        viewer: "P0lar1ght",
        prs: state === "pr-empty" ? [] : PRS,
        fetchedAtMs: NOW - 2 * MIN,
        cached: true,
      });
    }
    if (method === "vcs/pr/get") return ok({ available: true, pr: DETAIL });
    if (method === "schedule/list") return ok({ tasks: state === "sched-empty" ? [] : TASKS });
    return ok({});
  };
  useEffect(() => {
    // 要点开的状态：等列表渲染出来再点。
    const click = (selector: string, nth = 0) =>
      setTimeout(() => (document.querySelectorAll<HTMLElement>(selector)[nth] ?? null)?.click(), 200);
    if (state === "pr-detail") click('[role="button"]', 1);
    if (state === "sched-new") click("button.bg-ink");
    if (state === "sched-edit") click('button[aria-label="编辑"]', 1);
    if (state === "sched-delete") click('button[aria-label="删除"]', 1);
  }, [state]);
  const projects = [
    { name: "dock", path: "/Users/polar/Desktop/AILab/dock" },
    { name: "nova-gateway", path: "/tmp/nova-gateway" },
  ];
  return (
    <div className="relative h-[900px] w-[1192px] overflow-hidden bg-white">
      {state.startsWith("pr-") ? (
        <PullRequestsPage projects={projects} initialPath={projects[0].path} onAgentReview={() => {}} />
      ) : (
        <SchedulesPage sessions={SESSIONS} onOpenSession={() => {}} initialSessionId="s-review" />
      )}
    </div>
  );
}
