// 预设管理：列出全部预设；「编辑」打开预设编辑器，自定义的可删，改过的内置预设是「恢复内置」。
// 编辑器本身在 PresetStudio（盖在设置页上），这里只是入口。

import { useState } from "react";
import { Plus } from "lucide-react";
import { store, useApp } from "../../dock/store";
import { presetIcon } from "../presetIcon";
import { ErrorLine, Page, Pill, primaryButton, smallButton } from "./ui";

const ORIGIN = { shipped: "内置", user: "用户", project: "项目" } as const;

export function PresetsPane() {
  const presets = useApp().presets;
  const [confirm, setConfirm] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const remove = async (id: string) => {
    setConfirm(null);
    setError(null);
    await store.deletePreset(id).catch((e) => setError(e instanceof Error ? e.message : String(e)));
  };
  return (
    <Page
      title="预设管理"
      subtitle="新对话时选预设；「编辑」改人设、工具和子代理。自定义预设存在 ~/.dock/presets"
      actions={
        <button type="button" onClick={() => window.dispatchEvent(new Event("dock:new-preset"))} className={primaryButton}>
          <Plus size={13} />
          新建自定义预设
        </button>
      }
    >
      {error && <ErrorLine>{error}</ErrorLine>}
      <div className="flex flex-col gap-2.5">
        {presets.map((p) => {
          const Icon = presetIcon(p.id);
          return (
            <div key={p.id} className="flex items-center gap-3 rounded-xl border border-line-soft bg-white px-4 py-3.5 shadow-sm">
              <span className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-fill text-ink-2">
                <Icon size={15} />
              </span>
              <div className="flex min-w-0 flex-1 flex-col gap-0.5">
                <div className="flex items-center gap-2">
                  <span className="text-sm font-semibold text-ink">{p.label}</span>
                  <span className="font-mono text-[11px] text-ink-4">{p.id}</span>
                  <Pill>{p.builtin && p.origin !== "shipped" ? "内置 · 已修改" : ORIGIN[p.origin]}</Pill>
                </div>
                <span className={`truncate text-[12.5px] ${p.error ? "text-[#c0392b]" : "text-ink-4"}`}>{p.error ?? p.description}</span>
              </div>
              {confirm !== p.id && (
                <button
                  type="button"
                  onClick={() => window.dispatchEvent(new CustomEvent("dock:edit-preset", { detail: p.id }))}
                  title={p.available ? "编辑人设、工具和子代理" : "文件损坏：查看原因、重建"}
                  className={smallButton}
                >
                  编辑
                </button>
              )}
              {p.origin !== "shipped" &&
                (confirm === p.id ? (
                  <span className="enter-fade flex shrink-0 items-center gap-1.5">
                    <span className="text-[12px] text-ink-3">
                      {p.builtin ? "丢掉你对它的修改、恢复内置版本？" : "删除这个预设？"}
                    </span>
                    <button type="button" onClick={() => setConfirm(null)} className={smallButton}>
                      取消
                    </button>
                    <button
                      type="button"
                      onClick={() => remove(p.id)}
                      className="motion-colors flex h-7 items-center rounded-md bg-[#ff3b30] px-3 text-[11.5px] font-medium text-white hover:bg-[#e0352b]"
                    >
                      {p.builtin ? "恢复内置" : "删除"}
                    </button>
                  </span>
                ) : (
                  <button
                    type="button"
                    onClick={() => setConfirm(p.id)}
                    title={p.builtin ? "这是改过的内置预设：删掉你的修改，回到内置版本" : undefined}
                    className="motion-colors shrink-0 rounded-md px-2 py-1 text-[12px] font-medium text-[#ff3b30] hover:bg-[#ff3b3014]"
                  >
                    {p.builtin ? "恢复内置" : "删除"}
                  </button>
                ))}
            </div>
          );
        })}
      </div>
    </Page>
  );
}
