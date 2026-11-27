// 「HTTP 历史」：像抓包工具那样，上面是这一页的每次出网（方法、主机、路径、状态、大小、耗时），
// 点一行下面并排显示原样的请求包和响应包（请求行 / 状态行、头、正文；JSON 可以美化，
// 压缩的正文 Dock 已经解开，二进制给十六进制）。没解密的连接只有目的地和字节数，写明原因。

import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { Braces, Check, Copy, Lock, Search } from "lucide-react";
import { humanBytes, sandboxFlow, sandboxHttp, type FlowDetail, type HttpRow, type Packet } from "./api";

const POLL_MS = 2500;

export function HttpHistory({ sessionId, active }: { sessionId: string; active: boolean }) {
  const [rows, setRows] = useState<HttpRow[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const records = useRef<number | undefined>(undefined);

  // 这一页在看才轮询；记录数没变不换（列表不跟着闪）。
  useEffect(() => {
    if (!active) return;
    let live = true;
    const load = () =>
      sandboxHttp(sessionId)
        .then((r) => {
          if (!live) return;
          setError(null);
          if (rows && r.records === records.current) return;
          records.current = r.records;
          setRows(r.requests);
        })
        .catch((e) => live && setError(e instanceof Error ? e.message : String(e)));
    void load();
    const h = setInterval(() => !document.hidden && void load(), POLL_MS);
    return () => {
      live = false;
      clearInterval(h);
    };
  }, [sessionId, active]); // eslint-disable-line react-hooks/exhaustive-deps

  const list = useMemo(() => {
    const q = query.trim().toLowerCase();
    const all = [...(rows ?? [])].reverse();
    if (!q) return all;
    return all.filter((r) => `${r.method ?? ""} ${r.host ?? ""}${r.path ?? ""} ${r.status ?? ""}`.toLowerCase().includes(q));
  }, [rows, query]);
  const total = rows?.length ?? 0;

  if (error && !rows) return <div className="rounded-lg border border-[#fecaca] bg-[#fef2f2] px-3 py-2 text-[12.5px] text-[#c0392b]">{error}</div>;
  if (!rows) return <div className="skeleton h-24 rounded-lg" />;
  if (total === 0)
    return (
      <div className="flex flex-1 items-center justify-center px-6 text-center text-[13px] leading-relaxed text-ink-4">
        还没有出网：命令（curl、git、npm…）和内置浏览器发的请求会出现在这里
      </div>
    );

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-2">
      <div className="flex items-center gap-2">
        <div className="flex h-7 min-w-0 flex-1 items-center gap-1.5 rounded-md border border-line bg-white px-2">
          <Search size={12} className="shrink-0 text-ink-4" />
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="按方法、主机、路径、状态码过滤"
            className="min-w-0 flex-1 bg-transparent text-[12px] text-ink outline-none placeholder:text-ink-5"
          />
        </div>
        <span className="shrink-0 text-[11.5px] tabular-nums text-ink-4">
          {list.length === total ? `${total} 次` : `${list.length} / ${total} 次`}
        </span>
      </div>
      <div className="thin-scroll max-h-[38%] min-h-[96px] shrink-0 overflow-auto rounded-lg border border-line bg-white">
        <table className="w-full table-fixed border-collapse whitespace-nowrap text-[11.5px]">
          <thead className="sticky top-0 z-[1] bg-[#f7f7f8] text-left text-[10.5px] font-medium text-ink-4">
            <tr>
              <th className="w-[58px] px-2 py-1 font-medium">时间</th>
              <th className="w-[58px] px-1 py-1 font-medium">方法</th>
              <th className="px-1 py-1 font-medium">主机 / 路径</th>
              <th className="w-[44px] px-1 py-1 font-medium">状态</th>
              <th className="w-[62px] px-1 py-1 text-right font-medium">大小</th>
              <th className="w-[66px] px-2 py-1 text-right font-medium">耗时</th>
            </tr>
          </thead>
          <tbody>
            {list.map((r, i) => (
              <tr
                key={r.id ?? i}
                onClick={() => setSelected(r.id)}
                className={`motion-colors cursor-pointer border-t border-[#f0f0f2] ${selected === r.id ? "bg-[#eaf3ff]" : "hover:bg-[#f7f7f8]"}`}
              >
                <td className="px-2 py-1 tabular-nums text-ink-4">{new Date(r.ts).toLocaleTimeString("zh-CN", { hour12: false })}</td>
                <td className="px-1 py-1">
                  <Method method={r.method ?? (r.decrypted ? "" : "CONNECT")} />
                </td>
                <td className="truncate px-1 py-1" title={`${r.scheme ?? ""}://${r.host}${r.path ?? ""}`}>
                  <span className="font-medium text-ink-2">{r.host}</span>
                  <span className="text-ink-4">{r.path ?? ""}</span>
                  {!r.decrypted && (
                    <span className="ml-1.5 inline-flex items-center gap-0.5 text-[10.5px] text-[#b25e00]" title={r.note ?? "没解密"}>
                      <Lock size={9} />
                      未解密
                    </span>
                  )}
                </td>
                <td className="px-1 py-1">
                  <Status status={r.status} error={r.error} />
                </td>
                <td className="px-1 py-1 text-right tabular-nums text-ink-3">{humanBytes(r.bytes ?? 0)}</td>
                <td className="px-2 py-1 text-right tabular-nums text-ink-4">{r.ms != null ? `${r.ms} ms` : ""}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      {selected ? (
        <FlowView key={selected} sessionId={sessionId} id={selected} />
      ) : (
        <div className="flex flex-1 items-center justify-center text-[12.5px] text-ink-4">点一行看请求包和响应包</div>
      )}
    </div>
  );
}

function Method({ method }: { method: string }) {
  const tone: Record<string, string> = {
    GET: "text-[#1f7a3d] bg-[#e8f5ec]",
    POST: "text-[#0a6fd6] bg-[#eaf3ff]",
    PUT: "text-[#b25e00] bg-[#fff4e0]",
    PATCH: "text-[#b25e00] bg-[#fff4e0]",
    DELETE: "text-[#c0392b] bg-[#fef2f2]",
  };
  return (
    <span className={`inline-block rounded px-1 py-px font-mono text-[10px] font-semibold ${tone[method] ?? "bg-fill text-ink-3"}`}>
      {method || "—"}
    </span>
  );
}

function Status({ status, error }: { status?: number | null; error?: string | null }) {
  if (!status) return error ? <span className="text-[#c0392b]" title={error}>失败</span> : <span className="text-ink-5">—</span>;
  const tone = status >= 500 ? "text-[#c0392b]" : status >= 400 ? "text-[#b25e00]" : status >= 300 ? "text-[#0a6fd6]" : "text-[#1f7a3d]";
  return <span className={`font-mono font-semibold tabular-nums ${tone}`}>{status}</span>;
}

function FlowView({ sessionId, id }: { sessionId: string; id: string }) {
  const [flow, setFlow] = useState<FlowDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    sandboxFlow(sessionId, id)
      .then((f) => live && setFlow(f))
      .catch((e) => live && setError(e instanceof Error ? e.message : String(e)));
    return () => {
      live = false;
    };
  }, [sessionId, id]);

  if (error) return <div className="rounded-lg border border-[#fecaca] bg-[#fef2f2] px-3 py-2 text-[12.5px] text-[#c0392b]">{error}</div>;
  if (!flow) return <div className="skeleton min-h-24 flex-1 rounded-lg" />;
  if (!flow.request && !flow.response) {
    return (
      <div className="enter-fade flex flex-col gap-1.5 rounded-lg border border-line bg-white px-3 py-3 text-[12.5px] text-ink-3">
        <div className="flex items-center gap-1.5 font-medium text-ink-2">
          <Lock size={13} className="text-[#b25e00]" />
          {flow.scheme ?? "tcp"}://{flow.host}:{flow.port} 没有解密
        </div>
        <div>{flow.note ?? flow.error ?? "原样转发，只记了目的地和字节数。"}</div>
        <div className="text-ink-4">
          发出 {humanBytes(flow.bytesUp ?? 0)} · 收到 {humanBytes(flow.bytesDown ?? 0)}
          {flow.durationMs != null ? ` · ${flow.durationMs} ms` : ""}
        </div>
      </div>
    );
  }
  return (
    // 面板够宽（不看窗口宽）时请求、响应左右并排，窄了上下叠。
    <div className="@container flex min-h-0 flex-1 flex-col">
      <div className="enter-fade grid min-h-0 flex-1 grid-cols-1 gap-2 @[560px]:grid-cols-2">
        <PacketView title="请求" packet={flow.request} hostLine={`${flow.scheme ?? "https"}://${flow.host}${flow.port && ![80, 443].includes(flow.port) ? `:${flow.port}` : ""}`} />
        <PacketView title="响应" packet={flow.response} hostLine={flow.error ? `出错：${flow.error}` : flow.durationMs != null ? `${flow.durationMs} ms` : ""} />
      </div>
    </div>
  );
}

function PacketView({ title, packet, hostLine }: { title: string; packet: Packet | null; hostLine: string }) {
  const json = useMemo(() => (packet ? parseJson(packet) : null), [packet]);
  const [pretty, setPretty] = useState(true);
  const [copied, setCopied] = useState(false);
  const raw = packet ? rawText(packet) : "";
  const copy = () => {
    void navigator.clipboard.writeText(raw).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1200);
    });
  };
  return (
    <section className="flex min-h-[160px] min-w-0 flex-col overflow-hidden rounded-lg border border-line bg-white">
      <div className="flex h-8 shrink-0 items-center gap-2 border-b border-[#f0f0f2] px-2.5">
        <span className="text-[12px] font-semibold text-ink-2">{title}</span>
        <span className="min-w-0 flex-1 truncate text-[11px] text-ink-4">{hostLine}</span>
        {json !== null && (
          <button
            type="button"
            onClick={() => setPretty((v) => !v)}
            title={pretty ? "看原样的正文" : "把 JSON 排版好"}
            className={`motion-colors flex h-6 items-center gap-1 rounded px-1.5 text-[11px] ${pretty ? "bg-[#eaf3ff] text-[#0a6fd6]" : "text-ink-4 hover:bg-hover"}`}
          >
            <Braces size={11} />
            美化
          </button>
        )}
        {packet && (
          <button type="button" onClick={copy} title="复制原样的包" className="motion-colors flex size-6 items-center justify-center rounded text-ink-4 hover:bg-hover">
            {copied ? <Check size={12} className="text-[#1f7a3d]" /> : <Copy size={12} />}
          </button>
        )}
      </div>
      {!packet ? (
        <div className="flex flex-1 items-center justify-center p-3 text-[12px] text-ink-4">没有收到响应</div>
      ) : (
        <pre className="thin-scroll min-h-0 flex-1 overflow-auto whitespace-pre-wrap break-all px-2.5 py-2 font-mono text-[11.5px] leading-[1.55] text-ink-2">
          <FirstLine line={packet.line} />
          {"\n"}
          {packet.headers.map(([k, v], i) => (
            <span key={i}>
              <span className="text-[#0a6fd6]">{k}</span>
              <span className="text-ink-4">: </span>
              <span className={v.startsWith("[redacted") ? "text-ink-4 italic" : ""}>{v}</span>
              {"\n"}
            </span>
          ))}
          {"\n"}
          <Body packet={packet} json={pretty ? json : null} />
        </pre>
      )}
    </section>
  );
}

function FirstLine({ line }: { line: string }) {
  const [a, b, ...rest] = line.split(" ");
  if (a?.startsWith("HTTP/")) {
    const code = Number(b);
    const tone = code >= 500 ? "text-[#c0392b]" : code >= 400 ? "text-[#b25e00]" : code >= 300 ? "text-[#0a6fd6]" : "text-[#1f7a3d]";
    return (
      <span className="font-semibold">
        <span className="text-ink-4">{a} </span>
        <span className={tone}>{[b, ...rest].join(" ")}</span>
      </span>
    );
  }
  return (
    <span className="font-semibold">
      <span className="text-[#8a3ffc]">{a} </span>
      <span className="text-ink">{b} </span>
      <span className="text-ink-4">{rest.join(" ")}</span>
    </span>
  );
}

function Body({ packet, json }: { packet: Packet; json: unknown }) {
  const b = packet.body;
  const notes: string[] = [];
  if (b.decoded) notes.push(`已按 ${b.decoded} 解开`);
  if (b.truncated || b.captured < b.size) notes.push(`只记了开头 ${humanBytes(b.captured)}（共 ${humanBytes(b.size)}）`);
  const note = notes.length ? <span className="block pb-1 text-[10.5px] text-[#b25e00]">{`（${notes.join("，")}）`}</span> : null;
  if (b.size === 0 && !b.text && !b.base64) return <span className="text-ink-5">（没有正文）</span>;
  if (json !== null && json !== undefined) {
    return (
      <>
        {note}
        <JsonText value={json} />
      </>
    );
  }
  if (b.text !== null) {
    return (
      <>
        {note}
        {b.text}
      </>
    );
  }
  return (
    <>
      {note}
      <span className="text-ink-4">{`二进制 ${humanBytes(b.shown)}，开头：\n`}</span>
      {hexdump(b.base64 ?? "")}
    </>
  );
}

function parseJson(p: Packet): unknown | null {
  const text = p.body.text;
  if (!text) return null;
  const looks = (p.contentType ?? "").includes("json") || /^\s*[[{]/.test(text);
  if (!looks) return null;
  try {
    return JSON.parse(text);
  } catch {
    return null;
  }
}

/** 排好版的 JSON，键 / 字符串 / 数字 / 布尔分别着色。 */
function JsonText({ value }: { value: unknown }) {
  const text = JSON.stringify(value, null, 2);
  const parts: ReactNode[] = [];
  const re = /("(?:\\u[a-fA-F0-9]{4}|\\[^u]|[^\\"])*")(\s*:)?|\b(true|false|null)\b|-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?/g;
  let last = 0;
  let m: RegExpExecArray | null;
  let i = 0;
  while ((m = re.exec(text))) {
    if (m.index > last) parts.push(text.slice(last, m.index));
    const cls = m[1] ? (m[2] ? "text-[#0a6fd6]" : "text-[#1f7a3d]") : m[3] ? "text-[#8a3ffc]" : "text-[#b25e00]";
    parts.push(
      <span key={i++} className={cls}>
        {m[1] ?? m[0]}
      </span>,
    );
    if (m[2]) parts.push(m[2]);
    last = re.lastIndex;
  }
  parts.push(text.slice(last));
  return <>{parts}</>;
}

function rawText(p: Packet): string {
  const head = [p.line, ...p.headers.map(([k, v]) => `${k}: ${v}`)].join("\r\n");
  return `${head}\r\n\r\n${p.body.text ?? ""}`;
}

function hexdump(b64: string): string {
  let bytes: Uint8Array;
  try {
    bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0)).slice(0, 512);
  } catch {
    return "";
  }
  const lines: string[] = [];
  for (let off = 0; off < bytes.length; off += 16) {
    const chunk = [...bytes.slice(off, off + 16)];
    const hex = chunk.map((x) => x.toString(16).padStart(2, "0")).join(" ");
    const ascii = chunk.map((x) => (x >= 32 && x < 127 ? String.fromCharCode(x) : ".")).join("");
    lines.push(`${off.toString(16).padStart(6, "0")}  ${hex.padEnd(47)}  ${ascii}`);
  }
  return lines.join("\n");
}

