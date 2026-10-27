//! 设置页改 `~/.dock/config.toml`：模型目录、白名单里的标量键、MCP 行。
//!
//! 只写**用户级**这一份（桌面 GUI 多项目模式本来就只认它）。全部经 `toml_edit`
//! 就地改：注释、段落顺序、这里不认识的键都原样留着。写盘前先整份解析一遍，
//! 文件本身坏了就拒绝写——否则一次保存会把用户手写的半截内容覆盖掉。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{
    catalog_paths, dock_home, load_providers_from, merge_overrides, patch_toml, ApiBackend,
    FileConfig, McpServer, ModelChoice,
};

/// 设置页写的那一份配置。
pub fn user_config_path() -> PathBuf {
    dock_home().join("config.toml")
}

// ---------------------------------------------------------------------------
// 状态：文件在不在、能不能解析
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ParseError {
    pub message: String,
    /// 1 起算；toml 报不出位置时没有。
    pub line: Option<usize>,
}

/// 用户配置能不能读。解析失败时 Dock 会**整份忽略**这个文件（见 `read_file`），
/// 设置页要把这件事说出来，不能让人以为「模型没了」。
pub fn parse_error_in(path: &Path) -> Option<ParseError> {
    let raw = std::fs::read_to_string(path).ok()?;
    let err = toml::from_str::<FileConfig>(&raw).err()?;
    let line = err
        .span()
        .map(|span| raw[..span.start.min(raw.len())].matches('\n').count() + 1);
    // toml 的 Display 自带多行代码片段；第一行就是原因。
    let message = err
        .message()
        .lines()
        .next()
        .unwrap_or("解析失败")
        .to_string();
    Some(ParseError { message, line })
}

pub fn parse_error() -> Option<ParseError> {
    parse_error_in(&user_config_path())
}

fn ensure_parses(path: &Path) -> Result<(), String> {
    match parse_error_in(path) {
        None => Ok(()),
        Some(e) => Err(match e.line {
            Some(line) => format!("config.toml 第 {line} 行解析失败：{}", e.message),
            None => format!("config.toml 解析失败：{}", e.message),
        }),
    }
}

/// 读写都走这一个入口：先确认文件能解析，再交给 `patch_toml`。
fn edit(
    path: &Path,
    f: impl FnOnce(&mut toml_edit::DocumentMut) -> Result<(), String>,
) -> Result<(), String> {
    ensure_parses(path)?;
    patch_toml(path, f)
}

fn read_doc(path: &Path) -> toml_edit::DocumentMut {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| text.parse::<toml_edit::DocumentMut>().ok())
        .unwrap_or_default()
}

/// 只作为 `[parent.<child>]` 的前缀出现的父表：不单独打一行 `[parent]`。
fn implicit_table(doc: &mut toml_edit::DocumentMut, key: &str) -> Result<(), String> {
    match doc.get(key) {
        Some(item) if item.is_table_like() => Ok(()),
        Some(_) => Err(format!("config.toml 里的 {key} 不是表")),
        None => {
            let mut table = toml_edit::Table::new();
            table.set_implicit(true);
            doc[key] = toml_edit::Item::Table(table);
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// 模型目录
// ---------------------------------------------------------------------------

/// 设置页编辑的一条 `[model."<id>"]`。字段与 `config.toml.example` 一一对应，
/// `None` = 不写这个键（交给上游默认）。
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ModelEntry {
    pub id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub api_base_url: Option<String>,
    pub api_model: Option<String>,
    /// 第一条 = 默认协议。
    pub api_backends: Vec<String>,
    /// `bearer` / `x_api_key`；`None` = 跟着协议走。
    pub auth_scheme: Option<String>,
    /// `env`（读环境变量）或 `inline`（写在文件里）。
    pub key_mode: String,
    pub env_key: Option<String>,
    /// 只进不出：`None` = 文件里的行内密钥原样保留，`Some("")` = 删掉。
    #[serde(skip_serializing)]
    pub api_key: Option<String>,
    /// 只出不进：文件里有没有行内密钥。
    pub has_api_key: bool,
    pub context_window: Option<u64>,
    pub max_output_tokens: Option<u64>,
    pub reasoning: Option<bool>,
    pub reasoning_effort: Option<String>,
    pub reasoning_efforts: Option<Vec<String>>,
    pub supports_images: Option<bool>,
    pub prompt_cache: Option<bool>,
    pub pricing: Option<PricingEntry>,
    /// 协议块 `[model."<id>".<协议>]`，键是协议名。
    pub overrides: BTreeMap<String, BackendEntry>,
    /// `provider = "<id>"`：连接继承自 `[provider.<id>]`。设了它，地址、协议、密钥
    /// 都可以不写（写了的覆盖供应商的）。
    pub provider: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct PricingEntry {
    pub input: Option<f64>,
    pub cache_read: Option<f64>,
    pub cache_write: Option<f64>,
    pub output: Option<f64>,
}

impl PricingEntry {
    fn is_empty(&self) -> bool {
        self.input.is_none()
            && self.cache_read.is_none()
            && self.cache_write.is_none()
            && self.output.is_none()
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct BackendEntry {
    pub api_base_url: Option<String>,
    pub auth_scheme: Option<String>,
    pub api_model: Option<String>,
}

impl BackendEntry {
    fn is_empty(&self) -> bool {
        self.api_base_url.is_none() && self.auth_scheme.is_none() && self.api_model.is_none()
    }
}

/// 协议块在文件里可能用别名（`resp` / `chat` / `anthropic`），读的时候都认。
const BACKEND_KEYS: &[(ApiBackend, &[&str])] = &[
    (ApiBackend::Responses, &["responses", "resp"]),
    (
        ApiBackend::ChatCompletions,
        &["chat_completions", "chat-completions", "chat"],
    ),
    (ApiBackend::Messages, &["messages", "anthropic"]),
];

/// 这个 id 在用户配置里有没有自己的 `[model."<id>"]`（有才可编辑）。
pub fn user_model_ids_in(path: &Path) -> Vec<String> {
    read_doc(path)
        .get("model")
        .and_then(|t| t.as_table_like())
        .map(|t| t.iter().map(|(k, _)| k.to_string()).collect())
        .unwrap_or_default()
}

pub fn user_model_ids() -> Vec<String> {
    user_model_ids_in(&user_config_path())
}

fn str_of(t: &dyn toml_edit::TableLike, key: &str) -> Option<String> {
    t.get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

fn int_of(t: &dyn toml_edit::TableLike, key: &str) -> Option<u64> {
    t.get(key)
        .and_then(|v| v.as_integer())
        .and_then(|n| u64::try_from(n).ok())
}

fn float_of(t: &dyn toml_edit::TableLike, key: &str) -> Option<f64> {
    let v = t.get(key)?;
    v.as_float().or_else(|| v.as_integer().map(|n| n as f64))
}

fn bool_of(t: &dyn toml_edit::TableLike, key: &str) -> Option<bool> {
    t.get(key).and_then(|v| v.as_bool())
}

fn strs_of(t: &dyn toml_edit::TableLike, key: &str) -> Option<Vec<String>> {
    t.get(key).and_then(|v| v.as_array()).map(|a| {
        a.iter()
            .filter_map(|v| v.as_str())
            .map(String::from)
            .collect()
    })
}

/// 读出用户配置里的一条模型，给编辑表单用。不给行内密钥本身。
pub fn read_model_in(path: &Path, id: &str) -> Option<ModelEntry> {
    let doc = read_doc(path);
    let table = doc.get("model")?.get(id)?.as_table_like()?;
    let mut backends: Vec<String> = Vec::new();
    let mut push = |raw: &str| {
        if let Some(b) = ApiBackend::from_name(raw) {
            let name = b.name().to_string();
            if !backends.contains(&name) {
                backends.push(name);
            }
        }
    };
    if let Some(single) = str_of(table, "api_backend") {
        push(&single);
    }
    for raw in strs_of(table, "api_backends").unwrap_or_default() {
        push(&raw);
    }
    let provider = str_of(table, "provider");
    // 挂在供应商下的模型没写协议 = 跟供应商走，表单里保持空。
    if backends.is_empty() && provider.is_none() {
        backends.push(ApiBackend::default().name().to_string());
    }
    let pricing = table
        .get("pricing")
        .and_then(|p| p.as_table_like())
        .map(|p| PricingEntry {
            input: float_of(p, "input"),
            cache_read: float_of(p, "cache_read"),
            cache_write: float_of(p, "cache_write"),
            output: float_of(p, "output"),
        })
        .filter(|p| !p.is_empty());
    let mut overrides = BTreeMap::new();
    for (backend, keys) in BACKEND_KEYS {
        let Some(row) = keys
            .iter()
            .find_map(|k| table.get(k).and_then(|t| t.as_table_like()))
        else {
            continue;
        };
        let entry = BackendEntry {
            api_base_url: str_of(row, "api_base_url").or_else(|| str_of(row, "api_base")),
            auth_scheme: str_of(row, "auth_scheme"),
            api_model: str_of(row, "api_model"),
        };
        if !entry.is_empty() {
            overrides.insert(backend.name().to_string(), entry);
        }
    }
    let has_api_key = str_of(table, "api_key").is_some();
    Some(ModelEntry {
        id: id.to_string(),
        name: str_of(table, "name"),
        description: str_of(table, "description"),
        api_base_url: str_of(table, "api_base_url").or_else(|| str_of(table, "api_base")),
        api_model: str_of(table, "api_model"),
        api_backends: backends,
        auth_scheme: str_of(table, "auth_scheme"),
        key_mode: if has_api_key { "inline" } else { "env" }.into(),
        env_key: str_of(table, "env_key"),
        api_key: None,
        has_api_key,
        context_window: int_of(table, "context_window"),
        max_output_tokens: int_of(table, "max_output_tokens"),
        reasoning: bool_of(table, "reasoning"),
        reasoning_effort: str_of(table, "reasoning_effort"),
        reasoning_efforts: strs_of(table, "reasoning_efforts"),
        supports_images: bool_of(table, "supports_images"),
        prompt_cache: bool_of(table, "prompt_cache"),
        pricing,
        overrides,
        provider,
    })
}

pub fn read_model(id: &str) -> Option<ModelEntry> {
    read_model_in(&user_config_path(), id)
}

fn trimmed(value: &Option<String>) -> Option<&str> {
    value.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

/// 保存前的校验。错误文案直接给人看。
pub fn validate_model(entry: &ModelEntry) -> Result<(), String> {
    let id = entry.id.trim();
    if id.is_empty() {
        return Err("模型 id 不能为空".into());
    }
    if id.chars().any(|c| c.is_control() || c == '"') {
        return Err("模型 id 里不能有引号或控制字符".into());
    }
    let inherits = trimmed(&entry.provider).is_some();
    match trimmed(&entry.api_base_url) {
        None if inherits => {}
        None => return Err("接口地址不能为空".into()),
        Some(url) if !is_http_url(url) => return Err(format!("接口地址不是合法 URL：{url}")),
        _ => {}
    }
    if entry.api_backends.is_empty() && !inherits {
        return Err("至少勾选一个协议".into());
    }
    for name in &entry.api_backends {
        if ApiBackend::from_name(name).is_none() {
            return Err(format!("不认识的协议：{name}"));
        }
    }
    if let Some(scheme) = trimmed(&entry.auth_scheme) {
        if super::AuthScheme::parse(Some(scheme)).is_none() {
            return Err(format!("鉴权方式只能是 bearer / x_api_key：{scheme}"));
        }
    }
    match entry.key_mode.as_str() {
        "env" | "inline" => {}
        other => return Err(format!("密钥来源只能是 env / inline：{other}")),
    }
    for (backend, row) in &entry.overrides {
        if ApiBackend::from_name(backend).is_none() {
            return Err(format!("不认识的协议：{backend}"));
        }
        if let Some(url) = trimmed(&row.api_base_url) {
            if !is_http_url(url) {
                return Err(format!("{backend} 的地址不是合法 URL：{url}"));
            }
        }
        if let Some(scheme) = trimmed(&row.auth_scheme) {
            if super::AuthScheme::parse(Some(scheme)).is_none() {
                return Err(format!("{backend} 的鉴权方式只能是 bearer / x_api_key"));
            }
        }
    }
    Ok(())
}

fn is_http_url(url: &str) -> bool {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"));
    rest.is_some_and(|r| {
        let host = r.split(['/', '?', '#']).next().unwrap_or("");
        !host.is_empty() && !host.contains(char::is_whitespace)
    })
}

fn set_or_remove(table: &mut toml_edit::Table, key: &str, value: Option<toml_edit::Value>) {
    match value {
        Some(v) => table[key] = toml_edit::Item::Value(v),
        None => {
            table.remove(key);
        }
    }
}

fn str_value(value: &Option<String>) -> Option<toml_edit::Value> {
    trimmed(value).map(toml_edit::Value::from)
}

fn str_array(items: &[String]) -> toml_edit::Value {
    let mut arr = toml_edit::Array::new();
    for item in items {
        arr.push(item.as_str());
    }
    toml_edit::Value::Array(arr)
}

/// 把表单写进一张 `[model."<id>"]` 表。就地改：表里这里不管的键原样留着。
fn write_model_table(table: &mut toml_edit::Table, entry: &ModelEntry) {
    set_or_remove(table, "provider", str_value(&entry.provider));
    set_or_remove(table, "name", str_value(&entry.name));
    set_or_remove(table, "description", str_value(&entry.description));
    table.remove("api_base");
    set_or_remove(table, "api_base_url", str_value(&entry.api_base_url));
    set_or_remove(table, "api_model", str_value(&entry.api_model));
    // 单数写法会被提到队首，和列表一起写就有两个「默认」：统一成列表。
    table.remove("api_backend");
    let backends: Vec<String> = entry
        .api_backends
        .iter()
        .filter_map(|b| ApiBackend::from_name(b))
        .map(|b| b.name().to_string())
        .collect();
    // 空列表只在挂了供应商时出现（校验挡住了别的情况）：不写 = 跟供应商走。
    set_or_remove(
        table,
        "api_backends",
        (!backends.is_empty()).then(|| str_array(&backends)),
    );
    set_or_remove(table, "auth_scheme", str_value(&entry.auth_scheme));
    if entry.key_mode == "inline" {
        table.remove("env_key");
        match entry.api_key.as_deref().map(str::trim) {
            // 没带新值：保留文件里的。
            None => {}
            Some("") => {
                table.remove("api_key");
            }
            Some(key) => table["api_key"] = toml_edit::value(key),
        }
    } else {
        table.remove("api_key");
        set_or_remove(table, "env_key", str_value(&entry.env_key));
    }
    let int = |n: Option<u64>| {
        n.filter(|n| *n > 0)
            .map(|n| toml_edit::Value::from(n as i64))
    };
    set_or_remove(table, "context_window", int(entry.context_window));
    set_or_remove(table, "max_output_tokens", int(entry.max_output_tokens));
    set_or_remove(
        table,
        "reasoning",
        entry.reasoning.map(toml_edit::Value::from),
    );
    set_or_remove(
        table,
        "reasoning_effort",
        str_value(&entry.reasoning_effort),
    );
    set_or_remove(
        table,
        "reasoning_efforts",
        entry.reasoning_efforts.as_deref().map(str_array),
    );
    set_or_remove(
        table,
        "supports_images",
        entry.supports_images.map(toml_edit::Value::from),
    );
    // 提示缓存只对 messages 有意义：没勾 messages 就不留这个键。
    let has_messages = backends.iter().any(|b| b == ApiBackend::Messages.name());
    set_or_remove(
        table,
        "prompt_cache",
        entry
            .prompt_cache
            .filter(|_| has_messages)
            .map(toml_edit::Value::from),
    );
    match entry.pricing.as_ref().filter(|p| !p.is_empty()) {
        None => {
            table.remove("pricing");
        }
        Some(p) => {
            let mut sub = match table.remove("pricing") {
                Some(toml_edit::Item::Table(t)) => t,
                _ => toml_edit::Table::new(),
            };
            let price = |v: Option<f64>| {
                v.filter(|v| v.is_finite() && *v >= 0.0)
                    .map(toml_edit::Value::from)
            };
            set_or_remove(&mut sub, "input", price(p.input));
            set_or_remove(&mut sub, "cache_read", price(p.cache_read));
            set_or_remove(&mut sub, "cache_write", price(p.cache_write));
            set_or_remove(&mut sub, "output", price(p.output));
            table["pricing"] = toml_edit::Item::Table(sub);
        }
    }
    for (backend, keys) in BACKEND_KEYS {
        // 别名写法先收掉，统一成正式名。
        let mut sub = keys
            .iter()
            .find_map(|k| match table.remove(k) {
                Some(toml_edit::Item::Table(t)) => Some(t),
                _ => None,
            })
            .unwrap_or_default();
        for k in keys.iter() {
            table.remove(k);
        }
        let Some(row) = entry
            .overrides
            .get(backend.name())
            .filter(|r| !r.is_empty())
            .filter(|_| backends.iter().any(|b| b == backend.name()))
        else {
            continue;
        };
        sub.remove("api_base");
        set_or_remove(&mut sub, "api_base_url", str_value(&row.api_base_url));
        set_or_remove(&mut sub, "auth_scheme", str_value(&row.auth_scheme));
        set_or_remove(&mut sub, "api_model", str_value(&row.api_model));
        table[backend.name()] = toml_edit::Item::Table(sub);
    }
}

/// 新建或保存一条模型。`original_id` 是编辑前的 id（改名时和 `entry.id` 不同）。
/// `make_default` 同时把它设成 `[models].default`。
pub fn save_model_in(
    path: &Path,
    entry: &ModelEntry,
    original_id: Option<&str>,
    make_default: bool,
    other_ids: &[String],
) -> Result<(), String> {
    validate_model(entry)?;
    let id = entry.id.trim().to_string();
    let renaming = original_id.is_some_and(|o| o != id);
    if (original_id.is_none() || renaming) && other_ids.contains(&id) {
        return Err(format!("已存在同名模型 id：{id}"));
    }
    edit(path, |doc| {
        implicit_table(doc, "model")?;
        let models = doc["model"]
            .as_table_mut()
            .ok_or("config.toml 里的 model 不是表")?;
        let mut table = match original_id.and_then(|o| models.remove(o)) {
            Some(toml_edit::Item::Table(t)) => t,
            _ => toml_edit::Table::new(),
        };
        // 改名后 `model = "<旧 id>"` 会把它改回去。
        table.remove("model");
        write_model_table(&mut table, entry);
        models.insert(&id, toml_edit::Item::Table(table));
        let was_default = original_id.is_some_and(|o| default_of(doc).as_deref() == Some(o));
        if make_default || (renaming && was_default) {
            set_default(doc, &id)?;
        }
        Ok(())
    })
}

pub fn save_model(
    entry: &ModelEntry,
    original_id: Option<&str>,
    make_default: bool,
    other_ids: &[String],
) -> Result<(), String> {
    save_model_in(
        &user_config_path(),
        entry,
        original_id,
        make_default,
        other_ids,
    )
}

fn default_of(doc: &toml_edit::DocumentMut) -> Option<String> {
    doc.get("models")?
        .get("default")?
        .as_str()
        .map(String::from)
}

fn set_default(doc: &mut toml_edit::DocumentMut, id: &str) -> Result<(), String> {
    match doc.get("models") {
        Some(item) if item.is_table_like() => {}
        Some(_) => return Err("config.toml 里的 models 不是表".into()),
        None => doc["models"] = toml_edit::table(),
    }
    doc["models"]["default"] = toml_edit::value(id);
    Ok(())
}

/// 删一条模型。删的是默认模型时，默认改成 `next_default`（`None` = 去掉默认）。
pub fn delete_model_in(path: &Path, id: &str, next_default: Option<&str>) -> Result<(), String> {
    edit(path, |doc| {
        let removed = doc
            .get_mut("model")
            .and_then(|t| t.as_table_like_mut())
            .and_then(|t| t.remove(id));
        if removed.is_none() {
            return Err(format!("用户配置里没有模型 {id}（可能写在项目配置里）"));
        }
        if default_of(doc).as_deref() == Some(id) {
            match next_default {
                Some(next) => set_default(doc, next)?,
                None => {
                    if let Some(models) = doc.get_mut("models").and_then(|t| t.as_table_like_mut())
                    {
                        models.remove("default");
                    }
                }
            }
        }
        Ok(())
    })
}

pub fn delete_model(id: &str, next_default: Option<&str>) -> Result<(), String> {
    delete_model_in(&user_config_path(), id, next_default)
}

pub fn set_default_model_in(path: &Path, id: &str) -> Result<(), String> {
    edit(path, |doc| set_default(doc, id))
}

pub fn set_default_model(id: &str) -> Result<(), String> {
    set_default_model_in(&user_config_path(), id)
}

/// 表单 → 运行时的 [`ModelChoice`]，走和读配置文件**同一条**解析路径（测试连接用，
/// 不落盘）。`existing_key` 是文件里原有的行内密钥（表单没带新值时用它）。
pub fn choice_from_entry(
    entry: &ModelEntry,
    existing_key: Option<String>,
) -> Result<ModelChoice, String> {
    validate_model(entry)?;
    let mut entry = entry.clone();
    if entry.key_mode == "inline" && entry.api_key.is_none() {
        entry.api_key = existing_key;
    }
    let mut doc = toml_edit::DocumentMut::new();
    implicit_table(&mut doc, "model")?;
    let mut table = toml_edit::Table::new();
    write_model_table(&mut table, &entry);
    doc["model"][entry.id.trim()] = toml_edit::Item::Table(table);
    let file: FileConfig = toml::from_str(&doc.to_string()).map_err(|e| e.to_string())?;
    let providers = load_providers_from(&catalog_paths());
    let mut list = Vec::new();
    merge_overrides(&mut list, &file.model, &providers);
    list.into_iter()
        .next()
        .ok_or_else(|| "表单没生成模型".into())
}

/// 文件里某条模型的行内密钥（测试连接时表单没带新值就用它）。
pub fn inline_key(id: &str) -> Option<String> {
    let doc = read_doc(&user_config_path());
    let table = doc.get("model")?.get(id)?.as_table_like()?;
    str_of(table, "api_key")
}

// ---------------------------------------------------------------------------
// 供应商
// ---------------------------------------------------------------------------

/// 设置页编辑的一条 `[provider."<id>"]`。
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ProviderEntry {
    pub id: String,
    /// 给人看的名字（只给界面用，Dock 不读）。
    pub name: Option<String>,
    /// 来自哪个目录条目（models.dev 的供应商 id；只给界面用，Dock 不读）。
    pub catalog: Option<String>,
    pub api_base_url: Option<String>,
    /// 第一条 = 默认协议。
    pub api_backends: Vec<String>,
    pub auth_scheme: Option<String>,
    /// 密钥的名字：先找同名环境变量，再找 `secrets.json` 里同名的那项。
    pub env_key: Option<String>,
    /// 只出不进：文件里写了行内 `api_key`（老写法；设置页不再写它）。
    pub has_api_key: bool,
    /// 协议块 `[provider."<id>".<协议>]`，键是协议名。
    pub overrides: BTreeMap<String, BackendEntry>,
}

/// 供应商 id 的形状：同密钥名的规则（字母开头，字母数字 `_` `-`），
/// 这样 `<id>/<model>` 这种模型 id 和默认的密钥名都不用转义。
pub fn valid_provider_id(id: &str) -> bool {
    let mut chars = id.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && id.len() <= 64
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn read_provider_table(id: &str, table: &dyn toml_edit::TableLike) -> ProviderEntry {
    let mut backends: Vec<String> = Vec::new();
    let raw = str_of(table, "api_backend")
        .into_iter()
        .chain(strs_of(table, "api_backends").unwrap_or_default());
    for raw in raw {
        if let Some(b) = ApiBackend::from_name(&raw) {
            let name = b.name().to_string();
            if !backends.contains(&name) {
                backends.push(name);
            }
        }
    }
    if backends.is_empty() {
        backends.push(ApiBackend::default().name().to_string());
    }
    let mut overrides = BTreeMap::new();
    for (backend, keys) in BACKEND_KEYS {
        let Some(row) = keys
            .iter()
            .find_map(|k| table.get(k).and_then(|t| t.as_table_like()))
        else {
            continue;
        };
        let entry = BackendEntry {
            api_base_url: str_of(row, "api_base_url").or_else(|| str_of(row, "api_base")),
            auth_scheme: str_of(row, "auth_scheme"),
            api_model: None,
        };
        if !entry.is_empty() {
            overrides.insert(backend.name().to_string(), entry);
        }
    }
    ProviderEntry {
        id: id.to_string(),
        name: str_of(table, "name"),
        catalog: str_of(table, "catalog"),
        api_base_url: str_of(table, "api_base_url").or_else(|| str_of(table, "api_base")),
        api_backends: backends,
        auth_scheme: str_of(table, "auth_scheme"),
        env_key: str_of(table, "env_key"),
        has_api_key: str_of(table, "api_key").is_some(),
        overrides,
    }
}

/// 用户配置里的全部供应商（按文件里的顺序）。
pub fn read_providers_in(path: &Path) -> Vec<ProviderEntry> {
    let doc = read_doc(path);
    let Some(table) = doc.get("provider").and_then(|t| t.as_table_like()) else {
        return Vec::new();
    };
    table
        .iter()
        .filter_map(|(id, item)| Some(read_provider_table(id, item.as_table_like()?)))
        .collect()
}

pub fn read_providers() -> Vec<ProviderEntry> {
    read_providers_in(&user_config_path())
}

pub fn validate_provider(entry: &ProviderEntry) -> Result<(), String> {
    let id = entry.id.trim();
    if !valid_provider_id(id) {
        return Err(format!(
            "供应商 id 只能用字母开头，后面跟字母、数字、_ 或 -：{id:?}"
        ));
    }
    match trimmed(&entry.api_base_url) {
        None => return Err("接口地址不能为空".into()),
        Some(url) if !is_http_url(url) => return Err(format!("接口地址不是合法 URL：{url}")),
        _ => {}
    }
    if entry.api_backends.is_empty() {
        return Err("至少选一个协议".into());
    }
    for name in entry.api_backends.iter().chain(entry.overrides.keys()) {
        if ApiBackend::from_name(name).is_none() {
            return Err(format!("不认识的协议：{name}"));
        }
    }
    for scheme in
        std::iter::once(&entry.auth_scheme).chain(entry.overrides.values().map(|o| &o.auth_scheme))
    {
        if let Some(scheme) = trimmed(scheme) {
            if super::AuthScheme::parse(Some(scheme)).is_none() {
                return Err(format!("鉴权方式只能是 bearer / x_api_key：{scheme}"));
            }
        }
    }
    for (backend, row) in &entry.overrides {
        if let Some(url) = trimmed(&row.api_base_url) {
            if !is_http_url(url) {
                return Err(format!("{backend} 的地址不是合法 URL：{url}"));
            }
        }
    }
    if let Some(name) = trimmed(&entry.env_key) {
        if !valid_provider_id(name) {
            return Err(format!(
                "密钥名只能用字母开头，后面跟字母、数字、_ 或 -：{name}"
            ));
        }
    }
    Ok(())
}

fn write_provider_table(table: &mut toml_edit::Table, entry: &ProviderEntry) {
    set_or_remove(table, "name", str_value(&entry.name));
    set_or_remove(table, "catalog", str_value(&entry.catalog));
    table.remove("api_base");
    set_or_remove(table, "api_base_url", str_value(&entry.api_base_url));
    table.remove("api_backend");
    let backends: Vec<String> = entry
        .api_backends
        .iter()
        .filter_map(|b| ApiBackend::from_name(b))
        .map(|b| b.name().to_string())
        .collect();
    set_or_remove(table, "api_backends", Some(str_array(&backends)));
    set_or_remove(table, "auth_scheme", str_value(&entry.auth_scheme));
    set_or_remove(table, "env_key", str_value(&entry.env_key));
    for (backend, keys) in BACKEND_KEYS {
        let mut sub = keys
            .iter()
            .find_map(|k| match table.remove(k) {
                Some(toml_edit::Item::Table(t)) => Some(t),
                _ => None,
            })
            .unwrap_or_default();
        for k in keys.iter() {
            table.remove(k);
        }
        let Some(row) = entry
            .overrides
            .get(backend.name())
            .filter(|r| trimmed(&r.api_base_url).is_some() || trimmed(&r.auth_scheme).is_some())
            .filter(|_| backends.iter().any(|b| b == backend.name()))
        else {
            continue;
        };
        sub.remove("api_base");
        set_or_remove(&mut sub, "api_base_url", str_value(&row.api_base_url));
        set_or_remove(&mut sub, "auth_scheme", str_value(&row.auth_scheme));
        table[backend.name()] = toml_edit::Item::Table(sub);
    }
}

/// 新建或保存一个供应商。`original_id` 是编辑前的 id：改名时挂在它下面的模型跟着改
/// `provider = ...`（模型 id 不动，旧会话记着它）。
pub fn save_provider_in(
    path: &Path,
    entry: &ProviderEntry,
    original_id: Option<&str>,
) -> Result<(), String> {
    validate_provider(entry)?;
    let id = entry.id.trim().to_string();
    edit(path, |doc| {
        implicit_table(doc, "provider")?;
        let providers = doc["provider"]
            .as_table_mut()
            .ok_or("config.toml 里的 provider 不是表")?;
        let renaming = original_id.is_some_and(|o| o != id);
        if (original_id.is_none() || renaming) && providers.contains_key(&id) {
            return Err(format!("已存在同名供应商：{id}"));
        }
        let mut table = match original_id.and_then(|o| providers.remove(o)) {
            Some(toml_edit::Item::Table(t)) => t,
            _ => toml_edit::Table::new(),
        };
        write_provider_table(&mut table, entry);
        providers.insert(&id, toml_edit::Item::Table(table));
        if let (true, Some(old)) = (renaming, original_id) {
            for (_, model) in doc
                .get_mut("model")
                .and_then(|t| t.as_table_like_mut())
                .into_iter()
                .flat_map(|t| t.iter_mut())
            {
                if let Some(t) = model.as_table_like_mut() {
                    if t.get("provider").and_then(|v| v.as_str()) == Some(old) {
                        t.insert("provider", toml_edit::value(id.as_str()));
                    }
                }
            }
        }
        Ok(())
    })
}

pub fn save_provider(entry: &ProviderEntry, original_id: Option<&str>) -> Result<(), String> {
    save_provider_in(&user_config_path(), entry, original_id)
}

/// 删一个供应商和挂在它下面的模型。删掉的模型里有默认模型时，默认改成剩下的
/// 第一条（`other_ids` 里第一个没被删的；没有就去掉默认）。返回删掉的模型 id。
pub fn delete_provider_in(
    path: &Path,
    id: &str,
    other_ids: &[String],
) -> Result<Vec<String>, String> {
    let mut removed = Vec::new();
    edit(path, |doc| {
        let gone = doc
            .get_mut("provider")
            .and_then(|t| t.as_table_like_mut())
            .and_then(|t| t.remove(id));
        if gone.is_none() {
            return Err(format!("用户配置里没有供应商 {id}"));
        }
        if let Some(models) = doc.get_mut("model").and_then(|t| t.as_table_like_mut()) {
            let ids: Vec<String> = models
                .iter()
                .filter(|(_, m)| m.get("provider").and_then(|v| v.as_str()) == Some(id))
                .map(|(k, _)| k.to_string())
                .collect();
            for m in &ids {
                models.remove(m);
            }
            removed = ids;
        }
        if let Some(default) = default_of(doc).filter(|d| removed.contains(d)) {
            let next = other_ids
                .iter()
                .find(|m| !removed.contains(m) && **m != default);
            match next {
                Some(next) => set_default(doc, next)?,
                None => {
                    if let Some(models) = doc.get_mut("models").and_then(|t| t.as_table_like_mut())
                    {
                        models.remove("default");
                    }
                }
            }
        }
        Ok(())
    })?;
    Ok(removed)
}

pub fn delete_provider(id: &str, other_ids: &[String]) -> Result<Vec<String>, String> {
    delete_provider_in(&user_config_path(), id, other_ids)
}

/// 供应商表单 → 运行时的 [`ModelChoice`]（列模型、测连接用，不落盘）：挂一条临时
/// 模型到这个供应商上，走和读配置文件同一条继承路径。`api_key` 是表单里刚填、还没存的
/// 密钥；`None` = 按 `env_key`（环境变量 → secrets.json）或文件里的行内密钥取。
pub fn choice_from_provider(
    entry: &ProviderEntry,
    api_key: Option<String>,
) -> Result<ModelChoice, String> {
    validate_provider(entry)?;
    let id = entry.id.trim();
    let mut doc = toml_edit::DocumentMut::new();
    implicit_table(&mut doc, "provider")?;
    let mut table = toml_edit::Table::new();
    write_provider_table(&mut table, entry);
    let inline = api_key
        .filter(|k| !k.trim().is_empty())
        .or_else(|| provider_inline_key(id));
    if let Some(key) = inline {
        table["api_key"] = toml_edit::value(key.trim());
    }
    doc["provider"][id] = toml_edit::Item::Table(table);
    let file: FileConfig = toml::from_str(&doc.to_string()).map_err(|e| e.to_string())?;
    let mut probe = BTreeMap::new();
    probe.insert(
        "__provider_probe".to_string(),
        super::ModelOverride {
            provider: Some(id.to_string()),
            ..Default::default()
        },
    );
    let mut list = Vec::new();
    merge_overrides(&mut list, &probe, &file.provider);
    list.into_iter()
        .next()
        .ok_or_else(|| "表单没生成供应商".into())
}

/// 文件里某个供应商的行内密钥（老写法；设置页不写它，但手写的照认）。
pub fn provider_inline_key(id: &str) -> Option<String> {
    let doc = read_doc(&user_config_path());
    let table = doc.get("provider")?.get(id)?.as_table_like()?;
    str_of(table, "api_key")
}

/// 用户配置里还有没有别的供应商 / 模型用着这个密钥名（删供应商时决定要不要连密钥一起删）。
pub fn key_name_in_use(name: &str) -> bool {
    let doc = read_doc(&user_config_path());
    ["provider", "model"].iter().any(|section| {
        doc.get(section)
            .and_then(|t| t.as_table_like())
            .is_some_and(|t| {
                t.iter()
                    .any(|(_, row)| row.get("env_key").and_then(|v| v.as_str()) == Some(name))
            })
    })
}

// ---------------------------------------------------------------------------
// 白名单标量键
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyKind {
    Bool,
    Int,
    Str,
    StrList,
    /// 读的时候只说有没有，不回值。
    Secret,
}

/// 设置页能直接改的键（点分路径）。不在这里的一律拒绝。
pub const SETTING_KEYS: &[(&str, KeyKind)] = &[
    ("browser.headed", KeyKind::Bool),
    ("toolset.web_fetch.timeout_secs", KeyKind::Int),
    ("toolset.web_fetch.max_content_length", KeyKind::Int),
    ("toolset.web_fetch.max_markdown_length", KeyKind::Int),
    ("toolset.web_fetch.allowed_domains", KeyKind::StrList),
    ("toolset.web_fetch.proxy_endpoint", KeyKind::Str),
    ("toolset.web_fetch.allow_local", KeyKind::Bool),
    ("memory.enabled", KeyKind::Bool),
    ("memory.flush.enabled", KeyKind::Bool),
    ("memory.flush.soft_threshold_tokens", KeyKind::Int),
    ("memory.flush.max_flush_write_chars", KeyKind::Int),
    ("memory.dream.enabled", KeyKind::Bool),
    ("memory.dream.min_hours", KeyKind::Int),
    ("memory.dream.min_sessions", KeyKind::Int),
    ("memory.embedding.model", KeyKind::Str),
    ("memory.embedding.base", KeyKind::Str),
    ("memory.embedding.dimensions", KeyKind::Int),
    ("memory.embedding.api_key", KeyKind::Secret),
];

fn key_kind(key: &str) -> Option<KeyKind> {
    SETTING_KEYS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, kind)| *kind)
}

fn lookup<'a>(doc: &'a toml_edit::DocumentMut, key: &str) -> Option<&'a toml_edit::Item> {
    let mut parts = key.split('.');
    let mut item = doc.get(parts.next()?)?;
    for part in parts {
        item = item.get(part)?;
    }
    Some(item)
}

/// 白名单里每个键在用户配置里的值；没写是 `null`。密钥类只回 `true` / `null`。
pub fn read_settings_in(path: &Path) -> Value {
    let doc = read_doc(path);
    let mut out = serde_json::Map::new();
    for (key, kind) in SETTING_KEYS {
        let item = lookup(&doc, key);
        let value = match (kind, item) {
            (_, None) => Value::Null,
            (KeyKind::Bool, Some(i)) => i.as_bool().map(Value::from).unwrap_or(Value::Null),
            (KeyKind::Int, Some(i)) => i.as_integer().map(Value::from).unwrap_or(Value::Null),
            (KeyKind::Str, Some(i)) => i.as_str().map(Value::from).unwrap_or(Value::Null),
            (KeyKind::StrList, Some(i)) => i
                .as_array()
                .map(|a| json!(a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>()))
                .unwrap_or(Value::Null),
            (KeyKind::Secret, Some(i)) => {
                if i.as_str().is_some_and(|s| !s.is_empty()) {
                    Value::Bool(true)
                } else {
                    Value::Null
                }
            }
        };
        out.insert((*key).to_string(), value);
    }
    Value::Object(out)
}

pub fn read_settings() -> Value {
    read_settings_in(&user_config_path())
}

fn to_toml(kind: KeyKind, key: &str, value: &Value) -> Result<toml_edit::Value, String> {
    let bad = || format!("{key} 的值类型不对");
    Ok(match kind {
        KeyKind::Bool => toml_edit::Value::from(value.as_bool().ok_or_else(bad)?),
        KeyKind::Int => {
            let n = value.as_i64().ok_or_else(bad)?;
            if n < 0 {
                return Err(format!("{key} 不能是负数"));
            }
            toml_edit::Value::from(n)
        }
        KeyKind::Str | KeyKind::Secret => {
            let s = value.as_str().ok_or_else(bad)?.trim();
            toml_edit::Value::from(s)
        }
        KeyKind::StrList => {
            let items: Vec<String> = value
                .as_array()
                .ok_or_else(bad)?
                .iter()
                .map(|v| v.as_str().map(|s| s.trim().to_string()).ok_or_else(bad))
                .collect::<Result<_, _>>()?;
            str_array(&items)
        }
    })
}

/// 写一个白名单键；`value` 为 `null`（或空字符串）= 删掉这个键，回到默认。
pub fn write_setting_in(path: &Path, key: &str, value: &Value) -> Result<(), String> {
    let kind = key_kind(key).ok_or_else(|| format!("设置页不能改 {key}"))?;
    let remove = value.is_null() || value.as_str().is_some_and(|s| s.trim().is_empty());
    let new = if remove {
        None
    } else {
        Some(to_toml(kind, key, value)?)
    };
    let parts: Vec<&str> = key.split('.').collect();
    let (leaf, parents) = parts.split_last().ok_or("空键")?;
    edit(path, |doc| {
        let mut table = doc.as_table_mut();
        for (depth, part) in parents.iter().enumerate() {
            if table.get(part).is_none() {
                if new.is_none() {
                    return Ok(());
                }
                let mut t = toml_edit::Table::new();
                // 中间层（如 `toolset`）只当前缀，不单独成段。
                t.set_implicit(depth + 1 < parents.len());
                table.insert(part, toml_edit::Item::Table(t));
            }
            table = table
                .get_mut(part)
                .and_then(|i| i.as_table_mut())
                .ok_or_else(|| format!("config.toml 里的 {part} 不是表"))?;
        }
        match new {
            Some(v) => {
                table.insert(leaf, toml_edit::Item::Value(v));
            }
            None => {
                table.remove(leaf);
            }
        }
        Ok(())
    })
}

pub fn write_setting(key: &str, value: &Value) -> Result<(), String> {
    write_setting_in(&user_config_path(), key, value)
}

// ---------------------------------------------------------------------------
// MCP 行
// ---------------------------------------------------------------------------

/// 设置页编辑的一条 `[mcp_servers.<name>]`。
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct McpEntry {
    pub name: String,
    /// `stdio` / `http`。
    pub transport: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub url: String,
    pub bearer_token_env_var: Option<String>,
    pub headers: BTreeMap<String, String>,
    pub oauth_client_id: Option<String>,
    pub oauth_scopes: Vec<String>,
    pub startup_timeout_sec: Option<u64>,
    pub enabled: bool,
}

fn map_of(t: &dyn toml_edit::TableLike, key: &str) -> BTreeMap<String, String> {
    t.get(key)
        .and_then(|v| v.as_table_like())
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.to_string(), s.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

/// 用户配置里的一条 MCP 行（编辑表单用）。
pub fn read_mcp_in(path: &Path, name: &str) -> Option<McpEntry> {
    let doc = read_doc(path);
    let t = doc.get("mcp_servers")?.get(name)?.as_table_like()?;
    let command = str_of(t, "command").unwrap_or_default();
    let url = str_of(t, "url")
        .or_else(|| str_of(t, "urlTemplate"))
        .or_else(|| str_of(t, "url_template"))
        .unwrap_or_default();
    let oauth = t.get("oauth").and_then(|o| o.as_table_like());
    Some(McpEntry {
        name: name.to_string(),
        transport: if command.is_empty() { "http" } else { "stdio" }.into(),
        command,
        args: strs_of(t, "args").unwrap_or_default(),
        env: map_of(t, "env"),
        url,
        bearer_token_env_var: str_of(t, "bearer_token_env_var"),
        headers: map_of(t, "headers"),
        oauth_client_id: oauth
            .and_then(|o| str_of(o, "client_id"))
            .or_else(|| str_of(t, "oauth_client_id")),
        oauth_scopes: oauth
            .and_then(|o| strs_of(o, "scopes"))
            .or_else(|| strs_of(t, "oauth_scopes"))
            .unwrap_or_default(),
        startup_timeout_sec: int_of(t, "startup_timeout_sec"),
        enabled: bool_of(t, "enabled").unwrap_or(true),
    })
}

pub fn read_mcp(name: &str) -> Option<McpEntry> {
    read_mcp_in(&user_config_path(), name)
}

/// 用户配置里定义了哪些 MCP 行。
pub fn user_mcp_names_in(path: &Path) -> Vec<String> {
    read_doc(path)
        .get("mcp_servers")
        .and_then(|t| t.as_table_like())
        .map(|t| t.iter().map(|(k, _)| k.to_string()).collect())
        .unwrap_or_default()
}

pub fn user_mcp_names() -> Vec<String> {
    user_mcp_names_in(&user_config_path())
}

fn valid_server_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn string_table(map: &BTreeMap<String, String>) -> toml_edit::InlineTable {
    let mut t = toml_edit::InlineTable::new();
    for (k, v) in map {
        if !k.trim().is_empty() {
            t.insert(k.trim(), toml_edit::Value::from(v.as_str()));
        }
    }
    t
}

pub fn save_mcp_in(
    path: &Path,
    entry: &McpEntry,
    original_name: Option<&str>,
    other_names: &[String],
) -> Result<(), String> {
    let name = entry.name.trim();
    if !valid_server_name(name) {
        return Err("名称只能用字母、数字、- 和 _".into());
    }
    let renaming = original_name.is_some_and(|o| o != name);
    if (original_name.is_none() || renaming) && other_names.iter().any(|o| o == name) {
        return Err(format!("已存在同名 MCP 服务：{name}"));
    }
    let stdio = match entry.transport.as_str() {
        "stdio" => true,
        "http" => false,
        other => return Err(format!("类型只能是 stdio / http：{other}")),
    };
    if stdio && entry.command.trim().is_empty() {
        return Err("命令不能为空".into());
    }
    if !stdio && !is_http_url(entry.url.trim()) {
        return Err("URL 不是合法地址".into());
    }
    edit(path, |doc| {
        implicit_table(doc, "mcp_servers")?;
        let servers = doc["mcp_servers"]
            .as_table_mut()
            .ok_or("config.toml 里的 mcp_servers 不是表")?;
        let mut t = match original_name.and_then(|o| servers.remove(o)) {
            Some(toml_edit::Item::Table(t)) => t,
            _ => toml_edit::Table::new(),
        };
        // 两种类型的键互斥：留着另一种的会被 untagged 解析先认走。
        for key in [
            "command",
            "args",
            "env",
            "url",
            "urlTemplate",
            "url_template",
            "type",
            "bearer_token_env_var",
            "headers",
            "oauth_client_id",
            "oauth_scopes",
            "name",
        ] {
            t.remove(key);
        }
        if stdio {
            t["command"] = toml_edit::value(entry.command.trim());
            let args: Vec<String> = entry
                .args
                .iter()
                .map(|a| a.trim().to_string())
                .filter(|a| !a.is_empty())
                .collect();
            t["args"] = toml_edit::Item::Value(str_array(&args));
            if !entry.env.is_empty() {
                t["env"] =
                    toml_edit::Item::Value(toml_edit::Value::InlineTable(string_table(&entry.env)));
            }
            t.remove("oauth");
        } else {
            t["url"] = toml_edit::value(entry.url.trim());
            set_or_remove(
                &mut t,
                "bearer_token_env_var",
                str_value(&entry.bearer_token_env_var),
            );
            if !entry.headers.is_empty() {
                t["headers"] = toml_edit::Item::Value(toml_edit::Value::InlineTable(string_table(
                    &entry.headers,
                )));
            }
            let client = trimmed(&entry.oauth_client_id);
            if client.is_some() || !entry.oauth_scopes.is_empty() {
                let mut o = match t.remove("oauth") {
                    Some(toml_edit::Item::Table(o)) => o,
                    _ => toml_edit::Table::new(),
                };
                set_or_remove(&mut o, "client_id", client.map(toml_edit::Value::from));
                if entry.oauth_scopes.is_empty() {
                    o.remove("scopes");
                } else {
                    o["scopes"] = toml_edit::Item::Value(str_array(&entry.oauth_scopes));
                }
                t["oauth"] = toml_edit::Item::Table(o);
            } else {
                t.remove("oauth");
            }
        }
        set_or_remove(
            &mut t,
            "startup_timeout_sec",
            entry
                .startup_timeout_sec
                .filter(|n| *n > 0)
                .map(|n| toml_edit::Value::from(n as i64)),
        );
        t["enabled"] = toml_edit::value(entry.enabled);
        servers.insert(name, toml_edit::Item::Table(t));
        if renaming {
            if let Some(disabled) = doc
                .get_mut("disabled_mcp_tools")
                .and_then(|t| t.as_table_like_mut())
            {
                if let Some(list) = disabled.remove(original_name.unwrap_or_default()) {
                    disabled.insert(name, list);
                }
            }
        }
        Ok(())
    })
}

pub fn save_mcp(
    entry: &McpEntry,
    original_name: Option<&str>,
    other_names: &[String],
) -> Result<(), String> {
    save_mcp_in(&user_config_path(), entry, original_name, other_names)
}

pub fn delete_mcp_in(path: &Path, name: &str) -> Result<(), String> {
    edit(path, |doc| {
        let removed = doc
            .get_mut("mcp_servers")
            .and_then(|t| t.as_table_like_mut())
            .and_then(|t| t.remove(name));
        if removed.is_none() {
            return Err(format!("用户配置里没有 MCP 服务 {name}"));
        }
        if let Some(disabled) = doc
            .get_mut("disabled_mcp_tools")
            .and_then(|t| t.as_table_like_mut())
        {
            disabled.remove(name);
            if disabled.is_empty() {
                doc.remove("disabled_mcp_tools");
            }
        }
        Ok(())
    })
}

pub fn delete_mcp(name: &str) -> Result<(), String> {
    delete_mcp_in(&user_config_path(), name)
}

/// 内置行（目前只有 cua-driver）：配置文件里没写它时由 Dock 自己注入。
pub fn is_builtin_mcp(name: &str) -> bool {
    super::builtin_mcp_servers()
        .iter()
        .any(|s: &McpServer| s.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(text: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, text).unwrap();
        (dir, path)
    }

    fn entry(id: &str) -> ModelEntry {
        ModelEntry {
            id: id.into(),
            name: Some("测试".into()),
            api_base_url: Some("https://api.example.com/v1".into()),
            api_backends: vec!["chat_completions".into(), "messages".into()],
            key_mode: "env".into(),
            env_key: Some("EXAMPLE_KEY".into()),
            ..ModelEntry::default()
        }
    }

    fn provider(id: &str) -> ProviderEntry {
        ProviderEntry {
            id: id.into(),
            name: Some("DeepSeek".into()),
            catalog: Some("deepseek".into()),
            api_base_url: Some("https://api.deepseek.com".into()),
            api_backends: vec!["chat_completions".into()],
            env_key: Some("DEEPSEEK_API_KEY".into()),
            ..ProviderEntry::default()
        }
    }

    #[test]
    fn provider_round_trip_and_models_without_their_own_connection() {
        let (_d, path) = tmp("# 我的配置\n[models]\ndefault = \"a\"\n\n[model.a]\napi_base_url = \"https://a.example/v1\"\n");
        save_provider_in(&path, &provider("deepseek"), None).unwrap();
        let read = read_providers_in(&path);
        assert_eq!(read.len(), 1);
        assert_eq!(read[0], provider("deepseek"));
        // 同名再建要挡住。
        assert!(save_provider_in(&path, &provider("deepseek"), None).is_err());

        // 挂在供应商下的模型：不写地址、协议、密钥也能存，读回来协议仍是空（跟供应商走）。
        let model = ModelEntry {
            id: "deepseek/deepseek-chat".into(),
            provider: Some("deepseek".into()),
            api_model: Some("deepseek-chat".into()),
            key_mode: "env".into(),
            context_window: Some(128000),
            ..ModelEntry::default()
        };
        save_model_in(&path, &model, None, false, &["a".into()]).unwrap();
        let back = read_model_in(&path, "deepseek/deepseek-chat").unwrap();
        assert_eq!(back.provider.as_deref(), Some("deepseek"));
        assert!(back.api_backends.is_empty());
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# 我的配置"), "{text}");
        assert!(!text.contains("api_backends = []"), "{text}");
        let choice = super::super::load_catalog_from(std::slice::from_ref(&path))
            .into_iter()
            .find(|m| m.id == "deepseek/deepseek-chat")
            .unwrap();
        assert_eq!(
            choice.api_base_url.as_deref(),
            Some("https://api.deepseek.com")
        );
        assert_eq!(choice.env_key.as_deref(), Some("DEEPSEEK_API_KEY"));

        // 不挂供应商的模型照旧要求地址。
        let bare = ModelEntry {
            provider: None,
            ..model.clone()
        };
        assert!(validate_model(&bare).is_err());
    }

    #[test]
    fn renaming_a_provider_moves_its_models_and_delete_takes_them_along() {
        let (_d, path) = tmp(
            "[models]\ndefault = \"p/x\"\n\n[model.a]\napi_base_url = \"https://a.example/v1\"\n",
        );
        save_provider_in(&path, &provider("p"), None).unwrap();
        for id in ["p/x", "p/y"] {
            let m = ModelEntry {
                id: id.into(),
                provider: Some("p".into()),
                key_mode: "env".into(),
                ..ModelEntry::default()
            };
            save_model_in(&path, &m, None, false, &[]).unwrap();
        }
        save_provider_in(&path, &provider("q"), Some("p")).unwrap();
        assert_eq!(
            read_model_in(&path, "p/x").unwrap().provider.as_deref(),
            Some("q")
        );
        assert!(read_providers_in(&path).iter().all(|p| p.id == "q"));

        let removed =
            delete_provider_in(&path, "q", &["p/x".into(), "p/y".into(), "a".into()]).unwrap();
        assert_eq!(removed, vec!["p/x".to_string(), "p/y".to_string()]);
        assert!(read_model_in(&path, "p/x").is_none());
        assert!(read_model_in(&path, "a").is_some());
        // 默认模型被删了：改成剩下的那条。
        assert_eq!(default_of(&read_doc(&path)).as_deref(), Some("a"));
        assert!(delete_provider_in(&path, "q", &[]).is_err());
    }

    #[test]
    fn provider_ids_and_key_names_are_checked() {
        assert!(valid_provider_id("moonshot-cn"));
        assert!(!valid_provider_id("1abc"));
        assert!(!valid_provider_id("a.b"));
        assert!(!valid_provider_id(""));
        let mut p = provider("ok");
        p.env_key = Some("BAD NAME".into());
        assert!(validate_provider(&p).is_err());
        p.env_key = None;
        p.api_backends.clear();
        assert!(validate_provider(&p).is_err());
    }

    #[test]
    fn choice_from_provider_uses_the_form_key() {
        let mut p = provider("form");
        p.overrides.insert(
            "messages".into(),
            BackendEntry {
                api_base_url: Some("https://api.deepseek.com/anthropic".into()),
                ..Default::default()
            },
        );
        p.api_backends.push("messages".into());
        let choice = choice_from_provider(&p, Some("sk-form".into())).unwrap();
        assert_eq!(choice.resolved_api_key().as_deref(), Some("sk-form"));
        assert_eq!(
            choice.base_url_for(ApiBackend::Messages),
            Some("https://api.deepseek.com/anthropic")
        );
    }

    #[test]
    fn save_new_model_keeps_comments_and_parses_back() {
        let (_d, path) = tmp("# 我的配置\n[models]\ndefault = \"a\"\n\n[model.a]\napi_base_url = \"https://a.example/v1\"\n");
        save_model_in(&path, &entry("vendor/b"), None, false, &["a".into()]).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# 我的配置"), "{text}");
        assert!(text.contains("[model.\"vendor/b\"]"), "{text}");
        let back = read_model_in(&path, "vendor/b").unwrap();
        assert_eq!(back.api_backends, vec!["chat_completions", "messages"]);
        assert_eq!(back.env_key.as_deref(), Some("EXAMPLE_KEY"));
        let catalog = super::super::load_catalog_from(&[path]);
        assert!(catalog.iter().any(|m| m.id == "vendor/b"));
    }

    #[test]
    fn duplicate_and_invalid_are_rejected() {
        let (_d, path) = tmp("");
        let err = save_model_in(&path, &entry("a"), None, false, &["a".into()]).unwrap_err();
        assert!(err.contains("同名"), "{err}");
        let mut bad = entry("b");
        bad.api_base_url = Some("not-a-url".into());
        assert!(save_model_in(&path, &bad, None, false, &[])
            .unwrap_err()
            .contains("URL"));
        let mut none = entry("c");
        none.api_backends.clear();
        assert!(save_model_in(&path, &none, None, false, &[])
            .unwrap_err()
            .contains("协议"));
    }

    #[test]
    fn inline_key_is_kept_when_form_sends_none() {
        let (_d, path) =
            tmp("[model.a]\napi_base_url = \"https://a.example/v1\"\napi_key = \"sk-old\"\n");
        let mut e = read_model_in(&path, "a").unwrap();
        assert!(e.has_api_key);
        assert_eq!(e.key_mode, "inline");
        e.name = Some("改名".into());
        save_model_in(&path, &e, Some("a"), false, &["a".into()]).unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("sk-old"));
        e.key_mode = "env".into();
        e.env_key = Some("A_KEY".into());
        save_model_in(&path, &e, Some("a"), false, &["a".into()]).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("sk-old"), "{text}");
    }

    #[test]
    fn rename_moves_default_and_delete_picks_next() {
        let (_d, path) =
            tmp("[models]\ndefault = \"a\"\n[model.a]\napi_base_url = \"https://a.example/v1\"\n");
        let mut e = read_model_in(&path, "a").unwrap();
        e.id = "a2".into();
        save_model_in(&path, &e, Some("a"), false, &["a".into()]).unwrap();
        assert_eq!(
            super::super::load_default_model_from(std::slice::from_ref(&path)).as_deref(),
            Some("a2")
        );
        save_model_in(&path, &entry("b"), None, false, &["a2".into()]).unwrap();
        delete_model_in(&path, "a2", Some("b")).unwrap();
        assert_eq!(
            super::super::load_default_model_from(std::slice::from_ref(&path)).as_deref(),
            Some("b")
        );
        assert!(read_model_in(&path, "a2").is_none());
    }

    #[test]
    fn overrides_and_pricing_round_trip() {
        let (_d, path) = tmp("");
        let mut e = entry("d");
        e.pricing = Some(PricingEntry {
            input: Some(0.14),
            output: Some(0.28),
            ..PricingEntry::default()
        });
        e.prompt_cache = Some(false);
        e.overrides.insert(
            "messages".into(),
            BackendEntry {
                api_base_url: Some("https://api.example.com/anthropic".into()),
                auth_scheme: Some("bearer".into()),
                api_model: None,
            },
        );
        save_model_in(&path, &e, None, true, &[]).unwrap();
        let back = read_model_in(&path, "d").unwrap();
        assert_eq!(back.pricing, e.pricing);
        assert_eq!(back.overrides, e.overrides);
        assert_eq!(back.prompt_cache, Some(false));
        let choice = &super::super::load_catalog_from(std::slice::from_ref(&path))[0];
        assert_eq!(
            choice.base_url_for(ApiBackend::Messages),
            Some("https://api.example.com/anthropic")
        );
        assert_eq!(
            super::super::load_default_model_from(&[path]).as_deref(),
            Some("d")
        );
    }

    #[test]
    fn broken_file_is_reported_and_never_overwritten() {
        let (_d, path) = tmp("[models]\ndefault = \"a\"\n\n[model.a\n");
        let err = parse_error_in(&path).unwrap();
        assert_eq!(err.line, Some(4));
        assert!(save_model_in(&path, &entry("b"), None, false, &[]).is_err());
        assert!(write_setting_in(&path, "browser.headed", &json!(true)).is_err());
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .ends_with("[model.a\n"));
    }

    #[test]
    fn settings_whitelist_round_trip() {
        let (_d, path) = tmp("");
        write_setting_in(&path, "toolset.web_fetch.timeout_secs", &json!(30)).unwrap();
        write_setting_in(
            &path,
            "toolset.web_fetch.allowed_domains",
            &json!(["docs.rs"]),
        )
        .unwrap();
        write_setting_in(&path, "memory.embedding.api_key", &json!("sk-x")).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("[toolset.web_fetch]"), "{text}");
        assert!(!text.contains("[toolset]\n"), "{text}");
        let values = read_settings_in(&path);
        assert_eq!(values["toolset.web_fetch.timeout_secs"], json!(30));
        assert_eq!(values["memory.embedding.api_key"], json!(true));
        assert_eq!(values["browser.headed"], Value::Null);
        let cfg = super::super::load_web_fetch_config_from(std::slice::from_ref(&path));
        assert_eq!(cfg.timeout_secs, Some(30));
        write_setting_in(&path, "toolset.web_fetch.timeout_secs", &Value::Null).unwrap();
        assert_eq!(
            read_settings_in(&path)["toolset.web_fetch.timeout_secs"],
            Value::Null
        );
        assert!(write_setting_in(&path, "models.default", &json!("x")).is_err());
        assert!(write_setting_in(&path, "memory.enabled", &json!("yes")).is_err());
    }

    #[test]
    fn mcp_rows_round_trip_and_switch_transport() {
        let (_d, path) = tmp("[disabled_mcp_tools]\ngh = [\"delete_repo\"]\n");
        let stdio = McpEntry {
            name: "gh".into(),
            transport: "stdio".into(),
            command: "npx".into(),
            args: vec!["-y".into(), "@modelcontextprotocol/server-github".into()],
            env: BTreeMap::from([("GITHUB_TOKEN".into(), "t".into())]),
            enabled: true,
            ..McpEntry::default()
        };
        save_mcp_in(&path, &stdio, None, &[]).unwrap();
        assert_eq!(read_mcp_in(&path, "gh").unwrap(), stdio);
        let servers = super::super::load_mcp_servers_from(std::slice::from_ref(&path));
        assert_eq!(servers.len(), 1);
        let mut http = stdio.clone();
        http.name = "github".into();
        http.transport = "http".into();
        http.url = "https://mcp.example.com/mcp".into();
        http.oauth_scopes = vec!["repo".into()];
        save_mcp_in(&path, &http, Some("gh"), &["gh".into()]).unwrap();
        let back = read_mcp_in(&path, "github").unwrap();
        assert_eq!(back.transport, "http");
        assert!(back.command.is_empty() && back.env.is_empty());
        assert_eq!(back.oauth_scopes, vec!["repo"]);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("github = [\"delete_repo\"]"), "{text}");
        delete_mcp_in(&path, "github").unwrap();
        assert!(user_mcp_names_in(&path).is_empty());
        assert!(!std::fs::read_to_string(&path)
            .unwrap()
            .contains("delete_repo"));
    }
}
