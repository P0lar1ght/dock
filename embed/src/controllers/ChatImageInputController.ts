import {
  PendingImageInputStore,
  parseScreenshotCommand,
  type ImageTurnInput,
  type StartTurnInput
} from '../image-inputs/index.js';

/** Owns page-lifetime Composer image Blobs independently from Session history. */
export class ChatImageInputController {
  private readonly store = new PendingImageInputStore();

  get pending() {
    return this.store.snapshot();
  }

  add(files: readonly (File | Blob)[], imageSupported: boolean) {
    if (!imageSupported) throw new Error('当前模型未声明图片输入能力');
    this.store.add(files);
  }

  remove(id: string) {
    return this.store.remove(id);
  }

  move(id: string, delta: number) {
    return this.store.move(id, delta);
  }

  submission(message: string): string | StartTurnInput {
    const manual = this.store.inputs();
    const parsed = parseScreenshotCommand(message);
    if (!manual.length) return parsed;
    if (typeof parsed === 'string') {
      return { message: parsed, imageInputs: manual };
    }
    const commandImages = parsed.imageInputs || [];
    if (commandImages.some((image) =>
      image.type === 'reuse' || ('reuseTurnId' in image && Boolean(image.reuseTurnId))
    )) {
      throw new Error('复用图片不能与新选择的图片混合');
    }
    const imageInputs: readonly ImageTurnInput[] = [...manual, ...commandImages];
    if (imageInputs.length > 4) throw new Error('每轮最多发送 4 张图片');
    return { message: parsed.message, imageInputs };
  }

  clear() {
    this.store.clear();
  }
}

export function imageSubmissionError(error: unknown) {
  const code = error && typeof error === 'object' && 'code' in error
    ? String(error.code)
    : '';
  if (code === 'image_input_cancelled') return '已取消区域截图';
  if (code === 'image_input_region_too_small') return '选区至少需要 8 × 8 像素';
  if (code === 'image_input_selection_invalidated') {
    return '选择期间页面尺寸或滚动位置发生变化，请重新选择';
  }
  if (code === 'image_input_display_not_allowed') return '未授权或已取消屏幕共享';
  if (code === 'image_input_user_gesture_required') return '请通过输入框命令重新发起屏幕截图';
  if (code === 'image_input_capture_unsupported') return '当前浏览器或 WebView 不支持屏幕共享截图';
  if (code === 'image_input_full_page_too_large') return '页面过长，最多支持 24 个可视区域';
  if (code === 'image_input_full_page_changed') return '截图期间页面尺寸发生变化，请稳定页面后重试';
  const message = error instanceof Error ? error.message : '';
  if (
    message.includes('PNG')
    || message.includes('JPEG')
    || message.includes('WebP')
    || message.includes('16 MiB')
    || message.includes('4 images')
    || message.includes('4 张')
    || message.includes('复用图片')
    || message.includes('图片输入能力')
    || message.includes('Planning Turn')
    || message.includes('/plan')
  ) {
    return message;
  }
  return '消息未被 Gateway 接受，请检查连接后重试';
}
