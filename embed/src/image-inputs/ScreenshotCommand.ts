import type { StartTurnInput } from './types.js';

const COMMAND = 'screenshot';
const DEFAULT_QUERY = '请分析当前界面。';

export function isScreenshotCommand(value: string) {
  return new RegExp(`^/${COMMAND}(?:\\s|$)`, 'u').test(String(value || '').trim());
}

export function parseScreenshotCommand(
  value: string,
  latestScreenshotTurnId?: string
): StartTurnInput | string {
  const trimmed = String(value || '').trim();
  const match = trimmed.match(/^\/screenshot(?:\s+([\s\S]*))?$/u);
  if (!match) return trimmed;
  const argument = String(match[1] || '').trim();
  const reuse = argument.match(/^--reuse(?:\s+([\s\S]*))?$/u);
  const region = argument.match(/^--region(?:\s+([\s\S]*))?$/u);
  const screen = argument.match(/^--screen(?:\s+([\s\S]*))?$/u);
  const fullPage = argument.match(/^--full-page(?:\s+([\s\S]*))?$/u);
  if (reuse && !latestScreenshotTurnId) {
    throw new Error('No reusable screenshot Turn is available in this page session');
  }
  const query = reuse
    ? reuse[1]
    : region
      ? region[1]
      : screen
        ? screen[1]
        : fullPage
          ? fullPage[1]
          : argument;
  const message = String(query || '').trim() || DEFAULT_QUERY;
  return {
    message,
    imageInputs: reuse
      ? [{ type: 'reuse', detail: 'auto', reuseTurnId: latestScreenshotTurnId || '' }]
      : [{
        type: 'screenshot',
        detail: 'auto',
        ...(region
          ? { capture: 'region' as const }
          : screen
            ? { capture: 'screen' as const }
            : fullPage
              ? { capture: 'full-page' as const }
              : {})
      }]
  };
}
