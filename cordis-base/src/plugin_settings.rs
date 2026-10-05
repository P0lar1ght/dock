//! 插件设置卡的 schema（`docs/PLUGIN-VIEWS.md` 设置卡）：插件声明配置项，界面按它
//! 生成表单。这里只有数据：解析、规范化、按 schema 校验一组值。存哪、谁能写在 spine。

use serde_json::{json, Map, Value};

/// 一个字段的类型。
#[derive(Clone, Debug, PartialEq)]
pub enum FieldKind {
    /// 单行文本。
    String,
    /// 多行文本。
    Text,
    Number {
        min: Option<f64>,
        max: Option<f64>,
    },
    Boolean,
    Select {
        options: Vec<String>,
    },
    /// 只进不出：值写进密钥库（`secrets.json`，名字就是字段 `key`），插件用
    /// `host.secret(key)` 读，界面永远拿不到原值。
    Secret,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SettingsField {
    pub key: String,
    pub kind: FieldKind,
    pub label: String,
    pub description: Option<String>,
    pub default: Option<Value>,
    pub required: bool,
}

/// 一颗插件的设置卡。
#[derive(Clone, Debug, PartialEq)]
pub struct SettingsSchema {
    pub title: String,
    pub fields: Vec<SettingsField>,
}

/// 字段 key：`[a-zA-Z][a-zA-Z0-9_-]{0,63}`（密钥字段的 key 直接当密钥名用）。
fn valid_key(key: &str) -> bool {
    let mut chars = key.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic())
        && key.len() <= 64
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

impl SettingsSchema {
    /// 严格解析：插件声明错了要在登记时就说清楚。
    pub fn parse(v: &Value) -> Result<Self, String> {
        let title = v
            .get("title")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or("设置卡需要 title")?
            .to_string();
        let raw = v
            .get("fields")
            .and_then(Value::as_array)
            .ok_or("设置卡需要 fields 数组")?;
        let mut fields = Vec::with_capacity(raw.len());
        for f in raw {
            let key = f
                .get("key")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            if !valid_key(&key) {
                return Err(format!(
                    "字段 key 不合法：{key:?}（字母开头，只用字母、数字、_、-，最长 64）"
                ));
            }
            if fields.iter().any(|x: &SettingsField| x.key == key) {
                return Err(format!("字段 key 重复：{key}"));
            }
            let kind = match f.get("type").and_then(Value::as_str).unwrap_or("") {
                "string" => FieldKind::String,
                "text" => FieldKind::Text,
                "number" => FieldKind::Number {
                    min: f.get("min").and_then(Value::as_f64),
                    max: f.get("max").and_then(Value::as_f64),
                },
                "boolean" => FieldKind::Boolean,
                "select" => {
                    let options: Vec<String> = f
                        .get("options")
                        .and_then(Value::as_array)
                        .map(|o| {
                            o.iter()
                                .filter_map(|x| x.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_default();
                    if options.is_empty() {
                        return Err(format!("字段 {key} 是 select，需要 options"));
                    }
                    FieldKind::Select { options }
                }
                "secret" => FieldKind::Secret,
                other => return Err(format!("字段 {key} 的 type 不认识：{other:?}")),
            };
            let field = SettingsField {
                label: f
                    .get("label")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or(&key)
                    .to_string(),
                description: f
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                default: f.get("default").cloned().filter(|d| !d.is_null()),
                required: f.get("required").and_then(Value::as_bool).unwrap_or(false),
                key,
                kind,
            };
            if field.kind == FieldKind::Secret && field.default.is_some() {
                return Err(format!("密钥字段 {} 不能有 default", field.key));
            }
            if let Some(d) = &field.default {
                field
                    .check(d)
                    .map_err(|e| format!("字段 {} 的 default 不合法：{e}", field.key))?;
            }
            fields.push(field);
        }
        Ok(Self { title, fields })
    }

    pub fn to_value(&self) -> Value {
        json!({
            "title": self.title,
            "fields": self.fields.iter().map(SettingsField::to_value).collect::<Vec<_>>(),
        })
    }

    pub fn field(&self, key: &str) -> Option<&SettingsField> {
        self.fields.iter().find(|f| f.key == key)
    }

    /// 校验一组要写的值（只含要改的字段）。回规范化后的值，或逐字段的错误。
    /// `null` = 清掉（回到 default）；必填字段不能清。
    pub fn validate(
        &self,
        values: &Map<String, Value>,
    ) -> Result<Map<String, Value>, Vec<(String, String)>> {
        let mut ok = Map::new();
        let mut errors = Vec::new();
        for (key, value) in values {
            let Some(field) = self.field(key) else {
                errors.push((key.clone(), "没有这个字段".into()));
                continue;
            };
            if value.is_null() {
                if field.required && field.default.is_none() {
                    errors.push((key.clone(), "必填".into()));
                } else {
                    ok.insert(key.clone(), Value::Null);
                }
                continue;
            }
            match field.check(value) {
                Ok(v) => {
                    ok.insert(key.clone(), v);
                }
                Err(e) => errors.push((key.clone(), e)),
            }
        }
        if errors.is_empty() {
            Ok(ok)
        } else {
            Err(errors)
        }
    }
}

impl SettingsField {
    fn to_value(&self) -> Value {
        let mut v = json!({
            "key": self.key,
            "label": self.label,
            "description": self.description,
            "default": self.default,
            "required": self.required,
        });
        let kind = match &self.kind {
            FieldKind::String => "string",
            FieldKind::Text => "text",
            FieldKind::Number { min, max } => {
                v["min"] = json!(min);
                v["max"] = json!(max);
                "number"
            }
            FieldKind::Boolean => "boolean",
            FieldKind::Select { options } => {
                v["options"] = json!(options);
                "select"
            }
            FieldKind::Secret => "secret",
        };
        v["type"] = json!(kind);
        v
    }

    /// 一个值合不合这个字段，合就回规范化后的值。
    pub fn check(&self, value: &Value) -> Result<Value, String> {
        match &self.kind {
            FieldKind::String | FieldKind::Text | FieldKind::Secret => {
                let s = value.as_str().ok_or("要一段文字")?;
                if self.required && s.trim().is_empty() {
                    return Err("必填".into());
                }
                if self.kind == FieldKind::String && s.contains('\n') {
                    return Err("只能一行".into());
                }
                Ok(Value::String(s.to_string()))
            }
            FieldKind::Number { min, max } => {
                let n = match value {
                    Value::Number(n) => n.as_f64(),
                    Value::String(s) => s.trim().parse::<f64>().ok(),
                    _ => None,
                }
                .ok_or("要一个数")?;
                if let Some(m) = min {
                    if n < *m {
                        return Err(format!("最小 {m}"));
                    }
                }
                if let Some(m) = max {
                    if n > *m {
                        return Err(format!("最大 {m}"));
                    }
                }
                Ok(json!(n))
            }
            FieldKind::Boolean => value.as_bool().map(Value::Bool).ok_or("要开或关".into()),
            FieldKind::Select { options } => {
                let s = value.as_str().ok_or("要选一项")?;
                if options.iter().any(|o| o == s) {
                    Ok(Value::String(s.to_string()))
                } else {
                    Err(format!("只能是 {}", options.join(" / ")))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> SettingsSchema {
        SettingsSchema::parse(&json!({
            "title": "部署助手",
            "fields": [
                { "key": "region", "type": "select", "label": "区域", "options": ["cn", "us"], "default": "cn" },
                { "key": "deploy-token", "type": "secret", "label": "访问令牌" },
                { "key": "verbose", "type": "boolean", "label": "详细日志" },
                { "key": "timeout", "type": "number", "min": 10, "max": 600, "required": true, "default": 30 },
                { "key": "note", "type": "text" }
            ]
        }))
        .unwrap()
    }

    #[test]
    fn parses_and_round_trips() {
        let s = schema();
        assert_eq!(s.fields.len(), 5);
        assert_eq!(s.field("note").unwrap().label, "note", "没写 label 用 key");
        assert_eq!(SettingsSchema::parse(&s.to_value()).unwrap(), s);
    }

    #[test]
    fn rejects_bad_schemas() {
        for bad in [
            json!({ "fields": [] }),
            json!({ "title": "x", "fields": [{ "key": "1x", "type": "string" }] }),
            json!({ "title": "x", "fields": [{ "key": "a", "type": "string" }, { "key": "a", "type": "text" }] }),
            json!({ "title": "x", "fields": [{ "key": "a", "type": "select" }] }),
            json!({ "title": "x", "fields": [{ "key": "a", "type": "color" }] }),
            json!({ "title": "x", "fields": [{ "key": "a", "type": "secret", "default": "x" }] }),
            json!({ "title": "x", "fields": [{ "key": "a", "type": "number", "min": 1, "default": 0 }] }),
        ] {
            assert!(SettingsSchema::parse(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn validates_values_field_by_field() {
        let s = schema();
        let ok = s
            .validate(
                json!({ "region": "us", "timeout": "45", "verbose": true })
                    .as_object()
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(ok["timeout"], json!(45.0));
        let err = s
            .validate(
                json!({ "region": "eu", "timeout": 5, "nope": 1, "verbose": "yes" })
                    .as_object()
                    .unwrap(),
            )
            .unwrap_err();
        let msg = |k: &str| err.iter().find(|(key, _)| key == k).map(|(_, m)| m.clone());
        assert_eq!(msg("region").as_deref(), Some("只能是 cn / us"));
        assert_eq!(msg("timeout").as_deref(), Some("最小 10"));
        assert_eq!(msg("nope").as_deref(), Some("没有这个字段"));
        assert_eq!(msg("verbose").as_deref(), Some("要开或关"));
        // 有 default 的必填字段可以清（回到 default）。
        assert!(s
            .validate(json!({ "timeout": null }).as_object().unwrap())
            .is_ok());
    }
}
