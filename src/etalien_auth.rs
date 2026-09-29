//! Official website SMS authentication, independent of platform email login.
//! Evidence and wire format: docs/外星仔接入说明.md. Never log response bodies.
use reqwest::{blocking::Client, Method};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, io::Read, time::Duration};

const HOST: &str = "https://api.et-api.com";
const SEND: &str = "/account/v1/get_login_verification_code";
const LOGIN: &str = "/account/v1/login";
const LIMIT: u64 = 65536;
pub const DEFAULT_COOLDOWN: Duration = Duration::from_secs(60);

#[derive(Debug)]
pub struct Error {
    message: &'static str,
    pub retry_after: Option<Duration>,
    http_status: Option<u16>,
}

impl Error {
    fn new(message: &'static str) -> Self {
        Self {
            message,
            retry_after: None,
            http_status: None,
        }
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message)?;
        if let Some(status) = self.http_status {
            write!(f, "（HTTP {status}）")?;
        }
        Ok(())
    }
}

pub fn phone(input: &str) -> Result<String, Error> {
    let input = input.trim();
    let digits = input.strip_prefix("+86").unwrap_or(input);
    if digits.len() != 11
        || !digits.bytes().all(|b| b.is_ascii_digit())
        || !digits.starts_with('1')
        || !(b'3'..=b'9').contains(&digits.as_bytes()[1])
    {
        return Err(Error::new("请输入正确的中国大陆手机号（11 位，可带 +86）"));
    }
    Ok(format!("+86{digits}"))
}

pub fn valid_code(code: &str) -> bool {
    code.len() == 6 && code.bytes().all(|b| b.is_ascii_digit())
}

fn signed_url(
    method: &Method,
    path: &str,
    mut params: BTreeMap<&str, String>,
    nonce: String,
    ts: i64,
) -> String {
    params.insert("nonce", nonce);
    params.insert("ts", ts.to_string());
    params.insert("ver", "2023-08-28".into());
    // The official signer hashes sorted, unescaped values; URL encoding happens after signing.
    let query = params
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&");
    let sign = Sha256::digest(format!("{method}api.et-api.com{path}?{query}").as_bytes());
    params.insert("sig", format!("{sign:x}"));
    let mut url = reqwest::Url::parse(&format!("{HOST}{path}")).expect("fixed official origin");
    url.query_pairs_mut().extend_pairs(params);
    url.into()
}

fn request(
    method: Method,
    path: &str,
    device: &str,
    params: BTreeMap<&str, String>,
    body: Option<Value>,
) -> Result<Value, Error> {
    if device.len() != 32 || !device.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::new("外星仔设备标识无效，请重试"));
    }
    let client = Client::builder()
        .timeout(Duration::from_secs(12))
        .connect_timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .https_only(true)
        .build()
        .map_err(|_| Error::new("无法建立外星仔连接"))?;
    let nonce = crate::etalien_api::device_id().map_err(|_| Error::new("无法生成请求标识"))?;
    execute(build_request(
        &client, method, path, device, params, body, nonce,
    ))
}

fn build_request(
    client: &Client,
    method: Method,
    path: &str,
    device: &str,
    params: BTreeMap<&str, String>,
    body: Option<Value>,
    nonce: String,
) -> reqwest::blocking::RequestBuilder {
    let mut req = client
        .request(
            method.clone(),
            signed_url(&method, path, params, nonce, chrono::Utc::now().timestamp()),
        )
        .header("reqChannel", "1")
        .header("x-eta", format!("os=2&ver=1.0.0&dvc={device}&ch=h5"))
        .header("Accept", "application/json");
    // The official Axios adapter removes Content-Type when there is no body.
    // Declaring JSON on the SMS GET makes the API parse an empty body and reject
    // the request before reading phone_number from the query string.
    if let Some(body) = body {
        req = req.json(&body);
    }
    req
}

fn execute(req: reqwest::blocking::RequestBuilder) -> Result<Value, Error> {
    let response = req
        .send()
        .map_err(|_| Error::new("外星仔连接失败或超时，请检查网络后重试"))?;
    if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        let seconds = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(60)
            .clamp(60, 86400);
        return Err(Error {
            message: "请求过于频繁，请等待倒计时结束后重试",
            retry_after: Some(Duration::from_secs(seconds)),
            http_status: Some(429),
        });
    }
    if !response.status().is_success() {
        let mut error = Error::new(match response.status().as_u16() {
            400 => "外星仔未接受请求，请核对输入或更新应用后重试",
            401 | 422 => "外星仔验证未通过，请核对输入后重试",
            403 => "外星仔拒绝了本次登录请求，请稍后重试或使用其他登录方式",
            _ => "外星仔服务暂不可用，请稍后重试",
        });
        error.http_status = Some(response.status().as_u16());
        return Err(error);
    }
    if response.content_length().is_some_and(|len| len > LIMIT) {
        return Err(Error::new("外星仔登录响应过大，请稍后重试"));
    }
    let mut bytes = Vec::new();
    response
        .take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::new("无法读取外星仔登录响应"))?;
    if bytes.len() as u64 > LIMIT {
        return Err(Error::new("外星仔登录响应过大，请稍后重试"));
    }
    parse(&bytes)
}

fn parse(bytes: &[u8]) -> Result<Value, Error> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|_| Error::new("外星仔登录响应格式发生变化，请更新应用后重试"))?;
    // Current official endpoint returns a flat JSON object. Do not accept an error
    // envelope or infer success from an arbitrary 200 response.
    if !value.is_object()
        || value.get("error").is_some()
        || value.get("success").is_some_and(|v| v != true)
        || ["code", "retCode", "error_code"]
            .iter()
            .any(|key| value.get(key).is_some_and(|v| v.as_i64() != Some(0)))
    {
        return Err(Error::new(
            "外星仔未接受本次请求，请核对手机号和验证码后重试",
        ));
    }
    Ok(value)
}

fn cooldown(value: &Value) -> Result<Duration, Error> {
    let seconds = value
        .get("cool_down")
        .and_then(Value::as_u64)
        .ok_or_else(|| Error::new("外星仔未确认验证码发送，请稍后重试"))?;
    if seconds > 86400 {
        return Err(Error::new("外星仔验证码倒计时异常，请稍后重试"));
    }
    Ok(Duration::from_secs(seconds.max(60)))
}

fn token(value: &Value) -> Result<String, Error> {
    let id = value
        .get("user_id")
        .and_then(|v| v.as_i64().or_else(|| v.as_str()?.parse().ok()));
    let token = value
        .get("authorization")
        .and_then(Value::as_str)
        .unwrap_or("");
    if id.is_none_or(|id| id <= 0)
        || token.is_empty()
        || token.len() > 8192
        || !token.bytes().all(|b| (33..=126).contains(&b))
    {
        return Err(Error::new("外星仔登录未返回有效凭据，请重试"));
    }
    Ok(token.into())
}

pub fn send_code(input: &str, device: &str) -> Result<Duration, Error> {
    let phone = phone(input)?;
    let value = request(
        Method::GET,
        SEND,
        device,
        BTreeMap::from([("phone_number", phone)]),
        None,
    )?;
    cooldown(&value)
}

pub fn login(input: &str, code: &str, device: &str) -> Result<String, Error> {
    let phone = phone(input)?;
    if !valid_code(code) {
        return Err(Error::new("请输入短信中的 6 位数字验证码"));
    }
    let value = request(
        Method::POST,
        LOGIN,
        device,
        BTreeMap::new(),
        Some(json!({"phone_number": phone, "verification_code": code})),
    )?;
    token(&value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_validation_rejects_unicode_and_preserves_leading_zero_code() {
        for input in [
            "",
            "138",
            "+8612345678901",
            "１３８１２３４５６７８",
            "13812345678&extra=1",
            "+1(555)0101234",
        ] {
            assert!(phone(input).is_err());
        }
        assert_eq!(phone(" 13812345678 ").unwrap(), "+8613812345678");
        assert_eq!(phone("+8613812345678").unwrap(), "+8613812345678");
        assert!(valid_code("012345"));
        for code in ["", "12345", "１２３４５６", "12345a"] {
            assert!(!valid_code(code));
        }
        // These fail before constructing a client or contacting any official service.
        assert!(send_code("invalid", "invalid").is_err());
        assert!(login("13812345678", "bad", "invalid").is_err());
    }

    #[test]
    fn signature_sorts_raw_fields_and_transports_phone_plus_encoded() {
        let url = signed_url(
            &Method::GET,
            SEND,
            BTreeMap::from([("phone_number", "+8613812345678".into())]),
            "ABC".into(),
            123,
        );
        let parsed = reqwest::Url::parse(&url).unwrap();
        let params: BTreeMap<_, _> = parsed.query_pairs().collect();
        assert_eq!(parsed.host_str(), Some("api.et-api.com"));
        assert!(url.contains("phone_number=%2B8613812345678"));
        let expected = Sha256::digest(b"GETapi.et-api.com/account/v1/get_login_verification_code?nonce=ABC&phone_number=+8613812345678&ts=123&ver=2023-08-28");
        assert_eq!(params["sig"], format!("{expected:x}"));
        let login_url = signed_url(&Method::POST, LOGIN, BTreeMap::new(), "ABC".into(), 123);
        assert!(!login_url.contains("verification_code"));
        assert!(!login_url.contains("phone_number"));
    }

    #[test]
    fn sms_get_has_no_body_type_and_login_post_still_sends_json() {
        let client = Client::builder().no_proxy().build().unwrap();
        let device = "F".repeat(32);
        let sms = build_request(
            &client,
            Method::GET,
            SEND,
            &device,
            BTreeMap::from([("phone_number", "+860".into())]),
            None,
            "ABC".into(),
        )
        .build()
        .unwrap();
        assert!(sms.body().is_none());
        assert!(!sms.headers().contains_key(reqwest::header::CONTENT_TYPE));
        assert_eq!(sms.method(), Method::GET);
        assert_eq!(sms.headers()[reqwest::header::ACCEPT], "application/json");
        assert_eq!(
            sms.url()
                .query_pairs()
                .find(|(key, _)| key == "phone_number")
                .unwrap()
                .1,
            "+860"
        );

        let login = build_request(
            &client,
            Method::POST,
            LOGIN,
            &device,
            BTreeMap::new(),
            Some(json!({"phone_number":"+860", "verification_code":"012345"})),
            "ABC".into(),
        )
        .build()
        .unwrap();
        assert_eq!(
            login.headers()[reqwest::header::CONTENT_TYPE],
            "application/json"
        );
        assert_eq!(login.method(), Method::POST);
        let body: Value =
            serde_json::from_slice(login.body().unwrap().as_bytes().unwrap()).unwrap();
        assert_eq!(body["verification_code"], "012345");
        assert!(!login.url().as_str().contains("012345"));
    }

    #[test]
    fn official_flat_payloads_must_confirm_send_and_login() {
        assert_eq!(
            cooldown(&parse(br#"{"cool_down":120}"#).unwrap()).unwrap(),
            Duration::from_secs(120)
        );
        assert_eq!(cooldown(&json!({"cool_down":0})).unwrap(), DEFAULT_COOLDOWN);
        for value in [
            json!({}),
            json!({"cool_down":-1}),
            json!({"cool_down":"60"}),
            json!({"cool_down":86401}),
        ] {
            assert!(cooldown(&value).is_err());
        }
        assert_eq!(
            token(&json!({"user_id":42,"authorization":"fixture-token"})).unwrap(),
            "fixture-token"
        );
        for value in [
            json!({}),
            json!({"user_id":0,"authorization":"fixture"}),
            json!({"user_id":42,"authorization":""}),
            json!({"user_id":42,"authorization":"bad\nheader"}),
        ] {
            assert!(token(&value).is_err());
        }
        for bytes in [
            br#"{"code":400,"msg":"secret-value"}"#.as_slice(),
            br#"{"retCode":1}"#,
            br#"{"error":"secret-value"}"#,
            br#"[]"#,
            b"<html>secret-value</html>",
        ] {
            assert!(!parse(bytes)
                .unwrap_err()
                .to_string()
                .contains("secret-value"));
        }
    }

    #[test]
    fn http_errors_cooldowns_and_oversize_responses_are_safe() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
        };
        for (status, headers, body, expected) in [
            ("200 OK", "", "{\"cool_down\":90}", None),
            (
                "400 Bad Request",
                "",
                "{\"code\":1,\"msg\":\"secret-value\"}",
                Some(0),
            ),
            (
                "429 Too Many Requests",
                "Retry-After: 180\r\n",
                "secret-value",
                Some(180),
            ),
            ("401 Unauthorized", "", "secret-value", Some(0)),
            (
                "302 Found",
                "Location: https://example.invalid/\r\n",
                "",
                Some(0),
            ),
            ("200 OK", "", "<html>secret-value</html>", Some(0)),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let fixture = format!("HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut input = [0; 4096];
                stream.read(&mut input).unwrap();
                stream.write_all(fixture.as_bytes()).unwrap();
            });
            let client = Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(3))
                .build()
                .unwrap();
            let result = execute(client.get(format!("http://{addr}/fixture")));
            if let Some(delay) = expected {
                let error = result.unwrap_err();
                assert!(!error.to_string().contains("secret-value"));
                assert_eq!(error.retry_after.map(|d| d.as_secs()).unwrap_or(0), delay);
                if status.starts_with("400") {
                    assert!(error.to_string().contains("HTTP 400"));
                    assert!(!error.to_string().contains("手机号或验证码不正确"));
                }
            } else {
                assert_eq!(cooldown(&result.unwrap()).unwrap(), Duration::from_secs(90));
            }
            server.join().unwrap();
        }
    }
}
