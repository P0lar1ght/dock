// Word / PowerPoint 预览：分块拉字节（真实进度），同时把沙箱页面和排版库备好；
// 字节齐了转交（transfer，不复制）给 iframe 排版。页面怎么拼、安全边界见 frame.ts。

import { useEffect, useRef, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { store, useApp } from "../../dock/store";
import type { FilePreviewProps } from "../registry";
import { decodeBase64, formatSize } from "./bytes";
import { docxSrcdoc, pptxSrcdoc, type FromFrame } from "./frame";
import { canReadRaw, FileChangedError, pumpFile } from "./stream";
import { errorText, Failed, Loading } from "./ui";

const MAX_BYTES = 100 * 1024 * 1024;

type Phase =
  | { kind: "download"; loaded: number; total: number }
  | { kind: "render" }
  /** `done` = 全部画完（PowerPoint 分批画，先到的几页就能看）。 */
  | { kind: "ready"; pages: number; done: boolean }
  | { kind: "error"; message: string };

const KINDS = {
  docx: { srcdoc: docxSrcdoc, label: "Word", what: "文档" },
  pptx: { srcdoc: pptxSrcdoc, label: "PowerPoint", what: "演示文稿" },
};

export const DocxViewer = (props: FilePreviewProps) => <FrameViewer {...props} kind="docx" />;
export const PptxViewer = (props: FilePreviewProps) => <FrameViewer {...props} kind="pptx" />;

function FrameViewer({ threadId, file, onStatus, kind }: FilePreviewProps & { kind: keyof typeof KINDS }) {
  const spec = KINDS[kind];
  const { remote } = useApp();
  const [phase, setPhase] = useState<Phase>({ kind: "download", loaded: 0, total: file.size });
  const [srcdoc, setSrcdoc] = useState<string | null>(null);
  const [retry, setRetry] = useState(0);
  const frame = useRef<HTMLIFrameElement>(null);
  /** 页面就绪 / 字节齐了：两个都到了才发。 */
  const pending = useRef<{ ready: boolean; bytes: ArrayBuffer | null }>({ ready: false, bytes: null });

  const deliver = () => {
    const p = pending.current;
    const win = frame.current?.contentWindow;
    if (!p.ready || !p.bytes || !win) return;
    const data = p.bytes;
    p.bytes = null;
    win.postMessage({ type: "doc", data }, "*", [data]);
  };

  useEffect(() => {
    void spec.srcdoc().then(setSrcdoc, (e) => setPhase({ kind: "error", message: errorText(e) }));
  }, [spec]);

  useEffect(() => {
    const onMessage = (e: MessageEvent) => {
      if (!frame.current || e.source !== frame.current.contentWindow) return;
      const m = e.data as FromFrame & { __dockFrame?: number };
      if (!m || !m.__dockFrame) return;
      if (m.type === "ready") {
        pending.current.ready = true;
        deliver();
      } else if (m.type === "progress") setPhase({ kind: "ready", pages: m.pages, done: false });
      else if (m.type === "rendered") setPhase({ kind: "ready", pages: m.pages, done: true });
      else if (m.type === "error") setPhase({ kind: "error", message: m.message });
      else if (m.type === "link") void openUrl(m.href).catch(store.reportError);
      else if (m.type === "key") {
        const { type: _type, ...init } = m;
        window.dispatchEvent(new KeyboardEvent("keydown", { ...init, bubbles: true, cancelable: true }));
      }
    };
    window.addEventListener("message", onMessage);
    return () => window.removeEventListener("message", onMessage);
  }, []);

  useEffect(() => {
    pending.current = { ready: false, bytes: null };
    if (file.size > MAX_BYTES) {
      setPhase({ kind: "error", message: `文件有 ${formatSize(file.size)}，超过预览上限 100 MB。可以让 agent 用工具读它。` });
      return;
    }
    if (!canReadRaw()) {
      setPhase({ kind: "error", message: `连着的 Dock 版本不支持读${spec.what}，升级后再看。` });
      return;
    }
    setPhase({ kind: "download", loaded: 0, total: file.size });
    const ctl = new AbortController();
    const buf = new Uint8Array(file.size);
    let last = 0;
    pumpFile({
      threadId,
      path: file.path,
      remote: !!remote,
      signal: ctl.signal,
      onChunk: (c) => {
        const bytes = decodeBase64(c.data);
        buf.set(bytes, c.offset);
        const now = performance.now();
        if (now - last > 100) {
          last = now;
          setPhase({ kind: "download", loaded: c.offset + bytes.length, total: c.size });
        }
      },
    })
      .then(() => {
        if (ctl.signal.aborted) return;
        setPhase({ kind: "render" });
        pending.current.bytes = buf.buffer;
        deliver();
      })
      .catch((e) => {
        if (ctl.signal.aborted) return;
        setPhase({
          kind: "error",
          message: e instanceof FileChangedError ? "读的时候文件被改了，重试一下。" : errorText(e),
        });
      });
    return () => ctl.abort();
  }, [threadId, file.path, retry]); // 文件变了宿主会换 key 重来（自己存的不重来）

  useEffect(() => {
    if (phase.kind !== "ready") return onStatus(null);
    const pages = phase.done ? `${phase.pages} 页` : `已画 ${phase.pages} 页…`;
    onStatus(`${spec.label} · ${pages} · ${formatSize(file.size)}`);
  }, [phase, file.size, onStatus, spec]);

  if (phase.kind === "error")
    return <Failed title={`读不了这个${spec.what}`} text={phase.message} onRetry={() => setRetry((n) => n + 1)} />;

  return (
    <div className="relative flex min-h-0 flex-1 flex-col bg-[#f2f2f4]">
      {srcdoc && (
        <iframe
          // 换文件时宿主整个重挂这个组件；出错时 iframe 已经卸了，重试会重新挂。
          ref={frame}
          title={file.path}
          srcDoc={srcdoc}
          sandbox="allow-scripts"
          className={`min-h-0 flex-1 border-0 transition-opacity duration-[var(--dur-base)] ${
            phase.kind === "ready" ? "opacity-100" : "opacity-0"
          }`}
        />
      )}
      {phase.kind !== "ready" && (
        <div className="absolute inset-0 flex">
          {phase.kind === "download" ? (
            <Loading label="正在读文件…" loaded={phase.loaded} total={phase.total} />
          ) : (
            <Loading label="正在排版…" />
          )}
        </div>
      )}
    </div>
  );
}
