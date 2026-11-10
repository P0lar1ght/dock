// 用户头像：设置 › 个性化里传的图（prefs.avatar），没有就画名字首字。

import { displayName, usePrefs } from "../dock/prefs";

export function Avatar({ size = 22, className = "" }: { size?: number; className?: string }) {
  const p = usePrefs();
  const name = displayName(p);
  return p.avatar ? (
    <img
      src={p.avatar}
      alt=""
      draggable={false}
      style={{ width: size, height: size }}
      className={`glass-keep shrink-0 rounded-full object-cover ${className}`}
    />
  ) : (
    <span
      style={{ width: size, height: size, fontSize: Math.max(10, Math.round(size * 0.45)) }}
      className={`glass-keep flex shrink-0 items-center justify-center rounded-full bg-[#d1d1d6] font-semibold text-ink-2 ${className}`}
    >
      {[...name][0]?.toUpperCase()}
    </span>
  );
}

/** 选的图片缩成正方形 128px 的 JPEG data URL（居中裁切），存 localStorage 不占地方。 */
export async function avatarFromFile(file: File): Promise<string> {
  const url = URL.createObjectURL(file);
  try {
    const img = await new Promise<HTMLImageElement>((resolve, reject) => {
      const i = new Image();
      i.onload = () => resolve(i);
      i.onerror = () => reject(new Error("读不了这张图"));
      i.src = url;
    });
    const side = Math.min(img.naturalWidth, img.naturalHeight);
    const canvas = document.createElement("canvas");
    canvas.width = canvas.height = 128;
    const ctx = canvas.getContext("2d");
    if (!ctx) throw new Error("画不了头像");
    ctx.drawImage(img, (img.naturalWidth - side) / 2, (img.naturalHeight - side) / 2, side, side, 0, 0, 128, 128);
    return canvas.toDataURL("image/jpeg", 0.9);
  } finally {
    URL.revokeObjectURL(url);
  }
}
