// ```mermaid 代码块画成图。Mermaid 很大，第一次用到才加载；渲染串行（它内部共用临时节点）。
// 流式时代码没写完、语法暂时不对：保留上一张画好的图；一张都没画出来就先显示源码。
// securityLevel strict：图里的 HTML 标签会被清掉，点击回调不生效。

import { useEffect, useState } from "react";
import { Code2, Workflow } from "lucide-react";
import { CodeBlock, HeaderButton, type CodeSize } from "./CodeBlock";

type MermaidApi = (typeof import("mermaid"))["default"];

let api: Promise<MermaidApi> | null = null;
let queue: Promise<unknown> = Promise.resolve();
let seq = 0;

function mermaid(): Promise<MermaidApi> {
  api ??= import("mermaid").then(({ default: m }) => {
    m.initialize({
      startOnLoad: false,
      securityLevel: "strict",
      theme: "neutral",
      // 默认 16px 比正文大，图显得笨重。
      themeVariables: { fontSize: "13px" },
      fontFamily: getComputedStyle(document.documentElement).getPropertyValue("--font-chat").trim() || undefined,
    });
    return m;
  });
  return api;
}

/** 画成 SVG；语法不对返回 `null`（不抛，流式时这很常见）。 */
function render(code: string): Promise<string | null> {
  const run = queue.then(async () => {
    const m = await mermaid();
    if (!(await m.parse(code, { suppressErrors: true }))) return null;
    const { svg } = await m.render(`dock-mermaid-${++seq}`, code);
    return svg;
  });
  queue = run.catch(() => {});
  return run;
}

export function Mermaid({ code, size }: { code: string; size: CodeSize }) {
  const [svg, setSvg] = useState<string | null>(null);
  const [broken, setBroken] = useState(false);
  const [source, setSource] = useState(false);

  useEffect(() => {
    let alive = true;
    // 流式时攒一下再画；语法错先不报，停下来 1.5 秒还错才提示。
    const timer = window.setTimeout(() => {
      render(code)
        .then((out) => {
          if (!alive) return;
          if (out) {
            setSvg(out);
            setBroken(false);
          } else {
            window.setTimeout(() => alive && setBroken(true), 1500);
          }
        })
        .catch((e) => {
          console.warn("Mermaid 渲染失败", e);
          if (alive) setBroken(true);
        });
    }, 250);
    return () => {
      alive = false;
      window.clearTimeout(timer);
    };
  }, [code]);

  const showDiagram = svg !== null && !source;
  return (
    <CodeBlock
      code={code}
      lang="mermaid"
      size={size}
      actions={
        <>
          {broken && <span className="mr-1 text-[#c0392b]">{svg ? "最新版本有语法错误，显示的是上一版" : "语法有误，没画出图"}</span>}
          {svg !== null && (
            <HeaderButton title={source ? "看图" : "看源码"} onClick={() => setSource((s) => !s)}>
              {source ? <Workflow size={13} /> : <Code2 size={13} />}
              <span>{source ? "图" : "源码"}</span>
            </HeaderButton>
          )}
        </>
      }
    >
      {showDiagram ? (
        <div
          className="md-mermaid md-surface-fill thin-scroll flex justify-center overflow-x-auto bg-white px-4 py-4"
          // Mermaid 的输出在 strict 模式下已经过 DOMPurify。
          dangerouslySetInnerHTML={{ __html: svg }}
        />
      ) : undefined}
    </CodeBlock>
  );
}
