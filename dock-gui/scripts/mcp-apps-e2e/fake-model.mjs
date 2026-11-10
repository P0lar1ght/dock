// 本地假模型（OpenAI /chat/completions，流式）：按最后一条用户消息走固定剧本，调白板工具。
// 让 MCP App 的端到端测试不靠外网、结果确定。用法：node fake-model.mjs [端口]
import { createServer } from "node:http";

const PORT = Number(process.argv[2] ?? 18998);
let seq = 0;

const edit = (input) => ({ name: "use_tool", arguments: JSON.stringify({ tool_name: "mcp_whiteboard__whiteboard_edit", tool_input: input }) });
const SCRIPT = [
  {
    match: /登录流程/,
    call: edit({
      clear: true,
      add: [
        { type: "rectangle", id: "open", text: "打开 App", x: 0, y: 0, fill: "#a5d8ff" },
        { type: "rectangle", id: "login", text: "输入账号", x: 280, y: 0, fill: "#ffec99" },
        { type: "rectangle", id: "home", text: "进入首页", x: 560, y: 0, fill: "#b2f2bb" },
        { type: "arrow", id: "e1", from: "open", to: "login" },
        { type: "arrow", id: "e2", from: "login", to: "home", text: "成功" },
      ],
    }),
    reply: "画好了：三个方框从左到右，用箭头连起来。",
  },
  {
    match: /找回密码/,
    call: edit({ add: [{ type: "diamond", id: "forgot", text: "找回密码", x: 280, y: 200, fill: "#ffc9c9" }, { type: "arrow", id: "e3", from: "login", to: "forgot", text: "忘了" }] }),
    reply: "加了「找回密码」，从「输入账号」连过去。",
  },
  {
    match: /界面上下文/,
    echo: true,
  },
  {
    match: /改了什么/,
    // 不调工具：把界面交来的上下文原样念出来，测试据此断言它进了模型。
    echo: true,
  },
];

function lastUser(messages) {
  for (let i = messages.length - 1; i >= 0; i--) {
    const m = messages[i];
    if (m.role !== "user") continue;
    const text = typeof m.content === "string" ? m.content : (m.content ?? []).map((p) => p.text ?? "").join("");
    if (!text.includes("<system-reminder>")) return { index: i, text };
  }
  return { index: -1, text: "" };
}

function plan(messages) {
  const { index, text } = lastUser(messages);
  // 通用：「调用 <公名> <JSON>」→ 经 use_tool 调那颗工具（测第三方 MCP App 用）；有结果了就收尾。
  const direct = /^调用 (\S+)(?:\s+(\{.*\}))?\s*$/s.exec(text.trim());
  if (direct) {
    if (messages.slice(index + 1).some((m) => m.role === "tool")) return { content: `调完了 ${direct[1]}。` };
    return { tool: { name: "use_tool", arguments: JSON.stringify({ tool_name: direct[1], tool_input: JSON.parse(direct[2] ?? "{}") }) } };
  }
  const step = SCRIPT.find((s) => s.match.test(text));
  if (!step) return { content: "（假模型：没有这句的剧本）" };
  const after = messages.slice(index + 1);
  if (step.echo) {
    const ctx = messages
      .map((m) => (typeof m.content === "string" ? m.content : (m.content ?? []).map((p) => p.text ?? "").join("")))
      .filter((t) => t.includes("MCP App 界面更新"))
      .pop();
    return { content: ctx ? `界面告诉我：${ctx}` : "没有收到界面的上下文。" };
  }
  if (after.some((m) => m.role === "tool")) return { content: step.reply };
  return { tool: step.call };
}

function send(res, body, stream) {
  const id = `fake-${++seq}`;
  if (!stream) {
    const message = body.tool
      ? { role: "assistant", content: null, tool_calls: [{ id: `call_${seq}`, type: "function", function: body.tool }] }
      : { role: "assistant", content: body.content };
    res.writeHead(200, { "content-type": "application/json" });
    res.end(JSON.stringify({ id, object: "chat.completion", choices: [{ index: 0, message, finish_reason: body.tool ? "tool_calls" : "stop" }] }));
    return;
  }
  res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache" });
  const chunk = (delta, finish = null) =>
    res.write(`data: ${JSON.stringify({ id, object: "chat.completion.chunk", choices: [{ index: 0, delta, finish_reason: finish }] })}\n\n`);
  if (body.tool) {
    chunk({ role: "assistant", tool_calls: [{ index: 0, id: `call_${seq}`, type: "function", function: { name: body.tool.name, arguments: "" } }] });
    chunk({ tool_calls: [{ index: 0, function: { arguments: body.tool.arguments } }] });
    chunk({}, "tool_calls");
  } else {
    chunk({ role: "assistant", content: body.content });
    chunk({}, "stop");
  }
  res.write("data: [DONE]\n\n");
  res.end();
}

createServer((req, res) => {
  let raw = "";
  req.on("data", (c) => (raw += c));
  req.on("end", () => {
    if (!req.url.endsWith("/chat/completions")) {
      res.writeHead(404).end();
      return;
    }
    const body = JSON.parse(raw || "{}");
    send(res, plan(body.messages ?? []), body.stream !== false);
  });
}).listen(PORT, "127.0.0.1", () => console.log(`fake model on ${PORT}`));
