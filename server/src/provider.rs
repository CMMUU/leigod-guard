//! Async provider adapter. Never log request URLs, response bodies, or credentials.
use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit, OsRng, Payload},
    Aes256Gcm, Nonce,
};
use serde_json::Value;
use std::time::Duration;

#[derive(Clone)]
pub struct Provider {
    client: reqwest::Client,
    host: String,
    cipher: Aes256Gcm,
}
#[derive(Debug, PartialEq)]
pub enum Failure {
    Credential,
    Unavailable,
    InvalidAccount,
}
pub struct Info {
    pub key: String,
    pub label: String,
    pub paused: Option<bool>,
}
impl Provider {
    pub fn from_env(origin: &str) -> Result<Option<Self>, Box<dyn std::error::Error>> {
        if std::env::var("REMOTE_EXECUTION").as_deref() != Ok("true") {
            return Ok(None);
        }
        let file = std::env::var("REMOTE_KEY_FILE")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if std::fs::metadata(&file)?.permissions().mode() & 0o077 != 0 {
                return Err("remote key must be private to the service".into());
            }
        }
        let key = std::fs::read(file)?;
        if key.len() != 32 {
            return Err("remote key must contain 32 random bytes".into());
        }
        let host = match std::env::var("LEIGOD_TEST_ORIGIN") {
            Ok(test) => {
                let parsed = reqwest::Url::parse(&test)?;
                if !origin.starts_with("http://127.0.0.1:")
                    || parsed.scheme() != "http"
                    || parsed.host_str() != Some("127.0.0.1")
                    || parsed.path() != "/"
                    || parsed.query().is_some()
                    || parsed.fragment().is_some()
                    || !parsed.username().is_empty()
                    || parsed.password().is_some()
                {
                    return Err("provider test endpoint requires loopback isolation".into());
                }
                test.trim_end_matches('/').into()
            }
            Err(_) => "https://webapi.leigod.com".into(),
        };
        Ok(Some(Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(4))
                .timeout(Duration::from_secs(10))
                .build()?,
            host,
            cipher: Aes256Gcm::new_from_slice(&key).map_err(|_| "invalid remote key")?,
        }))
    }
    pub fn seal(&self, account: &str, token: &str) -> Result<Vec<u8>, Failure> {
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let mut result = nonce.to_vec();
        result.extend(
            self.cipher
                .encrypt(
                    &nonce,
                    Payload {
                        msg: token.as_bytes(),
                        aad: account.as_bytes(),
                    },
                )
                .map_err(|_| Failure::Unavailable)?,
        );
        Ok(result)
    }
    pub fn open(&self, account: &str, bytes: &[u8]) -> Result<String, Failure> {
        if bytes.len() < 28 {
            return Err(Failure::Credential);
        }
        let plain = self
            .cipher
            .decrypt(
                Nonce::from_slice(&bytes[..12]),
                Payload {
                    msg: &bytes[12..],
                    aad: account.as_bytes(),
                },
            )
            .map_err(|_| Failure::Credential)?;
        String::from_utf8(plain).map_err(|_| Failure::Credential)
    }
    async fn call(&self, path: &str, token: &str) -> Result<Value, Failure> {
        // The provider's existing protocol requires this query parameter. No redirects,
        // URL/error logging, or arbitrary host inputs are permitted.
        let mut response = self
            .client
            .post(format!("{}{path}", self.host))
            .query(&[
                ("os_type", "4"),
                ("account_token", token),
                ("region_code", "1"),
                ("src_channel", "guanwang"),
                ("lang", "zh_CN"),
            ])
            .json(&serde_json::json!({}))
            .send()
            .await
            .map_err(|_| Failure::Unavailable)?;
        if !response.status().is_success() {
            return Err(Failure::Unavailable);
        }
        let mut data = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| Failure::Unavailable)? {
            if data.len() + chunk.len() > 65536 {
                return Err(Failure::Unavailable);
            }
            data.extend_from_slice(&chunk);
        }
        let value: Value = serde_json::from_slice(&data).map_err(|_| Failure::Unavailable)?;
        if number(&value["code"]) == Some(400006) {
            return Err(Failure::Credential);
        }
        Ok(value)
    }
    pub async fn info(&self, token: &str) -> Result<Info, Failure> {
        parse_info(&self.call("/api/user/info", token).await?)
    }
    pub async fn pause(&self, token: &str) -> Result<(), Failure> {
        let response = self.call("/api/user/pause", token).await?;
        match number(&response["code"]) {
            Some(0 | 400803) => Ok(()),
            _ => Err(Failure::Unavailable),
        }
    }
}
fn number(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_str()?.parse().ok())
}
fn parse_info(v: &Value) -> Result<Info, Failure> {
    if number(&v["code"]) != Some(0) {
        return Err(Failure::Unavailable);
    }
    // Current user-info returns nn_number (the official NN account identifier),
    // but no user_id/id; user_name can be a masked phone number. Never derive
    // ownership from display names, phones, or a client-supplied account label.
    // Keep the legacy ID namespace for provider responses without nn_number.
    let (namespace, id) = if let Some(nn) = v.pointer("/data/nn_number") {
        let id = match nn {
            Value::String(s)
                if !s.is_empty() && s.len() <= 20 && s.bytes().all(|b| b.is_ascii_digit()) =>
            {
                s.parse::<u64>().ok()
            }
            _ => nn.as_u64(),
        }
        .filter(|id| *id > 0)
        .ok_or(Failure::InvalidAccount)?;
        ("nn", id.to_string())
    } else {
        ("id", legacy_id(v).ok_or(Failure::InvalidAccount)?)
    };
    let tail: String = id
        .chars()
        .rev()
        .take(4)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    Ok(Info {
        key: crate::auth::digest(&format!("leigod:{namespace}:{id}")),
        label: format!("雷神账号 · ***{tail}"),
        paused: match number(&v["data"]["pause_status_id"]) {
            Some(1) => Some(true),
            Some(0) => Some(false),
            _ => None,
        },
    })
}
fn legacy_id(v: &Value) -> Option<String> {
    v.pointer("/data/user_id")
        .or_else(|| v.pointer("/data/id"))
        .and_then(|v| {
            v.as_str()
                .map(str::to_owned)
                .or_else(|| v.as_u64().map(|n| n.to_string()))
        })
        .filter(|id| {
            !id.is_empty()
                && id.len() <= 100
                && id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn current_provider_shape_uses_nn_number_not_masked_phone() {
        let mut v = serde_json::json!({"code":0,"data":{
            "user_name":"138****0000","master_account":1,
            "nn_number":123456789,"pause_status_id":1
        }});
        let info = parse_info(&v).unwrap();
        assert_eq!(info.key, crate::auth::digest("leigod:nn:123456789"));
        assert_eq!(info.label, "雷神账号 · ***6789");
        assert_eq!(info.paused, Some(true));
        // String/number representations and optional legacy fields must not split
        // one provider account into separate ownership or heartbeat groups.
        v["data"]["nn_number"] = Value::from("000123456789");
        v["data"]["user_id"] = Value::from("another-id");
        assert_eq!(parse_info(&v).unwrap().key, info.key);
        v["data"]["user_name"] = Value::from("different display name");
        assert_eq!(parse_info(&v).unwrap().key, info.key);
        v["data"]["nn_number"] = Value::from(987654321);
        assert_ne!(parse_info(&v).unwrap().key, info.key);
        assert_ne!(crate::auth::digest("leigod:id:123456789"), info.key);
    }
    #[test]
    fn invalid_nn_identity_cannot_fall_back_to_another_namespace() {
        for id in [
            serde_json::json!(0),
            serde_json::json!(-1),
            serde_json::json!(1.5),
            Value::Null,
            Value::Bool(true),
            Value::from(""),
            Value::from("0"),
            Value::from("123****89"),
            Value::from(" 123"),
            Value::from("+123"),
            Value::from("18446744073709551616"),
            Value::from("0".repeat(21)),
        ] {
            let v = serde_json::json!({"code":0,"data":{"nn_number":id,"user_id":"fallback"}});
            assert!(matches!(parse_info(&v), Err(Failure::InvalidAccount)));
        }
        assert!(matches!(
            parse_info(&serde_json::json!({"code":0,"data":{
                "user_name":"138****0000","mobile":"138****0000","nickname":"demo"
            }})),
            Err(Failure::InvalidAccount)
        ));
    }
    #[test]
    fn identity_requires_verified_id_and_known_pause_state() {
        let v = serde_json::json!({"code":0,"data":{"user_id":123456,"pause_status_id":"1"}});
        let i = parse_info(&v).unwrap();
        assert_eq!(i.paused, Some(true));
        assert_eq!(i.label, "雷神账号 · ***3456");
        assert_eq!(i.key.len(), 64);
        assert!(parse_info(&serde_json::json!({"code":0,"data":{"username":"user"}})).is_err());
        assert_eq!(
            parse_info(&serde_json::json!({"code":0,"data":{"id":"abc","pause_status_id":2}}))
                .unwrap()
                .paused,
            None
        );
    }
    #[test]
    fn authenticated_encryption_binds_account_and_detects_tamper() {
        let p = Provider {
            client: reqwest::Client::new(),
            host: String::new(),
            cipher: Aes256Gcm::new_from_slice(&[17; 32]).unwrap(),
        };
        let a = p.seal("account-a", "fixture-secret").unwrap();
        let b = p.seal("account-a", "fixture-secret").unwrap();
        assert_ne!(a, b);
        assert_eq!(p.open("account-a", &a).unwrap(), "fixture-secret");
        assert!(p.open("account-b", &a).is_err());
        let mut bad = a;
        bad[15] ^= 1;
        assert!(p.open("account-a", &bad).is_err());
        assert!(p.open("account-a", &[]).is_err());
    }
}
