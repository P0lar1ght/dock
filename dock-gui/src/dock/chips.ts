// 网关给带图的消息正文前面加了「[Image #1] [Image #2] 」，图单独在附件里，
// 正文里就别再显示这串标记了（没带图的消息不动：用户自己写的 [Image #1] 留着）。

/** 去掉正文开头的图片标记（`[Image #1] [Image #2] `）。 */
export function stripImageChips(text: string): string {
  return text.replace(/^(\[Image #\d+\]\s*)+/, "");
}
