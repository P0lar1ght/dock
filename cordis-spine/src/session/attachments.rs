//! 用户消息里的图：内容寻址存盘 + 会话日志里的引用。
//!
//! 像素不进 JSONL（和工具图一个规矩）：落盘只写摘要（sha256）与元数据，读回来
//! 按摘要找回字节。会话历史因此还能看到用户发的图 —— 客户端刷新、重开 Dock、
//! 连远程都读得到。
//!
//! 存法照 DSH 的附件层：`<DOCK_HOME>/user-images/<摘要前两位>/<摘要>`，
//! 同样的字节只存一份。摘要是内容的 sha256，既是文件名也是引用。

use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine;
use cordis_base::config::dock_home;
use cordis_base::types::UserImage;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// 一张图的上限：和网关 `image_store` 的限额一致（GUI 侧也是这个数）。
pub const MAX_USER_IMAGE_BYTES: usize = 2 * 1024 * 1024;

/// 会话日志里的一条图片引用：**没有路径，也没有 base64**。
///
/// 摘要既定位 blob，也当鉴权凭据（网关只让会话引用过的摘要取图）。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UserImageRef {
    /// 内容的 sha256（十六进制）。也是 blob 的文件名。
    pub digest: String,
    pub mime: String,
    pub width: u32,
    pub height: u32,
    pub bytes: usize,
}

/// blob 根目录：`$DOCK_HOME/user-images`。
pub fn blob_dir() -> PathBuf {
    dock_home().join("user-images")
}

/// 内容寻址的路径：`<root>/<摘要前两位>/<摘要>`。
pub fn blob_path(digest: &str) -> PathBuf {
    let (prefix, _) = digest.split_at(digest.len().min(2));
    blob_dir().join(prefix).join(digest)
}

/// 字节的 sha256（十六进制）。
pub fn digest_of(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex(&hasher.finalize())
}

/// 字节的 sha256（base64，给协议用；和 digest_of 是同一个内容的两种写法）。
pub fn digest_b64(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    base64::engine::general_purpose::STANDARD.encode(hasher.finalize())
}

/// 合法的内容摘要：sha256 的 64 位十六进制。
///
/// 摘要会拼进文件路径：不合法的摘要能拼出 blob 目录以外的地方，所以只认这一种形状。
fn is_digest(digest: &str) -> bool {
    digest.len() == 64 && digest.chars().all(|c| c.is_ascii_hexdigit())
}

/// 把一张图存进 blob 库，返回引用。
///
/// 超限或写盘失败 → `None`（fail-open：这张图不进历史，消息本身照旧），
/// 但**每一次放弃都留日志** —— 静默丢图只会表现为「刷新后图不见了」。
pub fn store(img: &UserImage) -> Option<UserImageRef> {
    if img.data.is_empty() || img.data.len() > MAX_USER_IMAGE_BYTES {
        tracing::warn!(
            bytes = img.data.len(),
            "用户图片超过上限，不入历史（上限 {} 字节）",
            MAX_USER_IMAGE_BYTES
        );
        return None;
    }
    // 摘要一律按字节算：外面带来的摘要只用来对账，不拿来拼路径 ——
    // 对不上就落到别的文件上，甚至写到 blob 目录外面去。
    let actual = digest_of(&img.data);
    let digest = match img.digest.as_deref() {
        Some(d) if d == actual => actual,
        Some(d) => {
            tracing::warn!(
                expected = %actual,
                got = %d,
                "用户图片的摘要与字节对不上，按实际字节重算"
            );
            actual
        }
        None => actual,
    };
    let path = blob_path(&digest);
    if !path.exists() {
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::warn!(path = %path.display(), error = %e, "用户图片的目录建不出来，这张图不入历史");
                return None;
            }
        }
        // 先写临时文件再改名：会话日志引用到这个摘要时，文件一定已经完整。
        let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
        if let Err(e) = std::fs::write(&tmp, img.data.as_ref()) {
            let _ = std::fs::remove_file(&tmp);
            tracing::warn!(path = %path.display(), error = %e, "用户图片写不进临时文件，这张图不入历史");
            return None;
        }
        if let Err(e) = std::fs::rename(&tmp, &path) {
            let _ = std::fs::remove_file(&tmp);
            tracing::warn!(path = %path.display(), error = %e, "用户图片改不了名，这张图不入历史");
            return None;
        }
    }
    Some(UserImageRef {
        digest,
        mime: img.mime.clone(),
        width: img.width,
        height: img.height,
        bytes: img.data.len(),
    })
}

/// 按引用读回像素。文件没了 / 读失败 → `None`（那张图就没了，消息还在）。
///
/// 类型、宽高照引用里的来（写下去的时候就是它们），不重新猜。
pub fn load(reference: &UserImageRef) -> Option<UserImage> {
    let mut image = load_by_digest(&reference.digest)?;
    if !reference.mime.is_empty() {
        image.mime = reference.mime.clone();
    }
    if reference.width > 0 {
        image.width = reference.width;
    }
    if reference.height > 0 {
        image.height = reference.height;
    }
    Some(image)
}

/// 按摘要读回像素。
///
/// 摘要就是**会话级凭据**：它只说明「这张图是这个会话里的那张」，所以只认
/// 合法摘要，且读到的字节必须对得上（文件被换过 / 写坏了就不给）。
pub fn load_by_digest(digest: &str) -> Option<UserImage> {
    if !is_digest(digest) {
        return None;
    }
    let path = blob_path(digest);
    let data = std::fs::read(&path).ok()?;
    // 摘要对不上就别给：文件被换过 / 写坏了。
    if digest_of(&data) != digest {
        return None;
    }
    let mime = sniff_mime(&data).to_string();
    let (width, height) = dimensions(&data, &mime);
    Some(UserImage {
        mime,
        data: Arc::from(data.into_boxed_slice()),
        width,
        height,
        // 摘要带着走：调用方（网关）要按它找同一张图。
        digest: Some(digest.to_string()),
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn sniff_mime(data: &[u8]) -> &'static str {
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

fn dimensions(data: &[u8], mime: &str) -> (u32, u32) {
    crate::tools::tool_images::dimensions(data, mime)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_png() -> Vec<u8> {
        vec![
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00,
            0x00, 0x90, 0x77, 0x53, 0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x08,
            0xd7, 0x63, 0xf8, 0xff, 0xff, 0x3f, 0x00, 0x05, 0xfe, 0x02, 0xfe, 0xa7, 0x35, 0x81,
            0x84, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
        ]
    }

    #[test]
    fn digest_is_stable() {
        assert_eq!(digest_of(b"abc"), digest_of(b"abc"));
        assert_ne!(digest_of(b"abc"), digest_of(b"abd"));
        assert_eq!(digest_of(b"abc").len(), 64);
    }

    #[test]
    fn blob_path_is_content_addressed() {
        let d = digest_of(b"abc");
        let p = blob_path(&d);
        assert!(p.ends_with(&d));
        assert!(
            p.parent()
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .len()
                == 2
        );
    }

    #[test]
    fn load_rejects_bad_digest() {
        assert!(load_by_digest("").is_none());
        assert!(load_by_digest("not-hex").is_none());
        assert!(load_by_digest(&"z".repeat(64)).is_none());
    }

    fn tiny_image() -> UserImage {
        UserImage {
            mime: "image/png".into(),
            data: Arc::from(tiny_png().into_boxed_slice()),
            width: 1,
            height: 1,
            digest: None,
        }
    }

    /// 外面带来的摘要只用来对账：跟字节对不上就按实际字节算，
    /// 不能拿它拼路径（会写到别的图上，或写到 blob 目录外面）。
    #[test]
    fn store_uses_the_real_digest_when_the_given_one_is_wrong() {
        let _home = cordis_base::test_env::scoped().home();
        let mut img = tiny_image();
        let wrong = "00".repeat(32);
        img.digest = Some(wrong.clone());
        let stored = store(&img).expect("stored");
        assert_eq!(stored.digest, digest_of(&tiny_png()));
        assert!(!blob_path(&wrong).exists(), "不能写到错摘要指向的地方");
    }

    /// 读回来的图要带着摘要：调用方（网关）靠它按会话找同一张图。
    #[test]
    fn loaded_image_keeps_its_digest() {
        let _home = cordis_base::test_env::scoped().home();
        let img = tiny_image();
        let stored = store(&img).expect("stored");
        let loaded = load(&stored).expect("loaded");
        assert_eq!(loaded.digest.as_deref(), Some(stored.digest.as_str()));
        assert_eq!(loaded.mime, "image/png");
        assert_eq!(loaded.width, 1);
    }

    #[test]
    fn load_rejects_malformed_digest() {
        assert!(load_by_digest("").is_none());
        assert!(load_by_digest("not-hex").is_none());
        assert!(
            load_by_digest(&"a".repeat(64)).is_none(),
            "合法形状但没这张图"
        );
        assert!(load_by_digest("../etc/passwd").is_none());
    }

    #[test]
    fn sniff_png() {
        assert_eq!(sniff_mime(&tiny_png()), "image/png");
    }
}
