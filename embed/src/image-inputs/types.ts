export type ImageInputDetail = 'auto' | 'low' | 'high';
export type ScreenshotCaptureMode = 'viewport' | 'region' | 'screen' | 'full-page';

export interface ScreenshotTurnInput {
  type: 'screenshot';
  detail?: ImageInputDetail;
  capture?: ScreenshotCaptureMode;
  /** @deprecated Use `{ type: 'reuse', reuseTurnId }` for a prior ordered image set. */
  reuseTurnId?: string;
}

export interface ManualImageTurnInput {
  type: 'image';
  blob: Blob;
  detail?: ImageInputDetail;
  /** Page-lifetime display label only. It is never uploaded or persisted. */
  name?: string;
}

export interface ReuseImageTurnInput {
  type: 'reuse';
  /** Reuse the exact ordered in-memory image set from this prior Turn. */
  reuseTurnId: string;
  detail?: ImageInputDetail;
}

export type ImageTurnInput =
  | ScreenshotTurnInput
  | ManualImageTurnInput
  | ReuseImageTurnInput;

export interface StartTurnInput {
  message: string;
  imageInputs?: readonly ImageTurnInput[];
  intent?: {
    mode: 'default' | 'plan';
    goal?: {
      operation: 'start' | 'resume';
      objective?: string;
      operationId: string;
      expectedRevision?: number;
    };
  };
}

export interface ScreenshotProviderRequest {
  reason: 'user_command';
  signal: AbortSignal;
  limits: Readonly<{
    maxBytes: number;
    maxEdge: number;
    maxPixels: number;
    timeoutMs: number;
  }>;
  target: ScreenshotProviderTarget;
}

export interface ScreenshotRegion {
  /** Viewport-relative CSS pixel coordinate. */
  x: number;
  /** Viewport-relative CSS pixel coordinate. */
  y: number;
  /** Width in CSS pixels. */
  width: number;
  /** Height in CSS pixels. */
  height: number;
}

export type ScreenshotProviderTarget =
  | Readonly<{ kind: 'viewport' }>
  | Readonly<{ kind: 'full-page' }>
  | Readonly<{ kind: 'screen' }>
  | Readonly<{
    kind: 'region';
    rect: Readonly<ScreenshotRegion>;
    devicePixelRatio: number;
  }>;

export interface ScreenshotRegionSelection {
  rect: ScreenshotRegion;
  viewportWidth: number;
  viewportHeight: number;
  devicePixelRatio: number;
}

export interface ScreenshotRegionSelectorRequest {
  reason: 'user_command';
  signal: AbortSignal;
  minSize: number;
}

export type ScreenshotRegionSelector = (
  request: ScreenshotRegionSelectorRequest
) => ScreenshotRegionSelection | Promise<ScreenshotRegionSelection>;

export interface ScreenshotCapture {
  blob: Blob;
  width: number;
  height: number;
  capturedAt: number;
  label?: string;
}

export type ScreenshotProvider = (
  request: ScreenshotProviderRequest
) => ScreenshotCapture | Promise<ScreenshotCapture>;

export interface ScreenshotConfiguration {
  target?: Element | string | (() => Element | null | undefined);
  exclude?: readonly string[];
  mask?: readonly string[];
  regionSelector?: ScreenshotRegionSelector;
  timeoutMs?: number;
  /** Full-page capture timeout; defaults to 30 seconds and remains capped at 30 seconds. */
  fullPageTimeoutMs?: number;
  maxLongestEdge?: number;
  quality?: number;
}

export interface UploadedImageInputReference {
  id: string;
  digest: string;
  detail: ImageInputDetail;
}

export interface ReusedImageInputReference {
  reuseTurnId: string;
  detail: ImageInputDetail;
}

export interface TransientImagePreview {
  blob: Blob;
  source: 'screenshot' | 'upload';
  name?: string;
  mimeType: 'image/png' | 'image/jpeg' | 'image/webp';
  width: number;
  height: number;
  byteLength: number;
}

export type PreparedImageInputReference =
  | (UploadedImageInputReference & { preview: TransientImagePreview })
  | ReusedImageInputReference;

/** @deprecated Use TransientImagePreview. */
export type TransientScreenshotPreview = TransientImagePreview;
