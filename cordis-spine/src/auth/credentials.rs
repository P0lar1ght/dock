//! ChatGPT 订阅凭据的落盘。
//!
//! 布局（设计稿定的那套，**凭据只在用户自己控制的本机**）：
//!
//! ```text
//! $DOCK_HOME/host_id                                urn:uuid:…，本机一份，永不随凭据复制
//! $DOCK_HOME/credentials/chatgpt/<client_id>.json   0600，目录 0700
//! $DOCK_HOME/credentials/chatgpt/<client_id>.lock   刷新 / 登出的跨进程锁
//! $DOCK_HOME/credentials/chatgpt/active             当前活动账号的 client_id
//! ```
//!
//! 写入一律「同目录临时文件（创建即 0600）→ fsync → rename」：断电 / 崩在中间
//! 也不会留下半个 JSON。读的时候权限不是 0600 就**报错**，不静默改——权限被改过
//! 说明有人动过这个目录。
//!
//! **登出只清 token，不删文件**：官方要求登出后保留「账号 ↔ 注册 client」的
//! 对应与本机 host id（"Revoking this session does not delete the registered
//! client"），下次登录直接复用这个注册。token 清空的那份就是「登出了」的账号。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use cordis_base::config::dock_home;

use crate::auth::oauth::write_private;

pub const SCHEMA_VERSION: u32 = 1;

/// 一份 ChatGPT 订阅凭据。字段按官方示例来，另加 `schema_version` 与
/// `generation`（刷新代数：401 时靠它判断「是 token 旧了」还是「账号被撤了」）。
#[derive(Clone, Serialize, Deserialize)]
pub struct Credential {
    pub schema_version: u32,
    pub generation: u64,
    #[serde(default)]
    pub email: Option<String>,
    pub issuer: String,
    pub subject: String,
    pub client_id: String,
    pub id_token: String,
    pub access_token: String,
    pub refresh_token: String,
    /// access token 的过期时刻（unix 秒）。
    pub expires_at: u64,
    #[serde(default)]
    pub scopes: Vec<String>,
    pub saved_at: u64,
    /// 本机的 host id（官方凭据记录里也有这一项）：跟运行时走，不随凭据复制。
    #[serde(default)]
    pub ext_agent_host_id: String,
}

/// 手写 `Debug`：**三种 token 一个都不进日志**。
///
/// `derive(Debug)` 会把 access / refresh / id token 原样打出来——凭据一旦被
/// `tracing` 记一笔就等于泄进日志文件。脱敏版才挡得住。
impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credential")
            .field("schema_version", &self.schema_version)
            .field("generation", &self.generation)
            .field("email", &self.email)
            .field("issuer", &self.issuer)
            .field("subject", &self.subject)
            .field("client_id", &self.client_id)
            .field("id_token", &"<redacted>")
            .field("access_token", &"<redacted>")
            .field("refresh_token", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .field("scopes", &self.scopes)
            .field("saved_at", &self.saved_at)
            .field("ext_agent_host_id", &self.ext_agent_host_id)
            .finish()
    }
}

impl Credential {
    /// scope 里有没有订阅额度那一条。
    pub fn has_plan_usage(&self) -> bool {
        self.scopes
            .iter()
            .any(|s| s == crate::auth::chatgpt::PLAN_SCOPE)
    }

    /// 还登着：有能续期的 refresh token。登出 / 刷新被判死之后是 `false`，
    /// 但这份记录（client id、邮箱、subject）还留着，下次登录复用它。
    pub fn signed_in(&self) -> bool {
        !self.refresh_token.is_empty()
    }

    /// 清掉 token，留下注册（client id / 账号身份 / 本机 host id / 授权过的 scope）。
    pub fn without_tokens(&self) -> Credential {
        Credential {
            id_token: String::new(),
            access_token: String::new(),
            refresh_token: String::new(),
            expires_at: 0,
            saved_at: now_unix(),
            ..self.clone()
        }
    }

    /// access token 还能用多久（秒）。
    pub fn expires_in(&self) -> i64 {
        self.expires_at as i64 - now_unix() as i64
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CredentialError {
    #[error("凭据文件不存在")]
    Missing,
    #[error("凭据文件权限不对（应为 0600）：{0}")]
    Permissions(String),
    #[error("凭据格式不对：{0}")]
    Format(String),
    #[error("IO 错误：{0}")]
    Io(#[from] std::io::Error),
}

/// 凭据目录。`root` 可以指到别处（测试用）。
#[derive(Clone, Debug)]
pub struct CredentialStore {
    root: PathBuf,
}

impl CredentialStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// `$DOCK_HOME/credentials/chatgpt`。
    pub fn default_store() -> Self {
        Self::new(dock_home().join("credentials").join("chatgpt"))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn path_for(&self, client_id: &str) -> PathBuf {
        self.root.join(format!("{}.json", sanitize(client_id)))
    }

    pub fn lock_path_for(&self, client_id: &str) -> PathBuf {
        self.root.join(format!("{}.lock", sanitize(client_id)))
    }

    pub fn active_path(&self) -> PathBuf {
        self.root.join("active")
    }

    /// 读一份凭据。文件不在 = [`CredentialError::Missing`]。
    pub fn load(&self, client_id: &str) -> Result<Credential, CredentialError> {
        let path = self.path_for(client_id);
        if !path.exists() {
            return Err(CredentialError::Missing);
        }
        check_owner_only(&path)?;
        let raw = std::fs::read_to_string(&path)?;
        let cred: Credential =
            serde_json::from_str(&raw).map_err(|e| CredentialError::Format(e.to_string()))?;
        if cred.schema_version != SCHEMA_VERSION {
            return Err(CredentialError::Format(format!(
                "凭据版本不对（{}，期望 {SCHEMA_VERSION}）",
                cred.schema_version
            )));
        }
        Ok(cred)
    }

    /// 写一份凭据。整份替换：access / refresh / 过期时间一起换，不留半新半旧。
    pub fn save(&self, cred: &Credential) -> Result<(), CredentialError> {
        self.ensure_root()?;
        let json = serde_json::to_string_pretty(cred)
            .map_err(|e| CredentialError::Format(e.to_string()))?;
        write_private(&self.path_for(&cred.client_id), json.as_bytes())?;
        Ok(())
    }

    /// 登出：清掉这个账号的 token，**留下注册**。文件不在不算错。
    pub fn clear_tokens(&self, client_id: &str) -> Result<(), CredentialError> {
        match self.load(client_id) {
            Ok(cred) => self.save(&cred.without_tokens()),
            Err(CredentialError::Missing) => Ok(()),
            Err(e) => Err(e),
        }
    }

    fn ensure_root(&self) -> Result<(), CredentialError> {
        std::fs::create_dir_all(&self.root)?;
        set_dir_owner_only(&self.root)
    }

    /// 列出全部账号（按 client_id 排序）。
    pub fn list(&self) -> Result<Vec<Credential>, CredentialError> {
        if !self.root.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&self.root)? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            // 解析不出就跳过：一份坏文件不该让 `/auth list` 整个失败。
            if let Ok(cred) = self.load(stem) {
                out.push(cred);
            }
        }
        out.sort_by(|a, b| a.client_id.cmp(&b.client_id));
        Ok(out)
    }

    /// 已注册但**还没拿到 token** 的正式 client id 记在哪。
    ///
    /// 动态注册是**一次性**的：第一次回调给出 `oaiapp_…` 之后，再拿
    /// `dynamic_agent_client` 去注册会被拒（`invalid_client`）。官方要求
    /// 「Save it with the registration and reuse it for later sign-ins」——
    /// 所以正式 client id 一拿到就得落盘，哪怕换码还没成功。
    pub fn issued_path(&self) -> PathBuf {
        self.root.join("issued")
    }

    /// 记住已注册的正式 client id。
    pub fn remember_issued(
        &self,
        client_id: &str,
        email: Option<&str>,
    ) -> Result<(), CredentialError> {
        self.ensure_root()?;
        let record = serde_json::json!({ "client_id": client_id, "email": email });
        write_private(&self.issued_path(), record.to_string().as_bytes())?;
        Ok(())
    }

    /// 已注册的正式 client id（还没拿到 token 时用）。
    pub fn issued(&self) -> Option<String> {
        let raw = std::fs::read_to_string(self.issued_path()).ok()?;
        let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
        v.get("client_id")
            .and_then(|c| c.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    }

    /// 记下的邮箱（重新授权时当 `login_hint`）。
    pub fn issued_email(&self) -> Option<String> {
        let raw = std::fs::read_to_string(self.issued_path()).ok()?;
        let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
        v.get("email")
            .and_then(|c| c.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    }

    /// 凭据落盘后就不需要「已注册但未拿到 token」那条记录了。
    pub fn clear_issued(&self) {
        let _ = std::fs::remove_file(self.issued_path());
    }

    /// 当前活动账号的 client_id。
    pub fn active(&self) -> Option<String> {
        let raw = std::fs::read_to_string(self.active_path()).ok()?;
        let id = raw.trim().to_string();
        if id.is_empty() {
            None
        } else {
            Some(id)
        }
    }

    /// 切活动账号。
    pub fn set_active(&self, client_id: &str) -> Result<(), CredentialError> {
        self.ensure_root()?;
        write_private(&self.active_path(), client_id.as_bytes())?;
        Ok(())
    }
}

/// 本机的 host id：`urn:uuid:<v4>`。官方用它把「哪台机器在用这份授权」记下来。
///
/// **不随凭据复制**：凭据传到 VPS 后，VPS 用自己的 host_id 接管刷新。
pub fn host_id_path() -> PathBuf {
    dock_home().join("host_id")
}

/// 读本机 host id；没有就生成一个（0700 / 0600）。
pub fn load_or_create_host_id() -> Result<String, CredentialError> {
    let path = host_id_path();
    if let Ok(raw) = std::fs::read_to_string(&path) {
        let id = raw.trim().to_string();
        if !id.is_empty() {
            return Ok(id);
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let id = format!("urn:uuid:{}", uuid::Uuid::new_v4());
    write_private(&path, id.as_bytes())?;
    Ok(id)
}

pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// client_id 进文件名的部分。官方那串形如 `oaiapp_…`，没有斜杠；这里仍然滤一遍，
/// 免得哪天来了个带路径分隔符的值把文件写到别处。
fn sanitize(client_id: &str) -> String {
    let cleaned: String = client_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "unknown".to_string()
    } else {
        cleaned
    }
}

/// 权限不是 0600 就报错——**不静默修**。文件被动过说明这个目录被人动过。
fn check_owner_only(path: &Path) -> Result<(), CredentialError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::metadata(path)?;
        let mode = meta.permissions().mode() & 0o777;
        if mode != 0o600 {
            return Err(CredentialError::Permissions(format!(
                "{}（当前 {:o}）",
                path.display(),
                mode
            )));
        }
    }
    Ok(())
}

fn set_dir_owner_only(path: &Path) -> Result<(), CredentialError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::metadata(path)?;
        let mut perms = meta.permissions();
        if perms.mode() & 0o777 != 0o700 {
            perms.set_mode(0o700);
            std::fs::set_permissions(path, perms)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, CredentialStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = CredentialStore::new(dir.path().join("credentials").join("chatgpt"));
        (dir, store)
    }

    fn cred(client_id: &str) -> Credential {
        Credential {
            schema_version: SCHEMA_VERSION,
            generation: 1,
            email: Some("a@b.c".into()),
            issuer: "https://auth.openai.com".into(),
            subject: "user-1".into(),
            client_id: client_id.into(),
            id_token: "id".into(),
            access_token: "access".into(),
            refresh_token: "refresh".into(),
            expires_at: now_unix() + 3600,
            scopes: vec!["openid".into(), crate::auth::chatgpt::PLAN_SCOPE.into()],
            saved_at: now_unix(),
            ext_agent_host_id: "urn:uuid:test".into(),
        }
    }

    #[test]
    fn save_load_roundtrip_keeps_private_permissions() {
        let (_dir, store) = store();
        store.save(&cred("oaiapp_a")).unwrap();
        let loaded = store.load("oaiapp_a").unwrap();
        assert_eq!(loaded.refresh_token, "refresh");
        assert!(loaded.has_plan_usage());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(store.path_for("oaiapp_a"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "凭据文件必须是 0600");
            let dir_mode = std::fs::metadata(store.root())
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(dir_mode, 0o700, "凭据目录必须是 0700");
        }
    }

    /// 权限被人改过就报错，不静默修。
    #[test]
    fn loose_permissions_are_refused() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let (_dir, store) = store();
            store.save(&cred("oaiapp_b")).unwrap();
            let path = store.path_for("oaiapp_b");
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o644);
            std::fs::set_permissions(&path, perms).unwrap();
            assert!(matches!(
                store.load("oaiapp_b"),
                Err(CredentialError::Permissions(_))
            ));
        }
    }

    #[test]
    fn active_account_roundtrip() {
        let (_dir, store) = store();
        assert!(store.active().is_none());
        store.set_active("oaiapp_c").unwrap();
        assert_eq!(store.active().as_deref(), Some("oaiapp_c"));
    }

    /// 登出 = 清 token、留注册：client id / 邮箱 / subject / 授权过的 scope 都在。
    #[test]
    fn clearing_tokens_keeps_the_registration() {
        let (_dir, store) = store();
        store.save(&cred("oaiapp_out")).unwrap();
        store.clear_tokens("oaiapp_out").unwrap();
        let kept = store.load("oaiapp_out").expect("文件还在");
        assert!(!kept.signed_in());
        assert!(kept.access_token.is_empty());
        assert!(kept.refresh_token.is_empty());
        assert!(kept.id_token.is_empty());
        assert_eq!(kept.expires_at, 0);
        assert_eq!(kept.client_id, "oaiapp_out");
        assert_eq!(kept.email.as_deref(), Some("a@b.c"));
        assert_eq!(kept.subject, "user-1");
        assert!(
            kept.has_plan_usage(),
            "授权过的 scope 留着（重新授权时判断要不要同意页）"
        );
        // 没有这个账号不算错。
        store.clear_tokens("oaiapp_none").unwrap();
    }

    #[test]
    fn list_skips_broken_files() {
        let (_dir, store) = store();
        store.save(&cred("oaiapp_d")).unwrap();
        std::fs::write(store.path_for("broken"), "{ not json").unwrap();
        let list = store.list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].client_id, "oaiapp_d");
    }
}
