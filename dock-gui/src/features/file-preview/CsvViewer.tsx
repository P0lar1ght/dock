// CSV / TSV 看表。第一行当表头。
//
// - 小文件（宿主读全了、不到 256 KB）：主线程直接解析，不起 Worker。
// - 大文件：宿主已经读到的前 512 KB 先画出来，后面从那个字节接着分块拉，交给 Worker 解析，
//   边到边画；底栏写已读多少。
// - 宿主读不出文本的（GBK 之类不是 UTF-8 的）：从头分块拉，Worker 认编码。
// - 到 100 万行或 100 MB 就停，底栏说明。
//
// 能改（宿主点了「编辑」）：改格子、插删行列、撤销；存回去照原来的分隔符、换行、BOM，
// 只改内容。不是 UTF-8 的（GBK 之类）存成 UTF-8 带 BOM（Excel 能直接打开），编辑时提示。
// 只读了一部分（太大被截住）的不能改：存回去会丢后面的行。

import { useCallback, useEffect, useMemo, useRef, useState, type MouseEvent as ReactMouseEvent } from "react";
import { ArrowDownToLine, ArrowLeftToLine, ArrowRightToLine, ArrowUpToLine, Trash2 } from "lucide-react";
import { useApp } from "../../dock/store";
import { canEditBytes, saveBytes, saveFile } from "../../dock/fsEdit";
import { ContextMenu, type MenuEntry } from "../../components/SessionMenu";
import type { FilePreviewProps } from "../registry";
import { formatSize } from "./bytes";
import { DelimitedParser, numericColumns, serializeDelimited, sniffEol, sniffSeparator, type CsvFormat } from "./csv";
import type { FromCsvWorker, ToCsvWorker } from "./csv.worker";
import { Grid, type GridEdit } from "./Grid";
import { TableEdits } from "./tableEdit";
import { canReadRaw, FileChangedError, pumpFile } from "./stream";
import { count, errorText, Failed, Loading } from "./ui";

const SMALL = 256 * 1024;
const MAX_ROWS = 1_000_000;
const MAX_BYTES = 100 * 1024 * 1024;
/** 边加载边画：最多这么久重画一次。 */
const FLUSH_MS = 120;

type Phase =
  | { kind: "loading" }
  | { kind: "streaming"; loaded: number; total: number }
  | { kind: "done"; capped: "rows" | "bytes" | "old" | null }
  | { kind: "error"; message: string };

/** 老 Dock 只能按文本写：一次发得过去的才能存。 */
const TEXT_SAVE_MAX = 3 * 1024 * 1024;

type RowAction = "row-above" | "row-below" | "row-delete" | "col-left" | "col-right" | "col-delete";

export function CsvViewer({ threadId, file, onStatus, edit }: FilePreviewProps) {
  const { remote } = useApp();
  const tsv = file.path.toLowerCase().endsWith(".tsv");
  const [rows, setRows] = useState<string[][]>([]);
  const [version, setVersion] = useState(0);
  const [cols, setCols] = useState(0);
  const [phase, setPhase] = useState<Phase>({ kind: "loading" });
  const [info, setInfo] = useState<{ sep: string; encoding: string } | null>(null);
  const [retry, setRetry] = useState(0);
  const colsRef = useRef(0);
  /** 原文件的格式（存回去照着来）。 */
  const fmt = useRef<CsvFormat | null>(null);

  useEffect(() => {
    const out: string[][] = [];
    colsRef.current = 0;
    fmt.current = null;
    setRows(out);
    setCols(0);
    setVersion(0);
    setInfo(null);
    setPhase({ kind: "loading" });

    const add = (batch: string[][]) => {
      for (const row of batch) {
        out.push(row);
        if (row.length > colsRef.current) colsRef.current = row.length;
      }
    };
    let timer = 0;
    let first = true;
    const flush = () => {
      timer = 0;
      setCols(colsRef.current);
      setVersion((v) => v + 1);
    };
    // 第一批立刻画，之后攒一攒再画。
    const schedule = () => {
      if (first) {
        first = false;
        flush();
      } else if (!timer) timer = window.setTimeout(flush, FLUSH_MS);
    };

    const text = file.text;
    if (text !== undefined && !file.truncated && text.length <= SMALL) {
      const sep = tsv ? "\t" : sniffSeparator(text);
      const parser = new DelimitedParser(sep);
      add(parser.push(text));
      add(parser.end());
      setInfo({ sep, encoding: "utf-8" });
      fmt.current = {
        sep,
        encoding: "utf-8",
        bom: text.charCodeAt(0) === 0xfeff,
        eol: sniffEol(text),
        trailingNewline: /[\r\n]$/.test(text),
      };
      flush();
      setPhase({ kind: "done", capped: null });
      return;
    }

    const ctl = new AbortController();
    const worker = new Worker(new URL("./csv.worker.ts", import.meta.url), { type: "module" });
    const send = (m: ToCsvWorker) => worker.postMessage(m);
    let capped: "rows" | "bytes" | "old" | null = null;
    worker.onmessage = (e: MessageEvent<FromCsvWorker>) => {
      const m = e.data;
      if (m.t === "rows") {
        add(m.rows);
        schedule();
      } else if (m.t === "info") {
        setInfo({ sep: m.sep, encoding: m.encoding });
        fmt.current = { sep: m.sep, encoding: m.encoding, bom: m.bom, eol: "\n", trailingNewline: true };
      }
      else if (m.t === "capped") {
        capped = "rows";
        ctl.abort();
      } else if (m.t === "done") {
        if (fmt.current) fmt.current = { ...fmt.current, eol: m.eol, trailingNewline: m.trailingNewline };
        if (timer) window.clearTimeout(timer);
        flush();
        setPhase({ kind: "done", capped });
      }
    };
    worker.onerror = (e) => setPhase({ kind: "error", message: e.message || "解析失败" });

    send({ t: "start", sep: tsv ? "\t" : null, encoding: text !== undefined ? "utf-8" : null, maxRows: MAX_ROWS + 1 });
    let from = 0;
    if (text !== undefined) {
      send({ t: "text", text });
      from = new TextEncoder().encode(text).length;
    }
    if (text !== undefined && !file.truncated) {
      // 宿主已经读全了（256–512 KB），只是放到 Worker 里解析。
      send({ t: "end" });
    } else if (!canReadRaw()) {
      // 老 Dock：只有宿主读到的那 512 KB。
      if (text === undefined) setPhase({ kind: "error", message: "这个文件不是 UTF-8 文本；升级 Dock 后能按编码读。" });
      else {
        capped = "old";
        send({ t: "end" });
      }
    } else {
      setPhase({ kind: "streaming", loaded: from, total: file.size });
      let lastStatus = 0;
      pumpFile({
        threadId,
        path: file.path,
        from,
        limit: MAX_BYTES - from,
        remote: !!remote,
        signal: ctl.signal,
        onChunk: (c) => {
          send({ t: "bytes", data: c.data });
          const now = performance.now();
          if (now - lastStatus > FLUSH_MS) {
            lastStatus = now;
            setPhase({ kind: "streaming", loaded: c.offset + (c.data.length / 4) * 3, total: c.size });
          }
        },
      })
        .then(({ size, end }) => {
          if (capped) return; // 行数先到上限：Worker 已经收尾了。
          if (end < size && !ctl.signal.aborted) capped = "bytes";
          send({ t: "end" });
        })
        .catch((e) => {
          if (ctl.signal.aborted) return;
          setPhase({
            kind: "error",
            message: e instanceof FileChangedError ? "读的时候文件被改了，重试一下。" : errorText(e),
          });
        });
    }
    return () => {
      ctl.abort();
      worker.terminate();
      if (timer) window.clearTimeout(timer);
    };
  }, [threadId, file.path, retry]); // 文件变了宿主会换 key 重来（自己存的不重来）

  // ---- 编辑 ----

  const editingNow = !!edit?.active;
  const [, setTick] = useState(0);
  const edits = useMemo(
    () =>
      new TableEdits(rows, () => {
        colsRef.current = rows.reduce((m, r) => Math.max(m, r.length), 0);
        setCols(colsRef.current);
        setVersion((v) => v + 1);
        setTick((t) => t + 1);
      }),
    [rows],
  );
  const editRef = useRef(edit);
  editRef.current = edit;
  useEffect(() => editRef.current?.onDirty(edits.dirty()));

  // 能不能改：读完、没被截、Dock 存得了。
  const capped = phase.kind === "done" ? phase.capped : null;
  const editBlock =
    phase.kind !== "done"
      ? "还在读，读完才能改。"
      : capped
        ? "文件太大，只载入了一部分，不能在面板里改（存回去会丢后面的行）。"
        : !canEditBytes() && file.size > TEXT_SAVE_MAX
          ? "连着的 Dock 版本存不了这么大的表格，升级后再改。"
          : null;
  useEffect(() => edit?.onEditable(editBlock), [editBlock]);

  const baseRef = useRef(file.modifiedMs);
  baseRef.current = file.modifiedMs;
  const save = useCallback(
    async (force: boolean) => {
      const f = fmt.current ?? { sep: tsv ? "\t" : ",", encoding: "utf-8", bom: false, eol: "\n" as const, trailingNewline: true };
      // 不是 UTF-8 的存成 UTF-8 带 BOM：Excel 认 BOM，不会当乱码。
      const bom = f.bom || f.encoding !== "utf-8" ? "\ufeff" : "";
      const text = bom + serializeDelimited(rows, f);
      const base = force ? undefined : baseRef.current;
      const saved = canEditBytes()
        ? await saveBytes(threadId, file.path, new TextEncoder().encode(text), base, { remote: !!remote })
        : await saveFile(threadId, file.path, text, base);
      edits.markSaved();
      if (fmt.current && f.encoding !== "utf-8") fmt.current = { ...f, encoding: "utf-8", bom: true };
      return saved;
    },
    [rows, edits, threadId, file.path, remote, tsv],
  );
  useEffect(() => {
    if (!editingNow) return;
    editRef.current?.bind(save);
    return () => editRef.current?.bind(null);
  }, [editingNow, save]);

  const [menu, setMenu] = useState<{ x: number; y: number; ri?: number; c?: number } | null>(null);
  const width = Math.max(cols, 1);
  const runMenu = (a: RowAction) => {
    if (!menu) return;
    const { ri, c } = menu;
    if (a === "row-above" && ri !== undefined) edits.insertRow(Math.max(1, ri), width);
    else if (a === "row-below" && ri !== undefined) edits.insertRow(ri + 1, width);
    else if (a === "row-delete" && ri !== undefined) edits.deleteRow(ri);
    else if (a === "col-left" && c !== undefined) edits.insertCol(c);
    else if (a === "col-right" && c !== undefined) edits.insertCol(c + 1);
    else if (a === "col-delete" && c !== undefined) edits.deleteCol(c);
  };
  const menuEntries = (m: { ri?: number; c?: number }): MenuEntry<RowAction>[] => {
    const realRow = m.ri !== undefined && m.ri < rows.length;
    return [
      m.ri !== undefined && m.ri > 0 ? { action: "row-above", icon: ArrowUpToLine, label: "在上方插入一行" } : null,
      m.ri !== undefined ? { action: "row-below", icon: ArrowDownToLine, label: "在下方插入一行" } : null,
      realRow && m.ri! > 0 ? { action: "row-delete", icon: Trash2, label: "删除这一行", danger: true } : null,
      null,
      m.c !== undefined ? { action: "col-left", icon: ArrowLeftToLine, label: "在左侧插入一列" } : null,
      m.c !== undefined ? { action: "col-right", icon: ArrowRightToLine, label: "在右侧插入一列" } : null,
      m.c !== undefined && m.c < cols ? { action: "col-delete", icon: Trash2, label: "删除这一列", danger: true } : null,
    ];
  };

  const gridEdit: GridEdit | null = editingNow
    ? {
        canEdit: () => true,
        onCommit: (ri, c, text) => edits.setCell(ri, c, text),
        changed: edits.changed(),
        onUndo: () => edits.undo(),
        onRedo: () => edits.redo(),
        onMenu: (e: ReactMouseEvent, at) => setMenu({ x: e.clientX, y: e.clientY, ...at }),
      }
    : null;

  const dataRows = Math.max(0, rows.length - 1);
  const shownRows = Math.min(dataRows, MAX_ROWS);

  useEffect(() => {
    if (phase.kind === "error" || phase.kind === "loading") return onStatus(null);
    const parts = [tsv ? "TSV" : "CSV", `${count(shownRows)} 行`, `${count(cols)} 列`];
    if (info && info.encoding !== "utf-8") parts.push(info.encoding.toUpperCase());
    if (info && !tsv && info.sep !== ",") parts.push(SEP_NAMES[info.sep] ?? "");
    if (phase.kind === "streaming") parts.push(`已读 ${Math.floor((phase.loaded / Math.max(1, phase.total)) * 100)}%`);
    else {
      parts.push(formatSize(file.size));
      if (phase.capped === "rows") parts.push("只载入前 100 万行");
      else if (phase.capped === "bytes") parts.push("只载入前 100 MB");
      else if (phase.capped === "old") parts.push("只显示前 512 KB");
    }
    onStatus(parts.join(" · "));
  }, [phase, shownRows, cols, info, tsv, file.size, onStatus]);

  const header = rows[0];
  // 数字列只看前 1000 行：够判断，也不随加载一直重算。
  const sample = Math.min(dataRows, 1000);
  const numeric = useMemo(() => numericColumns(rows.slice(1, 1 + sample), cols), [rows, sample, cols]);

  if (phase.kind === "error")
    return <Failed title="读不了这个表格" text={phase.message} onRetry={() => setRetry((n) => n + 1)} />;
  if (!header) {
    if (phase.kind === "done") return <Failed title="空表格" text="这个文件里没有数据。" />;
    return <Loading label="正在读表格…" />;
  }
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {editingNow && info && info.encoding !== "utf-8" && (
        <div className="enter-fade shrink-0 border-b border-line-soft bg-[#f5f9ff] px-4 py-1.5 text-[11.5px] text-ink-3">
          这个文件是 {info.encoding.toUpperCase()} 编码；保存时会存成 UTF-8（带 BOM，Excel 能直接打开）。
        </div>
      )}
      <Grid rows={rows} version={version} start={1} cols={cols} header={header} numeric={numeric} edit={gridEdit} />
      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          entries={menuEntries(menu)}
          onAction={runMenu}
          onClose={() => setMenu(null)}
        />
      )}
    </div>
  );
}

const SEP_NAMES: Record<string, string> = { ";": "分号分隔", "\t": "制表符分隔", "|": "竖线分隔" };
