import type { JsonRpcPeer } from '../transport/JsonRpcPeer.js';
import { DockClientError } from '../protocol/errors.js';
import { createDomScreenshotProvider } from './DomScreenshotProvider.js';
import { createDisplayScreenshotProvider } from './DisplayScreenshotProvider.js';
import { createFullPageScreenshotProvider } from './FullPageScreenshotProvider.js';
import { normalizeBrowserImage } from './ImageNormalizer.js';
import {
  MIN_SCREENSHOT_REGION_SIZE,
  selectDomScreenshotRegion
} from './RegionSelector.js';
import type {
  ImageInputDetail,
  ImageTurnInput,
  ManualImageTurnInput,
  ScreenshotConfiguration,
  ScreenshotCaptureMode,
  PreparedImageInputReference,
  ScreenshotProviderTarget,
  ScreenshotRegionSelection,
  ScreenshotProvider
} from './types.js';

type Connection = {
  enabled: boolean;
  application: string;
  origin: string;
  connectionLeaseId: string;
  workspaceIds: string[];
};

const SCREENSHOT_LIMITS = Object.freeze({
  maxBytes: 2 * 1024 * 1024,
  maxSourceBytes: 16 * 1024 * 1024,
  imagesPerTurn: 4,
  maxEdge: 4096,
  maxPixels: 4_194_304
});

export class ImageInputsClient {
  readonly instanceId = randomId();
  private peer?: JsonRpcPeer;
  private connection?: Connection;
  private configuration: ScreenshotConfiguration = {
    timeoutMs: 10_000,
    maxLongestEdge: 2048,
    quality: 0.85
  };
  private customProvider?: ScreenshotProvider;
  private readonly defaultProvider = createDomScreenshotProvider(() => this.configuration);
  private readonly displayProvider = createDisplayScreenshotProvider();
  private readonly fullPageProvider = createFullPageScreenshotProvider(() => this.configuration);

  configureScreenshot(configuration: ScreenshotConfiguration) {
    this.configuration = { ...this.configuration, ...configuration };
  }

  setScreenshotProvider(provider?: ScreenshotProvider) {
    this.customProvider = provider;
  }

  async attach(peer: JsonRpcPeer, connection: Connection) {
    this.peer = peer;
    this.connection = connection;
    if (!connection.enabled) return;
    const synced = await peer.request<{
      connectionLeaseId: string;
      limitsVersion: number;
    }>('imageInputs/sync', { instanceId: this.instanceId });
    if (
      synced.connectionLeaseId !== connection.connectionLeaseId
      || synced.limitsVersion !== 3
    ) {
      this.detach(peer);
      throw new DockClientError(
        'image_input_protocol_mismatch',
        'Gateway image input lease or limits do not match this SDK'
      );
    }
  }

  detach(peer?: JsonRpcPeer) {
    if (peer && this.peer !== peer) return;
    this.peer = undefined;
    this.connection = undefined;
  }

  async prepareTurn(
    threadId: string,
    workspaceId: string,
    inputs: readonly ImageTurnInput[]
  ): Promise<PreparedImageInputReference[]> {
    validateInputs(inputs);
    const peer = this.peer;
    const connection = this.connection;
    if (!peer || !connection) {
      throw new DockClientError('not_connected', 'Connect to Dock Gateway before preparing image input');
    }
    if (!connection.enabled) {
      throw new DockClientError(
        'image_inputs_forbidden',
        'This Application binding is not authorized for image input'
      );
    }
    if (!connection.workspaceIds.includes(workspaceId)) {
      throw new DockClientError('workspace_not_allowed', 'Workspace is not authorized for image input');
    }
    const reuse = reuseInput(inputs[0]);
    if (reuse) {
      return [{
        reuseTurnId: reuse.reuseTurnId,
        detail: detailOf(reuse.detail)
      }];
    }
    const controller = new AbortController();
    const screen = inputs.some((input) =>
      input.type === 'screenshot' && !reuseInput(input) && captureModeOf(input.capture) === 'screen'
    );
    const fullPage = inputs.some((input) =>
      input.type === 'screenshot' && captureModeOf(input.capture) === 'full-page'
    );
    const timeoutMs = screen
      ? 120_000
      : fullPage
        ? this.configuration.fullPageTimeoutMs ?? 30_000
        : 30_000;
    const timer = setTimeout(() => controller.abort(), timeoutMs);
    try {
      const prepared: PreparedImageInputReference[] = [];
      for (const input of inputs) {
        const source = input.type === 'screenshot'
          ? await this.captureScreenshot(
            controller.signal,
            timeoutMs,
            captureModeOf(input.capture)
          )
          : input.type === 'image'
            ? manualImage(input)
            : invalidNewReuse();
        if (input.type === 'image') await assertStaticWebP(input.blob);
        const image = await normalizeBrowserImage(source.blob, {
          maxLongestEdge: this.configuration.maxLongestEdge ?? 2048,
          quality: this.configuration.quality ?? 0.85
        }, controller.signal, source.declared);
        prepared.push(await uploadImage(peer, {
          threadId,
          workspaceId,
          source: input.type === 'screenshot' ? 'screenshot' : 'upload',
          name: input.type === 'image' ? displayName(input.name) : undefined,
          detail: detailOf(input.detail),
          image
        }));
      }
      return prepared;
    } catch (error) {
      if (controller.signal.aborted) {
        throw new DockClientError('image_input_timeout', 'Image preparation timed out');
      }
      throw error;
    } finally {
      clearTimeout(timer);
    }
  }

  private async captureScreenshot(
    signal: AbortSignal,
    timeoutMs: number,
    captureMode: ScreenshotCaptureMode
  ) {
    const provider = this.customProvider
      || (captureMode === 'screen'
        ? this.displayProvider
        : captureMode === 'full-page'
          ? this.fullPageProvider
          : this.defaultProvider);
    const target = captureMode === 'region'
      ? await this.selectRegion(signal)
      : captureMode === 'screen'
        ? { kind: 'screen' } as const
        : captureMode === 'full-page'
          ? { kind: 'full-page' } as const
          : { kind: 'viewport' } as const;
    const capture = await provider({
      reason: 'user_command',
      signal,
      limits: {
        maxBytes: SCREENSHOT_LIMITS.maxBytes,
        maxEdge: SCREENSHOT_LIMITS.maxEdge,
        maxPixels: SCREENSHOT_LIMITS.maxPixels,
        timeoutMs
      },
      target
    });
    validateCapture(capture);
    return {
      blob: capture.blob,
      declared: { width: capture.width, height: capture.height }
    };
  }

  private async selectRegion(signal: AbortSignal): Promise<ScreenshotProviderTarget> {
    const selector = this.configuration.regionSelector || selectDomScreenshotRegion;
    const selection = await selector({
      reason: 'user_command',
      signal,
      minSize: MIN_SCREENSHOT_REGION_SIZE
    });
    validateRegionSelection(selection);
    return {
      kind: 'region',
      rect: { ...selection.rect },
      devicePixelRatio: selection.devicePixelRatio
    };
  }
}

async function uploadImage(
  peer: JsonRpcPeer,
  input: {
    threadId: string;
    workspaceId: string;
    source: 'screenshot' | 'upload';
    name?: string;
    detail: ImageInputDetail;
    image: Awaited<ReturnType<typeof normalizeBrowserImage>>;
  }
): Promise<PreparedImageInputReference> {
  const captureId = randomId();
  const uploaded = await peer.request<{
    imageInputId: string;
    digest: string;
    expiresAt: number;
    limitsVersion: number;
  }>('imageInputs/put', {
    threadId: input.threadId,
    workspaceId: input.workspaceId,
    captureId,
    source: input.source,
    mimeType: input.image.mimeType,
    width: input.image.width,
    height: input.image.height,
    byteLength: input.image.byteLength,
    digest: input.image.digest,
    dataBase64: input.image.dataBase64
  });
  if (uploaded.limitsVersion !== 3 || uploaded.digest !== input.image.digest) {
    throw new DockClientError(
      'image_input_protocol_mismatch',
      'Gateway image input limits or digest do not match this SDK'
    );
  }
  return {
    id: uploaded.imageInputId,
    digest: uploaded.digest,
    detail: input.detail,
    preview: {
      blob: input.image.blob,
      source: input.source,
      name: input.name,
      mimeType: input.image.mimeType,
      width: input.image.width,
      height: input.image.height,
      byteLength: input.image.byteLength
    }
  };
}

function validateInputs(inputs: readonly ImageTurnInput[]) {
  if (!Array.isArray(inputs) || inputs.length < 1 || inputs.length > SCREENSHOT_LIMITS.imagesPerTurn) {
    throw new DockClientError(
      'image_input_invalid',
      `A Turn accepts 1-${SCREENSHOT_LIMITS.imagesPerTurn} images`
    );
  }
  const reuseCount = inputs.filter((input) => Boolean(reuseInput(input))).length;
  if (reuseCount && (reuseCount !== 1 || inputs.length !== 1)) {
    throw new DockClientError(
      'image_input_invalid',
      'A reused image set cannot be mixed with new images'
    );
  }
  if (inputs.filter((input) => input.type === 'screenshot' && !input.reuseTurnId).length > 1) {
    throw new DockClientError(
      'image_input_invalid',
      'A Turn accepts at most one new screenshot'
    );
  }
  for (const input of inputs) {
    if (!input || !['image', 'screenshot', 'reuse'].includes(input.type)) {
      throw new DockClientError('image_input_invalid', 'Image input type is invalid');
    }
    detailOf(input.detail);
    if (input.type === 'image') manualImage(input);
    if (input.type === 'screenshot') captureModeOf(input.capture);
    if (reuseInput(input) && !reuseInput(input)?.reuseTurnId) {
      throw new DockClientError('image_input_invalid', 'reuseTurnId is required');
    }
  }
}

function captureModeOf(value: unknown): ScreenshotCaptureMode {
  if (value === undefined || value === 'viewport') return 'viewport';
  if (value === 'region' || value === 'screen' || value === 'full-page') return value;
  throw new DockClientError(
    'image_input_invalid',
    'Screenshot capture must be viewport, region, screen, or full-page'
  );
}

function validateRegionSelection(
  selection: ScreenshotRegionSelection
): asserts selection is ScreenshotRegionSelection {
  const rect = selection?.rect;
  if (
    !rect
    || !positiveInteger(selection.viewportWidth)
    || !positiveInteger(selection.viewportHeight)
    || !Number.isFinite(selection.devicePixelRatio)
    || selection.devicePixelRatio < 0.5
    || selection.devicePixelRatio > 8
    || !nonNegativeInteger(rect.x)
    || !nonNegativeInteger(rect.y)
    || !positiveInteger(rect.width)
    || !positiveInteger(rect.height)
    || rect.width < MIN_SCREENSHOT_REGION_SIZE
    || rect.height < MIN_SCREENSHOT_REGION_SIZE
    || rect.x + rect.width > selection.viewportWidth
    || rect.y + rect.height > selection.viewportHeight
  ) {
    throw new DockClientError(
      'image_input_invalid',
      'Region Selector returned an invalid viewport rectangle'
    );
  }
}

function positiveInteger(value: unknown): value is number {
  return Number.isInteger(value) && Number(value) > 0;
}

function nonNegativeInteger(value: unknown): value is number {
  return Number.isInteger(value) && Number(value) >= 0;
}

function manualImage(input: ManualImageTurnInput) {
  if (!(input.blob instanceof Blob)) {
    throw new DockClientError('image_input_invalid', 'Manual image input requires a Blob');
  }
  if (!['image/png', 'image/jpeg', 'image/webp'].includes(input.blob.type)) {
    throw new DockClientError(
      'image_input_mime_unsupported',
      'Manual image input must be PNG, JPEG, or static WebP'
    );
  }
  if (input.blob.size < 1 || input.blob.size > SCREENSHOT_LIMITS.maxSourceBytes) {
    throw new DockClientError(
      'image_input_too_large',
      'Manual image source must be between 1 byte and 16 MiB'
    );
  }
  return { blob: input.blob, declared: undefined };
}

function reuseInput(input: ImageTurnInput | undefined) {
  if (!input) return undefined;
  if (input.type === 'reuse') {
    return { reuseTurnId: String(input.reuseTurnId || '').trim(), detail: input.detail };
  }
  if (input.type === 'screenshot' && input.reuseTurnId) {
    return { reuseTurnId: String(input.reuseTurnId || '').trim(), detail: input.detail };
  }
  return undefined;
}

function detailOf(value: unknown): ImageInputDetail {
  if (value === undefined) return 'auto';
  if (value === 'auto' || value === 'low' || value === 'high') return value;
  throw new DockClientError('image_input_invalid', 'Image detail must be auto, low, or high');
}

function displayName(value: unknown) {
  const name = String(value || '').trim().slice(0, 120);
  return name || undefined;
}

function invalidNewReuse(): never {
  throw new DockClientError(
    'image_input_invalid',
    'A reuse input must be the only image input for a Turn'
  );
}

async function assertStaticWebP(blob: Blob) {
  if (blob.type !== 'image/webp') return;
  const bytes = new Uint8Array(await blob.slice(0, 64).arrayBuffer());
  const ascii = String.fromCharCode(...bytes);
  if (
    ascii.slice(0, 4) !== 'RIFF'
    || ascii.slice(8, 12) !== 'WEBP'
    || ascii.includes('ANIM')
    || (ascii.slice(12, 16) === 'VP8X' && Boolean(bytes[20] & 0x02))
  ) {
    throw new DockClientError(
      'image_input_mime_unsupported',
      'Manual WebP input must be a static WebP image'
    );
  }
}

function validateCapture(
  capture: Awaited<ReturnType<ScreenshotProvider>>
): asserts capture is Awaited<ReturnType<ScreenshotProvider>> {
  if (
    !capture
    || !(capture.blob instanceof Blob)
    || capture.blob.size < 1
    || capture.blob.size > SCREENSHOT_LIMITS.maxSourceBytes
    || !Number.isInteger(capture.width)
    || !Number.isInteger(capture.height)
    || capture.width < 1
    || capture.height < 1
    || capture.width > SCREENSHOT_LIMITS.maxEdge
    || capture.height > SCREENSHOT_LIMITS.maxEdge
    || capture.width * capture.height > SCREENSHOT_LIMITS.maxPixels
    || !Number.isFinite(capture.capturedAt)
  ) {
    throw new DockClientError(
      'image_input_invalid',
      'Screenshot Provider returned invalid capture metadata'
    );
  }
}

function randomId() {
  if (typeof globalThis.crypto?.randomUUID === 'function') return globalThis.crypto.randomUUID();
  const bytes = new Uint8Array(16);
  globalThis.crypto.getRandomValues(bytes);
  return [...bytes].map((value) => value.toString(16).padStart(2, '0')).join('');
}
