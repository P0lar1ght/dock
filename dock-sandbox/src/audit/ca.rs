//! Dock 自己的根证书：审计开着时，命令的 HTTPS 流量在出口代理处解密（中间人），
//! 代理按访问的主机现签叶子证书。
//!
//! - 根证书和私钥存在 `dir`（`ca.cert.pem` / `ca.key.pem`，私钥 0600），第一次用时生成，
//!   之后一直用同一张：命令信不信它全靠环境变量指过去的证书文件（[`Authority::bundle`]），
//!   不装进系统钥匙串，不影响 Dock 以外的程序。
//! - 签发者按固定的主题信息重建（同一把私钥、同样的主题），签出的叶子证书能链到存下的那张。
//! - 叶子证书按主机缓存在内存里（ECDSA P-256，有效期 30 天：客户端不收超过 398 天的叶子）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    Issuer, KeyPair, KeyUsagePurpose, SanType,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::sign::CertifiedKey;

const CERT_FILE: &str = "ca.cert.pem";
const KEY_FILE: &str = "ca.key.pem";
const BUNDLE_FILE: &str = "bundle.pem";
const CA_NAME: &str = "Dock Audit CA";
/// 叶子证书缓存上限：满了整个清掉重来（签一张几毫秒）。
const LEAF_CACHE_MAX: usize = 512;

/// 系统自带的根证书包（按顺序找第一个存在的）。命令信任的证书文件 = 它 + Dock 的根证书，
/// 免得只信 Dock、连不上真实站点。
const SYSTEM_BUNDLES: &[&str] = &[
    "/etc/ssl/cert.pem",
    "/etc/ssl/certs/ca-certificates.crt",
    "/etc/pki/tls/certs/ca-bundle.crt",
    "/etc/ssl/ca-bundle.pem",
    "/etc/pki/tls/cacert.pem",
];

pub struct Authority {
    dir: PathBuf,
    cert_pem: String,
    issuer: Issuer<'static, KeyPair>,
    leaves: Mutex<HashMap<String, Arc<CertifiedKey>>>,
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl Authority {
    /// 读 `dir` 里的根证书；没有就生成一张。
    pub fn load_or_create(dir: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
        let cert_path = dir.join(CERT_FILE);
        let key_path = dir.join(KEY_FILE);
        let (cert_pem, key) = match (
            std::fs::read_to_string(&cert_path),
            std::fs::read_to_string(&key_path),
        ) {
            (Ok(cert), Ok(key)) => (
                cert,
                KeyPair::from_pem(&key).map_err(|e| format!("Dock 根证书私钥读不了：{e}"))?,
            ),
            _ => {
                let key = KeyPair::generate().map_err(|e| format!("生成根证书私钥：{e}"))?;
                let cert = ca_params()
                    .self_signed(&key)
                    .map_err(|e| format!("生成根证书：{e}"))?;
                write_private(&key_path, key.serialize_pem().as_bytes())?;
                std::fs::write(&cert_path, cert.pem())
                    .map_err(|e| format!("write {}: {e}", cert_path.display()))?;
                (cert.pem(), key)
            }
        };
        Ok(Self {
            dir: dir.to_path_buf(),
            cert_pem,
            issuer: Issuer::new(ca_params(), key),
            leaves: Mutex::new(HashMap::new()),
            provider: Arc::new(rustls::crypto::ring::default_provider()),
        })
    }

    /// 根证书（PEM）。
    pub fn cert_pem(&self) -> &str {
        &self.cert_pem
    }

    /// 只有 Dock 根证书的文件（`NODE_EXTRA_CA_CERTS` 这种「在内置根证书之外再加」的用）。
    pub fn cert_path(&self) -> PathBuf {
        self.dir.join(CERT_FILE)
    }

    /// 系统根证书 + Dock 根证书（`SSL_CERT_FILE` 这种「整个换掉」的用）。每次启动重写，
    /// 跟上系统证书包的更新。
    pub fn bundle(&self) -> Result<PathBuf, String> {
        let mut out = String::new();
        if let Some(system) = system_bundle() {
            out.push_str(&system);
            if !out.ends_with('\n') {
                out.push('\n');
            }
        }
        out.push_str(&self.cert_pem);
        let path = self.dir.join(BUNDLE_FILE);
        std::fs::write(&path, out).map_err(|e| format!("write {}: {e}", path.display()))?;
        Ok(path)
    }

    /// `host`（域名或 IP）的叶子证书。
    pub fn leaf(&self, host: &str) -> Result<Arc<CertifiedKey>, String> {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        if let Some(hit) = self.leaves.lock().unwrap().get(&host) {
            return Ok(hit.clone());
        }
        let key = KeyPair::generate().map_err(|e| format!("生成证书私钥：{e}"))?;
        let mut params = CertificateParams::default();
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, host.clone());
        params.distinguished_name = dn;
        params.subject_alt_names = vec![match host.parse::<std::net::IpAddr>() {
            Ok(ip) => SanType::IpAddress(ip),
            Err(_) => SanType::DnsName(
                host.clone()
                    .try_into()
                    .map_err(|e| format!("主机名不能做证书：{host}：{e}"))?,
            ),
        }];
        let now = time_now();
        params.not_before = now - rcgen_days(1);
        params.not_after = now + rcgen_days(30);
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        // 严格校验（Python 3.13+ 默认开 `VERIFY_X509_STRICT`）要求叶子带颁发机构密钥标识符。
        params.use_authority_key_identifier_extension = true;
        let cert = params
            .signed_by(&key, &self.issuer)
            .map_err(|e| format!("签 {host} 的证书：{e}"))?;
        let der_key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der()));
        let signing = self
            .provider
            .key_provider
            .load_private_key(der_key)
            .map_err(|e| format!("加载证书私钥：{e}"))?;
        let certified = Arc::new(CertifiedKey::new(
            vec![CertificateDer::from(cert.der().to_vec())],
            signing,
        ));
        let mut leaves = self.leaves.lock().unwrap();
        if leaves.len() >= LEAF_CACHE_MAX {
            leaves.clear();
        }
        leaves.insert(host, certified.clone());
        Ok(certified)
    }

    pub(crate) fn provider(&self) -> Arc<rustls::crypto::CryptoProvider> {
        self.provider.clone()
    }
}

fn ca_params() -> CertificateParams {
    let mut params = CertificateParams::default();
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, CA_NAME);
    dn.push(DnType::OrganizationName, "Dock");
    params.distinguished_name = dn;
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    let now = time_now();
    params.not_before = now - rcgen_days(1);
    params.not_after = now + rcgen_days(3650);
    params
}

fn time_now() -> time::OffsetDateTime {
    time::OffsetDateTime::now_utc()
}

fn rcgen_days(n: i64) -> time::Duration {
    time::Duration::days(n)
}

fn system_bundle() -> Option<String> {
    SYSTEM_BUNDLES
        .iter()
        .find_map(|p| std::fs::read_to_string(p).ok())
        .filter(|s| s.contains("BEGIN CERTIFICATE"))
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .map_err(|e| format!("write {}: {e}", path.display()))?;
        f.write_all(bytes)
            .map_err(|e| format!("write {}: {e}", path.display()))
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, bytes).map_err(|e| format!("write {}: {e}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_authority_is_reused_and_signs_leaves() {
        let dir = tempfile::tempdir().unwrap();
        let a = Authority::load_or_create(dir.path()).unwrap();
        let again = Authority::load_or_create(dir.path()).unwrap();
        assert_eq!(a.cert_pem(), again.cert_pem(), "同一张根证书");
        let leaf = again.leaf("Example.COM.").unwrap();
        assert_eq!(leaf.cert.len(), 1);
        assert!(Arc::ptr_eq(&leaf, &again.leaf("example.com").unwrap()));
        // 颁发机构密钥标识符（OID 2.5.29.35）：严格校验要它（Python 3.13+ 没它就拒绝）。
        let der = leaf.cert[0].as_ref();
        assert!(
            der.windows(5).any(|w| w == [0x06, 0x03, 0x55, 0x1d, 0x23]),
            "叶子证书要带 AKI"
        );
        again.leaf("127.0.0.1").unwrap();
        let bundle = std::fs::read_to_string(again.bundle().unwrap()).unwrap();
        assert!(bundle.ends_with(a.cert_pem()), "Dock 根证书在最后");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join(KEY_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
