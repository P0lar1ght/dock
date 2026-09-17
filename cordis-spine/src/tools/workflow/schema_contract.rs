//! `agent(output_schema:)` 的产出契约：编译 schema、校验子代理的收尾正文。
//!
//! 没有这一层时 `output_schema` 只是拼进 prompt 的一段文字，脚本拿到的可能是
//! 一串散文；`deep_research.rhai` 每次 `agent()` 都带 schema，随后直接
//! `plan.output.questions` 这样取值，取空就整条 run 歪掉。

/// 契约不满足时再给一次机会。一次足够纠正"忘了包 JSON"这类失误，再多就是在
/// 烧预算。
pub(super) const SCHEMA_CONTRACT_RETRIES: u32 = 1;

const SCHEMA_MAX_BYTES: usize = 256 * 1024;
const CONTRACT_OUTPUT_MAX_BYTES: usize = 2 * 1024 * 1024;
const SCHEMA_REGEX_SIZE_LIMIT: usize = 256 * 1024;
const SCHEMA_REGEX_DFA_SIZE_LIMIT: usize = 2 * 1024 * 1024;

/// schema 里的 `$ref` 一律不许出网。
///
/// 脚本可以来自 `.dock/workflows/`，schema 是它给的数据；允许远程 `$ref` 等于
/// 让一段脚本在编译 schema 时发起任意 HTTP 请求。
#[derive(Debug)]
struct RejectExternalSchemaRefs;

impl jsonschema::Retrieve for RejectExternalSchemaRefs {
    fn retrieve(
        &self,
        uri: &jsonschema::Uri<String>,
    ) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
        Err(format!("schema 里的外部 $ref 已禁用：{uri}").into())
    }
}

/// 把 `output_schema` 编译成校验器。编译失败是脚本的错，不该等到子代理跑完。
pub(super) fn compile_contract_schema(
    schema: &serde_json::Value,
) -> Result<jsonschema::Validator, String> {
    let schema_len = serde_json::to_vec(schema)
        .map_err(|e| format!("output_schema 无法序列化：{e}"))?
        .len();
    if schema_len > SCHEMA_MAX_BYTES {
        return Err(format!(
            "output_schema 太大（{schema_len} 字节，上限 {SCHEMA_MAX_BYTES}）"
        ));
    }
    jsonschema::options()
        .with_retriever(RejectExternalSchemaRefs)
        .with_pattern_options(
            jsonschema::PatternOptions::regex()
                .size_limit(SCHEMA_REGEX_SIZE_LIMIT)
                .dfa_size_limit(SCHEMA_REGEX_DFA_SIZE_LIMIT),
        )
        .build(schema)
        .map_err(|e| format!("output_schema 不是一份自洽的 JSON Schema：{e}"))
}

/// 拼进子代理 prompt 的契约段。
pub(super) fn contract_prompt(prompt: &str, schema: &serde_json::Value) -> String {
    format!(
        "{prompt}\n\n<output-contract>\nDo the work above with your tools first. Then end \
         your final message with a single ```json fenced block containing exactly one \
         JSON value that conforms to this JSON Schema (no prose inside the block):\n\
         {schema}\n</output-contract>"
    )
}

/// 从子代理的收尾正文里取出符合契约的那个 JSON 值。
///
/// 依次试：最后一个 ```json 围栏、整段正文、正文里第一个 `{`…`}` / `[`…`]`。
/// 模型常在 JSON 前后带一句话，只认严格格式等于白跑一趟。
pub(super) fn validate_contract_output(
    validator: &jsonschema::Validator,
    final_text: &str,
) -> Result<serde_json::Value, String> {
    if final_text.len() > CONTRACT_OUTPUT_MAX_BYTES {
        return Err(format!(
            "收尾正文超过 {CONTRACT_OUTPUT_MAX_BYTES} 字节的结构化产出上限"
        ));
    }
    let text = final_text.trim();
    let mut candidates: Vec<&str> = Vec::new();
    if let Some(start) = text.rfind("```json") {
        if let Some(body) = text.get(start + "```json".len()..) {
            if let Some(end) = body.find("```") {
                candidates.push(body.get(..end).unwrap_or("").trim());
            }
        }
    }
    candidates.push(text);
    for (open, close) in [('{', '}'), ('[', ']')] {
        if let (Some(s), Some(e)) = (text.find(open), text.rfind(close)) {
            if s < e {
                if let Some(slice) = text.get(s..=e) {
                    candidates.push(slice.trim());
                }
            }
        }
    }
    let mut parse_err = String::new();
    for cand in candidates {
        match serde_json::from_str::<serde_json::Value>(cand) {
            Ok(value) => {
                return match validator.validate(&value) {
                    Ok(()) => Ok(value),
                    Err(e) => Err(format!("产出不符合约定的 schema：{e}")),
                };
            }
            Err(e) => {
                if parse_err.is_empty() {
                    parse_err = e.to_string();
                }
            }
        }
    }
    Err(format!(
        "收尾正文里没有可解析的 JSON（应当是一个 ```json 围栏块）：{parse_err}"
    ))
}

/// 契约没满足时下一轮的纠错 prompt。
pub(super) fn retry_prompt(error: &str) -> String {
    format!(
        "Your final message did not satisfy the output contract: {error}\n\
         Reply with a single ```json fenced block containing one JSON value \
         conforming to the schema from <output-contract>, and nothing else."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v() -> jsonschema::Validator {
        compile_contract_schema(&serde_json::json!({
            "type": "object", "required": ["ok"],
            "properties": { "ok": { "type": "boolean" } }
        }))
        .unwrap()
    }

    #[test]
    fn fenced_json_after_prose_validates() {
        let text = "我扫了 12 个文件。\n\n```json\n{\"ok\": true}\n```";
        assert_eq!(
            validate_contract_output(&v(), text).unwrap(),
            serde_json::json!({"ok": true})
        );
    }

    #[test]
    fn bare_json_validates() {
        assert!(validate_contract_output(&v(), "{\"ok\": false}").is_ok());
    }

    #[test]
    fn json_embedded_in_prose_validates() {
        let text = "结果是：{\"ok\": true} —— 完成。";
        assert!(validate_contract_output(&v(), text).is_ok());
    }

    #[test]
    fn last_fence_wins() {
        let text = "```json\n{\"wrong\": 1}\n```\n更正：\n```json\n{\"ok\": true}\n```";
        assert!(validate_contract_output(&v(), text).is_ok());
    }

    #[test]
    fn schema_violation_reports_a_schema_error() {
        let err = validate_contract_output(&v(), "{\"ok\": \"yes\"}").unwrap_err();
        assert!(err.contains("不符合约定的 schema"), "{err}");
    }

    #[test]
    fn no_json_reports_a_parse_error() {
        let err = validate_contract_output(&v(), "扫完了，没问题。").unwrap_err();
        assert!(err.contains("没有可解析的 JSON"), "{err}");
    }

    #[test]
    fn external_references_are_rejected() {
        let err = compile_contract_schema(&serde_json::json!({
            "$ref": "https://example.com/schema.json"
        }))
        .unwrap_err();
        assert!(err.contains("外部 $ref 已禁用"), "{err}");
    }

    #[test]
    fn an_oversized_schema_is_rejected() {
        let huge: Vec<String> = (0..20_000).map(|i| format!("field-{i}")).collect();
        let err = compile_contract_schema(&serde_json::json!({
            "type": "object",
            "required": huge,
        }))
        .unwrap_err();
        assert!(err.contains("太大"), "{err}");
    }
}
