//! 笔记之间「内容相近」的打分：图谱上的建议边。
//!
//! 不依赖嵌入模型（默认配置下没有）：词频—逆文档频率向量的余弦相似度。中文按相邻
//! 两字切（和检索索引 [`crate::tokenize`] 同一种切法），英文和标识符按词。只在少数
//! 几篇里出现的词权重高，篇篇都有的词（「可以」「the」、flush 模板里的小标题）接近 0。
//!
//! 分数只用来排序和过滤，不是概率；阈值见 [`SimilarConfig`]。

use std::collections::HashMap;

use crate::query_expansion::is_stop_word;
use crate::tokenize::is_cjk;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SimilarConfig {
    /// 每篇最多连几条建议。0 = 不算相似度。
    pub per_note: usize,
    /// 低于这个分数的不连。
    pub min_score: f32,
    /// 笔记数超过这个就不算了（两两比较是平方级的）。
    pub max_notes: usize,
}

impl Default for SimilarConfig {
    fn default() -> Self {
        Self {
            per_note: 3,
            // 拿真实记忆目录校准的：长笔记词表大，确实相关的两篇余弦也就 0.2–0.3，
            // 跨项目只共用 flush 模板措辞的在 0.09 以下。
            min_score: 0.1,
            max_notes: 1500,
        }
    }
}

/// 一对相近的笔记：`a < b` 是传入切片的下标。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SimilarPair {
    pub a: usize,
    pub b: usize,
    pub score: f32,
}

/// 两两比较，回每篇分数最高的 `per_note` 个邻居（并集，去重），分数从高到低。
/// 超过 `max_notes` 篇回 `None`（没算，不是没有）。
pub fn similar_pairs(texts: &[&str], config: &SimilarConfig) -> Option<Vec<SimilarPair>> {
    if texts.len() > config.max_notes {
        return None;
    }
    if config.per_note == 0 || texts.len() < 2 {
        return Some(Vec::new());
    }
    let vectors = tfidf_vectors(texts);
    // 倒排：词 → [(笔记, 权重)]。只有共用了词的两篇才会被比到。
    let mut postings: HashMap<u32, Vec<(usize, f32)>> = HashMap::new();
    for (doc, vector) in vectors.iter().enumerate() {
        for (term, weight) in vector {
            postings.entry(*term).or_default().push((doc, *weight));
        }
    }
    let mut dots: HashMap<(usize, usize), f32> = HashMap::new();
    for list in postings.values() {
        for (i, (a, wa)) in list.iter().enumerate() {
            for (b, wb) in list.iter().skip(i + 1) {
                *dots.entry((*a, *b)).or_default() += wa * wb;
            }
        }
    }
    // 向量已经归一，点积就是余弦。
    let mut neighbours: Vec<Vec<(usize, f32)>> = vec![Vec::new(); texts.len()];
    for ((a, b), score) in &dots {
        if *score < config.min_score {
            continue;
        }
        if let Some(list) = neighbours.get_mut(*a) {
            list.push((*b, *score));
        }
        if let Some(list) = neighbours.get_mut(*b) {
            list.push((*a, *score));
        }
    }
    let mut picked: HashMap<(usize, usize), f32> = HashMap::new();
    for (doc, list) in neighbours.iter_mut().enumerate() {
        list.sort_by(|x, y| y.1.total_cmp(&x.1).then(x.0.cmp(&y.0)));
        for (other, score) in list.iter().take(config.per_note) {
            picked.insert((doc.min(*other), doc.max(*other)), *score);
        }
    }
    let mut pairs: Vec<SimilarPair> = picked
        .into_iter()
        .map(|((a, b), score)| SimilarPair { a, b, score })
        .collect();
    pairs.sort_by(|x, y| {
        y.score
            .total_cmp(&x.score)
            .then((x.a, x.b).cmp(&(y.a, y.b)))
    });
    Some(pairs)
}

/// 每篇一个归一化的稀疏向量：`(词 id, 权重)`。
fn tfidf_vectors(texts: &[&str]) -> Vec<Vec<(u32, f32)>> {
    let mut vocabulary: HashMap<String, u32> = HashMap::new();
    let mut counts: Vec<HashMap<u32, u32>> = Vec::with_capacity(texts.len());
    let mut document_frequency: HashMap<u32, u32> = HashMap::new();
    for text in texts {
        let mut local: HashMap<u32, u32> = HashMap::new();
        for term in terms(text) {
            let next = vocabulary.len() as u32;
            let id = *vocabulary.entry(term).or_insert(next);
            *local.entry(id).or_default() += 1;
        }
        for id in local.keys() {
            *document_frequency.entry(*id).or_default() += 1;
        }
        counts.push(local);
    }
    let total = texts.len() as f32;
    counts
        .into_iter()
        .map(|local| {
            let mut vector: Vec<(u32, f32)> = local
                .into_iter()
                .map(|(id, count)| {
                    let df = document_frequency.get(&id).copied().unwrap_or(1) as f32;
                    // BM25 式的逆文档频率：恒为正，篇篇都有的词趋近 0。
                    let idf = (1.0 + (total - df + 0.5) / (df + 0.5)).ln();
                    (id, (1.0 + (count as f32).ln()) * idf)
                })
                .collect();
            let norm = vector.iter().map(|(_, w)| w * w).sum::<f32>().sqrt();
            if norm > 0.0 {
                for (_, weight) in &mut vector {
                    *weight /= norm;
                }
            }
            vector
        })
        .collect()
}

/// 词太长的多半是哈希、base64、路径拼出来的串。
const MAX_WORD_CHARS: usize = 40;

/// 正文切成词：中日韩文字相邻两字一组，其余按字母数字连成的词（小写）。
fn terms(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut cjk: Vec<char> = Vec::new();
    let flush_word = |word: &mut String, out: &mut Vec<String>| {
        if keep_word(word) {
            out.push(std::mem::take(word));
        } else {
            word.clear();
        }
    };
    let flush_cjk = |cjk: &mut Vec<char>, out: &mut Vec<String>| {
        for pair in cjk.windows(2) {
            out.push(pair.iter().collect());
        }
        cjk.clear();
    };
    for ch in text.chars() {
        if is_cjk(ch) && ch.is_alphanumeric() {
            flush_word(&mut word, &mut out);
            cjk.push(ch);
        } else if ch.is_alphanumeric() || ch == '_' {
            flush_cjk(&mut cjk, &mut out);
            word.extend(ch.to_lowercase());
        } else {
            flush_word(&mut word, &mut out);
            flush_cjk(&mut cjk, &mut out);
        }
    }
    flush_word(&mut word, &mut out);
    flush_cjk(&mut cjk, &mut out);
    out
}

fn keep_word(word: &str) -> bool {
    let len = word.chars().count();
    if !(2..=MAX_WORD_CHARS).contains(&len) || is_stop_word(word) {
        return false;
    }
    // 纯数字（日期、行号）和长十六进制串（哈希、会话 id）不带主题信息。
    let digits = word.chars().all(|c| c.is_numeric());
    let hex_blob = len >= 12 && word.chars().all(|c| c.is_ascii_hexdigit());
    !digits && !hex_blob
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score(pairs: &[SimilarPair], a: usize, b: usize) -> Option<f32> {
        pairs
            .iter()
            .find(|p| (p.a, p.b) == (a.min(b), a.max(b)))
            .map(|p| p.score)
    }

    #[test]
    fn terms_split_cjk_into_bigrams_and_keep_identifiers() {
        assert_eq!(
            terms("审计日志改造：AuditService.record() 走 audit_log 表，见 #284"),
            vec![
                "审计",
                "计日",
                "日志",
                "志改",
                "改造",
                "auditservice",
                "record",
                "audit_log"
            ]
        );
        // 单个汉字、停用词、纯数字、长哈希都不留。
        assert!(terms("表 the 20261115 01a1dac3b90d7c71af424c9e265d3 a").is_empty());
    }

    #[test]
    fn related_notes_score_above_unrelated_ones() {
        let texts = [
            "# 审计日志改造\n审计日志降噪，AuditApplicationService.record 是唯一入口，补全结构化字段。",
            "# 审计日志最终形态\n审计模块固定在 web/audit，AuditApplicationService 负责 record，日志结构化。",
            "# 分组删除 400\nTomcat 拒绝编码斜杠，encodeURIComponent 之后路径里是 %2F。",
            "# 浏览器面板\nscreencast 只出 CSS 像素，拖选要带 buttons。",
        ];
        let pairs = similar_pairs(
            &texts,
            &SimilarConfig {
                min_score: 0.0,
                ..SimilarConfig::default()
            },
        )
        .unwrap();
        let audit = score(&pairs, 0, 1).expect("两篇审计笔记应该相近");
        assert!(audit > 0.2, "audit={audit}");
        for (a, b) in [(0, 2), (0, 3), (1, 2), (1, 3), (2, 3)] {
            let other = score(&pairs, a, b).unwrap_or(0.0);
            assert!(other < audit / 4.0, "({a},{b})={other} audit={audit}");
        }
        // 默认阈值下只剩那一对。
        let strict = similar_pairs(&texts, &SimilarConfig::default()).unwrap();
        assert_eq!(
            strict.iter().map(|p| (p.a, p.b)).collect::<Vec<_>>(),
            vec![(0, 1)]
        );
    }

    #[test]
    fn each_note_keeps_only_its_best_neighbours() {
        // 五篇互相都像：每篇只留 1 个邻居时，边数不会是全连接的 10 条。
        let texts: Vec<String> = (0..5)
            .map(|i| format!("cordis plugin waterfall gateway memory note{i} extra{i}"))
            .collect();
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        let pairs = similar_pairs(
            &refs,
            &SimilarConfig {
                per_note: 1,
                min_score: 0.0,
                max_notes: 100,
            },
        )
        .unwrap();
        assert!(!pairs.is_empty() && pairs.len() <= 5, "len={}", pairs.len());
        assert!(pairs.iter().all(|p| p.a < p.b));
        assert!(pairs.windows(2).all(|w| match w {
            [x, y] => x.score >= y.score,
            _ => true,
        }));
    }

    #[test]
    fn degenerate_inputs_do_not_panic() {
        let config = SimilarConfig::default();
        assert_eq!(similar_pairs(&[], &config), Some(Vec::new()));
        assert_eq!(similar_pairs(&["只有一篇"], &config), Some(Vec::new()));
        // 空笔记、全是停用词的笔记：向量为空，不和谁相近。
        assert_eq!(
            similar_pairs(&["", "the a an", ""], &config),
            Some(Vec::new())
        );
        // 一模一样的两篇：词在每篇都出现，权重很低但仍然是 1.0 的余弦。
        let same = similar_pairs(&["cordis 插件树", "cordis 插件树"], &config).unwrap();
        let [pair] = same.as_slice() else {
            panic!("应该有一对：{same:?}");
        };
        assert!((pair.score - 1.0).abs() < 1e-4, "score={}", pair.score);
        // 关掉 / 超过上限。
        let off = SimilarConfig {
            per_note: 0,
            ..config
        };
        assert_eq!(similar_pairs(&["a b", "a b"], &off), Some(Vec::new()));
        let capped = SimilarConfig {
            max_notes: 1,
            ..config
        };
        assert_eq!(similar_pairs(&["a b", "a b"], &capped), None);
    }
}
