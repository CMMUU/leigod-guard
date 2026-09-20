//! Platform identity is independent of the Leigod accelerator account.
//! Fixed HTTPS origin, no redirects, and bounded responses; never log credentials.
use reqwest::blocking::{Client, Response};
use reqwest::header::{COOKIE, ORIGIN, SET_COOKIE};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::time::Duration;

pub const ORIGIN_URL: &str = "https://111.229.216.86";
const COOKIE_NAME: &str = "__Host-guard_session";
const MAX_RESPONSE: u64 = 32 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Unauthorized,
    Forbidden,
    RateLimited,
    Unavailable,
    Network,
    Protocol,
    InvalidInput,
}
impl Error {
    pub fn message(self) -> &'static str {
        match self {
            Self::Unauthorized => "账号或密码不正确，或登录已失效。请重新登录。",
            Self::Forbidden => "登录校验未通过，请重新登录。",
            Self::RateLimited => "请求过于频繁，请稍后重试。",
            Self::Unavailable => "平台暂时不可用，请稍后重试。本地守护仍可使用。",
            Self::Network => "未能连接上海平台，请检查网络后重试。本地守护仍可使用。",
            Self::Protocol => "平台返回了无法识别的登录信息，请稍后重试。",
            Self::InvalidInput => "请输入平台账号和密码；账号最多 100 字节，密码最多 128 字节。",
        }
    }
    pub fn invalidates_session(self) -> bool {
        matches!(self, Self::Unauthorized | Self::Forbidden)
    }
}

// Intentionally no Debug implementation: these objects contain session secrets.
#[derive(Clone, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub username: String,
    pub display_name: String,
    pub role: String,
    pub csrf: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Session {
    pub origin: String,
    pub token: String,
    pub user: User,
    pub expires_at: i64,
}
fn secret_valid(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|c| c.is_ascii_hexdigit())
}
impl Session {
    pub fn valid_for(&self, origin: &str, now: i64) -> bool {
        self.origin == origin
            && secret_valid(&self.token)
            && secret_valid(&self.user.csrf)
            && self.expires_at > now
            && self.expires_at <= now.saturating_add(86_460)
    }
}

pub struct Api {
    client: Client,
    origin: String,
}
impl Api {
    pub fn new() -> Result<Self, Error> {
        Self::build(ORIGIN_URL, Duration::from_secs(10))
    }
    fn build(origin: &str, timeout: Duration) -> Result<Self, Error> {
        // Only this module's loopback tests can select a different origin.
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(4))
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("LeigodGuard/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| Error::Network)?;
        Ok(Self {
            client,
            origin: origin.into(),
        })
    }
    pub fn valid_input(username: &str, password: &str) -> bool {
        let username = username.trim();
        (3..=100).contains(&username.len())
            && username
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"@._-+".contains(&c))
            && !password.is_empty()
            && password.len() <= 128
    }
    pub fn login(&self, username: &str, password: &str) -> Result<Session, Error> {
        if !Self::valid_input(username, password) {
            return Err(Error::InvalidInput);
        }
        let response = self
            .client
            .post(format!("{}/api/login", self.origin))
            .header(ORIGIN, &self.origin)
            .json(&serde_json::json!({"username":username.trim(),"password":password}))
            .send()
            .map_err(|_| Error::Network)?;
        let response = successful(response)?;
        let (token, seconds) = login_cookie(&response)?;
        let body: serde_json::Value = read_json(response)?;
        if body.get("ok").and_then(|v| v.as_bool()) != Some(true) {
            return Err(Error::Protocol);
        }
        let user = self.identity(&token)?;
        Ok(Session {
            origin: self.origin.clone(),
            token,
            user,
            expires_at: chrono::Utc::now().timestamp() + seconds,
        })
    }
    fn identity(&self, token: &str) -> Result<User, Error> {
        if !secret_valid(token) {
            return Err(Error::Protocol);
        }
        let response = self
            .client
            .get(format!("{}/api/me", self.origin))
            .header(COOKIE, format!("{COOKIE_NAME}={token}"))
            .send()
            .map_err(|_| Error::Network)?;
        let user: User = read_json(successful(response)?)?;
        if user.id.is_empty()
            || user.id.len() > 64
            || user.username.len() > 100
            || user.display_name.len() > 320
            || !secret_valid(&user.csrf)
            || !matches!(user.role.as_str(), "admin" | "user")
        {
            return Err(Error::Protocol);
        }
        Ok(user)
    }
    pub fn validate(&self, session: &Session) -> Result<Session, Error> {
        if !session.valid_for(&self.origin, chrono::Utc::now().timestamp()) {
            return Err(Error::Unauthorized);
        }
        let mut session = session.clone();
        let user = self.identity(&session.token)?;
        if user.id != session.user.id {
            return Err(Error::Protocol);
        }
        session.user = user;
        Ok(session)
    }
    pub fn logout(&self, session: &Session) -> Result<(), Error> {
        if !session.valid_for(&self.origin, chrono::Utc::now().timestamp()) {
            return Ok(());
        }
        let response = self
            .client
            .post(format!("{}/api/logout", self.origin))
            .header(ORIGIN, &self.origin)
            .header(COOKIE, format!("{COOKIE_NAME}={}", session.token))
            .header("X-CSRF-Token", &session.user.csrf)
            .json(&serde_json::json!({}))
            .send()
            .map_err(|_| Error::Network)?;
        // An already expired/revoked session is also successfully signed out.
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Ok(());
        }
        let body: serde_json::Value = read_json(successful(response)?)?;
        if body.get("ok").and_then(|v| v.as_bool()) == Some(true) {
            Ok(())
        } else {
            Err(Error::Protocol)
        }
    }
}
fn successful(response: Response) -> Result<Response, Error> {
    match response.status().as_u16() {
        200 => Ok(response),
        401 => Err(Error::Unauthorized),
        403 => Err(Error::Forbidden),
        429 => Err(Error::RateLimited),
        500..=599 => Err(Error::Unavailable),
        _ => Err(Error::Protocol),
    }
}
fn read_json<T: serde::de::DeserializeOwned>(response: Response) -> Result<T, Error> {
    let mut bytes = Vec::new();
    response
        .take(MAX_RESPONSE + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Network)?;
    if bytes.len() as u64 > MAX_RESPONSE {
        return Err(Error::Protocol);
    }
    serde_json::from_slice(&bytes).map_err(|_| Error::Protocol)
}
fn login_cookie(response: &Response) -> Result<(String, i64), Error> {
    for header in response.headers().get_all(SET_COOKIE) {
        let text = header.to_str().map_err(|_| Error::Protocol)?;
        let mut parts = text.split(';').map(str::trim);
        let Some((name, token)) = parts.next().and_then(|p| p.split_once('=')) else {
            continue;
        };
        if name != COOKIE_NAME {
            continue;
        }
        if !secret_valid(token) {
            return Err(Error::Protocol);
        }
        let attrs: Vec<_> = parts.collect();
        if !attrs.iter().any(|s| s.eq_ignore_ascii_case("Secure"))
            || !attrs.iter().any(|s| s.eq_ignore_ascii_case("HttpOnly"))
            || !attrs.iter().any(|s| s.eq_ignore_ascii_case("Path=/"))
            || attrs
                .iter()
                .any(|s| s.to_ascii_lowercase().starts_with("domain="))
        {
            return Err(Error::Protocol);
        }
        let seconds = attrs
            .iter()
            .filter_map(|s| s.split_once('='))
            .find(|(k, _)| k.eq_ignore_ascii_case("Max-Age"))
            .and_then(|(_, v)| v.parse::<i64>().ok())
            .filter(|s| (1..=86400).contains(s))
            .ok_or(Error::Protocol)?;
        return Ok((token.into(), seconds));
    }
    Err(Error::Protocol)
}

#[cfg(test)]
#[path = "platform_api_tests.rs"]
mod tests;
