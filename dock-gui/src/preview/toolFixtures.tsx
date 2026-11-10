// 预览 `#tool-fixtures-<名字>`：把实测录下的 thread/history 事件（fixtures/*.json）原样回放成对话流，
// 看工具卡在真输出上的样子，不用再叫模型。录法见 docs/TOOL-CARDS.md。

import { useEffect, useState } from "react";
import { replayHistory, type ThreadState } from "dock-core";
import { Transcript } from "../components/Transcript";

const FIXTURES = import.meta.glob<{ events: Record<string, unknown>[] }>("./fixtures/*.json", { import: "default" });

export const isToolFixturePreview = () => location.hash.startsWith("#tool-fixtures-");

export function ToolFixturePreview() {
  const name = location.hash.slice("#tool-fixtures-".length);
  const [thread, setThread] = useState<ThreadState | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    const load = FIXTURES[`./fixtures/${name}.json`];
    if (!load) return setError(`没有样本 ${name}（有：${Object.keys(FIXTURES).join(", ")}）`);
    void load().then((d) => setThread(replayHistory(d.events)));
  }, [name]);
  if (error) return <div className="p-6 text-sm text-[#c0392b]">{error}</div>;
  if (!thread) return null;
  return (
    <section data-preview="tool-fixtures" className="flex h-[3000px] w-[1000px] flex-col bg-pane">
      <Transcript thread={thread} sessionKey={`fixture-${name}`} />
    </section>
  );
}
