// 项目的 GitHub Pull Request（dock.1 `vcs/pr/*`，见网关 handlers/vcs.rs）。只读，经本机 gh。
// gh 用不了不是错误：回 `available: false` + `reason` / `hint`，页面画空状态。

import { dock } from "./client";

export type PrUnavailableReason = "gh_missing" | "gh_unauthenticated" | "not_git" | "not_github" | "gh_failed";

export interface PrUnavailable {
  available: false;
  reason: PrUnavailableReason;
  hint: string;
}

export interface PrChecks {
  state: "success" | "failure" | "pending" | "none";
  total: number;
  failed: number;
  pending: number;
}

export interface PrRow {
  number: number;
  title: string;
  url: string;
  author: string;
  isDraft: boolean;
  headRefName: string;
  baseRefName: string;
  updatedAt: string;
  /** APPROVED / CHANGES_REQUESTED / REVIEW_REQUIRED；没有为 null。 */
  reviewDecision: string | null;
  /** MERGEABLE / CONFLICTING / UNKNOWN。 */
  mergeable: string | null;
  checks: PrChecks;
  mine: boolean;
  reviewRequested: boolean;
}

export interface PrList {
  available: true;
  repo: string;
  viewer: string;
  prs: PrRow[];
  /** 什么时候从 GitHub 拿的。 */
  fetchedAtMs: number;
  /** 这次是网关的缓存（列表 60 秒内不重问 GitHub）。 */
  cached: boolean;
}

export interface PrDetail extends Omit<PrRow, "checks" | "mine" | "reviewRequested"> {
  body: string;
  state: string;
  mergeStateStatus: string | null;
  additions: number;
  deletions: number;
  changedFiles: number;
  createdAt: string;
  comments: number;
  reviews: { author: string; state: string; submittedAt: string }[];
  checksSummary: PrChecks;
  checks: { name: string; state: "success" | "failure" | "pending" | "skipped"; url: string | null }[];
}

/**
 * 项目：`cwd` 必须是 Dock 认识的（侧栏「项目」里的项目路径）。
 * 网关有缓存：列表 60 秒内直接回；`force` 绕过（「刷新」按钮）。
 */
export const prList = (cwd: string, force = false) =>
  dock.call<PrList | PrUnavailable>("vcs/pr/list", { cwd, force }).then((r) => {
    if (r.available) lastLists.set(cwd, r);
    return r;
  });

/** 详情：列表里这个 PR 没变（updatedAt + 检查汇总）时网关直接回缓存。 */
export const prGet = (cwd: string, number: number) =>
  dock.call<{ available: true; pr: PrDetail; cached?: boolean } | PrUnavailable>("vcs/pr/get", { cwd, number });

/** 一个 PR 的统一 diff（`gh pr diff`），超过 4MB 截断。 */
export const prDiff = (cwd: string, number: number) =>
  dock.call<{ available: true; diff: string; truncated: boolean } | PrUnavailable>("vcs/pr/diff", { cwd, number });

/** 每个项目上一次拿到的列表：再进页面先画它，后台再刷新。 */
const lastLists = new Map<string, PrList>();
export const cachedPrList = (cwd: string) => lastLists.get(cwd) ?? null;

/** 预取（悬停侧栏「Pull Request」、悬停某一行）：只为了让网关缓存热起来，结果和错误都不管。 */
export const prefetchPrList = (cwd: string) => void prList(cwd).catch(() => {});
export const prefetchPr = (cwd: string, number: number) => void prGet(cwd, number).catch(() => {});
