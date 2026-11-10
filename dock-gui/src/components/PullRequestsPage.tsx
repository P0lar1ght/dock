// 侧栏「Pull Request」：项目开着的 GitHub PR（dock.1 `vcs/pr/*`，经本机 gh，只读）。
// 照 wonder-ui/pages/PagePR*：列表三组（请我 review / 我的 / 其他）、右侧详情抽屉、
// gh 用不了时的空状态卡（没装 / 没登录 / 没有 GitHub 远端 / 没有开着的 PR）、骨架屏、出错条。

import { useEffect, useMemo, useState } from "react";
import {
  ArrowLeft,
  ArrowRight,
  Bot,
  FileDiff,
  ChevronDown,
  CircleCheck,
  CircleDashed,
  CircleX,
  Copy,
  ExternalLink,
  FolderGit2,
  GitPullRequest,
  KeyRound,
  RefreshCw,
  Terminal,
  X,
} from "lucide-react";
import {
  cachedPrList,
  prDiff,
  prefetchPr,
  prGet,
  prList,
  type PrChecks,
  type PrDetail,
  type PrList,
  type PrRow,
  type PrUnavailable,
} from "../dock/prApi";
import { errorText } from "../dock/settingsApi";
import {
  loadPrReview,
  PR_REVIEW_SAVED,
  type SavedPrReview,
} from "../dock/prReviews";
import { ReviewDiff } from "./review/ReviewDiff";
import { Markdown } from "./Markdown";
import { Pill, primaryButton, secondaryButton } from "./settings/ui";
import { MenuButton } from "./Popover";
import {
  ago,
  EmptyCard,
  GroupHeading,
  NavPageShell,
  openExternal,
} from "./navPage";

export interface PrProject {
  name: string;
  path: string;
}

type Loaded =
  | { kind: "loading" }
  | { kind: "error"; message: string }
  | { kind: "ok"; data: PrList | PrUnavailable };

/**
 * 「让 Agent review」发给新会话的那句话：跑内置的 code-review 工作流（多角度审查 + 逐条核实），
 * 批注会落到 PR 页的差异视图上。只读：不改代码、不评论、不合并。
 */
export function reviewPrompt(pr: {
  number: number;
  title: string;
  url: string;
}): string {
  return [
    `请 review 这个 Pull Request：#${pr.number} ${pr.title}`,
    pr.url,
    "",
    `用 workflow 工具跑内置的 code-review 工作流：source 用 type=name、name=code-review，args 给 {"pr": ${pr.number}}。`,
    "启动后不用轮询，跑完后用几句话总结最要紧的问题。",
    "只读：不要改代码，也不要在 GitHub 上评论或合并。",
  ].join("\n");
}

export function PullRequestsPage({
  projects,
  initialPath,
  onAgentReview,
  onChangeProject,
  insetLeft,
}: {
  insetLeft?: boolean;
  projects: PrProject[];
  initialPath: string | null;
  /** 用编码预设在这个项目开新会话，把 review 请求发进去。 */
  onAgentReview: (cwd: string, prompt: string) => void;
  onChangeProject?: (path: string) => void;
}) {
  const [path, setPath] = useState(
    () => initialPath ?? projects[0]?.path ?? "",
  );
  // 上次拿到过就先画上次的（不闪骨架），后台再刷新。
  const fromCache = (p: string): Loaded => {
    const hit = cachedPrList(p);
    return hit ? { kind: "ok", data: hit } : { kind: "loading" };
  };
  const [state, setState] = useState<Loaded>(() => fromCache(path));
  const [open, setOpen] = useState<number | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  // 「刷新」按钮：绕过网关缓存。
  const [forced, setForced] = useState(0);

  useEffect(() => {
    if (!path) return;
    let live = true;
    setRefreshing(true);
    prList(path, forced > 0)
      .then((data) => live && setState({ kind: "ok", data }))
      .catch(
        (e) =>
          live &&
          setState((s) =>
            s.kind === "ok" ? s : { kind: "error", message: errorText(e) },
          ),
      )
      .finally(() => live && setRefreshing(false));
    return () => {
      live = false;
    };
  }, [path, forced]);

  const reload = () => setForced((n) => n + 1);
  const pick = (p: string) => {
    setOpen(null);
    setState(fromCache(p));
    setForced(0);
    setPath(p);
    onChangeProject?.(p);
  };
  const project = projects.find((p) => p.path === path);
  const list = state.kind === "ok" && state.data.available ? state.data : null;

  const actions = (
    <>
      {projects.length > 0 && (
        <MenuButton
          title="切换项目"
          side="bottom"
          align="right"
          value={path}
          onPick={pick}
          options={projects.map((p) => ({
            id: p.path,
            label: p.name,
            hint: p.path,
          }))}
          trigger={
            <span className={`${secondaryButton} max-w-[220px]`}>
              <span className="truncate">{project?.name ?? "选择项目"}</span>
              <ChevronDown size={13} className="shrink-0 text-ink-4" />
            </span>
          }
        />
      )}
      <button
        type="button"
        onClick={reload}
        className={secondaryButton}
        disabled={!path}
      >
        <RefreshCw size={13} className={refreshing ? "animate-spin" : ""} />
        刷新
      </button>
    </>
  );

  return (
    <NavPageShell
      title="Pull Request"
      subtitle="经本机 gh 读取，只读"
      actions={actions}
      insetLeft={insetLeft}
    >
      {!path ? (
        <EmptyCard
          icon={FolderGit2}
          title="还没有项目"
          body="先在侧栏「项目」里建一个项目，这里才知道去哪个仓库看。"
        />
      ) : state.kind === "loading" ? (
        <Skeleton />
      ) : state.kind === "error" ? (
        <div className="flex flex-col px-8 pt-4">
          <ErrorBar message={state.message} onRetry={reload} />
        </div>
      ) : !state.data.available ? (
        <Unavailable
          data={state.data}
          path={path}
          onRetry={reload}
          onPickOther={
            projects.length > 1
              ? () => pickOther(projects, path, pick)
              : undefined
          }
        />
      ) : (
        <div className="relative flex min-h-0 flex-1 flex-col">
          <div className="shrink-0 px-8 pt-3 text-xs text-ink-4">
            仓库 {state.data.repo} · 我是 {state.data.viewer || "（未知）"}
            {state.data.fetchedAtMs > 0 && (
              <span
                title={new Date(state.data.fetchedAtMs).toLocaleString("zh-CN")}
              >
                {" "}
                ·{" "}
                {refreshing
                  ? "正在刷新…"
                  : `${ago(state.data.fetchedAtMs)}更新`}
              </span>
            )}
          </div>
          {state.data.prs.length === 0 ? (
            <EmptyCard
              icon={GitPullRequest}
              title="没有开着的 Pull Request"
              body="这个仓库目前没有开放的 PR。"
            >
              <button
                type="button"
                className={`${primaryButton} mt-1`}
                onClick={() =>
                  openExternal(
                    `https://github.com/${state.data.available && state.data.repo}`,
                  )
                }
              >
                在浏览器打开仓库
              </button>
            </EmptyCard>
          ) : (
            <PrGroups
              onHover={(n) => prefetchPr(path, n)}
              prs={state.data.prs}
              selected={open}
              onOpen={setOpen}
              onReview={(pr) => onAgentReview(path, reviewPrompt(pr))}
            />
          )}
          {open !== null && list && (
            <Drawer
              cwd={path}
              number={open}
              row={list.prs.find((p) => p.number === open) ?? null}
              onClose={() => setOpen(null)}
              onReview={(pr) => onAgentReview(path, reviewPrompt(pr))}
            />
          )}
        </div>
      )}
    </NavPageShell>
  );
}

/** 「换个项目」：切到下一个项目。 */
function pickOther(
  projects: PrProject[],
  path: string,
  pick: (p: string) => void,
) {
  const i = projects.findIndex((p) => p.path === path);
  pick(projects[(i + 1) % projects.length].path);
}

function PrGroups({
  prs,
  selected,
  onOpen,
  onReview,
  onHover,
}: {
  /** 指针在某一行上停一会儿：预取它的详情。 */
  onHover: (n: number) => void;
  prs: PrRow[];
  selected: number | null;
  onOpen: (n: number) => void;
  onReview: (pr: PrRow) => void;
}) {
  const groups = useMemo(() => {
    const requested = prs.filter((p) => p.reviewRequested);
    const mine = prs.filter((p) => p.mine && !p.reviewRequested);
    const others = prs.filter((p) => !p.mine && !p.reviewRequested);
    return [
      { title: "请我 review", rows: requested },
      { title: "我的", rows: mine },
      { title: "其他", rows: others },
    ].filter((g) => g.rows.length > 0);
  }, [prs]);
  return (
    <div className="thin-scroll flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto px-8 py-4">
      {groups.map((g) => (
        <section key={g.title} className="flex flex-col gap-1">
          <GroupHeading title={g.title} count={g.rows.length} />
          {g.rows.map((pr) => (
            <PrLine
              key={pr.number}
              pr={pr}
              selected={pr.number === selected}
              onOpen={() => onOpen(pr.number)}
              onReview={() => onReview(pr)}
              onHover={() => onHover(pr.number)}
            />
          ))}
        </section>
      ))}
    </div>
  );
}

function PrLine({
  pr,
  selected,
  onOpen,
  onReview,
  onHover,
}: {
  pr: PrRow;
  selected: boolean;
  onOpen: () => void;
  onReview: () => void;
  onHover: () => void;
}) {
  const updated = Date.parse(pr.updatedAt);
  // 停 150ms 才预取：扫过去的不算。
  const [timer, setTimer] = useState<number | null>(null);
  return (
    <div
      role="button"
      tabIndex={0}
      onMouseEnter={() => setTimer(window.setTimeout(onHover, 150))}
      onMouseLeave={() => {
        if (timer !== null) window.clearTimeout(timer);
        setTimer(null);
      }}
      onClick={onOpen}
      onKeyDown={(e) => e.key === "Enter" && onOpen()}
      className={`motion-colors group flex h-14 cursor-pointer items-center gap-3 rounded-lg px-2 ${selected ? "bg-[#0a84ff0f]" : "hover:bg-[#f7f7f8]"}`}
    >
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        <div className="flex min-w-0 items-center gap-2">
          <span className="shrink-0 font-mono text-xs text-ink-4">
            #{pr.number}
          </span>
          <span className="truncate text-[13px] font-medium text-ink">
            {pr.title}
          </span>
        </div>
        <span className="truncate text-xs text-ink-4">
          {pr.author} · {pr.headRefName} → {pr.baseRefName}
          {Number.isFinite(updated) && (
            <span title={new Date(updated).toLocaleString("zh-CN")}>
              {" "}
              · 更新于 {ago(updated)}
            </span>
          )}
        </span>
      </div>
      <div className="flex shrink-0 items-center gap-1.5">
        {pr.isDraft && <Pill>草稿</Pill>}
        <ChecksPill checks={pr.checks} />
        <ReviewPill decision={pr.reviewDecision} />
        {!pr.isDraft && pr.mergeable === "CONFLICTING" && (
          <Pill kind="red">有冲突</Pill>
        )}
        {!pr.isDraft &&
          pr.mergeable === "MERGEABLE" &&
          pr.checks.state === "success" &&
          !pr.reviewDecision && <Pill kind="green">可合并</Pill>}
      </div>
      <div className="hidden shrink-0 items-center gap-1 group-hover:flex group-focus-within:flex">
        <RowIcon
          icon={ExternalLink}
          label="在浏览器打开"
          onClick={() => openExternal(pr.url)}
        />
        <RowIcon icon={Bot} label="让 Agent review" onClick={onReview} />
      </div>
    </div>
  );
}

function RowIcon({
  icon: Icon,
  label,
  onClick,
}: {
  icon: typeof Bot;
  label: string;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      title={label}
      aria-label={label}
      onClick={(e) => {
        e.stopPropagation();
        onClick();
      }}
      className="motion-colors flex size-7 items-center justify-center rounded-md border border-line-strong bg-white text-ink-3 shadow-sm hover:text-ink"
    >
      <Icon size={13} />
    </button>
  );
}

function ChecksPill({ checks }: { checks: PrChecks }) {
  if (checks.state === "success") return <Pill kind="green">检查通过</Pill>;
  if (checks.state === "failure")
    return <Pill kind="red">{checks.failed} 项失败</Pill>;
  if (checks.state === "pending") return <Pill kind="orange">进行中</Pill>;
  return <Pill>无检查</Pill>;
}

function ReviewPill({ decision }: { decision: string | null }) {
  if (decision === "APPROVED") return <Pill kind="green">已批准</Pill>;
  if (decision === "CHANGES_REQUESTED") return <Pill kind="red">需要修改</Pill>;
  if (decision === "REVIEW_REQUIRED") return <Pill kind="blue">待 review</Pill>;
  return null;
}

function Drawer({
  cwd,
  number,
  row,
  onClose,
  onReview,
}: {
  cwd: string;
  number: number;
  row: PrRow | null;
  onClose: () => void;
  onReview: (pr: { number: number; title: string; url: string }) => void;
}) {
  const [detail, setDetail] = useState<{ pr?: PrDetail; error?: string }>({});
  const [expanded, setExpanded] = useState(false);
  const [view, setView] = useState<"details" | "diff">("details");
  const review = useSavedReview(cwd, number);
  useEffect(() => {
    let live = true;
    setDetail({});
    setExpanded(false);
    setView("details");
    prGet(cwd, number)
      .then(
        (r) =>
          live && setDetail(r.available ? { pr: r.pr } : { error: r.hint }),
      )
      .catch((e) => live && setDetail({ error: errorText(e) }));
    return () => {
      live = false;
    };
  }, [cwd, number]);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);
  const pr = detail.pr;
  const head = pr ?? row;
  return (
    <>
      <div
        className="enter-fade absolute inset-0 z-10 bg-[#1c1c1e0d]"
        onMouseDown={onClose}
      />
      <aside
        className={`glass-dialog slide-in-right absolute bottom-0 right-0 top-0 z-20 flex flex-col border-l border-[#e5e5ea] bg-white shadow-[-16px_0px_40px_rgba(0,0,0,0.08)] transition-[width] duration-300 ease-out ${
          view === "diff" ? "w-[min(1080px,calc(100%-40px))]" : "w-[440px]"
        }`}
      >
        <div className="flex shrink-0 items-start gap-3 border-b border-[#ececf0] px-5 pb-4 pt-5">
          <div className="flex min-w-0 flex-1 flex-col gap-1">
            <span className="font-mono text-xs text-ink-4">#{number}</span>
            <span className="text-base font-semibold leading-snug text-ink">
              {head?.title ?? "…"}
            </span>
          </div>
          <RowIcon icon={X} label="关闭" onClick={onClose} />
        </div>
        {view === "diff" ? (
          <PrDiffView
            cwd={cwd}
            number={number}
            review={review}
            onBack={() => setView("details")}
          />
        ) : (
          <div className="thin-scroll flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto px-5 py-4">
            {detail.error ? (
              <ErrorBar message={detail.error} />
            ) : !pr ? (
              <div className="flex flex-col gap-3">
                {[0, 1, 2].map((i) => (
                  <div
                    key={i}
                    className="skeleton h-4 rounded"
                    style={{ width: `${80 - i * 15}%` }}
                  />
                ))}
              </div>
            ) : (
              <>
                <div className="flex flex-wrap items-center gap-1.5">
                  {pr.isDraft ? (
                    <Pill>草稿</Pill>
                  ) : (
                    <Pill kind="green">
                      {pr.state === "OPEN" ? "开放" : pr.state}
                    </Pill>
                  )}
                  <MergePill pr={pr} />
                  <ReviewPill decision={pr.reviewDecision} />
                </div>
                <div className="flex items-center gap-1.5 font-mono text-xs text-[#3a3a3c]">
                  {pr.headRefName}
                  <ArrowRight size={11} className="text-ink-4" />
                  {pr.baseRefName}
                </div>
                <div className="text-[12.5px] text-[#636366]">
                  <span className="text-[#248a3d]">+{pr.additions}</span>{" "}
                  <span className="text-[#d70015]">−{pr.deletions}</span> ·{" "}
                  {pr.changedFiles} 个文件 · {pr.comments} 条评论
                </div>
                <section className="flex flex-col gap-2">
                  <span className="text-xs font-semibold text-ink-4">改动</span>
                  <button
                    type="button"
                    onClick={() => setView("diff")}
                    className="motion-colors flex h-9 items-center gap-2 rounded-lg border border-[#e5e5ea] px-3 text-left text-[13px] text-ink hover:bg-hover"
                  >
                    <FileDiff size={14} className="shrink-0 text-[#636366]" />
                    <span className="flex-1">查看改动</span>
                    {review ? (
                      <span className="text-xs text-ink-4">
                        {review.findings.length} 条 Agent 批注 ·{" "}
                        {ago(review.at)}
                      </span>
                    ) : (
                      <span className="text-xs text-ink-4">
                        还没有 Agent 批注
                      </span>
                    )}
                    <ArrowRight size={12} className="shrink-0 text-ink-4" />
                  </button>
                </section>
                <section className="flex flex-col gap-2">
                  <span className="text-xs font-semibold text-ink-4">检查</span>
                  {pr.checks.length === 0 ? (
                    <span className="text-[12.5px] text-ink-4">没有检查</span>
                  ) : (
                    pr.checks.map((c, i) => (
                      <div
                        key={`${c.name}-${i}`}
                        className="flex h-8 items-center gap-2"
                      >
                        <CheckIcon state={c.state} />
                        <span className="min-w-0 flex-1 truncate text-[13px] text-ink">
                          {c.name || "（未命名）"}
                        </span>
                        {c.url && (
                          <button
                            type="button"
                            onClick={() => openExternal(c.url!)}
                            className="text-xs text-accent hover:underline"
                          >
                            查看
                          </button>
                        )}
                      </div>
                    ))
                  )}
                </section>
                <section className="flex flex-col gap-2">
                  <span className="text-xs font-semibold text-ink-4">
                    Review
                  </span>
                  {pr.reviews.length === 0 ? (
                    <span className="text-[12.5px] text-ink-4">
                      还没有人 review
                    </span>
                  ) : (
                    pr.reviews.map((r) => (
                      <div
                        key={r.author}
                        className="flex h-8 items-center gap-2"
                      >
                        <span className="flex size-6 items-center justify-center rounded-full bg-[#f2f2f7] text-[11px] font-semibold text-ink-3">
                          {r.author.slice(0, 1).toUpperCase()}
                        </span>
                        <span className="min-w-0 flex-1 truncate text-[13px] text-ink">
                          {r.author}
                        </span>
                        <ReviewState state={r.state} />
                        {r.submittedAt &&
                          Number.isFinite(Date.parse(r.submittedAt)) && (
                            <span className="text-xs text-ink-4">
                              {ago(Date.parse(r.submittedAt))}
                            </span>
                          )}
                      </div>
                    ))
                  )}
                </section>
                <section className="flex flex-col gap-2">
                  <span className="text-xs font-semibold text-ink-4">描述</span>
                  {pr.body.trim() ? (
                    <>
                      <div
                        className={`relative text-[13px] leading-relaxed ${expanded ? "" : "max-h-[16.5em] overflow-hidden"}`}
                      >
                        <Markdown text={pr.body} size="card" />
                      </div>
                      {!expanded && pr.body.split("\n").length > 12 && (
                        <button
                          type="button"
                          onClick={() => setExpanded(true)}
                          className="flex w-fit items-center gap-1 text-xs font-medium text-accent"
                        >
                          展开 <ChevronDown size={12} />
                        </button>
                      )}
                    </>
                  ) : (
                    <span className="text-[12.5px] text-ink-4">没有描述</span>
                  )}
                </section>
              </>
            )}
          </div>
        )}
        <div className="flex shrink-0 items-center gap-2 border-t border-[#ececf0] px-5 py-4">
          <button
            type="button"
            className={`${primaryButton} flex-1 justify-center`}
            disabled={!head}
            onClick={() =>
              head && onReview({ number, title: head.title, url: head.url })
            }
          >
            <Bot size={13} />让 Agent review
          </button>
          <button
            type="button"
            className={secondaryButton}
            disabled={!head}
            onClick={() => head && openExternal(head.url)}
          >
            <ExternalLink size={13} />
            在浏览器打开
          </button>
        </div>
      </aside>
    </>
  );
}

/** 这个 PR 存下的最近一次 Agent review；审完存了新的一份就跟着换。 */
function useSavedReview(cwd: string, number: number): SavedPrReview | null {
  const [review, setReview] = useState(() => loadPrReview(cwd, number));
  useEffect(() => {
    setReview(loadPrReview(cwd, number));
    const on = (e: Event) => {
      const d = (e as CustomEvent<{ cwd: string; pr: number }>).detail;
      if (
        d.pr === number &&
        d.cwd.replace(/\/+$/, "") === cwd.replace(/\/+$/, "")
      )
        setReview(loadPrReview(cwd, number));
    };
    window.addEventListener(PR_REVIEW_SAVED, on);
    return () => window.removeEventListener(PR_REVIEW_SAVED, on);
  }, [cwd, number]);
  return review;
}

/** 抽屉展开成宽的：PR 的改动，Agent review 的批注落在行上。 */
function PrDiffView({
  cwd,
  number,
  review,
  onBack,
}: {
  cwd: string;
  number: number;
  review: SavedPrReview | null;
  onBack: () => void;
}) {
  const [diff, setDiff] = useState<{
    text?: string;
    truncated?: boolean;
    error?: string;
  }>({});
  useEffect(() => {
    let live = true;
    setDiff({});
    prDiff(cwd, number)
      .then(
        (r) =>
          live &&
          setDiff(
            r.available
              ? { text: r.diff, truncated: r.truncated }
              : { error: r.hint },
          ),
      )
      .catch((e) => live && setDiff({ error: errorText(e) }));
    return () => {
      live = false;
    };
  }, [cwd, number]);
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex h-10 shrink-0 items-center gap-2 border-b border-[#ececf0] px-5 text-[12.5px]">
        <button
          type="button"
          onClick={onBack}
          className="motion-colors -ml-1.5 flex h-7 items-center gap-1 rounded-md px-1.5 text-ink-3 hover:bg-hover hover:text-ink"
        >
          <ArrowLeft size={13} />
          详情
        </button>
        <span className="text-ink-5">·</span>
        <span className="font-semibold text-ink">改动</span>
        <span className="flex-1" />
        <span className="text-ink-4">
          {review
            ? `Agent review · ${review.findings.length} 条批注 · ${ago(review.at)}`
            : "还没有 Agent review：点下面的「让 Agent review」"}
        </span>
      </div>
      <div className="thin-scroll flex min-h-0 flex-1 flex-col overflow-auto bg-[#fafafa] px-5 py-4">
        {diff.error ? (
          <ErrorBar message={diff.error} />
        ) : diff.text === undefined ? (
          <div className="flex flex-col gap-3">
            {[0, 1, 2, 3].map((i) => (
              <div
                key={i}
                className="skeleton h-4 rounded"
                style={{ width: `${90 - i * 12}%` }}
              />
            ))}
          </div>
        ) : (
          <div className="flex flex-col gap-2">
            {diff.truncated && (
              <div className="text-[11.5px] text-[#b25e00]">
                这个 PR 的改动太大，只显示了前 4MB。
              </div>
            )}
            <ReviewDiff diff={diff.text} findings={review?.findings ?? []} />
          </div>
        )}
      </div>
    </div>
  );
}

function MergePill({ pr }: { pr: PrDetail }) {
  if (pr.mergeable === "CONFLICTING") return <Pill kind="red">有冲突</Pill>;
  if (pr.mergeStateStatus === "BLOCKED")
    return <Pill kind="orange">被阻塞</Pill>;
  if (pr.mergeable === "MERGEABLE") return <Pill kind="green">可合并</Pill>;
  return null;
}

function ReviewState({ state }: { state: string }) {
  if (state === "APPROVED") return <Pill kind="green">已批准</Pill>;
  if (state === "CHANGES_REQUESTED") return <Pill kind="red">需要修改</Pill>;
  if (state === "COMMENTED") return <Pill>评论</Pill>;
  return <Pill>{state.toLowerCase()}</Pill>;
}

function CheckIcon({ state }: { state: PrDetail["checks"][number]["state"] }) {
  if (state === "success")
    return <CircleCheck size={15} className="shrink-0 text-[#34c759]" />;
  if (state === "failure")
    return <CircleX size={15} className="shrink-0 text-[#ff3b30]" />;
  if (state === "pending")
    return (
      <CircleDashed
        size={15}
        className="shrink-0 animate-spin text-[#ff9f0a] [animation-duration:2.4s]"
      />
    );
  return <CircleDashed size={15} className="shrink-0 text-ink-5" />;
}

function Skeleton() {
  return (
    <div className="flex flex-col gap-1 px-8 pt-4">
      {[0, 1, 2].map((i) => (
        <div
          key={i}
          className="flex h-14 items-center gap-3 border-b border-[#f2f2f7] px-2"
        >
          <div className="flex flex-1 flex-col gap-2">
            <div
              className="skeleton h-3 rounded"
              style={{ width: `${30 - i * 4}%` }}
            />
            <div
              className="skeleton h-2.5 rounded"
              style={{ width: `${42 - i * 5}%` }}
            />
          </div>
          <div className="skeleton h-4 w-12 rounded-full" />
        </div>
      ))}
    </div>
  );
}

function ErrorBar({
  message,
  onRetry,
}: {
  message: string;
  onRetry?: () => void;
}) {
  return (
    <div className="enter-fade flex items-center gap-2 rounded-lg border border-[#ff3b3033] bg-[#ff3b3014] px-3 py-2 text-[12.5px] text-[#d70015]">
      <span className="min-w-0 flex-1 break-words">{message}</span>
      {onRetry && (
        <button
          type="button"
          onClick={onRetry}
          className="shrink-0 font-medium hover:underline"
        >
          重试
        </button>
      )}
    </div>
  );
}

function Command({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="flex h-9 w-full items-center justify-between rounded-lg bg-[#f5f5f7] px-3">
      <span className="font-mono text-xs text-ink">{text}</span>
      <button
        type="button"
        title={copied ? "已复制" : "复制"}
        aria-label="复制"
        onClick={() => {
          navigator.clipboard.writeText(text).then(() => {
            setCopied(true);
            setTimeout(() => setCopied(false), 1200);
          });
        }}
        className="motion-colors text-ink-4 hover:text-ink"
      >
        {copied ? (
          <CircleCheck size={13} className="text-[#34c759]" />
        ) : (
          <Copy size={13} />
        )}
      </button>
    </div>
  );
}

function Unavailable({
  data,
  path,
  onRetry,
  onPickOther,
}: {
  data: PrUnavailable;
  path: string;
  onRetry: () => void;
  onPickOther?: () => void;
}) {
  const recheck = (
    <button type="button" className={`${primaryButton} mt-1`} onClick={onRetry}>
      重新检测
    </button>
  );
  if (data.reason === "gh_missing")
    return (
      <EmptyCard
        icon={Terminal}
        title="没找到 GitHub CLI"
        body="Pull Request 页靠本机的 gh 读取。安装后登录一次即可。"
      >
        <Command text="brew install gh" />
        <Command text="gh auth login" />
        {recheck}
      </EmptyCard>
    );
  if (data.reason === "gh_unauthenticated")
    return (
      <EmptyCard
        icon={KeyRound}
        title="gh 还没登录 GitHub"
        body="登录一次后就能读取这个仓库的 Pull Request。"
      >
        <Command text="gh auth login" />
        {recheck}
      </EmptyCard>
    );
  if (data.reason === "not_github" || data.reason === "not_git")
    return (
      <EmptyCard
        icon={FolderGit2}
        title={
          data.reason === "not_git"
            ? "这个项目不是 git 仓库"
            : "这个项目没有 GitHub 远端"
        }
        body={
          <span className="break-all">
            {path}{" "}
            {data.reason === "not_git"
              ? "不在 git 仓库里。"
              : "不是 GitHub 仓库，或还没有指向 GitHub 的远端。"}
          </span>
        }
      >
        {onPickOther ? (
          <button
            type="button"
            className={`${primaryButton} mt-1`}
            onClick={onPickOther}
          >
            换个项目
          </button>
        ) : (
          recheck
        )}
      </EmptyCard>
    );
  return (
    <div className="flex flex-col px-8 pt-4">
      <ErrorBar message={data.hint} onRetry={onRetry} />
    </div>
  );
}
