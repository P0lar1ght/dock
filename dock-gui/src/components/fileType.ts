// 按文件名认类型：文件面板的树、快速打开、预览头栏用的图标和颜色，底栏的语言名，代码高亮的语言。
// 颜色取各语言社区常用色（GitHub linguist 一类），在浅色底上压暗了一档，图标 14px 也认得出。

import {
  Database,
  FileArchive,
  FileAudio,
  FileBraces,
  FileCode,
  FileCog,
  FileImage,
  FileLock,
  FileSpreadsheet,
  FileTerminal,
  FileText,
  FileVideo,
  Presentation,
  type LucideIcon,
} from "lucide-react";

export interface FileType {
  /** 底栏显示的名字。 */
  label: string;
  /** Shiki 的语言 id；不高亮的是 `text`。 */
  lang: string;
  icon: LucideIcon;
  color: string;
}

const T = (label: string, lang: string, icon: LucideIcon, color: string): FileType => ({ label, lang, icon, color });

const GRAY = "#8e8e93";

const BY_EXT: Record<string, FileType> = {
  rs: T("Rust", "rust", FileCode, "#c4622d"),
  ts: T("TypeScript", "typescript", FileCode, "#3178c6"),
  mts: T("TypeScript", "typescript", FileCode, "#3178c6"),
  cts: T("TypeScript", "typescript", FileCode, "#3178c6"),
  tsx: T("TypeScript React", "tsx", FileCode, "#3178c6"),
  js: T("JavaScript", "javascript", FileCode, "#b8960b"),
  mjs: T("JavaScript", "javascript", FileCode, "#b8960b"),
  cjs: T("JavaScript", "javascript", FileCode, "#b8960b"),
  jsx: T("JavaScript React", "jsx", FileCode, "#b8960b"),
  py: T("Python", "python", FileCode, "#3572a5"),
  ipynb: T("Jupyter", "json", FileCode, "#da5b0b"),
  go: T("Go", "go", FileCode, "#00a7cf"),
  java: T("Java", "java", FileCode, "#b07219"),
  kt: T("Kotlin", "kotlin", FileCode, "#7f52ff"),
  kts: T("Kotlin", "kotlin", FileCode, "#7f52ff"),
  swift: T("Swift", "swift", FileCode, "#f05138"),
  c: T("C", "c", FileCode, "#555555"),
  h: T("C 头文件", "c", FileCode, "#555555"),
  cc: T("C++", "cpp", FileCode, "#f34b7d"),
  cpp: T("C++", "cpp", FileCode, "#f34b7d"),
  hpp: T("C++ 头文件", "cpp", FileCode, "#f34b7d"),
  m: T("Objective-C", "objective-c", FileCode, "#438eff"),
  mm: T("Objective-C++", "objective-cpp", FileCode, "#438eff"),
  cs: T("C#", "csharp", FileCode, "#178600"),
  rb: T("Ruby", "ruby", FileCode, "#cc342d"),
  php: T("PHP", "php", FileCode, "#4f5d95"),
  lua: T("Lua", "lua", FileCode, "#000080"),
  dart: T("Dart", "dart", FileCode, "#00a4a0"),
  zig: T("Zig", "zig", FileCode, "#d9822b"),
  ex: T("Elixir", "elixir", FileCode, "#6e4a7e"),
  exs: T("Elixir", "elixir", FileCode, "#6e4a7e"),
  r: T("R", "r", FileCode, "#198ce7"),
  vue: T("Vue", "vue", FileCode, "#3fa877"),
  svelte: T("Svelte", "svelte", FileCode, "#ff3e00"),
  html: T("HTML", "html", FileCode, "#e34c26"),
  htm: T("HTML", "html", FileCode, "#e34c26"),
  css: T("CSS", "css", FileCode, "#663399"),
  scss: T("SCSS", "scss", FileCode, "#c6538c"),
  less: T("Less", "less", FileCode, "#1d365d"),
  graphql: T("GraphQL", "graphql", FileCode, "#e10098"),
  proto: T("Protobuf", "proto", FileCode, "#4a8bc2"),
  sql: T("SQL", "sql", Database, "#d97a00"),
  sh: T("Shell", "shellscript", FileTerminal, "#2b9a4a"),
  bash: T("Shell", "shellscript", FileTerminal, "#2b9a4a"),
  zsh: T("Shell", "shellscript", FileTerminal, "#2b9a4a"),
  fish: T("Fish", "fish", FileTerminal, "#2b9a4a"),
  ps1: T("PowerShell", "powershell", FileTerminal, "#2b5797"),
  md: T("Markdown", "markdown", FileText, "#3a7bbf"),
  mdx: T("MDX", "mdx", FileText, "#3a7bbf"),
  txt: T("文本", "text", FileText, GRAY),
  log: T("日志", "log", FileText, GRAY),
  json: T("JSON", "json", FileBraces, "#c08b1d"),
  jsonc: T("JSON", "jsonc", FileBraces, "#c08b1d"),
  jsonl: T("JSON Lines", "jsonl", FileBraces, "#c08b1d"),
  toml: T("TOML", "toml", FileCog, "#9c4221"),
  yaml: T("YAML", "yaml", FileCog, "#cb171e"),
  yml: T("YAML", "yaml", FileCog, "#cb171e"),
  ini: T("INI", "ini", FileCog, "#6d6d72"),
  cfg: T("配置", "ini", FileCog, "#6d6d72"),
  conf: T("配置", "ini", FileCog, "#6d6d72"),
  env: T("环境变量", "dotenv", FileCog, "#6d6d72"),
  xml: T("XML", "xml", FileCode, "#0060ac"),
  plist: T("Plist", "xml", FileCog, "#6d6d72"),
  gradle: T("Gradle", "groovy", FileCog, "#02303a"),
  lock: T("锁文件", "toml", FileLock, GRAY),
  csv: T("CSV", "csv", FileSpreadsheet, "#217346"),
  tsv: T("TSV", "tsv", FileSpreadsheet, "#217346"),
  xlsx: T("Excel", "text", FileSpreadsheet, "#217346"),
  xlsm: T("Excel", "text", FileSpreadsheet, "#217346"),
  xls: T("Excel", "text", FileSpreadsheet, "#217346"),
  ods: T("表格", "text", FileSpreadsheet, "#217346"),
  docx: T("Word", "text", FileText, "#2b579a"),
  doc: T("Word", "text", FileText, "#2b579a"),
  svg: T("SVG", "xml", FileImage, "#e6a23c"),
  png: T("PNG 图片", "text", FileImage, "#a259ff"),
  jpg: T("JPEG 图片", "text", FileImage, "#a259ff"),
  jpeg: T("JPEG 图片", "text", FileImage, "#a259ff"),
  gif: T("GIF 图片", "text", FileImage, "#a259ff"),
  webp: T("WebP 图片", "text", FileImage, "#a259ff"),
  ico: T("图标", "text", FileImage, "#a259ff"),
  pdf: T("PDF", "text", FileText, "#d93025"),
  pptx: T("PowerPoint", "text", Presentation, "#c43e1c"),
  ppt: T("PowerPoint", "text", Presentation, "#c43e1c"),
  zip: T("压缩包", "text", FileArchive, GRAY),
  gz: T("压缩包", "text", FileArchive, GRAY),
  tar: T("压缩包", "text", FileArchive, GRAY),
  mp4: T("视频", "text", FileVideo, "#e0457b"),
  mov: T("视频", "text", FileVideo, "#e0457b"),
  mp3: T("音频", "text", FileAudio, "#e0457b"),
  wav: T("音频", "text", FileAudio, "#e0457b"),
};

const BY_NAME: Record<string, FileType> = {
  dockerfile: T("Dockerfile", "docker", FileCode, "#2496ed"),
  makefile: T("Makefile", "make", FileCog, "#427819"),
  justfile: T("Justfile", "just", FileCog, "#427819"),
  "cargo.lock": T("Cargo 锁文件", "toml", FileLock, GRAY),
  "package-lock.json": T("npm 锁文件", "json", FileLock, GRAY),
  "pnpm-lock.yaml": T("pnpm 锁文件", "yaml", FileLock, GRAY),
  ".gitignore": T("Git 忽略规则", "text", FileCog, "#f05032"),
  ".gitattributes": T("Git 属性", "text", FileCog, "#f05032"),
  ".env": T("环境变量", "dotenv", FileCog, "#6d6d72"),
  license: T("许可证", "text", FileText, GRAY),
};

const PLAIN = T("文本", "text", FileText, GRAY);

export function fileType(name: string): FileType {
  const lower = name.toLowerCase();
  const byName = BY_NAME[lower] ?? (lower.startsWith(".env") ? BY_NAME[".env"] : undefined);
  if (byName) return byName;
  const dot = lower.lastIndexOf(".");
  return (dot > 0 ? BY_EXT[lower.slice(dot + 1)] : undefined) ?? PLAIN;
}

/** 文件夹的颜色（访达那种蓝）。 */
export const FOLDER_COLOR = "#3b8eea";
