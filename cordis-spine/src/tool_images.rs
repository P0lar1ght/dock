//! Tool-produced images → [`UserImage`] for multimodal sample payloads.
//!
//! Caps mirror Grok MCP extract (5 images / ~5–10 MiB). Persist stores paths
//! only — never megabyte base64 in transcript JSON.

use std::path::Path;
use std::sync::Arc;

use crate::types::UserImage;

/// Cap images attached to one tool result (Grok `MAX_IMAGES`).
pub const MAX_TOOL_IMAGES: usize = 5;
/// Soft max decoded bytes per image before we refuse to attach.
pub const MAX_TOOL_IMAGE_BYTES: usize = 5 * 1024 * 1024;
/// Skip tiny decorative icons.
pub const MIN_TOOL_IMAGE_BYTES: usize = 32;
/// Placeholder text when pixels ride separately (Grok read_file / MCP).
pub const IMAGE_INLINE_PLACEHOLDER: &str = "Image content included inline";
/// Stand-in when a data-URI was stripped from MCP text.
pub const IMAGE_SEPARATE_PLACEHOLDER: &str = "[image content will be provided separately]";

/// Build a [`UserImage`] from raw bytes. Returns `None` when oversize / empty /
/// unreadable. `mime_hint` wins when sniffing fails.
pub fn user_image_from_bytes(data: Vec<u8>, mime_hint: Option<&str>) -> Option<UserImage> {
    if data.len() < MIN_TOOL_IMAGE_BYTES || data.len() > MAX_TOOL_IMAGE_BYTES {
        return None;
    }
    let mime = mime_hint
        .filter(|m| m.starts_with("image/"))
        .map(|m| m.to_string())
        .unwrap_or_else(|| sniff_mime(&data).to_string());
    let (width, height) = dimensions(&data, &mime);
    Some(UserImage {
        mime,
        data: Arc::from(data.into_boxed_slice()),
        width,
        height,
    })
}

pub fn user_image_from_path(path: &Path) -> Option<UserImage> {
    let data = std::fs::read(path).ok()?;
    let hint = path.extension().and_then(|e| e.to_str()).map(ext_mime);
    user_image_from_bytes(data, hint)
}

pub fn user_image_from_base64(b64: &str, mime: &str) -> Option<UserImage> {
    use base64::Engine;
    let data = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(b64.trim()))
        .ok()?;
    user_image_from_bytes(data, Some(mime))
}

/// Keep at most [`MAX_TOOL_IMAGES`], dropping the rest.
pub fn cap_images(mut images: Vec<UserImage>) -> Vec<UserImage> {
    if images.len() > MAX_TOOL_IMAGES {
        images.truncate(MAX_TOOL_IMAGES);
    }
    images
}

fn ext_mime(ext: &str) -> &'static str {
    match ext.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => "image/png",
    }
}

pub fn sniff_mime(data: &[u8]) -> &'static str {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if data.starts_with(b"\xff\xd8\xff") {
        "image/jpeg"
    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        "image/gif"
    } else if data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WEBP") {
        "image/webp"
    } else {
        "image/png"
    }
}

pub fn dimensions(data: &[u8], mime: &str) -> (u32, u32) {
    match mime {
        "image/png" => png_size(data),
        "image/jpeg" => jpeg_size(data),
        "image/gif" => gif_size(data),
        "image/webp" => webp_size(data),
        _ => png_size(data)
            .or_else(|| jpeg_size(data))
            .or_else(|| gif_size(data))
            .or_else(|| webp_size(data)),
    }
    .unwrap_or((0, 0))
}

fn png_size(data: &[u8]) -> Option<(u32, u32)> {
    if data.len() < 24 || !data.starts_with(b"\x89PNG\r\n\x1a\n") {
        return None;
    }
    let w = u32::from_be_bytes(data[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(data[20..24].try_into().ok()?);
    Some((w, h))
}

fn gif_size(data: &[u8]) -> Option<(u32, u32)> {
    if data.len() < 10 {
        return None;
    }
    if !(data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a")) {
        return None;
    }
    let w = u16::from_le_bytes(data[6..8].try_into().ok()?) as u32;
    let h = u16::from_le_bytes(data[8..10].try_into().ok()?) as u32;
    Some((w, h))
}

fn webp_size(data: &[u8]) -> Option<(u32, u32)> {
    if data.len() < 30 || !data.starts_with(b"RIFF") || data.get(8..12) != Some(b"WEBP") {
        return None;
    }
    // VP8X
    if data.get(12..16) == Some(b"VP8X") && data.len() >= 30 {
        let w = 1 + u32::from_le_bytes([data[24], data[25], data[26], 0]);
        let h = 1 + u32::from_le_bytes([data[27], data[28], data[29], 0]);
        return Some((w, h));
    }
    // VP8 lossy
    if data.get(12..15) == Some(b"VP8") && data.get(15) == Some(&b' ') && data.len() >= 30 {
        let w = u16::from_le_bytes(data[26..28].try_into().ok()?) as u32 & 0x3fff;
        let h = u16::from_le_bytes(data[28..30].try_into().ok()?) as u32 & 0x3fff;
        return Some((w, h));
    }
    None
}

fn jpeg_size(data: &[u8]) -> Option<(u32, u32)> {
    if data.len() < 4 || !data.starts_with(b"\xff\xd8\xff") {
        return None;
    }
    let mut i = 2usize;
    while i + 9 < data.len() {
        if data[i] != 0xff {
            return None;
        }
        while i < data.len() && data[i] == 0xff {
            i += 1;
        }
        if i >= data.len() {
            return None;
        }
        let marker = data[i];
        i += 1;
        if marker == 0xd9 || marker == 0xda {
            return None;
        }
        if i + 1 >= data.len() {
            return None;
        }
        let len = u16::from_be_bytes([data[i], data[i + 1]]) as usize;
        if len < 2 || i + len > data.len() {
            return None;
        }
        // SOF0–SOF3 / SOF5–SOF7 / SOF9–SOF11 / SOF13–SOF15
        if matches!(
            marker,
            0xc0 | 0xc1
                | 0xc2
                | 0xc3
                | 0xc5
                | 0xc6
                | 0xc7
                | 0xc9
                | 0xca
                | 0xcb
                | 0xcd
                | 0xce
                | 0xcf
        ) && len >= 7
        {
            let h = u16::from_be_bytes([data[i + 3], data[i + 4]]) as u32;
            let w = u16::from_be_bytes([data[i + 5], data[i + 6]]) as u32;
            return Some((w, h));
        }
        i += len;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_png() -> Vec<u8> {
        // 1x1 PNG
        vec![
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00,
            0x00, 0x90, 0x77, 0x53, 0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x08,
            0xd7, 0x63, 0xf8, 0xff, 0xff, 0x3f, 0x00, 0x05, 0xfe, 0x02, 0xfe, 0xa7, 0x35, 0x81,
            0x84, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
        ]
    }

    #[test]
    fn png_round_trip_user_image() {
        let img = user_image_from_bytes(tiny_png(), Some("image/png")).expect("png");
        assert_eq!(img.mime, "image/png");
        assert_eq!((img.width, img.height), (1, 1));
    }

    #[test]
    fn rejects_empty() {
        assert!(user_image_from_bytes(vec![], None).is_none());
    }

    #[test]
    fn caps_at_five() {
        let one = user_image_from_bytes(tiny_png(), None).unwrap();
        let many = (0..8).map(|_| one.clone()).collect::<Vec<_>>();
        assert_eq!(cap_images(many).len(), MAX_TOOL_IMAGES);
    }

    #[test]
    fn tool_result_images_round_trip() {
        use crate::tools::tool_result_with_images;
        use crate::types::ToolCall;
        let img = user_image_from_bytes(tiny_png(), Some("image/png")).unwrap();
        let call = ToolCall {
            id: "c1".into(),
            name: "read_file".into(),
            arguments: "{}".into(),
        };
        let result = tool_result_with_images(call, IMAGE_INLINE_PLACEHOLDER, vec![img.clone()]);
        assert_eq!(result.images.len(), 1);
        assert_eq!(result.images[0].data, img.data);
        assert_eq!(result.content, IMAGE_INLINE_PLACEHOLDER);
    }
}
