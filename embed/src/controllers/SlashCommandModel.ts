export interface SlashCommandSuggestion {
  name: string;
  title: string;
  description: string;
  available: boolean;
  unavailableReason?: string;
}

const COMMANDS = [{
  name: '/plan',
  title: '先规划再自动实施',
  description: '参数是本次需求；只读调查，可向你提问，计划确定后用新 Turn 实施。',
  kind: 'plan'
}, {
  name: '/goal',
  title: '启动持续目标',
  description: '参数是可验证目标；原子保存并启动首个 Turn，之后有界自动推进。',
  kind: 'goal',
  goalAction: 'start'
}, {
  name: '/goal edit',
  title: '编辑当前目标',
  description: '参数是新目标；生成新 revision，旧 revision 的迟到进度会被拒绝。',
  kind: 'goal',
  goalAction: 'edit'
}, {
  name: '/goal pause',
  title: '暂停自动推进',
  description: '停止后续 continuation；不取消已经进入终态的历史 Turn。',
  kind: 'goal',
  goalAction: 'pause'
}, {
  name: '/goal resume',
  title: '恢复当前目标',
  description: '从同一 revision 启动一个新的 Turn，不复用已终结的 Run。',
  kind: 'goal',
  goalAction: 'resume'
}, {
  name: '/goal clear',
  title: '清除当前目标',
  description: '移除后续 Turn 的 Goal；不会改写历史 Transcript。',
  kind: 'goal',
  goalAction: 'clear'
}, {
  name: '/screenshot',
  title: '分析当前界面',
  description: '截取当前 viewport；输入 -- 可查看区域选择和图片复用参数。',
  kind: 'image'
}, {
  name: '/screenshot --region',
  title: '选择页面区域',
  description: '拖动选择一个区域，只把该区域发送给本轮模型。',
  kind: 'image'
}, {
  name: '/screenshot --reuse',
  title: '复用最近图片',
  description: '不重新截图，复用当前页面 Session 最近一次图片 Turn 的完整有序图片集。',
  requiresReusableImages: true,
  kind: 'image'
}, {
  name: '/screenshot --screen',
  title: '共享画面单帧',
  description: '打开浏览器共享选择器并捕获一帧；不会应用页面 DOM 遮罩规则。',
  requiresScreenCapture: true,
  kind: 'image'
}, {
  name: '/screenshot --full-page',
  title: '截取完整页面',
  description: '分段截取当前页面的完整纵向内容；页面会短暂滚动并自动恢复。',
  kind: 'image'
}] as const;

export function slashCommandSuggestions(
  draft: string,
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
  const optionQuery = /^\/screenshot\s+--[a-z-]*$/iu.test(input);
  const goalQuery = /^\/goal(?:\s+[a-z]*)?$/iu.test(input);
  if (/\s/u.test(input) && !optionQuery && !goalQuery) return [];
  const query = input.toLowerCase();
  return COMMANDS
    .filter((command) => command.name.startsWith(query))
    .filter((command) => optionQuery
      ? command.name.includes(' --')
      : goalQuery && /\s/u.test(input)
        ? command.name.startsWith('/goal ')
        : !command.name.includes(' --') && !command.name.startsWith('/goal '))
    .map((command) => {
      const requiresReusableImages = 'requiresReusableImages' in command
        && command.requiresReusableImages;
      const requiresScreenCapture = 'requiresScreenCapture' in command
        && command.requiresScreenCapture;
      const unavailableReason = command.kind === 'plan'
        ? !options.planSupported
          ? '当前 Runtime 未声明 Planning Turn 能力'
          : options.activeTurn ? 'Planning Turn 只能在空闲 Thread 中启动' : undefined
        : command.kind === 'goal'
          ? goalUnavailable(command.goalAction, options)
        : !options.imageSupported
          ? '当前模型未声明图片输入能力'
          : requiresReusableImages && !options.reusableImages
            ? '当前页面 Session 暂无可复用的图片 Turn'
            : requiresScreenCapture && !options.screenCaptureSupported
              ? '当前浏览器或 WebView 不支持屏幕共享截图'
              : undefined;
      return {
        name: command.name,
        title: command.title,
        description: command.description,
        available: !unavailableReason,
        ...(unavailableReason ? { unavailableReason } : {})
      };
    });
}

function goalUnavailable(
  action: 'start' | 'edit' | 'pause' | 'resume' | 'clear',
  options: {
    activeTurn: boolean;
    goalSupported?: boolean;
    goalStatus?: 'none' | 'active' | 'paused' | 'blocked' | 'completed';
  }
) {
  if (!options.goalSupported) return '当前 Runtime 未声明 Thread Goal 能力';
  const status = options.goalStatus || 'none';
  if (action === 'start') {
    return options.activeTurn ? '新 Goal 只能在空闲 Thread 中启动' : undefined;
  }
  if (status === 'none') return '当前 Thread 尚未设置 Goal';
  if (action === 'pause' && status !== 'active') return '只有 active Goal 可以暂停';
  if (action === 'resume') {
    if (status !== 'paused' && status !== 'blocked') return '只有 paused 或 blocked Goal 可以恢复';
    if (options.activeTurn) return '恢复 Goal 需要空闲 Thread';
  }
  return undefined;
}
