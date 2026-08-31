import { domToBlob } from 'modern-screenshot';

import { DockClientError } from '../protocol/errors.js';
import type {
  ScreenshotConfiguration,
  ScreenshotProvider,
  ScreenshotProviderTarget
} from './types.js';

export function createDomScreenshotProvider(
  configuration: () => ScreenshotConfiguration
): ScreenshotProvider {
  return async ({ signal, target: requestedTarget }) => {
    if (signal.aborted) throw cancelled();
    const config = configuration();
    const target = resolveDomCaptureTarget(config.target);
    if (!target) {
      throw new DockClientError(
        'image_input_target_missing',
        'Screenshot capture target was not found'
      );
    }
    const view = target.ownerDocument?.defaultView || globalThis.window;
    const geometry = domCaptureGeometry({
      viewportWidth: view?.innerWidth || target.clientWidth,
      viewportHeight: view?.innerHeight || target.clientHeight,
      scrollX: view?.scrollX || 0,
      scrollY: view?.scrollY || 0
    }, requestedTarget);
    const blob = await domToBlob(target, {
      type: 'image/webp',
      quality: Math.max(0.1, Math.min(1, config.quality ?? 0.85)),
      width: geometry.width,
      height: geometry.height,
      scale: 1,
      timeout: Math.max(1, config.timeoutMs ?? 10_000),
      maximumCanvasSize: 4_194_304,
      style: {
        transform: geometry.transform,
        transformOrigin: 'top left'
      },
      features: { restoreScrollPosition: true },
      filter: (node) => includeDomCaptureNode(node, config.exclude),
      onCloneEachNode: (node) => maskDomCaptureClone(node, config.mask)
    });
    if (signal.aborted) throw cancelled();
    return {
      blob,
      width: geometry.width,
      height: geometry.height,
      capturedAt: Date.now()
    };
  };
}

export function domCaptureGeometry(
  viewport: {
    viewportWidth: number;
    viewportHeight: number;
    scrollX: number;
    scrollY: number;
  },
  target: ScreenshotProviderTarget
) {
  const viewportWidth = Math.max(1, Math.round(viewport.viewportWidth));
  const viewportHeight = Math.max(1, Math.round(viewport.viewportHeight));
  const scrollX = Math.max(0, Math.round(viewport.scrollX));
  const scrollY = Math.max(0, Math.round(viewport.scrollY));
  if (target.kind === 'viewport') {
    return {
      width: viewportWidth,
      height: viewportHeight,
      transform: `translate(${-scrollX}px, ${-scrollY}px)`
    };
  }
  if (target.kind !== 'region') {
    throw new DockClientError(
      'image_input_capture_unsupported',
      'DOM Screenshot Provider cannot capture a shared display'
    );
  }
  const { x, y, width, height } = target.rect;
  if (
    ![x, y, width, height].every(Number.isInteger)
    || x < 0
    || y < 0
    || width < 1
    || height < 1
    || x + width > viewportWidth
    || y + height > viewportHeight
  ) {
    throw new DockClientError(
      'image_input_invalid',
      'Screenshot region is outside the current viewport'
    );
  }
  return {
    width,
    height,
    transform: `translate(${-(scrollX + x)}px, ${-(scrollY + y)}px)`
  };
}

export function resolveDomCaptureTarget(target: ScreenshotConfiguration['target']) {
  if (typeof target === 'function') return target() || null;
  if (typeof target === 'string') return document.querySelector(target);
  if (target instanceof Element) return target;
  return document.documentElement;
}

export function includeDomCaptureNode(node: Node, configured: readonly string[] = []) {
  if (!(node instanceof Element)) return true;
  if (node.localName === 'dock-agent') return false;
  if (node.getAttribute('data-dock-capture') === 'exclude') return false;
  return !matchesAny(node, configured);
}

export function maskDomCaptureClone(node: Node, configured: readonly string[] = []) {
  if (!(node instanceof HTMLElement)) return;
  const password = node instanceof HTMLInputElement && node.type === 'password';
  const masked = node.getAttribute('data-dock-capture') === 'mask'
    || matchesAny(node, configured);
  if (!password && !masked) return;
  if (node instanceof HTMLInputElement || node instanceof HTMLTextAreaElement) {
    node.value = '';
  } else {
    node.replaceChildren();
  }
  node.style.setProperty('background', '#111', 'important');
  node.style.setProperty('color', 'transparent', 'important');
  node.style.setProperty('border-color', '#111', 'important');
}

function matchesAny(node: Element, selectors: readonly string[]) {
  return selectors.some((selector) => {
    try {
      return Boolean(selector && node.matches(selector));
    } catch {
      throw new DockClientError(
        'image_input_selector_invalid',
        'Screenshot exclude or mask selector is invalid'
      );
    }
  });
}

function cancelled() {
  return new DockClientError('image_input_cancelled', 'Screenshot capture was cancelled');
}
