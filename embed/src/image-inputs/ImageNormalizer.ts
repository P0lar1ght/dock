import { DockClientError } from '../protocol/errors.js';

const ALLOWED_MIME = new Set(['image/webp', 'image/png', 'image/jpeg']);
const MAX_BYTES = 2 * 1024 * 1024;
const MAX_EDGE = 4096;
const MAX_PIXELS = 4_194_304;

export interface NormalizedBrowserImage {
  blob: Blob;
  mimeType: 'image/webp' | 'image/png' | 'image/jpeg';
  width: number;
  height: number;
  byteLength: number;
  digest: string;
  dataBase64: string;
}

export async function normalizeBrowserImage(
  blob: Blob,
  options: { maxLongestEdge: number; quality: number },
  signal: AbortSignal,
  declared?: { width: number; height: number }
): Promise<NormalizedBrowserImage> {
  if (signal.aborted) throw cancelled();
  if (!(blob instanceof Blob) || !ALLOWED_MIME.has(blob.type)) {
    throw new DockClientError(
      'image_input_mime_unsupported',
      'Screenshot Provider must return PNG, JPEG, or WebP'
    );
  }
  if (typeof globalThis.createImageBitmap !== 'function') {
    throw new DockClientError(
      'image_input_capture_unsupported',
      'This WebView cannot decode screenshots safely'
    );
  }
  const bitmap = await globalThis.createImageBitmap(blob);
  try {
    if (signal.aborted) throw cancelled();
    if (
      declared
      && (declared.width !== bitmap.width || declared.height !== bitmap.height)
    ) {
      throw new DockClientError(
        'image_input_invalid',
        'Screenshot Provider dimensions do not match the decoded image'
      );
    }
    const dimensions = targetDimensions(bitmap.width, bitmap.height, options.maxLongestEdge);
    const canvas = document.createElement('canvas');
    canvas.width = dimensions.width;
    canvas.height = dimensions.height;
    const context = canvas.getContext('2d', { alpha: false });
    if (!context) {
      throw new DockClientError('image_input_capture_failed', 'Screenshot canvas is unavailable');
    }
    context.drawImage(bitmap, 0, 0, dimensions.width, dimensions.height);
    const encoded = await canvasBlob(canvas, options.quality);
    if (signal.aborted) throw cancelled();
    if (!ALLOWED_MIME.has(encoded.type) || encoded.size > MAX_BYTES || encoded.size === 0) {
      throw new DockClientError(
        'image_input_too_large',
        'Normalized screenshot exceeds the image input limit'
      );
    }
    const bytes = new Uint8Array(await encoded.arrayBuffer());
    const digestBytes = await globalThis.crypto.subtle.digest('SHA-256', bytes);
    return {
      blob: encoded,
      mimeType: encoded.type as NormalizedBrowserImage['mimeType'],
      width: dimensions.width,
      height: dimensions.height,
      byteLength: bytes.byteLength,
      digest: hex(new Uint8Array(digestBytes)),
      dataBase64: base64(bytes)
    };
  } finally {
    bitmap.close();
  }
}

function targetDimensions(width: number, height: number, configuredEdge: number) {
  if (!Number.isInteger(width) || !Number.isInteger(height) || width < 1 || height < 1) {
    throw new DockClientError('image_input_invalid', 'Screenshot dimensions are invalid');
  }
  const longestEdge = Math.max(1, Math.min(2048, configuredEdge, MAX_EDGE));
  const edgeScale = Math.min(1, longestEdge / Math.max(width, height));
  const pixelScale = Math.min(1, Math.sqrt(MAX_PIXELS / (width * height)));
  const scale = Math.min(edgeScale, pixelScale);
  const result = {
    width: Math.max(1, Math.round(width * scale)),
    height: Math.max(1, Math.round(height * scale))
  };
  if (result.width > MAX_EDGE || result.height > MAX_EDGE || result.width * result.height > MAX_PIXELS) {
    throw new DockClientError('image_input_dimensions_exceeded', 'Screenshot dimensions exceed the limit');
  }
  return result;
}

function canvasBlob(canvas: HTMLCanvasElement, quality: number) {
  return new Promise<Blob>((resolve, reject) => {
    canvas.toBlob(
      (blob) => blob
        ? resolve(blob)
        : reject(new DockClientError('image_input_capture_failed', 'Screenshot encoding failed')),
      'image/webp',
      Math.max(0.1, Math.min(1, quality))
    );
  });
}

function base64(bytes: Uint8Array) {
  let binary = '';
  const chunk = 0x8000;
  for (let offset = 0; offset < bytes.length; offset += chunk) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + chunk));
  }
  return btoa(binary);
}

function hex(bytes: Uint8Array) {
  return [...bytes].map((value) => value.toString(16).padStart(2, '0')).join('');
}

function cancelled() {
  return new DockClientError('image_input_cancelled', 'Screenshot capture was cancelled');
}
