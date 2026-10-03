//! 设置页「测试连接」：对一条模型发一次最小请求，看端点、协议、密钥对不对。
//!
//! 不走 [`super::sample_http`]：那条路按**当前页**的设置挑模型和协议，还会往会话里
//! 记用量；这里测的是表单里还没保存的那条，什么都不该留下。地址 / 鉴权 / 上游模型名
//! 的取法和正式采样一致（`base_url_for` / `resolved_auth` / `wire_model_for`）。

use std::time::{Duration, Instant};

use serde_json::json;

use cordis_base::config::{ApiBackend, ModelChoice};

use super::{apply_auth, llm_client, transport_detail};

/// 测试请求整体超时：模型首包慢也不该让设置页一直转圈。
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);

/// 用模型的默认协议发一次最小请求。成功回耗时（毫秒）。
pub async fn probe_model(choice: &ModelChoice) -> Result<u64, String> {
    let backend = choice.default_backend();
    let base = choice
        .base_url_for(backend)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or("没有接口地址")?;
    // 写了环境变量名却没设：多半是忘了 export，直说。什么都没配（本地 ollama 这类）就不带鉴权头。
    let key = choice.resolved_api_key().filter(|k| !k.is_empty());
    if key.is_none() {
        if let Some(var) = choice.env_key.as_deref().filter(|v| !v.trim().is_empty()) {
            return Err(format!("环境变量 {var} 没有设置"));
        }
    }
    let wire = choice.wire_model_for(backend);
    let body = match backend {
        ApiBackend::ChatCompletions => json!({
            "model": wire,
            "messages": [{ "role": "user", "content": "ping" }],
            "max_tokens": 1,
            "stream": false
        }),
        // /responses 的输出上限有下限（OpenAI 是 16），给小了会 400。
        ApiBackend::Responses => json!({
            "model": wire,
            "input": "ping",
            "max_output_tokens": 16,
            "stream": false
        }),
        ApiBackend::Messages => json!({
            "model": wire,
            "max_tokens": 1,
            "messages": [{ "role": "user", "content": "ping" }]
        }),
    };
    let url = format!("{}/{}", base.trim_end_matches('/'), backend.path());
    let mut req = llm_client()
        .post(&url)
        .timeout(PROBE_TIMEOUT)
        .header("content-type", "application/json")
        .json(&body);
    if backend == ApiBackend::Messages {
        req = req.header("anthropic-version", "2023-06-01");
    }
    if let Some(key) = &key {
        req = apply_auth(req, choice.resolved_auth(backend), key);
    }
    let started = Instant::now();
    let resp = req.send().await.map_err(|e| transport_detail(&e))?;
    let status = resp.status();
    let elapsed = started.elapsed().as_millis() as u64;
    if status.is_success() {
        return Ok(elapsed);
    }
    let text = resp.text().await.unwrap_or_default();
    Err(http_error(status.as_u16(), &text))
}

/// 状态码 + 上游原话（截短），常见的几种补一句人话。
fn http_error(status: u16, body: &str) -> String {
    let hint = match status {
        401 | 403 => "：密钥不对或没有权限",
        404 => "：地址或协议不对（这个端点没有这条路径），或上游模型名不存在",
        429 => "：被限流了",
        _ => "",
    };
    let detail: String = body.trim().chars().take(200).collect();
    if detail.is_empty() {
        format!("HTTP {status}{hint}")
    } else {
        format!("HTTP {status}{hint}（{detail}）")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_error_explains_common_statuses() {
        assert!(http_error(401, "").contains("密钥不对"));
        assert!(http_error(404, "{\"error\":\"nope\"}").contains("nope"));
        assert_eq!(http_error(500, ""), "HTTP 500");
    }

    #[tokio::test]
    async fn missing_key_fails_before_any_request() {
        let choice = ModelChoice {
            id: "m".into(),
            name: "m".into(),
            description: String::new(),
            api_base_url: Some("http://127.0.0.1:9/v1".into()),
            api_key: None,
            env_key: Some("DOCK_TEST_PROBE_KEY_THAT_IS_NOT_SET".into()),
            context_window: None,
            api_backends: vec![ApiBackend::ChatCompletions],
            backend_overrides: Default::default(),
            auth_scheme: None,
            api_model: None,
            prompt_cache: None,
            max_output_tokens: None,
            reasoning: None,
            reasoning_effort: None,
            reasoning_efforts: None,
            supports_images: None,
            pricing: None,
        };
        let err = probe_model(&choice).await.unwrap_err();
        assert!(err.contains("DOCK_TEST_PROBE_KEY_THAT_IS_NOT_SET"), "{err}");
    }
}
