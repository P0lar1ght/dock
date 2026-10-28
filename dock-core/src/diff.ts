// 统一 diff（`git diff` / `gh pr diff` 的输出）→ 文件、块、行（带新旧行号），以及把 code-review
// 的批注（`workflow/updated` 的 `resultData`）落到具体行上。纯函数、无 DOM。

import type { WorkflowRun } from './events.ts';

export type DiffLineKind = 'add' | 'del' | 'same';

export interface DiffLine {
  kind: DiffLineKind;
  text: string;
  /** 旧版本里的行号（加的行没有）。 */
  oldNo: number | null;
  /** 新版本里的行号（删的行没有）。批注按它落。 */
  newNo: number | null;
}

export interface DiffHunk {
  /** `@@ -a,b +c,d @@ 上下文` 整行。 */
  header: string;
  lines: DiffLine[];
}

export type DiffFileStatus = 'modified' | 'added' | 'deleted' | 'renamed';

export interface DiffFile {
  /** 显示用的路径：新路径（删掉的文件用旧路径）。 */
  path: string;
  oldPath: string | null;
  newPath: string | null;
  status: DiffFileStatus;
  binary: boolean;
  hunks: DiffHunk[];
  additions: number;
  deletions: number;
}

const HUNK = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/;

/** `a/foo` / `b/foo` → `foo`；`/dev/null` → null。 */
function stripSide(raw: string): string | null {
  const p = raw.trim().replace(/^"(.*)"$/, '$1');
  if (p === '/dev/null') return null;
  return p.replace(/^[ab]\//, '');
}

export function parseUnifiedDiff(text: string): DiffFile[] {
  const files: DiffFile[] = [];
  let file: DiffFile | null = null;
  let hunk: DiffHunk | null = null;
  let oldNo = 0;
  let newNo = 0;
  const start = (oldPath: string | null, newPath: string | null) => {
    file = {
      path: newPath ?? oldPath ?? '',
      oldPath,
      newPath,
      status: 'modified',
      binary: false,
      hunks: [],
      additions: 0,
      deletions: 0,
    };
    files.push(file);
    hunk = null;
  };
  const rows = text.split('\n');
  // 以换行结尾时最后那个空串不是一行。
  if (rows.length && rows[rows.length - 1] === '') rows.pop();
  for (const line of rows) {
    if (line.startsWith('diff --git ')) {
      const m = line.match(/^diff --git (\S+) (\S+)$/);
      start(m ? stripSide(m[1]) : null, m ? stripSide(m[2]) : null);
      continue;
    }
    const f = file as DiffFile | null;
    if (!f) continue;
    if (!hunk) {
      if (line.startsWith('new file mode')) f.status = 'added';
      else if (line.startsWith('deleted file mode')) f.status = 'deleted';
      else if (line.startsWith('rename from ')) {
        f.status = 'renamed';
        f.oldPath = line.slice('rename from '.length);
      } else if (line.startsWith('rename to ')) {
        f.newPath = line.slice('rename to '.length);
        f.path = f.newPath;
      } else if (line.startsWith('Binary files ') || line.startsWith('GIT binary patch')) f.binary = true;
      else if (line.startsWith('--- ')) {
        f.oldPath = stripSide(line.slice(4));
        if (f.oldPath === null) f.status = 'added';
      } else if (line.startsWith('+++ ')) {
        f.newPath = stripSide(line.slice(4));
        if (f.newPath === null) f.status = 'deleted';
        f.path = f.newPath ?? f.oldPath ?? f.path;
      }
    }
    const h = line.match(HUNK);
    if (h) {
      oldNo = Number(h[1]);
      newNo = Number(h[3]);
      hunk = { header: line, lines: [] };
      f.hunks.push(hunk);
      continue;
    }
    const cur = hunk as DiffHunk | null;
    if (!cur) continue;
    if (line.startsWith('+')) {
      cur.lines.push({ kind: 'add', text: line.slice(1), oldNo: null, newNo: newNo++ });
      f.additions++;
    } else if (line.startsWith('-')) {
      cur.lines.push({ kind: 'del', text: line.slice(1), oldNo: oldNo++, newNo: null });
      f.deletions++;
    } else if (line.startsWith(' ') || line === '') {
      // 空行：块里的空上下文行有时会丢掉前导空格。
      cur.lines.push({ kind: 'same', text: line.slice(1), oldNo: oldNo++, newNo: newNo++ });
    }
    // `\ No newline at end of file` 之类的说明行不画。
  }
  return files;
}

// ---- code-review 的批注 ----

export type ReviewSeverity = 'critical' | 'high' | 'medium' | 'low';

export interface ReviewFinding {
  file: string;
  /** 新版本里的行号；0 = 不针对某一行。 */
  line: number;
  severity: ReviewSeverity;
  title: string;
  detail: string;
  suggestion: string;
  angles: string[];
  /** 核实员的一句话。 */
  note: string;
}

export interface CodeReview {
  diffCommand: string;
  /** 审的是 GitHub PR 时的编号，否则 0。 */
  pr: number;
  findings: ReviewFinding[];
}

const SEVERITIES: ReviewSeverity[] = ['critical', 'high', 'medium', 'low'];

/** 一次 code-review run 的结构化结果；不是 code-review、或还没结果，回 `null`。 */
export function codeReviewOf(run: WorkflowRun): CodeReview | null {
  return parseCodeReview(run.resultData);
}

export function parseCodeReview(data: unknown): CodeReview | null {
  if (!data || typeof data !== 'object') return null;
  const d = data as Record<string, unknown>;
  if (d.kind !== 'code-review' || typeof d.diff_command !== 'string') return null;
  const findings = (Array.isArray(d.findings) ? d.findings : []).flatMap((raw): ReviewFinding[] => {
    if (!raw || typeof raw !== 'object') return [];
    const f = raw as Record<string, unknown>;
    const file = typeof f.file === 'string' ? f.file.trim() : '';
    const title = typeof f.title === 'string' ? f.title : '';
    if (!file || !title) return [];
    const severity = SEVERITIES.includes(f.severity as ReviewSeverity) ? (f.severity as ReviewSeverity) : 'medium';
    return [
      {
        file,
        line: typeof f.line === 'number' && f.line > 0 ? Math.floor(f.line) : 0,
        severity,
        title,
        detail: typeof f.detail === 'string' ? f.detail : '',
        suggestion: typeof f.suggestion === 'string' ? f.suggestion : '',
        angles: Array.isArray(f.angles) ? f.angles.filter((a): a is string => typeof a === 'string') : [],
        note: typeof f.note === 'string' ? f.note : '',
      },
    ];
  });
  return { diffCommand: d.diff_command, pr: typeof d.pr === 'number' ? d.pr : 0, findings };
}

/** 批注落在差异视图的哪里。 */
export interface PlacedFindings {
  /** `文件下标:新行号` → 落在那一行下面的批注。 */
  byLine: Map<string, ReviewFinding[]>;
  /** 文件下标 → 找到了文件、但行不在 diff 里（或不针对某一行）的批注，挂在文件头下。 */
  byFile: Map<number, ReviewFinding[]>;
  /** diff 里找不到这个文件的批注。 */
  unplaced: ReviewFinding[];
}

export const lineKey = (fileIndex: number, newNo: number) => `${fileIndex}:${newNo}`;

/** 批注的文件对应 diff 里的哪个文件：先全等，再按路径后缀（审查员有时省掉前面的目录）。 */
function fileIndexOf(files: DiffFile[], path: string): number {
  const p = path.replace(/^\.\//, '');
  const exact = files.findIndex((f) => f.path === p || f.newPath === p || f.oldPath === p);
  if (exact >= 0) return exact;
  return files.findIndex((f) => f.path.endsWith(`/${p}`) || p.endsWith(`/${f.path}`));
}

export function placeFindings(files: DiffFile[], findings: readonly ReviewFinding[]): PlacedFindings {
  const byLine = new Map<string, ReviewFinding[]>();
  const byFile = new Map<number, ReviewFinding[]>();
  const unplaced: ReviewFinding[] = [];
  const push = <K>(map: Map<K, ReviewFinding[]>, key: K, f: ReviewFinding) => map.set(key, [...(map.get(key) ?? []), f]);
  for (const f of findings) {
    const fi = fileIndexOf(files, f.file);
    if (fi < 0) {
      unplaced.push(f);
      continue;
    }
    const onLine = f.line > 0 && files[fi].hunks.some((h) => h.lines.some((l) => l.newNo === f.line));
    if (onLine) push(byLine, lineKey(fi, f.line), f);
    else push(byFile, fi, f);
  }
  return { byLine, byFile, unplaced };
}
