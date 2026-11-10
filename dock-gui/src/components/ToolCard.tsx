// 工具卡：一行标题（图标 + 动词 + 参数 + 箭头），点开看正文。
// 设计：wonder-ui/tools/<Tool>/{collapsed,expanded,failed}.jsx；
// 会话流里的排法：wonder-ui/pages/ToolCardsStream。

import { useEffect, useState, type ReactNode } from "react";
import { formatElapsed } from "./chrome";
import { AlertCircle, ChevronRight, CircleSlash, LoaderCircle, ShieldX, type LucideIcon } from "lucide-react";
import { toolCardFeatures } from "../features/registry";
import type { ToolImage, ToolStatus } from "dock-core";
import { ToolImages } from "./tools/parts";
import { exitCode, specFor } from "./tools/specs";

export interface ToolCardProps {
  tool: string;
  args: Record<string, unknown>;
  output?: string;
  status: ToolStatus;
  startedAt?: number;
  endedAt?: number | null;
  /** 一开始就展开（组件预览用）。 */
  initialOpen?: boolean;
  /** 主会话的工具卡带上：功能（插件视图）按它们拉这次调用的视图。子代理的不带。 */
  threadId?: string;
  itemId?: string;
  /** 工具结果里的图（只有元数据，像素展开时按 `item/image` 取）。 */
  images?: ToolImage[];
}

/**
 * 通用状态（设计：wonder-ui/chat/ToolCardStates，moop / kiln / dray；外形仍按
 * Codex 式的一行，只取状态的标记）：
 * - 运行中：右侧蓝色用时在跳 + 转圈；
 * - 失败：红图标，右侧「执行失败 · 用时」，展开的输出放红框；
 * - 已中断：黄色「已中断」角标，展开的输出放灰框。
 * - 已拒绝（muid）：红色「已拒绝」角标，展开的输出放红框（Dock 报 denied）。
 */
export function ToolCard({ tool, args, output = "", status, startedAt, endedAt, initialOpen = false, threadId, itemId, images = [] }: ToolCardProps) {
  // 功能登记的头部图标（插件工具 → Puzzle）。登记顺序固定，按顺序调 hook。
  const featureIcon = toolCardFeatures().reduce<LucideIcon | null>((found, f) => f.useIcon?.(tool, args) ?? found, null);
  const [open, setOpen] = useState(initialOpen);
  // 展开过一次就保留正文，收起时才有折叠动画可看。
  const [seen, setSeen] = useState(initialOpen);
  const spec = specFor(tool, args, output);
  // 成败都看 Dock 给的状态（dock-core），不按输出猜。
  const failed = status === "failed";
  const interrupted = status === "cancelled";
  const denied = status === "denied";
  const running = status === "running";
  const Icon = failed ? AlertCircle : (featureIcon ?? spec.icon);
  const shots = threadId && itemId && images.length > 0 ? images : [];
  const hasBody = !running && (denied || shots.length > 0 || (!spec.noBody && (output.trim() !== "" || !!spec.bodyWithoutOutput)) || failed);
  const code = failed ? exitCode(output) : undefined;

  return (
    <div className="flex shrink-0 flex-col enter-up">
      <button
        type="button"
        disabled={!hasBody}
        onClick={() => {
          setSeen(true);
          setOpen((v) => !v);
        }}
        className="motion-colors -mx-1.5 flex min-w-0 items-center gap-2 rounded-md px-1.5 py-1.5 text-left enabled:hover:bg-hover"
      >
        {running ? (
          <LoaderCircle size={14} className="shrink-0 animate-spin text-accent" />
        ) : (
          <Icon
            size={14}
            className={`shrink-0 ${failed ? "text-[#dc2626]" : spec.iconColor && !featureIcon ? "" : "text-ink-4"}`}
            style={!failed && spec.iconColor && !featureIcon ? { color: spec.iconColor } : undefined}
          />
        )}
        {spec.summary ? (
          // 有描述（bash / monitor）：标题就是这句话，图标已说明是终端；命令原文悬停看、展开看。
          // 主程序挂在右侧（`find`、`cargo test`），和退出码、状态排在一起。
          <>
            <span className="min-w-0 flex-1 truncate text-[13px] text-ink-2" title={spec.detail}>
              {spec.summary}
            </span>
            {spec.tag && (
              <span className="shrink-0 rounded-[5px] bg-fill px-1.5 py-px font-mono text-[10.5px] text-ink-4">{spec.tag}</span>
            )}
          </>
        ) : (
          <>
            <span className="shrink-0 text-[13px] font-semibold text-ink">{spec.verb}</span>
            {spec.prompt && <span className="shrink-0 font-mono text-xs text-ink-4">{spec.prompt}</span>}
            <span className="min-w-0 truncate font-mono text-xs text-ink-3">{spec.detail}</span>
          </>
        )}
        {spec.note && (typeof spec.note === "string" ? <span className="shrink-0 text-xs text-ink-4">{spec.note}</span> : <span className="flex shrink-0 items-center">{spec.note}</span>)}
        {code && (
          <span className="shrink-0 rounded-[5px] bg-[#fee2e2] px-1.5 font-mono text-[10.5px] font-medium text-[#b91c1c]">{code}</span>
        )}
        <span className="ml-auto flex shrink-0 items-center gap-2">
          {running && startedAt !== undefined && <Ticker since={startedAt} />}
          {failed && (
            <span className="enter-fade text-[11px] font-medium text-[#c0392b]">
              执行失败{startedAt !== undefined && endedAt && endedAt - startedAt >= 100 ? ` · ${formatSeconds(endedAt - startedAt)}` : ""}
            </span>
          )}
          {denied && (
            <span className="enter-fade flex items-center gap-1 rounded-sm border border-[#fecaca] bg-[#fef2f2] px-1.5 py-px text-[11px] font-medium text-[#c0392b]">
              <ShieldX size={11} />
              已拒绝
            </span>
          )}
          {interrupted && (
            <span className="enter-fade flex items-center gap-1 rounded-sm border border-[#f5d48a] bg-[#fff8e6] px-1.5 py-px text-[11px] font-medium text-[#8a5a00]">
              <CircleSlash size={11} />
              已中断
            </span>
          )}
          {hasBody && <ChevronRight size={14} data-open={open} className="rotate-open shrink-0 text-ink-4" />}
        </span>
      </button>
      <div className="fold" data-open={open}>
        <div>
          <div className="pb-1.5 pl-[26px] pt-1">
            {hasBody && (open || seen) &&
              (denied ? (
                <div className="rounded-lg border border-[#fecaca] bg-[#fef6f6] px-3 py-2 text-[12px] text-[#c0392b]">用户拒绝了这次调用</div>
              ) : (failed || interrupted) && tool !== "bash" ? (
                // 失败 / 中断：原样给出 Dock 回的说明（错误原文最要紧），bash 照常按终端画。
                <div
                  className={`whitespace-pre-wrap break-words rounded-lg border px-3 py-2 font-mono text-[11.5px] leading-[1.6] ${
                    failed ? "border-[#fecaca] bg-[#fef6f6] text-[#b91c1c]" : "border-line-strong bg-fill text-ink-3"
                  }`}
                >
                  {output.trim()}
                </div>
              ) : shots.length > 0 && !failed && !interrupted ? (
                <div className="flex flex-col gap-1.5">
                  {!spec.noBody && spec.body(output, args)}
                  <ToolImages threadId={threadId!} itemId={itemId!} images={shots} />
                </div>
              ) : threadId && itemId && !failed && !interrupted ? (
                // 正常完成：功能可以换掉正文（插件视图带「视图 | 原始」切换），不接管就是原样。
                toolCardFeatures().reduce<ReactNode>(
                  (raw, f) => (f.Body ? <f.Body threadId={threadId} itemId={itemId} toolName={tool} args={args} raw={raw} /> : raw),
                  spec.body(output, args),
                )
              ) : (
                spec.body(output, args)
              ))}
          </div>
        </div>
      </div>
      {/* 功能的附件（MCP App 界面）：不用展开、运行中也在。 */}
      {threadId &&
        itemId &&
        toolCardFeatures().map((f, i) => f.Attachment && <f.Attachment key={i} threadId={threadId} itemId={itemId} toolName={tool} args={args} status={status} startedAt={startedAt} />)}
    </div>
  );
}

/** 运行中的用时：蓝色等宽，0.1s 一跳。 */
export function Ticker({ since }: { since: number }) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = window.setInterval(() => setNow(Date.now()), 100);
    return () => window.clearInterval(t);
  }, []);
  return <span className="font-mono text-[11px] font-medium text-accent">{formatSeconds(now - since)}</span>;
}

/** 工具用时：一分钟内带一位小数（0.4s），再长交给 formatElapsed。 */
export function formatSeconds(ms: number): string {
  return ms < 60_000 ? `${(Math.max(0, ms) / 1000).toFixed(1)}s` : formatElapsed(ms);
}

/** 一次工具调用的一行：图标、动词、主参数、描述（子代理卡的「正在读 …」和迷你步骤用）。 */
export function toolLine(
  tool: string,
  args: Record<string, unknown>,
): { icon: LucideIcon; verb: string; detail: string; summary?: string } {
  const { icon, verb, detail, summary } = specFor(tool, args, "");
  return { icon, verb, detail, summary };
}

