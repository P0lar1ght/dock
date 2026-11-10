// 一轮结束后的改动卡：「已编辑 N 个文件 +x -y · 查看文件 … 撤销 审查」（用户给的设计图）。
// 数据是 Dock 的每轮改动（`turn/changes/*`，只认 agent 的 search_replace / write_file）。
// 「查看文件」展开文件列表，点一行在文件面板打开；「审查」在右侧开这一轮的 diff；
// 「撤销」点两下才动手，之后又被改过的文件不动、在卡里说明。

import { useEffect, useState } from "react";
import { ChevronRight, Diff, FileMinus, FilePen, FilePlus, LoaderCircle } from "lucide-react";
import { turnChangesOf, type TurnFileChange } from "dock-core";
import { store, useTurnChanges } from "../../dock/store";
import { useFeatureHost, type TurnFooterProps } from "../registry";

const STATUS_ICON = { added: FilePlus, modified: FilePen, deleted: FileMinus } as const;
const STATUS_COLOR = { added: "text-[#15803d]", modified: "text-ink-3", deleted: "text-[#c0392b]" } as const;

export function TurnChangesCard({ threadId, turnId }: TurnFooterProps) {
  const changes = turnChangesOf(useTurnChanges(threadId), turnId);
  const host = useFeatureHost();
  const [open, setOpen] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<string | null>(null);

  // 「撤销」点一下变成「确认撤销」，3 秒内不点就回去。
  useEffect(() => {
    if (!confirming) return;
    const t = setTimeout(() => setConfirming(false), 3000);
    return () => clearTimeout(t);
  }, [confirming]);

  if (!changes) return null;
  const n = changes.files.length;

  const revert = async () => {
    if (!confirming) {
      setConfirming(true);
      return;
    }
    setConfirming(false);
    setBusy(true);
    try {
      const r = await store.revertTurn(threadId, turnId);
      if (r.restored.length > 0) host.toast(`已撤销 ${r.restored.length} 个文件的改动`);
      setNote(
        r.conflicts.length > 0
          ? `${r.conflicts.length} 个文件没动：${r.conflicts.map((c) => `${c.path}（${c.reason}）`).join("、")}`
          : null,
      );
    } catch (e) {
      setNote(`撤销失败：${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="md-surface enter-up flex shrink-0 flex-col rounded-[14px] border border-line-strong bg-white">
      <div className="flex items-center gap-3 px-3 py-2.5">
        <span className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-[#f2f2f7] text-ink-3">
          <Diff size={16} strokeWidth={1.75} />
        </span>
        <span className={`shrink-0 text-[13.5px] font-medium ${changes.reverted ? "text-ink-4" : "text-ink"}`}>
          {changes.reverted ? `已撤销 · 编辑过 ${n} 个文件` : `已编辑 ${n} 个文件`}
        </span>
        <span className="flex shrink-0 items-center gap-1.5 font-mono text-[12px]">
          <span className="text-[#15803d]">+{changes.added}</span>
          <span className="text-[#c0392b]">-{changes.removed}</span>
        </span>
        <span className="h-4 w-px shrink-0 bg-line" />
        <button
          type="button"
          onClick={() => setOpen((o) => !o)}
          className="motion-colors flex shrink-0 items-center gap-1 rounded-md px-1.5 py-1 text-[13px] font-medium text-ink-2 hover:bg-hover"
        >
          查看文件
          <ChevronRight size={13} className="rotate-open text-ink-4" data-open={open} />
        </button>
        <div className="min-w-0 flex-1" />
        {!changes.reverted && (
          <button
            type="button"
            disabled={busy}
            title="把这一轮改过的文件恢复成这一轮之前的样子（之后又被改过的不动）"
            onClick={() => void revert()}
            className={`motion-colors flex h-7 shrink-0 items-center gap-1 rounded-md px-2 text-[13px] font-medium hover:bg-hover disabled:opacity-50 ${
              confirming ? "text-[#c0392b]" : "text-ink-2"
            }`}
          >
            {busy && <LoaderCircle size={13} className="animate-spin" />}
            {confirming ? "确认撤销？" : "撤销"}
          </button>
        )}
        <button
          type="button"
          title="在右侧看这一轮的 diff"
          onClick={() => host.openPanel(`changes:turn:${turnId}`)}
          className="motion-colors flex h-7 shrink-0 items-center rounded-lg border border-line-strong bg-white px-3 text-[13px] font-medium text-ink hover:bg-hover active:scale-[0.97]"
        >
          审查
        </button>
      </div>
      <div className="fold" data-open={open} aria-hidden={!open}>
        <div>
          <div className="flex flex-col border-t border-line-soft px-2 py-1.5">
            {changes.files.map((f) => (
              <FileRow key={f.absPath} file={f} threadId={threadId} />
            ))}
          </div>
        </div>
      </div>
      {note && <div className="border-t border-line-soft px-3 py-2 text-[12px] text-[#b25e00]">{note}</div>}
    </div>
  );
}

function FileRow({ file, threadId }: { file: TurnFileChange; threadId: string }) {
  const Icon = STATUS_ICON[file.status];
  const gone = file.status === "deleted";
  return (
    <button
      type="button"
      disabled={gone}
      title={gone ? "这个文件被删了" : "在文件面板打开"}
      onClick={() => store.openFile(threadId, { path: file.path, line: null })}
      className="motion-colors flex h-8 items-center gap-2 rounded-md px-1.5 text-left enabled:hover:bg-hover"
    >
      <Icon size={14} className={`shrink-0 ${STATUS_COLOR[file.status]}`} />
      <span className="min-w-0 flex-1 truncate font-mono text-[12px] text-ink-2">{file.path}</span>
      {file.skipped ? (
        <span className="shrink-0 text-[11px] text-ink-4">{file.skipped}</span>
      ) : (
        <span className="flex shrink-0 items-center gap-1.5 font-mono text-[11.5px]">
          {file.added > 0 && <span className="text-[#15803d]">+{file.added}</span>}
          {file.removed > 0 && <span className="text-[#c0392b]">-{file.removed}</span>}
        </span>
      )}
    </button>
  );
}
