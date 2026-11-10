//! 用户消息里的图：内容寻址存盘 + 会话日志里的引用。
//!
//! 像素不进 JSONL（和工具图一个规矩）：落盘只写摘要（sha256）与元数据，读回来
//! 按摘要找回字节。会话历史因此还能看到用户发的图 —— 客户端刷新、重开 Dock、
//! 连远程都读得到。
//!
//! 存法照 DSH 的附件层：`<DOCK_HOME>/user-images/<摘要前两位>/<摘要>`，
//! 同样的字节只存一份。摘要是内容的 sha256，既是文件名也是引用。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

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
    // 还没读回像素的引用（归档会话懒加载，见 [`placeholder`]）：blob 早就在库里，
    // 原样写回引用，不用读字节也不用重算摘要。
    if img.data.is_empty() {
        if let Some(digest) = img.digest.as_deref().filter(|d| is_digest(d)) {
            if let Ok(meta) = std::fs::metadata(blob_path(digest)) {
                return Some(UserImageRef {
                    digest: digest.to_string(),
                    mime: img.mime.clone(),
                    width: img.width,
                    height: img.height,
                    bytes: meta.len() as usize,
                });
            }
            tracing::warn!(digest, "用户图片的 blob 不见了，这张图不再入历史");
            return None;
        }
    }
    if img.data.is_empty() || img.data.len() > MAX_USER_IMAGE_BYTES {
        tracing::warn!(
            bytes = img.data.len(),
            "用户图片超过上限，不入历史（上限 {} 字节）",
            MAX_USER_IMAGE_BYTES
        );
        return None;
    }
    // 已经带着摘要、blob 也已经在库里（同样大小）：这张图早就存过了。摘要只由进程内
    // 算出来（`append`、网关 `publish_images`、`load_by_digest` 校验过的），每次整份
    // 重写 `chat_history.jsonl` 都不必再把几 MB 的图重新哈希一遍。
    if let Some(digest) = img.digest.as_deref().filter(|d| is_digest(d)) {
        let path = blob_path(digest);
        if std::fs::metadata(&path).is_ok_and(|m| m.len() as usize == img.data.len()) {
            return Some(UserImageRef {
                digest: digest.to_string(),
                mime: img.mime.clone(),
                width: img.width,
                height: img.height,
                bytes: img.data.len(),
            });
        }
    }
    // 要写盘：摘要一律按字节算。带来的摘要只用来对账，不拿来拼路径 ——
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
        // 临时名带进程号 + 计数：同一进程里两次 store 碰上同一摘要也不会抢同一个文件。
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let tmp = path.with_extension(format!(
            "tmp-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        if let Err(e) = std::fs::write(&tmp, img.data.as_ref()) {
            let _ = std::fs::remove_file(&tmp);
            tracing::warn!(path = %path.display(), error = %e, "用户图片写不进临时文件，这张图不入历史");
            return None;
        }
        if let Err(e) = std::fs::rename(&tmp, &path) {
            let _ = std::fs::remove_file(&tmp);
            // 别人先一步写好了同一份内容（内容寻址，字节一样）：当成功。
            if path.exists() {
                return Some(UserImageRef {
                    digest,
                    mime: img.mime.clone(),
                    width: img.width,
                    height: img.height,
                    bytes: img.data.len(),
                });
            }
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

/// 按引用造一张**还没读像素**的图（`data` 为空，带摘要和元数据）。
///
/// 读归档会话（启动时整个 cwd 的历史）只留引用：不读文件、不算哈希、不常驻内存。
/// 真要像素时（恢复成活会话、`item/image` 取图）再 [`hydrate`]。摘要不合法 → `None`。
pub fn placeholder(reference: &UserImageRef) -> Option<UserImage> {
    if !is_digest(&reference.digest) {
        return None;
    }
    Some(UserImage {
        mime: reference.mime.clone(),
        data: Arc::from(Vec::new().into_boxed_slice()),
        width: reference.width,
        height: reference.height,
        digest: Some(reference.digest.clone()),
    })
}

/// 把 [`placeholder`] 读成真图（已有像素的原样返回）。blob 没了 / 对不上 → `None`。
pub fn hydrate(img: &UserImage) -> Option<UserImage> {
    if !img.data.is_empty() {
        return Some(img.clone());
    }
    let mut loaded = load_by_digest(img.digest.as_deref()?)?;
    if !img.mime.is_empty() {
        loaded.mime = img.mime.clone();
    }
    if img.width > 0 {
        loaded.width = img.width;
    }
    if img.height > 0 {
        loaded.height = img.height;
    }
    Some(loaded)
}

/// 一张图的字节数：有像素按像素算；还没读的按 blob 文件大小（不读内容）。
/// blob 没了 → `None`。
pub fn image_len(img: &UserImage) -> Option<usize> {
    if !img.data.is_empty() {
        return Some(img.data.len());
    }
    let digest = img.digest.as_deref().filter(|d| is_digest(d))?;
    std::fs::metadata(blob_path(digest))
        .ok()
        .map(|m| m.len() as usize)
}

/// 把事件里用户消息的图都读成真图（恢复成活会话前调：采样要像素）。读不回来的那张丢掉。
pub fn hydrate_events(events: &mut [cordis_base::types::LogEvent]) {
    for event in events.iter_mut() {
        if let cordis_base::types::LogEvent::User(msg) = event {
            if msg.images.iter().any(|img| img.data.is_empty()) {
                msg.images = msg.images.iter().filter_map(hydrate).collect();
            }
        }
    }
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
        let loaded = hydrate(&placeholder(&stored).unwrap()).expect("loaded");
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

    /// 已带摘要、blob 也在：不重算摘要（错摘要 + 大小对不上的照旧按字节重算）。
    #[test]
    fn store_reuses_a_known_digest_without_rehashing() {
        let _home = cordis_base::test_env::scoped().home();
        let img = tiny_image();
        let first = store(&img).expect("stored");
        let mut again = tiny_image();
        again.digest = Some(first.digest.clone());
        assert_eq!(store(&again), Some(first.clone()));
        // 指着库里另一张（大小不同）的错摘要：不能信，按字节重算。
        let other = UserImage {
            data: Arc::from(b"not the same bytes".to_vec().into_boxed_slice()),
            ..tiny_image()
        };
        let other_ref = store(&other).expect("stored other");
        let mut wrong = tiny_image();
        wrong.digest = Some(other_ref.digest.clone());
        assert_eq!(store(&wrong).unwrap().digest, first.digest);
    }

    /// 懒加载：占位图不带像素，写回引用不丢图，要用时按摘要读回来。
    #[test]
    fn placeholder_round_trips_and_hydrates() {
        let _home = cordis_base::test_env::scoped().home();
        let stored = store(&tiny_image()).expect("stored");
        let lazy = placeholder(&stored).expect("placeholder");
        assert!(lazy.data.is_empty());
        assert_eq!(image_len(&lazy), Some(stored.bytes));
        assert_eq!(store(&lazy), Some(stored.clone()), "写回引用原样");
        let full = hydrate(&lazy).expect("hydrated");
        assert_eq!(full.data.as_ref(), tiny_png().as_slice());
        assert_eq!(full.digest.as_deref(), Some(stored.digest.as_str()));
        assert!(placeholder(&UserImageRef {
            digest: "../etc/passwd".into(),
            ..stored
        })
        .is_none());
    }

    #[test]
    fn sniff_png() {
        assert_eq!(sniff_mime(&tiny_png()), "image/png");
    }
}
