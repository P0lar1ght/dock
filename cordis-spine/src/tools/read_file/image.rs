//! Image detect + compress for conversation embedding (Grok `read_file/image.rs` budgets).

use std::io::Cursor;

use cordis_base::types::UserImage;
use image::imageops::FilterType;
use image::{DynamicImage, ImageFormat, ImageReader};

use crate::tools::tool_images::{
    self, IMAGE_INLINE_PLACEHOLDER, MAX_TOOL_IMAGE_BYTES, MIN_TOOL_IMAGE_BYTES,
};

/// Max base64 size for an image embedded in the conversation (Grok).
pub const MAX_IMAGE_PAYLOAD_BYTES: usize = 768 * 1024;
/// Total pixel budget (w*h); preserves 1024×1024 as an aspect-agnostic area.
const MAX_IMAGE_PIXELS: u64 = 1_048_576;
/// Max side (width or height).
const MAX_IMAGE_DIMENSION: u32 = 2000;
/// Floor dimension — give up when `max_side` falls to or below this.
const MIN_IMAGE_DIMENSION: u32 = 128;
/// JPEG quality ladder.
const QUALITY_STEPS: &[u8] = &[85, 70, 50, 40];
/// Absolute upper bound on decoded pixels before refuse (API ceiling).
const MAX_DECODE_PIXELS: u64 = 178_956_970;

#[derive(Debug)]
pub enum CompressImageError {
    PixelLimitExceeded {
        width: u32,
        height: u32,
        limit_pixels: u64,
    },
    PayloadCapExceeded(usize),
    FormatDetectionFailed,
    DecodeFailed(String),
}

impl std::fmt::Display for CompressImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PixelLimitExceeded {
                width,
                height,
                limit_pixels,
            } => write!(
                f,
                "image dimensions {width}x{height} exceed the {limit_pixels} pixel decode limit"
            ),
            Self::PayloadCapExceeded(cap) => write!(
                f,
                "compressed image still exceeds the {cap}-byte conversation payload cap"
            ),
            Self::FormatDetectionFailed => write!(f, "image format could not be detected"),
            Self::DecodeFailed(msg) => write!(f, "image decode failed: {msg}"),
        }
    }
}

/// True when magic bytes look like a supported image (png/jpeg/gif/webp).
pub fn is_image_magic(bytes: &[u8]) -> bool {
    matches!(
        tool_images::sniff_mime(bytes),
        "image/png" | "image/jpeg" | "image/gif" | "image/webp"
    ) && (bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        || bytes.starts_with(b"\xff\xd8\xff")
        || bytes.starts_with(b"GIF87a")
        || bytes.starts_with(b"GIF89a")
        || (bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP")))
}

pub fn is_image_extension(ext: &str) -> bool {
    matches!(
        ext,
        "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "tif" | "tiff" | "ico"
    )
}

/// Compress then wrap as [`UserImage`] + placeholder text for the tool result.
pub fn image_read_output(
    path_display: &str,
    file_bytes: Vec<u8>,
    mime_hint: &str,
) -> Result<(String, Vec<UserImage>), String> {
    let (encoded, mime) = compress_image_for_conversation(file_bytes, mime_hint.to_string())
        .map_err(|e| format!("Could not embed image in conversation: {e}"))?;
    let img = tool_images::user_image_from_bytes(encoded, Some(&mime)).ok_or_else(|| {
        format!(
            "Could not embed image in conversation: compressed payload outside tool image size bounds ({MIN_TOOL_IMAGE_BYTES}..{MAX_TOOL_IMAGE_BYTES} bytes)"
        )
    })?;
    let content = format!("{path_display}\n{IMAGE_INLINE_PLACEHOLDER}");
    Ok((content, vec![img]))
}

/// Resize / re-encode so base64 stays under [`MAX_IMAGE_PAYLOAD_BYTES`].
pub fn compress_image_for_conversation(
    raw_bytes: Vec<u8>,
    original_mime: String,
) -> Result<(Vec<u8>, String), CompressImageError> {
    // GIF/BMP/TIFF/ICO → PNG first (endpoint prefers jpeg/png/webp).
    let (raw_bytes, original_mime) = match ImageFormat::from_mime_type(&original_mime)
        .or_else(|| image::guess_format(&raw_bytes).ok())
    {
        Some(
            fmt @ (ImageFormat::Gif | ImageFormat::Bmp | ImageFormat::Tiff | ImageFormat::Ico),
        ) => {
            let img = decode_checked(&raw_bytes)?;
            let mut png = Vec::new();
            img.write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
                .map_err(|e| CompressImageError::DecodeFailed(format!("transcode {fmt:?}: {e}")))?;
            (png, "image/png".to_string())
        }
        _ => (raw_bytes, original_mime),
    };

    let dims = ImageReader::new(Cursor::new(&raw_bytes))
        .with_guessed_format()
        .ok()
        .and_then(|r| r.into_dimensions().ok());

    let within_pixel_budget = dims.is_none_or(|(w, h)| {
        w <= MAX_IMAGE_DIMENSION
            && h <= MAX_IMAGE_DIMENSION
            && u64::from(w) * u64::from(h) <= MAX_IMAGE_PIXELS
    });

    let passthrough_sendable = matches!(
        image::guess_format(&raw_bytes),
        Ok(ImageFormat::Jpeg | ImageFormat::Png | ImageFormat::WebP)
    );

    if (raw_bytes.len() * 4).div_ceil(3) <= MAX_IMAGE_PAYLOAD_BYTES
        && within_pixel_budget
        && passthrough_sendable
    {
        return Ok((raw_bytes, original_mime));
    }

    let img = decode_checked(&raw_bytes)?;
    re_encode_under_limit(&img)
}

fn decode_checked(raw_bytes: &[u8]) -> Result<DynamicImage, CompressImageError> {
    let reader = ImageReader::new(Cursor::new(raw_bytes))
        .with_guessed_format()
        .map_err(|_| CompressImageError::FormatDetectionFailed)?;
    if reader.format().is_none() {
        return Err(CompressImageError::FormatDetectionFailed);
    }
    if let Ok((w, h)) = ImageReader::new(Cursor::new(raw_bytes))
        .with_guessed_format()
        .map_err(|_| ())
        .and_then(|r| r.into_dimensions().map_err(|_| ()))
    {
        if u64::from(w) * u64::from(h) > MAX_DECODE_PIXELS {
            return Err(CompressImageError::PixelLimitExceeded {
                width: w,
                height: h,
                limit_pixels: MAX_DECODE_PIXELS,
            });
        }
    }
    ImageReader::new(Cursor::new(raw_bytes))
        .with_guessed_format()
        .map_err(|_| CompressImageError::FormatDetectionFailed)?
        .decode()
        .map_err(|e| CompressImageError::DecodeFailed(e.to_string()))
}

fn re_encode_under_limit(img: &DynamicImage) -> Result<(Vec<u8>, String), CompressImageError> {
    let (orig_w, orig_h) = (img.width(), img.height());
    let mut max_side = MAX_IMAGE_DIMENSION.min(orig_w.max(orig_h));

    // Also shrink by area budget.
    let area = u64::from(orig_w) * u64::from(orig_h);
    if area > MAX_IMAGE_PIXELS {
        let scale = (MAX_IMAGE_PIXELS as f64 / area as f64).sqrt();
        let by_area = ((orig_w.max(orig_h) as f64) * scale).floor() as u32;
        max_side = max_side.min(by_area.max(1));
    }

    loop {
        let (tw, th) = fit_side(orig_w, orig_h, max_side);
        let resized = if tw == orig_w && th == orig_h {
            img.clone()
        } else {
            img.resize(tw, th, FilterType::Lanczos3)
        };

        for &q in QUALITY_STEPS {
            let mut jpeg = Vec::new();
            let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, q);
            if encoder.encode_image(&resized).is_ok()
                && (jpeg.len() * 4).div_ceil(3) <= MAX_IMAGE_PAYLOAD_BYTES
            {
                return Ok((jpeg, "image/jpeg".to_string()));
            }
        }

        // Try PNG as a last resort at this size (flat UI screenshots).
        let mut png = Vec::new();
        if resized
            .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
            .is_ok()
            && (png.len() * 4).div_ceil(3) <= MAX_IMAGE_PAYLOAD_BYTES
        {
            return Ok((png, "image/png".to_string()));
        }

        if max_side <= MIN_IMAGE_DIMENSION {
            return Err(CompressImageError::PayloadCapExceeded(
                MAX_IMAGE_PAYLOAD_BYTES,
            ));
        }
        max_side = (max_side * 3 / 4).max(MIN_IMAGE_DIMENSION);
    }
}

fn fit_side(w: u32, h: u32, max_side: u32) -> (u32, u32) {
    let long = w.max(h);
    if long <= max_side {
        return (w, h);
    }
    let scale = max_side as f64 / long as f64;
    (
        ((w as f64) * scale).round().max(1.0) as u32,
        ((h as f64) * scale).round().max(1.0) as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_small_png(width: u32, height: u32) -> Vec<u8> {
        use ::image::{ImageBuffer, Rgba};
        let img = ImageBuffer::from_pixel(width, height, Rgba([0u8, 0, 0, 255]));
        let mut buf = Cursor::new(Vec::new());
        img.write_to(&mut buf, ImageFormat::Png).unwrap();
        buf.into_inner()
    }

    fn make_noisy_png(width: u32, height: u32) -> Vec<u8> {
        use ::image::{ImageBuffer, Rgba};
        let img = ImageBuffer::from_fn(width, height, |x, y| {
            let seed = (x as u64).wrapping_mul(6364136223846793005)
                ^ (y as u64).wrapping_mul(1442695040888963407);
            Rgba([
                (seed & 0xFF) as u8,
                ((seed >> 8) & 0xFF) as u8,
                ((seed >> 16) & 0xFF) as u8,
                255u8,
            ])
        });
        let mut buf = Cursor::new(Vec::new());
        img.write_to(&mut buf, ImageFormat::Png).unwrap();
        buf.into_inner()
    }

    #[test]
    fn magic_detects_png() {
        let png = make_small_png(8, 8);
        assert!(is_image_magic(&png));
        assert!(!is_image_magic(b"not an image"));
    }

    #[test]
    fn small_png_passthrough() {
        let png = make_small_png(16, 16);
        let (out, mime) = compress_image_for_conversation(png.clone(), "image/png".into()).unwrap();
        assert_eq!(out, png);
        assert_eq!(mime, "image/png");
    }

    #[test]
    fn large_noisy_becomes_jpeg_under_cap() {
        let png = make_noisy_png(2048, 1536);
        let b64_before = (png.len() * 4).div_ceil(3);
        assert!(b64_before > MAX_IMAGE_PAYLOAD_BYTES);
        let (out, mime) = compress_image_for_conversation(png, "image/png".into()).unwrap();
        assert_eq!(mime, "image/jpeg");
        assert!((out.len() * 4).div_ceil(3) <= MAX_IMAGE_PAYLOAD_BYTES);
    }

    #[test]
    fn large_flat_downscales_by_area() {
        let png = make_small_png(2048, 2600);
        let (out, _) = compress_image_for_conversation(png, "image/png".into()).unwrap();
        let (w, h) = ImageReader::new(Cursor::new(&out))
            .with_guessed_format()
            .unwrap()
            .into_dimensions()
            .unwrap();
        assert!(w <= MAX_IMAGE_DIMENSION && h <= MAX_IMAGE_DIMENSION);
        assert!(u64::from(w) * u64::from(h) <= MAX_IMAGE_PIXELS);
    }
}
