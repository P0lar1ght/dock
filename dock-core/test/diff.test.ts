import assert from 'node:assert/strict';
import { test } from 'node:test';

import { lineKey, parseCodeReview, parseUnifiedDiff, placeFindings } from '../src/index.ts';

const DIFF = `diff --git a/src/users.py b/src/users.py
index 1111111..2222222 100644
--- a/src/users.py
+++ b/src/users.py
@@ -1,4 +1,6 @@
 import sqlite3
+
+_cache = {}

 def get_user(conn, user_id):
-    cur = conn.execute("SELECT name FROM users WHERE id = ?", (user_id,))
+    cur = conn.execute("SELECT name FROM users WHERE id = " + str(user_id))
@@ -10,2 +12,2 @@ def page(items, page_no, size):
-    start = page_no * size
+    start = (page_no - 1) * size
     return items[start:start + size]
diff --git a/new.txt b/new.txt
new file mode 100644
index 0000000..3333333
--- /dev/null
+++ b/new.txt
@@ -0,0 +1 @@
+hello
\\ No newline at end of file
diff --git a/old.bin b/old.bin
deleted file mode 100644
Binary files a/old.bin and /dev/null differ
`;

test('统一 diff 拆成文件、块、带新旧行号的行', () => {
  const files = parseUnifiedDiff(DIFF);
  assert.deepEqual(
    files.map((f) => [f.path, f.status, f.additions, f.deletions, f.binary]),
    [
      ['src/users.py', 'modified', 4, 2, false],
      ['new.txt', 'added', 1, 0, false],
      ['old.bin', 'deleted', 0, 0, true],
    ],
  );
  const [first, second] = files[0].hunks;
  assert.equal(first.lines[0].kind, 'same');
  assert.deepEqual([first.lines[0].oldNo, first.lines[0].newNo], [1, 1]);
  // 空的上下文行（丢了前导空格）也算一行，行号接着数。
  const blank = first.lines.find((l) => l.kind === 'same' && l.text === '');
  assert.deepEqual([blank?.oldNo, blank?.newNo], [2, 4]);
  const added = first.lines.filter((l) => l.kind === 'add').map((l) => l.newNo);
  assert.deepEqual(added, [2, 3, 6]);
  assert.equal(second.header.startsWith('@@ -10,2 +12,2 @@'), true);
  assert.deepEqual(second.lines.map((l) => [l.kind, l.oldNo, l.newNo]), [
    ['del', 10, null],
    ['add', null, 12],
    ['same', 11, 13],
  ]);
  // 「No newline」说明行不算一行。
  assert.equal(files[1].hunks[0].lines.length, 1);
});

test('批注按新行号落到行上；行不在 diff 里挂文件头；文件不在 diff 里单列', () => {
  const files = parseUnifiedDiff(DIFF);
  const review = parseCodeReview({
    kind: 'code-review',
    diff_command: 'git diff HEAD',
    pr: 0,
    findings: [
      { file: 'src/users.py', line: 6, severity: 'critical', title: 'SQL 注入', detail: 'd', angles: ['安全'] },
      // 省掉了目录的路径按后缀认。
      { file: 'users.py', line: 12, severity: 'high', title: '页码改成 1 基', detail: 'd' },
      { file: 'src/users.py', line: 99, severity: 'low', title: '不在 diff 里的行', detail: 'd' },
      { file: 'src/users.py', line: 0, severity: 'low', title: '整个文件', detail: 'd' },
      { file: 'elsewhere.rs', line: 3, severity: 'medium', title: '别的文件', detail: 'd' },
      { file: '', line: 3, title: '坏的' },
    ],
  });
  assert.ok(review);
  assert.equal(review.findings.length, 5, '坏的那条丢掉');
  const placed = placeFindings(files, review.findings);
  assert.deepEqual(placed.byLine.get(lineKey(0, 6))?.map((f) => f.title), ['SQL 注入']);
  assert.deepEqual(placed.byLine.get(lineKey(0, 12))?.map((f) => f.title), ['页码改成 1 基']);
  assert.deepEqual(placed.byFile.get(0)?.map((f) => f.title), ['不在 diff 里的行', '整个文件']);
  assert.deepEqual(placed.unplaced.map((f) => f.title), ['别的文件']);
});

test('不是 code-review 的结果认不出', () => {
  assert.equal(parseCodeReview(null), null);
  assert.equal(parseCodeReview({ kind: 'probe' }), null);
});
