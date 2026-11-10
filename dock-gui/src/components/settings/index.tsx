// 设置页：占满整个窗口（左侧分组、右侧内容）。设计：wonder-ui/settings（第五轮，ser 左栏）。
// 改 Dock 的（模型、MCP、记忆、浏览器、CUA、插件、配对）经 dock.1 的设置方法写
// ~/.dock/config.toml；GUI 自己的（常规、外观、快捷键）存 localStorage（dock/prefs.ts）。

import { useEffect, useState } from "react";
import {
  ArrowLeft,
  Cpu,
  Globe,
  Info,
  Keyboard,
  LayoutGrid,
  Monitor,
  Palette,
  Plug,
  Puzzle,
  Search,
  Server,
  Settings as Gear,
  Sparkles,
  User,
  type LucideIcon,
} from "lucide-react";
import { store } from "../../dock/store";
import { cuaStatus, type CuaStatus } from "../../dock/settingsApi";
import { TrafficLights } from "../chrome";
import { clearSaveState } from "./ui";
import { GeneralPane } from "./General";
import { AppearancePane } from "./Appearance";
import { ShortcutsPane } from "./Shortcuts";
import { ModelsPane } from "./Models";
import { PresetsPane } from "./Presets";
import { MemoryPane } from "./Memory";
import { McpPane } from "./Mcp";
import { PluginsPane } from "./Plugins";
import { CuaPane } from "./Cua";
import { BrowserPane } from "./Browser";
import { GatewayPane } from "./Gateway";
import { AboutPane } from "./About";
import { PersonalizationPane } from "./Personalization";
import { Avatar } from "../Avatar";
import { displayName, usePrefs } from "../../dock/prefs";

export type SettingsSection =
  | "general"
  | "appearance"
  | "shortcuts"
  | "personalization"
  | "models"
  | "presets"
  | "memory"
  | "mcp"
  | "plugins"
  | "cua"
  | "browser"
  | "gateway"
  | "about";

interface NavItem {
  id: SettingsSection;
  label: string;
  icon: LucideIcon;
  /** 页里的设置项：搜索能搜到它们，命中时右侧显示命中的那一项。 */
  keys: string[];
}

const GROUPS: { label: string; items: NavItem[] }[] = [
  {
    label: "通用",
    items: [
      {
        id: "general",
        label: "常规",
        icon: Gear,
        keys: ["默认预设", "默认审批", "默认思考", "恢复上次会话", "默认项目目录", "通知", "数据目录", "会话存档"],
      },
      { id: "appearance", label: "外观", icon: Palette, keys: ["主题", "强调色", "材质", "玻璃", "透明度", "界面字号", "时间戳", "思考过程", "工具卡", "侧栏"] },
      { id: "shortcuts", label: "键盘快捷键", icon: Keyboard, keys: ["快捷键", "切换侧栏", "终端", "快速打开"] },
      { id: "personalization", label: "个性化", icon: User, keys: ["头像", "名字", "个人资料", "自定义指令", "关于你", "AGENTS.md", "USER.md", "主动记忆"] },
    ],
  },
  {
    label: "模型与智能体",
    items: [
      { id: "models", label: "模型与密钥", icon: Cpu, keys: ["供应商", "API Key", "models.dev", "默认模型", "接口地址", "密钥", "协议", "上下文窗口", "推理", "单价", "测试连接"] },
      { id: "presets", label: "预设管理", icon: LayoutGrid, keys: ["预设", "人设", "子代理"] },
      { id: "memory", label: "记忆", icon: Sparkles, keys: ["跨会话记忆", "自动整理", "定期归纳", "向量检索", "嵌入模型"] },
    ],
  },
  {
    label: "工具与扩展",
    items: [
      { id: "mcp", label: "MCP 服务", icon: Server, keys: ["MCP", "添加服务", "OAuth", "工具开关"] },
      { id: "plugins", label: "插件与技能", icon: Puzzle, keys: ["插件", "技能", "插件密钥", "secrets"] },
      { id: "cua", label: "电脑操控 (CUA)", icon: Monitor, keys: ["cua-driver", "辅助功能", "屏幕录制", "agent 光标"] },
      { id: "browser", label: "浏览器与联网", icon: Globe, keys: ["有头", "网页抓取", "超时", "域名", "转发代理", "localhost"] },
    ],
  },
  {
    label: "系统",
    items: [
      { id: "gateway", label: "网关与设备", icon: Plug, keys: ["连接", "远程 Dock", "配对", "网页连接", "远程设备", "令牌", "重启 Dock"] },
      { id: "about", label: "关于 Dock", icon: Info, keys: ["版本", "Dock 路径", "协议"] },
    ],
  },
];

const ALL_ITEMS = GROUPS.flatMap((g) => g.items);

/** CUA 状态 → 左栏小圆点颜色（绿已连接 / 橙缺授权 / 红未连上 / 灰其余）。 */
function cuaDot(s: CuaStatus | null): string | null {
  if (!s) return null;
  switch (s.state) {
    case "connected":
      return "bg-[#34c759]";
    case "needs_permission":
    case "needs_auth":
      return "bg-[#ff9f0a]";
    case "unreachable":
      return "bg-[#ff3b30]";
    default:
      return "bg-ink-5";
  }
}

export function Settings({
  section,
  onSection,
  onClose,
}: {
  section: SettingsSection;
  onSection: (s: SettingsSection) => void;
  onClose: () => void;
}) {
  const userName = displayName(usePrefs());
  const [filter, setFilter] = useState("");
  const [mcpCount, setMcpCount] = useState<number | null>(null);
  const [cua, setCua] = useState<CuaStatus | null>(null);
  const version = store.serverInfo().version;

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && !e.defaultPrevented && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  // 左栏的 MCP 数和 CUA 圆点：开设置页时读一次，之后各自的页面会回填。
  useEffect(() => {
    store.mcpList().then((s) => setMcpCount(s.length), () => {});
    cuaStatus().then(setCua, () => {});
  }, []);

  // 换页时清掉上一页的「已保存 / 写入失败」。
  useEffect(() => clearSaveState(), [section]);

  const q = filter.trim().toLowerCase();
  const hit = (item: NavItem) =>
    !q
      ? { show: true, key: null }
      : item.label.toLowerCase().includes(q)
        ? { show: true, key: null }
        : { show: item.keys.some((k) => k.toLowerCase().includes(q)), key: item.keys.find((k) => k.toLowerCase().includes(q)) ?? null };

  const pane = (() => {
    switch (section) {
      case "general":
        return <GeneralPane />;
      case "appearance":
        return <AppearancePane />;
      case "shortcuts":
        return <ShortcutsPane />;
      case "personalization":
        return <PersonalizationPane onOpenMemory={() => onSection("memory")} />;
      case "models":
        return <ModelsPane />;
      case "presets":
        return <PresetsPane />;
      case "memory":
        return <MemoryPane />;
      case "mcp":
        return <McpPane onCount={setMcpCount} />;
      case "plugins":
        return <PluginsPane />;
      case "cua":
        return <CuaPane onStatus={setCua} onOpenGeneral={() => onSection("general")} />;
      case "browser":
        return <BrowserPane />;
      case "gateway":
        return <GatewayPane />;
      case "about":
        return <AboutPane version={version} />;
    }
  })();

  const dot = cuaDot(cua);
  return (
    <div className="glass-window enter-fade flex h-full w-full overflow-hidden bg-white">
      <aside className="glass-sidebar flex w-[256px] shrink-0 flex-col gap-3 border-r border-line bg-sidebar p-3">
        <div className="flex shrink-0 flex-col gap-3" data-tauri-drag-region>
          <div className="flex h-6 items-center px-1" data-tauri-drag-region>
            <TrafficLights />
          </div>
          <button
            type="button"
            onClick={onClose}
            title="返回应用（Esc）"
            className="motion-colors flex h-8 items-center gap-2 rounded-lg px-2 text-[13px] font-medium text-ink hover:bg-hover"
          >
            <ArrowLeft size={14} />
            返回应用
          </button>
          <label className="glass-field flex h-8 items-center gap-2 rounded-lg bg-[#eaebee] px-2.5">
            <Search size={14} className="shrink-0 text-ink-4" />
            <input
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
              placeholder="搜索设置…"
              className="min-w-0 flex-1 bg-transparent text-[12.5px] text-ink outline-none placeholder:text-ink-4"
            />
          </label>
        </div>
        <nav className="thin-scroll flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto">
          {GROUPS.map((g) => {
            const items = g.items.map((i) => ({ item: i, ...hit(i) })).filter((i) => i.show);
            if (items.length === 0) return null;
            return (
              <div key={g.label} className="flex flex-col gap-0.5">
                <div className="px-2.5 py-1 text-[11px] font-semibold tracking-[0.4px] text-ink-4">{g.label}</div>
                {items.map(({ item: { id, label, icon: Icon }, key }) => {
                  const on = id === section;
                  return (
                    <button
                      key={id}
                      type="button"
                      onClick={() => onSection(id)}
                      className={`motion-colors flex h-8 items-center gap-2.5 rounded-lg px-2.5 text-left text-[13px] ${
                        on ? "glass-state bg-line-strong font-semibold text-accent shadow-sm" : "font-medium text-ink-2 hover:bg-hover"
                      }`}
                    >
                      <Icon size={16} className={`shrink-0 ${on ? "text-accent" : "text-ink-3"}`} />
                      <span className="min-w-0 flex-1 truncate">{label}</span>
                      {key && <span className="shrink-0 text-[11px] font-normal text-ink-4">{key}</span>}
                      {!key && id === "mcp" && mcpCount !== null && (
                        <span className="min-w-[18px] rounded-full bg-[#0a84ff1a] px-1.5 text-center text-[11px] font-semibold leading-[18px] text-accent">
                          {mcpCount}
                        </span>
                      )}
                      {!key && id === "cua" && dot && (
                        <span title={cua?.state} className={`size-1.5 shrink-0 rounded-full ${dot}`} />
                      )}
                    </button>
                  );
                })}
              </div>
            );
          })}
          {q && ALL_ITEMS.every((i) => !hit(i).show) && (
            <div className="px-2.5 text-[12px] text-ink-4">没有匹配「{filter.trim()}」的设置</div>
          )}
        </nav>
        <div className="mt-auto flex h-10 shrink-0 items-center justify-between border-t border-line px-1 pt-2">
          <button
            type="button"
            title="个人资料与个性化"
            onClick={() => onSection("personalization")}
            className="motion-colors -ml-1 flex min-w-0 items-center gap-2 rounded-md px-1 py-0.5 hover:bg-hover"
          >
            <Avatar size={24} />
            <span className="truncate text-xs font-medium text-ink">{userName}</span>
          </button>
          {version && <span className="font-mono text-[11px] text-ink-4">v{version}</span>}
        </div>
      </aside>

      <main key={section} className="glass-pane enter-fade thin-scroll flex min-w-0 flex-1 flex-col overflow-y-auto bg-white px-10 py-8">
        {pane}
      </main>
    </div>
  );
}
