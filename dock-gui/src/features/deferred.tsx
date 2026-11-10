// 按需加载的组件（预览器、编辑器）：不用 React.lazy + Suspense——React 19 显示 Suspense 内容
// 时会节流约 300 ms，一个 5 KB 的块也要等那么久。这里块一到就画；宿主可以先 `preload`，
// 和别的请求（读元数据）并行。

import { useEffect, useState, type ComponentType } from "react";

export function deferred<P extends object>(load: () => Promise<ComponentType<P>>) {
  let loaded: ComponentType<P> | null = null;
  let pending: Promise<ComponentType<P>> | null = null;
  const preload = () =>
    (pending ??= load().then(
      (c) => (loaded = c),
      (e) => {
        pending = null; // 下次再试（比如断网后恢复）
        throw e;
      },
    ));

  function View(props: P) {
    const [Loaded, setLoaded] = useState(() => loaded);
    useEffect(() => {
      if (!Loaded) preload().then((c) => setLoaded(() => c), () => {});
    }, [Loaded]);
    return Loaded ? <Loaded {...props} /> : <div className="flex-1 bg-pane" />;
  }
  return {
    Viewer: View,
    preload: () => {
      preload().catch(() => {});
    },
  };
}
