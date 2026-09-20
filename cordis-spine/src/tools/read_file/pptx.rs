//! PPTX text extraction (zip + DrawingML), ported from Grok `read_file/pptx.rs`.
//!
//! Output: `--- Slide N ---` headers, one line per paragraph, optional
//! `Speaker Notes:` section. Slides ordered numerically; notes matched by
//! slide number.

use std::io::{Cursor, Read};
use std::time::Duration;

use quick_xml::events::Event;
use quick_xml::Reader;
use zip::ZipArchive;

const MAX_PPTX_BYTES: usize = 50 * 1024 * 1024;
const MAX_XML_ENTRY_BYTES: u64 = 64 * 1024 * 1024;
const PPTX_PROCESS_TIMEOUT: Duration = Duration::from_secs(60);

pub fn is_pptx_extension(ext: &str) -> bool {
    ext == "pptx"
}

/// Extract plain text from PPTX bytes.
pub fn extract_pptx_text_from_bytes(bytes: &[u8]) -> Result<String, String> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| format!("Failed to open PPTX archive: {e}"))?;

    let mut slide_numbers: Vec<u32> = archive
        .file_names()
        .filter_map(|name| {
            name.strip_prefix("ppt/slides/slide")?
                .strip_suffix(".xml")?
                .parse()
                .ok()
        })
        .collect();
    slide_numbers.sort_unstable();

    if slide_numbers.is_empty() {
        return Err("No slides found in PPTX".to_string());
    }

    let mut all_text = String::new();
    for number in slide_numbers {
        let slide_xml = read_entry(&mut archive, &format!("ppt/slides/slide{number}.xml"))?
            .ok_or_else(|| format!("Failed to read slide {number}"))?;
        let slide_text = extract_drawingml_text(&slide_xml)
            .map_err(|e| format!("Error parsing slide {number}: {e}"))?;

        let notes_text = read_entry(
            &mut archive,
            &format!("ppt/notesSlides/notesSlide{number}.xml"),
        )
        .ok()
        .flatten()
        .and_then(|xml| extract_drawingml_text(&xml).ok())
        .unwrap_or_default();

        if !all_text.is_empty() {
            all_text.push_str("\n\n");
        }
        all_text.push_str(&format!("--- Slide {number} ---\n"));
        all_text.push_str(&slide_text);
        if !notes_text.is_empty() {
            all_text.push_str("\n\nSpeaker Notes:\n");
            all_text.push_str(&notes_text);
        }
    }

    Ok(all_text)
}

fn read_entry(
    archive: &mut ZipArchive<Cursor<&[u8]>>,
    name: &str,
) -> Result<Option<String>, String> {
    let file = match archive.by_name(name) {
        Ok(file) => file,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(e) => return Err(format!("Failed to open {name}: {e}")),
    };
    let mut content = String::new();
    file.take(MAX_XML_ENTRY_BYTES)
        .read_to_string(&mut content)
        .map_err(|e| format!("Failed to read {name}: {e}"))?;
    if content.len() as u64 == MAX_XML_ENTRY_BYTES {
        return Err(format!("{name} exceeds the decompressed size limit"));
    }
    Ok(Some(content))
}

fn extract_drawingml_text(xml: &str) -> Result<String, String> {
    let mut reader = Reader::from_str(xml);

    let mut text = String::new();
    let mut in_text_run = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) if e.local_name().as_ref() == b"t" => in_text_run = true,
            Ok(Event::Text(e)) if in_text_run => {
                let content = e.decode().map_err(|e| e.to_string())?;
                text.push_str(&content);
            }
            Ok(Event::GeneralRef(e)) if in_text_run => {
                if let Some(ch) = e.resolve_char_ref().map_err(|e| e.to_string())? {
                    text.push(ch);
                } else {
                    let name = e.decode().map_err(|e| e.to_string())?;
                    match quick_xml::escape::resolve_predefined_entity(&name) {
                        Some(resolved) => text.push_str(resolved),
                        None => {
                            text.push('&');
                            text.push_str(&name);
                            text.push(';');
                        }
                    }
                }
            }
            Ok(Event::End(ref e)) => match e.local_name().as_ref() {
                b"t" => in_text_run = false,
                b"p" if !text.is_empty() && !text.ends_with('\n') => text.push('\n'),
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(e) => return Err(e.to_string()),
            _ => {}
        }
    }
    Ok(text.trim().to_string())
}

fn line_number_text(raw: &str) -> String {
    use std::fmt::Write as _;
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

pub async fn handle_pptx(path_display: &str, file_bytes: Vec<u8>) -> Result<String, String> {
    if file_bytes.len() > MAX_PPTX_BYTES {
        return Err(format!(
            "PPTX file is {:.1} MB, exceeds the {:.0} MB limit.",
            file_bytes.len() as f64 / 1_048_576.0,
            MAX_PPTX_BYTES as f64 / 1_048_576.0,
        ));
    }

    let path_owned = path_display.to_owned();
    let result = tokio::time::timeout(
        PPTX_PROCESS_TIMEOUT,
        tokio::task::spawn_blocking(move || {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let raw = extract_pptx_text_from_bytes(&file_bytes)
                    .map_err(|e| format!("Failed to extract text from PPTX: {e}"))?;
                Ok::<_, String>(line_number_text(&raw))
            }))
        }),
    )
    .await;

    match result {
        Ok(Ok(Ok(Ok(content)))) => Ok(content),
        Ok(Ok(Ok(Err(e)))) => Err(e),
        Ok(Ok(Err(_))) => Err(format!(
            "PPTX processing failed (internal error): {path_owned}"
        )),
        Ok(Err(e)) => Err(format!("PPTX processing failed: {e}")),
        Err(_) => Err(format!(
            "PPTX processing timed out after {}s: {path_owned}",
            PPTX_PROCESS_TIMEOUT.as_secs()
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    fn build_zip(entries: &[(&str, &str)]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        for (name, content) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(content.as_bytes()).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn slide_xml(paragraphs: &[&str]) -> String {
        let body: String = paragraphs
            .iter()
            .map(|p| format!("<a:p><a:r><a:t>{p}</a:t></a:r></a:p>"))
            .collect();
        format!(
            r#"<?xml version="1.0"?><p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree><p:sp><p:txBody>{body}</p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#
        )
    }

    #[test]
    fn extracts_slides_in_numeric_order_with_notes() {
        let s1 = slide_xml(&["Title", "Body line"]);
        let s2 = slide_xml(&["Second"]);
        let s10 = slide_xml(&["Tenth"]);
        let notes2 = slide_xml(&["A note"]);
        let bytes = build_zip(&[
            ("[Content_Types].xml", "<Types/>"),
            ("ppt/slides/slide10.xml", &s10),
            ("ppt/slides/slide1.xml", &s1),
            ("ppt/slides/slide2.xml", &s2),
            ("ppt/notesSlides/notesSlide2.xml", &notes2),
        ]);

        let text = extract_pptx_text_from_bytes(&bytes).unwrap();
        assert_eq!(
            text,
            "--- Slide 1 ---\nTitle\nBody line\n\n--- Slide 2 ---\nSecond\n\nSpeaker Notes:\nA note\n\n--- Slide 10 ---\nTenth"
        );
    }

    #[test]
    fn split_runs_concatenate() {
        let slide = r#"<p:sld xmlns:a="a" xmlns:p="p"><a:p><a:r><a:t>Hel</a:t></a:r><a:r><a:t>lo &amp; bye</a:t></a:r></a:p></p:sld>"#;
        let bytes = build_zip(&[("ppt/slides/slide1.xml", slide)]);
        let text = extract_pptx_text_from_bytes(&bytes).unwrap();
        assert_eq!(text, "--- Slide 1 ---\nHello & bye");
    }
}
