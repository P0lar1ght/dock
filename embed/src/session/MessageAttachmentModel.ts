export interface SessionImageAttachment {
  type: 'screenshot' | 'image';
  mimeType: 'image/png' | 'image/jpeg' | 'image/webp';
  width: number;
  height: number;
  byteLength: number;
  /** Page-lifetime display label only; never restored from history. */
  name?: string;
  /** Page-lifetime Blob URL for the exact normalized upload; never restored from history. */
  previewUrl?: string;
}

export type SessionScreenshotAttachment = SessionImageAttachment;
export type SessionMessageAttachment = SessionImageAttachment;

/** Projects untrusted transcript attachment metadata without retaining IDs, digests, or bytes. */
export function projectMessageAttachments(value: unknown): SessionMessageAttachment[] {
  if (!Array.isArray(value)) return [];
  return value.slice(0, 4).flatMap((item) => {
    const raw = objectValue(item);
    const mimeType = raw.mimeType;
    const width = finiteInteger(raw.width);
    const height = finiteInteger(raw.height);
    const byteLength = finiteInteger(raw.byteLength);
    if (
      (raw.type !== 'screenshot' && raw.type !== 'image')
      || (mimeType !== 'image/png' && mimeType !== 'image/jpeg' && mimeType !== 'image/webp')
      || width < 1
      || height < 1
      || byteLength < 1
    ) {
      return [];
    }
    return [{
      type: raw.type,
      mimeType,
      width,
      height,
      byteLength
    }];
  });
}

function finiteInteger(value: unknown) {
  const number = Number(value);
  return Number.isSafeInteger(number) ? number : 0;
}

function objectValue(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {};
}
