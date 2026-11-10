// 预设编辑器工具目录的分组（设计 tway：按类分组、每项一个短标签）。
// Dock 的 `tool/catalog` 只给名字和简介，分组是界面上的事，按名字归类；认不得的进「其他」。

import type { CatalogTool } from "../../model";

export interface ToolGroup {
  id: string;
  label: string;
  /** 行尾的短标签（设计里的 web / io / browser）。 */
  tag: string;
  match: (name: string) => boolean;
}

const names = (...list: string[]) => (n: string) => list.includes(n);
const prefix = (p: string) => (n: string) => n.startsWith(p);

export const TOOL_GROUPS: ToolGroup[] = [
  {
    id: "files",
    label: "文件读写与代码 (Files & Code)",
    tag: "io",
    match: names("list_dir", "read_file", "grep", "glob", "search_replace", "write_file"),
  },
  {
    id: "shell",
    label: "终端与后台任务 (Shell & Jobs)",
    tag: "shell",
    match: (n) =>
      names("bash", "run_terminal_cmd", "job", "kill_task", "monitor")(n) || prefix("scheduler_")(n),
  },
  {
    id: "plan",
    label: "计划与提问 (Plan & Ask)",
    tag: "plan",
    match: names("enter_plan_mode", "exit_plan_mode", "ask_user_question", "todo_write", "update_goal"),
  },
  { id: "web", label: "网页与检索 (Web & Search)", tag: "web", match: names("web_fetch", "web_search") },
  { id: "browser", label: "浏览器 (Browser)", tag: "browser", match: prefix("browser_") },
  {
    id: "agents",
    label: "子代理与协作 (Agents)",
    tag: "agent",
    match: names("task", "send_message", "list_agents", "interrupt_agent"),
  },
  { id: "memory", label: "记忆 (Memory)", tag: "memory", match: prefix("memory_") },
  { id: "skills", label: "技能与工作流 (Skills & Workflow)", tag: "skill", match: names("skill", "workflow") },
  { id: "mcp", label: "MCP 调度 (MCP)", tag: "mcp", match: names("search_tool", "use_tool") },
  { id: "lsp", label: "语言服务与插件 (LSP & Plugins)", tag: "lsp", match: (n) => n === "lsp" || prefix("cordis_")(n) },
];

const OTHER: ToolGroup = { id: "other", label: "其他 (Other)", tag: "tool", match: () => true };

export function groupOf(name: string): ToolGroup {
  return TOOL_GROUPS.find((g) => g.match(name)) ?? OTHER;
}

/** 目录按组排好；组内保持 Dock 给的顺序。空组不出现。 */
export function groupTools(tools: CatalogTool[]): { group: ToolGroup; tools: CatalogTool[] }[] {
  const out = [...TOOL_GROUPS, OTHER].map((group) => ({ group, tools: [] as CatalogTool[] }));
  for (const t of tools) out.find((g) => g.group === groupOf(t.name))!.tools.push(t);
  return out.filter((g) => g.tools.length > 0);
}

/** 界面上的中文简介（Dock 给的是写给模型的英文描述）。没收录的用 Dock 的。 */
const ZH: Record<string, string> = {
  list_dir: "列出目录内容",
  read_file: "读取文件内容，可按行号范围",
  grep: "按正则搜索文件内容",
  glob: "按通配符找文件",
  search_replace: "精准替换文件里的一段内容",
  write_file: "新建文件或整份覆盖",
  bash: "在工作区里跑 shell 命令",
  run_terminal_cmd: "在工作区里跑 shell 命令（bash 的别名）",
  job: "查看、等待后台命令",
  kill_task: "结束后台命令",
  monitor: "盯住长命令的输出",
  scheduler_create: "新建定时任务",
  scheduler_list: "列出定时任务",
  scheduler_delete: "删除定时任务",
  enter_plan_mode: "进入计划模式（只读，先出方案）",
  exit_plan_mode: "提交方案，请用户批准后退出计划模式",
  ask_user_question: "向用户提问并给出选项",
  todo_write: "维护任务清单",
  update_goal: "更新会话目标的进度",
  web_fetch: "抓取网页并转成 Markdown",
  web_search: "联网搜索，拿摘要和链接",
  task: "把一块活派给子代理",
  send_message: "给子代理或父会话发消息",
  list_agents: "查看子代理的状态",
  interrupt_agent: "打断子代理当前这一轮",
  memory_search: "搜索长期记忆",
  memory_get: "读取一条长期记忆",
  skill: "读取技能说明（SKILL.md）",
  workflow: "运行工作流",
  search_tool: "在 MCP 服务里找工具",
  use_tool: "调用 MCP 服务的工具",
  lsp: "查询语言服务器（定义、引用、诊断）",
  browser_open: "打开浏览器页面",
  browser_navigate: "页面跳转到网址",
  browser_navigate_back: "页面后退",
  browser_snapshot: "读取页面结构快照",
  browser_click: "点击页面元素",
  browser_hover: "悬停到页面元素",
  browser_type: "在输入框里打字",
  browser_press_key: "按键",
  browser_select_option: "选择下拉选项",
  browser_fill_form: "一次填好多个表单字段",
  browser_wait_for: "等待文字出现或消失",
  browser_drag: "拖拽页面元素",
  browser_handle_dialog: "处理页面弹窗",
  browser_file_upload: "上传文件",
  browser_resize: "调整浏览器窗口大小",
  browser_evaluate: "在页面里执行 JavaScript",
  browser_console_messages: "读取控制台消息",
  browser_network_requests: "查看网络请求",
  browser_screenshot: "页面截图",
  browser_tabs: "管理标签页",
  browser_close: "关闭浏览器",
  cordis_inspect: "查看 Cordis 插件树",
  cordis_define: "定义一个动态插件",
  cordis_run: "运行动态插件",
  cordis_call: "调用动态插件的服务",
  cordis_stop: "停掉动态插件",
  cordis_promote: "把动态插件存成永久插件",
};

/** 工具的中文简介，没收录就用 Dock 给的。 */
export function toolSummary(name: string, fallback: string): string {
  return ZH[name] ?? fallback;
}
