export function delay(milliseconds: number) {
  return new Promise<void>((resolve) => setTimeout(resolve, milliseconds));
}

export function resizeComposer(input: HTMLTextAreaElement | null) {
  if (!input) return;
  input.style.height = 'auto';
  input.style.height = `${Math.min(input.scrollHeight, 112)}px`;
}

const FOLLOW_SCROLL_SLOP_PX = 64;

export function isNearBottom(
  list: Pick<HTMLElement, 'scrollHeight' | 'scrollTop' | 'clientHeight'>,
  slop = FOLLOW_SCROLL_SLOP_PX
) {
  return list.scrollHeight - list.scrollTop - list.clientHeight <= slop;
}
