//! Rhai script codecs + HMAC: base64 / URL / hex / sha256 / hmac_sha256.
//!
//! Pure functions — no I/O, no permission gate. Registered **only on the run
//! engine** in [`super::rhai_host::apply_rhai`] (same place as `rhai_http`),
//! not on define-time preflight, so top-level calls during `cordis_define` fail.
//!
//! Layer 1 (#99): codecs. Layer 2: `hmac_sha256`. No AES / `host.secret`.

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine as _;
use hmac::{Hmac, Mac};
use percent_encoding::{percent_decode, utf8_percent_encode, AsciiSet, CONTROLS};
use rhai::{Blob, Engine, EvalAltResult, ImmutableString};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

/// Reject absurd inputs (same spirit as http body limits).
const MAX_INPUT: usize = 1024 * 1024;

/// Encode set suitable for query *values* (spaces → `%20`, not `+`).
/// Alphanumeric plus `-._~` stay bare; everything else is percent-encoded.
const QUERY_VALUE: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}')
    .add(b'/')
    .add(b':')
    .add(b';')
    .add(b'=')
    .add(b'@')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'|')
    .add(b'$')
    .add(b'&')
    .add(b'+')
    .add(b',')
    .add(b'%');

pub(crate) const BUILTINS: &[(&str, &str, &[&str])] = &[
    (
        "to_base64",
        "Standard Base64 encode. Accepts String or Blob. Runtime only.",
        &["to_base64(input: String | Blob) -> String"],
    ),
    (
        "from_base64",
        "Standard Base64 decode → Blob. Runtime only.",
        &["from_base64(text: String) -> Blob"],
    ),
    (
        "to_base64url",
        "URL-safe Base64 encode **without** padding. Accepts String or Blob. Runtime only.",
        &["to_base64url(input: String | Blob) -> String"],
    ),
    (
        "from_base64url",
        "URL-safe Base64 decode (padding optional) → Blob. Runtime only.",
        &["from_base64url(text: String) -> Blob"],
    ),
    (
        "url_encode",
        "Percent-encode UTF-8 text for query values (spaces as %20). Runtime only.",
        &["url_encode(text: String) -> String"],
    ),
    (
        "url_decode",
        "Percent-decode → String; errors on invalid UTF-8 after decode. Runtime only.",
        &["url_decode(text: String) -> String"],
    ),
    (
        "to_hex",
        "Lowercase hex encode. Accepts String or Blob. Runtime only.",
        &["to_hex(input: String | Blob) -> String"],
    ),
    (
        "from_hex",
        "Hex decode → Blob; rejects odd length / bad digits. Runtime only.",
        &["from_hex(text: String) -> Blob"],
    ),
    (
        "sha256",
        "SHA-256 digest as lowercase hex. Accepts String or Blob. Runtime only.",
        &["sha256(input: String | Blob) -> String"],
    ),
    (
        "sha256_blob",
        "SHA-256 digest as Blob (32 bytes). Accepts String or Blob. Runtime only.",
        &["sha256_blob(input: String | Blob) -> Blob"],
    ),
    (
        "hmac_sha256",
        "HMAC-SHA256 as lowercase hex. key and message are String or Blob. Runtime only.",
        &[
            "hmac_sha256(key: String | Blob, message: String | Blob) -> String",
        ],
    ),
    (
        "hmac_sha256_blob",
        "HMAC-SHA256 as Blob (32 bytes). Runtime only.",
        &["hmac_sha256_blob(key: String | Blob, message: String | Blob) -> Blob"],
    ),
];

fn err(message: impl Into<String>) -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(
        message.into().into(),
        rhai::Position::NONE,
    ))
}

fn check_len(n: usize, what: &str) -> Result<(), Box<EvalAltResult>> {
    if n > MAX_INPUT {
        return Err(err(format!(
            "{what} exceeds {MAX_INPUT} bytes (got {n})"
        )));
    }
    Ok(())
}

fn bytes_from_str(s: &str) -> Result<&[u8], Box<EvalAltResult>> {
    check_len(s.len(), "input")?;
    Ok(s.as_bytes())
}

fn bytes_from_blob(b: &Blob) -> Result<&[u8], Box<EvalAltResult>> {
    check_len(b.len(), "input")?;
    Ok(b.as_slice())
}

fn to_hex_bytes(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xf) as usize] as char);
    }
    out
}

const HEX: &[u8; 16] = b"0123456789abcdef";

fn from_hex_bytes(text: &str) -> Result<Blob, Box<EvalAltResult>> {
    check_len(text.len(), "hex input")?;
    let t = text.trim();
    if t.len() % 2 != 0 {
        return Err(err(format!(
            "from_hex: odd length {}",
            t.len()
        )));
    }
    let mut out = Vec::with_capacity(t.len() / 2);
    let bytes = t.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let hi = hex_digit(bytes[i])?;
        let lo = hex_digit(bytes[i + 1])?;
        out.push((hi << 4) | lo);
        i += 2;
    }
    Ok(out)
}

fn hex_digit(b: u8) -> Result<u8, Box<EvalAltResult>> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(err(format!(
            "from_hex: bad digit {:?}",
            b as char
        ))),
    }
}

fn sha256_bytes(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().into()
}

fn hmac_bytes(key: &[u8], message: &[u8]) -> Result<[u8; 32], Box<EvalAltResult>> {
    check_len(key.len(), "hmac key")?;
    check_len(message.len(), "hmac message")?;
    let mut mac = HmacSha256::new_from_slice(key).map_err(|e| err(format!("hmac: {e}")))?;
    mac.update(message);
    Ok(mac.finalize().into_bytes().into())
}

pub(crate) fn register(engine: &mut Engine) {
    // --- base64 ---
    engine.register_fn("to_base64", |s: ImmutableString| -> Result<String, Box<EvalAltResult>> {
        Ok(STANDARD.encode(bytes_from_str(&s)?))
    });
    engine.register_fn("to_base64", |b: Blob| -> Result<String, Box<EvalAltResult>> {
        Ok(STANDARD.encode(bytes_from_blob(&b)?))
    });
    engine.register_fn("from_base64", |s: ImmutableString| -> Result<Blob, Box<EvalAltResult>> {
        check_len(s.len(), "base64 input")?;
        STANDARD
            .decode(s.as_bytes())
            .map_err(|e| err(format!("from_base64: {e}")))
    });

    engine.register_fn(
        "to_base64url",
        |s: ImmutableString| -> Result<String, Box<EvalAltResult>> {
            Ok(URL_SAFE_NO_PAD.encode(bytes_from_str(&s)?))
        },
    );
    engine.register_fn("to_base64url", |b: Blob| -> Result<String, Box<EvalAltResult>> {
        Ok(URL_SAFE_NO_PAD.encode(bytes_from_blob(&b)?))
    });
    engine.register_fn(
        "from_base64url",
        |s: ImmutableString| -> Result<Blob, Box<EvalAltResult>> {
            check_len(s.len(), "base64url input")?;
            // Accept both padded and unpadded URL-safe input.
            URL_SAFE_NO_PAD
                .decode(s.as_bytes())
                .or_else(|_| {
                    base64::engine::general_purpose::URL_SAFE.decode(s.as_bytes())
                })
                .map_err(|e| err(format!("from_base64url: {e}")))
        },
    );

    // --- url ---
    engine.register_fn("url_encode", |s: ImmutableString| -> Result<String, Box<EvalAltResult>> {
        check_len(s.len(), "url_encode input")?;
        Ok(utf8_percent_encode(&s, QUERY_VALUE).to_string())
    });
    engine.register_fn("url_decode", |s: ImmutableString| -> Result<String, Box<EvalAltResult>> {
        check_len(s.len(), "url_decode input")?;
        percent_decode(s.as_bytes())
            .decode_utf8()
            .map(|c| c.into_owned())
            .map_err(|e| err(format!("url_decode: invalid UTF-8: {e}")))
    });

    // --- hex ---
    engine.register_fn("to_hex", |s: ImmutableString| -> Result<String, Box<EvalAltResult>> {
        Ok(to_hex_bytes(bytes_from_str(&s)?))
    });
    engine.register_fn("to_hex", |b: Blob| -> Result<String, Box<EvalAltResult>> {
        Ok(to_hex_bytes(bytes_from_blob(&b)?))
    });
    engine.register_fn("from_hex", |s: ImmutableString| -> Result<Blob, Box<EvalAltResult>> {
        from_hex_bytes(&s)
    });

    // --- sha256 ---
    engine.register_fn("sha256", |s: ImmutableString| -> Result<String, Box<EvalAltResult>> {
        Ok(to_hex_bytes(&sha256_bytes(bytes_from_str(&s)?)))
    });
    engine.register_fn("sha256", |b: Blob| -> Result<String, Box<EvalAltResult>> {
        Ok(to_hex_bytes(&sha256_bytes(bytes_from_blob(&b)?)))
    });
    engine.register_fn("sha256_blob", |s: ImmutableString| -> Result<Blob, Box<EvalAltResult>> {
        Ok(sha256_bytes(bytes_from_str(&s)?).to_vec())
    });
    engine.register_fn("sha256_blob", |b: Blob| -> Result<Blob, Box<EvalAltResult>> {
        Ok(sha256_bytes(bytes_from_blob(&b)?).to_vec())
    });

    // --- hmac_sha256 (all String/Blob combos) ---
    engine.register_fn(
        "hmac_sha256",
        |key: ImmutableString, msg: ImmutableString| -> Result<String, Box<EvalAltResult>> {
            Ok(to_hex_bytes(&hmac_bytes(key.as_bytes(), msg.as_bytes())?))
        },
    );
    engine.register_fn(
        "hmac_sha256",
        |key: ImmutableString, msg: Blob| -> Result<String, Box<EvalAltResult>> {
            Ok(to_hex_bytes(&hmac_bytes(key.as_bytes(), msg.as_slice())?))
        },
    );
    engine.register_fn(
        "hmac_sha256",
        |key: Blob, msg: ImmutableString| -> Result<String, Box<EvalAltResult>> {
            Ok(to_hex_bytes(&hmac_bytes(key.as_slice(), msg.as_bytes())?))
        },
    );
    engine.register_fn(
        "hmac_sha256",
        |key: Blob, msg: Blob| -> Result<String, Box<EvalAltResult>> {
            Ok(to_hex_bytes(&hmac_bytes(key.as_slice(), msg.as_slice())?))
        },
    );

    engine.register_fn(
        "hmac_sha256_blob",
        |key: ImmutableString, msg: ImmutableString| -> Result<Blob, Box<EvalAltResult>> {
            Ok(hmac_bytes(key.as_bytes(), msg.as_bytes())?.to_vec())
        },
    );
    engine.register_fn(
        "hmac_sha256_blob",
        |key: ImmutableString, msg: Blob| -> Result<Blob, Box<EvalAltResult>> {
            Ok(hmac_bytes(key.as_bytes(), msg.as_slice())?.to_vec())
        },
    );
    engine.register_fn(
        "hmac_sha256_blob",
        |key: Blob, msg: ImmutableString| -> Result<Blob, Box<EvalAltResult>> {
            Ok(hmac_bytes(key.as_slice(), msg.as_bytes())?.to_vec())
        },
    );
    engine.register_fn(
        "hmac_sha256_blob",
        |key: Blob, msg: Blob| -> Result<Blob, Box<EvalAltResult>> {
            Ok(hmac_bytes(key.as_slice(), msg.as_slice())?.to_vec())
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eng() -> Engine {
        let mut e = Engine::new();
        register(&mut e);
        e
    }

    #[test]
    fn base64_roundtrip() {
        let e = eng();
        let enc: String = e.eval(r#"to_base64("user:pass")"#).unwrap();
        assert_eq!(enc, "dXNlcjpwYXNz");
        let back: Blob = e.eval(&format!(r#"from_base64("{enc}")"#)).unwrap();
        assert_eq!(back, b"user:pass");
    }

    #[test]
    fn base64url_roundtrip_no_pad() {
        let e = eng();
        // 1 byte → would need padding in standard; URL-safe no-pad has none.
        let enc: String = e.eval(r#"to_base64url("hi!")"#).unwrap();
        assert!(!enc.contains('='), "{enc}");
        assert!(!enc.contains('+') && !enc.contains('/'), "{enc}");
        let back: Blob = e.eval(&format!(r#"from_base64url("{enc}")"#)).unwrap();
        assert_eq!(back, b"hi!");
    }

    #[test]
    fn hex_roundtrip() {
        let e = eng();
        let enc: String = e.eval(r#"to_hex("Ab")"#).unwrap();
        assert_eq!(enc, "4162");
        let back: Blob = e.eval(r#"from_hex("4162")"#).unwrap();
        assert_eq!(back, b"Ab");
    }

    #[test]
    fn from_hex_rejects_odd_length() {
        let e = eng();
        let err = e.eval::<Blob>(r#"from_hex("abc")"#).unwrap_err();
        assert!(err.to_string().contains("odd"), "{err}");
    }

    #[test]
    fn from_hex_rejects_bad_digit() {
        let e = eng();
        let err = e.eval::<Blob>(r#"from_hex("zz")"#).unwrap_err();
        assert!(err.to_string().contains("bad digit"), "{err}");
    }

    #[test]
    fn url_roundtrip() {
        let e = eng();
        let enc: String = e.eval(r#"url_encode("a b/中")"#).unwrap();
        assert!(enc.contains("%20"), "{enc}");
        assert!(!enc.contains(' '), "{enc}");
        let back: String = e.eval(&format!(r#"url_decode("{enc}")"#)).unwrap();
        assert_eq!(back, "a b/中");
    }

    #[test]
    fn sha256_known_vectors() {
        let e = eng();
        let empty: String = e.eval(r#"sha256("")"#).unwrap();
        assert_eq!(
            empty,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let hi: String = e.eval(r#"sha256("hi")"#).unwrap();
        assert_eq!(
            hi,
            "8f434346648f6b96df89dda901c5176b10a6d83961dd3c1ac88b59b2dc327aa4"
        );
    }

    #[test]
    fn sha256_blob_matches_hex() {
        let e = eng();
        let hex: String = e.eval(r#"sha256("hi")"#).unwrap();
        let blob: Blob = e.eval(r#"sha256_blob("hi")"#).unwrap();
        assert_eq!(to_hex_bytes(&blob), hex);
    }

    #[test]
    fn hmac_sha256_rfc_vector() {
        let e = eng();
        // RFC 4231 / common test: key="key", msg=fox sentence
        let got: String = e
            .eval(
                r#"hmac_sha256("key", "The quick brown fox jumps over the lazy dog")"#,
            )
            .unwrap();
        assert_eq!(
            got,
            "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8"
        );
    }

    #[test]
    fn blob_input_for_to_base64() {
        let e = eng();
        let enc: String = e.eval(r#"to_base64("hi".to_blob())"#).unwrap();
        assert_eq!(enc, STANDARD.encode(b"hi"));
    }

    #[test]
    fn rejects_oversized_input() {
        let big = "x".repeat(MAX_INPUT + 1);
        assert!(check_len(big.len(), "input").is_err());
    }
}
