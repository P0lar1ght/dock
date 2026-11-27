// 右侧「行为轨迹」面板（`sandbox:trace`）：这一页开了沙箱以后 agent 做过什么——
// 链路图（会话 → 工具 → 程序 → 主机）+ 时间线。点图上的节点只看和它有关的那几行。
// 面板开着时每 2.5 秒取一次（只在记录数变了时重画），关着不取。

import { useEffect, useMemo, useState } from "react";
import {
  CircleAlert,
  CornerDownRight,
  Download,
  Flag,
  GitBranch,
  Globe,
  RefreshCw,
  ShieldCheck,
  SquareTerminal,
  Waypoints,
  Wrench,
  X,
} from "lucide-react";
import { store } from "../../dock/store";
import { IconButton } from "../../components/chrome";
import { useFeatureHost, type PanelProps } from "../registry";
import {
  canTrace,
  humanBytes,
  sandboxStatus,
  sandboxTrace,
  type RowKind,
  type SandboxStatus,
  type SandboxTrace,
  type TraceNode,
  type TraceRow,
} from "./api";
import { TraceGraph } from "./TraceGraph";
import { HttpHistory } from "./HttpHistory";

const POLL_MS = 2500;
const STATUS_MS = 15000;

export function TracePanel({ sessionId, onClose, lead, controls }: PanelProps) {
  const host = useFeatureHost();
  const [status, setStatus] = useState<SandboxStatus | null>(null);
  const [trace, setTrace] = useState<SandboxTrace | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [nonce, setNonce] = useState(0);
  const [busy, setBusy] = useState(false);
  const [selected, setSelected] = useState<string | null>(null);
  const [tab, setTab] = useState<"trace" | "http">("trace");
  const supported = canTrace();

  // 现状：开面板、手动刷新、开关之后取，平时 15 秒一次（会验一遍日志，别太勤）。
  useEffect(() => {
    if (!sessionId || !supported) return;
    let live = true;
    const load = () =>
      sandboxStatus(sessionId)
        .then((s) => live && setStatus(s))
        .catch((e) => live && setError(e instanceof Error ? e.message : String(e)));
    void load();
    const h = setInterval(() => !document.hidden && void load(), STATUS_MS);
    return () => {
      live = false;
      clearInterval(h);
    };
  }, [sessionId, supported, nonce]);

  // 轨迹：轮询（看「HTTP 历史」时不取）；记录数没变就不换（流式时不让整张图跟着重画）。
  useEffect(() => {
    if (!sessionId || !supported || (tab !== "trace" && trace)) return;
    let live = true;
    const load = () =>
      sandboxTrace(sessionId)
        .then((t) => {
          if (!live) return;
          setError(null);
          setTrace((prev) => (prev && prev.auditing === t.auditing && prev.records === t.records ? prev : t));
        })
        .catch((e) => live && setError(e instanceof Error ? e.message : String(e)));
    void load();
    const h = setInterval(() => !document.hidden && void load(), POLL_MS);
    return () => {
      live = false;
      clearInterval(h);
    };
  }, [sessionId, supported, nonce, tab]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => setSelected(null), [sessionId]);

  const slash = async (text: string) => {
    if (!sessionId || busy) return null;
    setBusy(true);
    const r = await store.runSlash(sessionId, text);
    setBusy(false);
    setNonce((n) => n + 1);
    return r;
  };
  const toggle = () => void slash(status?.enabled ? "/sandbox off" : "/sandbox on");
  const exportReport = async () => {
    const r = await slash("/sandbox export");
    if (r?.kind === "notice") host.toast(r.body || r.title);
  };

  const nodes = trace?.graph.nodes ?? [];
  const edges = trace?.graph.edges ?? [];
  const rows = trace?.timeline ?? [];
  const selectedNode = nodes.find((n) => n.id === selected);
  const shown = useMemo(() => {
    const keep = relatedTo(rows, selectedNode, nodes);
    return rows.filter(keep).reverse();
  }, [rows, selectedNode, nodes]);

  return (
    <aside className="glass-pane enter-fade relative flex min-h-0 min-w-0 flex-1 flex-col bg-[#fafafa]">
      <header className="flex h-11 shrink-0 items-center gap-2 border-b border-[#ececf0] px-3" data-tauri-drag-region>
        {lead}
        <Waypoints size={16} className="shrink-0 text-[#636366]" />
        <span className="shrink-0 text-[13px] font-semibold text-ink">行为轨迹</span>
        {status && (
          <span
            className={`motion-colors shrink-0 rounded-full px-2 py-0.5 text-[11px] font-medium ${
              status.enabled ? "bg-[#e8f5ec] text-[#1f7a3d]" : "bg-fill text-ink-4"
            }`}
          >
            沙箱{status.enabled ? "开" : "关"}
          </span>
        )}
        {status?.enabled && (
          <div className="flex shrink-0 rounded-md bg-fill p-0.5 text-[11.5px]">
            {(
              [
                ["trace", "轨迹"],
                ["http", "HTTP 历史"],
              ] as const
            ).map(([id, label]) => (
              <button
                key={id}
                type="button"
                onClick={() => setTab(id)}
                className={`motion-colors rounded px-2 py-0.5 font-medium ${tab === id ? "bg-white text-ink shadow-[0_0_0_0.5px_rgba(0,0,0,0.08),0_1px_2px_rgba(0,0,0,0.06)]" : "text-ink-4 hover:text-ink-2"}`}
              >
                {label}
              </button>
            ))}
          </div>
        )}
        <div className="min-w-2 flex-1" data-tauri-drag-region />
        {status && supported && (
          <button
            type="button"
            disabled={!sessionId || busy}
            onClick={toggle}
            title={status.enabled ? "关掉这一页的沙箱（不再记录）" : "开这一页的沙箱：记下工具调用、命令和出网"}
            className="motion-colors flex h-7 shrink-0 items-center gap-1 rounded-md border border-line-strong bg-white px-2 text-[12px] font-medium text-ink-2 hover:bg-hover disabled:opacity-50"
          >
            <ShieldCheck size={12} />
            {status.enabled ? "关沙箱" : "开沙箱"}
          </button>
        )}
        {trace?.auditing && (
          <IconButton icon={Download} label="导出审计报告（HTML + 原始记录）" size={13} onClick={() => void exportReport()} className="size-7 text-ink-4" />
        )}
        <IconButton icon={RefreshCw} label="刷新" size={13} onClick={() => setNonce((n) => n + 1)} className="size-7 text-ink-4" />
        {controls}
        <span className="mx-0.5 h-4 w-px bg-line" />
        <IconButton icon={X} label="关闭行为轨迹" size={14} onClick={onClose} className="size-7 text-ink-4" />
      </header>
      <div className="thin-scroll flex min-h-0 flex-1 flex-col gap-3 overflow-auto p-3">
        {!sessionId ? (
          <Empty text="先打开一个会话" />
        ) : !supported ? (
          <Empty text="这个 Dock 版本还不支持行为轨迹，升级 Dock 之后再看" />
        ) : error && !trace ? (
          <div className="rounded-lg border border-[#fecaca] bg-[#fef2f2] px-3 py-2 text-[12.5px] text-[#c0392b]">{error}</div>
        ) : !status || !trace ? (
          <div className="flex flex-col gap-2">
            {[0, 1, 2, 3].map((i) => (
              <div key={i} className="skeleton h-4 rounded" style={{ width: `${90 - i * 12}%` }} />
            ))}
          </div>
        ) : (
          <>
            <StatusLine status={status} />
            {!status.enabled ? (
              <OffState busy={busy} onTurnOn={toggle} />
            ) : tab === "http" ? (
              <HttpHistory sessionId={sessionId} active={tab === "http"} />
            ) : rows.length === 0 ? (
              <Empty text="还没有记录：让 agent 干点活，工具调用、命令和出网会出现在这里" />
            ) : (
              <>
                {trace.totals && <Totals totals={trace.totals} />}
                <section className="enter-fade rounded-xl border border-line bg-white p-2">
                  <TraceGraph nodes={nodes} edges={edges} selected={selected} onSelect={setSelected} />
                </section>
                <section className="flex flex-col">
                  <div className="mb-1 flex items-center gap-2 px-1 text-[11.5px] text-ink-4">
                    <span>时间线（最新在上）</span>
                    {selectedNode && (
                      <button
                        type="button"
                        onClick={() => setSelected(null)}
                        className="motion-colors flex items-center gap-1 rounded-full bg-[#eaf3ff] px-2 py-0.5 text-[11px] text-[#0a6fd6] hover:bg-[#dcebff]"
                      >
                        只看「{selectedNode.label}」
                        <X size={10} />
                      </button>
                    )}
                    <span className="flex-1" />
                    <span>{shown.length} 条</span>
                  </div>
                  <ol className="flex flex-col">
                    {shown.map((r, i) => (
                      <Row key={`${r.ts}-${i}-${r.kind}`} row={r} />
                    ))}
                  </ol>
                </section>
              </>
            )}
          </>
        )}
      </div>
    </aside>
  );
}

function Empty({ text }: { text: string }) {
  return <div className="flex flex-1 items-center justify-center px-6 text-center text-[13px] leading-relaxed text-ink-4">{text}</div>;
}

function OffState({ busy, onTurnOn }: { busy: boolean; onTurnOn: () => void }) {
  return (
    <div className="enter-up flex flex-col items-center gap-3 rounded-xl border border-line bg-white px-6 py-8 text-center">
      <Waypoints size={22} className="text-ink-4" />
      <div className="text-[13px] font-medium text-ink-2">这一页的沙箱没开</div>
      <div className="max-w-[360px] text-[12px] leading-relaxed text-ink-4">
        开了以后，这一页的工具调用、命令、命令和浏览器的出网（HTTPS 也解密）都记进单独的审计日志，这里画成链路图和时间线。
        不减少 agent 能做的事；关着时没有任何开销。
      </div>
      <button
        type="button"
        disabled={busy}
        onClick={onTurnOn}
        className="motion-colors flex h-8 items-center gap-1.5 rounded-lg bg-accent px-3 text-[12.5px] font-medium text-white hover:opacity-90 disabled:opacity-50"
      >
        <ShieldCheck size={13} />
        开沙箱
      </button>
    </div>
  );
}

function StatusLine({ status }: { status: SandboxStatus }) {
  const ws = status.workspace;
  const name = (p: string) => p.split(/[\\/]/).filter(Boolean).pop() ?? p;
  const chips: { text: string; tone?: "bad" | "warn"; title?: string }[] = [];
  if (status.auditing) {
    chips.push({ text: `${status.records} 条记录` });
    chips.push(status.intact ? { text: "哈希链完好", title: "日志只追加、每行带上一行的哈希，正文块也对得上" } : { text: "哈希链对不上", tone: "bad", title: "有记录被改过或删过，导出的 verify.txt 里有细节" });
    chips.push({ text: status.decrypt ? "HTTPS 解密" : "HTTPS 不解密" });
  }
  if (status.confine) chips.push({ text: `命令在系统沙箱里${status.network === "proxy" ? " · 只经代理出网" : status.network === "off" ? " · 不联网" : ""}` });
  if (status.rules.length) chips.push({ text: `${status.rules.length} 条策略` });
  if (status.passthrough.length)
    chips.push({ text: `${status.passthrough.length} 个主机不解密`, tone: "warn", title: status.passthrough.map((p) => p.host).join("、") });
  return (
    <div className="flex flex-col gap-1.5 px-1">
      <div className="flex min-w-0 items-center gap-1.5 text-[12px] text-ink-3" title={`${ws.root}${status.cwd !== ws.root ? `\n当前目录：${status.cwd}` : ""}`}>
        {ws.mode === "worktree" ? <GitBranch size={12} className="shrink-0" /> : null}
        <span className="shrink-0 text-ink-4">工作区</span>
        <span className="min-w-0 truncate font-medium text-ink-2">{name(ws.root)}</span>
        <span className="shrink-0 text-ink-4">{ws.mode === "worktree" ? `工作树 ${ws.branch ?? ""}` : "原地"}</span>
        {ws.root !== ws.home && <span className="min-w-0 truncate text-ink-4">· 项目 {name(ws.home)}</span>}
      </div>
      {chips.length > 0 && (
        <div className="flex flex-wrap gap-1">
          {chips.map((c) => (
            <span
              key={c.text}
              title={c.title}
              className={`rounded-full px-2 py-0.5 text-[11px] ${
                c.tone === "bad" ? "bg-[#fef2f2] text-[#c0392b]" : c.tone === "warn" ? "bg-[#fff7e6] text-[#b25e00]" : "bg-fill text-ink-3"
              }`}
            >
              {c.text}
            </span>
          ))}
        </div>
      )}
    </div>
  );
}

function Totals({ totals }: { totals: NonNullable<SandboxTrace["totals"]> }) {
  const items = [
    { label: "工具调用", value: `${totals.toolCalls}` },
    { label: "命令", value: `${totals.commands}` },
    { label: "出网", value: `${totals.requests}`, hint: totals.requests ? `解密 ${totals.decrypted}` : undefined },
    { label: "流量", value: humanBytes(totals.bytes) },
    { label: "出错", value: `${totals.errors}`, bad: totals.errors > 0 },
  ];
  return (
    <div className="grid grid-cols-5 gap-1.5">
      {items.map((it) => (
        <div key={it.label} className="rounded-lg border border-line bg-white px-2 py-1.5">
          <div className="text-[10.5px] text-ink-4">{it.label}</div>
          <div className={`text-[14px] font-semibold tabular-nums ${it.bad ? "text-[#c0392b]" : "text-ink"}`}>{it.value}</div>
          {it.hint && <div className="text-[10px] text-ink-4">{it.hint}</div>}
        </div>
      ))}
    </div>
  );
}

const ICONS: Record<RowKind, typeof Wrench> = {
  tool: Wrench,
  result: CornerDownRight,
  command: SquareTerminal,
  command_end: Flag,
  net: Globe,
  audit: ShieldCheck,
};

const KIND_LABEL: Record<RowKind, string> = {
  tool: "工具",
  result: "结果",
  command: "命令",
  command_end: "命令结束",
  net: "出网",
  audit: "沙箱",
};

function Row({ row }: { row: TraceRow }) {
  const [open, setOpen] = useState(false);
  const Icon = row.error ? CircleAlert : ICONS[row.kind];
  const time = new Date(row.ts).toLocaleTimeString("zh-CN", { hour12: false });
  const indent = row.kind === "result" || row.kind === "command_end";
  return (
    <li>
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className={`motion-colors flex w-full items-start gap-2 rounded-md px-1.5 py-1 text-left hover:bg-hover ${indent ? "pl-5" : ""}`}
      >
        <span className="w-[54px] shrink-0 pt-px text-[10.5px] tabular-nums text-ink-4">{time}</span>
        <Icon size={13} className={`mt-px shrink-0 ${row.error ? "text-[#ff3b30]" : "text-ink-4"}`} aria-label={KIND_LABEL[row.kind]} />
        <span className="min-w-0 flex-1">
          <span className="flex min-w-0 items-baseline gap-1.5">
            <span className={`shrink-0 text-[12px] font-medium ${row.error ? "text-[#c0392b]" : "text-ink-2"}`}>{row.title || KIND_LABEL[row.kind]}</span>
            {row.error && <span className="shrink-0 text-[11px] text-[#c0392b]">{row.error}</span>}
            {!open && row.detail && <span className="min-w-0 truncate text-[11.5px] text-ink-4">{row.detail}</span>}
          </span>
          {open && row.detail && <span className="mt-0.5 block whitespace-pre-wrap break-all font-mono text-[11px] leading-relaxed text-ink-3">{row.detail}</span>}
        </span>
      </button>
    </li>
  );
}

/** 选中图上一个节点时，时间线里和它有关的那几行：工具 → 它的调用、调用起的命令、命令的出网；
 *  程序 → 那些命令和它们的出网；主机 → 发往它的出网。 */
function relatedTo(rows: TraceRow[], node: TraceNode | undefined, nodes: TraceNode[]): (r: TraceRow) => boolean {
  if (!node || node.kind === "session") return () => true;
  const jobsOfCalls = (calls: Set<string>) =>
    new Set(rows.filter((r) => r.kind === "command" && r.call && calls.has(r.call)).map((r) => r.job ?? ""));
  const byJobs = (jobs: Set<string>) => (r: TraceRow) =>
    ((r.kind === "command" || r.kind === "command_end") && !!r.job && jobs.has(r.job)) || (r.kind === "net" && !!r.tag && jobs.has(r.tag));
  if (node.kind === "tool") {
    if (node.id === "tool:浏览器") return (r) => r.kind === "net" && (r.tag ?? "").startsWith("browser");
    const name = node.id.slice("tool:".length);
    const calls = new Set(rows.filter((r) => (r.kind === "tool" || r.kind === "result") && r.title === name).map((r) => r.call ?? ""));
    const jobs = jobsOfCalls(calls);
    const viaJobs = byJobs(jobs);
    return (r) => ((r.kind === "tool" || r.kind === "result") && r.title === name) || viaJobs(r);
  }
  if (node.kind === "program") {
    const jobs = new Set(rows.filter((r) => r.kind === "command" && r.title === node.label).map((r) => r.job ?? ""));
    return byJobs(jobs);
  }
  if (node.kind === "host") return (r) => r.kind === "net" && r.host === node.label;
  if (node.kind === "others") {
    const named = new Set(nodes.filter((n) => n.kind === "host").map((n) => n.label));
    return (r) => r.kind === "net" && !named.has(r.host ?? "");
  }
  // 认不出是谁发的出网。
  const jobs = new Set(rows.filter((r) => r.kind === "command").map((r) => r.job ?? ""));
  return (r) => r.kind === "net" && !(r.tag ?? "").startsWith("browser") && !jobs.has(r.tag ?? "");
}
