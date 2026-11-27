//! macOS 后端：把 [`Policy`] 渲染成 Seatbelt 的 SBPL，交给 `/usr/bin/sandbox-exec`。
//!
//! 结构抄 Codex（`codex-rs/sandboxing/src/seatbelt.rs`）：默认全拒的底子（`base.sbpl`，
//! 原样拷贝），放开读，按可写根放开写，按需放开网络；Grok 的做法补上最后一段：受保护
//! 路径、拒读路径、控制 socket 的拒绝规则**排在最后**——Seatbelt 以最后一条匹配为准
//! （本机实测：先 deny 后 allow 子目录，子目录可写；反过来就不可写）。
//!
//! 路径一律走 `(param "NAME")` + `-DNAME=路径`，不拼进规则文本：带引号的路径改不了规则。

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::PathBuf;

use crate::paths::within;
use crate::policy::Policy;

const BASE: &str = include_str!("base.sbpl");
const PLATFORM: &str = include_str!("platform_defaults.sbpl");
const PREFERENCES: &str = include_str!("preferences.sbpl");
const NETWORK: &str = include_str!("network.sbpl");

/// 只认 `/usr/bin` 下的那一份（Codex 同款）：PATH 里冒出来的同名程序不算数。
pub(crate) const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

pub(crate) struct Profile {
    pub(crate) text: String,
    pub(crate) params: Vec<(String, PathBuf)>,
}

#[derive(Default)]
struct Builder {
    text: String,
    params: Vec<(String, PathBuf)>,
}

impl Builder {
    fn raw(&mut self, section: &str) {
        self.text.push_str(section);
        if !section.ends_with('\n') {
            self.text.push('\n');
        }
    }

    fn param(&mut self, prefix: &str, path: &std::path::Path) -> String {
        let key = format!("{prefix}_{}", self.params.len());
        self.params.push((key.clone(), path.to_path_buf()));
        key
    }
}

pub(crate) fn render(policy: &Policy) -> Profile {
    let mut b = Builder::default();
    b.raw(BASE);
    b.raw("; Dock：先放开全部读，拒读的路径在最后一段收回。\n(allow file-read*)");
    b.raw(PREFERENCES);
    b.raw(PLATFORM);

    let roots: Vec<String> = policy
        .write_roots()
        .iter()
        .map(|root| b.param("WRITE_ROOT", root))
        .collect();
    if !roots.is_empty() {
        let filters: Vec<String> = roots
            .iter()
            .map(|k| format!("  (subpath (param \"{k}\"))"))
            .collect();
        b.raw(&format!(
            "; Dock：可写根\n(allow file-write*\n{})",
            filters.join("\n")
        ));
    }

    if policy.proxy_only() {
        // 只许连本机：出网只能经本机上的审计代理。本机的开发服务器照常连、照常监听；
        // 证书校验、读网络配置这类本机服务（NETWORK 里那些）照常放行。
        b.raw(
            "; Dock：只许连本机（出网经审计代理）\n\
             (allow network-outbound (remote ip \"localhost:*\"))\n\
             (allow network-bind (local ip \"localhost:*\"))\n\
             (allow network-inbound (local ip \"localhost:*\"))",
        );
        b.raw(NETWORK);
    } else if policy.network() {
        b.raw("; Dock：联网\n(allow network*)");
        b.raw(NETWORK);
    }

    b.raw("; Dock：以下拒绝规则必须排在最后。");
    // 可写根本身和受保护路径的祖先目录不许删、不许改名：改名能把受保护的路径整个挪出去
    // （`mv .git g && 改 g/hooks && mv g .git`），删了根目录下一条命令的策略就落空了。
    let mut anchors: BTreeSet<PathBuf> = policy.write_roots().iter().cloned().collect();
    for guarded in policy.protected().iter().chain(policy.deny()) {
        let Some(root) = policy
            .write_roots()
            .iter()
            .find(|root| within(guarded, root))
        else {
            continue;
        };
        for ancestor in guarded.ancestors().skip(1) {
            if !within(ancestor, root) || ancestor == root.as_path() {
                break;
            }
            anchors.insert(ancestor.to_path_buf());
        }
    }
    for anchor in &anchors {
        let k = b.param("ANCHOR", anchor);
        b.raw(&format!(
            "(deny file-write-unlink (require-all (vnode-type DIRECTORY) (literal (param \"{k}\"))))"
        ));
    }
    for path in policy.protected() {
        let k = b.param("PROTECTED", path);
        b.raw(&format!(
            "(deny file-write* (literal (param \"{k}\")) (subpath (param \"{k}\")))"
        ));
    }
    for path in policy.deny() {
        let k = b.param("DENY", path);
        b.raw(&format!(
            "(deny file-read* file-write* (literal (param \"{k}\")) (subpath (param \"{k}\")))"
        ));
    }
    for path in policy.sockets() {
        let k = b.param("SOCKET", path);
        b.raw(&format!(
            "(deny network-outbound (remote unix-socket (subpath (param \"{k}\"))))"
        ));
    }
    Profile {
        text: b.text,
        params: b.params,
    }
}

/// `sandbox-exec -p <profile> -DK=V … -- argv…`
pub(crate) fn wrap(policy: &Policy, argv: &[OsString]) -> (OsString, Vec<OsString>) {
    let profile = render(policy);
    let mut args: Vec<OsString> = vec!["-p".into(), profile.text.into()];
    for (key, path) in profile.params {
        let mut def = OsString::from(format!("-D{key}="));
        def.push(path.as_os_str());
        args.push(def);
    }
    args.push("--".into());
    args.extend(argv.iter().cloned());
    (SANDBOX_EXEC.into(), args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{Access, Host, Settings};

    fn policy(network: bool) -> (tempfile::TempDir, Policy) {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let ws = root.join("ws");
        std::fs::create_dir_all(ws.join(".git/hooks")).unwrap();
        let host = Host {
            home: Some(root.join("home")),
            dock_home: root.join("home/.dock"),
            tmp_dirs: vec![],
            cargo_target_dir: None,
            runtime_dir: None,
        };
        let settings = Settings {
            network,
            ..Settings::default()
        };
        let p = Policy::build(Access::WorkspaceWrite, &ws, &settings, &host);
        (dir, p)
    }

    #[test]
    fn denies_come_after_every_allow() {
        let (_d, p) = policy(true);
        let text = render(&p).text;
        let last_allow = text.rfind("(allow ").unwrap();
        let first_dock_deny = text.find("(deny file-write-unlink").unwrap();
        assert!(
            first_dock_deny > last_allow,
            "Dock 的拒绝规则必须排在所有放行之后"
        );
    }

    #[test]
    fn paths_only_appear_as_params() {
        let (_d, p) = policy(false);
        let profile = render(&p);
        for (_, path) in &profile.params {
            assert!(
                !profile.text.contains(&*path.to_string_lossy()),
                "路径不该拼进规则文本：{}",
                path.display()
            );
        }
        assert!(profile
            .params
            .iter()
            .any(|(k, v)| k.starts_with("PROTECTED") && v.ends_with(".git/hooks")));
        assert!(profile
            .params
            .iter()
            .any(|(k, v)| k.starts_with("ANCHOR") && v.ends_with(".git")));
    }

    #[test]
    fn proxy_only_allows_loopback_but_not_the_network() {
        let (_dir, mut p) = policy(true);
        let settings = Settings {
            proxy_only: true,
            ..Settings::default()
        };
        p = Policy::build(
            Access::WorkspaceWrite,
            p.workspace(),
            &settings,
            &Host::default(),
        );
        let text = render(&p).text;
        assert!(text.contains("(remote ip \"localhost:*\")"), "{text}");
        assert!(!text.contains("(allow network*)"), "{text}");
    }

    #[test]
    fn network_rules_follow_the_policy() {
        let (_d, on) = policy(true);
        let (_d2, off) = policy(false);
        assert!(render(&on).text.contains("(allow network*)"));
        assert!(!render(&off).text.contains("(allow network*)"));
    }
}
