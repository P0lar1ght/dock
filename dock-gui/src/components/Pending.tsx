// 会话区底部、输入框上方：还在等用户的交互（dock-core 的 `pendingItems`）。
// 权限条照 wonder-ui/chat/PermissionStrip；提问卡照 QuestionCard（gena / golo /
// coft / wail / chih），计划卡照 PlanCard（jeel / khat / rimy），MCP 确认照 ElicitCard（bere）。
// 处理完 Dock 会推 resolved，卡片随之消失、在时间线里留一行记录。

import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { Check, ChevronDown, ClipboardList, CornerDownLeft, FileText, MessageCircleQuestion, Plug, Shield } from "lucide-react";
import type { PendingItem } from "dock-core";
import type { QuestionAnswer } from "../dock/store";
import { Markdown } from "./Markdown";
import { imeComposing } from "../dock/ime";
import { permissionParts } from "../dock/permission";

export interface PendingActions {
  /** `always`：本会话里这个工具以后不再问（子代理也算，和 TUI 的「始终允许」同一语义）。 */
  onPermission: (allow: boolean, always?: boolean) => void;
  onAnswer: (answers: QuestionAnswer[]) => void;
  onPlan: (decision: "approve" | "revise" | "quit") => void;
  onElicit: (action: "accept" | "decline") => void;
  /** URL 模式：开浏览器并把「用户同意」回给服务器。 */
  onElicitOpen: () => void;
}

/**
 * 权限摘要：首行是要放行的东西（命令工具是 `$ 完整命令`），最多三行、悬停看全文；
 * 后面的行是说明（命令工具的 `description`，模型写的），正文字、只作补充。
 * 单行摘要（其它工具）和以前一样一行截断。
 */
function PermissionSummary({ summary }: { summary: string }) {
  const { main, note } = permissionParts(summary);
  // 命令要看全：宁可多占两行，也别把后半截（往往是要紧的部分）截掉。
  const command = main.startsWith("$ ");
  return (
    <>
      <span
        title={main}
        className={`font-mono text-[11px] ${command || note ? "line-clamp-3 whitespace-pre-wrap break-all text-ink-3" : "truncate text-ink-4"}`}
      >
        {main}
      </span>
      {note && <span className="truncate text-[11.5px] text-ink-3">{note}</span>}
    </>
  );
}

const card = "md-surface enter-up mx-4 mb-2 flex shrink-0 flex-col gap-2.5 rounded-[10px] border border-line-strong bg-[#f5f5f7] p-3";
const primary =
  "motion-colors flex h-[26px] shrink-0 items-center rounded-[7px] bg-ink px-3 text-[11px] font-medium text-white hover:bg-ink-2 active:scale-[0.97] disabled:bg-line-strong disabled:text-ink-4 disabled:active:scale-100";
const secondary =
  "motion-colors flex h-[26px] shrink-0 items-center rounded-[7px] border border-line-strong bg-white px-2.5 text-[11px] font-medium text-ink-2 hover:bg-hover active:scale-[0.97]";
/** 卡头右侧的小标签：计划文件路径、MCP 服务名。 */
const tag = "flex min-w-0 items-center gap-1.5 rounded-sm border border-line-strong bg-white px-2 py-0.5";

export function Pending({
  item,
  subagentRole,
  noAlways = false,
  ...on
}: {
  item: PendingItem;
  /** 权限请求是子代理发的：它的角色名（设计 are 的「来自子代理 · 探索」）。 */
  subagentRole?: string | null;
  /** 不给「始终允许」（侧边聊天：只读旁问里会改东西的命令每次都问）。 */
  noAlways?: boolean;
} & PendingActions) {
  switch (item.kind) {
    case "permission": {
      const tool = item.toolName || "工具";
      return (
        <div key={item.id} className={`${card} flex-row items-center gap-2.5 py-2`}>
          <Shield size={14} className="shrink-0 text-ink-3" />
          {item.agentId && (
            <span className="flex h-5 shrink-0 items-center rounded-full bg-[#e8e8ec] px-2 text-[11px] font-medium leading-none text-[#636366]">
              来自子代理 · {subagentRole || "子代理"}
            </span>
          )}
          <div className="flex min-w-0 flex-1 flex-col gap-px">
            <span className="text-xs font-medium text-ink">允许 {tool}？</span>
            <PermissionSummary summary={item.summary} />
          </div>
          <button type="button" onClick={() => on.onPermission(true)} className={primary}>
            允许本次
          </button>
          {/* 设计稿只有「允许本次 / 拒绝」；补上 TUI 有的「始终允许」，样式同次按钮。 */}
          {!noAlways && (
            <button
              type="button"
              title={`这个会话里 ${tool} 以后不再问（子代理也算），重开会话后恢复询问`}
              onClick={() => on.onPermission(true, true)}
              className={secondary}
            >
              始终允许
            </button>
          )}
          <button type="button" onClick={() => on.onPermission(false)} className={secondary}>
            拒绝
          </button>
        </div>
      );
    }
    case "question":
      return <QuestionCard key={item.id} item={item} onAnswer={on.onAnswer} />;
    case "plan":
      return <PlanCard key={item.id} item={item} onPlan={on.onPlan} />;
    case "elicit":
      return (
        <div key={item.id} className={card}>
          <Header icon={Plug} title={item.heading || `${item.server} 请求确认`}>
            <span className={tag}>
              <span className="size-1.5 shrink-0 rounded-full bg-[#34c759]" />
              <span className="truncate font-mono text-[11px] leading-none text-ink-3">{item.server}</span>
            </span>
          </Header>
          <div className="flex items-center gap-4 rounded-lg border border-line-strong bg-white px-3.5 py-3">
            <div className="min-w-0 flex-1 whitespace-pre-wrap text-[13px] font-medium text-ink">
              {item.message}
              {/* URL 模式：完整地址 + 域名 + 可疑警告（规范要求展示目标域名）。 */}
              {item.url && (
                <div className="mt-1.5 space-y-1">
                  <div className="break-all font-mono text-[11px] text-ink-3">{item.url}</div>
                  {item.urlDomain && (
                    <div className="text-[11px] text-ink-3">
                      域名 <span className="font-mono text-ink-2">{item.urlDomain}</span>
                    </div>
                  )}
                  {item.urlWarning && (
                    <div className="text-[11px] font-medium text-[#c47f00]">⚠ {item.urlWarning}</div>
                  )}
                </div>
              )}
            </div>
            <div className="flex shrink-0 items-center gap-2">
              {item.url ? (
                <button type="button" onClick={() => on.onElicitOpen()} className={primary}>
                  打开链接
                </button>
              ) : (
                <>
                  <button type="button" onClick={() => on.onElicit("decline")} className={secondary}>
                    拒绝
                  </button>
                  <button type="button" onClick={() => on.onElicit("accept")} className={primary}>
                    接受
                  </button>
                </>
              )}
            </div>
          </div>
        </div>
      );
  }
}

/** 卡头：浅蓝圆底图标 + 标题，右侧可放标签。 */
function Header({ icon: Icon, title, children }: { icon: typeof Shield; title: string; children?: ReactNode }) {
  return (
    <div className="flex min-w-0 items-center justify-between gap-3">
      <div className="flex min-w-0 items-center gap-2">
        <span className="flex size-5 shrink-0 items-center justify-center rounded-full bg-[#0a84ff1a]">
          <Icon size={12} className="text-accent" />
        </span>
        <span className="truncate text-xs font-semibold text-ink">{title}</span>
      </div>
      {children}
    </div>
  );
}

/** 收起时正文的最大高度；超出就盖一层渐隐 +「展开全部」。 */
const PLAN_FOLDED = 190;

function PlanCard({
  item,
  onPlan,
}: {
  item: Extract<PendingItem, { kind: "plan" }>;
  onPlan: PendingActions["onPlan"];
}) {
  const body = useRef<HTMLDivElement>(null);
  const [long, setLong] = useState(false);
  const [open, setOpen] = useState(false);
  useLayoutEffect(() => {
    setLong((body.current?.scrollHeight ?? 0) > PLAN_FOLDED + 8);
  }, [item.body]);
  const lines = item.body.split("\n").length;
  return (
    <div className={card}>
      <Header icon={ClipboardList} title="计划待批准">
        {item.path && (
          <span className={tag} title={item.path}>
            <FileText size={12} className="shrink-0 text-ink-4" />
            <span className="truncate font-mono text-[11px] leading-none text-ink-3">{item.path}</span>
          </span>
        )}
      </Header>
      <div className="relative overflow-hidden rounded-lg border border-line-strong bg-white">
        <div
          ref={body}
          style={{ maxHeight: long && !open ? PLAN_FOLDED : undefined }}
          className={`thin-scroll max-h-[45vh] p-3 transition-[max-height] duration-200 ease-out ${long && !open ? "glass-fold overflow-hidden" : "overflow-y-auto"}`}
        >
          {item.empty ? <span className="text-xs text-ink-4">计划文件是空的。</span> : <Markdown text={item.body} size="card" />}
        </div>
        {long && !open && (
          <div className="glass-fade absolute inset-x-0 bottom-0 flex h-14 items-end justify-center bg-gradient-to-t from-white via-white/90 to-transparent pb-2">
            <button
              type="button"
              onClick={() => setOpen(true)}
              className="motion-colors enter-fade flex items-center gap-1.5 rounded-full border border-line-strong bg-white px-3 py-1 text-[11px] font-medium leading-none text-ink shadow-sm hover:bg-hover"
            >
              展开全部计划（共 {lines} 行）
              <ChevronDown size={12} className="text-ink-4" />
            </button>
          </div>
        )}
      </div>
      <div className="flex items-center justify-between pt-1">
        <button type="button" onClick={() => onPlan("quit")} className={secondary}>
          退出计划
        </button>
        <div className="flex items-center gap-2">
          <button type="button" onClick={() => onPlan("revise")} className={secondary}>
            要求修改
          </button>
          <button type="button" onClick={() => onPlan("approve")} className={primary}>
            批准并执行
          </button>
        </div>
      </div>
    </div>
  );
}

type Draft = { picked: string[]; other: string };
const EMPTY: Draft = { picked: [], other: "" };
const answered = (d: Draft | undefined) => !!d && (d.picked.length > 0 || d.other.trim() !== "");

/**
 * 模型的提问（`ask` 工具）。单选题点一个选项或写自己的回答（二选一）；多选题
 * 可以点多个，还能再写一句；每题都答了才能提交。
 * 键盘：数字键作用于当前这题（单选选中后自动跳到下一道没答的），Enter 提交。
 * 卡片出现时把焦点给它（刚发完消息焦点多半还在空的输入框里），只有正在别处
 * 打字时不抢。
 */
function QuestionCard({
  item,
  onAnswer,
}: {
  item: Extract<PendingItem, { kind: "question" }>;
  onAnswer: PendingActions["onAnswer"];
}) {
  const [drafts, setDrafts] = useState<Record<string, Draft>>({});
  const [current, setCurrent] = useState(0);
  const ready = item.questions.every((q) => answered(drafts[q.id]));
  const submit = () => {
    if (!ready) return;
    onAnswer(
      item.questions.map((q) => {
        const d = drafts[q.id] ?? EMPTY;
        const other = d.other.trim();
        return { questionId: q.id, values: d.picked, ...(other ? { other } : {}) };
      }),
    );
  };

  const pick = (index: number, label: string) => {
    const q = item.questions[index];
    const d = drafts[q.id] ?? EMPTY;
    let next: Draft;
    if (q.multiSelect) {
      next = { ...d, picked: d.picked.includes(label) ? d.picked.filter((l) => l !== label) : [...d.picked, label] };
    } else {
      next = { picked: [label], other: "" };
    }
    const all = { ...drafts, [q.id]: next };
    setDrafts(all);
    if (q.multiSelect) {
      setCurrent(index);
    } else {
      // 单选答完跳到下一道没答的题，接着按数字键就行。
      const after = item.questions.findIndex((x, i) => i > index && !answered(all[x.id]));
      setCurrent(after === -1 ? index : after);
    }
  };

  const write = (index: number, text: string) => {
    const q = item.questions[index];
    const d = drafts[q.id] ?? EMPTY;
    // 单选题写了自己的回答就不再算选了某个选项。
    setDrafts({ ...drafts, [q.id]: { picked: q.multiSelect ? d.picked : [], other: text } });
    setCurrent(index);
  };

  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const active = document.activeElement;
    const typing = (active instanceof HTMLInputElement || active instanceof HTMLTextAreaElement) && active.value.trim() !== "";
    if (!typing) ref.current?.focus();
  }, []);

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    // 输入法选字时的 Enter 不是提交。
    if (e.metaKey || e.ctrlKey || e.altKey || imeComposing(e)) return;
    // 焦点在选项按钮上时 Enter 是按下这个按钮（选中它），交给浏览器。
    if (e.key === "Enter" && !e.shiftKey && !(e.target instanceof HTMLButtonElement)) {
      e.preventDefault();
      submit();
      return;
    }
    if (e.target instanceof HTMLInputElement || !/^[1-9]$/.test(e.key)) return;
    const option = item.questions[current]?.options[Number(e.key) - 1];
    if (!option) return;
    e.preventDefault();
    pick(current, option.label);
  };

  const many = item.questions.length > 1;
  return (
    <div ref={ref} tabIndex={-1} onKeyDown={onKeyDown} className={`${card} gap-3 outline-none`}>
      <Header icon={MessageCircleQuestion} title={many ? `Dock 在问你 · ${item.questions.length} 个问题` : "Dock 在问你"} />
      {item.questions.map((q, qi) => {
        const d = drafts[q.id] ?? EMPTY;
        const keys = qi === current;
        return (
          <div key={q.id} className="flex flex-col gap-3">
            {qi > 0 && <div className="h-px bg-line-strong" />}
            <div className="flex flex-col gap-2">
              <div className="flex items-center gap-2">
                {many && (
                  <span className="flex size-4 shrink-0 items-center justify-center rounded-full bg-ink text-[10px] font-bold text-white">
                    {qi + 1}
                  </span>
                )}
                <span className="min-w-0 text-sm font-semibold text-ink">{q.question}</span>
                {(many || q.multiSelect) && (
                  <span className="shrink-0 rounded-sm border border-line-strong bg-fill px-1.5 py-0.5 text-[11px] font-medium leading-none text-ink-3">
                    {q.multiSelect ? "可多选" : "单选"}
                  </span>
                )}
                {keys && q.options.length > 0 && (
                  <span className="enter-fade ml-auto shrink-0 text-xs text-ink-4">按数字键快速选择</span>
                )}
              </div>
              <div className="grid grid-cols-[repeat(auto-fit,minmax(160px,1fr))] gap-2">
                {q.options.map((o, i) => (
                  <Option
                    key={o.label}
                    label={o.label}
                    description={o.description !== o.label ? o.description : ""}
                    multi={!!q.multiSelect}
                    on={d.picked.includes(o.label)}
                    digit={keys && i < 9 ? i + 1 : undefined}
                    onClick={() => pick(qi, o.label)}
                  />
                ))}
              </div>
              <input
                value={d.other}
                onChange={(e) => write(qi, e.target.value)}
                onFocus={() => setCurrent(qi)}
                placeholder={q.multiSelect ? "还可以补一句你自己的回答" : "或者写你自己的回答"}
                className="motion-colors h-8 rounded-[7px] border border-line-strong bg-white px-2.5 text-xs text-ink outline-none select-text placeholder:text-ink-4 focus:border-accent"
              />
            </div>
          </div>
        );
      })}
      <div className="flex items-center justify-end gap-2 pt-1">
        {ready && (
          <span className="enter-fade flex items-center gap-1.5 text-[11px] text-ink-4">
            <kbd className="flex h-[18px] items-center gap-1 rounded border border-line-strong bg-white px-1.5 font-mono text-[10px] text-ink-3">
              <CornerDownLeft size={10} />
              Enter
            </kbd>
            提交
          </span>
        )}
        <button type="button" disabled={!ready} onClick={submit} className={primary}>
          提交回答
        </button>
      </div>
    </div>
  );
}

/** 一个选项：左侧数字键（当前这题才显示）+ 单选圆点 / 多选勾框，下方直接写说明。 */
function Option({
  label,
  description,
  multi,
  on,
  digit,
  onClick,
}: {
  label: string;
  description: string;
  multi: boolean;
  on: boolean;
  digit?: number;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      aria-pressed={on}
      onClick={onClick}
      className={`motion-colors flex min-w-0 flex-col gap-1 rounded-lg border bg-white p-2.5 text-left active:scale-[0.99] ${
        on ? "border-accent shadow-sm ring-1 ring-accent" : "border-line-strong hover:bg-[#fbfbfc] hover:border-ink-5"
      }`}
    >
      <span className="flex min-w-0 items-center gap-2">
        {digit !== undefined && (
          <kbd
            className={`motion-colors flex size-4 shrink-0 items-center justify-center rounded-[4px] border font-mono text-[10px] ${
              on ? "border-accent text-accent" : "border-line-strong bg-fill text-ink-3"
            }`}
          >
            {digit}
          </kbd>
        )}
        {multi ? (
          <span
            className={`motion-colors flex size-3.5 shrink-0 items-center justify-center rounded-sm border ${
              on ? "border-accent bg-accent" : "border-ink-5 bg-white"
            }`}
          >
            {on && <Check size={10} strokeWidth={3} className="pop-in text-white" />}
          </span>
        ) : (
          <span
            className={`motion-colors flex size-3.5 shrink-0 items-center justify-center rounded-full bg-white ${
              on ? "border-2 border-accent" : "border border-ink-5"
            }`}
          >
            {on && <span className="pop-in size-1.5 rounded-full bg-accent" />}
          </span>
        )}
        <span className={`truncate text-[13px] ${on ? "font-semibold text-accent" : "font-medium text-ink"}`}>{label}</span>
      </span>
      {description && (
        <span className={`pl-[22px] text-xs ${on ? "text-ink-3" : "text-ink-4"} ${digit !== undefined ? "ml-6" : ""}`}>
          {description}
        </span>
      )}
    </button>
  );
}
