// 预览：预设编辑器的各个状态（#preset-*）。store 的三个方法换成假数据，不连 Dock。

import { PresetStudio } from "../components/preset/PresetStudio";
import { PresetEditor } from "../components/PresetEditor";
import { PresetDraftCard } from "../features/preset-draft/PresetDraftCard";
import { store } from "../dock/store";
import type { AiPresetDraft, CatalogTool, PresetDetail, PresetOption } from "../model";

const tool = (name: string, summary: string, kind: CatalogTool["kind"] = "resident"): CatalogTool => ({ name, summary, kind });

const CATALOG: CatalogTool[] = [
  tool("read_file", "读取工作区里的文件内容，可带行号范围"),
  tool("list_dir", "列出目录内容"),
  tool("grep", "按正则搜索文件内容"),
  tool("glob", "按通配符找文件"),
  tool("search_replace", "精准替换现有文件里的一段内容"),
  tool("write_file", "新建或整份覆盖一个文件"),
  tool("bash", "在工作区里跑 shell 命令"),
  tool("job", "查看、等待后台命令"),
  tool("kill_task", "结束后台命令"),
  tool("monitor", "盯住长命令的输出", "deferred"),
  tool("web_fetch", "抓取网页并转成 Markdown"),
  tool("web_search", "联网搜索，拿摘要和链接"),
  tool("browser_open", "打开浏览器页面", "deferred"),
  tool("browser_snapshot", "读取页面结构快照", "deferred"),
  tool("task", "把一块活派给子代理"),
  tool("send_message", "给子代理或父会话发消息"),
  tool("todo_write", "维护任务清单"),
  tool("ask_user_question", "向用户提问并给选项"),
  tool("memory_search", "搜索长期记忆"),
  tool("skill", "读取技能说明"),
  tool("lsp", "查询语言服务器", "deferred"),
];

const researcher: PresetDetail = {
  id: "researcher",
  name: "深度研究员",
  description: "面向复杂主题研究、跨文档综合分析与深度报告撰写",
  icon: "flask-conical",
  order: 10,
  persona:
    "# 深度研究员 (Deep Researcher) 行为准则\n\n你是一个专业级深度研究员。在处理技术调研、学术分析和跨领域综合任务时，必须严格遵守：\n\n## 1. 深度分析与交叉验证\n- 查阅事实性信息时，至少检索两份独立来源进行比对；\n- 引用规范、仓库或论文时，在文末列出参考链接。\n\n## 2. 工具调用规范\n- 优先用 web_fetch 提取事实，必要时用浏览器抓交互式页面；\n- 涉及计算或数据清洗时，写临时脚本验证，不凭空估算。",
  replacePrompt: false,
  tools: ["web_fetch", "web_search", "read_file", "search_replace", "task", "lsp_symbol"],
  agents: [
    {
      id: "web-researcher",
      name: "网页调研",
      description: "负责公开网页定向检索、HTML 降噪抽取与关键事实交叉比对",
      persona: "你是一个专注于网页情报提取与降噪分析的子代理。\n1. 接收主 Agent 分派的关键词或 URL；\n2. 提取核心事实与证据引用；\n3. 以结构化报告交付。",
      tools: ["web_fetch", "web_search", "read_file", "send_message"],
      replacePrompt: false,
      listings: true,
      builtin: false,
    },
    {
      id: "data-analyst",
      name: "",
      description: "负责长文本结构化分析、表格抽取与临时数据清洗脚本运行",
      persona: "你负责数据分析。",
      tools: null,
      replacePrompt: false,
      listings: false,
      builtin: false,
    },
  ],
  origin: "user",
  builtin: false,
  available: true,
  error: null,
  path: "/Users/me/.dock/presets/researcher/agent.yml",
};

const code: PresetDetail = {
  ...researcher,
  id: "code",
  name: "编码",
  description: "工作区读写跑，加上计划、提问、后台任务、网页、LSP、委派",
  icon: null,
  order: 1,
  tools: CATALOG.map((t) => t.name),
  agents: [
    { ...researcher.agents[1], id: "explore", description: "只读探索代码库", builtin: true },
    { ...researcher.agents[1], id: "plan", description: "只出方案不改代码", builtin: true },
  ],
  origin: "shipped",
  builtin: true,
  path: null,
};

const broken: PresetDetail = {
  ...researcher,
  id: "broken-agent",
  name: "broken-agent",
  available: false,
  error: "不是合法 YAML: did not find expected ',' or ']' at line 14 column 33, while parsing a flow sequence at line 14 column 8",
  path: "/Users/me/.dock/presets/broken-agent/agent.yml",
};

const FIXTURES: Record<string, PresetDetail> = { researcher, code, "broken-agent": broken };

const AI_DRAFT: AiPresetDraft = {
  name: "技术周报研究员",
  description: "每周搜集科技动态并写成 Markdown 周报",
  icon: "globe",
  persona:
    "# 角色\n你是技术周报编辑，负责追踪开源生态与工程动态。\n\n# 做事方式\n1. 先用 web_search 找本周热点，再用 web_fetch 读原文。\n2. 按「架构热点 / 开源速递 / AI 实践」三段整理。\n\n# 边界\n- 不转述没读过的内容，每条都附出处。",
  replacePrompt: false,
  tools: ["web_search", "web_fetch", "write_file", "todo_write", "task"],
  toolReasons: [
    { name: "web_search", reason: "检索本周科技新闻与 GitHub 热点" },
    { name: "web_fetch", reason: "读原文，写摘要要有依据" },
    { name: "write_file", reason: "把周报存成 Markdown 文件" },
    { name: "todo_write", reason: "拆分选题并跟踪进度" },
    { name: "task", reason: "把长文精读派给子代理" },
  ],
  agents: [
    {
      id: "web-researcher",
      name: "网页调研",
      description: "并行精读长文并提炼三句话要点",
      persona: "你只负责读网页并提炼要点，附原文链接。",
      tools: ["web_fetch"],
      replacePrompt: false,
      listings: false,
    },
  ],
};

const PRESETS: PresetOption[] = [
  { id: "code", label: "编码", description: "工作区读写跑", origin: "shipped", builtin: true, available: true },
  { id: "minimal", label: "极简", description: "仅工作区工具", origin: "shipped", builtin: true, available: true },
];

const never = () => new Promise<never>(() => {});
store.getPreset = async (id: string) => structuredClone(FIXTURES[id]);
store.draftPreset = async () =>
  location.hash === "#ai-generating"
    ? never()
    : location.hash === "#ai-failed"
      ? Promise.reject(new Error("模型调用失败：HTTP 401 Unauthorized（API Key 无效或已过期）"))
      : structuredClone(AI_DRAFT);
store.rewritePersona = async () =>
  "## 角色定位\n你是一位资深技术情报分析师与科技周报主编，负责追踪前沿开源生态、工程架构与 AI 进展。\n\n## 执行规范\n1. 信息检索：用 web_search 找本周热点，再用 web_fetch 读原文。\n2. 结构编排：按「架构热点」「开源速递」「AI 实践」三段排版。\n\n## 约束原则\n拒绝营销软文与非权威信源；每条动态都附摘要和出处链接。";
store.suggestTools = async () => [
  { name: "web_search", reason: "提示词要求搜集本周科技新闻" },
  { name: "bash", reason: "需要跑脚本整理数据" },
  { name: "read_file", reason: "读本地周报模板" },
];
store.askPreset = async () =>
  "会改动工作区的只有这几个：\n\n```text\nsearch_replace\nwrite_file\nbash\n```\n\n`bash` 能跑任何命令，也能改文件。审查员只读的话，去掉这三个，留 `read_file`、`grep`、`glob` 就够了。";
store.toolCatalog = async () => CATALOG;
store.updatePreset = async () => {};

/** 各状态：打开哪个预设，挂上后点哪里。 */
const STATES: Record<string, { id: string; steps?: (() => void)[] }> = {
  "#preset-basic": { id: "researcher" },
  "#preset-prompt": { id: "researcher", steps: [() => click("角色提示词")] },
  "#preset-tools": { id: "researcher", steps: [() => click("工具配置")] },
  "#preset-tools-empty": { id: "researcher", steps: [() => click("工具配置"), () => type("搜索工具名称或描述...", "docker-compose")] },
  "#preset-agents": { id: "researcher", steps: [() => click("子代理名册")] },
  "#preset-agent": { id: "researcher", steps: [() => click("子代理名册"), () => click("编辑")] },
  "#preset-agent-tools": {
    id: "researcher",
    steps: [() => click("子代理名册"), () => click("编辑"), () => click("自选勾选")],
  },
  "#preset-builtin": { id: "code" },
  "#preset-unsaved": {
    id: "researcher",
    steps: [() => type("这个 Agent 擅长什么", "改过的描述"), () => click("取消")],
  },
  "#preset-errors": { id: "researcher", steps: [() => type("请输入预设名称（不能为空）", ""), () => click("保存修改")] },
  "#preset-broken": { id: "broken-agent" },
  "#preset-ask-empty": { id: "researcher", steps: [() => click("角色提示词"), () => click("问 AI")] },
  "#preset-ask": {
    id: "researcher",
    steps: [() => click("工具配置"), () => click("问 AI"), () => click("这些工具里哪些会改文件或跑命令？")],
  },
  "#ai-create": { id: "" },
  "#ai-prompt": { id: "", steps: [() => click("不想从空白开始"), () => click("技术周报研究员")] },
  "#ai-generating": {
    id: "",
    steps: [() => click("不想从空白开始"), () => click("技术周报研究员"), () => click("起草并打开编辑器")],
  },
  "#ai-failed": {
    id: "",
    steps: [() => click("不想从空白开始"), () => click("技术周报研究员"), () => click("起草并打开编辑器")],
  },
  "#preset-draft-card": { id: "" },
  "#ai-draft": { id: "", steps: [() => click("工具配置")] },
  "#ai-diff": { id: "researcher", steps: [() => click("角色提示词"), () => click("AI 润色")] },
  "#ai-recommend": { id: "researcher", steps: [() => click("工具配置"), () => click("按描述推荐")] },
};

function click(text: string) {
  [...document.querySelectorAll<HTMLButtonElement>("button")].find((b) => b.textContent?.trim().startsWith(text))?.click();
}

function type(placeholder: string, value: string) {
  const el = document.querySelector<HTMLInputElement>(`input[placeholder="${placeholder}"]`);
  if (!el) return;
  Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(el, value);
  el.dispatchEvent(new Event("input", { bubbles: true }));
}

export function isPresetPreview() {
  return location.hash in STATES;
}

export function PresetStudioPreview() {
  const state = STATES[location.hash];
  if (location.hash === "#preset-draft-card") {
    const args = {
      name: "代码审查员",
      description: "只读审代码，按严重程度列出问题并给修改建议",
      persona: "你是代码审查员。只读，不改文件。".repeat(20),
      tools: [{ name: "read_file" }, { name: "grep" }, { name: "glob" }, { name: "list_dir" }, { name: "bash" }],
      subagents: [
        { id: "security", name: "安全复查", persona: "查安全问题" },
        { id: "coverage", name: "测试覆盖", persona: "看测试" },
      ],
    };
    return (
      <div className="flex w-[900px] flex-col gap-4 bg-white p-4">
        <PresetDraftCard origin="preview:pending" args={args} running={false} />
        <PresetDraftCard origin="preview:running" args={args} running />
      </div>
    );
  }
  if (location.hash.startsWith("#ai-") && location.hash !== "#ai-draft" && !state.id) {
    return (
      <div className="relative h-[900px] w-[1440px] bg-window">
        <PresetEditor presets={PRESETS} defaultBase="code" onClose={() => {}} onCreated={() => {}} onDrafted={() => {}} onOpenModels={() => {}} />
      </div>
    );
  }
  if (location.hash === "#ai-draft") {
    return (
      <div className="h-[900px] w-[1440px]">
        <PresetStudio
          presetId={null}
          aiDraft={{ draft: structuredClone(AI_DRAFT), description: "每周搜集科技新闻和 GitHub 热点，写 Markdown 周报" }}
          onClose={() => {}}
          onOpenMcp={() => {}}
        />
      </div>
    );
  }
  return (
    <div className="h-[900px] w-[1440px]">
      <PresetStudio presetId={state.id} onClose={() => {}} onOpenMcp={() => {}} />
    </div>
  );
}

// 挂上后按顺序点（模块里只排一次；StrictMode 会渲染两遍）。
STATES[location.hash]?.steps?.forEach((step, i) => setTimeout(step, 400 + i * 250));
