// PDF 预览（pdf.js）。
//
// - 小文件（≤ 4 MB）整份拉；大的按需取段（PDFDataRangeTransport + disableAutoFetch）：
//   先拿开头一块，pdf.js 要哪段再经 fs/read { raw } 取哪段，第一页不用等整份下完。
// - 只画视口附近的页（IntersectionObserver），远了就把画布放掉，几百页也不占内存。
// - 默认适合宽度；底下的小工具条缩放；文字层可以选中复制。
// - 中日韩字体映射（cmaps）、标准字体、wasm 解码器随包带，用到才取（glob 成资源 URL）。

import { useEffect, useLayoutEffect, useRef, useState, type RefObject } from "react";
import { Minus, MoveHorizontal, Plus } from "lucide-react";
import * as pdfjs from "pdfjs-dist";
import type { PDFDocumentLoadingTask, PDFDocumentProxy } from "pdfjs-dist";
import workerUrl from "pdfjs-dist/build/pdf.worker.min.mjs?url";
import "./pdf.css";
import { useApp } from "../../dock/store";
import type { FilePreviewProps } from "../registry";
import { decodeBase64, formatSize } from "./bytes";
import { canReadRaw, FileChangedError, pumpFile, readRange } from "./stream";
import { errorText, Failed, Loading } from "./ui";

pdfjs.GlobalWorkerOptions.workerSrc = workerUrl;

/** 比这小的整份拉；大的按需取段。 */
const WHOLE_MAX = 4 * 1024 * 1024;
/** 按需取段时先给 pdf.js 的开头一块、之后每段多大。 */
const HEAD = 512 * 1024;
const RANGE_CHUNK = 256 * 1024;
const MAX_BYTES = 500 * 1024 * 1024;
const PAGE_GAP = 16;
const MIN_SCALE = 0.25;
const MAX_SCALE = 4;

const ASSETS: Record<string, Record<string, () => Promise<unknown>>> = {
  cMapUrl: import.meta.glob("/node_modules/pdfjs-dist/cmaps/*", { query: "?url", import: "default" }),
  standardFontDataUrl: import.meta.glob("/node_modules/pdfjs-dist/standard_fonts/*", { query: "?url", import: "default" }),
  wasmUrl: import.meta.glob("/node_modules/pdfjs-dist/wasm/*", { query: "?url", import: "default" }),
};
const ASSET_DIRS: Record<string, string> = { cMapUrl: "cmaps", standardFontDataUrl: "standard_fonts", wasmUrl: "wasm" };

/** 字体映射 / 标准字体 / wasm 从随包资源里取（pdf.js 在主线程上调它）。 */
class BundledData {
  constructor(_options: unknown) {}
  async fetch({ kind, filename }: { kind: string; filename: string }): Promise<Uint8Array> {
    const load = ASSETS[kind]?.[`/node_modules/pdfjs-dist/${ASSET_DIRS[kind]}/${filename}`];
    if (!load) throw new Error(`没有随包带 ${filename}`);
    const res = await fetch((await load()) as string);
    return new Uint8Array(await res.arrayBuffer());
  }
}

const COMMON = {
  BinaryDataFactory: BundledData,
  // 只要非空：pdf.js 据此知道有这些数据可取，实际经 BundledData 读。
  cMapUrl: "dock:/cmaps/",
  standardFontDataUrl: "dock:/standard_fonts/",
  wasmUrl: "dock:/wasm/",
  useWorkerFetch: false,
  isEvalSupported: false,
  enableXfa: false,
};

interface Loaded {
  doc: PDFDocumentProxy;
  pages: number;
  /** 第一页在 100% 下的大小：没画到的页先按它占位。 */
  base: { w: number; h: number };
}

type Phase = { kind: "loading"; loaded?: number } | { kind: "ready" } | { kind: "error"; message: string };

export function PdfViewer({ threadId, file, onStatus }: FilePreviewProps) {
  const { remote } = useApp();
  const [phase, setPhase] = useState<Phase>({ kind: "loading" });
  const [loaded, setLoaded] = useState<Loaded | null>(null);
  const [retry, setRetry] = useState(0);
  const [page, setPage] = useState(1);

  useEffect(() => {
    setLoaded(null);
    setPhase({ kind: "loading" });
    if (file.size > MAX_BYTES) {
      setPhase({ kind: "error", message: `文件有 ${formatSize(file.size)}，超过预览上限 500 MB。` });
      return;
    }
    if (!canReadRaw()) {
      setPhase({ kind: "error", message: "连着的 Dock 版本不支持读 PDF，升级后再看。" });
      return;
    }
    const ctl = new AbortController();
    let task: PDFDocumentLoadingTask | null = null;
    const fail = (e: unknown) => {
      if (ctl.signal.aborted) return;
      setPhase({
        kind: "error",
        message: e instanceof FileChangedError ? "读的时候文件被改了，重试一下。" : errorText(e),
      });
    };
    void (async () => {
      if (file.size <= WHOLE_MAX) {
        const buf = new Uint8Array(file.size);
        await pumpFile({
          threadId,
          path: file.path,
          remote: !!remote,
          signal: ctl.signal,
          onChunk: (c) => buf.set(decodeBase64(c.data), c.offset),
        });
        if (ctl.signal.aborted) return;
        task = pdfjs.getDocument({ data: buf, ...COMMON });
      } else {
        const head = await readRange(threadId, file.path, 0, Math.min(HEAD, file.size));
        if (ctl.signal.aborted) return;
        const transport = new pdfjs.PDFDataRangeTransport(file.size, head);
        transport.requestDataRange = (begin: number, end: number) => {
          readRange(threadId, file.path, begin, end).then(
            (bytes) => !ctl.signal.aborted && transport.onDataRange(begin, bytes),
            fail,
          );
        };
        task = pdfjs.getDocument({
          range: transport,
          rangeChunkSize: RANGE_CHUNK,
          disableAutoFetch: true,
          disableStream: true,
          ...COMMON,
        });
      }
      task.onPassword = () => {
        fail(new Error("这个 PDF 设了打开密码，面板里暂时打不开。"));
        void task?.destroy();
      };
      const doc = await task.promise;
      const first = await doc.getPage(1);
      const vp = first.getViewport({ scale: 1 });
      if (ctl.signal.aborted) return;
      setLoaded({ doc, pages: doc.numPages, base: { w: vp.width, h: vp.height } });
      setPhase({ kind: "ready" });
    })().catch(fail);
    return () => {
      ctl.abort();
      void (task as PDFDocumentLoadingTask | null)?.destroy();
    };
  }, [threadId, file.path, retry]); // 文件变了宿主会换 key 重来（自己存的不重来）

  useEffect(() => {
    if (phase.kind !== "ready" || !loaded) return onStatus(null);
    onStatus(`PDF · 第 ${page} / ${loaded.pages} 页 · ${formatSize(file.size)}`);
  }, [phase, loaded, page, file.size, onStatus]);

  if (phase.kind === "error")
    return <Failed title="读不了这个 PDF" text={phase.message} onRetry={() => setRetry((n) => n + 1)} />;
  if (!loaded) return <Loading label="正在打开 PDF…" />;
  return <Pages loaded={loaded} onPage={setPage} />;
}

function Pages({ loaded, onPage }: { loaded: Loaded; onPage: (n: number) => void }) {
  const scroller = useRef<HTMLDivElement>(null);
  const pageEls = useRef<(HTMLDivElement | null)[]>([]);
  const [width, setWidth] = useState(0);
  /** null = 适合宽度。 */
  const [zoom, setZoom] = useState<number | null>(null);

  // 面板宽度：拖分隔线时别每一帧都重画，停一下再算。
  useLayoutEffect(() => {
    const el = scroller.current;
    if (!el) return;
    setWidth(el.clientWidth);
    let timer = 0;
    const ro = new ResizeObserver(() => {
      window.clearTimeout(timer);
      timer = window.setTimeout(() => setWidth(el.clientWidth), 150);
    });
    ro.observe(el);
    return () => {
      ro.disconnect();
      window.clearTimeout(timer);
    };
  }, []);

  const fit = width > 0 ? (width - 48) / loaded.base.w : 1;
  const scale = Math.min(MAX_SCALE, Math.max(MIN_SCALE, zoom ?? fit));

  // 当前页：视口上三分之一处落在哪一页。
  useEffect(() => {
    const el = scroller.current;
    if (!el) return;
    let frame = 0;
    const read = () => {
      frame = 0;
      const y = el.scrollTop + el.clientHeight / 3;
      const els = pageEls.current;
      let lo = 0;
      let hi = loaded.pages - 1;
      while (lo < hi) {
        const mid = (lo + hi + 1) >> 1;
        if ((els[mid]?.offsetTop ?? 0) <= y) lo = mid;
        else hi = mid - 1;
      }
      onPage(lo + 1);
    };
    const onScroll = () => {
      if (!frame) frame = requestAnimationFrame(read);
    };
    el.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      el.removeEventListener("scroll", onScroll);
      if (frame) cancelAnimationFrame(frame);
    };
  }, [loaded, onPage]);

  const step = (factor: number) => setZoom(Math.min(MAX_SCALE, Math.max(MIN_SCALE, scale * factor)));

  return (
    <div className="dock-pdf relative flex min-h-0 flex-1 flex-col bg-[#f2f2f4]">
      <div ref={scroller} className="thin-scroll min-h-0 flex-1 overflow-auto py-6">
        {width > 0 &&
          Array.from({ length: loaded.pages }, (_, i) => (
            <PdfPage
              key={i}
              doc={loaded.doc}
              index={i + 1}
              scale={scale}
              fallback={loaded.base}
              root={scroller}
              elRef={(el) => (pageEls.current[i] = el)}
            />
          ))}
      </div>
      <div className="pointer-events-none absolute inset-x-0 bottom-4 flex justify-center">
        <div className="glass-popover pointer-events-auto flex h-8 items-center gap-0.5 rounded-full border border-line-strong bg-white/95 px-1 shadow-[0px_4px_14px_rgba(0,0,0,0.10)] backdrop-blur">
          <ZoomButton label="缩小" onClick={() => step(1 / 1.25)} icon={Minus} />
          <span className="w-12 text-center text-[11.5px] tabular-nums text-ink-3">{Math.round(scale * 100)}%</span>
          <ZoomButton label="放大" onClick={() => step(1.25)} icon={Plus} />
          <div className="mx-0.5 h-4 w-px bg-line-strong" />
          <ZoomButton label="适合宽度" onClick={() => setZoom(null)} icon={MoveHorizontal} active={zoom === null} />
        </div>
      </div>
    </div>
  );
}

function ZoomButton({
  label,
  onClick,
  icon: Icon,
  active,
}: {
  label: string;
  onClick: () => void;
  icon: typeof Plus;
  active?: boolean;
}) {
  return (
    <button
      type="button"
      title={label}
      aria-label={label}
      onClick={onClick}
      className={`motion-colors flex size-6 items-center justify-center rounded-full ${active ? "bg-fill text-ink" : "text-ink-3 hover:bg-hover"}`}
    >
      <Icon size={13} />
    </button>
  );
}

/** 一页：先按第一页的大小占位；进到视口附近才取页、画画布和文字层，离远了放掉。 */
function PdfPage({
  doc,
  index,
  scale,
  fallback,
  root,
  elRef,
}: {
  doc: PDFDocumentProxy;
  index: number;
  scale: number;
  fallback: { w: number; h: number };
  root: RefObject<HTMLDivElement | null>;
  elRef: (el: HTMLDivElement | null) => void;
}) {
  const box = useRef<HTMLDivElement | null>(null);
  const holder = useRef<HTMLDivElement>(null);
  const [near, setNear] = useState(false);
  const [size, setSize] = useState<{ w: number; h: number } | null>(null);
  const [drawn, setDrawn] = useState(false);

  useEffect(() => {
    const el = box.current;
    if (!el) return;
    const io = new IntersectionObserver(([entry]) => setNear(entry.isIntersecting), {
      root: root.current,
      rootMargin: "150% 0px",
    });
    io.observe(el);
    return () => io.disconnect();
  }, [root]);

  // 离远了：画布和文字层都放掉。
  useEffect(() => {
    if (near) return;
    holder.current?.replaceChildren();
    setDrawn(false);
  }, [near]);

  useEffect(() => {
    if (!near) return;
    let cancelled = false;
    let render: { cancel: () => void } | null = null;
    let text: pdfjs.TextLayer | null = null;
    void (async () => {
      const page = await doc.getPage(index);
      if (cancelled) return;
      const natural = page.getViewport({ scale: 1 });
      setSize({ w: natural.width, h: natural.height });
      const viewport = page.getViewport({ scale });
      const ratio = window.devicePixelRatio || 1;
      const canvas = document.createElement("canvas");
      canvas.width = Math.floor(viewport.width * ratio);
      canvas.height = Math.floor(viewport.height * ratio);
      canvas.style.cssText = "position:absolute;inset:0;width:100%;height:100%";
      const task = page.render({
        canvas,
        viewport,
        transform: ratio === 1 ? undefined : [ratio, 0, 0, ratio, 0, 0],
      });
      render = task;
      await task.promise;
      if (cancelled) return;
      const layer = document.createElement("div");
      layer.className = "textLayer";
      text = new pdfjs.TextLayer({ textContentSource: page.streamTextContent(), container: layer, viewport });
      // 先换上画布（旧倍率的那张留到这一刻，缩放时不闪白）。
      holder.current?.replaceChildren(canvas);
      setDrawn(true);
      await text.render();
      if (!cancelled) holder.current?.append(layer);
    })().catch(() => {
      // 取消（翻走了、缩放了）或这一页坏了：留着占位。
    });
    return () => {
      cancelled = true;
      render?.cancel();
      text?.cancel();
    };
  }, [near, scale, doc, index]);

  const w = (size ?? fallback).w * scale;
  const h = (size ?? fallback).h * scale;
  return (
    <div
      ref={(el) => {
        box.current = el;
        elRef(el);
      }}
      className="glass-keep relative mx-auto bg-white shadow-[0px_6px_20px_rgba(0,0,0,0.10)]"
      style={{ width: w, height: h, marginBottom: PAGE_GAP, ["--scale-factor" as string]: scale, ["--total-scale-factor" as string]: scale }}
    >
      <div ref={holder} className="absolute inset-0" />
      {!drawn && (
        <span className="absolute inset-0 flex items-center justify-center text-xs tabular-nums text-ink-5">{index}</span>
      )}
    </div>
  );
}
