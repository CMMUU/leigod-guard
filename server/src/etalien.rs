//! Official PC protocol, limited to verified identity, time query and pause.
//! Never trust a client-supplied account ID or infer identity from a token hash.
use crate::provider::{Failure, Info};
use prost::Message;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Credential {
    pub token: String,
    pub device_id: String,
    pub paused_state: i64,
}
impl Credential {
    pub fn parse(value: &str) -> Result<Self, Failure> {
        let c: Self = serde_json::from_str(value).map_err(|_| Failure::Credential)?;
        if c.token.is_empty()
            || c.token.len() > 4096
            || c.token.chars().any(char::is_control)
            || c.device_id.len() != 32
            || !c.device_id.bytes().all(|b| b.is_ascii_hexdigit())
            || c.paused_state <= 0
        {
            return Err(Failure::Credential);
        }
        Ok(c)
    }
}
#[derive(Clone, PartialEq, Message)]
struct Profile {
    #[prost(int64, tag = "1")]
    user_id: i64,
}
#[derive(Clone, PartialEq, Message)]
struct Remaining {
    #[prost(int64, tag = "1")]
    vip_seconds: i64,
    #[prost(int64, tag = "2")]
    free_seconds: i64,
    #[prost(int64, tag = "3")]
    timestamp: i64,
    #[prost(int64, tag = "4")]
    pause_state: i64,
}
#[derive(Clone, PartialEq, Message)]
struct Pause {
    #[prost(int64, tag = "1")]
    pause_state: i64,
}

async fn request(
    client: &reqwest::Client,
    host: &str,
    method: reqwest::Method,
    path: &str,
    c: &Credential,
    body: Vec<u8>,
) -> Result<Vec<u8>, Failure> {
    let query = format!(
        "nonce={}&ts={}&ver=2023-08-28",
        uuid::Uuid::new_v4().simple(),
        chrono::Utc::now().timestamp()
    );
    let sig = Sha256::digest(format!("{method}api.et-api.com{path}?{query}"));
    let mut r = client
        .request(method, format!("{host}{path}?{query}&sig={sig:x}"))
        .header("Authorization", &c.token)
        .header("Content-Type", "application/x-protobuf")
        .header("Accept", "application/x-protobuf")
        .header("reqChannel", "1")
        .header("x-eta", format!("os=2&ver=1.0.0&dvc={}&ch=h5", c.device_id))
        .body(body)
        .send()
        .await
        .map_err(|_| Failure::Unavailable)?;
    if matches!(r.status().as_u16(), 401 | 403) {
        return Err(Failure::Credential);
    }
    if !r.status().is_success() {
        return Err(Failure::Unavailable);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = r.chunk().await.map_err(|_| Failure::Unavailable)? {
        if bytes.len() + chunk.len() > 65536 {
            return Err(Failure::Unavailable);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
pub async fn info(client: &reqwest::Client, host: &str, value: &str) -> Result<Info, Failure> {
    let c = Credential::parse(value)?;
    let bytes = request(
        client,
        host,
        reqwest::Method::GET,
        "/account/v1/my_profile",
        &c,
        vec![],
    )
    .await?;
    let profile = Profile::decode(bytes.as_slice()).map_err(|_| Failure::InvalidAccount)?;
    if profile.user_id <= 0 {
        return Err(Failure::InvalidAccount);
    }
    let bytes = request(
        client,
        host,
        reqwest::Method::POST,
        "/v2/account/remain/duration",
        &c,
        vec![],
    )
    .await?;
    let remain = Remaining::decode(bytes.as_slice()).map_err(|_| Failure::Unavailable)?;
    if remain.timestamp <= 0 || remain.vip_seconds < 0 || remain.free_seconds < 0 {
        return Err(Failure::Unavailable);
    }
    let id = profile.user_id.to_string();
    let tail: String = id
        .chars()
        .rev()
        .take(4)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    Ok(Info {
        key: crate::auth::digest(&format!("etalien:id:{id}")),
        label: format!("外星仔账号 · ***{tail}"),
        paused: if remain.pause_state == c.paused_state {
            Some(true)
        } else if remain.pause_state == 0 {
            Some(false)
        } else {
            None
        },
    })
}
pub async fn pause(client: &reqwest::Client, host: &str, value: &str) -> Result<(), Failure> {
    let c = Credential::parse(value)?;
    request(
        client,
        host,
        reqwest::Method::POST,
        "/v2/account/update/pause/state",
        &c,
        Pause {
            pause_state: c.paused_state,
        }
        .encode_to_vec(),
    )
    .await?;
    Ok(())
}
