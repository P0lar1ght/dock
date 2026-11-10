// 排队中的消息：本轮结束后才发。虚线灰气泡靠右，下方「排队中 · 本轮结束后发送」+ 立即插话 + 取消。
// 设计：wonder-ui/chat/UserTurnQueued（fuci）。放在对话流和输入框之间。
// 插话（点了「立即插话」）：Dock 在下一个步骤边界把它并进正在跑的这一轮，不打断采样和工具；
// 送达前蓝色实线气泡 +「插话 · 等待合适时机送达」，还能取消。送达后它出现在对话流里。

import { useEffect, useState } from "react";
import { Clock, CornerDownRight, ImageIcon, X } from "lucide-react";
import type { QueuedPrompt } from "../dock/store";
import { stripImageChips } from "../dock/chips";
import { fetchUserImage } from "../dock/userImages";

/** 等待送达期间显示的缩略图：像素按摘要向 Dock 要（还没落进历史，走 item/image）。 */
function QueuedImage_({ threadId, digest, width, height }: { threadId: string; digest: string; width: number; height: number }) {
  const [url, setUrl] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    void fetchUserImage(threadId, digest).then((u) => {
      if (alive) setUrl(u);
    });
    return () => {
      alive = false;
    };
  }, [threadId, digest]);
  return url ? (
    <img src={url} alt="" className="h-[64px] w-[104px] rounded-md border border-line-strong object-cover" />
  ) : (
    <div
      title={`${width}×${height}`}
      className="flex h-[64px] w-[104px] items-center justify-center rounded-md border border-line-strong bg-[#e8e8ec] text-ink-4"
    >
      <ImageIcon size={18} strokeWidth={1.5} />
    </div>
  );
}

export function Queued({
  threadId,
  items,
  canSteer,
  onSteer,
  onCancel,
}: {
  threadId: string;
  items: QueuedPrompt[];
  /** 网关支持插话（`turnSteer`）才给「立即插话」。 */
  canSteer: boolean;
  onSteer: (queueId: string) => void;
  onCancel: (queueId: string) => void;
}) {
  return (
    <div className="mx-auto flex w-full max-w-[900px] shrink-0 flex-col items-end gap-3 px-4 pb-2">
      {items.map((q) => {
        const steer = q.kind === "steer";
        // 带图的消息正文前面有「[Image #1] 」，图在下面单独显示，正文里不重复。
        const text = q.images.length > 0 ? stripImageChips(q.message) : q.message;
        return (
          <div key={q.id} className="enter-up flex max-w-[420px] flex-col items-end gap-1.5">
            <div
              className={`motion-colors flex max-w-full flex-col gap-2 rounded-[14px] rounded-br-sm border px-3.5 py-2.5 text-[13px] leading-[1.4] shadow-sm ${
                steer
                  ? "border-[#0a84ff33] bg-[#0a84ff0f] text-ink"
                  : "border-dashed border-[#d1d1d6] bg-[#f0f0f2e6] text-ink-2"
              }`}
            >
              {q.images.length > 0 && (
                <div className="flex flex-wrap justify-end gap-2">
                  {q.images.map((img) => (
                    <QueuedImage_
                      key={img.digest}
                      threadId={threadId}
                      digest={img.digest}
                      width={img.width}
                      height={img.height}
                    />
                  ))}
                </div>
              )}
              {text && <div className="whitespace-pre-wrap wrap-anywhere">{text}</div>}
            </div>
            <div className="flex items-center gap-2 pr-1">
              {steer ? (
                <span className="flex items-center gap-1 text-[11px] text-accent">
                  <CornerDownRight size={12} />
                  <span className="shimmer-text">插话 · 等待合适时机送达</span>
                </span>
              ) : (
                <span className="flex items-center gap-1 text-[11px] text-ink-4">
                  <Clock size={12} />
                  排队中 · 本轮结束后发送
                </span>
              )}
              {!steer && canSteer && (
                <button
                  type="button"
                  title="不等这一轮结束：在下一个步骤边界并进正在跑的这一轮（不打断正在进行的思考和工具）"
                  onClick={() => onSteer(q.id)}
                  className="motion-colors flex items-center gap-0.5 rounded-full bg-[#0a84ff14] px-1.5 py-px text-[10px] font-medium text-accent hover:bg-[#0a84ff24] active:scale-95"
                >
                  <CornerDownRight size={11} />
                  立即插话
                </button>
              )}
              <button
                type="button"
                title={steer ? "撤回这条插话（还没送达）" : "取消这条排队的消息"}
                onClick={() => onCancel(q.id)}
                className="motion-colors flex items-center gap-0.5 rounded-full bg-[#e8e8ec] px-1.5 py-px text-[10px] font-medium text-ink-3 hover:bg-[#dcdce0] hover:text-ink active:scale-95"
              >
                <X size={11} />
                取消
              </button>
            </div>
          </div>
        );
      })}
    </div>
  );
}
