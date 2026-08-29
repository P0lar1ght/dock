//! Clipboard write (`/copy`) and paste (Grok Ctrl/Cmd+V + image probe).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

use crate::image_meta;

#[derive(Clone, Debug)]
pub struct ClipboardImage {
    pub mime: String,
    pub data: Arc<[u8]>,
    pub width: u32,
    pub height: u32,
}

impl ClipboardImage {
    pub fn from_bytes(data: Vec<u8>, mime_hint: Option<&str>) -> Option<Self> {
        if data.is_empty() {
            return None;
        }
        let mime = mime_hint
            .filter(|m| m.starts_with("image/"))
            .unwrap_or_else(|| image_meta::sniff_mime(&data))
            .to_string();
        let (width, height) = image_meta::dimensions(&data, &mime);
        Some(Self {
            mime,
            data: Arc::from(data),
            width,
            height,
        })
    }

    pub fn format_name(&self) -> &str {
        match self.mime.as_str() {
            "image/png" => "PNG",
            "image/jpeg" => "JPEG",
            "image/tiff" => "TIFF",
            "image/gif" => "GIF",
            "image/webp" => "WEBP",
            other => other,
        }
    }
}

pub enum PastePayload {
    Empty,
    Text(String),
    Image(ClipboardImage),
}

pub fn copy_text(text: &str) -> Result<(), String> {
    try_clipboard(text).or_else(|clip_err| {
        let path = PathBuf::from("grok-copy.txt");
        std::fs::write(&path, text).map_err(|e| format!("{clip_err}; file fallback: {e}"))?;
        Ok(())
    })
}

pub fn write_file(path: &Path, text: &str) -> Result<(), String> {
    std::fs::write(path, text).map_err(|e| e.to_string())
}

pub fn paste_text() -> Option<String> {
    let raw = read_clipboard_text()?;
    let text = normalize_cr(&raw);
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// Grok paste-key order: file/image path, else clipboard text, else raster.
pub fn paste_from_clipboard() -> PastePayload {
    let text = paste_text();
    if let Some(t) = text.as_deref() {
        if let Some(img) = image_from_path_or_url(t.trim()) {
            return PastePayload::Image(img);
        }
        if !t.trim().is_empty() {
            return PastePayload::Text(t.to_string());
        }
    }
    match paste_image() {
        Some(img) => PastePayload::Image(img),
        None => text.map(PastePayload::Text).unwrap_or(PastePayload::Empty),
    }
}

/// Bracketed `Event::Paste`: insert text unless it is an image path, or empty
/// (then probe the host clipboard for a screenshot).
pub fn paste_from_event(text: &str) -> PastePayload {
    let text = normalize_cr(text);
    if text.trim().is_empty() {
        return paste_from_clipboard();
    }
    if let Some(img) = image_from_path_or_url(text.trim()) {
        return PastePayload::Image(img);
    }
    PastePayload::Text(text)
}

pub fn paste_image() -> Option<ClipboardImage> {
    #[cfg(target_os = "macos")]
    {
        return macos_clipboard_image();
    }
    #[cfg(target_os = "linux")]
    {
        return linux_clipboard_image();
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

pub fn normalize_cr(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn image_from_path_or_url(raw: &str) -> Option<ClipboardImage> {
    let line = raw.lines().next()?.trim();
    if raw.lines().nth(1).is_some_and(|l| !l.trim().is_empty()) {
        return None;
    }
    let path = line
        .strip_prefix("file://")
        .map(urlencoding_decode_path)
        .unwrap_or_else(|| line.to_string());
    let path = Path::new(&path);
    if !is_image_ext(path) || !path.is_file() {
        return None;
    }
    let data = std::fs::read(path).ok()?;
    ClipboardImage::from_bytes(data, None)
}

fn urlencoding_decode_path(s: &str) -> String {
    percent_decode(s)
}

fn percent_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) =
                u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16)
            {
                out.push(v as char);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn is_image_ext(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "tif" | "tiff" | "bmp")
    )
}

fn read_clipboard_text() -> Option<String> {
    let candidates: &[&[&str]] = if cfg!(target_os = "macos") {
        &[&["pbpaste"]]
    } else if cfg!(target_os = "windows") {
        &[&["powershell", "-NoProfile", "-Command", "Get-Clipboard"]]
    } else {
        &[
            &["wl-paste", "--no-newline"],
            &["xclip", "-selection", "clipboard", "-o"],
        ]
    };
    for cmd in candidates {
        if let Some(text) = stdout_string(cmd) {
            return Some(text);
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn macos_clipboard_image() -> Option<ClipboardImage> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir();
    let png = dir.join(format!("dock-clip-{stamp}.png"));
    let tiff = dir.join(format!("dock-clip-{stamp}.tiff"));
    let jpg = dir.join(format!("dock-clip-{stamp}.jpg"));
    let script = format!(
        "try\n\
         set imgData to the clipboard as \u{00ab}class PNGf\u{00bb}\n\
         set filePath to POSIX file \"{png}\" as text\n\
         set fRef to open for access file filePath with write permission\n\
         set eof of fRef to 0\n\
         write imgData to fRef\n\
         close access fRef\n\
         return \"PNGf\"\n\
         on error\n\
         try\n\
         set imgData to the clipboard as \u{00ab}class TIFF\u{00bb}\n\
         set filePath to POSIX file \"{tiff}\" as text\n\
         set fRef to open for access file filePath with write permission\n\
         set eof of fRef to 0\n\
         write imgData to fRef\n\
         close access fRef\n\
         return \"TIFF\"\n\
         on error\n\
         try\n\
         set imgData to the clipboard as \u{00ab}class JPEG\u{00bb}\n\
         set filePath to POSIX file \"{jpg}\" as text\n\
         set fRef to open for access file filePath with write permission\n\
         set eof of fRef to 0\n\
         write imgData to fRef\n\
         close access fRef\n\
         return \"JPEG\"\n\
         on error\n\
         return \"none\"\n\
         end try\n\
         end try\n\
         end try",
        png = png.display(),
        tiff = tiff.display(),
        jpg = jpg.display(),
    );
    let class = Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())?;
    let class = class.trim();
    let (path, mime) = match class {
        "PNGf" => (png.clone(), "image/png"),
        "TIFF" => (tiff.clone(), "image/tiff"),
        "JPEG" => (jpg.clone(), "image/jpeg"),
        _ => {
            let _ = std::fs::remove_file(&png);
            let _ = std::fs::remove_file(&tiff);
            let _ = std::fs::remove_file(&jpg);
            return None;
        }
    };
    let data = std::fs::read(&path).ok();
    let _ = std::fs::remove_file(&png);
    let _ = std::fs::remove_file(&tiff);
    let _ = std::fs::remove_file(&jpg);
    ClipboardImage::from_bytes(data?, Some(mime))
}

#[cfg(target_os = "linux")]
fn linux_clipboard_image() -> Option<ClipboardImage> {
    for cmd in [
        &["wl-paste", "--type", "image/png"][..],
        &["xclip", "-selection", "clipboard", "-t", "image/png", "-o"][..],
    ] {
        if let Some(bytes) = stdout_bytes(cmd) {
            if let Some(img) = ClipboardImage::from_bytes(bytes, Some("image/png")) {
                return Some(img);
            }
        }
    }
    None
}

fn stdout_string(cmd: &[&str]) -> Option<String> {
    stdout_bytes(cmd).and_then(|b| String::from_utf8(b).ok())
}

fn stdout_bytes(cmd: &[&str]) -> Option<Vec<u8>> {
    let out = Command::new(cmd[0])
        .args(&cmd[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() || out.stdout.is_empty() {
        return None;
    }
    Some(out.stdout)
}

fn try_clipboard(text: &str) -> Result<(), String> {
    let candidates: &[&[&str]] = if cfg!(target_os = "macos") {
        &[&["pbcopy"]]
    } else if cfg!(target_os = "windows") {
        &[&["clip"]]
    } else {
        &[&["wl-copy"], &["xclip", "-selection", "clipboard"]]
    };
    let mut last = "no clipboard command".to_string();
    for cmd in candidates {
        match pipe_stdin(cmd, text) {
            Ok(()) => return Ok(()),
            Err(e) => last = e,
        }
    }
    Err(last)
}

fn pipe_stdin(cmd: &[&str], text: &str) -> Result<(), String> {
    let mut child = Command::new(cmd[0])
        .args(&cmd[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("{}: {e}", cmd[0]))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(text.as_bytes())
            .map_err(|e| format!("{}: {e}", cmd[0]))?;
    }
    let status = child.wait().map_err(|e| format!("{}: {e}", cmd[0]))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{} exited {status}", cmd[0]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_windows_newlines() {
        assert_eq!(normalize_cr("a\r\nb\rc"), "a\nb\nc");
    }
}
