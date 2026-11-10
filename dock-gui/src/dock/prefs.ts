// GUI 自己的偏好（不进 Dock 的 config.toml）：新对话默认值、启动、通知、外观、快捷键。
// 存 localStorage，一个 key 一份 JSON；组件用 usePrefs() 订阅，改了立即生效。

import { useSyncExternalStore } from "react";

export type Approval = "ask" | "auto";
export type FontSize = "sm" | "md" | "lg";
export type ChatFont = keyof typeof CHAT_FONTS;
export type CodeFont = keyof typeof CODE_FONTS;
/** 界面材质：平面 = Wonder 设计稿原样；玻璃 = 侧栏半透 + 弹层毛玻璃（styles.css「玻璃材质」）。 */
export type Material = "flat" | "glass";
/**
 * 玻璃的原生底子（只在 Tauri 主窗口）：透明 = macOS 26 Liquid Glass clear（几乎无色、模糊轻），
 * 标准 = Liquid Glass regular，磨砂 = 系统侧栏材质（偏灰、偏不透）。macOS 26 以前前两种自动退回磨砂。
 */
export type GlassStyle = "clear" | "regular" | "frosted";

/** 对话正文（回复 + 你发的消息）的字体：都是 macOS 自带的苹果字体。界面外壳不跟着换。 */
export const CHAT_FONTS = {
  sf: { label: "San Francisco + 苹方", stack: '-apple-system, BlinkMacSystemFont, "SF Pro Text", "PingFang SC", sans-serif' },
  pingfang: { label: "苹方", stack: '"PingFang SC", -apple-system, BlinkMacSystemFont, sans-serif' },
  rounded: { label: "SF Rounded + 苹方", stack: 'ui-rounded, "SF Pro Rounded", -apple-system, "PingFang SC", sans-serif' },
  newyork: { label: "New York + 宋体", stack: 'ui-serif, "New York", "Songti SC", serif' },
} as const;

/** 等宽字体：代码块、行内代码、工具卡都用它。 */
export const CODE_FONTS = {
  sfmono: { label: "SF Mono", stack: '"SF Mono", ui-monospace, Menlo, monospace' },
  menlo: { label: "Menlo", stack: "Menlo, ui-monospace, monospace" },
  monaco: { label: "Monaco", stack: "Monaco, ui-monospace, Menlo, monospace" },
} as const;

export interface Prefs {
  /** 新对话默认审批：「始终允许」= Dock 的 auto。 */
  approval: Approval;
  /** 新对话默认开思考（模型不支持推理时无效）。 */
  thinking: boolean;
  /** 启动时回到上次看的会话。 */
  restoreSession: boolean;
  /** 新对话的默认项目目录（没最近会话时用）。空 = 不指定。 */
  projectDir: string;
  notifyTurnDone: boolean;
  notifyNeedsInput: boolean;
  /** 只在窗口不在前台时通知。 */
  notifyBackgroundOnly: boolean;
  accent: string;
  material: Material;
  glassStyle: GlassStyle;
  /** 玻璃透明度 0–100：越大越透（styles.css 的 --glass-alpha = 1 − 它/100，各层白色按它缩放）。 */
  glassClarity: number;
  fontSize: FontSize;
  chatFont: ChatFont;
  codeFont: CodeFont;
  /** 代码块语法高亮配色（Shiki 主题 id，见 components/markdown/highlight.ts）。 */
  codeTheme: string;
  /** 用户消息旁显示时间。 */
  timestamps: boolean;
  /** 思考过程默认展开。 */
  expandReasoning: boolean;
  /** 工具卡默认展开。 */
  expandTools: boolean;
  /** 启动时侧栏收起（整列收起，左上角留展开按钮）。 */
  sidebarCollapsed: boolean;
  /** 桌面面板画 agent 光标。 */
  agentCursor: boolean;
  /** 接不接新的网页配对请求（Dock 不落盘，GUI 每次连上再设一次）。 */
  acceptPairing: boolean;
  /** 改过的快捷键：动作 id → 键位（见 shortcuts.ts）。没改过的不在里面。 */
  shortcuts: Record<string, string>;
  /** 左下角显示的名字（只在本机界面上用，不发给模型）。空 = 「我」。 */
  profileName: string;
  /** 头像：缩到 128px 的 data URL。空 = 名字首字。 */
  avatar: string;
}

export const DEFAULT_PREFS: Prefs = {
  approval: "ask",
  thinking: true,
  restoreSession: true,
  projectDir: "",
  notifyTurnDone: true,
  notifyNeedsInput: true,
  notifyBackgroundOnly: true,
  accent: "#0a84ff",
  material: "flat",
  glassStyle: "clear",
  glassClarity: 50,
  fontSize: "md",
  chatFont: "sf",
  codeFont: "sfmono",
  codeTheme: "github-light",
  timestamps: false,
  expandReasoning: false,
  expandTools: false,
  sidebarCollapsed: false,
  agentCursor: true,
  acceptPairing: true,
  shortcuts: {},
  profileName: "",
  avatar: "",
};

/** 界面上显示的名字。 */
export const displayName = (p: Prefs) => p.profileName.trim() || "我";

const KEY = "dock-gui.prefs";

function load(): Prefs {
  try {
    const raw = JSON.parse(localStorage.getItem(KEY) ?? "{}");
    return { ...DEFAULT_PREFS, ...(raw && typeof raw === "object" ? raw : {}) };
  } catch {
    return { ...DEFAULT_PREFS };
  }
}

let current = load();
const listeners = new Set<() => void>();

export const prefs = {
  get: () => current,
  set: <K extends keyof Prefs>(key: K, value: Prefs[K]) => {
    current = { ...current, [key]: value };
    localStorage.setItem(KEY, JSON.stringify(current));
    applyAppearance(current);
    for (const l of listeners) l();
  },
  subscribe: (l: () => void) => {
    listeners.add(l);
    return () => listeners.delete(l);
  },
};

// 别的窗口改了偏好（主窗口设置页 → 拖出来的独立窗）：这边跟着换，材质、透明度、字体才一致。
window.addEventListener("storage", (e) => {
  if (e.key !== KEY) return;
  current = load();
  applyAppearance(current);
  for (const l of listeners) l();
});

export function usePrefs(): Prefs {
  return useSyncExternalStore(prefs.subscribe, prefs.get);
}

const ZOOM: Record<FontSize, string> = { sm: "0.92", md: "1", lg: "1.08" };

/**
 * 强调色、字体走 CSS 变量（Tailwind 的 accent / font-chat / font-mono 都读它）；
 * 材质挂在 <html data-material>；字号整窗缩放（界面字号都是像素写死的）。
 */
export function applyAppearance(p: Prefs = current) {
  const root = document.documentElement;
  root.style.setProperty("--color-accent", p.accent);
  root.dataset.material = p.material === "glass" ? "glass" : "flat";
  const clarity = Math.min(100, Math.max(0, Number(p.glassClarity) || 0));
  root.style.setProperty("--glass-alpha", String(1 - clarity / 100));
  void syncNativeGlass(p.material, p.glassStyle);
  root.style.setProperty("--font-chat", (CHAT_FONTS[p.chatFont] ?? CHAT_FONTS.sf).stack);
  root.style.setProperty("--font-mono", (CODE_FONTS[p.codeFont] ?? CODE_FONTS.sfmono).stack);
  const app = document.getElementById("root");
  if (app) app.style.setProperty("zoom", ZOOM[p.fontSize]);
}

/**
 * 玻璃在 Tauri 窗口里用 macOS 原生材质（windowEffects，底下是 window-vibrancy）：
 * 系统合成、不走网页的 backdrop-filter。主窗口和拖出来的独立窗都建成透明的
 * （tauri.conf.json、lib.rs 的 detached_builder），网页只在 <html data-native-glass> 时把窗口底放透明。
 * 浏览器预览没有原生材质，退回 styles.css 的 CSS 玻璃。
 */
let nativeKey: string | null = null;
async function syncNativeGlass(material: Material, style: GlassStyle) {
  const key = material === "glass" ? `glass:${style}` : "flat";
  if (!("__TAURI_INTERNALS__" in window) || nativeKey === key) return;
  nativeKey = key;
  const { getCurrentWindow, Effect, EffectState } = await import("@tauri-apps/api/window");
  const win = getCurrentWindow();
  const root = document.documentElement;
  try {
    if (material === "glass") {
      // 系统深色时侧栏材质会变深，界面只有浅色：玻璃开着就钉住浅色。
      await win.setTheme("light");
      // 列表里带上 Sidebar：macOS 26 以前没有 Liquid Glass，Tauri 退回列表里的磨砂。
      const effect =
        style === "frosted" ? Effect.Sidebar : style === "regular" ? Effect.LiquidGlassRegular : Effect.LiquidGlassClear;
      await win.setEffects({
        effects: effect === Effect.Sidebar ? [Effect.Sidebar] : [effect, Effect.Sidebar],
        state: EffectState.FollowsWindowActiveState,
      });
      root.dataset.glassStyle = style;
      root.dataset.nativeGlass = "";
    } else {
      // 先让网页不透再撤材质：反过来会闪一下没模糊的桌面。
      delete root.dataset.nativeGlass;
      delete root.dataset.glassStyle;
      await win.clearEffects();
      await win.setTheme(null);
    }
  } catch {
    delete root.dataset.nativeGlass;
  }
}

// ---- 系统通知 ----

/** 窗口在不在前台（「只在不在前台时通知」用）。 */
function windowFocused() {
  return document.visibilityState === "visible" && document.hasFocus();
}

/**
 * 发一条系统通知。Tauri 里走 notification 插件（WKWebView 自己的 Notification 不弹）；
 * 浏览器预览里走 Web Notification。没授权就先问一次。
 */
export async function notify(kind: "done" | "input", title: string, body: string) {
  const p = current;
  if (kind === "done" && !p.notifyTurnDone) return;
  if (kind === "input" && !p.notifyNeedsInput) return;
  if (p.notifyBackgroundOnly && windowFocused()) return;
  try {
    if ("__TAURI_INTERNALS__" in window) {
      const n = await import("@tauri-apps/plugin-notification");
      let ok = await n.isPermissionGranted();
      if (!ok) ok = (await n.requestPermission()) === "granted";
      if (ok) n.sendNotification({ title, body });
      return;
    }
    if (!("Notification" in window)) return;
    if (Notification.permission === "default") await Notification.requestPermission();
    if (Notification.permission === "granted") new Notification(title, { body });
  } catch {
    // 通知失败不影响主流程。
  }
}
