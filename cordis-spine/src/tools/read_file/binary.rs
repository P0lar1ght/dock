//! Binary-file gate for `read_file` (aligned with Grok `util/binary.rs`).
//!
//! PDF / PPTX / images are handled *before* this gate in the read order, so
//! their extensions are either absent from this list (pdf, pptx) or present
//! but unreachable once magic/extension routing has claimed them (png, …).

/// Known binary extensions — skip content reading for these.
/// PDF and PPTX are intentionally excluded (dedicated handlers).
pub const BINARY_EXTENSIONS: &[&str] = &[
    "7z", "a", "avi", "avif", "bin", "bmp", "class", "dat", "dll", "doc", "docx", "dylib", "exe",
    "gif", "gz", "ico", "jar", "jpeg", "jpg", "lib", "mov", "mp3", "mp4", "o", "obj", "odp", "ods",
    "odt", "png", "ppt", "pyc", "pyd", "pyo", "qoi", "rar", "so", "tar", "tif", "tiff", "war",
    "wasm", "webp", "xls", "xlsx", "zip",
];

const SAMPLE_SIZE: usize = 8192;
const NON_PRINTABLE_THRESHOLD: f64 = 0.3;

/// `true` when the file should be treated as binary (extension or content).
pub fn is_binary(extension: &str, bytes: &[u8]) -> bool {
    if BINARY_EXTENSIONS.binary_search(&extension).is_ok() {
        return true;
    }
    if bytes.is_empty() {
        return false;
    }

    let sample = &bytes[..bytes.len().min(SAMPLE_SIZE)];

    if sample.contains(&0x00) {
        return true;
    }

    let non_printable = sample
        .iter()
        .filter(|&&b| b < 9 || (14..=31).contains(&b))
        .count();
    let ratio = non_printable as f64 / sample.len() as f64;
    ratio > NON_PRINTABLE_THRESHOLD
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_sorted() {
        let mut sorted = BINARY_EXTENSIONS.to_vec();
        sorted.sort();
        assert_eq!(BINARY_EXTENSIONS, &sorted[..]);
    }

    #[test]
    fn pdf_pptx_not_listed() {
        assert!(!BINARY_EXTENSIONS.contains(&"pdf"));
        assert!(!BINARY_EXTENSIONS.contains(&"pptx"));
    }

    #[test]
    fn rejects_docx() {
        assert!(is_binary("docx", &[]));
    }

    #[test]
    fn null_byte_is_binary() {
        assert!(is_binary("txt", b"ab\0cd"));
    }

    #[test]
    fn plain_text_ok() {
        assert!(!is_binary("rs", b"fn main() {}\n"));
    }
}
