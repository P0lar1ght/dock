//! Named `"plugin.settings"`：插件声明的设置卡（`docs/PLUGIN-VIEWS.md` 设置卡）。
//!
//! 插件登记一份 schema（[`SettingsSchema`]），界面按它生成表单。值按插件存：
//! - 普通字段写 `$DOCK_HOME/plugin-settings.json`（`{ 插件 id: { key: 值 } }`），
//!   插件用 `host.setting(key)` 读，没写过就是 schema 的 `default`；
//! - 密钥字段写密钥库 `secrets.json`（名字就是字段 key），插件照旧 `host.secret(key)` 读，
//!   这里只回「设没设」，永远不回原值。
//!
//! 登记、卸下、改值都发 [`PLUGIN_SETTINGS_CHANGED`]（载荷插件 id）。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cordis::{plugin, Context, Disposable, Inject, Plugin};
use cordis_base::plugin_settings::{FieldKind, SettingsSchema};
use indexmap::IndexMap;
use serde_json::{json, Map, Value};

use crate::names::{PLUGIN_SETTINGS, PLUGIN_SETTINGS_CHANGED};

const FILE: &str = "plugin-settings.json";

/// 一组字段错误：`(key, 原因)`。
pub type FieldErrors = Vec<(String, String)>;

/// Named `"plugin.settings"`。调用点 live-lookup。
#[derive(Clone)]
pub struct PluginSettings {
    schemas: Arc<Mutex<IndexMap<String, SettingsSchema>>>,
    /// 普通字段落盘的文件；测试里指到临时目录。
    path: PathBuf,
    /// 写文件互斥：两页同时保存不能互相盖掉。
    write: Arc<Mutex<()>>,
    ctx: Option<Context>,
}

impl PluginSettings {
    pub fn at(path: PathBuf) -> Self {
        Self {
            schemas: Default::default(),
            path,
            write: Default::default(),
            ctx: None,
        }
    }

    fn changed(&self, plugin_id: &str) {
        if let Some(ctx) = &self.ctx {
            ctx.emit(PLUGIN_SETTINGS_CHANGED, plugin_id.to_string());
        }
    }

    /// 插件 `plugin_id` 登记它的设置卡。同一颗插件只能有一份。
    pub fn register(&self, plugin_id: &str, schema: SettingsSchema) -> cordis::Result<Disposable> {
        let id = plugin_id.to_string();
        {
            let mut schemas = self.schemas.lock().unwrap();
            if schemas.contains_key(&id) {
                return Err(cordis::Error::plugin(format!("插件 {id} 已有设置卡")));
            }
            schemas.insert(id.clone(), schema);
        }
        self.changed(&id);
        let this = self.clone();
        Ok(Disposable::from_fn(move || {
            this.schemas.lock().unwrap().shift_remove(&id);
            this.changed(&id);
        }))
    }

    /// 有设置卡的插件：`(插件 id, 标题)`。
    pub fn list(&self) -> Vec<(String, String)> {
        self.schemas
            .lock()
            .unwrap()
            .iter()
            .map(|(id, s)| (id.clone(), s.title.clone()))
            .collect()
    }

    pub fn schema(&self, plugin_id: &str) -> Option<SettingsSchema> {
        self.schemas.lock().unwrap().get(plugin_id).cloned()
    }

    fn load(&self) -> Map<String, Value> {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default()
    }

    fn stored(&self, plugin_id: &str) -> Map<String, Value> {
        self.load()
            .get(plugin_id)
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default()
    }

    /// 普通字段 `key` 的当前值：写过的，否则 schema 的 `default`；密钥字段与没有的字段回 `None`。
    pub fn get(&self, plugin_id: &str, key: &str) -> Option<Value> {
        let schema = self.schema(plugin_id)?;
        let field = schema.field(key)?;
        if field.kind == FieldKind::Secret {
            return None;
        }
        self.stored(plugin_id)
            .get(key)
            .cloned()
            .filter(|v| field.check(v).is_ok())
            .or_else(|| field.default.clone())
    }

    /// 给界面的一份：schema、普通字段的当前值、密钥字段设没设（不回原值）。
    pub fn snapshot(&self, plugin_id: &str) -> Option<Value> {
        let schema = self.schema(plugin_id)?;
        let secret_names = crate::secret_names().unwrap_or_default();
        let mut values = Map::new();
        let mut secrets = Map::new();
        for field in &schema.fields {
            if field.kind == FieldKind::Secret {
                secrets.insert(field.key.clone(), json!(secret_names.contains(&field.key)));
            } else {
                values.insert(
                    field.key.clone(),
                    self.get(plugin_id, &field.key).unwrap_or(Value::Null),
                );
            }
        }
        Some(json!({
            "pluginId": plugin_id,
            "schema": schema.to_value(),
            "values": values,
            "secrets": secrets,
        }))
    }

    /// 按 schema 校验并写一组值（只含要改的字段）。
    /// - 普通字段：`null` 清掉（回到 default）；
    /// - 密钥字段：空字符串 = 不改，`null` = 删掉，其余写进密钥库。
    ///
    /// 有任何一个字段不合法就整组不写。
    pub fn set(&self, plugin_id: &str, values: &Map<String, Value>) -> Result<(), FieldErrors> {
        let Some(schema) = self.schema(plugin_id) else {
            return Err(vec![(
                String::new(),
                format!("插件 {plugin_id} 没有设置卡"),
            )]);
        };
        // 密钥字段「留空不改」：空串不参与校验。
        let values: Map<String, Value> = values
            .iter()
            .filter(|(k, v)| {
                !(schema.field(k).is_some_and(|f| f.kind == FieldKind::Secret)
                    && v.as_str().is_some_and(str::is_empty))
            })
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let ok = schema.validate(&values)?;
        let _guard = self.write.lock().unwrap();
        let mut all = self.load();
        let mut mine = all
            .get(plugin_id)
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        for (key, value) in ok {
            let secret = schema
                .field(&key)
                .is_some_and(|f| f.kind == FieldKind::Secret);
            let result = match (secret, value) {
                (true, Value::Null) => match crate::delete_secret(&key) {
                    // 本来就没设也算清掉了。
                    Err(e) if e.starts_with("密钥不存在") => Ok(()),
                    other => other,
                },
                (true, Value::String(s)) => crate::set_secret(&key, &s),
                (false, Value::Null) => {
                    mine.remove(&key);
                    Ok(())
                }
                (false, v) => {
                    mine.insert(key.clone(), v);
                    Ok(())
                }
                (true, _) => Ok(()),
            };
            if let Err(e) = result {
                return Err(vec![(key, e)]);
            }
        }
        all.insert(plugin_id.to_string(), Value::Object(mine));
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| vec![(String::new(), e.to_string())])?;
        }
        let text = serde_json::to_string_pretty(&Value::Object(all))
            .map_err(|e| vec![(String::new(), e.to_string())])?;
        std::fs::write(&self.path, text)
            .map_err(|e| vec![(String::new(), format!("写 {FILE} 失败：{e}"))])?;
        drop(_guard);
        self.changed(plugin_id);
        Ok(())
    }
}

pub fn plugin_settings() -> Plugin {
    plugin("plugin-settings", Inject::new(), |ctx, _: &()| {
        let settings = PluginSettings {
            ctx: Some(ctx.clone()),
            ..PluginSettings::at(cordis_base::config::dock_home().join(FILE))
        };
        Ok(Some(ctx.provide(PLUGIN_SETTINGS, settings)?))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> SettingsSchema {
        SettingsSchema::parse(&json!({
            "title": "部署助手",
            "fields": [
                { "key": "region", "type": "select", "options": ["cn", "us"], "default": "cn" },
                { "key": "timeout", "type": "number", "min": 10, "max": 600 }
            ]
        }))
        .unwrap()
    }

    #[test]
    fn values_persist_per_plugin_and_fall_back_to_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        let settings = PluginSettings::at(path.clone());
        let _a = settings.register("deploy", schema()).unwrap();
        let _b = settings.register("other", schema()).unwrap();
        assert!(settings.register("deploy", schema()).is_err());
        assert_eq!(settings.get("deploy", "region"), Some(json!("cn")));

        settings
            .set(
                "deploy",
                json!({ "region": "us", "timeout": 30 })
                    .as_object()
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(settings.get("deploy", "region"), Some(json!("us")));
        assert_eq!(
            settings.get("other", "region"),
            Some(json!("cn")),
            "按插件分开存"
        );

        // 有一个字段不合法，整组都不写。
        let err = settings
            .set(
                "deploy",
                json!({ "region": "cn", "timeout": 5 }).as_object().unwrap(),
            )
            .unwrap_err();
        assert_eq!(err, vec![("timeout".to_string(), "最小 10".to_string())]);
        assert_eq!(settings.get("deploy", "region"), Some(json!("us")));

        // 换一个实例读同一个文件：值落盘了。`null` 清回 default。
        let again = PluginSettings::at(path);
        let _c = again.register("deploy", schema()).unwrap();
        assert_eq!(again.get("deploy", "timeout"), Some(json!(30.0)));
        again
            .set("deploy", json!({ "region": null }).as_object().unwrap())
            .unwrap();
        assert_eq!(again.get("deploy", "region"), Some(json!("cn")));
        let snap = again.snapshot("deploy").unwrap();
        assert_eq!(snap["values"]["timeout"], json!(30.0));
    }
}
