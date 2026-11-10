// 差异视图 + 审查批注（code-review 工作流的结果，见 dock-core `diff.ts`）。
// 样式沿用工具卡里的 diff（Edit 卡的增删底色）和对话流里的 Patch Diff 卡；批注像 GitHub 的行内评论，
// 落在它指的那一行下面，左边一道按严重程度着色的竖条。对不上 diff 行的批注挂在文件头下，
// 连文件都对不上的列在最上面。

import { useMemo, useState } from "react";
import { ChevronRight, FileCode2, MessageSquareWarning } from "lucide-react";
import {
  lineKey,
  parseUnifiedDiff,
  placeFindings,
  type DiffFile,
  type ReviewFinding,
  type ReviewSeverity,
} from "dock-core";
import { Markdown } from "../Markdown";

export const SEVERITY_LABEL: Record<ReviewSeverity, string> = { critical: "严重", high: "高", medium: "中", low: "低" };
const SEVERITY_COLOR: Record<ReviewSeverity, { bar: string; pill: string }> = {
  critical: { bar: "bg-[#ff3b30]", pill: "bg-[#fef2f2] text-[#c0392b]" },
  high: { bar: "bg-[#ff9500]", pill: "bg-[#fff4e5] text-[#b25e00]" },
  medium: { bar: "bg-[#e0b000]", pill: "bg-[#fdf8e1] text-[#8a6d00]" },
  low: { bar: "bg-[#aeaeb2]", pill: "bg-[#f2f2f7] text-[#636366]" },
};
const STATUS_LABEL: Record<DiffFile["status"], string | null> = {
  modified: null,
  added: "新增",
  deleted: "删除",
  renamed: "重命名",
};
/** 文件多、又没有批注时默认收起：一次画几千行没必要。 */
const COLLAPSE_LINES = 400;

export function ReviewDiff({ diff, findings }: { diff: string; findings: readonly ReviewFinding[] }) {
  const files = useMemo(() => parseUnifiedDiff(diff), [diff]);
  const placed = useMemo(() => placeFindings(files, findings), [files, findings]);
  const totalLines = files.reduce((n, f) => n + f.hunks.reduce((m, h) => m + h.lines.length, 0), 0);

  if (files.length === 0) {
    return <div className="flex flex-1 items-center justify-center p-6 text-[13px] text-ink-4">没有改动（diff 是空的）</div>;
  }
  return (
    <div className="flex flex-col gap-3">
      <Summary findings={findings} files={files.length} />
      {placed.unplaced.length > 0 && (
        <section className="flex flex-col gap-1.5">
          <div className="px-0.5 text-[11px] font-medium text-ink-4">没对上 diff 的批注 · {placed.unplaced.length}</div>
          {placed.unplaced.map((f, i) => (
            <Comment key={i} finding={f} where />
          ))}
        </section>
      )}
      {files.map((file, fi) => {
        const comments = [...placed.byLine.entries()].filter(([k]) => k.startsWith(`${fi}:`)).reduce((n, [, v]) => n + v.length, 0) + (placed.byFile.get(fi)?.length ?? 0);
        return (
          <FileBlock
            key={`${file.path}-${fi}`}
            file={file}
            index={fi}
            byLine={placed.byLine}
            fileComments={placed.byFile.get(fi) ?? []}
            comments={comments}
            startOpen={comments > 0 || totalLines <= COLLAPSE_LINES}
          />
        );
      })}
    </div>
  );
}

function Summary({ findings, files }: { findings: readonly ReviewFinding[]; files: number }) {
  const counts = (["critical", "high", "medium", "low"] as const).map((s) => [s, findings.filter((f) => f.severity === s).length] as const).filter(([, n]) => n > 0);
  return (
    <div className="flex flex-wrap items-center gap-2 text-[12px] text-ink-3">
      <MessageSquareWarning size={14} className="text-ink-4" />
      {findings.length === 0 ? (
        <span>没有批注</span>
      ) : (
        <span>
          {findings.length} 条批注
          {counts.map(([s, n]) => (
            <span key={s} className={`ml-1.5 rounded-full px-1.5 py-px text-[11px] font-medium ${SEVERITY_COLOR[s].pill}`}>
              {SEVERITY_LABEL[s]} {n}
            </span>
          ))}
        </span>
      )}
      <span className="text-ink-5">·</span>
      <span>{files} 个文件</span>
    </div>
  );
}

function FileBlock({
  file,
  index,
  byLine,
  fileComments,
  comments,
  startOpen,
}: {
  file: DiffFile;
  index: number;
  byLine: Map<string, ReviewFinding[]>;
  fileComments: ReviewFinding[];
  comments: number;
  startOpen: boolean;
}) {
  const [open, setOpen] = useState(startOpen);
  const status = STATUS_LABEL[file.status];
  return (
    <section className="md-surface flex flex-col overflow-hidden rounded-[10px] border border-[#e5e5ea] bg-white">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className="sticky top-0 z-[1] flex h-9 shrink-0 items-center gap-2 border-b border-[#ececf0] bg-[#f5f5f7] px-3 text-left"
      >
        <ChevronRight size={13} className="rotate-open shrink-0 text-ink-4" data-open={open} />
        <FileCode2 size={13} className="shrink-0 text-[#636366]" />
        {/* 窄的时候先截目录、文件名留全（以前整条从尾巴截，只剩「S…」）。 */}
        <span
          className="flex min-w-0 items-baseline font-mono text-[11.5px]"
          title={file.oldPath && file.oldPath !== file.path ? `${file.oldPath} → ${file.path}` : file.path}
        >
          {file.path.includes("/") && (
            <span className="min-w-0 truncate text-[#8e8e93]">{file.path.slice(0, file.path.lastIndexOf("/") + 1)}</span>
          )}
          <span className="max-w-full shrink-0 truncate text-[#3a3a3c]">{file.path.slice(file.path.lastIndexOf("/") + 1)}</span>
        </span>
        {status && <span className="shrink-0 rounded bg-[#e8e8ec] px-1.5 text-[10.5px] text-[#636366]">{status}</span>}
        <span className="shrink-0 font-mono text-[11px]">
          <span className="text-[#16a34a]">+{file.additions}</span> <span className="text-[#dc2626]">−{file.deletions}</span>
        </span>
        <span className="flex-1" />
        {comments > 0 && (
          <span className="flex shrink-0 items-center gap-1 rounded-full bg-[#fff4e5] px-1.5 text-[11px] font-medium text-[#b25e00]">
            <MessageSquareWarning size={11} />
            {comments}
          </span>
        )}
      </button>
      <div className="fold" data-open={open} aria-hidden={!open}>
        <div>
          {fileComments.length > 0 && (
            <div className="flex flex-col gap-1.5 border-b border-[#ececf0] bg-[#fafafa] px-3 py-2">
              {fileComments.map((f, i) => (
                <Comment key={i} finding={f} where={f.line > 0} />
              ))}
            </div>
          )}
          {file.binary ? (
            <div className="px-3 py-2 text-[12px] text-ink-4">二进制文件，不显示内容</div>
          ) : (
            <div className="thin-scroll overflow-x-auto font-mono text-[11.5px] leading-[1.6]">
              {file.hunks.map((hunk, hi) => (
                <div key={hi}>
                  <div className="min-w-max bg-[#f0f4fa] px-3 py-0.5 text-[11px] text-[#6e7781]">{hunk.header}</div>
                  {hunk.lines.map((line, li) => {
                    const here = line.newNo !== null ? byLine.get(lineKey(index, line.newNo)) : undefined;
                    return (
                      <div key={li}>
                        <div className={`flex min-w-max ${line.kind === "del" ? "bg-[#fdecec]" : line.kind === "add" ? "bg-[#e9f7ee]" : ""} ${here ? "outline outline-1 -outline-offset-1 outline-[#ffd8a8]" : ""}`}>
                          <span className="w-10 shrink-0 select-none pr-1.5 text-right text-ink-5">{line.oldNo ?? ""}</span>
                          <span className="w-10 shrink-0 select-none pr-1.5 text-right text-ink-5">{line.newNo ?? ""}</span>
                          <span className={`w-4 shrink-0 select-none text-center ${line.kind === "del" ? "text-[#c0392b]" : line.kind === "add" ? "text-[#1a7f37]" : "text-ink-5"}`}>
                            {line.kind === "del" ? "−" : line.kind === "add" ? "+" : " "}
                          </span>
                          <span className={`whitespace-pre pr-4 ${line.kind === "del" ? "text-[#8f1d1d]" : line.kind === "add" ? "text-[#14532d]" : "text-ink-3"}`}>{line.text || " "}</span>
                        </div>
                        {here && (
                          <div className="flex flex-col gap-1.5 border-y border-[#ececf0] bg-[#fafafa] py-2 pl-[88px] pr-3 font-sans">
                            {here.map((f, i) => (
                              <Comment key={i} finding={f} />
                            ))}
                          </div>
                        )}
                      </div>
                    );
                  })}
                </div>
              ))}
            </div>
          )}
        </div>
      </div>
    </section>
  );
}

/** 一条批注。`where`：写上文件和行（挂在文件头或单列时）。 */
export function Comment({ finding, where = false }: { finding: ReviewFinding; where?: boolean }) {
  const color = SEVERITY_COLOR[finding.severity];
  return (
    <div className="enter-fade relative flex max-w-[760px] flex-col gap-1 overflow-hidden rounded-lg border border-[#e5e5ea] bg-white py-2 pl-4 pr-3 shadow-[0_1px_2px_rgba(0,0,0,0.04)]">
      <span className={`absolute bottom-0 left-0 top-0 w-[3px] ${color.bar}`} />
      <div className="flex min-w-0 flex-wrap items-center gap-1.5">
        <span className={`shrink-0 rounded-full px-1.5 py-px text-[10.5px] font-semibold ${color.pill}`}>{SEVERITY_LABEL[finding.severity]}</span>
        <span className="min-w-0 text-[12.5px] font-semibold text-ink">{finding.title}</span>
        {where && (
          <span className="font-mono text-[11px] text-ink-4">
            {finding.file}
            {finding.line > 0 ? `:${finding.line}` : ""}
          </span>
        )}
        {finding.angles.length > 0 && <span className="text-[11px] text-ink-4">{finding.angles.join("、")}</span>}
      </div>
      {finding.detail && (
        <div className="text-[12.5px] leading-relaxed text-[#3a3a3c]">
          <Markdown text={finding.detail} size="card" />
        </div>
      )}
      {finding.suggestion && (
        <div className="text-[12.5px] leading-relaxed text-[#3a3a3c]">
          <Markdown text={`**建议**：${finding.suggestion}`} size="card" />
        </div>
      )}
      {finding.note && <div className="text-[11.5px] text-ink-4">核实：{finding.note}</div>}
    </div>
  );
}
