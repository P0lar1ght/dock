//! Shared helpers for attaching tool-result images to sample payloads.

use base64::Engine;
use serde_json::{json, Value};

use crate::types::UserImage;

/// Text-only Chat Completions `role:tool` message (images go on a batched user msg).
pub(crate) fn chat_tool_message(id: &str, content: &str) -> Value {
    json!({
        "role": "tool",
        "tool_call_id": id,
        "content": content,
    })
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
            "content": content,
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
            "output": content,
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
