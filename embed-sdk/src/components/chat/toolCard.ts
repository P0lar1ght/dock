/** TUI `glyphs::diamond_filled` — collapsed tool-card bullet. */
export const TOOL_DIAMOND = '\u{25C6}';

const FIRST_LINES = 2;
const LAST_LINES = 3;

export function argumentSummary(toolName: string, raw?: string) {
  if (!raw) return '';
  const value = parseJson(raw);
  // 没有参数的工具（enter_plan_mode 之类）：别把 `{}` 当摘要画出来。
  if (value && typeof value === 'object' && Object.keys(value).length === 0) return '';
  const picked = pickArg(toolName, value);
  if (picked) return oneLine(picked, 72);
  if (value && typeof value === 'object' && !Array.isArray(value)) {
    const first = Object.values(value as Record<string, unknown>).find((item) => typeof item === 'string');
    if (typeof first === 'string' && first.trim()) return oneLine(first, 72);
  }
  return oneLine(raw, 72);
}

export function prettyArgs(raw?: string) {
  if (!raw) return '';
  const value = parseJson(raw);
  if (value === undefined) return raw;
  // 没有参数（`{}` / `[]`）就不画「输入」那一栏：权限请求只带摘要，没有原始参数。
  if (value && typeof value === 'object' && Object.keys(value).length === 0) return '';
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return raw;
  }
}

export function truncatedOutput(text: string) {
  const lines = text.replace(/\r\n?/gu, '\n').split('\n');
  const threshold = FIRST_LINES + LAST_LINES;
  if (lines.length <= threshold) return { text, hidden: 0 };
  const hidden = lines.length - threshold;
  return {
    text: [...lines.slice(0, FIRST_LINES), `\u{2026} +${hidden} lines`, ...lines.slice(-LAST_LINES)].join('\n'),
    hidden
  };
}

export function isShellTool(name: string) {
  return /^(bash|run_terminal_cmd|execute)$/i.test(name);
}

function pickArg(toolName: string, value: unknown) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return '';
  const record = value as Record<string, unknown>;
  // ask_user_question：参数是一组题，摘要用第一题的题干（不是整段 JSON）。
  if (Array.isArray(record.questions)) {
    const first = record.questions[0] as Record<string, unknown> | undefined;
    if (typeof first?.question === 'string' && first.question.trim()) return first.question;
  }
  const keys = keysFor(toolName);
  for (const key of keys) {
    const item = record[key];
    if (typeof item === 'string' && item.trim()) return item;
  }
  return '';
}

function keysFor(toolName: string) {
  if (isShellTool(toolName)) return ['command'];
  if (/^(read_file|write_file|read|write)$/i.test(toolName)) return ['target_file', 'path'];
  if (/^list_dir$/i.test(toolName)) return ['target_directory', 'path'];
  if (/^grep$/i.test(toolName)) return ['pattern'];
  if (/^glob$/i.test(toolName)) return ['glob_pattern', 'pattern'];
  if (/^search_replace$/i.test(toolName)) return ['file_path', 'path'];
  if (/^(web_search|web_fetch)$/i.test(toolName)) return ['query', 'url'];
  return [];
}

function parseJson(raw: string) {
  try {
    return JSON.parse(raw) as unknown;
  } catch {
    return undefined;
  }
}

function oneLine(value: string, limit: number) {
  const flat = value.replace(/\s+/gu, ' ').trim();
  return flat.length <= limit ? flat : `${[...flat].slice(0, limit - 1).join('')}…`;
}
