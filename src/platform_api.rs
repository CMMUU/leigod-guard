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
    Conflict,
}
impl Error {
    pub fn message(self) -> &'static str {
        match self {
            Self::Unauthorized => "验证码或密码不正确，或登录已失效。请重新登录。",
            Self::Forbidden => "登录校验未通过，请重新登录。",
            Self::RateLimited => "请求过于频繁，请稍后重试。",
            Self::Unavailable => "平台暂时不可用，请稍后重试。本地守护仍可使用。",
            Self::Network => "未能连接上海平台，请检查网络后重试。本地守护仍可使用。",
            Self::Protocol => "平台返回了无法识别的登录信息，请稍后重试。",
            Self::InvalidInput => "请检查邮箱、验证码或账号信息后重试。",
            Self::Conflict => "设备绑定冲突或已撤销。请在原账号解除绑定，或手动重新绑定本机。",
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
            && self.expires_at <= now.saturating_add(30 * 86400 + 60)
    }
}

#[derive(Clone, Deserialize)]
pub struct CodeChallenge {
    pub request_id: String,
    pub expires_in: u32,
    pub retry_after: u32,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct DeviceBinding {
    pub device_id: String,
    pub device_token: String,
    pub owner_id: String,
    pub owner: String,
    pub sequence: i64,
}
impl DeviceBinding {
    pub fn valid(&self) -> bool {
        self.device_id.len() == 36
            && self.owner_id.len() == 36
            && self.owner.len() <= 254
            && secret_valid(&self.device_token)
            && self.sequence >= -1
            && self.sequence < i64::MAX
    }
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccountLink {
    pub key: String,
    pub label: String,
}
#[derive(Serialize)]
pub struct Heartbeat {
    pub run_generation: i64,
    pub remote_revision: Option<i64>,
    pub sequence: i64,
    pub game_running: Option<bool>,
    pub prepare_seconds: u32,
    pub version: String,
    pub account_action: String,
    pub leigod_account: Option<AccountLink>,
}
#[derive(Clone, Default, Deserialize)]
pub struct RemoteStatus {
    pub available: bool,
    pub enabled: bool,
    pub revision: i64,
    #[serde(default)]
    pub account_key: String,
    #[serde(default)]
    pub label: String,
    pub protection: String,
    pub credential: String,
    pub last_result: String,
}
impl RemoteStatus {
    pub fn message(&self) -> &'static str {
        match self.protection.as_str() {
            "off" => "远程保护已关闭",
            "awaiting_heartbeat" => "授权已保存，等待首次心跳",
            "armed" => "服务器已确认远程保护生效",
            "waiting" => "设备失联观察中，等待全部受保护设备超时",
            "executing" => "服务器正在执行暂停并查询确认",
            "confirmed" => "服务器已查询确认暂停",
            "unconfirmed" => "远程暂停未确认，请检查雷神账号状态",
            "reauthorize" => "雷神凭据失效，请重新登录雷神并开启保护",
            "service_abnormal" => "远程服务观察或异常期间，自动暂停暂缓",
            _ => "远程保护状态未知",
        }
    }
}
fn read_binding(response: Response) -> Result<DeviceBinding, Error> {
    let binding: DeviceBinding = read_json(successful(response)?)?;
    if !binding.valid() {
        return Err(Error::Protocol);
    }
    Ok(binding)
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
    pub fn login(&self, username: &str, password: &str, remember: bool) -> Result<Session, Error> {
        if !Self::valid_input(username, password) {
            return Err(Error::InvalidInput);
        }
        let response = self
            .client
            .post(format!("{}/api/login", self.origin))
            .header(ORIGIN, &self.origin)
            .json(&serde_json::json!({"username":username.trim(),"password":password,"remember":remember}))
            .send()
            .map_err(|_| Error::Network)?;
        self.finish_login(response)
    }
    fn finish_login(&self, response: Response) -> Result<Session, Error> {
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
    pub fn valid_email(email: &str) -> bool {
        let email = email.trim();
        email.len() <= 254
            && email.is_ascii()
            && !email
                .bytes()
                .any(|c| c.is_ascii_whitespace() || c.is_ascii_control())
            && email.split_once('@').is_some_and(|(local, domain)| {
                !local.is_empty() && domain.contains('.') && !domain.contains('@')
            })
    }
    pub fn send_code(&self, email: &str) -> Result<CodeChallenge, Error> {
        if !Self::valid_email(email) {
            return Err(Error::InvalidInput);
        }
        let response = self
            .client
            .post(format!("{}/api/email/code", self.origin))
            .header(ORIGIN, &self.origin)
            .json(&serde_json::json!({"email":email.trim().to_ascii_lowercase()}))
            .send()
            .map_err(|_| Error::Network)?;
        let challenge: CodeChallenge = read_json(successful(response)?)?;
        if challenge.request_id.len() != 36
            || challenge.expires_in != 600
            || challenge.retry_after != 60
        {
            return Err(Error::Protocol);
        }
        Ok(challenge)
    }
    pub fn login_code(
        &self,
        email: &str,
        request_id: &str,
        code: &str,
        remember: bool,
    ) -> Result<Session, Error> {
        if !Self::valid_email(email)
            || request_id.len() != 36
            || code.len() != 6
            || !code.bytes().all(|c| c.is_ascii_digit())
        {
            return Err(Error::InvalidInput);
        }
        let response=self.client.post(format!("{}/api/email/login",self.origin)).header(ORIGIN,&self.origin)
            .json(&serde_json::json!({"email":email.trim().to_ascii_lowercase(),"request_id":request_id,"code":code,"remember":remember}))
            .send().map_err(|_|Error::Network)?;
        self.finish_login(response)
    }
    fn authenticated(
        &self,
        path: &str,
        session: &Session,
    ) -> Result<reqwest::blocking::RequestBuilder, Error> {
        if !session.valid_for(&self.origin, chrono::Utc::now().timestamp()) {
            return Err(Error::Unauthorized);
        }
        Ok(self
            .client
            .post(format!("{}/api{path}", self.origin))
            .header(ORIGIN, &self.origin)
            .header(COOKIE, format!("{COOKIE_NAME}={}", session.token))
            .header("X-CSRF-Token", &session.user.csrf))
    }
    pub fn register_device(
        &self,
        session: &Session,
        key: &str,
        name: &str,
        reactivate: bool,
    ) -> Result<DeviceBinding, Error> {
        if !secret_valid(key) || name.is_empty() || name.len() > 100 {
            return Err(Error::InvalidInput);
        }
        let r=self.authenticated("/devices/register",session)?.json(&serde_json::json!({"installation_key":key,"name":name,"version":env!("CARGO_PKG_VERSION"),"reactivate":reactivate})).send().map_err(|_|Error::Network)?;
        let binding = read_binding(r)?;
        if binding.owner_id != session.user.id {
            return Err(Error::Protocol);
        }
        Ok(binding)
    }
    pub fn pair_device(
        &self,
        code: &str,
        key: &str,
        name: &str,
        session: Option<&Session>,
    ) -> Result<DeviceBinding, Error> {
        if code.len() != 32
            || !code.bytes().all(|c| c.is_ascii_hexdigit())
            || !secret_valid(key)
            || name.is_empty()
            || name.len() > 100
        {
            return Err(Error::InvalidInput);
        }
        let request = if let Some(s) = session {
            self.authenticated("/device/pair", s)?
        } else {
            self.client
                .post(format!("{}/api/device/pair", self.origin))
                .header(ORIGIN, &self.origin)
        };
        let response=request.json(&serde_json::json!({"code":code,"installation_key":key,"name":name,"version":env!("CARGO_PKG_VERSION")})).send().map_err(|_|Error::Network)?;
        let binding = read_binding(response)?;
        if session.is_some_and(|s| s.user.id != binding.owner_id) {
            return Err(Error::Protocol);
        }
        Ok(binding)
    }
    pub fn heartbeat(&self, binding: &DeviceBinding, payload: &Heartbeat) -> Result<(), Error> {
        if !binding.valid() || payload.sequence < 0 {
            return Err(Error::InvalidInput);
        }
        let response = self
            .client
            .post(format!("{}/api/device/heartbeat", self.origin))
            .bearer_auth(&binding.device_token)
            .json(payload)
            .send()
            .map_err(|_| Error::Network)?;
        let value: serde_json::Value = read_json(successful(response)?)?;
        if value.get("ok").and_then(|v| v.as_bool()) != Some(true)
            || value.get("remote_execution").and_then(|v| v.as_bool()) != Some(false)
        {
            return Err(Error::Protocol);
        }
        Ok(())
    }
    pub fn remote_status(&self, binding: &DeviceBinding) -> Result<RemoteStatus, Error> {
        let response = self
            .client
            .get(format!("{}/api/device/remote", self.origin))
            .bearer_auth(&binding.device_token)
            .send()
            .map_err(|_| Error::Network)?;
        read_remote(response)
    }
    pub fn remote_authorize(
        &self,
        binding: &DeviceBinding,
        token: &str,
        revision: i64,
        run: i64,
        refresh: bool,
    ) -> Result<RemoteStatus, Error> {
        if token.is_empty() || token.len() > 4096 {
            return Err(Error::InvalidInput);
        }
        let response=self.client.post(format!("{}/api/device/remote/authorize",self.origin)).timeout(std::time::Duration::from_secs(25)).bearer_auth(&binding.device_token)
            .json(&serde_json::json!({"account_token":token,"revision":revision,"run_generation":run,"refresh":refresh})).send().map_err(|_|Error::Network)?;
        read_remote(response)
    }
    pub fn remote_disable(
        &self,
        binding: &DeviceBinding,
        revision: i64,
    ) -> Result<RemoteStatus, Error> {
        let response = self
            .client
            .post(format!("{}/api/device/remote/disable", self.origin))
            .bearer_auth(&binding.device_token)
            .json(&serde_json::json!({"revision":revision}))
            .send()
            .map_err(|_| Error::Network)?;
        read_remote(response)
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
            || user.username.len() > 254
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
        400 => Err(Error::InvalidInput),
        409 => Err(Error::Conflict),
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
            .filter(|s| (1..=30 * 86400).contains(s))
            .ok_or(Error::Protocol)?;
        return Ok((token.into(), seconds));
    }
    Err(Error::Protocol)
}

fn read_remote(response: Response) -> Result<RemoteStatus, Error> {
    let status: RemoteStatus = read_json(successful(response)?)?;
    if status.revision < 0
        || status.revision == i64::MAX
        || status.account_key.len() > 64
        || status.label.len() > 100
        || status.protection.len() > 40
        || status.credential.len() > 40
        || status.last_result.len() > 100
    {
        return Err(Error::Protocol);
    }
    Ok(status)
}

#[cfg(test)]
#[path = "platform_api_tests.rs"]
mod tests;
