//! 设备令牌：远程客户端（本地 GUI、浏览器 UI）连 `dock serve --remote` 用的长期凭据。
//!
//! - `dock device add <名字>` 签一枚令牌，**只显示这一次**；盘上只存它的 sha256。
//! - `dock device list` / `dock device revoke <名字或 id>`。撤销立刻生效：网关每次鉴权都
//!   重读文件，已连着的这台设备在下一次复查时被断开（见 `ws.rs`）。
//! - 文件 `$DOCK_HOME/devices.json`，unix 上 0600。先写临时文件再改名。只有 add / revoke
//!   写它（进程内一把锁）；网关只读。
//! - 「最后使用」单独存在 `devices.seen.json`，只有网关写：和名单分开，网关记使用时间
//!   不会把同时 `dock device add` 进来的设备覆盖掉。丢一次更新也无妨。
//!
//! 令牌 = `dock_` + 32 个随机字节的 base64url。比对的是摘要，逐字节等时比较。

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use cordis_base::config::dock_home;

pub const TOKEN_PREFIX: &str = "dock_";
const NAME_MAX: usize = 40;
/// 「最后使用」最多这么久写一次盘：每次重连都写没必要。
const TOUCH_EVERY_MS: u64 = 60_000;

/// 一台设备（不含令牌，令牌只在签发时出现一次）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub created_ms: u64,
    pub last_used_ms: Option<u64>,
}

#[derive(Clone, Debug)]
struct Row {
    device: Device,
    hash: String,
}

pub fn devices_path() -> PathBuf {
    dock_home().join("devices.json")
}

/// 名单的读改写一次一个（同一进程里的并发 add / revoke，包括测试）。
static WRITE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn seen_path(path: &Path) -> PathBuf {
    path.with_extension("seen.json")
}

/// 签一枚新令牌。回设备与**明文令牌**（调用方只显示一次）。
pub fn add(name: &str) -> Result<(Device, String), String> {
    add_in(&devices_path(), name)
}

pub fn add_in(path: &Path, name: &str) -> Result<(Device, String), String> {
    let name = check_name(name)?;
    let _guard = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut rows = load(path)?;
    if rows.iter().any(|r| r.device.name == name) {
        return Err(format!("已经有叫「{name}」的设备；换个名字，或先撤销它"));
    }
    let token = format!("{TOKEN_PREFIX}{}", random_b64(32)?);
    let device = Device {
        id: random_hex(4)?,
        name,
        created_ms: now_ms(),
        last_used_ms: None,
    };
    rows.push(Row {
        device: device.clone(),
        hash: digest(&token),
    });
    save(path, &rows)?;
    Ok((device, token))
}

pub fn list() -> Result<Vec<Device>, String> {
    list_in(&devices_path())
}

pub fn list_in(path: &Path) -> Result<Vec<Device>, String> {
    let seen = load_seen(path);
    Ok(load(path)?
        .into_iter()
        .map(|r| {
            let mut device = r.device;
            device.last_used_ms = seen.get(&device.id).copied();
            device
        })
        .collect())
}

/// 按名字或 id 撤销。
pub fn revoke(which: &str) -> Result<Device, String> {
    revoke_in(&devices_path(), which)
}

pub fn revoke_in(path: &Path, which: &str) -> Result<Device, String> {
    let which = which.trim();
    let _guard = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut rows = load(path)?;
    let index = rows
        .iter()
        .position(|r| r.device.name == which || r.device.id == which)
        .ok_or_else(|| format!("没有设备「{which}」（dock device list 查看）"))?;
    let removed = rows.remove(index);
    save(path, &rows)?;
    Ok(removed.device)
}

/// 令牌对得上哪台设备。顺手记「最后使用」（写失败不影响鉴权）。
pub fn verify(token: &str) -> Option<Device> {
    verify_in(&devices_path(), token)
}

pub fn verify_in(path: &Path, token: &str) -> Option<Device> {
    let token = token.trim();
    if !token.starts_with(TOKEN_PREFIX) {
        return None;
    }
    let presented = digest(token);
    let rows = load(path).ok()?;
    // 挨个比完，不在第一个命中处提前返回。
    let mut hit = None;
    for (i, row) in rows.iter().enumerate() {
        if constant_time_eq(presented.as_bytes(), row.hash.as_bytes()) {
            hit = Some(i);
        }
    }
    let mut device = rows.into_iter().nth(hit?)?.device;
    let now = now_ms();
    let mut seen = load_seen(path);
    let stale = seen
        .get(&device.id)
        .is_none_or(|t| now.saturating_sub(*t) >= TOUCH_EVERY_MS);
    if stale {
        seen.insert(device.id.clone(), now);
        let body: serde_json::Map<String, Value> = seen
            .iter()
            .map(|(id, ms)| (id.clone(), json!(ms)))
            .collect();
        let tmp = path.with_extension("seen.json.tmp");
        if write_private(&tmp, Value::Object(body).to_string().as_bytes()).is_ok() {
            let _ = std::fs::rename(&tmp, seen_path(path));
        }
    }
    device.last_used_ms = seen.get(&device.id).copied();
    Some(device)
}

fn load_seen(path: &Path) -> std::collections::BTreeMap<String, u64> {
    std::fs::read_to_string(seen_path(path))
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|v| v.as_object().cloned())
        .map(|m| {
            m.into_iter()
                .filter_map(|(id, ms)| Some((id, ms.as_u64()?)))
                .collect()
        })
        .unwrap_or_default()
}

/// 这台设备还在（没被撤销）吗。连着的连接定期拿它复查。
pub fn is_active(id: &str) -> bool {
    is_active_in(&devices_path(), id)
}

pub fn is_active_in(path: &Path, id: &str) -> bool {
    load(path)
        .map(|rows| rows.iter().any(|r| r.device.id == id))
        .unwrap_or(false)
}

fn check_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("设备名不能为空".into());
    }
    if name.chars().count() > NAME_MAX {
        return Err(format!("设备名最多 {NAME_MAX} 个字"));
    }
    if name.chars().any(|c| c.is_control()) {
        return Err("设备名不能有控制字符".into());
    }
    Ok(name.to_string())
}

fn load(path: &Path) -> Result<Vec<Row>, String> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("读 {} 失败：{e}", path.display())),
    };
    let v: Value =
        serde_json::from_str(&raw).map_err(|e| format!("{} 不是合法 JSON：{e}", path.display()))?;
    let rows = v
        .get("devices")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|d| {
            Some(Row {
                device: Device {
                    id: d.get("id")?.as_str()?.to_string(),
                    name: d.get("name")?.as_str()?.to_string(),
                    created_ms: d.get("createdMs").and_then(Value::as_u64).unwrap_or(0),
                    last_used_ms: None,
                },
                hash: d.get("sha256")?.as_str()?.to_string(),
            })
        })
        .collect();
    Ok(rows)
}

fn save(path: &Path, rows: &[Row]) -> Result<(), String> {
    let devices: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({
                "id": r.device.id,
                "name": r.device.name,
                "sha256": r.hash,
                "createdMs": r.device.created_ms,
            })
        })
        .collect();
    let body =
        serde_json::to_string_pretty(&json!({ "devices": devices })).map_err(|e| e.to_string())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("建 {} 失败：{e}", dir.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    write_private(&tmp, body.as_bytes())?;
    std::fs::rename(&tmp, path).map_err(|e| format!("写 {} 失败：{e}", path.display()))
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("写 {} 失败：{e}", path.display()))?;
    // 文件早就存在时 mode 不生效：再设一次。
    let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
    file.write_all(bytes)
        .map_err(|e| format!("写 {} 失败：{e}", path.display()))
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    std::fs::write(path, bytes).map_err(|e| format!("写 {} 失败：{e}", path.display()))
}

fn digest(token: &str) -> String {
    let mut out = String::with_capacity(64);
    for b in Sha256::digest(token.as_bytes()) {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn random_bytes(n: usize) -> Result<Vec<u8>, String> {
    let mut buf = vec![0u8; n];
    getrandom::getrandom(&mut buf).map_err(|e| format!("取随机数失败：{e}"))?;
    Ok(buf)
}

fn random_b64(n: usize) -> Result<String, String> {
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random_bytes(n)?))
}

fn random_hex(n: usize) -> Result<String, String> {
    Ok(random_bytes(n)?
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_verify_revoke_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devices.json");
        let (laptop, token) = add_in(&path, " laptop ").unwrap();
        assert_eq!(laptop.name, "laptop");
        assert!(
            token.starts_with(TOKEN_PREFIX) && token.len() > 40,
            "{token}"
        );

        // 盘上没有明文令牌。
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains(&token));
        assert!(raw.contains(&digest(&token)));

        let hit = verify_in(&path, &token).unwrap();
        assert_eq!(hit.id, laptop.id);
        assert!(
            list_in(&path).unwrap()[0].last_used_ms.is_some(),
            "记下最后使用"
        );
        assert!(verify_in(&path, "dock_wrong").is_none());
        assert!(verify_in(&path, "no-prefix").is_none());
        assert!(is_active_in(&path, &laptop.id));

        assert!(add_in(&path, "laptop").is_err(), "重名");
        let (phone, phone_token) = add_in(&path, "手机").unwrap();
        assert_eq!(list_in(&path).unwrap().len(), 2);

        revoke_in(&path, "laptop").unwrap();
        assert!(verify_in(&path, &token).is_none(), "撤销后立刻失效");
        assert!(!is_active_in(&path, &laptop.id));
        assert!(verify_in(&path, &phone_token).is_some());
        revoke_in(&path, &phone.id).unwrap();
        assert!(revoke_in(&path, "nope").is_err());
        assert!(list_in(&path).unwrap().is_empty());
    }

    /// 网关记「最后使用」不能写名单：否则它读名单与写回之间 `dock device add` 进来的
    /// 设备会被覆盖掉。这里模拟：先 verify 把「最后使用」记下，再 add，再 verify 旧的——
    /// 新设备必须还在。
    #[test]
    fn recording_use_never_rewrites_the_device_list() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devices.json");
        let (_a, token_a) = add_in(&path, "a").unwrap();
        let before = std::fs::read_to_string(&path).unwrap();
        assert!(verify_in(&path, &token_a).is_some());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            before,
            "verify 不动名单文件"
        );
        let (_b, token_b) = add_in(&path, "b").unwrap();
        assert!(verify_in(&path, &token_a).is_some());
        assert!(verify_in(&path, &token_b).is_some(), "b 没被覆盖");
        assert!(dir.path().join("devices.seen.json").exists());
    }

    #[test]
    fn names_are_checked() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devices.json");
        assert!(add_in(&path, "  ").is_err());
        assert!(add_in(&path, &"x".repeat(41)).is_err());
        assert!(add_in(&path, "a\nb").is_err());
        assert!(!path.exists(), "校验不过不写盘");
    }

    #[cfg(unix)]
    #[test]
    fn file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devices.json");
        add_in(&path, "a").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn missing_or_broken_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devices.json");
        assert!(list_in(&path).unwrap().is_empty());
        assert!(verify_in(&path, "dock_x").is_none());
        std::fs::write(&path, "{not json").unwrap();
        assert!(list_in(&path).is_err());
        assert!(add_in(&path, "a").is_err(), "坏文件不覆盖");
        assert!(verify_in(&path, "dock_x").is_none());
    }
}
