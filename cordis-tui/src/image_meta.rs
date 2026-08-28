//! PNG / JPEG / TIFF header size probe (no `image` crate).

pub fn dimensions(data: &[u8], mime: &str) -> (u32, u32) {
    match mime {
        "image/png" => png_size(data),
        "image/jpeg" => jpeg_size(data),
        "image/tiff" => tiff_size(data),
        "image/gif" => gif_size(data),
        "image/webp" => webp_size(data),
        _ => png_size(data)
            .or_else(|| jpeg_size(data))
            .or_else(|| tiff_size(data))
            .or_else(|| gif_size(data))
            .or_else(|| webp_size(data)),
    }
    .unwrap_or((0, 0))
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
    } else if data.starts_with(b"II*\0") || data.starts_with(b"MM\0*") {
        "image/tiff"
    } else {
        "image/png"
    }
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
    let kind = data.get(12..16)?;
    if kind == b"VP8X" && data.len() >= 30 {
        let w = 1 + u32::from_le_bytes([data[24], data[25], data[26], 0]);
        let h = 1 + u32::from_le_bytes([data[27], data[28], data[29], 0]);
        return Some((w, h));
    }
    if kind == b"VP8 " && data.len() >= 30 {
        let w = u16::from_le_bytes(data[26..28].try_into().ok()?) as u32 & 0x3fff;
        let h = u16::from_le_bytes(data[28..30].try_into().ok()?) as u32 & 0x3fff;
        return Some((w, h));
    }
    None
}

fn jpeg_size(data: &[u8]) -> Option<(u32, u32)> {
    if data.len() < 4 || data[0] != 0xff || data[1] != 0xd8 {
        return None;
    }
    let mut i = 2usize;
    while i + 8 < data.len() {
        if data[i] != 0xff {
            i += 1;
            continue;
        }
        let marker = data[i + 1];
        i += 2;
        if marker == 0xd8 || marker == 0xd9 || (0xd0..=0xd7).contains(&marker) {
            continue;
        }
        if i + 2 > data.len() {
            break;
        }
        let len = u16::from_be_bytes([data[i], data[i + 1]]) as usize;
        if matches!(marker, 0xc0 | 0xc1 | 0xc2) && i + 7 <= data.len() {
            let h = u16::from_be_bytes([data[i + 3], data[i + 4]]) as u32;
            let w = u16::from_be_bytes([data[i + 5], data[i + 6]]) as u32;
            return Some((w, h));
        }
        i += len;
    }
    None
}

fn tiff_size(data: &[u8]) -> Option<(u32, u32)> {
    if data.len() < 8 {
        return None;
    }
    let le = match &data[0..4] {
        b"II*\0" => true,
        b"MM\0*" => false,
        _ => return None,
    };
    let read_u16 = |at: usize| -> Option<u16> {
        let b: [u8; 2] = data.get(at..at + 2)?.try_into().ok()?;
        Some(if le {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    };
    let read_u32 = |at: usize| -> Option<u32> {
        let b: [u8; 4] = data.get(at..at + 4)?.try_into().ok()?;
        Some(if le {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    };
    let ifd = read_u32(4)? as usize;
    let count = read_u16(ifd)? as usize;
    let mut width = 0u32;
    let mut height = 0u32;
    for i in 0..count {
        let entry = ifd + 2 + i * 12;
        let tag = read_u16(entry)?;
        let typ = read_u16(entry + 2)?;
        let val_at = entry + 8;
        let value = match typ {
            3 => read_u16(val_at)? as u32,
            _ => read_u32(val_at)?,
        };
        match tag {
            256 => width = value,
            257 => height = value,
            _ => {}
        }
    }
    (width > 0 && height > 0).then_some((width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_ihdr() {
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        png.extend_from_slice(&[0, 0, 0, 13]);
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&3302u32.to_be_bytes());
        png.extend_from_slice(&962u32.to_be_bytes());
        png.extend_from_slice(&[8, 2, 0, 0, 0]);
        assert_eq!(png_size(&png), Some((3302, 962)));
        assert_eq!(sniff_mime(&png), "image/png");
    }
}
