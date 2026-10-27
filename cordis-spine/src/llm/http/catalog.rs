//! 设置页「从 models.dev 更新供应商目录」：拉 <https://models.dev/api.json>（opencode 用的
//! 开源供应商 / 模型目录），只留界面用得到的字段再交给 GUI。
//!
//! 只在用户点了才拉；Dock 自己不读这份目录、不按它猜模型能力——加模型时由界面把
//! 目录里的值写进 `[model.<id>]`，之后以 config 为准。

use std::time::Duration;

use serde_json::{Map, Value};

use super::{llm_client, transport_detail};

pub const MODELS_DEV_URL: &str = "https://models.dev/api.json";
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// 供应商上留的键。
const PROVIDER_KEYS: &[&str] = &["id", "name", "api", "npm", "env", "doc"];
/// 模型上留的键（嵌套的 `modalities` / `limit` / `cost` 原样留）。
const MODEL_KEYS: &[&str] = &[
    "id",
    "name",
    "reasoning",
    "tool_call",
    "attachment",
    "modalities",
    "limit",
    "cost",
    "release_date",
    "status",
];

/// 拉 models.dev 并精简。
pub async fn fetch_models_dev() -> Result<Value, String> {
    let resp = llm_client()
        .get(MODELS_DEV_URL)
        .timeout(FETCH_TIMEOUT)
        .send()
        .await
        .map_err(|e| format!("连不上 models.dev：{}", transport_detail(&e)))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("models.dev 回了 HTTP {}", status.as_u16()));
    }
    let raw: Value = resp
        .json()
        .await
        .map_err(|e| format!("models.dev 回的不是 JSON：{e}"))?;
    trim_models_dev(&raw)
}

/// 只留 [`PROVIDER_KEYS`] / [`MODEL_KEYS`]；形状不对（不是对象）就报错，不给半份。
pub fn trim_models_dev(raw: &Value) -> Result<Value, String> {
    let providers = raw.as_object().ok_or("models.dev 目录不是对象")?;
    let mut out = Map::new();
    for (pid, p) in providers {
        let Some(p) = p.as_object() else { continue };
        let mut provider = pick(p, PROVIDER_KEYS);
        let mut models = Map::new();
        for (mid, m) in p
            .get("models")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            if let Some(m) = m.as_object() {
                models.insert(mid.clone(), Value::Object(pick(m, MODEL_KEYS)));
            }
        }
        provider.insert("models".into(), Value::Object(models));
        out.insert(pid.clone(), Value::Object(provider));
    }
    if out.is_empty() {
        return Err("models.dev 目录是空的".into());
    }
    Ok(Value::Object(out))
}

fn pick(obj: &Map<String, Value>, keys: &[&str]) -> Map<String, Value> {
    keys.iter()
        .filter_map(|k| obj.get(*k).map(|v| (k.to_string(), v.clone())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn trim_keeps_only_the_fields_the_settings_page_reads() {
        let raw = json!({
            "deepseek": {
                "id": "deepseek", "name": "DeepSeek", "api": "https://api.deepseek.com",
                "npm": "@ai-sdk/openai-compatible", "env": ["DEEPSEEK_API_KEY"], "extra": 1,
                "models": {
                    "deepseek-chat": {
                        "id": "deepseek-chat", "name": "DeepSeek Chat", "reasoning": false,
                        "limit": { "context": 128000, "output": 8192 },
                        "cost": { "input": 0.28, "output": 0.42 },
                        "knowledge": "2024-07", "open_weights": true
                    }
                }
            },
            "broken": "not an object"
        });
        let out = trim_models_dev(&raw).unwrap();
        let p = &out["deepseek"];
        assert_eq!(p["api"], "https://api.deepseek.com");
        assert!(p.get("extra").is_none());
        let m = &p["models"]["deepseek-chat"];
        assert_eq!(m["limit"]["context"], 128000);
        assert!(m.get("knowledge").is_none());
        assert!(out.get("broken").is_none());
        assert!(trim_models_dev(&json!([])).is_err());
    }
}
