// 每颗工具怎么画：行头（图标、动词、主参数、补充）+ 正文。输出格式以 Dock 实际回的为准
// （样本见 docs/TOOL-CARDS.md 的实测列）。认不出的格式一律退回原文，不丢信息。

import { useCallback, useState, type ReactNode } from "react";
import {
  Bot,
  CalendarClock,
  Camera,
  ChevronDown,
  Circle,
  CircleCheck,
  CircleDot,
  CircleSlash,
  Code2,
  Eye,
  FileSearch,
  Files,
  Folder,
  Globe,
  Hand,
  Goal,
  Keyboard,
  LayoutGrid,
  ListChecks,
  ListTree,
  MessageCircleQuestion,
  MessageSquare,
  Monitor,
  MousePointer2,
  Navigation,
  NotebookPen,
  Pencil,
  Puzzle,
  Radar,
  Search,
  Sparkles,
  Square,
  SquareTerminal,
  Workflow,
  Wrench,
  type LucideIcon,
} from "lucide-react";
import { Markdown } from "../Markdown";
import { fileType } from "../fileType";
import { programLabel } from "../../dock/programLabel";
import { Badge, CodeView, FileRow, KeyValues, MoreBar, Panel, stripAnsi, usePeek, type BadgeTone } from "./parts";

export interface Spec {
  icon: LucideIcon;
  /** 图标颜色（文件类工具按文件类型着色）；不给就是灰。 */
  iconColor?: string;
  verb: string;
  /** 动词后面的主参数（等宽灰字）。 */
  detail: string;
  /**
   * 给人看的一句话（bash / monitor 的 `description`）：有就由它当标题，动词和主参数
   * 都不上标题（悬停看主参数原文、展开看正文），右侧只挂 `tag`。
   */
  summary?: string;
  /** 有 `summary` 时右侧的灰色小徽标：命令的主程序（`find`、`cargo test`）。 */
  tag?: string;
  /** 主参数后面的补充：小号浅灰字，或一枚徽标。 */
  note?: ReactNode;
  prompt?: string;
  body: (output: string, args: Record<string, unknown>) => ReactNode;
  /** 输出空着也有正文（编辑看 diff、写文件看内容）。 */
  bodyWithoutOutput?: boolean;
  /** 行头已经说完了（「已停止」「已关闭」），不给展开。 */
  noBody?: boolean;
}

const str = (v: unknown) => (typeof v === "string" ? v : "");
/** 折成一行的文字；空串当没有。 */
const oneLine = (v: unknown) => str(v).replace(/\s+/g, " ").trim() || undefined;

export function specFor(tool: string, args: Record<string, unknown>, output: string): Spec {
  const out = output.replace(/\n+$/, "");
  const lines = out ? out.split("\n") : [];
  switch (tool) {
    case "list_dir": {
      // 第一行是解析后的绝对目录，下面一行一项，目录带 `/`。
      const entries = lines.slice(1).filter(Boolean);
      const base = str(args.target_directory) || ".";
      return {
        icon: Folder,
        verb: "List",
        detail: base,
        note: output ? `${entries.length} 项` : undefined,
        body: () => <EntryList base={base} entries={entries} />,
      };
    }
    case "read_file": {
      const path = str(args.target_file);
      const t = fileType(baseName(path));
      return {
        icon: t.icon,
        iconColor: t.color,
        verb: "Read",
        detail: path,
        note: rangeNote(args),
        // 读图片：输出只有「路径 + Image content included inline」，图走缩略图，不再列这两行。
        noBody: /\nImage content included inline\s*$/.test(out) || undefined,
        body: (o, a) => <ReadBody output={o} start={intArg(a.offset) ?? 1} lang={t.lang} />,
      };
    }
    case "bash": {
      const bg = out.startsWith("[Command moved to background]");
      const job = out.match(/job_id: (\S+)/)?.[1];
      return {
        icon: SquareTerminal,
        verb: "Bash",
        summary: oneLine(args.description),
        tag: programLabel(str(args.command)),
        prompt: "$",
        detail: str(args.command).replace(/\s+/g, " "),
        note: bg ? <Badge tone="blue">后台 · {job}</Badge> : undefined,
        body: (o, a) => (bg ? <BackgroundNote job={job} /> : <Terminal command={str(a.command)} output={o} />),
      };
    }
    case "search_replace":
    case "edit_file": {
      const path = str(args.target_file) || str(args.file_path);
      const t = fileType(baseName(path));
      return {
        icon: Pencil,
        iconColor: t.color,
        verb: "Edit",
        detail: path,
        note: <DiffStat oldText={str(args.old_string)} newText={str(args.new_string)} />,
        body: (_, a) => <DiffView oldText={str(a.old_string)} newText={str(a.new_string)} />,
        bodyWithoutOutput: true,
      };
    }
    case "write_file": {
      const path = str(args.target_file) || str(args.file_path);
      const t = fileType(baseName(path));
      const text = str(args.contents) || str(args.content);
      return {
        icon: t.icon,
        iconColor: t.color,
        verb: "Write",
        detail: path,
        note: text ? <span className="font-mono text-[11px] text-[#15803d]">+{text.replace(/\n$/, "").split("\n").length}</span> : undefined,
        body: () => <CodeView code={text.replace(/\n$/, "")} lang={t.lang} />,
        bodyWithoutOutput: true,
      };
    }
    case "glob": {
      const paths = lines.filter(Boolean);
      const none = paths.length === 1 && /no files|no matches/i.test(paths[0]);
      return {
        icon: Files,
        verb: "Glob",
        detail: str(args.glob_pattern) || str(args.pattern),
        note: output ? (none ? "无结果" : `${paths.length} 个文件`) : undefined,
        body: () => (none ? <Plain lines={paths} /> : <PathList paths={paths} />),
      };
    }
    case "grep": {
      const pattern = str(args.pattern) || str(args.query);
      return {
        icon: Search,
        verb: "Search",
        detail: `"${pattern}"`,
        note: [str(args.path), output ? grepNote(str(args.output_mode), lines) : ""].filter(Boolean).join(" · ") || undefined,
        body: () => <GrepResults lines={lines} pattern={pattern} />,
      };
    }
    case "job":
      return jobSpec(args, out);
    case "kill_task":
      return {
        icon: Square,
        verb: "Stop",
        detail: str(args.job_id),
        note: /^killed/.test(out) ? <Badge tone="gray">已停止</Badge> : undefined,
        body: () => <Plain lines={lines} />,
        noBody: /^killed/.test(out),
      };
    case "monitor": {
      const job = out.match(/job_id: (\S+)/)?.[1];
      return {
        icon: Radar,
        verb: "Monitor",
        summary: oneLine(args.description),
        tag: programLabel(str(args.command)),
        detail: str(args.command).replace(/\s+/g, " "),
        note: job ? <Badge tone="blue">后台 · {job}</Badge> : undefined,
        body: (_, a) => <Terminal command={str(a.command)} output="" note={`盯着这条命令的输出${a.timeout_ms ? `，最长 ${Math.round(Number(a.timeout_ms) / 1000)} 秒` : ""}；结果用 job 取。`} />,
      };
    }
    case "web_search": {
      const hits = searchHits(lines);
      return {
        icon: Globe,
        verb: "Web Search",
        detail: `"${str(args.query)}"`,
        note: hits.length ? `${hits.length} 条结果` : undefined,
        body: () => (hits.length ? <WebHits hits={hits} /> : <Plain lines={lines} />),
      };
    }
    case "web_fetch": {
      // 第一行是「HTTP 200 <最终地址>」，正文从空行后开始（已转成 Markdown）。
      const head = lines[0]?.match(/^HTTP (\d+)\s*(.*)$/);
      const code = head ? Number(head[1]) : null;
      const text = head ? lines.slice(lines[1] === "" ? 2 : 1).join("\n") : out;
      return {
        icon: Globe,
        verb: "Fetch",
        detail: str(args.url),
        note: code ? <Badge tone={code < 300 ? "green" : code < 400 ? "amber" : "red"} mono>HTTP {code}</Badge> : undefined,
        body: () => <PagePreview text={text} />,
      };
    }
    case "todo_write": {
      const items = todoItems(output);
      const done = items.filter((t) => t.status === "completed").length;
      return {
        icon: ListChecks,
        verb: "Todo",
        detail: items.length ? `${done}/${items.length} 完成` : "",
        body: () => <Todos items={items} />,
      };
    }
    case "ask_user_question": {
      const questions = Array.isArray(args.questions) ? (args.questions as { question?: unknown }[]) : [];
      return {
        icon: MessageCircleQuestion,
        verb: "Ask",
        detail: questions.map((q) => str(q.question)).filter(Boolean).join(" / "),
        body: (o) => <Answers output={o} />,
      };
    }
  }
  return otherSpec(tool, args, out, lines);
}

/** 不在上面的：计划 / 目标 / 子代理 / 技能 / 记忆 / LSP / 定时 / 扩展 / MCP / 浏览器 / 桌面。 */
function otherSpec(tool: string, args: Record<string, unknown>, out: string, lines: string[]): Spec {
  if (tool === "search_tool") {
    const found = searchToolHits(out);
    return {
      icon: Search,
      verb: "Find Tool",
      detail: str(args.query),
      note: found ? `${found.length} 个` : undefined,
      body: () => (found ? <ToolHits hits={found} /> : <Plain lines={lines} />),
    };
  }
  // use_tool：MCP 和按需工具经它调用，按里面那颗工具画，不出现 use_tool。
  if (tool === "use_tool") {
    const inner = str(args.tool_name);
    const input = args.tool_input && typeof args.tool_input === "object" ? (args.tool_input as Record<string, unknown>) : {};
    if (inner) {
      // 正文要拿里面那颗工具的参数（ToolCard 传进来的是 use_tool 自己的）。
      const s = specFor(inner, input, out);
      return { ...s, body: (o) => s.body(o, input) };
    }
  }
  const mcp = tool.match(/^mcp_(.+?)__(.+)$/);
  if (mcp) return mcpSpec(mcp[1], mcp[2], args, out);
  // 浏览器改成 MCP 之前的进程内工具（旧会话里还有）：同一套动作。
  if (tool.startsWith("browser_")) return browserSpec(tool.slice("browser_".length), args, out);
  const simple = SIMPLE[tool];
  if (simple) return simple(args, out);
  return {
    icon: tool === "task" ? Code2 : Wrench,
    verb: titleCase(tool),
    detail: firstString(args),
    body: (o, a) => <GenericBody args={a} output={o} />,
  };
}

/** 计划、目标、子代理、技能、记忆、LSP、定时、工作流、Cordis：格式见 TOOL-CARDS.md 实测。 */
const SIMPLE: Record<string, (args: Record<string, unknown>, out: string) => Spec> = {
  // 请用户在浏览器面板里接手（登录、验证码…）：一道两选项的提问，回话写着做没做完。
  handoff_to_user: (a, out) => ({
    icon: Hand,
    verb: "Handoff",
    detail: str(a.reason),
    note: out ? <Badge tone={out.startsWith("用户说已经") ? "green" : "amber"}>{out.startsWith("用户说已经") ? "已完成" : "没完成"}</Badge> : undefined,
    body: (o) => <Plain lines={o.split("\n")} tone="ink" />,
  }),
  // 写画布的三颗走 CanvasChip；读的这颗：`id「标题」最新 vN，版本：v1 说明；…` +
  // `--- vN HTML ---` + `--- data ---`。不给 id 是列表，一块画布一行。
  canvas_read: (a, out) => {
    const head = out.match(/^(\S+?)「(.*?)」最新 v(\d+)/);
    const read = out.match(/^([^\n]*)\n\n--- v(\d+) HTML ---\n([\s\S]*?)(?:\n\n--- data ---\n([\s\S]*))?$/);
    return {
      icon: LayoutGrid,
      verb: "Canvas",
      detail: str(a.id) || "全部画布",
      note: read && head ? `「${head[2]}」v${read[2]}${Number(read[2]) !== Number(head[3]) ? ` / 最新 v${head[3]}` : ""}` : undefined,
      body: (o) => (read ? <CanvasRead header={read[1]} html={read[3]} data={read[4] ?? ""} /> : <Plain lines={o.split("\n")} />),
    };
  },
  // 「已进入计划模式\n\n把计划写到 <路径>…」：只露计划文件在哪，后面给模型的守则不要。
  enter_plan_mode: (_, out) => ({
    icon: NotebookPen,
    verb: "Plan",
    detail: "进入计划模式",
    note: out.startsWith("已进入") ? <Badge tone="purple">计划模式</Badge> : undefined,
    body: (o) => {
      const path = o.match(/把计划写到 (\S+?)。/)?.[1];
      return path ? <div className="text-[12px] text-ink-4">计划写在 <span className="font-mono text-ink-3">{tildeTail(path)}</span></div> : <Plain lines={o.split("\n")} />;
    },
  }),
  // 「用户已批准计划。…## 计划：\n<正文>」：正文按 Markdown 排。
  exit_plan_mode: (_, out) => {
    const approved = /^用户已批准/.test(out);
    const plan = out.split(/^## 计划：\s*$/m)[1];
    return {
      icon: NotebookPen,
      verb: "Plan",
      detail: "提交计划",
      note: approved ? <Badge tone="green">已批准</Badge> : out ? <Badge tone="amber">未批准</Badge> : undefined,
      body: (o) => (plan ? <PagePreview text={plan.trim()} /> : <Plain lines={o.split("\n")} />),
    };
  },
  // 参数 objective / message / completed / blocked_reason；结果 `{"success","summary"}`。
  update_goal: (a, out) => {
    const summary = (parseJson(out) as { summary?: string } | undefined)?.summary;
    return {
      icon: Goal,
      verb: "Goal",
      detail: str(a.objective) || str(a.message),
      note: a.completed === true ? <Badge tone="green">已完成</Badge> : str(a.blocked_reason) ? <Badge tone="red">卡住了</Badge> : str(a.objective) && !a.message ? <Badge tone="blue">新目标</Badge> : <Badge tone="gray">进展</Badge>,
      body: (_, x) => (
        <div className="flex flex-col gap-1">
          {(str(x.message) || str(x.blocked_reason)) && (
            <div className={`text-[12.5px] ${str(x.blocked_reason) ? "text-[#b91c1c]" : "text-ink-2"}`}>{str(x.blocked_reason) || str(x.message)}</div>
          )}
          {summary && <div className="text-[11.5px] text-ink-4">{summary}</div>}
        </div>
      ),
      bodyWithoutOutput: true,
    };
  },
  // 结果「message delivered to idle agent <id>; …」。
  send_message: (a, out) => ({
    icon: MessageSquare,
    verb: "Message",
    detail: shortId(str(a.agent_id) || str(a.to) || str(a.recipient)),
    note: /^message delivered/.test(out) ? <Badge tone="green">已送达</Badge> : undefined,
    body: (_, x) => <MessageBody text={str(x.message) || str(x.content)} output="" />,
    bodyWithoutOutput: true,
  }),
  // 一行一个：「<id> [状态] — 角色 — 说明」。
  list_agents: (_, out) => {
    const agents = out
      .split("\n")
      .map((l) => l.match(/^(\S+) \[(\w+)\] — (.+?) — (.*)$/))
      .filter((m): m is RegExpMatchArray => !!m);
    return {
      icon: Bot,
      verb: "Agents",
      detail: agents.length ? `${agents.length} 个子代理` : "",
      body: (o) =>
        agents.length ? (
          <Panel>
            <div className="flex flex-col py-1">
              {agents.map((m) => (
                <div key={m[1]} className="flex min-w-0 items-center gap-2 px-3 py-1 text-[12px]">
                  <Badge tone={m[2] === "running" ? "blue" : m[2] === "idle" ? "gray" : "amber"}>{AGENT_STATUS[m[2]] ?? m[2]}</Badge>
                  <Badge tone="purple">{m[3]}</Badge>
                  <span className="min-w-0 truncate text-ink-2">{m[4]}</span>
                  <span className="ml-auto shrink-0 font-mono text-[10.5px] text-ink-5">{shortId(m[1])}</span>
                </div>
              ))}
            </div>
          </Panel>
        ) : (
          <Plain lines={o.split("\n")} />
        ),
    };
  },
  interrupt_agent: (a, out) => ({
    icon: CircleSlash,
    verb: "Interrupt",
    detail: shortId(str(a.agent_id) || str(a.id)),
    note: /already idle/.test(out) ? <Badge tone="gray">本来就空闲</Badge> : out ? <Badge tone="amber">已打断</Badge> : undefined,
    body: (o) => <Plain lines={o.split("\n")} />,
    noBody: /already idle/.test(out),
  }),
  // `<skill name="…" path="…">` 包着 SKILL.md 正文。
  skill: (a, out) => {
    const m = out.match(/^<skill name="([^"]*)" path="([^"]*)">\n?([\s\S]*?)(?:<\/skill>\s*)?$/);
    return {
      icon: Sparkles,
      verb: "Skill",
      detail: str(a.name) || m?.[1] || firstString(a),
      note: m ? <span className="font-mono text-[11px] text-ink-4">{tildeTail(m[2])}</span> : undefined,
      body: (o) => <PagePreview text={m ? m[3] : o} />,
    };
  },
  // 「Found N memory result(s):」+ 每条「### Result i (score: x, source: s)」「**File:** p (lines a-b)」+ 代码块。
  memory_search: (a, out) => {
    const hits = [...out.matchAll(/### Result \d+ \(score: ([\d.]+), source: ([^)]*)\)\n\*\*File:\*\* (.+?) \(lines (\d+)-(\d+)\)\n```\n([\s\S]*?)\n```/g)];
    return {
      icon: FileSearch,
      verb: "Recall",
      detail: `"${str(a.query)}"`,
      note: out ? (hits.length ? `${hits.length} 条` : "无结果") : undefined,
      body: (o) =>
        hits.length ? (
          <div className="flex flex-col gap-1.5">
            {hits.map((h, i) => (
              <Panel key={i} copy={h[6]}>
                <div className="flex min-w-0 items-center gap-2 border-b border-line-soft px-3 py-1.5">
                  <Badge tone="blue" mono>{Number(h[1]).toFixed(2)}</Badge>
                  <span className="min-w-0 truncate font-mono text-[11px] text-ink-3">{tildeTail(h[3])}</span>
                  <span className="ml-auto shrink-0 text-[10.5px] text-ink-5">第 {h[4]}–{h[5]} 行 · {h[2]}</span>
                </div>
                <div className="whitespace-pre-wrap px-3 py-2 text-[12px] leading-[1.6] text-ink-2">{h[6]}</div>
              </Panel>
            ))}
          </div>
        ) : (
          <div className="text-[12px] text-ink-4">{/^No memory/.test(o) ? "没有相关记忆。" : o}</div>
        ),
    };
  },
  // 「**File:** <路径>\n**Lines:** …」+ 带 `N→` 行号的正文。
  memory_get: (a, out) => {
    const file = out.match(/^\*\*File:\*\* (.+)$/m)?.[1] ?? str(a.path);
    const body = out.replace(/^\*\*File:\*\*.*\n\*\*Lines:\*\*.*\n\n?/, "");
    return {
      icon: Eye,
      verb: "Memory",
      detail: tildeTail(file),
      body: () => <ReadBody output={body} start={1} lang="markdown" />,
    };
  },
  // documentSymbol 一类：「Kind name (路径:行)」一行一个；别的操作原样。
  lsp: (a, out) => {
    const syms = out
      .split("\n")
      .map((l) => l.match(/^(\w+) (.+?) \((.+):(\d+)\)$/))
      .filter((m): m is RegExpMatchArray => !!m);
    return {
      icon: Code2,
      verb: "LSP",
      detail: [str(a.operation) || str(a.action), str(a.file_path) || str(a.query)].filter(Boolean).join(" "),
      note: syms.length ? `${syms.length} 个符号` : undefined,
      body: (o) =>
        syms.length ? (
          <Panel>
            <div className="flex flex-col py-1">
              {syms.map((m, i) => (
                <FileRow
                  key={i}
                  path={`${m[3]}:${m[4]}`}
                  label={m[2]}
                  extra={<span className="ml-auto shrink-0 pl-2 text-[10.5px] text-ink-4">{m[1]} · 第 {m[4]} 行</span>}
                />
              ))}
            </div>
          </Panel>
        ) : (
          <Plain lines={o.split("\n")} />
        ),
    };
  },
  // 「已设定 <id>（<周期>）\n立即执行：否\n7 天后自动过期\n提问：…\n取消：…」。
  scheduler_create: (a, out) => {
    const head = out.match(/^已设定 (\S+)（(.+?)）/);
    const fields = out
      .split("\n")
      .slice(1)
      .map((l) => l.match(/^(.+?)：(.*)$/))
      .filter((m): m is RegExpMatchArray => !!m && m[1] !== "取消" && m[1] !== "提问")
      .map((m) => [m[1], m[2]] as [string, string]);
    const expire = out.match(/^(\d+ 天后自动过期)$/m)?.[1];
    return {
      icon: CalendarClock,
      verb: "Schedule",
      detail: str(a.prompt) || firstString(a),
      note: head ? <Badge tone="blue">{cadence(head[2])}</Badge> : undefined,
      body: (o) =>
        head ? (
          <Panel tone="muted">
            <KeyValues rows={[["任务", head[1]], ["周期", cadence(head[2])], ...fields, ...(expire ? ([["过期", expire]] as [string, string][]) : [])]} />
          </Panel>
        ) : (
          <Plain lines={o.split("\n")} tone="danger" />
        ),
    };
  },
  // 一行一个：「<id>：<周期> — <提问>（下次 9m 52s）」。
  scheduler_list: (_, out) => {
    const tasks = out
      .split("\n")
      .map((l) => l.match(/^(\S+?)：(.+?) — (.*?)(?:（下次 (.+)）)?$/))
      .filter((m): m is RegExpMatchArray => !!m);
    return {
      icon: CalendarClock,
      verb: "Schedules",
      detail: tasks.length ? `${tasks.length} 个任务` : "",
      body: (o) =>
        tasks.length ? (
          <Panel>
            <div className="flex flex-col py-1">
              {tasks.map((m) => (
                <div key={m[1]} className="flex min-w-0 items-center gap-2 px-3 py-1 text-[12px]">
                  <CalendarClock size={13} className="shrink-0 text-ink-4" />
                  <span className="min-w-0 truncate text-ink">{m[3]}</span>
                  <Badge tone="blue">{cadence(m[2])}</Badge>
                  {m[4] && <span className="shrink-0 text-[11px] text-ink-4">下次 {m[4]}</span>}
                  <span className="ml-auto shrink-0 font-mono text-[10.5px] text-ink-5">{m[1]}</span>
                </div>
              ))}
            </div>
          </Panel>
        ) : (
          <Plain lines={o.split("\n")} />
        ),
    };
  },
  scheduler_delete: (a, out) => ({
    icon: CalendarClock,
    verb: "Unschedule",
    detail: str(a.id) || str(a.task_id) || firstString(a),
    note: /^已关闭/.test(out) ? <Badge tone="gray">已关闭</Badge> : undefined,
    body: (o) => <Plain lines={o.split("\n")} />,
    noBody: /^已关闭/.test(out),
  }),
  workflow: (a) => ({ icon: Workflow, verb: "Workflow", detail: [str(a.action), str(a.name)].filter(Boolean).join(" "), body: (o, x) => <GenericBody args={x} output={o} /> }),
};
// Cordis：define 带 Rhai 源码（按 Rust 高亮，语法够像）；run / stop 回 JSON 状态；inspect 是一段说明文字。
for (const name of ["cordis_call", "cordis_promote", "cordis_stop", "cordis_undefine"]) {
  SIMPLE[name] = (a) => ({ icon: Puzzle, verb: titleCase(name.replace("cordis_", "")), detail: str(a.pluginId) || str(a.name) || firstString(a), body: (o, x) => <GenericBody args={x} output={o} /> });
}
SIMPLE.cordis_inspect = (a) => ({ icon: Puzzle, verb: "Inspect", detail: str(a.path) || str(a.pluginId) || "插件树", body: (o) => <Plain lines={o.split("\n")} tone="ink" /> });
SIMPLE.cordis_define = (a, out) => ({
  icon: Puzzle,
  verb: "Define",
  detail: str(a.name) || str(a.purpose),
  note: str(a.factory) ? <Badge tone="purple">{str(a.factory)}</Badge> : undefined,
  body: (o, x) => (
    <div className="flex flex-col gap-1.5">
      {str(x.purpose) && <div className="text-[12px] text-ink-3">{str(x.purpose)}</div>}
      {str(x.source) && <CodeView code={str(x.source)} lang="rust" />}
      {o.trim() && <div className="text-[11.5px] text-ink-4">{o.trim()}</div>}
    </div>
  ),
  bodyWithoutOutput: !!out || !!str(a.source),
});
SIMPLE.cordis_run = (a, out) => {
  const r = parseJson(out) as { status?: string; pluginId?: string; packageId?: string; pluginRunId?: string; host?: { fiberState?: string; provides?: string[]; waitingFor?: string[] } } | undefined;
  return {
    icon: Puzzle,
    verb: "Run",
    detail: [str(a.pluginId), str(a.packageId)].filter(Boolean).join(" · "),
    note: r?.status ? <Badge tone={r.status === "running" ? "green" : r.status === "failed" ? "red" : "gray"}>{r.status}</Badge> : undefined,
    body: (o) =>
      r ? (
        <Panel tone="muted">
          <KeyValues
            rows={[
              ["运行", r.pluginRunId ?? ""],
              ["fiber", r.host?.fiberState ?? ""],
              ["提供", (r.host?.provides ?? []).join(", ") || "—"],
              ["在等", (r.host?.waitingFor ?? []).join(", ") || "—"],
            ]}
          />
        </Panel>
      ) : (
        <Plain lines={o.split("\n")} />
      ),
  };
};

// ---- MCP / 浏览器 / 桌面 ----

const BROWSER_VERB: Record<string, [LucideIcon, string]> = {
  navigate: [Navigation, "Open"],
  navigate_back: [Navigation, "Back"],
  open: [Navigation, "Open"],
  click: [MousePointer2, "Click"],
  hover: [MousePointer2, "Hover"],
  drag: [MousePointer2, "Drag"],
  type: [Keyboard, "Type"],
  press_key: [Keyboard, "Key"],
  fill_form: [Keyboard, "Fill"],
  select_option: [MousePointer2, "Select"],
  screenshot: [Camera, "Screenshot"],
  snapshot: [ListTree, "Snapshot"],
  evaluate: [Code2, "Evaluate"],
  js: [Code2, "Script"],
  tabs: [Globe, "Tabs"],
  console_messages: [SquareTerminal, "Console"],
  network_requests: [Globe, "Network"],
};

function mcpSpec(server: string, name: string, args: Record<string, unknown>, out: string): Spec {
  const action = name.replace(/^browser_/, "");
  if (server === "browser") return browserSpec(action, args, out);
  if (server === "cua-driver") return cuaSpec(name, args, out);
  return {
    icon: Wrench,
    verb: titleCase(name),
    detail: argSummary(args) ?? "",
    note: <Badge tone="gray">{server}</Badge>,
    body: (o, a) => <GenericBody args={a} output={o} />,
  };
}

// ---- 浏览器（内置 MCP browser） ----

/**
 * 浏览器动作的结果大多是一句话（「opened <url>」「clicked @e1 (link)」）：放行头，不给展开。
 * snapshot 是一行一个元素（`- role "名字" [ref=e1]`）；screenshot 的图走 ToolImages；
 * evaluate 回的是 JS 结果。
 */
function browserSpec(action: string, args: Record<string, unknown>, out: string): Spec {
  const [icon, verb] = BROWSER_VERB[action] ?? [Globe, titleCase(action)];
  const base = { icon, verb, note: <Badge tone="blue">浏览器</Badge> };
  switch (action) {
    case "open":
    case "navigate":
    case "navigate_back":
      return {
        ...base,
        // 后退没有参数：地址在结果里（「navigated back to <url>」「opened <url>」）。
        detail: str(args.url) || out.match(/(?:opened|navigated(?: back)? to) (\S+)/)?.[1] || "",
        body: (o) => <Plain lines={o.split("\n")} />,
        noBody: /^(opened|navigated)/.test(out),
      };
    case "click":
    case "hover":
      return { ...base, detail: [str(args.ref), str(args.element)].filter(Boolean).join(" "), body: (o) => <Plain lines={o.split("\n")} />, noBody: true };
    case "snapshot":
      return { ...base, detail: "", body: (o) => <SnapshotTree text={o} /> };
    case "screenshot":
      // 文字只有「saved <路径>」和占位说明：图在 ToolImages 里。
      return { ...base, detail: args.full_page ? "整页" : "", body: () => null, noBody: true };
    case "evaluate":
      return {
        ...base,
        detail: str(args.expression) || str(args.function) || "",
        body: (o) => {
          const json = parseJson(o);
          return json !== undefined ? <CodeView code={JSON.stringify(json, null, 2)} lang="json" /> : <Plain lines={o.split("\n")} tone="ink" />;
        },
      };
    // browser_js：一段 Playwright 脚本（参考 Codex 的 node_repl）。行头写模型给的 title。
    case "js":
      return {
        ...base,
        detail: str(args.title) || (str(args.code).split("\n").find((l) => l.trim()) ?? "").trim(),
        body: (o, a) => <ScriptBody code={str(a.code)} output={o} />,
      };
    case "tabs":
      return {
        ...base,
        detail: str(args.action) || "",
        body: (o) => <TabList text={o} />,
      };
    case "console_messages":
    case "network_requests":
      return {
        ...base,
        detail: "",
        body: (o) => (/^\(no .* yet\)$/.test(o.trim()) ? <div className="text-[12px] text-ink-4">还没有记录。</div> : <Plain lines={o.split("\n")} />),
      };
    case "type":
    case "press_key":
    case "fill_form":
    case "select_option":
      return { ...base, detail: str(args.text) || str(args.key) || str(args.value) || str(args.ref) || "", body: (o, a) => <GenericBody args={a} output={o} /> };
  }
  return { ...base, detail: str(args.url) || str(args.ref) || str(args.text) || "", body: (o, a) => <GenericBody args={a} output={o} /> };
}

/** 可访问性快照：`- role "名字" [ref=eN]`，缩进表示层级。 */
function SnapshotTree({ text }: { text: string }) {
  const rows = text.replace(/\n+$/, "").split("\n");
  const { shown, bar } = usePeek(rows, 16);
  return (
    <Panel copy={text}>
      <div className="thin-scroll overflow-x-auto py-1.5 font-mono text-[11.5px] leading-[1.6]">
        {shown.map((l, i) => {
          const m = l.match(/^(\s*)- (\S+)(?: "(.*?)")?(.*)$/);
          if (!m) return <div key={i} className="whitespace-pre px-3 text-ink-3">{l}</div>;
          // `[ref=e3]` 不一定在行尾（后面还能跟 `value="…"`）。
          const ref = m[4].match(/\[ref=(\w+)\]/)?.[1];
          const rest = m[4].replace(/\s*\[ref=\w+\]/, "").replace(/\s*:$/, "").trim();
          return (
            <div key={i} className="flex min-w-max items-center gap-1.5 px-3" style={{ paddingLeft: 12 + m[1].length * 6 }}>
              <span className="text-ink-4">{m[2]}</span>
              {m[3] !== undefined && <span className="text-ink">"{m[3]}"</span>}
              {ref && <Badge tone="blue" mono>{ref}</Badge>}
              {rest && <span className="text-ink-4">{rest}</span>}
            </div>
          );
        })}
      </div>
      {bar}
    </Panel>
  );
}

/** browser_tabs：`* [0] <url>`（星号是当前页）。 */
function TabList({ text }: { text: string }) {
  const tabs = text
    .split("\n")
    .map((l) => l.match(/^(\*)?\s*\[(\d+)\]\s+(.*)$/))
    .filter((m): m is RegExpMatchArray => !!m);
  if (!tabs.length) return <Plain lines={text.split("\n")} />;
  return (
    <Panel>
      <div className="flex flex-col py-1">
        {tabs.map((m) => (
          <div key={m[2]} className="flex min-w-0 items-center gap-2 px-3 py-0.5 text-[12px]">
            <span className={`size-1.5 shrink-0 rounded-full ${m[1] ? "bg-accent" : "bg-ink-5"}`} />
            <span className="w-5 shrink-0 font-mono text-[10.5px] text-ink-5">{m[2]}</span>
            <span className={`min-w-0 truncate font-mono text-[11.5px] ${m[1] ? "text-ink" : "text-ink-3"}`}>{m[3]}</span>
            {m[1] && <Badge tone="blue">当前</Badge>}
          </div>
        ))}
      </div>
    </Panel>
  );
}

// ---- 桌面（cua-driver） ----

const CUA_VERB: Record<string, [LucideIcon, string]> = {
  click: [MousePointer2, "Click"],
  double_click: [MousePointer2, "Double Click"],
  right_click: [MousePointer2, "Right Click"],
  move_cursor: [MousePointer2, "Move"],
  drag: [MousePointer2, "Drag"],
  scroll: [MousePointer2, "Scroll"],
  type_text: [Keyboard, "Type"],
  press_key: [Keyboard, "Key"],
  hotkey: [Keyboard, "Hotkey"],
  set_value: [Keyboard, "Set Value"],
  get_window_state: [Camera, "Window"],
  get_desktop_state: [Camera, "Desktop"],
  verify_state: [Camera, "Verify"],
  bring_to_front: [Monitor, "Front"],
  zoom: [Camera, "Zoom"],
  list_apps: [Monitor, "Apps"],
  list_windows: [Monitor, "Windows"],
  launch_app: [Monitor, "Launch"],
  kill_app: [Monitor, "Quit"],
};

/** 图的元数据键（zoom 等回的 JSON 只有这些时不列）。 */
const IMAGE_META = new Set(["format", "width", "height", "mime_type"]);

/** 工具结果里图的占位行（图本身走 ToolImages）。 */
const IMAGE_MARKER = /^\[image content will be provided separately\]$/;

/**
 * cua-driver 的结果：第一行一句话（常带 ✅ / ❌），后面常跟一段 JSON。
 * 一句话放行头，JSON 按形状画：窗口列表成表、小对象成键值、别的高亮。
 */
function cuaSpec(name: string, args: Record<string, unknown>, out: string): Spec {
  const [icon, verb] = CUA_VERB[name] ?? [Monitor, titleCase(name)];
  const lines = cuaLines(out);
  const { json } = splitJson(lines.slice(1).join("\n"));
  const effect = json && typeof json === "object" ? (json as { effect?: unknown }).effect : undefined;
  return {
    icon,
    verb,
    detail: cuaDetail(name, args, lines),
    note: (
      <>
        <Badge tone="purple">桌面</Badge>
        {effect === "confirmed" && <Badge tone="green">已确认</Badge>}
        {effect === "unverifiable" && <Badge tone="gray">未验证</Badge>}
      </>
    ),
    body: (o) => <CuaBody output={o} />,
  };
}

/** 去掉图的占位行和尾部空行。 */
function cuaLines(out: string): string[] {
  return out
    .replace(/\n+$/, "")
    .split("\n")
    .filter((l) => !IMAGE_MARKER.test(l));
}

/** 行头的主参数：打的字 / 按键 / 应用 / 元素 / 坐标 / 区域；截窗口的写窗口标题。 */
function cuaDetail(name: string, args: Record<string, unknown>, lines: string[]): string {
  const keys = Array.isArray(args.keys) ? (args.keys as unknown[]).map(String).join("+") : "";
  const named = str(args.text) || keys || str(args.key) || str(args.app_name) || str(args.name) || str(args.bundle_id);
  if (named) return named;
  // 按元素点的：`element_index` 或 `element_token`（`s00000321:13`）；结果里有角色和名字
  // （`Performed AXPress on [13] AXButton "7"`）。
  const element = args.element_index ?? (typeof args.element_token === "string" ? args.element_token.split(":").pop() : undefined);
  if (element !== undefined) {
    const hit = lines[0]?.match(/ on \[\d+\] (AX\w+)(?: "(.*)")?/);
    return [`元素 #${element}`, hit?.[1], hit?.[2] ? `"${hit[2]}"` : ""].filter(Boolean).join(" ");
  }
  if (name === "zoom" && args.x1 !== undefined) return `(${args.x1}, ${args.y1}) – (${args.x2}, ${args.y2})`;
  if (name === "scroll" && args.direction) return `${args.direction}${args.amount !== undefined ? ` × ${args.amount}` : ""}`;
  if (args.x !== undefined && args.y !== undefined) return `(${args.x}, ${args.y})`;
  // get_window_state：`window_id=… pid=… size=… elements=262`，下面是 AX 树，根是窗口。
  const title = lines.find((l) => /^- \[0\] AXWindow /.test(l))?.match(/AXWindow "(.*)"/)?.[1];
  if (title) return title;
  if (args.window_id !== undefined) return `窗口 ${args.window_id}`;
  return "";
}

/** 嵌一层的小对象（`{"delivery":{"mode":…},"effect":…}`）压平成 `delivery.mode` 这样的键。 */
function flatRows(json: Record<string, unknown>): [string, unknown][] | null {
  const rows: [string, unknown][] = [];
  for (const [k, v] of Object.entries(json)) {
    if (v && typeof v === "object" && !Array.isArray(v)) {
      for (const [k2, v2] of Object.entries(v as Record<string, unknown>)) {
        if (v2 && typeof v2 === "object") return null;
        rows.push([`${k}.${k2}`, v2]);
      }
    } else if (Array.isArray(v) && v.some((x) => x && typeof x === "object")) {
      return null;
    } else {
      rows.push([k, v]);
    }
  }
  return rows.length <= 16 ? rows : null;
}

/**
 * 一句话后面：整段是 JSON，或者先几行文字（`Windows:` 列表）再跟一行 JSON。
 * 都不是就当文字。
 */
function splitJson(rest: string): { text: string; json: unknown } {
  const whole = parseJson(rest);
  if (whole !== undefined) return { text: "", json: whole };
  const at = rest.search(/^[{[]/m);
  if (at > 0) {
    const json = parseJson(rest.slice(at));
    if (json !== undefined) return { text: rest.slice(0, at).trim(), json };
  }
  return { text: rest, json: undefined };
}

function CuaBody({ output }: { output: string }) {
  const lines = cuaLines(output);
  const summary = lines[0]?.replace(/^[✅❌⚠️]\s*/u, "");
  const { text, json } = splitJson(lines.slice(1).join("\n"));
  const rest = text;
  const apps = lines
    .slice(1)
    .map((l) => l.match(/^- (.+?) \(pid (\d+)\)(?: \[(.+)\])?$/))
    .filter((m): m is RegExpMatchArray => !!m);
  const rows = json && typeof json === "object" && !Array.isArray(json) ? flatRows(json as Record<string, unknown>) : null;
  // zoom 之类只回图的元数据（格式、宽高）：一句话里已经写了，图走缩略图。
  const flat = rows && rows.every(([k]) => IMAGE_META.has(k)) ? [] : rows;
  return (
    <div className="flex flex-col gap-1.5">
      {summary && <div className={`text-[12px] ${lines[0]?.startsWith("❌") ? "text-[#b91c1c]" : "text-ink-3"}`}>{summary}</div>}
      {apps.length > 0 ? (
        <AppList apps={apps} />
      ) : json && typeof json === "object" && Array.isArray((json as { windows?: unknown }).windows) ? (
        <WindowList windows={(json as { windows: CuaWindow[] }).windows} />
      ) : json === undefined && rest.includes('"windows":[') && /\(truncated\)|…\s*$/.test(rest) ? (
        // 太长被 Dock 截断（末尾「…(truncated)」）：JSON 解析不了，按窗口逐个抠出来。
        <WindowList windows={scrapeWindows(rest)} truncated />
      ) : json && typeof json === "object" && typeof (json as { tree_markdown?: unknown }).tree_markdown === "string" ? (
        // get_window_state 带 JSON 时，要看的是 AX 树，别的字段是驱动的诊断。
        <Plain lines={(json as { tree_markdown: string }).tree_markdown.split("\n")} />
      ) : flat ? (
        flat.length > 0 && (
          <Panel tone="muted">
            <KeyValues rows={flat} />
          </Panel>
        )
      ) : json !== undefined ? (
        <CodeView code={JSON.stringify(json, null, 2)} lang="json" />
      ) : (
        rest.trim() && <Plain lines={rest.replace(/^\n+/, "").split("\n")} />
      )}
    </div>
  );
}

function AppList({ apps }: { apps: RegExpMatchArray[] }) {
  const { shown, bar } = usePeek(apps, 10);
  return (
    <Panel>
      <div className="flex flex-col py-1">
        {shown.map((m) => (
          <div key={m[2]} className="flex min-w-0 items-center gap-2 px-3 py-0.5 text-[12px]">
            <Monitor size={12} className="shrink-0 text-ink-4" />
            <span className="shrink-0 text-ink">{m[1]}</span>
            {m[3] && <span className="min-w-0 truncate font-mono text-[10.5px] text-ink-5">{m[3]}</span>}
            <span className="ml-auto shrink-0 font-mono text-[10.5px] text-ink-5">pid {m[2]}</span>
          </div>
        ))}
      </div>
      {bar}
    </Panel>
  );
}

interface CuaWindow {
  app_name?: string;
  title?: string;
  window_id?: number;
  is_on_screen?: boolean;
  bounds?: { width?: number; height?: number };
}

/** 截断的 list_windows JSON：一个窗口一个对象，字段顺序固定，逐个匹配。 */
function scrapeWindows(text: string): CuaWindow[] {
  const re = /\{"app_name":"((?:[^"\\]|\\.)*)","bounds":\{"height":([\d.]+),"width":([\d.]+)[^}]*\}.*?"is_on_screen":(true|false).*?"title":"((?:[^"\\]|\\.)*)","window_id":(\d+)/g;
  const out: CuaWindow[] = [];
  for (const m of text.matchAll(re)) {
    out.push({ app_name: m[1], bounds: { height: Number(m[2]), width: Number(m[3]) }, is_on_screen: m[4] === "true", title: m[5], window_id: Number(m[6]) });
  }
  return out;
}

function WindowList({ windows, truncated = false }: { windows: CuaWindow[]; truncated?: boolean }) {
  // 屏上的排前面；没标题的系统小窗挪到后面。
  const sorted = [...windows].sort((a, b) => Number(!!b.is_on_screen) - Number(!!a.is_on_screen) || Number(!!b.title) - Number(!!a.title));
  const { shown, bar } = usePeek(sorted, 10);
  return (
    <Panel>
      <div className="flex flex-col py-1">
        {shown.map((w, i) => (
          <div key={w.window_id ?? i} className="flex min-w-0 items-center gap-2 px-3 py-0.5 text-[12px]">
            <span className={`size-1.5 shrink-0 rounded-full ${w.is_on_screen ? "bg-[#34c759]" : "bg-ink-5"}`} />
            <span className="shrink-0 text-ink">{w.app_name}</span>
            <span className="min-w-0 truncate text-ink-4">{w.title || "（无标题）"}</span>
            {w.bounds && (
              <span className="ml-auto shrink-0 font-mono text-[10.5px] text-ink-5">
                {Math.round(w.bounds.width ?? 0)}×{Math.round(w.bounds.height ?? 0)}
              </span>
            )}
          </div>
        ))}
        {truncated && <div className="px-3 pt-1 text-[11px] text-ink-4">结果太长被截断，只列了能读出的 {windows.length} 个。</div>}
      </div>
      {bar}
    </Panel>
  );
}

// ---- 正文 ----

/** 一行超过这么多字先截住（整段 JSON 挤在一行时别铺满屏）。 */
const LINE_CLIP = 600;

function Plain({ lines, tone = "muted" }: { lines: string[]; tone?: "muted" | "ink" | "danger" }) {
  const rows = lines.length && lines[lines.length - 1] === "" ? lines.slice(0, -1) : lines;
  const [wide, setWide] = useState(false);
  const longLine = rows.some((l) => l.length > LINE_CLIP);
  const peek = usePeek(rows);
  const shown = longLine && !wide ? peek.shown.map((l) => (l.length > LINE_CLIP ? `${l.slice(0, LINE_CLIP)} …` : l)) : peek.shown;
  const bar = longLine ? (
    <MoreBar hidden={rows.reduce((n, l) => n + Math.max(0, l.length - LINE_CLIP), 0)} open={wide} onToggle={() => setWide((v) => !v)} unit="字" />
  ) : (
    peek.bar
  );
  if (rows.length === 0 || (rows.length === 1 && !rows[0].trim())) return null;
  return (
    <Panel copy={rows.join("\n")} tone={tone === "danger" ? "danger" : "plain"}>
      <div className="thin-scroll overflow-x-auto px-3 py-2 font-mono text-[11.5px] leading-[1.6]">
        {shown.map((l, i) => (
          <div key={i} className={`whitespace-pre-wrap break-all ${tone === "danger" ? "text-[#b91c1c]" : tone === "ink" ? "text-ink" : "text-ink-3"}`}>
            {l || " "}
          </div>
        ))}
      </div>
      {bar}
    </Panel>
  );
}

/** 终端：`$ 命令` 一行 + 输出；报错样的行标红。 */
function Terminal({ command, output, note }: { command: string; output: string; note?: string }) {
  const rows = stripAnsi(output.replace(/\n+$/, "")).split("\n");
  const exit = rows[0]?.match(/^exit (?:exit status: )?(\d+)$/);
  const body = exit ? rows.slice(1) : rows;
  const { shown, bar } = usePeek(body.length === 1 && body[0] === "" ? [] : body);
  return (
    <Panel tone="terminal" copy={output}>
      <div className="thin-scroll overflow-x-auto px-3 py-2 font-mono text-[11.5px] leading-[1.6]">
        <div className="whitespace-pre-wrap break-all text-ink">
          <span className="select-none text-[#2b9a4a]">$ </span>
          {command}
        </div>
        {shown.map((l, i) => (
          <div key={i} className={`whitespace-pre-wrap break-all ${looksFailedLine(l) ? "text-[#b91c1c]" : "text-ink-3"}`}>
            {l || " "}
          </div>
        ))}
        {note && <div className="pt-1 font-sans text-[11px] text-ink-4">{note}</div>}
      </div>
      {bar}
    </Panel>
  );
}

function BackgroundNote({ job }: { job?: string }) {
  return (
    <div className="text-[12px] text-ink-4">
      命令转到后台跑{job ? `（${job}）` : ""}，输出用 job 取。
    </div>
  );
}

function jobSpec(args: Record<string, unknown>, out: string): Spec {
  const jobs = parseJobs(out);
  const ids = Array.isArray(args.job_ids) ? (args.job_ids as unknown[]).map(String) : [];
  const one = jobs.length === 1 ? jobs[0] : null;
  return {
    icon: SquareTerminal,
    verb: "Job",
    detail: one ? one.id : ids.join(", ") || "全部任务",
    note: one ? <Badge tone={JOB_TONE[one.status] ?? "gray"}>{JOB_TEXT[one.status] ?? one.status}</Badge> : jobs.length ? `${jobs.length} 个任务` : undefined,
    body: () =>
      jobs.length ? (
        <div className="flex flex-col gap-1.5">
          {jobs.map((j) => (
            <Panel key={j.id} tone="terminal" copy={j.output}>
              <div className="flex items-center gap-2 border-b border-line-soft px-3 py-1.5">
                {jobs.length > 1 && <Badge tone={JOB_TONE[j.status] ?? "gray"}>{JOB_TEXT[j.status] ?? j.status}</Badge>}
                <span className="font-mono text-[11px] text-ink-4">{j.id}</span>
                <span className="min-w-0 truncate font-mono text-[11.5px] text-ink">{j.command}</span>
              </div>
              {j.output.trim() && <JobOutput text={j.output} />}
            </Panel>
          ))}
        </div>
      ) : (
        <Plain lines={out.split("\n")} />
      ),
  };
}

const JOB_TONE: Record<string, BadgeTone> = { running: "blue", done: "green", failed: "red", killed: "gray", timeout: "amber", cancelled: "gray" };
const JOB_TEXT: Record<string, string> = { running: "运行中", done: "已完成", failed: "失败", killed: "已停止", timeout: "超时", cancelled: "已取消" };

function JobOutput({ text }: { text: string }) {
  const rows = stripAnsi(text.replace(/\n+$/, "")).split("\n");
  const { shown, bar } = usePeek(rows);
  return (
    <>
      <div className="thin-scroll overflow-x-auto px-3 py-1.5 font-mono text-[11.5px] leading-[1.6] text-ink-3">
        {shown.map((l, i) => (
          <div key={i} className="whitespace-pre-wrap break-all">
            {l || " "}
          </div>
        ))}
      </div>
      {bar}
    </>
  );
}

/** job 的输出：`[状态] job-N 命令` 起头，下面（可能缩进）是这个任务的输出。 */
function parseJobs(out: string): { id: string; status: string; command: string; output: string }[] {
  const jobs: { id: string; status: string; command: string; output: string[] }[] = [];
  for (const line of out.split("\n")) {
    const m = line.match(/^\[(\w+)\] (\S+) (.*)$/);
    if (m) jobs.push({ status: m[1], id: m[2], command: m[3], output: [] });
    else if (jobs.length) jobs[jobs.length - 1].output.push(line.replace(/^ {4}/, ""));
  }
  return jobs.map((j) => ({ ...j, output: j.output.join("\n") }));
}

function ReadBody({ output, start, lang }: { output: string; start: number; lang: string }) {
  // Dock 只给第 1 行和每逢 10 的行标行号（`N→`），其余行照顺序数；末尾可能有截断说明。
  const [body, trailer] = splitTrailer(output.replace(/\n+$/, ""));
  let n = start - 1;
  const numbers: number[] = [];
  const texts = body.split("\n").map((row) => {
    const m = row.match(/^(\d+)→(.*)$/);
    n = m ? Number(m[1]) : n + 1;
    numbers.push(n);
    return m ? m[2] : row;
  });
  return (
    <div className="flex flex-col gap-1">
      <CodeView code={texts.join("\n")} lang={lang} numbers={numbers} />
      {trailer && <div className="text-[11px] text-ink-4">{trailer}</div>}
    </div>
  );
}

function EntryList({ base, entries }: { base: string; entries: string[] }) {
  const prefix = base === "." || base === "./" ? "" : `${base.replace(/\/$/, "")}/`;
  const { shown, bar } = usePeek(entries, 16);
  return (
    <Panel>
      <div className="grid grid-cols-[repeat(auto-fill,minmax(180px,1fr))] gap-x-1 py-1">
        {shown.map((e) => (
          <FileRow key={e} path={prefix + e} label={e.replace(/\/$/, "")} dir={e.endsWith("/")} />
        ))}
      </div>
      {bar}
    </Panel>
  );
}

function PathList({ paths }: { paths: string[] }) {
  const { shown, bar } = usePeek(paths);
  return (
    <Panel>
      <div className="flex flex-col py-1">
        {shown.map((p) => (
          <FileRow key={p} path={p} />
        ))}
      </div>
      {bar}
    </Panel>
  );
}

/** grep：按文件分组；命中行 `路径:行:正文`，上下文行 `路径-行-正文`，命中的词标出来。 */
function GrepResults({ lines, pattern }: { lines: string[]; pattern: string }) {
  const rows = lines.filter(Boolean);
  const groups: { file: string; hits: { n: string | null; text: string; ctx: boolean }[] }[] = [];
  let plain = false;
  for (const line of rows) {
    const hit = line.match(/^(.+?):(\d+):(.*)$/);
    const ctx = !hit ? line.match(/^(.+?)-(\d+)-(.*)$/) : null;
    const count = !hit && !ctx ? line.match(/^(.+?):(\d+)$/) : null;
    const m = hit ?? ctx;
    const file = m?.[1] ?? count?.[1] ?? (/[/.]/.test(line) && !/\s/.test(line) ? line : null);
    if (!file) {
      plain = true;
      break;
    }
    let g = groups[groups.length - 1];
    if (!g || g.file !== file) groups.push((g = { file, hits: [] }));
    if (m) g.hits.push({ n: m[2], text: m[3], ctx: !!ctx });
    else if (count) g.hits.push({ n: null, text: `${count[2]} 处`, ctx: true });
  }
  if (plain || groups.length === 0) return <Plain lines={lines} />;
  let re: RegExp | null = null;
  try {
    re = pattern ? new RegExp(pattern, "gi") : null;
  } catch {
    re = null;
  }
  return <GrepGroups groups={groups} re={re} />;
}

function GrepGroups({ groups, re }: { groups: { file: string; hits: { n: string | null; text: string; ctx: boolean }[] }[]; re: RegExp | null }) {
  const { shown, bar } = usePeek(groups, 8);
  return (
    <Panel>
      <div className="flex flex-col py-1">
        {shown.map((g) => (
          <div key={g.file} className="flex flex-col">
            <FileRow path={g.file} extra={g.hits.some((h) => h.n) ? <span className="ml-1 text-[10.5px] text-ink-4">{g.hits.filter((h) => !h.ctx).length}</span> : undefined} />
            {g.hits.map((h, i) =>
              h.n ? (
                <div key={i} className="flex min-w-0 gap-2 pl-[30px] pr-3 font-mono text-[11.5px] leading-[1.6]">
                  <span className="w-7 shrink-0 text-right text-[10.5px] text-ink-5">{h.n}</span>
                  <span className={`min-w-0 truncate ${h.ctx ? "text-ink-5" : "text-ink-2"}`}>{mark(h.text.trim(), h.ctx ? null : re)}</span>
                </div>
              ) : null,
            )}
          </div>
        ))}
      </div>
      {bar}
    </Panel>
  );
}

/** 把正则命中的片段包成黄底。 */
function mark(text: string, re: RegExp | null): ReactNode {
  if (!re) return text;
  const out: ReactNode[] = [];
  let last = 0;
  re.lastIndex = 0;
  for (let m = re.exec(text); m && m[0]; m = re.exec(text)) {
    out.push(text.slice(last, m.index));
    out.push(
      <mark key={m.index} className="rounded-[3px] bg-[#fff1a8] px-px text-ink">
        {m[0]}
      </mark>,
    );
    last = m.index + m[0].length;
  }
  out.push(text.slice(last));
  return out;
}

/** 行级 diff（LCS），改了的一对行再标出中间变了的那段。 */
function DiffView({ oldText, newText }: { oldText: string; newText: string }) {
  const rows = diffLines(oldText ? oldText.split("\n") : [], newText ? newText.split("\n") : []);
  const { shown, bar } = usePeek(rows, 20);
  return (
    <Panel copy={newText}>
      <div className="thin-scroll overflow-x-auto py-1 font-mono text-[11.5px] leading-[1.6]">
        {shown.map((r, i) => (
          <div
            key={i}
            className={`flex min-w-max pr-4 ${r.kind === "del" ? "bg-[#fdecec]" : r.kind === "add" ? "bg-[#e9f7ee]" : ""}`}
          >
            <span
              className={`w-6 shrink-0 select-none text-center ${r.kind === "del" ? "text-[#c0392b]" : r.kind === "add" ? "text-[#1a7f37]" : "text-ink-5"}`}
            >
              {r.kind === "del" ? "−" : r.kind === "add" ? "+" : " "}
            </span>
            <span className={`whitespace-pre ${r.kind === "del" ? "text-[#8f1d1d]" : r.kind === "add" ? "text-[#14532d]" : "text-ink-3"}`}>
              {r.parts
                ? r.parts.map((p, j) =>
                    p.changed ? (
                      <span key={j} className={r.kind === "del" ? "rounded-[2px] bg-[#f8c4c4]" : "rounded-[2px] bg-[#b7e4c7]"}>
                        {p.text}
                      </span>
                    ) : (
                      <span key={j}>{p.text}</span>
                    ),
                  )
                : r.text || " "}
            </span>
          </div>
        ))}
      </div>
      {bar}
    </Panel>
  );
}

type DiffRow = { kind: "same" | "del" | "add"; text: string; parts?: { text: string; changed: boolean }[] };

function diffLines(a: string[], b: string[]): DiffRow[] {
  // 编辑片段都不大：O(n·m) 的 LCS 够用；太大就退回整段删 + 整段加。
  if (a.length * b.length > 250_000) return [...a.map((text) => ({ kind: "del" as const, text })), ...b.map((text) => ({ kind: "add" as const, text }))];
  const dp = Array.from({ length: a.length + 1 }, () => new Array<number>(b.length + 1).fill(0));
  for (let i = a.length - 1; i >= 0; i--) for (let j = b.length - 1; j >= 0; j--) dp[i][j] = a[i] === b[j] ? dp[i + 1][j + 1] + 1 : Math.max(dp[i + 1][j], dp[i][j + 1]);
  const rows: DiffRow[] = [];
  let i = 0;
  let j = 0;
  while (i < a.length || j < b.length) {
    if (i < a.length && j < b.length && a[i] === b[j]) {
      rows.push({ kind: "same", text: a[i] });
      i++;
      j++;
    } else if (i < a.length && (j >= b.length || dp[i + 1][j] >= dp[i][j + 1])) {
      // 平手先删后加：读起来是「这行改成了那行」。
      rows.push({ kind: "del", text: a[i++] });
    } else {
      rows.push({ kind: "add", text: b[j++] });
    }
  }
  // 删一行紧跟加一行：标出中间改了的那段。
  for (let k = 0; k + 1 < rows.length; k++) {
    const d = rows[k];
    const ad = rows[k + 1];
    if (d.kind !== "del" || ad.kind !== "add") continue;
    let p = 0;
    while (p < d.text.length && p < ad.text.length && d.text[p] === ad.text[p]) p++;
    let s = 0;
    while (s < d.text.length - p && s < ad.text.length - p && d.text[d.text.length - 1 - s] === ad.text[ad.text.length - 1 - s]) s++;
    if (p + s === 0) continue;
    const split = (t: string) => [
      { text: t.slice(0, p), changed: false },
      { text: t.slice(p, t.length - s), changed: true },
      { text: t.slice(t.length - s), changed: false },
    ];
    d.parts = split(d.text);
    ad.parts = split(ad.text);
    k++;
  }
  return rows;
}

function DiffStat({ oldText, newText }: { oldText: string; newText: string }) {
  const rows = diffLines(oldText ? oldText.split("\n") : [], newText ? newText.split("\n") : []);
  const add = rows.filter((r) => r.kind === "add").length;
  const del = rows.filter((r) => r.kind === "del").length;
  if (!add && !del) return null;
  return (
    <span className="font-mono text-[11px]">
      <span className="text-[#15803d]">+{add}</span> <span className="text-[#c0392b]">−{del}</span>
    </span>
  );
}

/** web_search 的结果行 `N. <百分号编码的跳转地址>`：解码、去掉跟踪参数。 */
function searchHits(lines: string[]): URL[] {
  const hits: URL[] = [];
  for (const line of lines) {
    const m = line.match(/^\d+\.\s+(\S+)/);
    if (!m) continue;
    let raw = m[1].replace(/\)$/, "");
    try {
      raw = decodeURIComponent(raw);
    } catch {
      // 不是编码过的就原样。
    }
    raw = raw.replace(/&rut=[0-9a-f]+$/i, "");
    try {
      hits.push(new URL(raw));
    } catch {
      // 不是地址的行跳过。
    }
  }
  return hits;
}

function WebHits({ hits }: { hits: URL[] }) {
  const { shown, bar } = usePeek(hits, 8);
  return (
    <Panel>
      <div className="flex flex-col py-1">
        {shown.map((u, i) => (
          <a
            key={i}
            href={u.href}
            target="_blank"
            rel="noreferrer"
            className="motion-colors flex min-w-0 items-center gap-2 px-2.5 py-1 hover:bg-[#eef5ff]"
          >
            <span className="w-4 shrink-0 text-right font-mono text-[10.5px] text-ink-5">{i + 1}</span>
            <Globe size={13} className="shrink-0 text-ink-4" />
            <span className="shrink-0 text-[12px] font-medium text-ink">{u.hostname.replace(/^www\./, "")}</span>
            <span className="min-w-0 truncate font-mono text-[11px] text-ink-4">{decodeURI(u.pathname + u.search)}</span>
          </a>
        ))}
      </div>
      {bar}
    </Panel>
  );
}

/** 网页 / 技能 / 记忆正文：Markdown 排版，限高可滚。 */
/** 收起时 Markdown 正文露这么高（px），下沿渐隐。 */
const PAGE_PEEK_PX = 320;

function PagePreview({ text }: { text: string }) {
  const [open, setOpen] = useState(false);
  const [tall, setTall] = useState(false);
  const measure = useCallback((el: HTMLDivElement | null) => {
    if (el) setTall(el.scrollHeight > PAGE_PEEK_PX + 40);
  }, []);
  if (!text.trim()) return null;
  const clipped = tall && !open;
  return (
    <Panel copy={text}>
      <div
        ref={measure}
        className="overflow-hidden px-4 py-3 [--chat-text:13px]"
        style={clipped ? { maxHeight: PAGE_PEEK_PX, maskImage: "linear-gradient(to bottom, #000 75%, transparent)" } : undefined}
      >
        <Markdown text={text} />
      </div>
      {tall && (
        <button
          type="button"
          onClick={() => setOpen((v) => !v)}
          className="md-surface-head motion-colors flex w-full items-center justify-center gap-1 border-t border-line-soft bg-[#fafafb] py-1 text-[11px] text-ink-4 hover:bg-hover hover:text-ink-2"
        >
          <ChevronDown size={12} className={`transition-transform duration-[var(--dur-base)] ${open ? "rotate-180" : ""}`} />
          {open ? "收起" : "展开全文"}
        </button>
      )}
    </Panel>
  );
}

/** browser_js 的正文：脚本（高亮）+ log + 返回值；截图走 ToolImages。 */
function ScriptBody({ code, output }: { code: string; output: string }) {
  // 输出：可选的「log:\n…」段、结果、可选的「screenshot: <路径>」，段与段空一行。
  const parts = stripAnsi(output).split("\n\n");
  const logs = parts[0]?.startsWith("log:\n") ? parts.shift()!.slice(5) : "";
  const rest = parts.filter((p) => !p.startsWith("screenshot: ")).join("\n\n").trim();
  const json = parseJson(rest);
  return (
    <div className="flex flex-col gap-1.5">
      {code && <CodeView code={code} lang="javascript" />}
      {logs && <Plain lines={logs.split("\n")} />}
      {json !== undefined ? (
        <CodeView code={JSON.stringify(json, null, 2)} lang="json" />
      ) : (
        rest && <Plain lines={rest.split("\n")} tone={rest.startsWith("Error:") ? "danger" : "ink"} />
      )}
    </div>
  );
}

function CanvasRead({ header, html, data }: { header: string; html: string; data: string }) {
  // 版本说明：`v1 初版；v2 …`，新的在后面——倒过来，最新的在上。
  const versions = (header.split("，版本：")[1] ?? "").split("；").filter(Boolean).reverse();
  const { shown, bar } = usePeek(versions, 3);
  const hasData = data.trim() && !["null", "{}", "[]"].includes(data.trim());
  return (
    <div className="flex flex-col gap-1.5">
      {versions.length > 0 && (
        <Panel tone="muted">
          <div className="flex flex-col py-1">
            {shown.map((v, i) => {
              const m = v.match(/^v(\d+)\s*(.*)$/);
              return (
                <div key={i} className="flex min-w-0 items-baseline gap-2 px-3 py-0.5 text-[11.5px]">
                  <span className="w-8 shrink-0 font-mono text-ink-4">v{m?.[1] ?? "?"}</span>
                  <span className="min-w-0 text-ink-2">{m?.[2] || "—"}</span>
                </div>
              );
            })}
          </div>
          {bar}
        </Panel>
      )}
      <CodeView code={html} lang="html" />
      {hasData && <CodeView code={prettyJson(data)} lang="json" />}
    </div>
  );
}

function prettyJson(s: string): string {
  const v = parseJson(s);
  return v === undefined ? s : JSON.stringify(v, null, 2);
}

function MessageBody({ text, output }: { text: string; output: string }) {
  return (
    <div className="flex flex-col gap-1.5">
      {text && <div className="w-fit max-w-full whitespace-pre-wrap rounded-[14px] rounded-tl-sm bg-[#eaf3ff] px-3 py-1.5 text-[12.5px] text-ink">{text}</div>}
      {output.trim() && <div className="text-[11.5px] text-ink-4">{output.trim()}</div>}
    </div>
  );
}

/** 输出像 Markdown 文档（有标题行，或成对的代码围栏）就按 Markdown 排。 */
function looksMarkdown(s: string): boolean {
  return /^#{1,4} \S/m.test(s) || (s.match(/^```/gm)?.length ?? 0) >= 2;
}

function GenericBody({ args, output }: { args: Record<string, unknown>; output: string }) {
  const entries = Object.entries(args);
  const json = parseJson(output);
  // 只有一个简单参数时行头（argSummary）已经写了，不再列一遍。
  const inHeader = entries.length === 1 && ["string", "number", "boolean"].includes(typeof entries[0][1]) && String(entries[0][1]).length <= 40;
  return (
    <div className="flex flex-col gap-1.5">
      {entries.length > 0 && !inHeader && (
        <Panel tone="muted">
          <KeyValues rows={entries} />
        </Panel>
      )}
      {json !== undefined ? (
        <CodeView code={JSON.stringify(json, null, 2)} lang="json" />
      ) : looksMarkdown(output) ? (
        <PagePreview text={output} />
      ) : (
        output.trim() && <Plain lines={stripAnsi(output).split("\n")} />
      )}
    </div>
  );
}

function parseJson(s: string): unknown {
  const t = s.trim();
  if (!/^[[{]/.test(t)) return undefined;
  try {
    return JSON.parse(t);
  } catch {
    return undefined;
  }
}

/** `awaiting_user`：在等用户回答或确认（不算完成，也不是模型欠的活）。 */
type TodoStatus = "pending" | "in_progress" | "completed" | "cancelled" | "awaiting_user";

/** todo_write 输出里的「- [状态] id: 内容」行；后面给模型的提示不要。 */
function todoItems(output: string): { status: TodoStatus; text: string }[] {
  return output
    .split("\n")
    .map((l) => l.match(/^- \[(\w+)\] [^:]+: (.*)$/))
    .filter((m): m is RegExpMatchArray => !!m)
    .map((m) => ({ status: m[1] as TodoStatus, text: m[2] }));
}

function Todos({ items }: { items: { status: TodoStatus; text: string }[] }) {
  const done = items.filter((t) => t.status === "completed").length;
  return (
    <Panel>
      <div className="flex flex-col gap-1.5 px-3 py-2">
        {items.length > 0 && (
          <div className="mb-0.5 h-1 overflow-hidden rounded-full bg-fill">
            <div className="h-full rounded-full bg-[#34c759] transition-[width] duration-[var(--dur-base)]" style={{ width: `${(done / items.length) * 100}%` }} />
          </div>
        )}
        {items.map((t, i) => (
          <div key={i} className="flex items-center gap-2 text-[12.5px]">
            {t.status === "completed" ? (
              <CircleCheck size={14} className="shrink-0 text-[#34c759]" />
            ) : t.status === "in_progress" ? (
              <CircleDot size={14} className="shrink-0 text-accent" />
            ) : t.status === "awaiting_user" ? (
              <MessageCircleQuestion size={14} className="shrink-0 text-[#ff9f0a]" />
            ) : (
              <Circle size={14} className="shrink-0 text-ink-5" />
            )}
            <span
              className={
                t.status === "completed" || t.status === "cancelled"
                  ? "text-ink-4 line-through"
                  : t.status === "in_progress"
                    ? "font-medium text-ink"
                    : t.status === "awaiting_user"
                      ? "text-ink-3"
                      : "text-ink-2"
              }
            >
              {t.text}
            </span>
          </div>
        ))}
      </div>
    </Panel>
  );
}

/** ask_user_question 的结果：`"问题"="回答"` 一对对拆出来；认不出就原样。 */
function Answers({ output }: { output: string }) {
  const pairs = [...output.matchAll(/"([^"]+)"="([^"]*)"/g)];
  if (pairs.length === 0) return <Plain lines={output.split("\n")} />;
  return (
    <Panel>
      <div className="flex flex-col divide-y divide-line-soft">
        {pairs.map((p, i) => (
          <div key={i} className="flex flex-col gap-0.5 px-3 py-2">
            <span className="text-[11.5px] text-ink-4">{p[1]}</span>
            <span className="text-[13px] font-medium text-ink">{p[2]}</span>
          </div>
        ))}
      </div>
    </Panel>
  );
}

/** search_tool 的 JSON 结果 → 找到的工具（服务器 + 名字 + 一句说明）；不是 JSON 就 null。 */
function searchToolHits(output: string): { server: string; name: string; description: string }[] | null {
  try {
    const parsed = JSON.parse(output) as {
      results?: { server?: string; tools?: { tool_name?: string; name?: string; description?: string }[] }[];
    };
    if (!Array.isArray(parsed.results)) return null;
    return parsed.results.flatMap((r) =>
      (r.tools ?? []).map((t) => {
        // 现在的字段是 tool_name，早先是 name。
        const full = t.tool_name ?? t.name ?? "";
        return {
          server: r.server ?? "",
          name: full.match(/^mcp_.+?__(.+)$/)?.[1] ?? full,
          description: (t.description ?? "").split(/(?<=[.。])\s/)[0],
        };
      }),
    );
  } catch {
    return null;
  }
}

function ToolHits({ hits }: { hits: { server: string; name: string; description: string }[] }) {
  const { shown, bar } = usePeek(hits, 8);
  return (
    <Panel>
      <div className="flex flex-col py-1">
        {shown.map((h, i) => (
          <div key={i} className="flex min-w-0 items-center gap-2 px-3 py-1 text-[12px]">
            <Badge tone="gray">{h.server}</Badge>
            <span className="shrink-0 font-mono font-medium text-ink">{h.name}</span>
            {h.description && <span className="min-w-0 truncate text-ink-4">{h.description}</span>}
          </div>
        ))}
      </div>
      {bar}
    </Panel>
  );
}

// ---- 小工具 ----

/** 「every 10 minutes」→「每 10 分钟」；认不出的原样。 */
function cadence(s: string): string {
  const m = s.match(/^every (\d+) (second|minute|hour|day|week)s?$/);
  if (m) return `每 ${m[1]} ${{ second: "秒", minute: "分钟", hour: "小时", day: "天", week: "周" }[m[2]]}`;
  const one = s.match(/^every (second|minute|hour|day|week)$/);
  if (one) return `每${{ second: "秒", minute: "分钟", hour: "小时", day: "天", week: "周" }[one[1]]}`;
  return s;
}

const AGENT_STATUS: Record<string, string> = { running: "运行中", idle: "空闲", queued: "排队" };

/** 子代理 id 很长：留前 8 位。 */
function shortId(id: string): string {
  return id.length > 12 ? id.slice(0, 8) : id;
}

/** 长绝对路径只留最后三段（计划文件在 DOCK_HOME 深处）。 */
function tildeTail(path: string): string {
  const parts = path.split("/");
  return parts.length > 4 ? `…/${parts.slice(-3).join("/")}` : path;
}

function baseName(path: string): string {
  return path.split("/").pop() ?? path;
}

function intArg(v: unknown): number | undefined {
  const n = typeof v === "number" ? v : typeof v === "string" ? Number(v) : NaN;
  return Number.isFinite(n) && n > 0 ? Math.floor(n) : undefined;
}

function grepNote(mode: string, lines: string[]): string {
  const rows = lines.filter(Boolean);
  if (rows.length === 1 && /no matches/i.test(rows[0])) return "无结果";
  if (mode === "files_with_matches" || mode === "files" || mode === "count") return `${rows.length} 个文件`;
  const hits = rows.filter((l) => /^.+?:\d+:/.test(l)).length;
  const files = new Set(rows.map((l) => l.match(/^(.+?):\d+:/)?.[1]).filter(Boolean)).size;
  return hits ? `${hits} 处 · ${files} 个文件` : "";
}

function splitTrailer(out: string): [string, string] {
  const at = out.lastIndexOf("\n\n[… truncated");
  if (at < 0) return [out, ""];
  return [out.slice(0, at), out.slice(at + 2).replace(/^\[|\]$/g, "")];
}

function rangeNote(args: Record<string, unknown>): string | undefined {
  const offset = Number(args.offset);
  const limit = Number(args.limit);
  if (!offset && !limit) return undefined;
  const start = offset || 1;
  return limit ? `第 ${start}–${start + limit - 1} 行` : `从第 ${start} 行`;
}

export function looksFailedLine(line: string): boolean {
  // FAILED 区分大小写：cargo 的「0 failed」是通过，不能标红。
  return /\bFAILED\b|^\s*(error|Error|ERROR)\b|panicked/.test(line) || /No such file|command not found|not found/i.test(line);
}

export function exitCode(output: string): string | undefined {
  const code = output.match(/(?:^|\n)exit (?:exit status: )?(\d+)/)?.[1];
  return code ? `exit ${code}` : undefined;
}

function argSummary(args: Record<string, unknown>): string | undefined {
  const simple = Object.entries(args).filter(([, v]) => ["string", "number", "boolean"].includes(typeof v));
  if (simple.length === 0) return undefined;
  // 先挑短的单行参数（id、名字），整段代码 / 长文放正文。
  const [k, v] = simple.find(([, v]) => !String(v).includes("\n") && String(v).length <= 60) ?? simple[0];
  const more = Object.keys(args).length - 1;
  return `${k}: ${String(v).slice(0, 40)}${more > 0 ? ` 等 ${more + 1} 项` : ""}`;
}

export function titleCase(s: string): string {
  return s
    .split(/[_\s-]+/)
    .filter(Boolean)
    .map((w) => w[0].toUpperCase() + w.slice(1))
    .join(" ");
}

function firstString(args: Record<string, unknown>): string {
  for (const v of Object.values(args)) {
    if (typeof v === "string" && v.trim()) return v.replace(/\s+/g, " ");
  }
  return "";
}
