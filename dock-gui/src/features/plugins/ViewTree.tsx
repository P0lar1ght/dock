// 插件视图树 dock.view.1 的通用画法。设计：wonder-ui/plugins/PluginViewGallery（hum）。
// 同一种节点在哪都长一样：面板、工具卡、以后别的地方都用这一个组件。
// 规则（第七轮验收）：只有 section 画成卡片；kv 标签列 96；表格自带边框；
// 执行中被点的按钮转圈，同一棵树里其它可点的都禁用。

import { useState, type ReactNode } from "react";
import { ChevronDown, ChevronRight, Copy, ExternalLink, Inbox, Loader } from "lucide-react";
import type { Tone, ViewNode } from "dock-core";
import { Markdown } from "../../components/Markdown";

/** 色调 → 字色。 */
export const TONE_TEXT: Record<Tone, string> = {
  default: "text-ink",
  muted: "text-ink-4",
  accent: "text-accent",
  success: "text-[#248a3d]",
  warning: "text-[#c93400]",
  danger: "text-[#d70015]",
};

/** 胶囊（badge / 列表行右侧）：底色 + 字色。 */
export const TONE_PILL: Record<Tone, string> = {
  default: "bg-[#e8e8ec] text-ink-2",
  muted: "bg-[#f2f2f7] text-ink-4",
  accent: "bg-[#0a84ff1a] text-accent",
  success: "bg-[#34c7591a] text-[#248a3d]",
  warning: "bg-[#ff9f0a1a] text-[#c93400]",
  danger: "bg-[#ff3b301a] text-[#d70015]",
};

const GAP = { s: "gap-1.5", m: "gap-3", l: "gap-4" } as const;
const TEXT_SIZE = { s: "text-xs", m: "text-[13px]", l: "text-[17px] font-semibold" } as const;

/** 表格超过这么多行先收起，出现「展开」。 */
const TABLE_FOLD = 20;

export interface ViewActions {
  /** 点了按钮 / 列表行 / 空态按钮。没有就是只读（如回放的工具卡）。 */
  onAction?: (action: string) => void;
  /** 正在执行的动作：它转圈，别的可点项都禁用。 */
  busy?: string | null;
}

export function ViewTree({ node, ...actions }: { node: ViewNode } & ViewActions) {
  return <Node node={node} {...actions} />;
}

function Node({ node, onAction, busy }: { node: ViewNode } & ViewActions) {
  const actions = { onAction, busy };
  const kids = (children: ViewNode[]) => children.map((c, i) => <Node key={i} node={c} {...actions} />);
  switch (node.type) {
    case "stack":
      return <div className={`flex flex-col ${GAP[node.gap]}`}>{kids(node.children)}</div>;
    case "row":
      return (
        <div className={`flex flex-wrap items-center gap-2 ${node.align === "between" ? "justify-between" : ""}`}>
          {node.align === "between" ? <RowGroups nodes={node.children} {...actions} /> : kids(node.children)}
        </div>
      );
    case "section":
      return (
        <Section title={node.title} collapsed={node.collapsed}>
          {kids(node.children)}
        </Section>
      );
    case "text":
      return (
        <div
          className={`whitespace-pre-wrap break-words leading-normal ${TONE_TEXT[node.tone]} ${TEXT_SIZE[node.size]} ${
            node.weight === "bold" ? "font-bold" : ""
          }`}
        >
          {node.text}
        </div>
      );
    case "markdown":
      return <Markdown text={node.text} size="view" />;
    case "code":
      return <CodeBlock text={node.text} />;
    case "kv":
      return (
        <div className="flex flex-col gap-1.5">
          {node.items.map((item, i) => (
            <div key={i} className="flex items-baseline text-xs leading-normal">
              <span className="w-24 shrink-0 truncate pr-2 text-ink-4">{item.label}</span>
              <span className={`min-w-0 flex-1 truncate font-medium ${TONE_TEXT[item.tone]}`} title={item.value}>
                {item.value}
              </span>
            </div>
          ))}
        </div>
      );
    case "table":
      return <Table columns={node.columns} rows={node.rows} />;
    case "list":
      return (
        <div className="flex flex-col gap-0.5">
          {node.items.map((item, i) => {
            const clickable = !!item.action && !!onAction;
            const disabled = clickable && !!busy;
            const body = (
              <>
                <span className="flex min-w-0 flex-1 flex-col gap-1">
                  <span className="truncate text-[13px] font-medium leading-none text-ink">{item.title}</span>
                  {item.subtitle && <span className="truncate text-[11px] leading-none text-ink-4">{item.subtitle}</span>}
                </span>
                {item.badge && <Pill tone={item.badge.tone}>{item.badge.text}</Pill>}
              </>
            );
            return clickable ? (
              <button
                key={i}
                type="button"
                disabled={disabled}
                onClick={() => onAction!(item.action!)}
                className="motion-colors -mx-2 flex items-center gap-2 rounded-lg p-2 text-left hover:bg-[#0a84ff14] disabled:opacity-50 disabled:hover:bg-transparent"
              >
                {body}
              </button>
            ) : (
              <div key={i} className="-mx-2 flex items-center gap-2 rounded-lg p-2">
                {body}
              </div>
            );
          })}
        </div>
      );
    case "badge":
      return (
        <span className="flex">
          <Pill tone={node.tone}>{node.text}</Pill>
        </span>
      );
    case "progress":
      return <Progress value={node.value} label={node.label} />;
    case "button":
      return <ViewButton label={node.label} action={node.action} style={node.style} {...actions} />;
    case "link":
      return (
        <a
          href={node.url}
          target="_blank"
          rel="noreferrer"
          className="motion-colors flex w-fit items-center gap-1 text-[13px] font-medium text-accent hover:underline"
        >
          {node.label || node.url}
          <ExternalLink size={14} />
        </a>
      );
    case "divider":
      return <div className="h-px w-full bg-line-strong" />;
    case "empty":
      return (
        <div className="flex flex-col items-center gap-2 px-4 py-5 text-center">
          <Inbox size={24} className="text-ink-5" />
          <span className="text-[13px] font-medium text-ink">{node.title}</span>
          {node.text && <span className="text-xs text-ink-4">{node.text}</span>}
          {node.action && (
            <span className="mt-1">
              <ViewButton label={node.label ?? node.action} action={node.action} style="primary" {...actions} />
            </span>
          )}
        </div>
      );
    case "unsupported":
      return <div className="text-xs text-ink-4">不支持的视图：{node.kind}</div>;
  }
}

/** `row align: between`：两头分开时，最后一串连续按钮成一组靠右（图鉴 row 示例）。 */
function RowGroups({ nodes, ...actions }: { nodes: ViewNode[] } & ViewActions) {
  let split = nodes.length;
  while (split > 0 && nodes[split - 1].type === "button") split -= 1;
  if (split === 0 || split === nodes.length) {
    return (
      <>
        {nodes.map((c, i) => (
          <Node key={i} node={c} {...actions} />
        ))}
      </>
    );
  }
  return (
    <>
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        {nodes.slice(0, split).map((c, i) => (
          <Node key={i} node={c} {...actions} />
        ))}
      </div>
      <div className="flex items-center gap-2">
        {nodes.slice(split).map((c, i) => (
          <Node key={i} node={c} {...actions} />
        ))}
      </div>
    </>
  );
}

export function Pill({ tone, children }: { tone: Tone; children: ReactNode }) {
  return (
    <span className={`flex h-5 shrink-0 items-center rounded-full px-2 text-[11px] font-medium leading-none ${TONE_PILL[tone]}`}>
      {children}
    </span>
  );
}

const BUTTON_STYLE = {
  primary: "bg-ink text-white hover:bg-ink-2",
  secondary: "border border-line-strong bg-white text-ink hover:bg-hover",
  danger: "bg-[#ff3b301a] text-[#d70015] hover:bg-[#ff3b3026]",
} as const;

export function ViewButton({
  label,
  action,
  style,
  onAction,
  busy,
}: { label: string; action: string; style: keyof typeof BUTTON_STYLE } & ViewActions) {
  const running = busy === action;
  const disabled = !onAction || (!!busy && !running);
  if (running) {
    return (
      <span className="flex h-8 items-center gap-1.5 rounded-lg bg-[#1c1c1e99] px-3 text-xs font-medium text-white">
        <Loader size={14} className="animate-spin" />
        {label}
      </span>
    );
  }
  return (
    <button
      type="button"
      disabled={disabled}
      onClick={() => onAction?.(action)}
      className={`motion-colors flex h-8 shrink-0 items-center rounded-lg px-3 text-xs font-medium ${
        disabled && onAction ? "bg-[#f2f2f7] text-ink-5" : BUTTON_STYLE[style]
      } disabled:cursor-default`}
    >
      {label}
    </button>
  );
}

function Section({ title, collapsed, children }: { title: string; collapsed: boolean; children: ReactNode }) {
  const [open, setOpen] = useState(!collapsed);
  return (
    <div className="flex flex-col overflow-hidden rounded-[10px] border border-line-strong bg-white">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className="motion-colors flex items-center gap-2 px-3 py-2.5 text-left hover:bg-hover"
      >
        <span className="flex-1 truncate text-[13px] font-medium leading-none text-ink">{title}</span>
        {open ? <ChevronDown size={14} className="text-ink-4" /> : <ChevronRight size={14} className="text-ink-4" />}
      </button>
      {/* 展开 / 收起：grid 行高 + 透明度（MOTION.md 工具卡同款）。 */}
      <div
        className="grid transition-[grid-template-rows,opacity] duration-[var(--dur-base)] ease-[var(--ease-out)]"
        style={{ gridTemplateRows: open ? "1fr" : "0fr", opacity: open ? 1 : 0 }}
      >
        <div className="min-h-0 overflow-hidden">
          <div className="flex flex-col gap-3 border-t border-line-soft px-3 py-2.5">{children}</div>
        </div>
      </div>
    </div>
  );
}

function CodeBlock({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="group relative rounded-lg bg-ink px-3 py-2.5">
      <pre className="thin-scroll overflow-x-auto pr-7 font-mono text-[11px] leading-normal text-[#e5e5ea]">{text}</pre>
      <button
        type="button"
        title={copied ? "已复制" : "复制"}
        onClick={() =>
          void navigator.clipboard.writeText(text).then(() => {
            setCopied(true);
            setTimeout(() => setCopied(false), 1500);
          })
        }
        className="motion-colors absolute right-2 top-2 flex size-6 items-center justify-center rounded-md text-ink-4 hover:bg-white/10 hover:text-white"
      >
        <Copy size={14} />
      </button>
    </div>
  );
}

/** 看起来是数字的格子用等宽数字（图鉴「实例」列）。 */
const NUMERIC = /^[-+]?[\d.,]+%?$/;

function Table({ columns, rows }: { columns: string[]; rows: string[][] }) {
  const [all, setAll] = useState(false);
  const n = Math.max(columns.length, ...rows.map((r) => r.length), 1);
  // 第一列略宽（图鉴 1.2fr / 0.8fr）。
  const template = `minmax(0,1.2fr) ${Array(n - 1).fill("minmax(0,0.8fr)").join(" ")}`.trim();
  const shown = all ? rows : rows.slice(0, TABLE_FOLD);
  return (
    <div className="flex flex-col overflow-hidden rounded-lg border border-line-strong">
      {columns.length > 0 && (
        <div className="grid gap-2 bg-[#f5f5f7] px-2.5 py-1.5" style={{ gridTemplateColumns: template }}>
          {columns.map((c, i) => (
            <span key={i} className="truncate text-[11px] font-medium leading-none text-ink-4">
              {c}
            </span>
          ))}
        </div>
      )}
      {shown.map((row, r) => (
        <div key={r} className="grid items-baseline gap-2 border-t border-line-soft px-2.5 py-1.5 first:border-t-0" style={{ gridTemplateColumns: template }}>
          {Array.from({ length: n }, (_, i) => row[i] ?? "").map((cell, i) => (
            <span key={i} title={cell} className={`truncate py-px text-xs leading-none text-ink ${NUMERIC.test(cell) ? "font-mono tabular-nums" : ""}`}>
              {cell}
            </span>
          ))}
        </div>
      ))}
      {rows.length > TABLE_FOLD && (
        <button
          type="button"
          onClick={() => setAll((a) => !a)}
          className="motion-colors border-t border-line-soft px-2.5 py-1.5 text-left text-[11px] font-medium text-accent hover:bg-hover"
        >
          {all ? "收起" : `展开（共 ${rows.length} 行）`}
        </button>
      )}
    </div>
  );
}

function Progress({ value, label }: { value: number | null; label: string | null }) {
  const pct = value === null ? null : Math.round(value * 100);
  return (
    <div className="flex flex-col gap-1.5">
      {(label || pct !== null) && (
        <div className="flex items-center justify-between gap-3 text-xs leading-none">
          <span className="truncate text-ink">{label}</span>
          {pct !== null && <span className="shrink-0 text-ink-4 tabular-nums">{pct}%</span>}
        </div>
      )}
      <div className="relative h-1.5 w-full overflow-hidden rounded-full bg-[#e8e8ec]">
        {pct === null ? (
          <div className="sweep absolute h-full w-[40%] rounded-full bg-[#0a84ff66]" />
        ) : (
          <div
            className="h-full origin-left rounded-full bg-accent transition-transform duration-[var(--dur-slow)] ease-[var(--ease-out)]"
            style={{ transform: `scaleX(${value})` }}
          />
        )}
      </div>
    </div>
  );
}
