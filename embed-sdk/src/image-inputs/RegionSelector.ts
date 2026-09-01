import { DockClientError } from '../protocol/errors.js';
import { hideEmbeddedAgentUi } from './CaptureUiVisibility.js';
import type {
  ScreenshotRegion,
  ScreenshotRegionSelection,
  ScreenshotRegionSelector
} from './types.js';

export const MIN_SCREENSHOT_REGION_SIZE = 8;

type Point = Readonly<{ x: number; y: number }>;
type Viewport = Readonly<{ width: number; height: number }>;

export const selectDomScreenshotRegion: ScreenshotRegionSelector = async ({
  signal,
  minSize
}) => {
  if (signal.aborted) throw cancelled();
  if (typeof document === 'undefined' || typeof window === 'undefined' || !document.body) {
    throw new DockClientError(
      'image_input_capture_unsupported',
      'Region selection requires a browser document'
    );
  }
  const viewport = currentViewport(window);
  const previousFocus = document.activeElement instanceof HTMLElement
    ? document.activeElement
    : undefined;
  const overlay = createOverlay(document);
  const selectionBox = overlay.querySelector<HTMLElement>('[data-region-selection]');
  const hint = overlay.querySelector<HTMLElement>('[data-region-hint]');
  const cancelButton = overlay.querySelector<HTMLButtonElement>('[data-region-cancel]');
  if (!selectionBox || !hint || !cancelButton) throw captureFailed();
  const restoreAgentUi = hideEmbeddedAgentUi(document);

  return new Promise<ScreenshotRegionSelection>((resolve, reject) => {
    let settled = false;
    let pointerId: number | undefined;
    let start: Point | undefined;

    const cleanup = () => {
      overlay.remove();
      restoreAgentUi();
      signal.removeEventListener('abort', abort);
      window.removeEventListener('keydown', keydown, true);
      window.removeEventListener('resize', invalidate);
      window.removeEventListener('scroll', invalidate, true);
      previousFocus?.focus({ preventScroll: true });
    };
    const fail = (error: DockClientError) => {
      if (settled) return;
      settled = true;
      cleanup();
      reject(error);
    };
    const complete = (rect: ScreenshotRegion) => {
      if (settled) return;
      settled = true;
      cleanup();
      afterOverlayRemoval(window, signal).then(() => resolve({
        rect,
        viewportWidth: viewport.width,
        viewportHeight: viewport.height,
        devicePixelRatio: boundedDevicePixelRatio(window.devicePixelRatio)
      }), reject);
    };
    const abort = () => fail(cancelled());
    const invalidate = () => fail(new DockClientError(
      'image_input_selection_invalidated',
      'The page changed while selecting a screenshot region'
    ));
    const keydown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      event.preventDefault();
      event.stopPropagation();
      fail(cancelled());
    };
    const pointerDown = (event: PointerEvent) => {
      if (event.button !== 0 || (event.target as Element | null)?.closest('[data-region-cancel]')) {
        return;
      }
      event.preventDefault();
      pointerId = event.pointerId;
      start = clampPoint({ x: event.clientX, y: event.clientY }, viewport);
      overlay.setPointerCapture?.(event.pointerId);
      renderSelection(selectionBox, start, start);
      hint.textContent = '拖动选择要发送给模型的区域';
    };
    const pointerMove = (event: PointerEvent) => {
      if (pointerId !== event.pointerId || !start) return;
      event.preventDefault();
      renderSelection(
        selectionBox,
        start,
        clampPoint({ x: event.clientX, y: event.clientY }, viewport)
      );
    };
    const pointerUp = (event: PointerEvent) => {
      if (pointerId !== event.pointerId || !start) return;
      event.preventDefault();
      const rect = normalizeScreenshotRegion(
        start,
        clampPoint({ x: event.clientX, y: event.clientY }, viewport),
        viewport,
        minSize
      );
      if (!rect) {
        fail(new DockClientError(
          'image_input_region_too_small',
          `Screenshot region must be at least ${minSize} × ${minSize} CSS pixels`
        ));
        return;
      }
      complete(rect);
    };

    overlay.addEventListener('pointerdown', pointerDown);
    overlay.addEventListener('pointermove', pointerMove);
    overlay.addEventListener('pointerup', pointerUp);
    overlay.addEventListener('pointercancel', () => fail(cancelled()));
    overlay.addEventListener('wheel', (event) => event.preventDefault(), { passive: false });
    cancelButton.addEventListener('click', () => fail(cancelled()));
    signal.addEventListener('abort', abort, { once: true });
    window.addEventListener('keydown', keydown, true);
    window.addEventListener('resize', invalidate, { once: true });
    window.addEventListener('scroll', invalidate, { capture: true, once: true });
    document.body.append(overlay);
    cancelButton.focus({ preventScroll: true });
  });
};

export function normalizeScreenshotRegion(
  start: Point,
  end: Point,
  viewport: Viewport,
  minSize = MIN_SCREENSHOT_REGION_SIZE
): ScreenshotRegion | undefined {
  const first = clampPoint(start, viewport);
  const last = clampPoint(end, viewport);
  const x = Math.floor(Math.min(first.x, last.x));
  const y = Math.floor(Math.min(first.y, last.y));
  const right = Math.ceil(Math.max(first.x, last.x));
  const bottom = Math.ceil(Math.max(first.y, last.y));
  const width = right - x;
  const height = bottom - y;
  if (width < minSize || height < minSize) return undefined;
  return { x, y, width, height };
}

function createOverlay(documentValue: Document) {
  const overlay = documentValue.createElement('div');
  overlay.setAttribute('data-dock-capture', 'exclude');
  overlay.setAttribute('data-dock-region-selector', '');
  overlay.setAttribute('role', 'dialog');
  overlay.setAttribute('aria-label', '选择要发送给模型的页面区域');
  Object.assign(overlay.style, {
    position: 'fixed',
    inset: '0',
    zIndex: '2147483646',
    cursor: 'crosshair',
    touchAction: 'none',
    userSelect: 'none',
    background: 'rgba(8, 15, 28, 0.48)'
  });

  const hint = documentValue.createElement('div');
  hint.setAttribute('data-region-hint', '');
  hint.textContent = '拖动选择区域 · Esc 取消';
  Object.assign(hint.style, {
    position: 'fixed',
    top: '18px',
    left: '50%',
    transform: 'translateX(-50%)',
    padding: '10px 14px',
    borderRadius: '10px',
    color: '#fff',
    background: 'rgba(8, 15, 28, 0.9)',
    font: '600 14px/1.4 system-ui, sans-serif',
    boxShadow: '0 8px 24px rgba(0, 0, 0, 0.24)',
    pointerEvents: 'none'
  });

  const selection = documentValue.createElement('div');
  selection.setAttribute('data-region-selection', '');
  Object.assign(selection.style, {
    position: 'fixed',
    display: 'none',
    border: '2px solid #7dd3fc',
    borderRadius: '4px',
    background: 'rgba(125, 211, 252, 0.12)',
    boxShadow: '0 0 0 1px rgba(8, 15, 28, 0.7)',
    pointerEvents: 'none'
  });

  const cancel = documentValue.createElement('button');
  cancel.type = 'button';
  cancel.setAttribute('data-region-cancel', '');
  cancel.textContent = '取消';
  Object.assign(cancel.style, {
    position: 'fixed',
    top: '18px',
    right: '18px',
    padding: '9px 14px',
    border: '1px solid rgba(255, 255, 255, 0.35)',
    borderRadius: '9px',
    color: '#fff',
    background: 'rgba(8, 15, 28, 0.9)',
    font: '600 13px/1 system-ui, sans-serif',
    cursor: 'pointer'
  });

  overlay.append(hint, selection, cancel);
  return overlay;
}

function renderSelection(element: HTMLElement, start: Point, end: Point) {
  const x = Math.min(start.x, end.x);
  const y = Math.min(start.y, end.y);
  element.style.display = 'block';
  element.style.left = `${x}px`;
  element.style.top = `${y}px`;
  element.style.width = `${Math.abs(end.x - start.x)}px`;
  element.style.height = `${Math.abs(end.y - start.y)}px`;
}

function currentViewport(windowValue: Window): Viewport {
  const width = Math.max(1, Math.floor(windowValue.innerWidth));
  const height = Math.max(1, Math.floor(windowValue.innerHeight));
  return { width, height };
}

function clampPoint(point: Point, viewport: Viewport): Point {
  return {
    x: Math.max(0, Math.min(viewport.width, Number(point.x) || 0)),
    y: Math.max(0, Math.min(viewport.height, Number(point.y) || 0))
  };
}

function boundedDevicePixelRatio(value: number) {
  return Number.isFinite(value) ? Math.max(0.5, Math.min(8, value)) : 1;
}

function afterOverlayRemoval(windowValue: Window, signal: AbortSignal) {
  if (signal.aborted) return Promise.reject(cancelled());
  return new Promise<void>((resolve, reject) => {
    const callback = () => signal.aborted ? reject(cancelled()) : resolve();
    if (typeof windowValue.requestAnimationFrame === 'function') {
      windowValue.requestAnimationFrame(() => windowValue.requestAnimationFrame(callback));
    } else {
      windowValue.setTimeout(callback, 0);
    }
  });
}

function cancelled() {
  return new DockClientError('image_input_cancelled', 'Screenshot capture was cancelled');
}

function captureFailed() {
  return new DockClientError(
    'image_input_capture_failed',
    'Region selection controls could not be created'
  );
}
