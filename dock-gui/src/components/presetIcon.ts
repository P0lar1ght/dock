// 预设图标：内置四个用设计稿的语义图标；自定义预设用它在 agent.yml 里记的 `icon`
// （lucide 名，由「新建自定义预设」从图标库选），没记的是圆圈。
// store 拉到 preset/list 后调 `registerPresetIcons`，之后 `presetIcon(id)` 就认得。

import {
  Bookmark,
  Bot,
  Box,
  Braces,
  Circle,
  Code,
  Cpu,
  Database,
  Eye,
  FlaskConical,
  GitBranch,
  Globe,
  Layers,
  MessageSquare,
  Pencil,
  Puzzle,
  Radar,
  Rocket,
  Shield,
  Sparkles,
  Star,
  SquareTerminal,
  Wrench,
  Zap,
  type LucideIcon,
} from "lucide-react";

export type IconCategory = "work" | "status" | "meaning";

/** 图标库（设计 bos：Custom Preset Icon Library）：名字 → 图标 + 分类。顺序即摆放顺序。 */
export const ICON_LIBRARY: { name: string; icon: LucideIcon; category: IconCategory }[] = [
  { name: "rocket", icon: Rocket, category: "meaning" },
  { name: "code", icon: Code, category: "work" },
  { name: "circle", icon: Circle, category: "status" },
  { name: "sparkles", icon: Sparkles, category: "meaning" },
  { name: "shield", icon: Shield, category: "meaning" },
  { name: "bot", icon: Bot, category: "meaning" },
  { name: "square-terminal", icon: SquareTerminal, category: "work" },
  { name: "braces", icon: Braces, category: "work" },
  { name: "cpu", icon: Cpu, category: "work" },
  { name: "flask-conical", icon: FlaskConical, category: "meaning" },
  { name: "puzzle", icon: Puzzle, category: "meaning" },
  { name: "eye", icon: Eye, category: "status" },
  { name: "radar", icon: Radar, category: "status" },
  { name: "wrench", icon: Wrench, category: "work" },
  { name: "zap", icon: Zap, category: "status" },
  { name: "layers", icon: Layers, category: "work" },
  { name: "box", icon: Box, category: "work" },
  { name: "git-branch", icon: GitBranch, category: "work" },
  { name: "database", icon: Database, category: "work" },
  { name: "globe", icon: Globe, category: "work" },
  { name: "message-square", icon: MessageSquare, category: "meaning" },
  { name: "pencil", icon: Pencil, category: "work" },
  { name: "bookmark", icon: Bookmark, category: "status" },
  { name: "star", icon: Star, category: "status" },
];

const BY_NAME = new Map(ICON_LIBRARY.map((i) => [i.name, i.icon]));
const BUILTIN: Record<string, LucideIcon> = { code: Code, cordis: Sparkles, warden: Shield };
let custom: Record<string, LucideIcon> = {};

/** 记下各预设在 agent.yml 里选的图标（认不得的名字忽略）。 */
export function registerPresetIcons(presets: { id: string; icon?: string | null }[]) {
  custom = {};
  for (const p of presets) {
    const icon = p.icon && BY_NAME.get(p.icon);
    if (icon) custom[p.id] = icon;
  }
}

export function presetIcon(id: string | null | undefined): LucideIcon {
  return (id && (custom[id] ?? BUILTIN[id])) || Circle;
}

/** 图标名 → 显示名：`flask-conical` → `Flask Conical`。 */
export function iconLabel(name: string): string {
  return name
    .split("-")
    .map((w) => w[0].toUpperCase() + w.slice(1))
    .join(" ");
}
