//! ID token（JWT）的读取与校验。
//!
//! 官方文档写的是「验签名不可省」，这里照做：拿 OIDC discovery 的
//! `jwks_uri` 里的 RSA 公钥验 RS256 签名（`ring` 验，ring 早就在依赖树里——
//! rustls 带的），再校验声明（iss / aud / exp / nonce）。
//!
//! 顺序不能反：**先验签名再看声明**——没验过签名的声明就是攻击者写的。

use base64::Engine;
use serde_json::Value;

use crate::auth::chatgpt::ISSUER;

/// ID token 的声明。
#[derive(Debug, Clone)]
pub struct Claims {
    pub issuer: String,
    pub audience: Vec<String>,
    pub subject: Option<String>,
    pub expires_at: Option<u64>,
    pub nonce: Option<String>,
    pub email: Option<String>,
    pub scope: Option<String>,
}

/// 声明校验：签发方 / 受众 / 过期 / nonce。
///
/// **`exp` 与 `nonce` 必须有**：少了就说明这不是这次授权发出来的那张 token。
/// 早先当可选处理，缺了也能过——nonce 是这次授权与那张 token 的唯一绑定。
fn validate_claims(claims: &Claims, expected_audience: &str, nonce: &str) -> Result<(), String> {
    if claims.issuer != ISSUER {
        return Err(format!(
            "ID token 的签发方不对（{}，期望 {ISSUER}）",
            claims.issuer
        ));
    }
    if !claims.audience.iter().any(|a| a == expected_audience) {
        return Err(format!(
            "ID token 的受众不对（{:?}，期望 {expected_audience}）",
            claims.audience
        ));
    }
    let Some(exp) = claims.expires_at else {
        return Err("ID token 里没有 exp".into());
    };
    let now = crate::auth::credentials::now_unix();
    if exp <= now {
        return Err("ID token 已经过期".into());
    }
    match &claims.nonce {
        Some(n) if n == nonce => {}
        Some(_) => return Err("ID token 的 nonce 对不上".into()),
        None => return Err("ID token 里没有 nonce".into()),
    }
    Ok(())
}

/// 一把公钥（RSA）。
#[derive(Debug, Clone)]
struct Jwk {
    kid: Option<String>,
    alg: Option<String>,
    n: Vec<u8>,
    e: Vec<u8>,
}

/// JWKS：授权服务器的公钥集。
#[derive(Debug, Clone, Default)]
pub struct Jwks {
    keys: Vec<Jwk>,
}

impl Jwks {
    /// 解析 JWKS 文档。只收 RSA 用的（`kty == "RSA"`）。
    pub fn parse(v: &Value) -> Result<Self, String> {
        let list = v
            .get("keys")
            .and_then(|k| k.as_array())
            .ok_or("JWKS 里没有 keys")?;
        let mut keys = Vec::new();
        for k in list {
            if k.get("kty").and_then(|t| t.as_str()) != Some("RSA") {
                continue;
            }
            let (Some(n), Some(e)) = (
                k.get("n").and_then(|n| n.as_str()),
                k.get("e").and_then(|e| e.as_str()),
            ) else {
                continue;
            };
            let (Ok(n), Ok(e)) = (
                base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(n.as_bytes()),
                base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(e.as_bytes()),
            ) else {
                continue;
            };
            keys.push(Jwk {
                kid: k.get("kid").and_then(|x| x.as_str()).map(str::to_string),
                alg: k.get("alg").and_then(|x| x.as_str()).map(str::to_string),
                // JWK 规定 n / e 是不带前导零的大端整数；有些导出工具仍给前导零，
                // 留着会让 ring 拒（它要的是正整数）。
                n: strip_leading_zeros(&n),
                e: strip_leading_zeros(&e),
            });
        }
        if keys.is_empty() {
            return Err("JWKS 里没有可用的 RSA 公钥".into());
        }
        Ok(Self { keys })
    }

    /// 验 RS256 签名。
    ///
    /// `kid` 是 token 头里写的；没有就试每一把钥匙（官方那边一般会写 kid）。
    fn verify_rs256(
        &self,
        kid: Option<&str>,
        message: &[u8],
        signature: &[u8],
    ) -> Result<(), String> {
        let mut tried = 0usize;
        for key in &self.keys {
            if key.alg.as_deref().is_some_and(|a| a != "RS256") {
                continue;
            }
            if let (Some(want), Some(have)) = (kid, key.kid.as_deref()) {
                if want != have {
                    continue;
                }
            }
            tried += 1;
            let components = ring::signature::RsaPublicKeyComponents {
                n: key.n.clone(),
                e: key.e.clone(),
            };
            if components
                .verify(
                    &ring::signature::RSA_PKCS1_2048_8192_SHA256,
                    message,
                    signature,
                )
                .is_ok()
            {
                return Ok(());
            }
        }
        Err(format!("ID token 签名验不过（试了 {tried} 把公钥）"))
    }
}

/// 解析 JWT 的载荷段。
pub fn parse_claims(token: &str) -> Result<Claims, String> {
    let v = payload_of(token)?;
    Ok(Claims {
        issuer: v["iss"].as_str().unwrap_or_default().to_string(),
        audience: v["aud"]
            .as_str()
            .map(|s| vec![s.to_string()])
            .unwrap_or_else(|| {
                v["aud"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default()
            }),
        subject: v["sub"].as_str().map(str::to_string),
        expires_at: v["exp"].as_u64(),
        nonce: v["nonce"].as_str().map(str::to_string),
        email: v["email"].as_str().map(str::to_string),
        scope: v["scope"].as_str().map(str::to_string),
    })
}

/// 去掉大端整数的前导零字节。
fn strip_leading_zeros(v: &[u8]) -> Vec<u8> {
    let first = v.iter().position(|b| *b != 0).unwrap_or(v.len());
    v[first..].to_vec()
}

/// JWT 的载荷段（解 base64url 后的 JSON）。
fn payload_of(token: &str) -> Result<Value, String> {
    let mut parts = token.split('.');
    let _header = parts.next().ok_or("ID token 不是 JWT")?;
    let payload = parts.next().ok_or("ID token 不是 JWT")?;
    let _sig = parts.next().ok_or("ID token 不是 JWT")?;
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.as_bytes())
        .map_err(|e| format!("ID token 载荷解不开：{e}"))?;
    serde_json::from_slice(&raw).map_err(|e| format!("ID token 载荷不是 JSON：{e}"))
}

/// 校验 ID token：先验签名，再看声明。
///
/// `expected_audience` 是这次授权用的 client（首登是 `dynamic_agent_client`，
/// 之后是正式的 `oaiapp_…`）。**回调带回的 client 与它不一致就拒**——那说明
/// 授权给的不是我们要的那个客户端。
pub fn verify_id_token(
    token: &str,
    expected_audience: &str,
    nonce: &str,
    jwks: &Jwks,
) -> Result<Claims, String> {
    let mut parts = token.split('.');
    let header_b64 = parts.next().ok_or("ID token 不是 JWT")?;
    let payload_b64 = parts.next().ok_or("ID token 不是 JWT")?;
    let sig_b64 = parts.next().ok_or("ID token 不是 JWT")?;
    if parts.next().is_some() {
        return Err("ID token 的段数不对".into());
    }

    let header_raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(header_b64.as_bytes())
        .map_err(|e| format!("ID token 头解不开：{e}"))?;
    let header: Value =
        serde_json::from_slice(&header_raw).map_err(|e| format!("ID token 头不是 JSON：{e}"))?;
    match header["alg"].as_str() {
        Some("RS256") => {}
        Some(other) => return Err(format!("ID token 的算法不对（{other}，只认 RS256）")),
        None => return Err("ID token 头里没有 alg".into()),
    }
    let kid = header["kid"].as_str();
    let signature = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(sig_b64.as_bytes())
        .map_err(|e| format!("ID token 签名解不开：{e}"))?;
    // 签名盖的是前两段原文。
    let signing_input = format!("{header_b64}.{payload_b64}");
    jwks.verify_rs256(kid, signing_input.as_bytes(), &signature)?;

    let claims = parse_claims(token)?;
    validate_claims(&claims, expected_audience, nonce)?;
    Ok(claims)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// openssl 现场造的一把 RSA 测试钥匙（只在测试里用，不是任何真实凭据）。
    const JWK: &str = r#"{"keys":[{"kty":"RSA","alg":"RS256","use":"sig","kid":"test-key","n":"AMcDP0M010ZGD3vZ16kuEzgYQ65YM6umybUSh0UZxJSOeb8DE2aLaVwqhN3J7pBmLRqb2hSomwTlalfIuk4ToMFORhdzGvIxSm4qCclkcbIWloX-tLdR7VrWgTKHycv6HmTTMZPJ6wRmXR09YQg6kHHrru1mVXTsBNW6s_BAfbsNFo7bXg6CEDFvc9jx9NkMXm5LrbCGgV_7b6MDwCOY8RlNmWz_x3m0kCX9C_bvMq76buOW5zKK_v56Dj-0QYw1xWeHzAKr2PuD3TYqFh2mfoghnJ4WDH9Sbf08dY1_yQcaM0WThoxEMC2p3aiiofIJQCigzrrN8Y2MCWjcpXuQYa0","e":"AQAB"}]}"#;
    /// 与上面那把钥匙配对的、openssl 签出来的 ID token。
    const TOKEN: &str = "eyJhbGciOiJSUzI1NiIsImtpZCI6InRlc3Qta2V5IiwidHlwIjoiSldUIn0.eyJpc3MiOiJodHRwczovL2F1dGgub3BlbmFpLmNvbSIsImF1ZCI6Im9haWFwcF90ZXN0Iiwic3ViIjoidXNlci0xIiwiZXhwIjo0MTAyNDQ0ODAwLCJub25jZSI6Im4xIiwiZW1haWwiOiJhQGIuYyIsInNjb3BlIjoib3BlbmlkIGNoYXRncHQudG9rZW5zLnVzZS5kaXJlY3QifQ.l5P54YHzN-XbBQF0hzz5kjsOPnjqXWGyrPEymGBrO92GaEM4hvb1GmmnwOcJY9O5JtnjD4Cb3SwgAUQaOxGRx7hTlzN6qwvn9L7URJjrpmOcVKioi5H7NZNERrRt7evbEBl1-VuKCDHxpMYWvJTjzLiKqz2u7S-2nw0IdhtKMm19nIkf4n5Z3GV6yuRVZswx35FSKGm20Dc2gNbQNfW0ba5nAQq89ag-j_hYlOle-dBeOBj69g9iZR6G2W4HFB9ad2R9o3nEfr5LSZv1i0RqbR-Bqd7mEvUketTsCLJmmdXf3YdlMaRmMLx9dMoGp6SgpEP9ExQhPV3Kx0rhOk3GsA";

    /// 线上 `auth.openai.com/.well-known/jwks.json` 里的一把真公钥（公钥不是秘密）。
    /// 用来保证解析器吃得住**真格式**：带 `kid`、带 `use`、`alg` 是 RS256。
    const REAL_JWKS: &str = r#"{"keys":[{"kty":"RSA","kid":"SltYPghZ7R5RAGg34zm4x__sAkW02weWOKzHYBskY6E","alg":"RS256","use":"sig","n":"vvTdkMd0ri_9Dy7FIe7_u0Ay2qt3dA8rQz6UzEsdU-r9nMIWzVdDYrbMlsRxIZ8Cp382ymPLaIn0uY0Jc5mO3PhiFyp9jk37Cdxx9SGsDYYzXvBwToIO6RpNzr8d2ZIXmtIxA2jiTcmYhZgi7aH6v1F_Lc2Vs1nm0SvcZIXH4Tc1DEpHq3QGjCzNvyG-srbzrQWdeBQ9SdOu61HM-K5PzzyEgH66kVL8R0nzzxRMq7-qhvcrItxp3RH7Hcw2ZCqNkoGIJFj7dzGXkRXHIagv4AeH1HUVztalSjWNeTB6e51qKhDHP5goQl-Qs2pPh69ePE_Mufy7jTtk1f_p-ElWCw","e":"AQAB"}]}"#;

    #[test]
    fn parses_the_real_jwks_shape() {
        let v: Value = serde_json::from_str(REAL_JWKS).unwrap();
        let keys = Jwks::parse(&v).unwrap();
        assert_eq!(keys.keys.len(), 1);
        assert_eq!(
            keys.keys[0].kid.as_deref(),
            Some("SltYPghZ7R5RAGg34zm4x__sAkW02weWOKzHYBskY6E")
        );
        assert_eq!(keys.keys[0].alg.as_deref(), Some("RS256"));
        // 模数 256 字节（大端、不带前导零）。
        assert_eq!(keys.keys[0].n.len(), 256);
        assert_eq!(keys.keys[0].e, vec![0x01, 0x00, 0x01]);
    }

    fn jwks() -> Jwks {
        Jwks::parse(&serde_json::from_str(JWK).unwrap()).unwrap()
    }

    #[test]
    fn a_signed_token_passes() {
        let claims = verify_id_token(TOKEN, "oaiapp_test", "n1", &jwks()).unwrap();
        assert_eq!(claims.subject.as_deref(), Some("user-1"));
        assert_eq!(claims.email.as_deref(), Some("a@b.c"));
    }

    /// 签名不对就拒——**这是验签的全部意义**。
    #[test]
    fn a_tampered_token_is_refused() {
        use base64::Engine;
        let enc = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let mut parts: Vec<String> = TOKEN.split('.').map(str::to_string).collect();
        // 改载荷里的一个字段（重新编码），签名就对不上了。
        let raw = enc.decode(parts[1].as_bytes()).unwrap();
        let mut v: Value = serde_json::from_slice(&raw).unwrap();
        v["sub"] = json!("user-2");
        parts[1] = enc.encode(serde_json::to_vec(&v).unwrap());
        let tampered = parts.join(".");
        let err = verify_id_token(&tampered, "oaiapp_test", "n1", &jwks())
            .expect_err("改过载荷的 token 必须拒");
        assert!(err.contains("验签") || err.contains("签名"), "{err}");
    }

    /// 换一把公钥（JWKS 里没有这把钥匙）就拒。
    #[test]
    fn another_key_does_not_verify() {
        let mut v: Value = serde_json::from_str(JWK).unwrap();
        v["keys"][0]["n"] = json!("ANotTheRightModulusAtAllAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
        let other = Jwks::parse(&v).unwrap();
        let err =
            verify_id_token(TOKEN, "oaiapp_test", "n1", &other).expect_err("没有这把公钥就拒");
        assert!(err.contains("验签") || err.contains("签名"), "{err}");
    }

    #[test]
    fn rejects_wrong_audience_and_nonce() {
        let err =
            verify_id_token(TOKEN, "oaiapp_other", "n1", &jwks()).expect_err("受众不对必须拒");
        assert!(err.contains("受众"), "{err}");
        let err =
            verify_id_token(TOKEN, "oaiapp_test", "n2", &jwks()).expect_err("nonce 不对必须拒");
        assert!(err.contains("nonce"), "{err}");
    }

    /// `exp` / `nonce` **缺一不可**：少了就该拒（不是「有就查，没有就算」）。
    #[test]
    fn a_token_without_exp_or_nonce_is_refused() {
        let base = Claims {
            issuer: ISSUER.to_string(),
            audience: vec!["oaiapp_test".into()],
            subject: Some("user-1".into()),
            expires_at: Some(crate::auth::credentials::now_unix() + 600),
            nonce: Some("n1".into()),
            email: None,
            scope: None,
        };

        let mut no_exp = base.clone();
        no_exp.expires_at = None;
        let err = validate_claims(&no_exp, "oaiapp_test", "n1").expect_err("没有 exp 必须拒");
        assert_eq!(err, "ID token 里没有 exp");

        let mut no_nonce = base.clone();
        no_nonce.nonce = None;
        let err = validate_claims(&no_nonce, "oaiapp_test", "n1").expect_err("没有 nonce 必须拒");
        assert_eq!(err, "ID token 里没有 nonce");

        let mut wrong_nonce = base.clone();
        wrong_nonce.nonce = Some("n2".into());
        let err = validate_claims(&wrong_nonce, "oaiapp_test", "n1").expect_err("nonce 不对必须拒");
        assert_eq!(err, "ID token 的 nonce 对不上");

        // 声明都对 = 过。
        validate_claims(&base, "oaiapp_test", "n1").expect("声明都对就该过");
    }

    /// 声明对了但签名是假的（自造 token）就拒。
    #[test]
    fn an_unsigned_token_is_refused() {
        let token = fake_token(json!({
            "iss": ISSUER,
            "aud": "oaiapp_test",
            "exp": crate::auth::credentials::now_unix() + 600,
            "nonce": "n1",
        }));
        let err = verify_id_token(&token, "oaiapp_test", "n1", &jwks()).expect_err("假签名必须拒");
        assert!(err.contains("验签") || err.contains("签名"), "{err}");
    }

    /// 自造一个 JWT（签名是假的），用来看声明校验确实在跑。
    fn fake_token(payload: Value) -> String {
        use base64::Engine;
        let enc = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        format!(
            "{}.{}.{}",
            enc.encode(br#"{"alg":"RS256","kid":"test-key"}"#),
            enc.encode(serde_json::to_vec(&payload).unwrap()),
            enc.encode(b"not-a-signature")
        )
    }
}
