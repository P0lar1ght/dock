import { DockClientError } from '../protocol/errors.js';
import { hideEmbeddedAgentUi } from './CaptureUiVisibility.js';
import type { ScreenshotProvider } from './types.js';

export interface DisplayCaptureProviderOptions {
  getDisplayMedia?: (options: DisplayMediaStreamOptions) => Promise<MediaStream>;
  createVideo?: () => HTMLVideoElement;
  createCanvas?: () => HTMLCanvasElement;
  waitForFrame?: (
    video: HTMLVideoElement,
    track: MediaStreamTrack,
    signal: AbortSignal
  ) => Promise<void>;
  hideAgentUi?: () => () => void;
}

export function browserDisplayCaptureSupported() {
  return globalThis.isSecureContext !== false
    && typeof globalThis.navigator?.mediaDevices?.getDisplayMedia === 'function';
}

/** Captures exactly one frame from a display surface explicitly chosen by the user agent. */
export function createDisplayScreenshotProvider(
  options: DisplayCaptureProviderOptions = {}
): ScreenshotProvider {
  return async ({ signal, limits, target }) => {
    if (target.kind !== 'screen') {
      throw new DockClientError(
        'image_input_invalid',
        'Display Screenshot Provider requires a screen target'
      );
    }
    if (signal.aborted) throw cancelled();
    const getDisplayMedia = options.getDisplayMedia || browserGetDisplayMedia();
    let stream: MediaStream | undefined;
    let video: HTMLVideoElement | undefined;
    let restoreAgentUi: () => void = () => undefined;
    try {
      stream = await acquireDisplayStream(getDisplayMedia, signal);
      const tracks = stream.getVideoTracks();
      if (tracks.length !== 1) {
        throw new DockClientError(
          'image_input_capture_failed',
          'Screen sharing did not provide exactly one video track'
        );
      }
      const track = tracks[0];
      restoreAgentUi = (options.hideAgentUi || defaultHideAgentUi)();
      video = (options.createVideo || defaultCreateVideo)();
      video.muted = true;
      video.playsInline = true;
      video.srcObject = stream;
      await (options.waitForFrame || waitForFirstFrame)(video, track, signal);
      const dimensions = fitDimensions(
        video.videoWidth || numberSetting(track, 'width'),
        video.videoHeight || numberSetting(track, 'height'),
        limits.maxEdge,
        limits.maxPixels
      );
      const canvas = (options.createCanvas || defaultCreateCanvas)();
      canvas.width = dimensions.width;
      canvas.height = dimensions.height;
      const context = canvas.getContext('2d', { alpha: false });
      if (!context) {
        throw new DockClientError(
          'image_input_capture_failed',
          'Screen capture canvas is unavailable'
        );
      }
      context.drawImage(video, 0, 0, dimensions.width, dimensions.height);
      const blob = await canvasBlob(canvas, signal);
      return {
        blob,
        width: dimensions.width,
        height: dimensions.height,
        capturedAt: Date.now(),
        label: 'shared display'
      };
    } catch (error) {
      throw displayCaptureError(error);
    } finally {
      restoreAgentUi();
      if (video) video.srcObject = null;
      if (stream) stopStream(stream);
    }
  };
}

function browserGetDisplayMedia() {
  if (globalThis.isSecureContext === false) {
    throw new DockClientError(
      'image_input_capture_unsupported',
      'Screen capture requires a secure browser context'
    );
  }
  const mediaDevices = globalThis.navigator?.mediaDevices;
  if (!browserDisplayCaptureSupported() || !mediaDevices) {
    throw new DockClientError(
      'image_input_capture_unsupported',
      'This browser or WebView does not support screen capture'
    );
  }
  return (options: DisplayMediaStreamOptions) => mediaDevices.getDisplayMedia(options);
}

function defaultHideAgentUi() {
  return typeof document === 'undefined' ? () => undefined : hideEmbeddedAgentUi(document);
}

function defaultCreateVideo() {
  if (typeof document === 'undefined') throw unsupported();
  return document.createElement('video');
}

function defaultCreateCanvas() {
  if (typeof document === 'undefined') throw unsupported();
  return document.createElement('canvas');
}

function acquireDisplayStream(
  getDisplayMedia: (options: DisplayMediaStreamOptions) => Promise<MediaStream>,
  signal: AbortSignal
) {
  let pending: Promise<MediaStream>;
  try {
    pending = getDisplayMedia({ video: true, audio: false });
  } catch (error) {
    return Promise.reject(error);
  }
  return new Promise<MediaStream>((resolve, reject) => {
    let settled = false;
    const abort = () => {
      if (settled) return;
      settled = true;
      reject(cancelled());
    };
    signal.addEventListener('abort', abort, { once: true });
    pending.then((stream) => {
      if (settled) {
        stopStream(stream);
        return;
      }
      settled = true;
      signal.removeEventListener('abort', abort);
      resolve(stream);
    }, (error) => {
      if (settled) return;
      settled = true;
      signal.removeEventListener('abort', abort);
      reject(error);
    });
  });
}

async function waitForFirstFrame(
  video: HTMLVideoElement,
  track: MediaStreamTrack,
  signal: AbortSignal
) {
  if (track.readyState === 'ended') throw ended();
  await abortable(video.play(), signal, track);
  if (!hasCurrentFrame(video)) await currentFrameEvent(video, track, signal);
}

function currentFrameEvent(
  video: HTMLVideoElement,
  track: MediaStreamTrack,
  signal: AbortSignal
) {
  return new Promise<void>((resolve, reject) => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    let settled = false;
    const check = () => {
      if (hasCurrentFrame(video)) finish(resolve);
      else timer = setTimeout(check, 25);
    };
    const abort = () => finish(() => reject(cancelled()));
    const trackEnded = () => finish(() => reject(ended()));
    const finish = (done: () => void) => {
      if (settled) return;
      settled = true;
      if (timer) clearTimeout(timer);
      video.removeEventListener('loadeddata', check);
      track.removeEventListener('unmute', check);
      signal.removeEventListener('abort', abort);
      track.removeEventListener('ended', trackEnded);
      done();
    };
    video.addEventListener('loadeddata', check);
    track.addEventListener('unmute', check);
    signal.addEventListener('abort', abort, { once: true });
    track.addEventListener('ended', trackEnded, { once: true });
    check();
  });
}

function hasCurrentFrame(video: HTMLVideoElement) {
  return video.readyState >= 2 && video.videoWidth > 0 && video.videoHeight > 0;
}

function abortable(
  pending: Promise<void>,
  signal: AbortSignal,
  track: MediaStreamTrack
) {
  return new Promise<void>((resolve, reject) => {
    let settled = false;
    const finish = (error?: unknown) => {
      if (settled) return;
      settled = true;
      signal.removeEventListener('abort', abort);
      track.removeEventListener('ended', trackEnded);
      error ? reject(error) : resolve();
    };
    const abort = () => finish(cancelled());
    const trackEnded = () => finish(ended());
    signal.addEventListener('abort', abort, { once: true });
    track.addEventListener('ended', trackEnded, { once: true });
    pending.then(() => finish(), finish);
  });
}

function fitDimensions(width: number, height: number, maxEdge: number, maxPixels: number) {
  if (!Number.isFinite(width) || !Number.isFinite(height) || width < 1 || height < 1) {
    throw new DockClientError(
      'image_input_capture_failed',
      'Screen capture frame has invalid dimensions'
    );
  }
  const safeWidth = Math.floor(width);
  const safeHeight = Math.floor(height);
  const edgeScale = Math.min(1, maxEdge / Math.max(safeWidth, safeHeight));
  const pixelScale = Math.min(1, Math.sqrt(maxPixels / (safeWidth * safeHeight)));
  const scale = Math.min(edgeScale, pixelScale);
  return {
    width: Math.max(1, Math.floor(safeWidth * scale)),
    height: Math.max(1, Math.floor(safeHeight * scale))
  };
}

function numberSetting(track: MediaStreamTrack, key: 'width' | 'height') {
  const value = track.getSettings()[key];
  return typeof value === 'number' ? value : 0;
}

function canvasBlob(canvas: HTMLCanvasElement, signal: AbortSignal) {
  return new Promise<Blob>((resolve, reject) => {
    let settled = false;
    const abort = () => {
      if (settled) return;
      settled = true;
      reject(cancelled());
    };
    signal.addEventListener('abort', abort, { once: true });
    canvas.toBlob((blob) => {
      if (settled) return;
      settled = true;
      signal.removeEventListener('abort', abort);
      blob
        ? resolve(blob)
        : reject(new DockClientError(
          'image_input_capture_failed',
          'Screen capture encoding failed'
        ));
    }, 'image/webp', 0.9);
  });
}

function stopStream(stream: MediaStream) {
  for (const track of new Set(stream.getTracks())) track.stop();
}

function displayCaptureError(error: unknown) {
  if (error instanceof DockClientError) return error;
  const name = error && typeof error === 'object' && 'name' in error
    ? String(error.name)
    : '';
  if (name === 'NotAllowedError') {
    return new DockClientError(
      'image_input_display_not_allowed',
      'Screen sharing was not allowed or was cancelled'
    );
  }
  if (name === 'InvalidStateError') {
    return new DockClientError(
      'image_input_user_gesture_required',
      'Screen sharing requires a new explicit user action'
    );
  }
  if (name === 'NotFoundError') return unsupported();
  if (name === 'AbortError') return cancelled();
  return new DockClientError(
    'image_input_capture_failed',
    'The selected display could not be captured'
  );
}

function unsupported() {
  return new DockClientError(
    'image_input_capture_unsupported',
    'This browser or WebView does not support screen capture'
  );
}

function cancelled() {
  return new DockClientError('image_input_cancelled', 'Screen capture was cancelled');
}

function ended() {
  return new DockClientError(
    'image_input_capture_failed',
    'Screen sharing ended before the first frame was captured'
  );
}
