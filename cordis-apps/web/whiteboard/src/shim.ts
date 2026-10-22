// 沙箱 iframe（只有 allow-scripts，不同源）里碰 localStorage / sessionStorage 会抛 SecurityError，
// Excalidraw 启动时要读它们。换成只在内存里的一份：界面状态本来就存在服务器上。
// 必须在 Excalidraw 之前执行：main.tsx 第一行引这个文件。

function memoryStorage(): Storage {
  const data = new Map<string, string>();
  return {
    get length() {
      return data.size;
    },
    clear: () => data.clear(),
    getItem: (k) => data.get(String(k)) ?? null,
    key: (i) => [...data.keys()][i] ?? null,
    removeItem: (k) => void data.delete(String(k)),
    setItem: (k, v) => void data.set(String(k), String(v)),
  };
}

for (const name of ["localStorage", "sessionStorage"] as const) {
  try {
    void window[name].length;
  } catch {
    Object.defineProperty(window, name, { value: memoryStorage(), configurable: true });
  }
}
