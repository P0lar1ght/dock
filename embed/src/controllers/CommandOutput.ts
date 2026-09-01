import type { SlashExecuteResult } from '../protocol/responses.js';

export type CommandOutputKind = 'notice' | 'applied';

/** Slash / overlay command output shown outside the transcript. */
export interface CommandOutputView {
  kind: CommandOutputKind;
  title: string;
  body: string;
}

export function commandOutputFromSlash(result: SlashExecuteResult): CommandOutputView | undefined {
  if (result.kind !== 'notice' && result.kind !== 'applied') return undefined;
  const title = result.notice?.title?.trim() || '';
  const body = result.notice?.body?.trim() || '';
  if (!title && !body) return undefined;
  return {
    kind: result.kind,
    title: title || (result.kind === 'applied' ? '已完成' : '命令输出'),
    body: body || title
  };
}

/** Plans and rich notices use markdown; /help and /usage stay preformatted. */
export function commandOutputLooksLikeMarkdown(body: string) {
  return /^(#{1,6}\s|```|~~~|\|.+\|)/m.test(body)
    || /^\s*[-*+]\s+\[[ xX]\]/m.test(body)
    || /^\s*[-*+]\s+\S/m.test(body)
    || /(\*\*[^*]+\*\*|`[^`]+`)/u.test(body);
}
