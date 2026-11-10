// 预览器共用的小件：加载卡（真实进度，不假装）、出错卡。

import { FileWarning } from "lucide-react";
import { formatSize } from "./bytes";

/** 居中的加载卡：知道总量就画进度条，否则只写在干什么。 */
export function Loading({ label, loaded, total }: { label: string; loaded?: number; total?: number }) {
  const pct = total && loaded !== undefined ? Math.min(100, (loaded / total) * 100) : null;
  return (
    <div className="enter-fade flex min-h-0 flex-1 flex-col items-center justify-center gap-3 bg-pane">
      <span className="text-[13px] text-ink-3">{label}</span>
      {pct !== null && (
        <>
          <div className="h-1 w-[200px] overflow-hidden rounded-full bg-line">
            <div className="h-full rounded-full bg-accent transition-[width] duration-[var(--dur-base)]" style={{ width: `${pct}%` }} />
          </div>
          <span className="text-[11px] tabular-nums text-ink-4">
            {formatSize(loaded!)} / {formatSize(total!)}
          </span>
        </>
      )}
    </div>
  );
}

export function Failed({ title, text, onRetry }: { title: string; text: string; onRetry?: () => void }) {
  return (
    <div className="enter-fade flex min-w-0 flex-1 flex-col items-center justify-center gap-3 bg-pane px-10">
      <div className="flex size-12 items-center justify-center rounded-full border border-line-strong bg-white">
        <FileWarning size={20} className="text-ink-3" />
      </div>
      <span className="text-[15px] font-semibold text-ink">{title}</span>
      <span className="max-w-[340px] text-center text-[13px] leading-[1.55] text-ink-3">{text}</span>
      {onRetry && (
        <button type="button" onClick={onRetry} className="text-[13px] font-medium text-[#0a6fd6] hover:underline">
          重试
        </button>
      )}
    </div>
  );
}

/** 「12,345」。 */
export const count = (n: number) => n.toLocaleString("en-US");

export function errorText(e: unknown): string {
  const text = e instanceof Error ? e.message : String(e);
  return text.replace(/^fs\/\w+: /, "");
}
