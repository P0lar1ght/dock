import { DockClientError } from '../protocol/errors.js';

const TYPES: ReadonlyArray<['image/webp' | 'image/jpeg' | 'image/png', number | undefined]> = [
  ['image/webp', 0.85],
  ['image/jpeg', 0.85],
  ['image/png', undefined]
];

export async function encodeCanvasBlob(
  canvas: HTMLCanvasElement,
  quality = 0.85,
  signal?: AbortSignal
): Promise<Blob> {
  const bounded = Math.max(0.1, Math.min(1, quality));
  for (const [type, configured] of TYPES) {
    if (signal?.aborted) {
      throw new DockClientError('image_input_cancelled', 'Screenshot capture was cancelled');
    }
    const blob = await canvasToBlob(canvas, type, configured === undefined ? undefined : bounded, signal);
    if (blob && blob.size > 0) {
      return blob.type ? blob : new Blob([blob], { type });
    }
  }
  throw new DockClientError('image_input_capture_failed', 'Screen capture encoding failed');
}

function canvasToBlob(
  canvas: HTMLCanvasElement,
  type: string,
  quality: number | undefined,
  signal?: AbortSignal
) {
  return new Promise<Blob | null>((resolve, reject) => {
    let settled = false;
    const abort = () => {
      if (settled) return;
      settled = true;
      reject(new DockClientError('image_input_cancelled', 'Screenshot capture was cancelled'));
    };
    signal?.addEventListener('abort', abort, { once: true });
    const finish = (blob: Blob | null) => {
      if (settled) return;
      settled = true;
      signal?.removeEventListener('abort', abort);
      resolve(blob);
    };
    try {
      canvas.toBlob(finish, type, quality);
    } catch (error) {
      signal?.removeEventListener('abort', abort);
      reject(error);
    }
  });
}
