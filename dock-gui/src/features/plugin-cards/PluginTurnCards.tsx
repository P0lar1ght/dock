// 插件挂在一轮回答下面的卡片（Rhai `host.card`）：标题、正文、可选视图树、按钮。
// 按钮点了跑插件的回调（在这一页上），回调回一句话就弹个提示。插件停了卡片留着、按钮置灰。

import { useState } from "react";
import { LoaderCircle, Puzzle } from "lucide-react";
import { turnCardsOf, type TurnCard } from "dock-core";
import { useFeatureHost, type TurnFooterProps } from "../registry";
import { Pill, ViewTree } from "../plugins/ViewTree";
import { cardAction, useTurnCards } from "./store";

export function PluginTurnCards({ threadId, turnId }: TurnFooterProps) {
  const cards = turnCardsOf(useTurnCards(threadId), turnId);
  if (cards.length === 0) return null;
  return (
    <>
      {cards.map((card) => (
        <PluginCard key={card.id} threadId={threadId} card={card} />
      ))}
    </>
  );
}

function PluginCard({ threadId, card }: { threadId: string; card: TurnCard }) {
  const host = useFeatureHost();
  const [busy, setBusy] = useState<string | null>(null);

  const run = async (action: string) => {
    if (!card.live || busy) return;
    setBusy(action);
    try {
      const toast = await cardAction(threadId, card.id, action);
      if (toast) host.toast(toast);
    } catch (e) {
      host.toast(`没做成：${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="md-surface enter-up flex shrink-0 flex-col rounded-[14px] border border-line-strong bg-white">
      <div className="flex items-center gap-3 px-3 py-2.5">
        <span className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-[#f2f2f7] text-ink-3">
          <Puzzle size={16} strokeWidth={1.75} />
        </span>
        <div className="flex min-w-0 flex-1 flex-col gap-0.5">
          <div className="flex min-w-0 items-center gap-2">
            <span className="truncate text-[13.5px] font-medium text-ink">{card.title}</span>
            {card.tone !== "default" && card.tone !== "muted" && <Pill tone={card.tone}>{card.plugin || "插件"}</Pill>}
          </div>
          {card.text && <span className="text-[12.5px] break-words whitespace-pre-wrap text-ink-3">{card.text}</span>}
        </div>
        {card.actions.length > 0 && (
          <div className="flex shrink-0 items-center gap-1.5">
            {card.actions.map((a) => (
              <button
                key={a.id}
                type="button"
                disabled={!card.live || busy !== null}
                title={card.live ? undefined : "发这张卡的插件已经停了"}
                onClick={() => void run(a.id)}
                className="motion-colors flex h-7 shrink-0 items-center gap-1 rounded-lg border border-line-strong bg-white px-3 text-[13px] font-medium text-ink hover:bg-hover active:scale-[0.97] disabled:opacity-50"
              >
                {busy === a.id && <LoaderCircle size={13} className="animate-spin" />}
                {a.label}
              </button>
            ))}
          </div>
        )}
      </div>
      {card.view && (
        <div className="border-t border-line-soft px-3 py-2.5">
          <ViewTree node={card.view} onAction={(action) => void run(action)} busy={busy ?? undefined} />
        </div>
      )}
    </div>
  );
}
