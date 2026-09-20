//! `read_file` workspace tool — text, images, PDF, PPTX (Grok-aligned).
//!
//! Branch order (bytes already loaded):
//! image → PDF → PPTX → binary reject → text.

mod binary;
mod image;
mod pdf;
mod pptx;

use cordis_base::types::{ToolSpec, UserImage};

use crate::tools::fs_common::{int_field, parse_args, resolve, str_field};
const READ_FILE_PARAMS: &str = r#"{"type":"object","properties":{"target_file":{"type":"string","description":"Path of the file to read (relative to cwd or absolute)."},"offset":{"type":"integer","description":"1-based start line. Omit to start at line 1. Use with limit for large files."},"limit":{"type":"integer","description":"Max lines to return. Omit to use the default cap (1000). Pass a smaller value for a tight window."},"pages":{"type":"string","description":"Page range for PDF files (e.g. '1-5', '3', '10-'). Required for PDFs with more than 10 pages. Max 20 pages per call. Ignored for non-PDF files."},"format":{"type":"string","description":"Output format for PDF files. 'text' (default) extracts text. 'image' (page rasterisation) is deferred until MSRV allows pdf_oxide rendering. Ignored for non-PDF files."}},"required":["target_file"]}"#;

const READ_FILE_DESC: &str = "Read a file.\n\
- Use this instead of `cat` / `head` / `sed -n` through bash: it is gated as read-only, works in plan mode, and tells you how much of the file you have not seen.\n\
- By default reads up to 1000 lines from offset (default line 1).\n\
- For large files, pass offset + limit to page through; the result notes how many lines remain.\n\
- Line anchors appear as N→ on line 1 and every 10th line. That prefix is not part of the file — when passing text to search_replace, match only what comes after the →.\n\
- This tool can read PDF files (.pdf), PowerPoint files (.pptx), and image files (PNG, JPG, GIF, WebP, …).\n\
- When reading an image, the contents are presented visually via multimodal images.\n\
- PDF: `pages` selects a page range (required when the document has more than 10 pages; max 20 per call). `format` is `text` (default, extract text) or `image` (page rasterisation, deferred until MSRV allows pdf_oxide rendering; currently returns an error).\n\
- Binary office formats like .docx / .xlsx are rejected — use an external converter.";

/// Default max lines when the model omits `limit` (grok `MAX_LINES_READ`).
const MAX_LINES_READ: usize = 1_000;

/// Unified entry: content string + optional multimodal images.
pub(crate) async fn execute(args: &str) -> (String, Vec<UserImage>) {
    match execute_inner(args).await {
        Ok(pair) => pair,
        Err(msg) => (msg, Vec::new()),
    }
}

async fn execute_inner(args: &str) -> Result<(String, Vec<UserImage>), String> {
    let v = parse_args(args);
    let target = str_field(&v, &["target_file", "path"])
        .ok_or_else(|| "Error: target_file is required".to_string())?;
    let path = resolve(&target);
    let path_display = path.display().to_string();

    let file_bytes =
        std::fs::read(&path).map_err(|e| format!("Error reading {path_display}: {e}"))?;

    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    // 1. Image (magic first, then extension).
    if image::is_image_magic(&file_bytes) || image::is_image_extension(&extension) {
        let mime = crate::tools::tool_images::sniff_mime(&file_bytes);
        // Extension-claimed but not magic: still try compress (bmp/tiff/ico).
        let mime = if image::is_image_magic(&file_bytes) {
            mime
        } else {
            match extension.as_str() {
                "png" => "image/png",
                "jpg" | "jpeg" => "image/jpeg",
                "gif" => "image/gif",
                "webp" => "image/webp",
                "bmp" => "image/bmp",
                "tif" | "tiff" => "image/tiff",
                "ico" => "image/x-icon",
                _ => mime,
            }
        };
        return image::image_read_output(&path_display, file_bytes, mime)
            .map_err(|e| format!("Error: {e}"));
    }

    // 2. PDF
    if pdf::is_pdf_file(&file_bytes, &extension) {
        let pages = str_field(&v, &["pages"]);
        let format = str_field(&v, &["format"]);
        return pdf::handle_pdf(
            &path_display,
            file_bytes,
            pages.as_deref(),
            format.as_deref(),
        )
        .await
        .map_err(|e| format!("Error: {e}"));
    }

    // 3. PPTX
    if pptx::is_pptx_extension(&extension) {
        let content = pptx::handle_pptx(&path_display, file_bytes)
            .await
            .map_err(|e| format!("Error: {e}"))?;
        return Ok((content, Vec::new()));
    }

    // 4. Binary gate
    if binary::is_binary(&extension, &file_bytes) {
        return Err(format!("Error: Cannot read binary file: {path_display}"));
    }

    // 5. Text
    let text = String::from_utf8_lossy(&file_bytes).into_owned();
    let offset = int_field(&v, "offset").unwrap_or(1).max(1) as usize;
    let limit = int_field(&v, "limit")
        .map(|n| n.max(1) as usize)
        .unwrap_or(MAX_LINES_READ);
    Ok((page_text(&path_display, &text, offset, limit), Vec::new()))
}

fn page_text(path_display: &str, text: &str, offset: usize, limit: usize) -> String {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let total = lines.len();
    let start = offset.saturating_sub(1).min(total);
    let end = (start + limit).min(total);
    let slice = &lines[start..end];
    if slice.is_empty() {
        return format!("{path_display}: no lines in range (file has {total} lines)");
    }
    let mut out: String = slice
        .iter()
        .enumerate()
        .map(|(i, line)| {
            let n = start + i + 1;
            if n == 1 || n.is_multiple_of(10) {
                format!("{n}→{line}")
            } else {
                (*line).to_string()
            }
        })
        .collect();
    if end < total {
        let shown = end - start;
        let next = end + 1;
        let remaining = total - end;
        out.push_str(&format!(
            "\n\n[… truncated: showed lines {}-{end} ({shown} of {total}). \
             {remaining} lines remain — call read_file again with offset={next} \
             and a limit, or raise limit.]",
            start + 1
        ));
    }
    out
}

pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "read_file".into(),
        description: READ_FILE_DESC.into(),
        parameters_json: READ_FILE_PARAMS.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn text_paging() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "a\nb\nc\nd\n").unwrap();
        let args = serde_json::json!({
            "target_file": path,
            "offset": 2,
            "limit": 2,
        })
        .to_string();
        let (content, images) = execute(&args).await;
        assert!(images.is_empty());
        assert!(content.contains("b\n"), "{content}");
        assert!(content.contains("c\n"), "{content}");
        assert!(!content.contains("d\n"), "{content}");
    }

    #[tokio::test]
    async fn rejects_docx() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.docx");
        std::fs::write(&path, b"PK\x03\x04fake").unwrap();
        let args = serde_json::json!({ "target_file": path }).to_string();
        let (content, _) = execute(&args).await;
        assert!(content.contains("Cannot read binary file"), "{content}");
    }

    #[tokio::test]
    async fn pdf_text_smoke() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.pdf");
        std::fs::write(&path, pdf::make_test_pdf(&["Dock PDF"])).unwrap();
        let args = serde_json::json!({
            "target_file": path,
            "format": "text",
        })
        .to_string();
        let (content, images) = execute(&args).await;
        assert!(images.is_empty());
        assert!(content.contains("Dock PDF"), "{content}");
    }

    #[tokio::test]
    async fn image_png_smoke() {
        use ::image::{ImageBuffer, ImageFormat, Rgba};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shot.png");
        let img = ImageBuffer::from_pixel(32, 32, Rgba([10u8, 20, 30, 255]));
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, ImageFormat::Png).unwrap();
        std::fs::write(&path, buf.into_inner()).unwrap();
        let args = serde_json::json!({ "target_file": path }).to_string();
        let (content, images) = execute(&args).await;
        assert!(
            content.contains(crate::tools::tool_images::IMAGE_INLINE_PLACEHOLDER),
            "{content}"
        );
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].mime, "image/png");
    }
}
