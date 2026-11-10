// 侧栏「+」：创建项目 = 选一个 Agent 预设 + 一个目录，GUI 本地记住（store.addProject）。
// 设计：wonder-ui/shell/CreateProject（pdf 空状态 / yeo 选好文件夹）。

import { useEffect, useState } from "react";
import { Folder, FolderPlus, X } from "lucide-react";
import type { PresetOption } from "../model";
import { basename } from "../dock/store";
import { pickDirectory } from "./pickDirectory";
import { presetIcon } from "./presetIcon";
import { imeComposing } from "../dock/ime";

export function CreateProject({
  presets,
  defaultPreset,
  onCreate,
  onCancel,
}: {
  presets: PresetOption[];
  defaultPreset: string | null;
  onCreate: (project: { presetId: string; cwd: string; name: string }) => void;
  onCancel: () => void;
}) {
  const [name, setName] = useState("");
  const [presetId, setPresetId] = useState(defaultPreset ?? presets.find((p) => p.available)?.id ?? "");
  const [cwd, setCwd] = useState("");
  // 原生选文件夹框打不开（浏览器预览里没有 Tauri）：退回手填绝对路径。
  const [manual, setManual] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onCancel();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onCancel]);

  const pick = async () => {
    try {
      const dir = await pickDirectory(cwd);
      if (dir) {
        setCwd(dir);
        setError(null);
      }
    } catch {
      setManual(true);
    }
  };

  const chosen = presets.find((p) => p.id === presetId);
  const path = cwd.trim();
  const ready = !!chosen?.available && path.startsWith("/");
  const submit = () => {
    if (!path.startsWith("/")) {
      setError("目录要是绝对路径");
      return;
    }
    if (ready) onCreate({ presetId, cwd: path, name: name.trim() });
  };

  return (
    <div className="enter-fade fixed inset-0 z-50 flex items-center justify-center bg-black/15" onMouseDown={onCancel}>
      <div
        role="dialog"
        aria-label="创建项目"
        onMouseDown={(e) => e.stopPropagation()}
        className="glass-dialog pop-in flex w-[440px] flex-col gap-4 rounded-[14px] border border-line-strong bg-white p-5 shadow-xl"
      >
        <div className="flex items-center justify-between">
          <span className="text-[17px] font-semibold text-ink">创建项目</span>
          <button
            type="button"
            title="关闭"
            onClick={onCancel}
            className="motion-colors flex size-7 items-center justify-center rounded-lg text-ink-4 hover:bg-hover hover:text-ink"
          >
            <X size={16} />
          </button>
        </div>

        <label className="motion-colors flex h-9 items-center rounded-[10px] border border-line-strong bg-white focus-within:border-accent focus-within:ring-2 focus-within:ring-[#0a84ff26]">
          <span className="flex h-full w-9 shrink-0 items-center justify-center border-r border-line-strong">
            <Folder size={16} className="text-ink-3" />
          </span>
          <input
            autoFocus
            value={name}
            onChange={(e) => setName(e.target.value)}
            onKeyDown={(e) => !imeComposing(e) && e.key === "Enter" && submit()}
            placeholder={path ? basename(path) : "项目名称（默认取文件夹名）"}
            className="min-w-0 flex-1 bg-transparent px-3 text-[13px] text-ink outline-none placeholder:text-ink-4 select-text"
          />
        </label>

        <div className="flex flex-col gap-2">
          <span className="text-xs font-medium text-ink-2">Agent 预设</span>
          <div className="flex flex-wrap gap-1.5">
            {presets.map((p) => {
              const Icon = presetIcon(p.id);
              const selected = p.id === presetId;
              return (
                <button
                  key={p.id}
                  type="button"
                  disabled={!p.available}
                  title={p.available ? p.description : `不可用：${p.error ?? "预设损坏"}`}
                  onClick={() => setPresetId(p.id)}
                  className={`motion-colors flex h-7 items-center gap-1.5 rounded-full border px-2.5 text-xs active:scale-[0.97] disabled:opacity-50 ${
                    selected ? "border-accent bg-[#f0f7ff] font-medium text-accent" : "border-line-strong text-ink-2 hover:bg-hover"
                  }`}
                >
                  <Icon size={14} className={selected ? "text-accent" : "text-ink-3"} />
                  {p.label}
                </button>
              );
            })}
          </div>
        </div>

        <div className="flex flex-col gap-2">
          <span className="text-xs font-medium text-ink-2">源文件夹</span>
          {manual ? (
            <label className="flex h-9 items-center gap-2 rounded-xl border border-line-strong px-3 focus-within:border-accent">
              <Folder size={14} className="shrink-0 text-ink-4" />
              <input
                value={cwd}
                onChange={(e) => setCwd(e.target.value)}
                onKeyDown={(e) => !imeComposing(e) && e.key === "Enter" && submit()}
                placeholder="项目目录（绝对路径）"
                spellCheck={false}
                className="min-w-0 flex-1 bg-transparent font-mono text-xs text-ink outline-none placeholder:text-ink-4 select-text"
              />
            </label>
          ) : path ? (
            <div className="enter-fade flex h-11 items-center gap-2.5 rounded-xl border border-line-strong bg-pane px-3">
              <Folder size={16} className="shrink-0 text-ink-3" />
              <span className="flex min-w-0 flex-1 flex-col">
                <span className="truncate text-[13px] font-medium text-ink">{basename(path)}</span>
                <span className="truncate font-mono text-[11px] text-ink-4">{path}</span>
              </span>
              <button type="button" onClick={pick} className="shrink-0 text-xs font-medium text-accent hover:underline">
                更换
              </button>
              <button
                type="button"
                title="移除"
                onClick={() => setCwd("")}
                className="motion-colors flex size-5 shrink-0 items-center justify-center rounded text-ink-4 hover:bg-hover hover:text-ink"
              >
                <X size={14} />
              </button>
            </div>
          ) : (
            <div className="flex h-[120px] flex-col items-center justify-center gap-3 rounded-xl border border-dashed border-ink-5 bg-pane">
              <span className="text-xs text-ink-4">在此电脑上选择项目目录</span>
              <button
                type="button"
                onClick={pick}
                className="motion-colors flex h-7 items-center gap-1.5 rounded-lg border border-line-strong bg-white px-3 text-xs font-medium text-ink shadow-sm hover:bg-hover active:scale-[0.97]"
              >
                <FolderPlus size={14} className="text-ink-3" />
                添加
              </button>
            </div>
          )}
          {error ? (
            <span className="enter-fade text-[11px] text-[#ff3b30]">{error}</span>
          ) : (
            path && (
              <span className="min-w-0 truncate text-[11px] text-ink-4">
                将归入：{chosen?.label ?? "—"} / {name.trim() || basename(path)}
              </span>
            )
          )}
        </div>

        <div className="flex justify-end gap-2">
          <button
            type="button"
            onClick={onCancel}
            className="motion-colors flex h-8 items-center rounded-lg border border-line-strong bg-white px-4 text-[13px] font-medium text-ink hover:bg-hover"
          >
            取消
          </button>
          <button
            type="button"
            disabled={!ready}
            onClick={submit}
            className="motion-colors flex h-8 items-center rounded-lg bg-accent px-4 text-[13px] font-medium text-white hover:bg-[#0077ed] active:scale-[0.97] disabled:opacity-50"
          >
            创建项目
          </button>
        </div>
      </div>
    </div>
  );
}
