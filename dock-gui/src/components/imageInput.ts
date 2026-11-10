// 输入框附件：把用户给的图（选文件 / 粘贴 / 拖进来）规范成网关收的样子。
// 网关的限制（cordis-gateway image_store.rs）：PNG / JPEG / WebP，≤ 2MB，边长 ≤ 4096，
// digest = 字节的 SHA-256 十六进制，一轮最多 4 张。

import type { ImageInput } from "../model";

export const MAX_IMAGES = 4;
const MAX_BYTES = 2 * 1024 * 1024;
const MAX_SIDE = 4096;
const ACCEPTED = ["image/png", "image/jpeg", "image/webp"];

/**
 * 读一张图并规范化。本来就合规的原样发（截图多是 PNG，不重新编码免得糊字）；
 * 不合规的（HEIC / GIF、太大、太长）画到 canvas 上按 JPEG 重编，放不下就逐次缩小。
 * 解不开的图抛中文错误。
 */
export async function prepareImage(file: Blob): Promise<ImageInput> {
  let bitmap: ImageBitmap;
  try {
    bitmap = await createImageBitmap(file);
  } catch {
    throw new Error("这张图打不开（只支持常见图片格式）");
  }
  const { width, height } = bitmap;
  let bytes: Uint8Array<ArrayBuffer>;
  let mimeType = file.type;
  let outW = width;
  let outH = height;
  if (ACCEPTED.includes(file.type) && file.size <= MAX_BYTES && width <= MAX_SIDE && height <= MAX_SIDE) {
    bytes = new Uint8Array(await file.arrayBuffer());
  } else {
    mimeType = "image/jpeg";
    let scale = Math.min(1, MAX_SIDE / Math.max(width, height));
    let blob: Blob | null = null;
    for (let attempt = 0; attempt < 8; attempt++) {
      outW = Math.max(1, Math.round(width * scale));
      outH = Math.max(1, Math.round(height * scale));
      blob = await encodeJpeg(bitmap, outW, outH);
      if (blob.size <= MAX_BYTES) break;
      scale *= 0.75;
    }
    if (!blob || blob.size > MAX_BYTES) throw new Error("这张图太大，压不到 2MB 以内");
    bytes = new Uint8Array(await blob.arrayBuffer());
  }
  bitmap.close();
  const digest = hex(await crypto.subtle.digest("SHA-256", bytes));
  return {
    url: URL.createObjectURL(new Blob([bytes], { type: mimeType })),
    mimeType,
    width: outW,
    height: outH,
    byteLength: bytes.length,
    digest,
    base64: toBase64(bytes),
  };
}

async function encodeJpeg(bitmap: ImageBitmap, w: number, h: number): Promise<Blob> {
  const canvas = document.createElement("canvas");
  canvas.width = w;
  canvas.height = h;
  const g = canvas.getContext("2d");
  if (!g) throw new Error("画布不可用，没法压缩图片");
  // 透明底的图转 JPEG 会变黑：先铺白。
  g.fillStyle = "#fff";
  g.fillRect(0, 0, w, h);
  g.drawImage(bitmap, 0, 0, w, h);
  return new Promise((resolve, reject) =>
    canvas.toBlob((b) => (b ? resolve(b) : reject(new Error("图片编码失败"))), "image/jpeg", 0.88),
  );
}

function hex(buf: ArrayBuffer): string {
  return [...new Uint8Array(buf)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

function toBase64(bytes: Uint8Array): string {
  let s = "";
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(s);
}

/** 「PNG · 1920×1080 · 340 KB」：缩略图悬停和大图预览的说明。 */
export function describeImage(img: { mimeType: string; width: number; height: number; byteLength: number }): string {
  const kind = img.mimeType.replace("image/", "").toUpperCase();
  const size = img.byteLength >= 1024 * 1024 ? `${(img.byteLength / 1024 / 1024).toFixed(1)} MB` : `${Math.max(1, Math.round(img.byteLength / 1024))} KB`;
  return `${kind} · ${img.width}×${img.height} · ${size}`;
}
