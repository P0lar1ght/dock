// 文件面板的文本编辑器（CodeMirror）：代码第一次要用（或预加载）时才取。
import { deferred } from "../deferred";
import { registerFileEditor } from "../registry";

const editor = deferred(() => import("./CodeEditor").then((m) => m.CodeEditor));

registerFileEditor({ Editor: editor.Viewer, preload: editor.preload });
