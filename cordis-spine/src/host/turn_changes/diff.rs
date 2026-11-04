//! 两份文本 → 统一 diff（`git diff` 的样子）+ 增删行数。给每轮改动（[`super`]）和网关的
//! 工作区 diff（未跟踪的新文件）用。
//!
//! 先去掉首尾相同的行，中间那段做一次 LCS；中间段太大（两边行数之积超过
//! [`LCS_CELLS`]）就整段算删了再加——行数仍对，只是块不够细。

/// LCS 表最多这么多格（u32，约 16MB）。
const LCS_CELLS: usize = 4_000_000;
/// 每块前后带几行上下文（同 git）。
const CONTEXT: usize = 3;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DiffStat {
    pub added: usize,
    pub removed: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Same(usize, usize),
    Del(usize),
    Add(usize),
}

fn lines(s: &str) -> Vec<&str> {
    s.lines().collect()
}

fn ops(a: &[&str], b: &[&str]) -> Vec<Op> {
    let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..]
        .iter()
        .rev()
        .zip(b[prefix..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (ma, mb) = (a.len() - prefix - suffix, b.len() - prefix - suffix);
    let mut out: Vec<Op> = (0..prefix).map(|i| Op::Same(i, i)).collect();
    let mid_a = &a[prefix..prefix + ma];
    let mid_b = &b[prefix..prefix + mb];
    if ma == 0 || mb == 0 || (ma + 1).saturating_mul(mb + 1) > LCS_CELLS {
        out.extend((0..ma).map(|i| Op::Del(prefix + i)));
        out.extend((0..mb).map(|j| Op::Add(prefix + j)));
    } else {
        // dp[i][j] = mid_a[i..] 与 mid_b[j..] 的 LCS 长度。
        let w = mb + 1;
        let mut dp = vec![0u32; (ma + 1) * w];
        for i in (0..ma).rev() {
            for j in (0..mb).rev() {
                dp[i * w + j] = if mid_a[i] == mid_b[j] {
                    dp[(i + 1) * w + j + 1] + 1
                } else {
                    dp[(i + 1) * w + j].max(dp[i * w + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < ma && j < mb {
            if mid_a[i] == mid_b[j] {
                out.push(Op::Same(prefix + i, prefix + j));
                i += 1;
                j += 1;
            } else if dp[(i + 1) * w + j] >= dp[i * w + j + 1] {
                out.push(Op::Del(prefix + i));
                i += 1;
            } else {
                out.push(Op::Add(prefix + j));
                j += 1;
            }
        }
        out.extend((i..ma).map(|i| Op::Del(prefix + i)));
        out.extend((j..mb).map(|j| Op::Add(prefix + j)));
    }
    let (ta, tb) = (a.len() - suffix, b.len() - suffix);
    out.extend((0..suffix).map(|k| Op::Same(ta + k, tb + k)));
    out
}

/// 只要增删行数（不拼文本）。
pub fn diff_stat(before: Option<&str>, after: Option<&str>) -> DiffStat {
    let (a, b) = (lines(before.unwrap_or("")), lines(after.unwrap_or("")));
    let mut stat = DiffStat::default();
    for op in ops(&a, &b) {
        match op {
            Op::Del(_) => stat.removed += 1,
            Op::Add(_) => stat.added += 1,
            Op::Same(..) => {}
        }
    }
    stat
}

/// `label` 一个文件从 `before` 到 `after` 的统一 diff；`None` = 那一边文件不存在。
/// 两边一样回空串。
pub fn unified_diff(label: &str, before: Option<&str>, after: Option<&str>) -> (String, DiffStat) {
    if before == after {
        return (String::new(), DiffStat::default());
    }
    let (a, b) = (lines(before.unwrap_or("")), lines(after.unwrap_or("")));
    let ops = ops(&a, &b);
    let mut stat = DiffStat::default();
    let mut out = format!("diff --git a/{label} b/{label}\n");
    match (before, after) {
        (None, _) => out.push_str("new file mode 100644\n"),
        (_, None) => out.push_str("deleted file mode 100644\n"),
        _ => {}
    }
    out.push_str(&match before {
        Some(_) => format!("--- a/{label}\n"),
        None => "--- /dev/null\n".into(),
    });
    out.push_str(&match after {
        Some(_) => format!("+++ b/{label}\n"),
        None => "+++ /dev/null\n".into(),
    });

    // 改动所在的 op 下标，按「间隔不超过两倍上下文」并成块。
    let changed: Vec<usize> = ops
        .iter()
        .enumerate()
        .filter(|(_, op)| !matches!(op, Op::Same(..)))
        .map(|(i, _)| i)
        .collect();
    let mut hunks: Vec<(usize, usize)> = Vec::new();
    for &i in &changed {
        let start = i.saturating_sub(CONTEXT);
        let end = (i + CONTEXT + 1).min(ops.len());
        match hunks.last_mut() {
            Some(last) if start <= last.1 => last.1 = end,
            _ => hunks.push((start, end)),
        }
    }
    // 每个 op 之前旧 / 新各走过几行。
    let mut before_old = Vec::with_capacity(ops.len() + 1);
    let mut before_new = Vec::with_capacity(ops.len() + 1);
    let (mut o, mut n) = (0usize, 0usize);
    for op in &ops {
        before_old.push(o);
        before_new.push(n);
        match op {
            Op::Same(..) => {
                o += 1;
                n += 1;
            }
            Op::Del(_) => o += 1,
            Op::Add(_) => n += 1,
        }
    }
    for (start, end) in hunks {
        let mut body = String::new();
        let (mut old_len, mut new_len) = (0, 0);
        for op in &ops[start..end] {
            match *op {
                Op::Same(i, _) => {
                    body.push_str(&format!(" {}\n", a[i]));
                    old_len += 1;
                    new_len += 1;
                }
                Op::Del(i) => {
                    body.push_str(&format!("-{}\n", a[i]));
                    old_len += 1;
                    stat.removed += 1;
                }
                Op::Add(j) => {
                    body.push_str(&format!("+{}\n", b[j]));
                    new_len += 1;
                    stat.added += 1;
                }
            }
        }
        // 空的一边按 git：起点是它前面那一行（文件开头就是 0）。
        let old_start = before_old[start] + usize::from(old_len > 0);
        let new_start = before_new[start] + usize::from(new_len > 0);
        out.push_str(&format!(
            "@@ -{old_start},{old_len} +{new_start},{new_len} @@\n{body}"
        ));
    }
    (out, stat)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modified_file_has_context_and_line_numbers() {
        let before = "a\nb\nc\nd\ne\nf\ng\nh\n";
        let after = "a\nb\nc\nD\ne\nf\ng\nh\ni\n";
        let (diff, stat) = unified_diff("src/x.rs", Some(before), Some(after));
        assert_eq!(
            stat,
            DiffStat {
                added: 2,
                removed: 1
            }
        );
        assert_eq!(
            diff,
            "diff --git a/src/x.rs b/src/x.rs\n--- a/src/x.rs\n+++ b/src/x.rs\n\
             @@ -1,8 +1,9 @@\n a\n b\n c\n-d\n+D\n e\n f\n g\n h\n+i\n"
        );
    }

    #[test]
    fn far_apart_changes_make_two_hunks() {
        let before: String = (1..=20).map(|i| format!("l{i}\n")).collect();
        let after = before
            .replace("l2\n", "two\n")
            .replace("l19\n", "nineteen\n");
        let (diff, stat) = unified_diff("n.txt", Some(&before), Some(&after));
        assert_eq!(
            stat,
            DiffStat {
                added: 2,
                removed: 2
            }
        );
        assert_eq!(diff.matches("\n@@ ").count(), 2, "{diff}");
        assert!(diff.contains("@@ -1,5 +1,5 @@"), "{diff}");
        assert!(diff.contains("@@ -16,5 +16,5 @@"), "{diff}");
    }

    #[test]
    fn created_and_deleted_files() {
        let (diff, stat) = unified_diff("new.md", None, Some("x\ny\n"));
        assert_eq!(
            stat,
            DiffStat {
                added: 2,
                removed: 0
            }
        );
        assert!(diff.contains("new file mode"), "{diff}");
        assert!(
            diff.contains("--- /dev/null\n+++ b/new.md\n@@ -0,0 +1,2 @@\n+x\n+y\n"),
            "{diff}"
        );
        let (diff, stat) = unified_diff("old.md", Some("x\n"), None);
        assert_eq!(
            stat,
            DiffStat {
                added: 0,
                removed: 1
            }
        );
        assert!(
            diff.contains("--- a/old.md\n+++ /dev/null\n@@ -1,1 +0,0 @@\n-x\n"),
            "{diff}"
        );
    }

    #[test]
    fn same_text_is_empty_and_stat_agrees_with_diff() {
        assert_eq!(unified_diff("a", Some("x"), Some("x")).0, "");
        let before = "fn a() {}\nfn b() {}\n";
        let after = "fn a() {}\n// note\nfn b() { todo!() }\n";
        let (_, stat) = unified_diff("a", Some(before), Some(after));
        assert_eq!(stat, diff_stat(Some(before), Some(after)));
        assert_eq!(
            stat,
            DiffStat {
                added: 2,
                removed: 1
            }
        );
    }

    /// 中间段大到不做 LCS 时整段算删了再加：行数仍对。
    #[test]
    fn huge_middle_falls_back_to_replace() {
        let before: String = (0..3000).map(|i| format!("a{i}\n")).collect();
        let after: String = (0..3000).map(|i| format!("b{i}\n")).collect();
        let stat = diff_stat(Some(&before), Some(&after));
        assert_eq!(
            stat,
            DiffStat {
                added: 3000,
                removed: 3000
            }
        );
    }
}
