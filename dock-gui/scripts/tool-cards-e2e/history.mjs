// 用法：node history.mjs <输出文件>：最近一个会话的 thread/history 事件（工具卡样本）。
import { readFileSync, writeFileSync } from "node:fs";
const token = readFileSync("/tmp/dock-e2e/token", "utf8").trim();
const ws = new WebSocket("ws://127.0.0.1:18997/api/ws");
let id = 0;
const pending = new Map();
ws.onmessage = (ev) => {
  const f = JSON.parse(ev.data);
  if (f.id && pending.has(f.id)) {
    pending.get(f.id)(f);
    pending.delete(f.id);
  }
};
const call = (method, params = {}) =>
  new Promise((res) => {
    const i = ++id;
    pending.set(i, res);
    ws.send(JSON.stringify({ id: i, method, params }));
  });
await new Promise((r) => (ws.onopen = r));
await call("connection/authenticate", { token });
await call("initialize", {});
const list = await call("thread/list", { scope: "all" });
const threads = (list.result?.threads ?? [])
  .filter((t) => t.id !== "live" && (t.cwd ?? "").includes("dock-e2e"))
  .filter((t) => !process.argv[3] || process.argv[3] === "--list" || t.title.startsWith(process.argv[3]))
  .reverse();
if (process.argv[3] === "--list") console.log(JSON.stringify(list.result?.threads?.map((t) => ({ id: t.id, title: t.title, cwd: t.cwd, u: t.updatedAt }))));
// 同名的会话可能有好几个：取最后一个事件最新的。
let best = null;
for (const t of threads) {
  const h = await call("thread/history", { threadId: t.id });
  const evs = h.result?.events ?? [];
  const last = Number(evs.at(-1)?.timestamp ?? 0);
  if (!best || last > best.last) best = { t, h, last };
}
writeFileSync(process.argv[2], JSON.stringify(best.h.result, null, 1));
console.log(best.t.id, best.t.title, (best.h.result?.events ?? []).length, "events");
ws.close();
