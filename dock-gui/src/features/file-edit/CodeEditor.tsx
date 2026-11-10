// 文件面板里的文本编辑器（CodeMirror 6）：按文件名按需加载语言；⌘S 保存；保留 CRLF 换行。
// 只在点了「编辑」后挂上：看文件仍走面板原来的只读视图（Shiki 高亮）。

import { useEffect, useRef } from "react";
import { basicSetup, EditorView } from "codemirror";
import { Compartment, EditorState } from "@codemirror/state";
import { keymap } from "@codemirror/view";
import { indentWithTab } from "@codemirror/commands";
import { LanguageDescription } from "@codemirror/language";
import { languages } from "@codemirror/language-data";
import type { FileEditorProps } from "../registry";

const theme = EditorView.theme({
  "&": { height: "100%", fontSize: "12.5px", backgroundColor: "#ffffff" },
  "&.cm-focused": { outline: "none" },
  ".cm-scroller": { fontFamily: "var(--font-mono)", lineHeight: "21px" },
  ".cm-content": { padding: "12px 0", caretColor: "#0a84ff" },
  ".cm-gutters": { backgroundColor: "#ffffff", border: "none", color: "#c7c7cc" },
  ".cm-lineNumbers .cm-gutterElement": { padding: "0 12px 0 16px", fontSize: "12px" },
  ".cm-activeLine": { backgroundColor: "#0a84ff0a" },
  ".cm-activeLineGutter": { backgroundColor: "transparent", color: "#8e8e93" },
  ".cm-cursor": { borderLeftColor: "#0a84ff", borderLeftWidth: "2px" },
  "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, ::selection": { backgroundColor: "#0a84ff33" },
  ".cm-panels": { backgroundColor: "#f5f5f7", borderColor: "#e5e5ea" },
  ".cm-searchMatch": { backgroundColor: "#ffd60a55" },
});

export function CodeEditor({ path, initial, onChange, onSave, line }: FileEditorProps) {
  const host = useRef<HTMLDivElement>(null);
  const latest = useRef({ onChange, onSave });
  latest.current = { onChange, onSave };

  useEffect(() => {
    const language = new Compartment();
    const view = new EditorView({
      parent: host.current!,
      state: EditorState.create({
        doc: initial,
        extensions: [
          basicSetup,
          keymap.of([
            {
              key: "Mod-s",
              preventDefault: true,
              run: () => {
                latest.current.onSave();
                return true;
              },
            },
            indentWithTab,
          ]),
          language.of([]),
          // Windows 换行的文件存回去还是 CRLF。
          /\r\n/.test(initial) ? EditorState.lineSeparator.of("\r\n") : [],
          EditorView.updateListener.of((u) => {
            if (u.docChanged) latest.current.onChange(() => u.view.state.doc.toString());
          }),
          theme,
        ],
      }),
    });
    const name = path.slice(path.lastIndexOf("/") + 1);
    LanguageDescription.matchFilename(languages, name)
      ?.load()
      .then((support) => view.dispatch({ effects: language.reconfigure(support) }))
      .catch(() => {});
    if (line) {
      const at = view.state.doc.line(Math.min(Math.max(1, line), view.state.doc.lines));
      view.dispatch({ selection: { anchor: at.from }, effects: EditorView.scrollIntoView(at.from, { y: "center" }) });
    }
    view.focus();
    return () => view.destroy();
  }, [path, initial]);

  return <div ref={host} className="min-h-0 flex-1 overflow-hidden" />;
}
