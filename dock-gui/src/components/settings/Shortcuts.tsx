// 键盘快捷键（lab / was）：分组列出，悬停「修改」「恢复默认」；修改时录下一次按键，
// 冲突就提示「替换」/「取消」。输入框里的键由输入框自己管，只展示。

import { useEffect, useState } from "react";
import { prefs, usePrefs } from "../../dock/prefs";
import { SHORTCUTS, bindingOf, comboOf, conflictOf, keycaps, setBinding, type ShortcutDef } from "../../dock/shortcuts";
import { Banner, Card, Page, Section, flashSaved, secondaryButton } from "./ui";

export function ShortcutsPane() {
  usePrefs(); // 改了键位要重画
  const [recording, setRecording] = useState<string | null>(null);
  const [pending, setPending] = useState<{ id: string; combo: string; conflict: ShortcutDef } | null>(null);

  useEffect(() => {
    if (!recording) return;
    // 捕获阶段拦掉：录制时按下的键不该触发任何全局快捷键（也不该 Esc 关掉设置页）。
    const onKey = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.key === "Escape" && !e.metaKey && !e.ctrlKey && !e.altKey) {
        setRecording(null);
        return;
      }
      const combo = comboOf(e);
      if (!combo) return;
      // 全局快捷键至少要带 ⌘ / ⌃ / ⌥，否则打字时会误触。
      if (!/(Meta|Control|Alt)\+/.test(combo)) return;
      const conflict = conflictOf(recording, combo);
      setRecording(null);
      if (conflict) setPending({ id: recording, combo, conflict });
      else {
        setBinding(recording, combo);
        flashSaved();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [recording]);

  const groups = ["窗口", "面板", "输入框"] as const;
  return (
    <Page
      title="键盘快捷键"
      subtitle={recording ? "正在录制新快捷键… 按 Esc 取消" : "自定义 Dock 的全局快捷键；冲突时会提示"}
      actions={
        <button
          type="button"
          onClick={() => {
            prefs.set("shortcuts", {});
            flashSaved();
          }}
          className={secondaryButton}
        >
          全部恢复默认
        </button>
      }
    >
      {groups.map((g) => (
        <Section key={g} title={g}>
          <Card>
            {SHORTCUTS.filter((s) => s.group === g).map((s) => (
              <div key={s.id} className="border-b border-[#f0f0f3] last:border-b-0">
                <div
                  className={`group flex h-[46px] items-center justify-between gap-4 px-4 ${
                    recording === s.id || pending?.id === s.id ? "bg-[#0a84ff0d]" : "hover:bg-[#fafafc]"
                  }`}
                >
                  <span className="text-[13px] text-ink">{s.label}</span>
                  <div className="flex items-center gap-3">
                    {!s.fixed && recording !== s.id && (
                      <span className="flex gap-2 opacity-0 transition-opacity group-hover:opacity-100">
                        <button type="button" onClick={() => setRecording(s.id)} className="text-[11.5px] font-medium text-accent">
                          修改
                        </button>
                        {prefs.get().shortcuts[s.id] && (
                          <button
                            type="button"
                            onClick={() => {
                              setBinding(s.id, null);
                              flashSaved();
                            }}
                            className="text-[11.5px] text-ink-4 hover:text-ink-2"
                          >
                            恢复默认
                          </button>
                        )}
                      </span>
                    )}
                    {recording === s.id ? (
                      <span className="enter-fade flex h-7 w-[160px] items-center justify-center rounded-md border border-dashed border-accent text-[11.5px] text-accent">
                        按下新快捷键…
                      </span>
                    ) : (
                      <Keycaps combo={bindingOf(s.id)} />
                    )}
                  </div>
                </div>
                {pending?.id === s.id && (
                  <div className="px-4 pb-3">
                    <Banner
                      kind="warn"
                      action={
                        <span className="flex gap-1.5">
                          <button
                            type="button"
                            onClick={() => {
                              // 替换：新动作拿走这个键，原来那个回默认（默认也撞就清空）。
                              const other = pending.conflict;
                              setBinding(pending.id, pending.combo);
                              if (other.default === pending.combo) prefs.set("shortcuts", { ...prefs.get().shortcuts, [other.id]: "" });
                              else setBinding(other.id, null);
                              setPending(null);
                              flashSaved();
                            }}
                            className="h-6 rounded-md bg-ink px-2.5 text-[11.5px] font-medium text-white"
                          >
                            替换
                          </button>
                          <button
                            type="button"
                            onClick={() => setPending(null)}
                            className="h-6 rounded-md border border-line-strong bg-white px-2.5 text-[11.5px] text-ink"
                          >
                            取消
                          </button>
                        </span>
                      }
                    >
                      {keycaps(pending.combo).join("")} 已用于「{pending.conflict.label}」
                    </Banner>
                  </div>
                )}
              </div>
            ))}
          </Card>
        </Section>
      ))}
    </Page>
  );
}

export function Keycaps({ combo }: { combo: string }) {
  if (!combo) return <span className="text-[11.5px] text-ink-4">未设置</span>;
  return (
    <span className="flex gap-1">
      {keycaps(combo).map((k, i) => (
        <kbd
          key={i}
          className="flex h-6 min-w-6 items-center justify-center rounded-md border border-line-strong bg-white px-1.5 font-sans text-[11px] font-medium text-ink-2 shadow-[0_1px_0_rgba(0,0,0,0.06)]"
        >
          {k}
        </kbd>
      ))}
    </span>
  );
}
