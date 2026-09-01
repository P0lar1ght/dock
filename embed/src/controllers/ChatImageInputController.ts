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
  if (code === 'image_input_cancelled') return '已取消截图';
  if (code === 'image_inputs_forbidden') return '当前 Gateway 未开启图片输入';
  if (code === 'image_input_timeout' || code === 'request_timeout') return '截图或图片上传超时，请重试';
  if (code === 'image_input_protocol_mismatch') return '图片协议与 Gateway 不匹配，请升级后重试';
  if (code === 'image_input_region_too_small') return '选区至少需要 8 × 8 像素';
  if (code === 'image_input_selection_invalidated') {
    return '选择期间页面尺寸或滚动位置发生变化，请重新选择';
  }
  if (code === 'image_input_display_not_allowed') return '未授权或已取消屏幕共享';
  if (code === 'image_input_user_gesture_required') return '请再点一次相机按钮截取屏幕';
  if (code === 'image_input_capture_unsupported') return '当前浏览器或 WebView 不支持屏幕共享截图';
  if (code === 'image_input_capture_failed') return '未能截取所选屏幕，请再试一次';
  if (code === 'image_input_too_large' || code === 'image_input_dimensions_exceeded') {
    return '截图太大，请改选一个窗口后再试';
  }
  if (code === 'image_input_mime_unsupported') return '只支持 PNG、JPEG 或静态 WebP 图片';
  if (code === 'image_input_invalid') return '截图数据无效，请再试一次';
  if (code === 'image_input_full_page_too_large') return '页面过长，最多支持 24 个可视区域';
  if (code === 'image_input_full_page_changed') return '截图期间页面尺寸发生变化，请稳定页面后重试';
  if (code === 'not_connected' || code === 'client_closed') return '尚未连上 Gateway，请等待连接后再截图';
  if (code === 'workspace_not_allowed') return '当前工作区不允许发送图片';
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
  return message.trim() || '消息未被 Gateway 接受，请检查连接后重试';
}
