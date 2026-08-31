import type { SlashCatalogCommand } from '../protocol/responses.js';

export type SlashCatalogEntry = SlashCatalogCommand;

export interface SlashCommandSuggestion {
  name: string;
  title: string;
  description: string;
  available: boolean;
  unavailableReason?: string;
}

/** Host-only fallback so `/screenshot` still completes before `slash/list` returns. */
export const SCREENSHOT_CATALOG: readonly SlashCatalogEntry[] = [
  {
    name: 'screenshot',
    display: '/screenshot',
    description: '截取当前 viewport；参数是发给模型的问题。',
    takesArgs: true,
    surface: 'embed',
    kind: 'capture',
    capture: 'viewport'
  },
  {
    name: 'screenshot --region',
    display: '/screenshot --region',
    description: '拖动选择一个区域，只把该区域发给本轮模型。',
    takesArgs: true,
    surface: 'embed',
    kind: 'capture',
    capture: 'region'
  },
  {
    name: 'screenshot --reuse',
    display: '/screenshot --reuse',
    description: '不重新截图，复用当前页面最近一次图片。',
    takesArgs: true,
    surface: 'embed',
    kind: 'capture',
    capture: 'reuse'
  },
  {
    name: 'screenshot --screen',
    display: '/screenshot --screen',
    description: '打开浏览器共享选择器并捕获一帧。',
    takesArgs: true,
    surface: 'embed',
    kind: 'capture',
    capture: 'screen'
  },
  {
    name: 'screenshot --full-page',
    display: '/screenshot --full-page',
    description: '分段截取当前页面的完整纵向内容。',
    takesArgs: true,
    surface: 'embed',
    kind: 'capture',
    capture: 'full-page'
  }
];

export function mergeSlashCatalog(remote: readonly SlashCatalogEntry[] | undefined) {
  const list = [...(remote || [])];
  const hasScreenshot = list.some((entry) =>
    entry.name === 'screenshot' || entry.display === '/screenshot'
  );
  if (!hasScreenshot) list.push(...SCREENSHOT_CATALOG);
  return list;
}

export function slashCommandSuggestions(
  draft: string,
  catalog: readonly SlashCatalogEntry[],
  options: {
    activeTurn: boolean;
    imageSupported: boolean;
    reusableImages?: boolean;
    screenCaptureSupported?: boolean;
    planSupported?: boolean;
    goalSupported?: boolean;
    goalStatus?: 'none' | 'active' | 'paused' | 'blocked' | 'completed';
  }
): SlashCommandSuggestion[] {
  const input = String(draft || '').trimStart();
  if (!input.startsWith('/') || input.startsWith('//')) return [];
  const query = input.toLowerCase();
  const typedSpace = /\s/u.test(input);
  return catalog.flatMap((entry) => {
    const labels = commandLabels(entry);
    const match = labels.find((label) => label.toLowerCase().startsWith(query));
    if (!match) return [];
    const isSubcommand = entry.name.includes(' ') || labels[0]?.includes(' ') === true;
    if (!typedSpace && isSubcommand) return [];
    if (typedSpace && !isSubcommand) return [];
    const unavailableReason = unavailable(entry, options);
    return [{
      name: match,
      title: match,
      description: entry.description,
      available: !unavailableReason,
      ...(unavailableReason ? { unavailableReason } : {})
    }];
  });
}

function commandLabels(entry: SlashCatalogEntry) {
  const display = entry.display || `/${entry.name}`;
  const aliases = (entry.aliases || []).map((alias) => `/${alias}`);
  return [display, ...aliases];
}

function unavailable(
  entry: SlashCatalogEntry,
  options: {
    activeTurn: boolean;
    imageSupported: boolean;
    reusableImages?: boolean;
    screenCaptureSupported?: boolean;
    planSupported?: boolean;
    goalSupported?: boolean;
    goalStatus?: 'none' | 'active' | 'paused' | 'blocked' | 'completed';
  }
) {
  if (entry.surface === 'terminal') return '请在 Dock 终端使用该命令';
  if (entry.kind === 'capture' || entry.surface === 'embed') {
    if (!options.imageSupported) return '当前模型未声明图片输入能力';
    if (entry.capture === 'reuse' && !options.reusableImages) {
      return '当前页面 Session 暂无可复用的图片 Turn';
    }
    if (entry.capture === 'screen' && !options.screenCaptureSupported) {
      return '当前浏览器或 WebView 不支持屏幕共享截图';
    }
    return undefined;
  }
  const goalAction = goalActionFor(entry.name);
  if (goalAction) return goalUnavailable(goalAction, options);
  if (entry.name === 'plan' && options.activeTurn) {
    return '计划模式请在空闲 Thread 中启动带说明的 /plan';
  }
  return undefined;
}

function goalActionFor(name: string) {
  if (name === 'goal') return 'start' as const;
  if (name === 'goal pause') return 'pause' as const;
  if (name === 'goal resume') return 'resume' as const;
  if (name === 'goal clear' || name === 'goal edit') return name.slice('goal '.length) as 'clear' | 'edit';
  return undefined;
}

function goalUnavailable(
  action: 'start' | 'edit' | 'pause' | 'resume' | 'clear',
  options: {
    activeTurn: boolean;
    goalSupported?: boolean;
    goalStatus?: 'none' | 'active' | 'paused' | 'blocked' | 'completed';
  }
) {
  if (!options.goalSupported) return '当前会话未挂载目标服务';
  const status = options.goalStatus || 'none';
  if (action === 'start') return undefined;
  if (status === 'none') return '当前 Thread 尚未设置 Goal';
  if (action === 'pause' && status !== 'active') return '只有 active Goal 可以暂停';
  if (action === 'resume' && status !== 'paused' && status !== 'blocked') {
    return '只有 paused 或 blocked Goal 可以恢复';
  }
  return undefined;
}
