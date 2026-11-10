// 新对话：先选 Agent 预设，再定项目目录，然后 `thread/start {cwd, presetId}`。
// 预设在这里定下，之后不在会话中途换（设计稿 Dock New Chat Select Preset）。

import { useEffect, useState } from "react";
import { Folder, FolderOpen } from "lucide-react";
import type { PresetOption } from "../model";
import { store } from "../dock/store";
import { pickDirectory } from "./pickDirectory";
import { presetIcon } from "./presetIcon";

export function NewChat({
  presets,
  defaultPreset,
  defaultCwd,
  busy,
  onStart,
  onNewPreset,
}: {
  presets: PresetOption[];
  defaultPreset: string | null;
  /** 当前选中会话的目录，没有就空着让用户填。 */
  defaultCwd: string;
  busy: boolean;
  onStart: (presetId: string, cwd: string) => void;
  onNewPreset: () => void;
}) {
  const [presetId, setPresetId] = useState(defaultPreset ?? "");
  const [cwd, setCwd] = useState(defaultCwd);
  // 默认预设变了（品牌行改了默认、刚新建了预设）就跟着选中它。
  useEffect(() => {
    if (defaultPreset) setPresetId(defaultPreset);
  }, [defaultPreset]);

  // 选文件夹框打不开（浏览器预览）就只留手填，按钮藏起来。
  const [canPick, setCanPick] = useState(true);
  const pick = async () => {
    try {
      const dir = await pickDirectory(cwd.trim());
      if (dir) setCwd(dir);
    } catch {
      setCanPick(false);
    }
  };

  // 项目层预设（`<项目>/.dock/presets`）只在那个项目里能选：目录定了就按它重列一次。
  const [scoped, setScoped] = useState<{ cwd: string; presets: PresetOption[] } | null>(null);
  useEffect(() => {
    const dir = cwd.trim();
    if (!dir.startsWith("/")) return;
    let live = true;
    const t = setTimeout(() => {
      store
        .presetsAt(dir)
        .then((list) => live && setScoped({ cwd: dir, presets: list }))
        .catch(() => {});
    }, 300);
    return () => {
      live = false;
      clearTimeout(t);
    };
  }, [cwd]);
  const shown = scoped && scoped.cwd === cwd.trim() ? scoped.presets : presets;

  const chosen = shown.find((p) => p.id === presetId);
  const project = cwd.split("/").filter(Boolean).pop() ?? "";
  const ready = !!chosen?.available && cwd.trim().startsWith("/") && !busy;

  return (
    <div className="enter-up flex flex-1 flex-col items-center justify-center gap-5 px-6">
      <div className="flex flex-col items-center gap-1.5">
        <div className="text-[17px] font-semibold text-ink">新对话</div>
        <div className="text-xs text-ink-4">先选择 Agent 预设，会话将归入该预设下的项目目录</div>
      </div>

      <div className="flex w-full max-w-[420px] flex-col gap-2.5 rounded-2xl border border-line-strong bg-white p-4 shadow-[0px_4px_16px_rgba(0,0,0,0.05)]">
        <div className="flex items-center justify-between">
          <span className="text-[13px] font-semibold text-ink">选择 Agent 预设</span>
          <button type="button" onClick={onNewPreset} className="text-xs font-medium text-accent hover:underline">
            新建预设
          </button>
        </div>

        {shown.map((p) => {
          const Icon = presetIcon(p.id);
          const selected = p.id === presetId;
          return (
            <button
              key={p.id}
              type="button"
              disabled={!p.available}
              title={p.error ?? undefined}
              onClick={() => setPresetId(p.id)}
              className={`motion-colors flex items-center gap-3 rounded-xl border px-3 py-2.5 text-left active:scale-[0.99] disabled:opacity-50 ${
                selected ? "border-accent bg-[#f0f7ff]" : "border-line-strong hover:bg-hover"
              }`}
            >
              <span
                className={`flex size-4 shrink-0 items-center justify-center rounded-full border ${
                  selected ? "border-accent" : "border-ink-5"
                }`}
              >
                {selected && <span className="pop-in size-2 rounded-full bg-accent" />}
              </span>
              <Icon size={15} className="shrink-0 text-ink-3" />
              <span className="flex min-w-0 flex-1 flex-col gap-0.5">
                <span className="text-[13px] font-medium text-ink">{p.label}</span>
                <span className="line-clamp-2 text-[11px] leading-[1.4] text-ink-4">
                  {p.available ? p.description : `不可用：${p.error ?? "预设损坏"}`}
                </span>
              </span>
              <span className="shrink-0 font-mono text-[11px] text-ink-4">{p.id}</span>
            </button>
          );
        })}

        <div className="motion-colors mt-1 flex items-center gap-2 rounded-xl border border-line-strong py-1 pl-3 pr-1 focus-within:border-accent">
          <button
            type="button"
            title="选择项目目录"
            disabled={!canPick}
            onClick={pick}
            className="shrink-0 text-ink-4 hover:text-ink-2 disabled:hover:text-ink-4"
          >
            <Folder size={14} />
          </button>
          <input
            value={cwd}
            onChange={(e) => setCwd(e.target.value)}
            placeholder="项目目录（绝对路径）"
            spellCheck={false}
            className="min-w-0 flex-1 bg-transparent py-1 font-mono text-xs text-ink outline-none placeholder:text-ink-4 select-text"
          />
          {canPick && (
            <button
              type="button"
              onClick={pick}
              className="motion-colors flex h-7 shrink-0 items-center gap-1.5 rounded-lg border border-line-strong bg-white px-2.5 text-xs font-medium text-ink-2 shadow-sm hover:bg-hover active:scale-[0.97]"
            >
              <FolderOpen size={13} className="text-ink-3" />
              选择…
            </button>
          )}
        </div>

        <div className="mt-1 flex items-center justify-between gap-3">
          <span className="min-w-0 truncate text-[11px] text-ink-4">
            将归入：{chosen?.label ?? "—"}
            {project && ` / ${project}`}
          </span>
          <button
            type="button"
            disabled={!ready}
            onClick={() => onStart(presetId, cwd.trim())}
            className="motion-colors flex h-8 shrink-0 items-center rounded-lg bg-accent px-4 text-[13px] font-medium text-white hover:bg-[#0077ed] active:scale-[0.97] disabled:opacity-50"
          >
            {busy ? "创建中…" : "继续"}
          </button>
        </div>
      </div>
    </div>
  );
}
