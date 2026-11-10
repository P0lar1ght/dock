// 连远程 Dock 时的「选择项目目录」：系统对话框只看得到本机，这里经 `fs/dirs` 逐层看远端的目录。
// 顶上的路径框可以直接改（回车跳过去）；远端不让列目录（旧 Dock / 没有权限）时只剩手填。

import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { ChevronLeft, Folder, Home, LoaderCircle } from "lucide-react";
import { dock } from "../dock/client";
import { errorText } from "../dock/settingsApi";
import { imeComposing } from "../dock/ime";
import { Modal, primaryButton, secondaryButton } from "./settings/ui";

interface Listing {
  path: string;
  parent: string | null;
  home: string | null;
  entries: { name: string; path: string }[];
  truncated: boolean;
}

/** 弹框选远端的一个目录：选了回绝对路径，取消回 `null`。 */
export function pickRemoteDirectory(from?: string): Promise<string | null> {
  return new Promise((resolve) => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    const root = createRoot(host);
    const done = (dir: string | null) => {
      root.unmount();
      host.remove();
      resolve(dir);
    };
    root.render(<RemoteDirectoryPicker from={from} onDone={done} />);
  });
}

function RemoteDirectoryPicker({ from, onDone }: { from?: string; onDone: (dir: string | null) => void }) {
  const [listing, setListing] = useState<Listing | null>(null);
  const [typed, setTyped] = useState(from?.startsWith("/") ? from : "");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const go = async (path: string) => {
    setLoading(true);
    try {
      const next = await dock.call<Listing>("fs/dirs", { path });
      setListing(next);
      setTyped(next.path);
      setError(null);
    } catch (e) {
      setError(errorText(e));
    }
    setLoading(false);
  };

  useEffect(() => void go(from?.startsWith("/") ? from : ""), []); // eslint-disable-line react-hooks/exhaustive-deps

  const chosen = typed.trim();
  return (
    <Modal onClose={() => onDone(null)}>
      <span className="text-base font-semibold text-ink">选择远端的项目目录</span>
      <div className="flex items-center gap-1.5">
        <button
          type="button"
          title="上一级"
          disabled={!listing?.parent || loading}
          onClick={() => listing?.parent && go(listing.parent)}
          className="motion-colors flex size-8 shrink-0 items-center justify-center rounded-lg border border-line-strong text-ink-3 hover:bg-hover disabled:opacity-40"
        >
          <ChevronLeft size={15} />
        </button>
        <button
          type="button"
          title="主目录"
          disabled={loading}
          onClick={() => go("")}
          className="motion-colors flex size-8 shrink-0 items-center justify-center rounded-lg border border-line-strong text-ink-3 hover:bg-hover disabled:opacity-40"
        >
          <Home size={14} />
        </button>
        <input
          value={typed}
          spellCheck={false}
          placeholder="/home/you/project"
          onChange={(e) => setTyped(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && !imeComposing(e) && chosen && go(chosen)}
          className="motion-colors h-8 min-w-0 flex-1 rounded-lg border border-line-strong bg-white px-2.5 font-mono text-xs text-ink shadow-sm outline-none focus:border-accent"
        />
      </div>
      <div className="flex h-[280px] flex-col overflow-y-auto rounded-lg border border-line-soft bg-window py-1">
        {loading && !listing && (
          <span className="flex items-center gap-2 px-3 py-2 text-xs text-ink-4">
            <LoaderCircle size={13} className="animate-spin" />
            正在读取…
          </span>
        )}
        {listing?.entries.map((d) => (
          <button
            key={d.path}
            type="button"
            onClick={() => go(d.path)}
            className="motion-colors flex h-8 shrink-0 items-center gap-2 px-3 text-left text-[12.5px] text-ink hover:bg-hover"
          >
            <Folder size={14} className="shrink-0 text-ink-4" />
            <span className="truncate">{d.name}</span>
          </button>
        ))}
        {listing && listing.entries.length === 0 && <span className="px-3 py-2 text-xs text-ink-4">没有子目录</span>}
        {listing?.truncated && <span className="px-3 py-2 text-xs text-ink-4">目录太多，只列了前 1000 个</span>}
      </div>
      {error && <span className="text-xs text-[#ff3b30]">{error}（可以直接在上面填路径）</span>}
      <div className="flex justify-end gap-2">
        <button type="button" onClick={() => onDone(null)} className={secondaryButton}>
          取消
        </button>
        <button type="button" disabled={!chosen.startsWith("/")} onClick={() => onDone(chosen)} className={primaryButton}>
          选这个目录
        </button>
      </div>
    </Modal>
  );
}
