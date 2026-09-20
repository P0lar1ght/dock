//! Split markdown into FTS chunks (header-aware).

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub text: String,
    pub start_line: usize,
    pub end_line: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct ChunkConfig {
    pub max_chunk_chars: usize,
    pub chunk_overlap_chars: usize,
}

impl Default for ChunkConfig {
    fn default() -> Self {
        Self {
            max_chunk_chars: 1600,
            chunk_overlap_chars: 320,
        }
    }
}

pub fn chunk_hash(text: &str) -> String {
    blake3::hash(text.as_bytes()).to_hex().to_string()
}

pub fn chunk_markdown(content: &str, config: &ChunkConfig) -> Vec<Chunk> {
    if content.is_empty() {
        return vec![];
    }
    let max_chars = config.max_chunk_chars.max(64);
    let lines: Vec<&str> = content.lines().collect();
    if content.len() <= max_chars {
        return vec![Chunk {
            text: content.to_string(),
            start_line: 0,
            end_line: lines.len(),
        }];
    }

    // Split on ## headers; oversized sections split by blank lines.
    let mut chunks = Vec::new();
    let mut section_start = 0usize;
    let mut i = 0usize;
    while i <= lines.len() {
        let at_header = i < lines.len()
            && lines
                .get(i)
                .is_some_and(|l| l.starts_with("## ") || l.starts_with("# "));
        if (at_header && i > section_start) || i == lines.len() {
            let section = lines
                .get(section_start..i)
                .unwrap_or(&[])
                .join("\n");
            if !section.trim().is_empty() {
                push_section(&mut chunks, &section, section_start, max_chars);
            }
            section_start = i;
        }
        i += 1;
    }
    chunks
}

fn push_section(chunks: &mut Vec<Chunk>, section: &str, start_line: usize, max_chars: usize) {
    if section.len() <= max_chars {
        let nlines = section.lines().count();
        chunks.push(Chunk {
            text: section.to_string(),
            start_line,
            end_line: start_line + nlines,
        });
        return;
    }
    let paragraphs: Vec<&str> = section.split("\n\n").collect();
    let mut buf = String::new();
    let mut buf_start = start_line;
    let mut line_cursor = start_line;
    for para in paragraphs {
        let para_lines = para.lines().count().max(1);
        if !buf.is_empty() && buf.len() + para.len() + 2 > max_chars {
            let nlines = buf.lines().count();
            chunks.push(Chunk {
                text: std::mem::take(&mut buf),
                start_line: buf_start,
                end_line: buf_start + nlines,
            });
            buf_start = line_cursor;
        }
        if !buf.is_empty() {
            buf.push_str("\n\n");
        }
        buf.push_str(para);
        line_cursor += para_lines + 1;
    }
    if !buf.trim().is_empty() {
        let nlines = buf.lines().count();
        chunks.push(Chunk {
            text: buf,
            start_line: buf_start,
            end_line: buf_start + nlines,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_content_one_chunk() {
        let c = chunk_markdown("hello\nworld", &ChunkConfig::default());
        assert_eq!(c.len(), 1);
        assert_eq!(c.first().unwrap().text, "hello\nworld");
    }

    #[test]
    fn splits_on_headers() {
        let text = format!(
            "## A\n{}\n## B\n{}",
            "x".repeat(100),
            "y".repeat(100)
        );
        let c = chunk_markdown(
            &text,
            &ChunkConfig {
                max_chunk_chars: 80,
                chunk_overlap_chars: 0,
            },
        );
        assert!(c.len() >= 2);
    }
}
