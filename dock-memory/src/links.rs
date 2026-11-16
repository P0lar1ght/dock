//! `[[wikilink]]` 解析与改写（Obsidian 语法）。
//!
//! 记忆笔记之间的显式关系只有这一种写法：`[[笔记名]]`、`[[笔记名|显示文字]]`、
//! `[[笔记名#小标题]]`。`笔记名` 是文件名去掉 `.md`，也可以带路径
//! （`topics/foo`、`global/topics/foo`）；怎么对到具体文件由 [`crate::graph`] 决定，
//! 这里只管文本。
//!
//! 围栏代码块和行内代码里的 `[[...]]` 不算链接：笔记里常贴 Rust / shell 片段
//! （`[[bin]]`、`[[ -f x ]]`），认成链接会在图上凭空多出一堆节点。

use std::ops::Range;

/// 一条 `[[...]]`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WikiLink {
    /// 整个 `[[...]]` 在原文里的字节区间（嵌入写法 `![[...]]` 不含前面的 `!`）。
    pub span: Range<usize>,
    /// 目标原样（去掉 `#小标题` 与 `|显示文字`，两端空白已去）。
    pub target: String,
    pub heading: Option<String>,
    pub alias: Option<String>,
    /// 所在行，从 1 起。
    pub line: usize,
}

impl WikiLink {
    /// 断开链接后留在原处的文字：有显示文字用显示文字，否则用目标名。
    pub fn display_text(&self) -> &str {
        self.alias.as_deref().unwrap_or(&self.target)
    }
}

/// 目标名里不允许的字符：出现了就不当链接（也免得写出解析不回来的链接）。
const FORBIDDEN_IN_TARGET: &[char] = &['[', ']', '|', '#', '^', '\n', '\r'];

/// 最长的目标名（字节）。再长的多半是贴进来的别的东西。
const MAX_TARGET_BYTES: usize = 200;

/// 找出正文里所有 `[[...]]`，按出现顺序。
pub fn parse_wikilinks(text: &str) -> Vec<WikiLink> {
    let mut out = Vec::new();
    let mut fence: Option<(u8, usize)> = None;
    let mut offset = 0usize;
    for (index, line) in text.split_inclusive('\n').enumerate() {
        let start = offset;
        offset += line.len();
        let body = line.trim_end_matches(['\n', '\r']);
        if let Some(marker) = fence_marker(body) {
            match fence {
                // 收尾的围栏：同一种符号、不短于开头、后面没有别的字。
                Some((ch, len)) if marker.0 == ch && marker.1 >= len && marker.2 => fence = None,
                Some(_) => {}
                None => fence = Some((marker.0, marker.1)),
            }
            continue;
        }
        if fence.is_some() {
            continue;
        }
        scan_line(body, start, index + 1, &mut out);
    }
    out
}

/// 一行是不是围栏：`(符号, 长度, 后面是否为空)`。
fn fence_marker(line: &str) -> Option<(u8, usize, bool)> {
    let trimmed = line.trim_start_matches(' ');
    // 缩进 4 格以上是缩进代码块里的字，不是围栏。
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let ch = *trimmed.as_bytes().first()?;
    if ch != b'`' && ch != b'~' {
        return None;
    }
    let len = trimmed.bytes().take_while(|b| *b == ch).count();
    if len < 3 {
        return None;
    }
    let rest = trimmed.get(len..).unwrap_or("");
    // 反引号围栏的说明里不能再有反引号（那是行内代码）。
    if ch == b'`' && rest.contains('`') {
        return None;
    }
    Some((ch, len, rest.trim().is_empty()))
}

fn scan_line(line: &str, line_start: usize, line_no: usize, out: &mut Vec<WikiLink>) {
    let bytes = line.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes.get(i) {
            Some(b'`') => {
                let run = bytes
                    .get(i..)
                    .unwrap_or(&[])
                    .iter()
                    .take_while(|b| **b == b'`')
                    .count();
                // 行内代码：找到同样长度的收尾才算，找不到就是普通字符。
                i = match closing_backticks(bytes, i + run, run) {
                    Some(end) => end,
                    None => i + run,
                };
            }
            Some(b'[') if bytes.get(i + 1) == Some(&b'[') => {
                let inner_start = i + 2;
                let Some(close) = line.get(inner_start..).and_then(|rest| rest.find("]]")) else {
                    i += 2;
                    continue;
                };
                let inner_end = inner_start + close;
                let inner = line.get(inner_start..inner_end).unwrap_or("");
                match parse_inner(inner) {
                    Some((target, heading, alias)) => {
                        out.push(WikiLink {
                            span: line_start + i..line_start + inner_end + 2,
                            target,
                            heading,
                            alias,
                            line: line_no,
                        });
                        i = inner_end + 2;
                    }
                    // `[[` 后面不是链接（如 `[[[x]]`）：只跳过一个 `[` 再试。
                    None => i += 1,
                }
            }
            Some(_) => i += 1,
            None => break,
        }
    }
}

/// `from` 之后第一段长度恰好为 `run` 的反引号，返回它结束的位置。
fn closing_backticks(bytes: &[u8], from: usize, run: usize) -> Option<usize> {
    let mut i = from;
    while i < bytes.len() {
        if bytes.get(i) == Some(&b'`') {
            let len = bytes
                .get(i..)
                .unwrap_or(&[])
                .iter()
                .take_while(|b| **b == b'`')
                .count();
            if len == run {
                return Some(i + len);
            }
            i += len;
        } else {
            i += 1;
        }
    }
    None
}

fn parse_inner(inner: &str) -> Option<(String, Option<String>, Option<String>)> {
    let (left, alias) = match inner.split_once('|') {
        Some((left, alias)) => (left, Some(alias.trim()).filter(|a| !a.is_empty())),
        None => (inner, None),
    };
    let (target, heading) = match left.split_once('#') {
        Some((target, heading)) => (target, Some(heading.trim()).filter(|h| !h.is_empty())),
        None => (left, None),
    };
    let target = target.trim();
    if target.is_empty()
        || target.len() > MAX_TARGET_BYTES
        || target.contains(FORBIDDEN_IN_TARGET)
        || alias.is_some_and(|a| a.contains(['[', ']']))
        || heading.is_some_and(|h| h.contains(['[', ']']))
    {
        return None;
    }
    Some((
        target.to_string(),
        heading.map(str::to_string),
        alias.map(str::to_string),
    ))
}

/// 目标名能不能写进 `[[...]]`（不含会破坏语法的字符）。
pub fn is_linkable_name(name: &str) -> bool {
    let name = name.trim();
    !name.is_empty() && name.len() <= MAX_TARGET_BYTES && !name.contains(FORBIDDEN_IN_TARGET)
}

/// 拼一条链接：`[[target#heading|alias]]`。
pub fn format_wikilink(target: &str, heading: Option<&str>, alias: Option<&str>) -> String {
    let mut out = String::with_capacity(target.len() + 4);
    out.push_str("[[");
    out.push_str(target);
    if let Some(heading) = heading {
        out.push('#');
        out.push_str(heading);
    }
    if let Some(alias) = alias {
        out.push('|');
        out.push_str(alias);
    }
    out.push_str("]]");
    out
}

/// 逐条改写目标名：`retarget` 回 `Some(新目标)` 的换掉（小标题、显示文字照留），
/// 回 `None` 的不动。返回新正文和改了几条。
pub fn retarget_wikilinks(
    text: &str,
    mut retarget: impl FnMut(&WikiLink) -> Option<String>,
) -> (String, usize) {
    let links = parse_wikilinks(text);
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    let mut changed = 0usize;
    for link in &links {
        let Some(target) = retarget(link) else {
            continue;
        };
        if target == link.target {
            continue;
        }
        out.push_str(text.get(cursor..link.span.start).unwrap_or(""));
        out.push_str(&format_wikilink(
            &target,
            link.heading.as_deref(),
            link.alias.as_deref(),
        ));
        cursor = link.span.end;
        changed += 1;
    }
    out.push_str(text.get(cursor..).unwrap_or(""));
    (out, changed)
}

/// 界面里「建立链接」写进笔记的那一节的标题。
pub const RELATED_HEADING: &str = "## 相关";

/// 认作「相关」一节的标题文字（小写比较）。
const RELATED_TITLES: &[&str] = &["相关", "相关笔记", "related", "related notes", "see also"];

/// 在「相关」一节里加一条 `- [[target]]`；没有这一节就在文末新开。
/// 调用方先自己判断是不是已经链接过（那要看解析结果，这里只管写）。
pub fn append_related_link(text: &str, target: &str) -> String {
    let item = format!("- {}", format_wikilink(target, None, None));
    let lines: Vec<&str> = text.lines().collect();
    let Some(heading_at) = lines.iter().rposition(|l| is_related_heading(l)) else {
        let mut out = text.trim_end().to_string();
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(RELATED_HEADING);
        out.push_str("\n\n");
        out.push_str(&item);
        out.push('\n');
        return out;
    };
    // 这一节到下一个标题（或文末）为止；插在最后一行有字的后面。
    let section_end = lines
        .iter()
        .enumerate()
        .skip(heading_at + 1)
        .find(|(_, l)| heading_level(l).is_some())
        .map(|(i, _)| i)
        .unwrap_or(lines.len());
    let last_text = (heading_at..section_end)
        .rev()
        .find(|i| lines.get(*i).is_some_and(|l| !l.trim().is_empty()))
        .unwrap_or(heading_at);
    let mut out = String::with_capacity(text.len() + item.len() + 2);
    for (i, line) in lines.iter().enumerate() {
        out.push_str(line);
        out.push('\n');
        if i == last_text {
            // 标题下面直接是条目的话，中间留一个空行。
            if last_text == heading_at {
                out.push('\n');
            }
            out.push_str(&item);
            out.push('\n');
        }
    }
    out
}

fn heading_level(line: &str) -> Option<usize> {
    let trimmed = line.trim_start();
    let level = trimmed.bytes().take_while(|b| *b == b'#').count();
    if level == 0 || level > 6 {
        return None;
    }
    trimmed
        .get(level..)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with([' ', '\t']))
        .then_some(level)
}

fn is_related_heading(line: &str) -> bool {
    let Some(level) = heading_level(line) else {
        return false;
    };
    let title = line
        .trim_start()
        .get(level..)
        .unwrap_or("")
        .trim()
        .trim_end_matches('#')
        .trim()
        .to_lowercase();
    RELATED_TITLES.contains(&title.as_str())
}

/// 断开链接：`should_remove` 回 `true` 的那些。
///
/// - 独占一个列表项的（`- [[x]]`，「相关」一节里就是这样）整行删掉；
/// - 夹在句子里的只去掉方括号，留下显示文字，句子不断；
/// - 「相关」一节删空了连标题一起去掉。
///
/// 返回新正文和断开了几条。
pub fn remove_wikilinks(
    text: &str,
    mut should_remove: impl FnMut(&WikiLink) -> bool,
) -> (String, usize) {
    let links = parse_wikilinks(text);
    let doomed: Vec<&WikiLink> = links.iter().filter(|l| should_remove(l)).collect();
    if doomed.is_empty() {
        return (text.to_string(), 0);
    }
    let mut out = String::with_capacity(text.len());
    let mut offset = 0usize;
    for line in text.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        let here: Vec<&&WikiLink> = doomed
            .iter()
            .filter(|l| l.span.start >= start && l.span.end <= offset)
            .collect();
        if here.is_empty() {
            out.push_str(line);
            continue;
        }
        if here.len() == 1 && is_bare_list_item(line, start, here.first().map(|l| &l.span)) {
            continue;
        }
        let mut cursor = start;
        for link in here {
            let before = text.get(cursor..link.span.start).unwrap_or("");
            // 嵌入写法 `![[x]]`：前面的 `!` 一起去掉，不留一个孤零零的叹号。
            out.push_str(before.strip_suffix('!').unwrap_or(before));
            out.push_str(link.display_text());
            cursor = link.span.end;
        }
        out.push_str(text.get(cursor..offset).unwrap_or(""));
    }
    (drop_empty_related_section(&out), doomed.len())
}

/// 这一行除了列表符号和这条链接以外没有别的字。
fn is_bare_list_item(line: &str, line_start: usize, span: Option<&Range<usize>>) -> bool {
    let Some(span) = span else {
        return false;
    };
    // 列表符号和链接之间的空白不算字；`- ![[x]]`（嵌入）也算独占一项。
    let before: String = line
        .get(..span.start.saturating_sub(line_start))
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let after = line
        .get(span.end.saturating_sub(line_start)..)
        .unwrap_or("")
        .trim();
    after.is_empty() && matches!(before.as_str(), "-" | "*" | "+" | "-!" | "*!" | "+!")
}

fn drop_empty_related_section(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut keep = vec![true; lines.len()];
    for (i, line) in lines.iter().enumerate() {
        if !is_related_heading(line) {
            continue;
        }
        let end = lines
            .iter()
            .enumerate()
            .skip(i + 1)
            .find(|(_, l)| heading_level(l).is_some())
            .map(|(j, _)| j)
            .unwrap_or(lines.len());
        let empty = lines
            .get(i + 1..end)
            .unwrap_or(&[])
            .iter()
            .all(|l| l.trim().is_empty());
        if empty {
            for slot in keep.iter_mut().take(end).skip(i) {
                *slot = false;
            }
        }
    }
    if keep.iter().all(|k| *k) {
        return text.to_string();
    }
    let mut out = lines
        .iter()
        .zip(&keep)
        .filter(|(_, keep)| **keep)
        .map(|(line, _)| *line)
        .collect::<Vec<_>>()
        .join("\n");
    let trimmed = out.trim_end().len();
    out.truncate(trimmed);
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn targets(text: &str) -> Vec<String> {
        parse_wikilinks(text)
            .into_iter()
            .map(|l| l.target)
            .collect()
    }

    #[test]
    fn parses_plain_alias_and_heading_forms() {
        let text =
            "见 [[prefs]]、[[topics/auth|登录方案]] 和 [[db#索引|库]]。\n第二行 ![[diagram]]\n";
        let links = parse_wikilinks(text);
        let [prefs, auth, db, diagram] = links.as_slice() else {
            panic!("应该正好四条：{links:?}");
        };
        assert_eq!(
            [&prefs.target, &auth.target, &db.target, &diagram.target],
            ["prefs", "topics/auth", "db", "diagram"]
        );
        assert_eq!(auth.alias.as_deref(), Some("登录方案"));
        assert_eq!(db.heading.as_deref(), Some("索引"));
        assert_eq!(db.alias.as_deref(), Some("库"));
        assert_eq!((prefs.line, diagram.line), (1, 2));
        // 区间正好是 `[[...]]` 本身（嵌入写法不含 `!`）。
        assert_eq!(text.get(prefs.span.clone()), Some("[[prefs]]"));
        assert_eq!(text.get(diagram.span.clone()), Some("[[diagram]]"));
    }

    #[test]
    fn code_is_not_scanned_for_links() {
        let text = "\
正文 [[real]]
```toml
[[bin]]
name = \"dock\"
```
行内 `[[ -f x ]]` 和 ``a `[[b]]` c`` 不算，[[also-real]] 算。
~~~
[[tilde-fenced]]
~~~
    [[indented-code-is-still-text]]
";
        assert_eq!(
            targets(text),
            vec!["real", "also-real", "indented-code-is-still-text"]
        );
    }

    #[test]
    fn a_fence_only_closes_on_a_matching_marker() {
        // 四个反引号开的围栏里出现三个反引号不算收尾。
        let text = "````\n```\n[[inside]]\n```\n````\n[[outside]]\n";
        assert_eq!(targets(text), vec!["outside"]);
        // 没收尾的围栏一直管到文末。
        assert!(targets("```\n[[never]]\n").is_empty());
    }

    #[test]
    fn malformed_brackets_are_ignored() {
        assert!(targets("[[]] [[ ]] [[#只有标题]] [[a\nb]]").is_empty());
        assert!(targets("没有收尾 [[open").is_empty());
        // 多一层方括号：里面那对仍然是链接。
        assert_eq!(targets("[[[note]]]"), vec!["note"]);
        // 落单的反引号不是行内代码。
        assert_eq!(targets("it`s [[fine]]"), vec!["fine"]);
    }

    #[test]
    fn retarget_keeps_heading_and_alias() {
        let text = "a [[old]] b [[old#节|叫法]] c [[other]] `[[old]]`\n";
        let (out, changed) = retarget_wikilinks(text, |l| {
            (l.target == "old").then(|| "new-name".to_string())
        });
        assert_eq!(changed, 2);
        assert_eq!(
            out,
            "a [[new-name]] b [[new-name#节|叫法]] c [[other]] `[[old]]`\n"
        );
        // 没有要改的：原样、计数 0。
        let (same, none) = retarget_wikilinks(text, |_| None);
        assert_eq!((same.as_str(), none), (text, 0));
    }

    #[test]
    fn append_creates_then_extends_the_related_section() {
        let first = append_related_link("# 标题\n\n正文\n", "auth");
        assert_eq!(first, "# 标题\n\n正文\n\n## 相关\n\n- [[auth]]\n");
        let second = append_related_link(&first, "db");
        assert_eq!(
            second,
            "# 标题\n\n正文\n\n## 相关\n\n- [[auth]]\n- [[db]]\n"
        );
        // 「相关」不在文末：插在这一节里，不跑到后面那节去。
        let middle = append_related_link("## Related\n- [[a]]\n\n## 后记\n尾巴\n", "b");
        assert_eq!(middle, "## Related\n- [[a]]\n- [[b]]\n\n## 后记\n尾巴\n");
        // 空笔记。
        assert_eq!(append_related_link("", "x"), "## 相关\n\n- [[x]]\n");
        // 只有标题、下面还没条目。
        assert_eq!(
            append_related_link("## 相关\n", "x"),
            "## 相关\n\n- [[x]]\n"
        );
    }

    #[test]
    fn remove_drops_list_items_and_unwraps_inline_links() {
        let text = "\
# 笔记

方案见 [[auth|登录方案]]，另见 [[db]]。

## 相关

- [[auth]]
- [[db]]
";
        let (out, removed) = remove_wikilinks(text, |l| l.target == "auth");
        assert_eq!(removed, 2);
        assert_eq!(
            out,
            "# 笔记\n\n方案见 登录方案，另见 [[db]]。\n\n## 相关\n\n- [[db]]\n"
        );
        // 再断开 db：「相关」一节空了，标题一起去掉。
        let (out, removed) = remove_wikilinks(&out, |l| l.target == "db");
        assert_eq!(removed, 2);
        assert_eq!(out, "# 笔记\n\n方案见 登录方案，另见 db。\n");
    }

    #[test]
    fn remove_treats_an_embed_item_as_a_whole_line() {
        let (out, removed) = remove_wikilinks(
            "- ![[图]]\n*  [[图]]\n- 留着 [[图]] 和 ![[图]]。\n",
            |_| true,
        );
        assert_eq!((out.as_str(), removed), ("- 留着 图 和 图。\n", 4));
    }

    #[test]
    fn remove_without_a_match_returns_the_text_untouched() {
        let text = "只有 [[a]]\r\n保留行尾\r\n";
        let (out, removed) = remove_wikilinks(text, |l| l.target == "zzz");
        assert_eq!((out.as_str(), removed), (text, 0));
    }

    #[test]
    fn linkable_names_exclude_syntax_characters() {
        assert!(is_linkable_name("chatgpt-订阅通道"));
        assert!(!is_linkable_name("a|b"));
        assert!(!is_linkable_name("a#b"));
        assert!(!is_linkable_name("a]b"));
        assert!(!is_linkable_name("  "));
    }
}
