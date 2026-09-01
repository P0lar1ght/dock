import { domToBlob } from 'modern-screenshot';

import { DockClientError } from '../protocol/errors.js';
import { hideEmbeddedAgentUi } from './CaptureUiVisibility.js';
import {
  includeDomCaptureNode,
  maskDomCaptureClone,
  resolveDomCaptureTarget
} from './DomScreenshotProvider.js';
import type {
  ScreenshotConfiguration,
  ScreenshotProvider
} from './types.js';

export const MAX_FULL_PAGE_TILES = 24;

interface FullPageMetrics {
  viewportWidth: number;
  viewportHeight: number;
  pageHeight: number;
  scrollX: number;
  scrollY: number;
}

interface FullPageTile {
  index: number;
  contentY: number;
  scrollY: number;
  sourceY: number;
  sourceHeight: number;
}

interface DecodedTile {
  source: CanvasImageSource;
  width: number;
  height: number;
  close(): void;
}

interface FullPageRuntime {
  metrics(): FullPageMetrics;
  begin(): () => void;
  scrollTo(left: number, top: number): void;
  waitForScroll(expectedY: number, signal: AbortSignal): Promise<void>;
  captureTile(index: number, metrics: FullPageMetrics): Promise<Blob>;
  decode(blob: Blob): Promise<DecodedTile>;
  createCanvas(width: number, height: number): HTMLCanvasElement;
  encode(canvas: HTMLCanvasElement, quality: number): Promise<Blob>;
}

export interface FullPageCaptureProviderOptions {
  createRuntime?: (
    configuration: ScreenshotConfiguration,
    limits: {
      maxEdge: number;
      maxPixels: number;
      timeoutMs: number;
    }
  ) => FullPageRuntime;
}

/** Captures the current document vertically in bounded viewport tiles. */
export function createFullPageScreenshotProvider(
  configuration: () => ScreenshotConfiguration,
  options: FullPageCaptureProviderOptions = {}
): ScreenshotProvider {
  return async ({ signal, limits, target }) => {
    if (target.kind !== 'full-page') {
      throw new DockClientError(
        'image_input_invalid',
        'Full-page Screenshot Provider requires a full-page target'
      );
    }
    if (signal.aborted) throw cancelled();
    const config = configuration();
    const runtime = (options.createRuntime || createDomFullPageRuntime)(config, limits);
    const initial = checkedMetrics(runtime.metrics());
    const tiles = fullPageTiles(initial);
    const output = fitFullPageDimensions(
      initial.viewportWidth,
      initial.pageHeight,
      limits.maxEdge,
      limits.maxPixels
    );
    const restore = runtime.begin();
    try {
      const canvas = runtime.createCanvas(output.width, output.height);
      const context = canvas.getContext('2d', { alpha: false });
      if (!context) throw captureFailed('Full-page screenshot canvas is unavailable');
      for (const tile of tiles) {
        if (signal.aborted) throw cancelled();
        runtime.scrollTo(initial.scrollX, tile.scrollY);
        await runtime.waitForScroll(tile.scrollY, signal);
        const current = checkedMetrics(runtime.metrics());
        assertStablePage(initial, current, tile.scrollY);
        const blob = await abortable(runtime.captureTile(tile.index, current), signal);
        const decoded = await abortable(runtime.decode(blob), signal);
        try {
          drawTile(context, decoded, tile, initial, output);
        } finally {
          decoded.close();
        }
      }
      const blob = await abortable(
        runtime.encode(canvas, Math.max(0.1, Math.min(1, config.quality ?? 0.85))),
        signal
      );
      if (!blob.size) throw captureFailed('Full-page screenshot encoding failed');
      return {
        blob,
        width: output.width,
        height: output.height,
        capturedAt: Date.now(),
        label: 'full page'
      };
    } catch (error) {
      if (error instanceof DockClientError) throw error;
      throw captureFailed('Full-page screenshot capture failed');
    } finally {
      try {
        runtime.scrollTo(initial.scrollX, initial.scrollY);
        await runtime.waitForScroll(initial.scrollY, new AbortController().signal);
      } finally {
        restore();
      }
    }
  };
}

export function fullPageTiles(metrics: FullPageMetrics): FullPageTile[] {
  const count = Math.ceil(metrics.pageHeight / metrics.viewportHeight);
  if (count > MAX_FULL_PAGE_TILES) {
    throw new DockClientError(
      'image_input_full_page_too_large',
      `Full-page capture accepts at most ${MAX_FULL_PAGE_TILES} viewport tiles`
    );
  }
  const maxScroll = Math.max(0, metrics.pageHeight - metrics.viewportHeight);
  const tiles: FullPageTile[] = [];
  let contentY = 0;
  while (contentY < metrics.pageHeight) {
    const scrollY = Math.min(contentY, maxScroll);
    const sourceY = contentY - scrollY;
    const sourceHeight = Math.min(
      metrics.viewportHeight - sourceY,
      metrics.pageHeight - contentY
    );
    tiles.push({
      index: tiles.length,
      contentY,
      scrollY,
      sourceY,
      sourceHeight
    });
    contentY += sourceHeight;
  }
  return tiles;
}

export function fitFullPageDimensions(
  width: number,
  height: number,
  maxEdge: number,
  maxPixels: number
) {
  const edgeScale = Math.min(1, maxEdge / Math.max(width, height));
  const pixelScale = Math.min(1, Math.sqrt(maxPixels / (width * height)));
  const scale = Math.min(edgeScale, pixelScale);
  return {
    width: Math.max(1, Math.floor(width * scale)),
    height: Math.max(1, Math.floor(height * scale))
  };
}

function createDomFullPageRuntime(
  config: ScreenshotConfiguration,
  limits: { maxEdge: number; maxPixels: number; timeoutMs: number }
): FullPageRuntime {
  const target = resolveDomCaptureTarget(config.target);
  if (!target) throw new DockClientError(
    'image_input_target_missing',
    'Screenshot capture target was not found'
  );
  const documentValue = target.ownerDocument;
  const view = documentValue?.defaultView;
  if (!documentValue || !view) throw captureFailed('Full-page document is unavailable');
  const root = documentValue.documentElement;
  const body = documentValue.body;
  const scrollingElement = documentValue.scrollingElement || root;
  const tileScale = () => {
    const metrics = readMetrics();
    return Math.min(
      1,
      limits.maxEdge / Math.max(metrics.viewportWidth, metrics.viewportHeight),
      Math.sqrt(limits.maxPixels / (metrics.viewportWidth * metrics.viewportHeight))
    );
  };
  const readMetrics = (): FullPageMetrics => ({
    viewportWidth: Math.max(1, Math.round(view.innerWidth || root.clientWidth)),
    viewportHeight: Math.max(1, Math.round(view.innerHeight || root.clientHeight)),
    pageHeight: Math.max(
      view.innerHeight || root.clientHeight,
      root.scrollHeight,
      body?.scrollHeight || 0,
      scrollingElement.scrollHeight
    ),
    scrollX: Math.max(0, Math.round(view.scrollX)),
    scrollY: Math.max(0, Math.round(view.scrollY))
  });
  return {
    metrics: readMetrics,
    begin: () => {
      const restoreAgentUi = hideEmbeddedAgentUi(documentValue);
      const styles = uniqueElements([root, scrollingElement]).map((element) => ({
        element,
        value: element.style.getPropertyValue('scroll-behavior'),
        priority: element.style.getPropertyPriority('scroll-behavior')
      }));
      for (const { element } of styles) {
        element.style.setProperty('scroll-behavior', 'auto', 'important');
      }
      return () => {
        for (const { element, value, priority } of styles) {
          if (value) element.style.setProperty('scroll-behavior', value, priority);
          else element.style.removeProperty('scroll-behavior');
        }
        restoreAgentUi();
      };
    },
    scrollTo: (left, top) => view.scrollTo({ left, top, behavior: 'auto' }),
    waitForScroll: (expectedY, signal) => waitForScroll(view, expectedY, signal),
    captureTile: (index, metrics) => domToBlob(target, {
      type: 'image/webp',
      quality: Math.max(0.1, Math.min(1, config.quality ?? 0.85)),
      width: metrics.viewportWidth,
      height: metrics.viewportHeight,
      scale: tileScale(),
      timeout: Math.max(1, limits.timeoutMs),
      maximumCanvasSize: limits.maxPixels,
      style: {
        transformOrigin: 'top left'
      },
      features: { restoreScrollPosition: true },
      filter: (node) => includeDomCaptureNode(node, config.exclude),
      onCloneEachNode: (node) => {
        maskDomCaptureClone(node, config.mask);
        normalizeFullPageClone(node, index);
      }
    }),
    decode: async (blob) => {
      if (typeof globalThis.createImageBitmap !== 'function') {
        throw new DockClientError(
          'image_input_capture_unsupported',
          'This WebView cannot decode full-page screenshot tiles'
        );
      }
      const bitmap = await globalThis.createImageBitmap(blob);
      return {
        source: bitmap,
        width: bitmap.width,
        height: bitmap.height,
        close: () => bitmap.close()
      };
    },
    createCanvas: (width, height) => {
      const canvas = documentValue.createElement('canvas');
      canvas.width = width;
      canvas.height = height;
      return canvas;
    },
    encode: (canvas, quality) => canvasBlob(canvas, quality)
  };
}

function normalizeFullPageClone(node: Node, tileIndex: number) {
  if (!(node instanceof HTMLElement)) return;
  const position = node.style.getPropertyValue('position');
  if (position === 'sticky') node.style.setProperty('position', 'static', 'important');
  if (position === 'fixed' && tileIndex > 0) {
    node.style.setProperty('visibility', 'hidden', 'important');
  }
}

function drawTile(
  context: CanvasRenderingContext2D,
  decoded: DecodedTile,
  tile: FullPageTile,
  page: FullPageMetrics,
  output: { width: number; height: number }
) {
  if (decoded.width < 1 || decoded.height < 1) throw captureFailed('Screenshot tile is empty');
  const sourceScale = decoded.height / page.viewportHeight;
  const sourceY = Math.round(tile.sourceY * sourceScale);
  const sourceHeight = Math.max(1, Math.round(tile.sourceHeight * sourceScale));
  const destinationY = Math.round(tile.contentY * output.height / page.pageHeight);
  const destinationBottom = Math.round(
    (tile.contentY + tile.sourceHeight) * output.height / page.pageHeight
  );
  context.drawImage(
    decoded.source,
    0,
    sourceY,
    decoded.width,
    Math.min(sourceHeight, decoded.height - sourceY),
    0,
    destinationY,
    output.width,
    Math.max(1, destinationBottom - destinationY)
  );
}

function checkedMetrics(value: FullPageMetrics) {
  if (
    !positiveInteger(value.viewportWidth)
    || !positiveInteger(value.viewportHeight)
    || !positiveInteger(value.pageHeight)
    || value.pageHeight < value.viewportHeight
    || !nonNegativeInteger(value.scrollX)
    || !nonNegativeInteger(value.scrollY)
  ) {
    throw captureFailed('Full-page screenshot geometry is invalid');
  }
  return value;
}

function assertStablePage(initial: FullPageMetrics, current: FullPageMetrics, expectedY: number) {
  let reason = '';
  if (
    current.viewportWidth !== initial.viewportWidth
    || current.viewportHeight !== initial.viewportHeight
  ) reason = 'viewport changed';
  else if (current.pageHeight !== initial.pageHeight) reason = 'document height changed';
  else if (Math.abs(current.scrollY - expectedY) > 4) {
    reason = `scroll position did not settle (${current.scrollY}/${expectedY})`;
  }
  if (!reason) return;
  throw new DockClientError(
    'image_input_full_page_changed',
    `Page geometry changed during full-page capture: ${reason}`
  );
}

function abortable<T>(pending: Promise<T>, signal: AbortSignal) {
  return new Promise<T>((resolve, reject) => {
    let settled = false;
    const finish = (value: { result: T } | { error: unknown }) => {
      if (settled) return;
      settled = true;
      signal.removeEventListener('abort', abort);
      if ('error' in value) reject(value.error);
      else resolve(value.result);
    };
    const abort = () => finish({ error: cancelled() });
    signal.addEventListener('abort', abort, { once: true });
    pending.then(
      (result) => finish({ result }),
      (error) => finish({ error })
    );
  });
}

function waitForScroll(view: Window, expectedY: number, signal: AbortSignal) {
  return new Promise<void>((resolve, reject) => {
    let attempts = 0;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const check = () => {
      if (Math.abs(Math.round(view.scrollY) - expectedY) <= 4 || attempts >= 20) {
        finish();
        return;
      }
      attempts += 1;
      timer = setTimeout(check, 25);
    };
    const abort = () => {
      if (timer) clearTimeout(timer);
      signal.removeEventListener('abort', abort);
      reject(cancelled());
    };
    function finish() {
      if (timer) clearTimeout(timer);
      signal.removeEventListener('abort', abort);
      resolve();
    }
    signal.addEventListener('abort', abort, { once: true });
    timer = setTimeout(check, 25);
  });
}

function canvasBlob(canvas: HTMLCanvasElement, quality: number) {
  return new Promise<Blob>((resolve, reject) => {
    canvas.toBlob(
      (blob) => blob ? resolve(blob) : reject(captureFailed('Full-page screenshot encoding failed')),
      'image/webp',
      quality
    );
  });
}

function uniqueElements(elements: Element[]) {
  return [...new Set(elements)] as HTMLElement[];
}

function positiveInteger(value: unknown): value is number {
  return Number.isInteger(value) && Number(value) > 0;
}

function nonNegativeInteger(value: unknown): value is number {
  return Number.isInteger(value) && Number(value) >= 0;
}

function cancelled() {
  return new DockClientError('image_input_cancelled', 'Screenshot capture was cancelled');
}

function captureFailed(message: string) {
  return new DockClientError('image_input_capture_failed', message);
}
