import type { TransientImagePreview } from '../image-inputs/types.js';
import type { SessionImageAttachment } from './MessageAttachmentModel.js';
import type { SessionMessage } from './MessageModel.js';
import type { SessionState } from './SessionState.js';

interface StoredPreview extends Omit<TransientImagePreview, 'blob'> {
  previewUrl: string;
}

const MAX_TRANSIENT_TURNS = 4;

/**
 * Keeps a small, page-lifetime view of the exact normalized screenshot sent
 * for a Turn. Blob URLs never enter protocol messages or browser storage.
 */
export class TransientScreenshotPreviewStore {
  private readonly previews = new Map<string, readonly StoredPreview[]>();
  private unclaimed?: readonly StoredPreview[];

  add(
    turnId: string,
    value: TransientImagePreview | readonly TransientImagePreview[]
  ) {
    const id = String(turnId || '').trim();
    if (typeof globalThis.URL?.createObjectURL !== 'function') return;
    const previews = Array.isArray(value) ? value : [value];
    const stored = previews.slice(0, 4).map((preview) => ({
      previewUrl: globalThis.URL.createObjectURL(preview.blob),
      source: preview.source || 'screenshot',
      name: preview.name,
      mimeType: preview.mimeType,
      width: preview.width,
      height: preview.height,
      byteLength: preview.byteLength
    }));
    if (id) {
      this.remove(id);
      this.previews.set(id, stored);
    }
    this.unclaimed = stored;
    while (this.previews.size > MAX_TRANSIENT_TURNS) {
      const oldest = this.previews.keys().next().value;
      if (typeof oldest !== 'string') break;
      this.remove(oldest);
    }
  }

  project(state: SessionState): SessionState {
    this.claimUnclaimed(state);
    let changed = false;
    const messages = state.messages.map((message) => {
      const previews = this.previews.get(message.turnId);
      const current = message.attachments || [];
      const attachments = current.length
        ? current.map((attachment, index) => {
          const preview = fittingPreview(previews?.[index], attachment);
          if (!preview && !attachment.previewUrl) return attachment;
          const next: SessionImageAttachment = preview
            ? {
                ...attachment,
                previewUrl: preview.previewUrl,
                name: preview.name || attachment.name,
                type: preview.source === 'screenshot' ? 'screenshot' : attachment.type
              }
            : withoutPreview(attachment);
          if (samePreview(attachment, next)) return attachment;
          changed = true;
          return next;
        })
        : previews?.length && message.role === 'user'
          ? synthesize(previews)
          : current;
      if (!current.length && attachments?.length) changed = true;
      if (attachments === message.attachments) return message;
      return { ...message, attachments };
    });
    return changed ? { ...state, messages } : state;
  }

  clear() {
    for (const turnId of [...this.previews.keys()]) this.remove(turnId);
    this.unclaimed = undefined;
  }

  private claimUnclaimed(state: SessionState) {
    const unclaimed = this.unclaimed;
    if (!unclaimed) return;
    if (state.messages.some((message) => this.previews.get(message.turnId) === unclaimed)) {
      this.unclaimed = undefined;
      return;
    }
    const latestUser = [...state.messages].reverse().find((message) => message.role === 'user');
    if (!latestUser || this.previews.has(latestUser.turnId)) return;
    if (!fitsMessage(unclaimed, latestUser)) return;
    this.previews.set(latestUser.turnId, unclaimed);
    this.unclaimed = undefined;
  }

  private remove(turnId: string) {
    const current = this.previews.get(turnId) || [];
    const shared = [...this.previews.entries()].some(
      ([id, value]) => id !== turnId && value === current
    );
    if (!shared) {
      for (const preview of current) globalThis.URL.revokeObjectURL(preview.previewUrl);
    }
    this.previews.delete(turnId);
    if (this.unclaimed === current) this.unclaimed = undefined;
  }
}

function synthesize(previews: readonly StoredPreview[]): SessionImageAttachment[] {
  return previews.map((preview) => ({
    type: preview.source === 'screenshot' ? 'screenshot' : 'image',
    mimeType: preview.mimeType,
    width: preview.width,
    height: preview.height,
    byteLength: preview.byteLength,
    name: preview.name,
    previewUrl: preview.previewUrl
  }));
}

function fitsMessage(previews: readonly StoredPreview[], message: SessionMessage) {
  const attachments = message.attachments || [];
  if (!attachments.length) return true;
  if (attachments.length !== previews.length) return false;
  return attachments.every((attachment, index) => fittingPreview(previews[index], attachment));
}

function fittingPreview(
  preview: StoredPreview | undefined,
  attachment: SessionImageAttachment
) {
  if (!preview) return undefined;
  if (
    preview.mimeType !== attachment.mimeType
    || preview.width !== attachment.width
    || preview.height !== attachment.height
    || preview.byteLength !== attachment.byteLength
  ) {
    return undefined;
  }
  return preview;
}

function withoutPreview(attachment: SessionImageAttachment): SessionImageAttachment {
  const { previewUrl: _previewUrl, name: _name, ...safe } = attachment;
  return safe;
}

function samePreview(
  current: SessionImageAttachment,
  next: SessionImageAttachment
) {
  return current.previewUrl === next.previewUrl
    && current.type === next.type
    && current.name === next.name
    && current.width === next.width
    && current.height === next.height
    && current.byteLength === next.byteLength;
}
