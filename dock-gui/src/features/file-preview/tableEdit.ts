// 表格的改动：直接改 `rows`（表格显示的那份），记撤销 / 重做，记每格「上次保存时」的值。
//
// - 改过的格子（和上次保存时不一样的）标底色，也只有它们要写回去（xlsx 只改这些格）；
//   改了又撤回去的不算，不会被重写（免得数字、日期被写成文字）。
// - 插删行列只给 CSV：下标整体挪动，「上次保存时」的记录对不上了就清掉，只记「结构改过」。

type Op =
  | { kind: "cell"; ri: number; c: number; before: string; after: string; grewRow: boolean }
  | { kind: "insertRow"; ri: number; row: string[] }
  | { kind: "deleteRow"; ri: number; row: string[] }
  | { kind: "insertCol"; c: number; values: (string | undefined)[] }
  | { kind: "deleteCol"; c: number; values: (string | undefined)[] };

const key = (ri: number, c: number) => `${ri}:${c}`;

export class TableEdits {
  private done: Op[] = [];
  private undone: Op[] = [];
  /** 动过的格子在上次保存时的值。 */
  private baseline = new Map<string, string>();
  /** 插删过行列（还没保存）。 */
  private structural = false;

  constructor(
    private rows: string[][],
    /** 数据变了：重画、报「有没有改」。 */
    private onChange: () => void,
  ) {}

  get(ri: number, c: number): string {
    return this.rows[ri]?.[c] ?? "";
  }

  /** 改一格（`ri` 可以是末尾的下一行：新加一行）。 */
  setCell(ri: number, c: number, text: string) {
    const before = this.get(ri, c);
    if (before === text) return;
    const grewRow = ri >= this.rows.length;
    this.push({ kind: "cell", ri, c, before, after: text, grewRow });
  }

  insertRow(ri: number, width: number) {
    this.push({ kind: "insertRow", ri, row: new Array<string>(width).fill("") });
  }

  deleteRow(ri: number) {
    if (ri >= this.rows.length) return;
    this.push({ kind: "deleteRow", ri, row: this.rows[ri] });
  }

  insertCol(c: number) {
    this.push({ kind: "insertCol", c, values: this.rows.map(() => "") });
  }

  deleteCol(c: number) {
    this.push({ kind: "deleteCol", c, values: this.rows.map((r) => r[c]) });
  }

  undo() {
    const op = this.done.pop();
    if (!op) return;
    this.apply(op, true);
    this.undone.push(op);
    this.onChange();
  }

  redo() {
    const op = this.undone.pop();
    if (!op) return;
    this.apply(op, false);
    this.done.push(op);
    this.onChange();
  }

  /** 和上次保存时比有没有不一样。 */
  dirty(): boolean {
    if (this.structural) return true;
    for (const [k, v] of this.baseline) {
      const [ri, c] = k.split(":").map(Number);
      if (this.get(ri, c) !== v) return true;
    }
    return false;
  }

  /** 和上次保存时不一样的格子（标底色用）。 */
  changed(): Set<string> {
    const out = new Set<string>();
    for (const [k, v] of this.baseline) {
      const [ri, c] = k.split(":").map(Number);
      if (this.get(ri, c) !== v) out.add(k);
    }
    return out;
  }

  /** 要写回去的格子（xlsx 只改这些）。 */
  pending(): { ri: number; c: number; text: string }[] {
    return [...this.changed()].map((k) => {
      const [ri, c] = k.split(":").map(Number);
      return { ri, c, text: this.get(ri, c) };
    });
  }

  /** 存好了：现在的样子就是「上次保存时」。撤销历史留着（撤回去又算改动）。 */
  markSaved() {
    this.baseline.clear();
    this.structural = false;
    this.onChange();
  }

  private push(op: Op) {
    this.apply(op, false);
    this.done.push(op);
    this.undone = [];
    this.onChange();
  }

  /** 改一格的值，先记下它上次保存时是什么。 */
  private write(ri: number, c: number, text: string) {
    const k = key(ri, c);
    if (!this.baseline.has(k)) this.baseline.set(k, this.get(ri, c));
    // 新加的行和第一行（表头）一样宽：CSV 存出来每行列数对得上。
    while (this.rows.length <= ri) this.rows.push(new Array<string>(this.rows[0]?.length ?? 0).fill(""));
    const row = this.rows[ri];
    while (row.length <= c) row.push("");
    row[c] = text;
  }

  private apply(op: Op, reverse: boolean) {
    switch (op.kind) {
      case "cell":
        this.write(op.ri, op.c, reverse ? op.before : op.after);
        // 撤销「在末尾空行里填的那一格」：那一行又空了就去掉，存的时候不多一行。
        if (reverse && op.grewRow && op.ri === this.rows.length - 1 && this.rows[op.ri].every((v) => !v)) {
          this.rows.pop();
          this.baseline.delete(key(op.ri, op.c));
        }
        return;
      case "insertRow":
      case "deleteRow": {
        const insert = (op.kind === "insertRow") !== reverse;
        if (insert) this.rows.splice(op.ri, 0, op.row);
        else this.rows.splice(op.ri, 1);
        return this.shifted();
      }
      case "insertCol":
      case "deleteCol": {
        const insert = (op.kind === "insertCol") !== reverse;
        this.rows.forEach((row, i) => {
          if (insert) {
            if (row.length >= op.c) row.splice(op.c, 0, op.values[i] ?? "");
          } else if (row.length > op.c) row.splice(op.c, 1);
        });
        return this.shifted();
      }
    }
  }

  /** 插删了行列：下标都挪了，按格子记的「上次保存时」作废，改记「结构改过」。 */
  private shifted() {
    this.baseline.clear();
    this.structural = true;
  }
}
