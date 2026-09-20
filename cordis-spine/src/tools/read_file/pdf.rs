//! PDF text extraction via `pdf_oxide` (Grok-aligned).
//!
//! **Dependency choice:** `pdf_oxide` on `cordis-spine` (not `cordis-base`).
//! Same pure-Rust engine Grok vendors against. Default features only —
//! the `rendering` feature (page→JPEG) pulls `hayro-jpeg2000` (rustc 1.92+)
//! which exceeds dock's MSRV 1.88, so `format=image` is deferred. Dock
//! therefore defaults `format` to **text** (Grok defaults to image).

use std::fmt::Write as _;
use std::time::Duration;

use cordis_base::types::UserImage;

pub const MAX_PDF_BYTES: usize = 50 * 1024 * 1024;
const PDF_AUTO_READ_THRESHOLD: usize = 10;
pub const PDF_MAX_PAGES_PER_READ: usize = 20;
pub const PDF_PROCESS_TIMEOUT: Duration = Duration::from_secs(60);

/// Three-tier PDF detection: magic bytes or extension.
pub fn is_pdf_file(file_bytes: &[u8], extension: &str) -> bool {
    is_pdf_magic(file_bytes) || extension == "pdf"
}

pub fn is_pdf_magic(bytes: &[u8]) -> bool {
    bytes.len() >= 5 && &bytes[..5] == b"%PDF-"
}

/// Parse page range into sorted, deduplicated 0-based indices.
pub fn parse_page_range(spec: &str, page_count: usize) -> Result<Vec<usize>, String> {
    let mut pages = Vec::new();
    for part in spec.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((start, end)) = part.split_once('-') {
            let start: usize = start
                .trim()
                .parse()
                .map_err(|_| format!("invalid page number: '{}'", start.trim()))?;
            let end = if end.trim().is_empty() {
                page_count
            } else {
                end.trim()
                    .parse()
                    .map_err(|_| format!("invalid page number: '{}'", end.trim()))?
            };
            if start < 1 || start > page_count {
                return Err(format!(
                    "page {start} out of range (document has {page_count} pages)"
                ));
            }
            if start > end {
                return Err(format!(
                    "invalid page range: {start}-{end} (start must be ≤ end)"
                ));
            }
            let end = end.min(page_count);
            for p in start..=end {
                pages.push(p - 1);
            }
        } else {
            let p: usize = part
                .parse()
                .map_err(|_| format!("invalid page number: '{part}'"))?;
            if p < 1 || p > page_count {
                return Err(format!(
                    "page {p} out of range (document has {page_count} pages)"
                ));
            }
            pages.push(p - 1);
        }
    }
    pages.sort_unstable();
    pages.dedup();
    if pages.len() > PDF_MAX_PAGES_PER_READ {
        return Err(format!(
            "requested {} pages, maximum is {PDF_MAX_PAGES_PER_READ} per call",
            pages.len()
        ));
    }
    if pages.is_empty() {
        return Err("no pages specified".to_string());
    }
    Ok(pages)
}

fn open_pdf_document(bytes: Vec<u8>) -> Result<(pdf_oxide::PdfDocument, usize), String> {
    let doc = pdf_oxide::PdfDocument::from_bytes(bytes)
        .map_err(|e| format!("Failed to open PDF: {e}"))?;
    let page_count = doc
        .page_count()
        .map_err(|e| format!("Failed to read PDF page count: {e}"))?;
    if page_count == 0 {
        return Err("PDF has no pages".to_string());
    }
    Ok((doc, page_count))
}

fn open_pdf_and_resolve_pages(
    bytes: Vec<u8>,
    pages_spec: Option<&str>,
) -> Result<(pdf_oxide::PdfDocument, usize, Vec<usize>), String> {
    let (doc, page_count) = open_pdf_document(bytes)?;
    let page_indices = match pages_spec {
        Some(spec) => parse_page_range(spec, page_count)?,
        None => {
            if page_count > PDF_AUTO_READ_THRESHOLD {
                return Err(format!(
                    "PDF has {page_count} pages which exceeds the {PDF_AUTO_READ_THRESHOLD} page \
                     auto-read limit. Use the `pages` parameter to specify which pages to read \
                     (e.g. pages=\"1-5\"). Maximum {PDF_MAX_PAGES_PER_READ} pages per call."
                ));
            }
            (0..page_count).collect()
        }
    };
    Ok((doc, page_count, page_indices))
}

fn extract_pdf_text(bytes: Vec<u8>, pages_spec: Option<&str>) -> Result<String, String> {
    let (doc, _page_count, page_indices) = open_pdf_and_resolve_pages(bytes, pages_spec)?;
    let mut text = String::new();
    for (i, &page_idx) in page_indices.iter().enumerate() {
        if i > 0 {
            text.push('\n');
        }
        let _ = writeln!(&mut text, "--- Page {} ---", page_idx + 1);
        match doc.extract_text(page_idx) {
            Ok(page_text) => text.push_str(&page_text),
            Err(e) => {
                let _ = writeln!(
                    &mut text,
                    "[Failed to extract text from page {}: {e}]",
                    page_idx + 1
                );
            }
        }
    }
    Ok(text)
}

fn line_number_text(raw: &str) -> String {
    let mut content = String::new();
    for (i, line) in raw.split('\n').enumerate() {
        if i > 0 {
            content.push('\n');
        }
        let n = i + 1;
        if n == 1 || n.is_multiple_of(10) {
            let _ = write!(&mut content, "{n}→{line}");
        } else {
            content.push_str(line);
        }
    }
    content
}

/// Handle a PDF. Dock defaults to `format=text` (see module docs).
pub async fn handle_pdf(
    path_display: &str,
    file_bytes: Vec<u8>,
    pages: Option<&str>,
    format: Option<&str>,
) -> Result<(String, Vec<UserImage>), String> {
    if file_bytes.len() > MAX_PDF_BYTES {
        return Err(format!(
            "PDF file is {:.1} MB, exceeds the {:.0} MB limit.",
            file_bytes.len() as f64 / 1_048_576.0,
            MAX_PDF_BYTES as f64 / 1_048_576.0,
        ));
    }

    match format {
        None | Some("text") => {}
        Some("image") => {
            return Err(
                "PDF format=image (page rasterisation) is not available yet on this \
                 rustc MSRV — pdf_oxide's `rendering` feature needs rustc ≥ 1.92. \
                 Use format=text (default) to extract text, or raise the workspace MSRV."
                    .into(),
            );
        }
        Some(other) => {
            return Err(format!(
                "Invalid format '{other}'. Supported values: 'text' (default), 'image' (deferred)."
            ));
        }
    }

    let pages_owned = pages.map(str::to_owned);
    let path_owned = path_display.to_owned();

    let result = tokio::time::timeout(
        PDF_PROCESS_TIMEOUT,
        tokio::task::spawn_blocking(move || {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let raw = extract_pdf_text(file_bytes, pages_owned.as_deref())?;
                let content = line_number_text(&raw);
                let _ = path_owned;
                Ok::<_, String>((content, Vec::new()))
            }))
        }),
    )
    .await;

    match result {
        Ok(Ok(Ok(Ok(pair)))) => Ok(pair),
        Ok(Ok(Ok(Err(e)))) => Err(e),
        Ok(Ok(Err(_panic))) => Err(format!(
            "PDF processing failed (internal error): {path_display}"
        )),
        Ok(Err(e)) => Err(format!("PDF processing failed: {e}")),
        Err(_elapsed) => Err(format!(
            "PDF processing timed out after {}s: {path_display}",
            PDF_PROCESS_TIMEOUT.as_secs()
        )),
    }
}

/// Minimal multi-page PDF fixture for unit tests.
#[cfg(test)]
pub fn make_test_pdf(page_texts: &[&str]) -> Vec<u8> {
    let mut pdf = Vec::new();
    pdf.extend_from_slice(b"%PDF-1.4\n");

    let mut offsets = Vec::new();

    offsets.push(pdf.len());
    pdf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");

    let page_count = page_texts.len();
    let kids: Vec<String> = (0..page_count)
        .map(|i| format!("{} 0 R", 3 + i * 3))
        .collect();
    offsets.push(pdf.len());
    let pages_obj = format!(
        "2 0 obj\n<< /Type /Pages /Kids [{}] /Count {page_count} >>\nendobj\n",
        kids.join(" "),
    );
    pdf.extend_from_slice(pages_obj.as_bytes());

    for (i, text) in page_texts.iter().enumerate() {
        let page_obj = 3 + i * 3;
        let content_obj = 4 + i * 3;
        let font_obj = 5 + i * 3;

        let stream_content = format!("BT /F1 12 Tf 72 720 Td ({text}) Tj ET");
        let stream_len = stream_content.len();

        offsets.push(pdf.len());
        pdf.extend_from_slice(
            format!(
                "{page_obj} 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
                 /Contents {content_obj} 0 R /Resources << /Font << /F1 {font_obj} 0 R >> >> >>\nendobj\n"
            )
            .as_bytes(),
        );

        offsets.push(pdf.len());
        pdf.extend_from_slice(
            format!(
                "{content_obj} 0 obj\n<< /Length {stream_len} >>\nstream\n{stream_content}\nendstream\nendobj\n"
            )
            .as_bytes(),
        );

        offsets.push(pdf.len());
        pdf.extend_from_slice(
            format!(
                "{font_obj} 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\n"
            )
            .as_bytes(),
        );
    }

    let xref_offset = pdf.len();
    let total_objects = 2 + page_count * 3 + 1;
    pdf.extend_from_slice(format!("xref\n0 {total_objects}\n").as_bytes());
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    for offset in &offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }

    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {total_objects} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n"
        )
        .as_bytes(),
    );

    pdf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_page_range_basic() {
        assert_eq!(parse_page_range("3", 10).unwrap(), vec![2]);
        assert_eq!(parse_page_range("1-3", 10).unwrap(), vec![0, 1, 2]);
    }

    #[test]
    fn parse_page_range_rejects_too_many() {
        let err = parse_page_range("1-21", 30).unwrap_err();
        assert!(err.contains("maximum is"), "{err}");
    }

    #[test]
    fn extract_text_multi_page() {
        let pdf = make_test_pdf(&["Alpha", "Beta"]);
        let text = extract_pdf_text(pdf, None).unwrap();
        assert!(text.contains("--- Page 1 ---"));
        assert!(text.contains("--- Page 2 ---"));
        assert!(text.contains("Alpha"));
        assert!(text.contains("Beta"));
    }

    #[test]
    fn extract_text_with_pages_spec() {
        let pdf = make_test_pdf(&["First", "Second", "Third"]);
        let text = extract_pdf_text(pdf, Some("2")).unwrap();
        assert!(text.contains("--- Page 2 ---"));
        assert!(text.contains("Second"));
        assert!(!text.contains("--- Page 1 ---"));
    }

    #[test]
    fn auto_read_rejects_large_pdf() {
        let pages: Vec<&str> = (0..12).map(|_| "x").collect();
        let pdf = make_test_pdf(&pages);
        let err = extract_pdf_text(pdf, None).unwrap_err();
        assert!(err.contains("auto-read limit"), "{err}");
    }

    #[tokio::test]
    async fn handle_pdf_format_text() {
        let pdf = make_test_pdf(&["Hello World"]);
        let (content, images) = handle_pdf("/tmp/t.pdf", pdf, None, Some("text"))
            .await
            .unwrap();
        assert!(images.is_empty());
        assert!(content.contains("Hello World"));
    }

    #[tokio::test]
    async fn handle_pdf_default_is_text() {
        let pdf = make_test_pdf(&["Default Text"]);
        let (content, images) = handle_pdf("/tmp/t.pdf", pdf, None, None).await.unwrap();
        assert!(images.is_empty());
        assert!(content.contains("Default Text"));
    }

    #[tokio::test]
    async fn handle_pdf_format_image_deferred() {
        let pdf = make_test_pdf(&["Some Text"]);
        let err = handle_pdf("/tmp/t.pdf", pdf, None, Some("image"))
            .await
            .unwrap_err();
        assert!(err.contains("format=image"), "{err}");
        assert!(err.contains("format=text"), "{err}");
    }
}
