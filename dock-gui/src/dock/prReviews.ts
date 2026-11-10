// 每个 PR 最近一次 Agent review（code-review 工作流）的批注，按「项目路径 + PR 号」存在本机。
// run 只活在 Dock 内存里（重启就没了），PR 页的差异视图要在之后也能看到批注，所以收尾时存一份。

import type { ReviewFinding } from "dock-core";

export interface SavedPrReview {
  at: number;
  threadId: string;
  runId: string;
  diffCommand: string;
  findings: ReviewFinding[];
}

const KEY = (cwd: string, pr: number) => `dock.prReview:${cwd.replace(/\/+$/, "")}#${pr}`;
/** 存了新的一份：PR 页开着就跟着换。 */
export const PR_REVIEW_SAVED = "dock:pr-review-saved";

export function savePrReview(cwd: string, pr: number, review: SavedPrReview) {
  try {
    localStorage.setItem(KEY(cwd, pr), JSON.stringify(review));
  } catch {
    // 存不下（配额满）就算了：当次会话里的面板照样能看。
  }
  window.dispatchEvent(new CustomEvent(PR_REVIEW_SAVED, { detail: { cwd, pr } }));
}

export function loadPrReview(cwd: string, pr: number): SavedPrReview | null {
  try {
    const raw = localStorage.getItem(KEY(cwd, pr));
    const v = raw ? (JSON.parse(raw) as SavedPrReview) : null;
    return v && Array.isArray(v.findings) ? v : null;
  } catch {
    return null;
  }
}
