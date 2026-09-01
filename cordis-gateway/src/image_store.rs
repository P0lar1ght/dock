//! Short-lived image uploads for `imageInputs/put` → `turn/start`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cordis_spine::UserImage;
use sha2::{Digest, Sha256};

use crate::protocol::RpcError;

pub const MAX_BYTES: usize = 2 * 1024 * 1024;
const MAX_PENDING: usize = 16;
const TTL: Duration = Duration::from_secs(120);

#[derive(Clone, Debug)]
pub struct StoredImage {
    pub digest: String,
    pub mime: String,
    pub width: u32,
    pub height: u32,
    pub data: Arc<[u8]>,
}

pub struct ImageInputStore {
    items: HashMap<String, (Instant, StoredImage)>,
}

impl ImageInputStore {
    pub fn new() -> Self {
        Self {
            items: HashMap::new(),
        }
    }

    pub fn put(
        &mut self,
        id: String,
        mime: String,
        width: u32,
        height: u32,
        digest: String,
        data: Vec<u8>,
    ) -> Result<StoredImage, RpcError> {
        self.gc();
        if !matches!(mime.as_str(), "image/png" | "image/jpeg" | "image/webp") {
            return Err(RpcError::app(
                "image_input_mime_unsupported",
                "Image must be PNG, JPEG, or WebP",
            ));
        }
        if data.is_empty() || data.len() > MAX_BYTES {
            return Err(RpcError::app(
                "image_input_too_large",
                "Normalized screenshot exceeds the image input limit",
            ));
        }
        if width < 1 || height < 1 || width > 4096 || height > 4096 {
            return Err(RpcError::app(
                "image_input_dimensions_exceeded",
                "Screenshot dimensions exceed the limit",
            ));
        }
        let got = hex(Sha256::digest(&data).as_slice());
        if got != digest {
            return Err(RpcError::app(
                "image_input_protocol_mismatch",
                "Gateway image input limits or digest do not match this SDK",
            ));
        }
        while self.items.len() >= MAX_PENDING {
            if let Some(oldest) = self
                .items
                .iter()
                .min_by_key(|(_, (at, _))| *at)
                .map(|(k, _)| k.clone())
            {
                self.items.remove(&oldest);
            } else {
                break;
            }
        }
        let stored = StoredImage {
            digest,
            mime,
            width,
            height,
            data: data.into(),
        };
        self.items.insert(id, (Instant::now(), stored.clone()));
        Ok(stored)
    }

    pub fn take(&mut self, id: &str, digest: &str) -> Result<StoredImage, RpcError> {
        self.gc();
        let Some((_, stored)) = self.items.remove(id) else {
            return Err(RpcError::app(
                "image_input_invalid",
                "Image input id is missing or expired",
            ));
        };
        if stored.digest != digest {
            return Err(RpcError::app(
                "image_input_protocol_mismatch",
                "Gateway image input limits or digest do not match this SDK",
            ));
        }
        Ok(stored)
    }

    fn gc(&mut self) {
        let now = Instant::now();
        self.items
            .retain(|_, (at, _)| now.duration_since(*at) < TTL);
    }
}

impl Default for ImageInputStore {
    fn default() -> Self {
        Self::new()
    }
}

impl From<StoredImage> for UserImage {
    fn from(value: StoredImage) -> Self {
        UserImage {
            mime: value.mime,
            data: value.data,
            width: value.width,
            height: value.height,
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png() -> Vec<u8> {
        vec![
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9C, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00,
            0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ]
    }

    #[test]
    fn put_rejects_digest_mismatch() {
        let mut store = ImageInputStore::new();
        let err = store
            .put(
                "img-1".into(),
                "image/png".into(),
                1,
                1,
                "deadbeef".into(),
                png(),
            )
            .unwrap_err();
        assert_eq!(err.details_code, "image_input_protocol_mismatch");
    }

    #[test]
    fn take_consumes_matching_digest() {
        let mut store = ImageInputStore::new();
        let data = png();
        let digest = hex(Sha256::digest(&data).as_slice());
        store
            .put(
                "img-1".into(),
                "image/png".into(),
                1,
                1,
                digest.clone(),
                data,
            )
            .unwrap();
        let stored = store.take("img-1", &digest).unwrap();
        assert_eq!(stored.width, 1);
        assert!(store.take("img-1", &digest).is_err());
    }
}
