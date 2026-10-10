//! Shared helpers for attaching tool-result images to sample payloads.

use base64::Engine;
use serde_json::{json, Value};

use cordis_base::types::UserImage;

/// 纯文本模型（`[model.<id>].supports_images = false`）丢掉图片时，留给模型的
/// 说明。**必须说出来**：留白等于让模型自己猜，而工具那条正文的占位符
/// （`Image content included inline`）字面就是「图已经内联了」——模型会照着
/// 编一段它根本没收到的画面。
pub(crate) fn text_only_note(count: usize) -> String {
    format!(
        "[这里原本有 {count} 张图片。当前模型是纯文本的（config.toml 里 supports_images = false），图片没有进入上下文，你看不到它们，不要猜测或描述其内容；需要看图请让用户换一个支持图片的模型。]"
    )
}

/// 把 `text` 和「图片被丢掉」的说明拼起来（`text` 为空时只留说明）。
pub(crate) fn with_text_only_note(text: &str, count: usize) -> String {
    let note = text_only_note(count);
    if text.trim().is_empty() {
        note
    } else {
        format!("{text}\n{note}")
    }
}

/// Text-only Chat Completions `role:tool` message (images go on a batched user msg).
pub(crate) fn chat_tool_message(id: &str, content: &str) -> Value {
    json!({
        "role": "tool",
        "tool_call_id": id,
        "content": content,
    })
}

/// 工具结果里有图、但模型是纯文本：正文里的「图已内联」占位符要换成说明，
/// 否则模型会当成自己看见了图。没有图、或模型看得见图时原样返回。
pub(crate) fn degrade_tool_text(content: &str, images: &[UserImage], vision: bool) -> String {
    if vision || images.is_empty() {
        return content.to_string();
    }
    let note = text_only_note(images.len());
    // 只去掉工具自己追加的那一行；正文里恰好提到这串字的要留着。
    let stripped = content
        .lines()
        .filter(|line| line.trim() != crate::tools::tool_images::IMAGE_INLINE_PLACEHOLDER)
        .collect::<Vec<_>>()
        .join("\n");
    let stripped = stripped.trim();
    if stripped.is_empty() {
        note
    } else {
        format!("{stripped}\n{note}")
    }
}

/// Adjacent user message carrying accumulated tool-result images for vision models.
/// Returns `None` when the model rejects images or `images` is empty.
pub(crate) fn chat_tool_images_user(images: &[UserImage], vision: bool) -> Option<Value> {
    if !vision || images.is_empty() {
        return None;
    }
    Some(user_with_images(
        "Tool result image content included inline.",
        images,
    ))
}

pub(crate) fn messages_tool_result_block(
    id: &str,
    content: &str,
    images: &[UserImage],
    vision: bool,
) -> Value {
    if !vision || images.is_empty() {
        return json!({
            "type": "tool_result",
            "tool_use_id": id,
            "content": degrade_tool_text(content, images, vision),
        });
    }
    let mut parts = Vec::new();
    if !content.trim().is_empty() {
        parts.push(json!({"type": "text", "text": content}));
    }
    for img in images {
        let b64 = base64::engine::general_purpose::STANDARD.encode(img.data.as_ref());
        parts.push(json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": img.mime,
                "data": b64,
            }
        }));
    }
    json!({
        "type": "tool_result",
        "tool_use_id": id,
        "content": parts,
    })
}

pub(crate) fn responses_tool_output(
    id: &str,
    content: &str,
    images: &[UserImage],
    vision: bool,
) -> Vec<Value> {
    if !vision || images.is_empty() {
        return vec![json!({
            "type": "function_call_output",
            "call_id": id,
            "output": degrade_tool_text(content, images, vision),
        })];
    }
    let mut parts = Vec::new();
    if !content.trim().is_empty() {
        parts.push(json!({"type": "input_text", "text": content}));
    }
    for img in images {
        let b64 = base64::engine::general_purpose::STANDARD.encode(img.data.as_ref());
        parts.push(json!({
            "type": "input_image",
            "image_url": format!("data:{};base64,{b64}", img.mime),
            "detail": "auto",
        }));
    }
    vec![json!({
        "type": "function_call_output",
        "call_id": id,
        "output": parts,
    })]
}

fn user_with_images(text: &str, images: &[UserImage]) -> Value {
    let mut parts = Vec::new();
    if !text.trim().is_empty() {
        parts.push(json!({"type": "text", "text": text}));
    }
    for img in images {
        let b64 = base64::engine::general_purpose::STANDARD.encode(img.data.as_ref());
        parts.push(json!({
            "type": "image_url",
            "image_url": {
                "url": format!("data:{};base64,{b64}", img.mime)
            }
        }));
    }
    json!({"role": "user", "content": parts})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::tool_images::IMAGE_INLINE_PLACEHOLDER;

    fn img() -> UserImage {
        UserImage {
            mime: "image/png".into(),
            data: std::sync::Arc::from(vec![1u8, 2, 3].into_boxed_slice()),
            width: 1,
            height: 1,
        }
    }

    /// 只换掉工具自己追加的那一行占位符；正文里恰好提到这串字（比如
    /// `read_file` 读到一个引用它的源码文件）要原样留着。
    #[test]
    fn degrade_only_replaces_the_placeholder_line() {
        let body = format!("let s = \"{IMAGE_INLINE_PLACEHOLDER}\";\n{IMAGE_INLINE_PLACEHOLDER}");
        let out = degrade_tool_text(&body, &[img()], false);
        assert!(
            out.starts_with(&format!("let s = \"{IMAGE_INLINE_PLACEHOLDER}\";\n")),
            "{out}"
        );
        assert_eq!(out.matches(IMAGE_INLINE_PLACEHOLDER).count(), 1, "{out}");
        assert!(out.contains("纯文本"), "{out}");

        let only = degrade_tool_text(IMAGE_INLINE_PLACEHOLDER, &[img()], false);
        assert_eq!(only, text_only_note(1));
    }
}
