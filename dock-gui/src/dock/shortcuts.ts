// 全局快捷键：一份表，App 的按键处理和设置页「键盘快捷键」共用。
// 键位写成 "Meta+Shift+D" 这种：修饰键按 Control / Alt / Shift / Meta 的顺序，最后是主键。

import { prefs } from "./prefs";

export interface ShortcutDef {
  id: string;
  label: string;
  group: "窗口" | "面板" | "输入框";
  default: string;
  /** 输入框里的键只展示、不让改（由输入框自己处理）。 */
  fixed?: boolean;
}

export const SHORTCUTS: ShortcutDef[] = [
  { id: "toggleSidebar", label: "切换侧栏", group: "窗口", default: "Meta+B" },
  { id: "openSettings", label: "打开设置", group: "窗口", default: "Meta+," },
  { id: "toggleTerminal", label: "切换终端", group: "窗口", default: "Control+`" },
  { id: "toggleBrowser", label: "浏览器", group: "面板", default: "Meta+T" },
  { id: "toggleDesktop", label: "桌面", group: "面板", default: "Meta+Shift+D" },
  { id: "toggleCanvas", label: "画布", group: "面板", default: "Meta+Shift+C" },
  { id: "toggleSideChat", label: "侧边聊天", group: "面板", default: "Alt+Meta+S" },
  { id: "quickOpen", label: "快速打开文件", group: "面板", default: "Meta+P" },
  { id: "send", label: "发送", group: "输入框", default: "Enter", fixed: true },
  { id: "newline", label: "换行", group: "输入框", default: "Shift+Enter", fixed: true },
  { id: "stopAndSend", label: "生成中停止并发送", group: "输入框", default: "Meta+Enter", fixed: true },
  { id: "dismiss", label: "关闭浮层", group: "输入框", default: "Escape", fixed: true },
];

export function bindingOf(id: string): string {
  const def = SHORTCUTS.find((s) => s.id === id);
  return prefs.get().shortcuts[id] ?? def?.default ?? "";
}

/** 一次按键 → 键位字符串；只按了修饰键回 null。 */
export function comboOf(e: KeyboardEvent): string | null {
  if (["Control", "Alt", "Shift", "Meta"].includes(e.key)) return null;
  const mods = [e.ctrlKey && "Control", e.altKey && "Alt", e.shiftKey && "Shift", e.metaKey && "Meta"].filter(
    Boolean,
  ) as string[];
  // 字母统一大写；Shift 下 e.key 已经变了（如 Shift+, → <），用 code 取回原键。
  let key = e.key;
  if (/^Key[A-Z]$/.test(e.code)) key = e.code.slice(3);
  else if (/^Digit\d$/.test(e.code)) key = e.code.slice(5);
  else if (e.code === "Comma") key = ",";
  else if (e.code === "Period") key = ".";
  else if (e.code === "Backquote") key = "`";
  else if (e.code === "Slash") key = "/";
  else if (key.length === 1) key = key.toUpperCase();
  return [...mods, key].join("+");
}

/** 这次按键是不是这个动作。 */
export function matches(e: KeyboardEvent, id: string): boolean {
  return comboOf(e) === bindingOf(id);
}

/** 已被别的动作占用的键位（录制时提示冲突）。 */
export function conflictOf(id: string, combo: string): ShortcutDef | null {
  return SHORTCUTS.find((s) => s.id !== id && bindingOf(s.id) === combo) ?? null;
}

const SYMBOL: Record<string, string> = {
  Meta: "⌘",
  Shift: "⇧",
  Alt: "⌥",
  Control: "⌃",
  Enter: "Enter",
  Escape: "Esc",
};

/** 键帽：每个键一个。 */
export function keycaps(combo: string): string[] {
  return combo ? combo.split("+").map((k) => SYMBOL[k] ?? k) : [];
}

export function setBinding(id: string, combo: string | null) {
  const next = { ...prefs.get().shortcuts };
  const def = SHORTCUTS.find((s) => s.id === id);
  if (combo === null || combo === def?.default) delete next[id];
  else next[id] = combo;
  prefs.set("shortcuts", next);
}
