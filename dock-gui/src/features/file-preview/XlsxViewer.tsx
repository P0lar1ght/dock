// xlsx / xlsm / xls / ods 看表：底下一排工作表标签，像 Excel 那样列字母 + 行号。
//
// 字节分块拉、直接灌进 Worker（主线程不拼整份）；拉的时候显示真实进度。
// Worker 先回前 200 行（首屏），再把整张表分批补上；切到别的工作表才解析那一张，解析过的留着。
//
// 能改（.xlsx / .xlsm，宿主点了「编辑」）：只改格子的值（公式格不让改）。存的时候 Worker 在原来的
// zip 里只换改过的格子（样式、图表、别的表原样留着，见 xlsxPatch.ts），Dock 存好了才算数。

import { useCallback, useEffect, useRef, useState } from "react";
import { useApp } from "../../dock/store";
import { canEditBytes, saveBytes } from "../../dock/fsEdit";
import type { FilePreviewProps } from "../registry";
import { formatSize } from "./bytes";
import { columnName, Grid, type GridEdit } from "./Grid";
import { TableEdits } from "./tableEdit";
import { canReadRaw, FileChangedError, pumpFile } from "./stream";
import { count, errorText, Failed, Loading } from "./ui";
import type { FromXlsxWorker, Merge, ToXlsxWorker } from "./xlsx.worker";

const MAX_BYTES = 100 * 1024 * 1024;
const FLUSH_MS = 120;

interface Sheet {
  rows: string[][];
  cols: number;
  /** 整张解析完了（之前只有首屏的行）。 */
  done: boolean;
  merges: Merge[];
  numeric: boolean[];
  /** 公式格（`r * 16384 + c`）：不让改。 */
  formulas: Set<number>;
  edits: TableEdits;
  error?: string;
}

type Phase =
  | { kind: "download"; loaded: number; total: number }
  | { kind: "parse" }
  | { kind: "ready" }
  | { kind: "error"; message: string };

export function XlsxViewer({ threadId, file, onStatus, edit }: FilePreviewProps) {
  const { remote } = useApp();
  const [phase, setPhase] = useState<Phase>({ kind: "download", loaded: 0, total: file.size });
  const [names, setNames] = useState<string[]>([]);
  const [active, setActive] = useState(0);
  const [version, setVersion] = useState(0);
  const [retry, setRetry] = useState(0);
  const sheets = useRef(new Map<number, Sheet>());
  const worker = useRef<Worker | null>(null);
  /** 等 Worker 回改好的字节。 */
  const saves = useRef(new Map<number, { resolve: (b: Uint8Array) => void; reject: (e: Error) => void }>());
  const [hint, setHint] = useState<string | null>(null);

  useEffect(() => {
    sheets.current = new Map();
    setNames([]);
    setActive(0);
    if (file.size > MAX_BYTES) {
      setPhase({ kind: "error", message: `文件有 ${formatSize(file.size)}，超过预览上限 100 MB。可以让 agent 用工具读它。` });
      return;
    }
    if (!canReadRaw()) {
      setPhase({ kind: "error", message: "连着的 Dock 版本不支持读表格文件，升级后再看。" });
      return;
    }
    setPhase({ kind: "download", loaded: 0, total: file.size });
    const ctl = new AbortController();
    const w = new Worker(new URL("./xlsx.worker.ts", import.meta.url), { type: "module" });
    worker.current = w;
    const send = (m: ToXlsxWorker) => w.postMessage(m);

    let timer = 0;
    const flush = () => {
      timer = 0;
      setVersion((v) => v + 1);
    };
    const schedule = (now: boolean) => {
      if (now) {
        if (timer) window.clearTimeout(timer);
        flush();
      } else if (!timer) timer = window.setTimeout(flush, FLUSH_MS);
    };
    const sheetOf = (i: number) => {
      let s = sheets.current.get(i);
      if (!s) {
        s = newSheet(() => schedule(true));
        sheets.current.set(i, s);
      }
      return s;
    };

    w.onmessage = (e: MessageEvent<FromXlsxWorker>) => {
      const m = e.data;
      if (m.t === "book") setNames(m.sheets);
      else if (m.t === "rows") {
        const s = sheetOf(m.sheet);
        const first = s.rows.length === 0;
        for (let i = 0; i < m.rows.length; i++) s.rows[m.from + i] = m.rows[i];
        s.cols = Math.max(s.cols, m.cols);
        setPhase((p) => (p.kind === "parse" ? { kind: "ready" } : p));
        schedule(first);
      } else if (m.t === "sheetDone") {
        const s = sheetOf(m.sheet);
        s.rows.length = m.rows;
        s.cols = m.cols;
        s.merges = m.merges;
        s.numeric = m.numeric;
        s.formulas = new Set(m.formulas);
        s.done = true;
        setPhase((p) => (p.kind === "parse" ? { kind: "ready" } : p));
        schedule(true);
      } else if (m.t === "saved") {
        saves.current.get(m.id)?.resolve(new Uint8Array(m.bytes));
        saves.current.delete(m.id);
      } else if (m.t === "saveError") {
        saves.current.get(m.id)?.reject(new Error(m.message));
        saves.current.delete(m.id);
      } else if (m.t === "error") {
        const s = sheetOf(m.sheet);
        s.error = m.message;
        s.done = true;
        setPhase((p) => (p.kind === "parse" ? { kind: "ready" } : p));
        schedule(true);
      }
    };
    w.onerror = (e) => setPhase({ kind: "error", message: e.message || "解析失败" });

    send({ t: "begin", size: file.size });
    let lastStatus = 0;
    pumpFile({
      threadId,
      path: file.path,
      remote: !!remote,
      signal: ctl.signal,
      onChunk: (c) => {
        send({ t: "chunk", offset: c.offset, data: c.data });
        const now = performance.now();
        if (now - lastStatus > FLUSH_MS) {
          lastStatus = now;
          setPhase({ kind: "download", loaded: c.offset + (c.data.length / 4) * 3, total: c.size });
        }
      },
    })
      .then(() => {
        if (ctl.signal.aborted) return;
        setPhase({ kind: "parse" });
        sheetOf(0);
        send({ t: "sheet", index: 0 });
      })
      .catch((e) => {
        if (ctl.signal.aborted) return;
        setPhase({
          kind: "error",
          message: e instanceof FileChangedError ? "读的时候文件被改了，重试一下。" : errorText(e),
        });
      });
    return () => {
      ctl.abort();
      w.terminate();
      worker.current = null;
      if (timer) window.clearTimeout(timer);
    };
  }, [threadId, file.path, retry]); // 文件变了宿主会换 key 重来（自己存的不重来）

  const pick = (i: number) => {
    setActive(i);
    if (!sheets.current.has(i)) {
      sheets.current.set(i, newSheet(() => setVersion((v) => v + 1)));
      worker.current?.postMessage({ t: "sheet", index: i } satisfies ToXlsxWorker);
    }
  };

  const sheet = sheets.current.get(active);
  const rowCount = sheet?.rows.length ?? 0;

  // ---- 编辑 ----

  const editingNow = !!edit?.active;
  const editRef = useRef(edit);
  editRef.current = edit;
  const anyDirty = () => [...sheets.current.values()].some((s) => s.edits.dirty());
  useEffect(() => editRef.current?.onDirty(anyDirty()));

  const ext = file.path.slice(file.path.lastIndexOf(".") + 1).toLowerCase();
  const editBlock =
    ext !== "xlsx" && ext !== "xlsm"
      ? "面板里只能改 .xlsx / .xlsm（.xls / .ods 是另外的格式）。"
      : !canEditBytes()
        ? "连着的 Dock 版本存不了表格文件，升级后再改。"
        : phase.kind !== "ready" || !sheet?.done
          ? "还在读，读完才能改。"
          : null;
  useEffect(() => edit?.onEditable(editBlock), [editBlock]);

  const baseRef = useRef(file.modifiedMs);
  baseRef.current = file.modifiedMs;
  const saveSeq = useRef(0);
  const save = useCallback(
    async (force: boolean) => {
      const edits: { sheet: number; r: number; c: number; text: string }[] = [];
      for (const [index, s] of sheets.current) for (const p of s.edits.pending()) edits.push({ sheet: index, r: p.ri, c: p.c, text: p.text });
      if (edits.length === 0) return { path: file.path, size: file.size, modifiedMs: file.modifiedMs };
      const id = ++saveSeq.current;
      const bytes = await new Promise<Uint8Array>((resolve, reject) => {
        saves.current.set(id, { resolve, reject });
        worker.current?.postMessage({ t: "save", id, edits } satisfies ToXlsxWorker);
      });
      const saved = await saveBytes(threadId, file.path, bytes, force ? undefined : baseRef.current, { remote: !!remote });
      worker.current?.postMessage({ t: "commit", id } satisfies ToXlsxWorker);
      for (const s of sheets.current.values()) s.edits.markSaved();
      return saved;
    },
    [threadId, file.path, file.size, file.modifiedMs, remote],
  );
  useEffect(() => {
    if (!editingNow) return;
    editRef.current?.bind(save);
    return () => editRef.current?.bind(null);
  }, [editingNow, save]);

  useEffect(() => {
    if (!hint) return;
    const t = window.setTimeout(() => setHint(null), 2200);
    return () => window.clearTimeout(t);
  }, [hint]);

  const gridEdit: GridEdit | null =
    editingNow && sheet?.done && !sheet.error
      ? {
          canEdit: (ri, c) => !sheet.formulas.has(ri * 16384 + c),
          onCommit: (ri, c, text) => sheet.edits.setCell(ri, c, text),
          changed: sheet.edits.changed(),
          onUndo: () => sheet.edits.undo(),
          onRedo: () => sheet.edits.redo(),
          onBlocked: (ri, c) => setHint(`${columnName(c)}${ri + 1} 是公式，面板里不能改（改它引用的格子就行）。`),
        }
      : null;

  useEffect(() => {
    if (phase.kind !== "ready" || !sheet) return onStatus(null);
    const parts = ["Excel", `${count(rowCount)} 行`, `${count(sheet.cols)} 列`];
    if (names.length > 1) parts.push(`${names.length} 张工作表`);
    parts.push(sheet.done ? formatSize(file.size) : "正在解析…");
    onStatus(parts.join(" · "));
  }, [phase, sheet, sheet?.done, rowCount, names.length, file.size, onStatus, version]);

  if (phase.kind === "error")
    return <Failed title="读不了这个表格" text={phase.message} onRetry={() => setRetry((n) => n + 1)} />;
  if (phase.kind === "download") return <Loading label="正在读文件…" loaded={phase.loaded} total={phase.total} />;

  return (
    <div className="relative flex min-h-0 flex-1 flex-col">
      {hint && (
        <div className="glass-popover pop-in absolute left-1/2 top-3 z-10 -translate-x-1/2 rounded-full border border-line-strong bg-white px-3 py-1 text-xs text-ink-2 shadow-[0px_4px_14px_rgba(0,0,0,0.10)]">
          {hint}
        </div>
      )}
      {!sheet || (sheet.rows.length === 0 && !sheet.done) ? (
        <Loading label="正在解析…" />
      ) : sheet.error ? (
        <Failed title="这张表解析失败" text={sheet.error} />
      ) : sheet.rows.length === 0 ? (
        <Failed title="空工作表" text="这张表里没有数据。" />
      ) : (
        <Grid
          key={active}
          rows={sheet.rows}
          version={version}
          start={0}
          cols={sheet.cols}
          header="letters"
          numeric={sheet.numeric}
          merges={sheet.merges}
          edit={gridEdit}
        />
      )}
      {names.length > 0 && (
        <div className="thin-scroll flex h-8 shrink-0 items-stretch gap-px overflow-x-auto border-t border-line-soft bg-pane px-2">
          {names.map((name, i) => (
            <button
              key={i}
              type="button"
              onClick={() => pick(i)}
              className={`motion-colors -mt-px shrink-0 border-t-2 px-3 text-xs ${
                i === active ? "border-accent bg-white font-medium text-ink" : "border-transparent text-ink-3 hover:bg-hover"
              }`}
            >
              {name}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

function newSheet(onChange: () => void): Sheet {
  const rows: string[][] = [];
  const sheet: Sheet = { rows, cols: 0, done: false, merges: [], numeric: [], formulas: new Set(), edits: null! };
  // 在右边 / 下面的空格里填了：表跟着变宽变长。
  sheet.edits = new TableEdits(rows, () => {
    sheet.cols = rows.reduce((m, r) => Math.max(m, r?.length ?? 0), sheet.cols);
    onChange();
  });
  return sheet;
}
