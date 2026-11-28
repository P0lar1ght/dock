// 「HTTP 历史」：像抓包工具那样，上面是这一页的每次出网（方法、主机、路径、状态、大小、耗时），
// 点一行下面并排显示原样的请求包和响应包（请求行 / 状态行、头、正文；JSON 可以美化，
// 压缩的正文 Dock 已经解开，二进制给十六进制）。没解密的连接只有目的地和字节数，写明原因。
// WebSocket 是握手那一行（WS），点开看来回的消息（开着就跟着刷新），握手的包在另一页。

import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { ArrowDown, ArrowUp, Braces, Check, Copy, Lock, Search } from "lucide-react";
import { humanBytes, sandboxFlow, sandboxHttp, type FlowDetail, type HttpRow, type Packet, type WsConversation, type WsMessage } from "./api";

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
          // 只看记录数（ref）：闭包里的 rows 一直是第一次的值，拿它判断不会生效。
          if (records.current !== undefined && r.records === records.current) return;
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
    return all.filter((r) =>
      `${r.ws ? "ws websocket " : ""}${r.scheme ?? ""} ${r.method ?? ""} ${r.host ?? ""}${r.path ?? ""} ${r.status ?? ""}`.toLowerCase().includes(q),
    );
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
            placeholder="按方法、主机、路径、状态码过滤（ws 只看 WebSocket）"
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
                  <Method method={r.ws ? "WS" : (r.method ?? (r.decrypted ? "" : "CONNECT"))} />
                </td>
                <td className="truncate px-1 py-1" title={`${r.scheme ?? ""}://${r.host}${r.path ?? ""}`}>
                  <span className="font-medium text-ink-2">{r.host}</span>
                  <span className="text-ink-4">{r.path ?? ""}</span>
                  {!r.decrypted && !r.method && (
                    <span className="ml-1.5 inline-flex items-center gap-0.5 text-[10.5px] text-[#b25e00]" title={r.note ?? "没解密"}>
                      <Lock size={9} />
                      未解密
                    </span>
                  )}
                  {r.ws && (
                    <span className="ml-1.5 text-[10.5px] tabular-nums text-ink-4" title={`发出 ${r.ws.sent} 条 · 收到 ${r.ws.received} 条`}>
                      ↑{r.ws.sent} ↓{r.ws.received}
                    </span>
                  )}
                </td>
                <td className="px-1 py-1">
                  <Status status={r.status} error={r.error} />
                </td>
                <td className="px-1 py-1 text-right tabular-nums text-ink-3">{humanBytes(r.bytes ?? 0)}</td>
                <td className="px-2 py-1 text-right tabular-nums text-ink-4">
                  {r.ws?.open ? (
                    <span className="inline-flex items-center gap-1 text-[#1f7a3d]" title={r.ms != null ? `已连 ${duration(r.ms)}` : undefined}>
                      <span className="breathe size-1.5 rounded-full bg-[#1f7a3d]" />
                      开着
                    </span>
                  ) : r.ms != null ? (
                    r.ws ? duration(r.ms) : `${r.ms} ms`
                  ) : (
                    ""
                  )}
                </td>
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
    WS: "text-[#8a3ffc] bg-[#f3ecff]",
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
  // 还开着的 WebSocket 跟着刷新（新消息每两秒落一段）；别的取一次就够。
  useEffect(() => {
    let live = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const load = () => {
      if (document.hidden) {
        timer = setTimeout(load, POLL_MS);
        return;
      }
      sandboxFlow(sessionId, id)
        .then((f) => {
          if (!live) return;
          setFlow(f);
          if (f.ws?.open) timer = setTimeout(load, POLL_MS);
        })
        .catch((e) => live && setError(e instanceof Error ? e.message : String(e)));
    };
    load();
    return () => {
      live = false;
      clearTimeout(timer);
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
  if (flow.ws) return <WsView flow={flow} ws={flow.ws} />;
  return <Packets flow={flow} />;
}

/** 请求包和响应包：面板够宽（不看窗口宽）时左右并排，窄了上下叠。 */
function Packets({ flow }: { flow: FlowDetail }) {
  return (
    <div className="@container flex min-h-0 flex-1 flex-col">
      <div className="enter-fade grid min-h-0 flex-1 grid-cols-1 gap-2 @[560px]:grid-cols-2">
        <PacketView title="请求" packet={flow.request} hostLine={`${flow.scheme ?? "https"}://${flow.host}${portSuffix(flow.port)}`} />
        <PacketView title="响应" packet={flow.response} hostLine={flow.error ? `出错：${flow.error}` : flow.durationMs != null ? `${flow.durationMs} ms` : ""} />
      </div>
    </div>
  );
}

/** 不是默认端口（80 / 443）就带上 `:端口`。 */
function portSuffix(port: number | null | undefined): string {
  return port && ![80, 443].includes(port) ? `:${port}` : "";
}

/** 时长：短的给毫秒，长的给秒 / 分。 */
function duration(ms: number): string {
  if (ms < 1000) return `${ms} ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)} s`;
  const m = Math.floor(ms / 60_000);
  if (m < 60) return `${m} 分 ${Math.floor((ms % 60_000) / 1000)} 秒`;
  return `${Math.floor(m / 60)} 小时 ${m % 60} 分`;
}

type WsTab = "messages" | "handshake";
type WsDir = "all" | "up" | "down";

/** WebSocket 这一条：上面一行汇总（条数、流量、开着 / 谁关的），下面「消息」「握手」两页。 */
function WsView({ flow, ws }: { flow: FlowDetail; ws: WsConversation }) {
  const [tab, setTab] = useState<WsTab>("messages");
  const close = ws.close;
  const state = ws.open ? (
    <span className="inline-flex items-center gap-1 text-[#1f7a3d]">
      <span className="breathe size-1.5 rounded-full bg-[#1f7a3d]" />
      开着
    </span>
  ) : close ? (
    <span className="text-ink-3">
      {close.by === "client" ? "客户端" : "服务器"}先关的
      {close.code != null ? ` · ${close.code}` : ""}
      {close.reason ? ` ${close.reason}` : ""}
    </span>
  ) : ws.error ? (
    <span className="text-[#c0392b]" title={ws.error}>
      断了：{ws.error}
    </span>
  ) : (
    <span className="text-ink-4">已断开</span>
  );
  return (
    <div className="enter-fade flex min-h-0 flex-1 flex-col gap-2">
      <div className="flex shrink-0 items-center gap-2 text-[11.5px]">
        <span className="min-w-0 flex-1 truncate tabular-nums text-ink-3">
          <span className="font-medium text-ink-2">{`${flow.scheme ?? "ws"}://${flow.host}${portSuffix(flow.port)}${flow.path ?? ""}`}</span>
          {` · ↑${ws.sent} ↓${ws.received} · ${humanBytes(ws.bytesUp + ws.bytesDown)}`}
          {ws.durationMs != null ? ` · ${duration(ws.durationMs)}` : ""}
          {" · "}
          {state}
        </span>
        <div className="flex shrink-0 rounded-md bg-fill p-0.5">
          {(
            [
              ["messages", `消息 ${ws.messages.length}`],
              ["handshake", "握手"],
            ] as const
          ).map(([key, label]) => (
            <button
              key={key}
              type="button"
              onClick={() => setTab(key)}
              className={`motion-colors h-6 rounded px-2 text-[11.5px] ${tab === key ? "bg-white font-medium text-ink shadow-sm" : "text-ink-4 hover:text-ink-2"}`}
            >
              {label}
            </button>
          ))}
        </div>
      </div>
      {tab === "messages" ? <WsMessages ws={ws} /> : <Packets flow={flow} />}
    </div>
  );
}

/** 消息列表：方向、时间、类型、大小、内容开头；点一条在下面展开全文（JSON 排好版、二进制给十六进制）。
 *  开着的连接停在最底下时跟着新消息往下走。 */
function WsMessages({ ws }: { ws: WsConversation }) {
  const [dir, setDir] = useState<WsDir>("all");
  const [query, setQuery] = useState("");
  const [open, setOpen] = useState<Set<number>>(() => new Set());
  const box = useRef<HTMLDivElement>(null);
  const atBottom = useRef(true);
  const list = useMemo(() => {
    const q = query.trim().toLowerCase();
    return ws.messages
      .map((m, i) => ({ m, i }))
      .filter(({ m }) => dir === "all" || m.dir === dir)
      .filter(({ m }) => !q || `${m.op} ${m.text ?? ""} ${m.reason ?? ""}`.toLowerCase().includes(q));
  }, [ws.messages, dir, query]);
  useEffect(() => {
    const el = box.current;
    if (el && atBottom.current) el.scrollTop = el.scrollHeight;
  }, [ws.messages.length]);
  const toggle = (i: number) =>
    setOpen((prev) => {
      const next = new Set(prev);
      if (next.has(i)) next.delete(i);
      else next.add(i);
      return next;
    });
  const hidden = ws.omitted + ws.lost;
  return (
    <section className="flex min-h-[160px] min-w-0 flex-1 flex-col overflow-hidden rounded-lg border border-line bg-white">
      <div className="flex h-8 shrink-0 items-center gap-2 border-b border-[#f0f0f2] px-2">
        <div className="flex shrink-0 rounded bg-fill p-0.5">
          {(
            [
              ["all", "全部"],
              ["up", "发出"],
              ["down", "收到"],
            ] as const
          ).map(([key, label]) => (
            <button
              key={key}
              type="button"
              onClick={() => setDir(key)}
              className={`motion-colors h-5 rounded px-1.5 text-[11px] ${dir === key ? "bg-white font-medium text-ink shadow-sm" : "text-ink-4 hover:text-ink-2"}`}
            >
              {label}
            </button>
          ))}
        </div>
        <Search size={11} className="shrink-0 text-ink-4" />
        <input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="按内容过滤"
          className="min-w-0 flex-1 bg-transparent text-[11.5px] text-ink outline-none placeholder:text-ink-5"
        />
      </div>
      <div
        ref={box}
        onScroll={(e) => {
          const el = e.currentTarget;
          atBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
        }}
        className="thin-scroll min-h-0 flex-1 overflow-auto"
      >
        {list.length === 0 ? (
          <div className="flex h-full min-h-20 items-center justify-center px-4 text-center text-[12px] text-ink-4">
            {ws.messages.length === 0 ? (ws.open ? "连上了，还没有消息（新消息每两秒落一次）" : "这条连接没有留下消息") : "没有符合的消息"}
          </div>
        ) : (
          list.map(({ m, i }) => <WsRow key={i} m={m} expanded={open.has(i)} onToggle={() => toggle(i)} />)
        )}
        {(hidden > 0 || ws.note || ws.error) && (
          <div className="border-t border-[#f0f0f2] px-2.5 py-1.5 text-[11px] leading-relaxed text-[#b25e00]">
            {ws.omitted > 0 && <div>{`另有 ${ws.omitted} 条没留下来（一条连接最多留 2000 条，之后只计数）`}</div>}
            {ws.lost > 0 && <div>{`${ws.lost} 条的内容块不见了（审计目录被改过）`}</div>}
            {ws.note && <div>{ws.note}</div>}
            {ws.error && <div className="text-[#c0392b]">{ws.error}</div>}
          </div>
        )}
      </div>
    </section>
  );
}

function WsRow({ m, expanded, onToggle }: { m: WsMessage; expanded: boolean; onToggle: () => void }) {
  const up = m.dir === "up";
  const preview =
    m.op === "close"
      ? `${m.code ?? ""} ${m.reason ?? ""}`.trim() || "（没有关闭码）"
      : (m.text?.replace(/\s+/g, " ") ?? (m.note ? m.note : m.base64 ? `二进制 ${humanBytes(m.size)}` : ""));
  const opTone: Record<WsMessage["op"], string> = {
    text: "text-ink-3",
    binary: "text-[#b25e00]",
    close: "text-[#c0392b]",
    ping: "text-ink-5",
    pong: "text-ink-5",
  };
  return (
    <div className="border-b border-[#f6f6f8] last:border-b-0">
      <button
        type="button"
        onClick={onToggle}
        className={`motion-colors flex w-full items-center gap-2 px-2 py-1 text-left text-[11.5px] ${expanded ? "bg-[#f7f7f8]" : "hover:bg-[#fafafb]"}`}
      >
        {up ? <ArrowUp size={11} className="shrink-0 text-[#0a6fd6]" /> : <ArrowDown size={11} className="shrink-0 text-[#1f7a3d]" />}
        <span className="w-[52px] shrink-0 tabular-nums text-ink-4">{`+${(m.at / 1000).toFixed(2)}s`}</span>
        <span className={`w-[42px] shrink-0 font-mono text-[10.5px] ${opTone[m.op]}`}>{m.op}</span>
        <span className="w-[56px] shrink-0 text-right tabular-nums text-ink-4">{humanBytes(m.size)}</span>
        <span className="min-w-0 flex-1 truncate font-mono text-[11px] text-ink-2">{preview}</span>
      </button>
      {expanded && (
        <pre className="enter-fade thin-scroll max-h-64 overflow-auto whitespace-pre-wrap break-all bg-[#fafafb] px-3 py-2 font-mono text-[11.5px] leading-[1.55] text-ink-2">
          <WsBody m={m} />
        </pre>
      )}
    </div>
  );
}

function WsBody({ m }: { m: WsMessage }) {
  const json = useMemo(() => {
    if (!m.text || !/^\s*[[{]/.test(m.text)) return null;
    try {
      return JSON.parse(m.text) as unknown;
    } catch {
      return null;
    }
  }, [m.text]);
  const notes: string[] = [];
  if (m.compressed) notes.push("已按 permessage-deflate 解开");
  if (m.truncated) notes.push(`只记了开头（共 ${humanBytes(m.size)}）`);
  const note = notes.length ? <span className="block pb-1 text-[10.5px] text-[#b25e00]">{`（${notes.join("，")}）`}</span> : null;
  if (m.op === "close") return <>{`关闭码 ${m.code ?? "（没有）"}${m.reason ? `\n原因：${m.reason}` : ""}`}</>;
  if (m.note && m.text == null && m.base64 == null) return <span className="text-[#b25e00]">{m.note}</span>;
  if (m.text != null) {
    return (
      <>
        {note}
        {json !== null ? <JsonText value={json} /> : m.text || <span className="text-ink-5">（空）</span>}
      </>
    );
  }
  if (m.base64) {
    return (
      <>
        {note}
        <span className="text-ink-4">{`二进制 ${humanBytes(m.size)}，开头：\n`}</span>
        {hexdump(m.base64)}
      </>
    );
  }
  return <span className="text-ink-5">（没有内容）</span>;
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

