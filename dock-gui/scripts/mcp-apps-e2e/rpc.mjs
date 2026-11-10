// 直接问隔离 Dock 的网关：node rpc.mjs <method> [json]
import { readFileSync } from "node:fs";
const token = readFileSync("/tmp/dock-e2e/token", "utf8").trim();
const [method, params = "{}"] = process.argv.slice(2);
const ws = new WebSocket("ws://127.0.0.1:18997/api/ws");
let id = 0;
const pending = new Map();
const call = (m, p = {}) => new Promise((res) => { pending.set(++id, res); ws.send(JSON.stringify({ jsonrpc: "2.0", id, method: m, params: p })); });
ws.onmessage = (e) => { const m = JSON.parse(e.data); if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); } };
ws.onopen = async () => {
  await call("connection/authenticate", { token });
  await call("initialize", {});
  console.log(JSON.stringify(await call(method, JSON.parse(params)), null, 1));
  ws.close();
};
