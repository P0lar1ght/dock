// 文件面板的表格 / 文档预览：CSV、TSV、Excel、Word、PowerPoint、PDF。
// 预览器按需加载（../deferred.tsx）：解析库只在 Worker / 沙箱 iframe 里，启动路径上不带。

import { registerFilePreview } from "../registry";
import { deferred } from "../deferred";

const csv = deferred(() => import("./CsvViewer").then((m) => m.CsvViewer));
const xlsx = deferred(() => import("./XlsxViewer").then((m) => m.XlsxViewer));
const docx = deferred(() => import("./FrameViewer").then((m) => m.DocxViewer));
const pptx = deferred(() => import("./FrameViewer").then((m) => m.PptxViewer));
const pdf = deferred(() => import("./PdfViewer").then((m) => m.PdfViewer));

const ext = (name: string) => name.slice(name.lastIndexOf(".") + 1);

// 表格能在预览里改（xls / ods 由预览器自己说明改不了）。
registerFilePreview({ match: (n) => ["csv", "tsv"].includes(ext(n)), ...csv, binary: false, editable: true });
registerFilePreview({ match: (n) => ["xlsx", "xlsm", "xls", "ods"].includes(ext(n)), ...xlsx, binary: true, editable: true });
registerFilePreview({ match: (n) => ext(n) === "docx", ...docx, binary: true });
registerFilePreview({ match: (n) => ext(n) === "pptx", ...pptx, binary: true });
registerFilePreview({ match: (n) => ext(n) === "pdf", ...pdf, binary: true });
