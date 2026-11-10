// 点缩略图看大图。设计：wonder-ui/chat/Images（iten Gemini-Image Preview）。
// 深色遮罩 + 居中大图 + 顶部说明和关闭；Esc / 点遮罩关。

import { useEffect } from "react";
import { createPortal } from "react-dom";
import { X } from "lucide-react";

export function ImageViewer({ url, caption, onClose }: { url: string; caption: string; onClose: () => void }) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      // 捕获阶段拦下：别让底下的弹层（设置页、预设编辑器）也收到 Esc。
      e.stopPropagation();
      onClose();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onClose]);
  // 挂到 body：在对话流里渲染会被滚动区的渐隐遮罩、动画的 transform 裁在那一栏里。
  // 点击照样沿 React 树冒泡，拦住，别让底下的工具卡跟着收起。
  return createPortal(
    <div
      role="dialog"
      aria-label="图片预览"
      onMouseDown={onClose}
      onClick={(e) => e.stopPropagation()}
      className="enter-fade fixed inset-0 z-[60] flex flex-col items-center justify-center gap-3 bg-black/80 p-8"
    >
      <div className="flex w-full max-w-[90vw] items-center justify-between">
        <span className="text-[13px] font-medium text-white">{caption}</span>
        <button
          type="button"
          title="关闭 Esc"
          onClick={onClose}
          className="motion-colors flex size-[30px] items-center justify-center rounded-full bg-white/10 text-white hover:bg-white/20"
        >
          <X size={16} />
        </button>
      </div>
      <img
        src={url}
        alt={caption}
        onMouseDown={(e) => e.stopPropagation()}
        className="pop-in max-h-[80vh] max-w-[90vw] rounded-lg border border-white/10 object-contain shadow-2xl"
      />
    </div>,
    document.body,
  );
}
