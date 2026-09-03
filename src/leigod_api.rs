//! 雷神加速器 Web API 封装层（未公开接口，逆向自官方客户端，可能随时失效）。
//! 接口研究参考：https://github.com/XuHandsome/leigod-helper
use md5::{Digest, Md5};
use serde_json::{Map, Value};

pub const HOST: &str = "https://webapi.leigod.com";
pub const PAUSE_PATH: &str = "/api/user/pause";
/// 恢复计时接口（官方未公开，按对称命名推测，失效时会自动降级提醒）
pub const RECOVER_PATH: &str = "/api/user/recover";
/// 账户信息接口（用于查询剩余时长，尽力而为）
pub const USER_INFO_PATH: &str = "/api/user/info";

// 已公开的客户端协议常量，不是任何用户的密码、账户 token 或私人 API 密钥。
// 公开出处：https://github.com/XuHandsome/leigod-helper/blob/main/libs/consts.go
const SIGN_SECRET: &str = "5C5A639C20665313622F51E93E3F2783";

/// 极验 v4 行为验证通过后的凭据。
/// 官网当前设置（is_off_geetest_login=1）使用极验 v4（captchaId 模式），
/// 验证结果字段为 lot_number/captcha_output/pass_token/gen_time，
/// 并额外回传 geetest/config 拿到的 server_status。
/// （旧版 v3 的 geetest_challenge/validate/seccode 服务器已不认可，会报 400001）
#[derive(Clone, serde::Deserialize)]
pub struct CaptchaProof {
    pub lot_number: String,
    pub captcha_output: String,
    pub pass_token: String,
    pub gen_time: String,
    #[serde(default)]
    pub server_status: i64,
}

/// 官网前端硬编码的极验 v4 captchaId（发短信用）
pub const GEETEST_V4_CAPTCHA_ID: &str = "95b0b1c603d85acf526d8c82fcc5b731";
/// 官网密码登录专用的极验 v4 captchaId（与发短信的不是同一个）
pub const GEETEST_V4_CAPTCHA_ID_PWD: &str = "f3b7531c463d4779cbbd6e2c21d8b9b6";

impl CaptchaProof {
    /// 把验证凭据并入请求体（官网做法：{...表单, ...captcha.getValidate(), server_status}）
    pub fn apply(&self, body: &mut Map<String, Value>) {
        body.insert("lot_number".into(), Value::from(self.lot_number.clone()));
        body.insert(
            "captcha_output".into(),
            Value::from(self.captcha_output.clone()),
        );
        body.insert("pass_token".into(), Value::from(self.pass_token.clone()));
        body.insert("gen_time".into(), Value::from(self.gen_time.clone()));
        body.insert("server_status".into(), Value::from(self.server_status));
    }

    pub fn parse(json: &str) -> Result<Self, ApiError> {
        // serde 的类型错误可能包含字段原值，不能把验证凭据带入日志。
        serde_json::from_str(json).map_err(|e| {
            ApiError(format!(
                "验证结果解析失败（第 {} 行，第 {} 列）",
                e.line(),
                e.column()
            ))
        })
    }
}

/// 判断错误是否为「需要人机验证/触发风控」类错误
pub fn is_captcha_err(e: &ApiError) -> bool {
    e.0.contains("验证")
        || e.0.contains("风控")
        || e.0.contains("code=500003")
        || e.0.contains("code=400857")
        || e.0.contains("code=400001")
}

/// 获取极验服务器状态（官网前端流程：POST /tools/captcha/geetest/config {type:"web"}，
/// v4 验证通过后需把 server_status 一并回传给业务接口）。
pub fn geetest_server_status() -> Result<i64, ApiError> {
    let mut body = Map::new();
    body.insert("type".into(), Value::from("web"));
    let v = post("/tools/captcha/geetest/config", &body)?;
    check_ok(&v)?;
    Ok(v.pointer("/data/server_status")
        .and_then(|s| s.as_i64())
        .unwrap_or(1))
}

#[derive(Debug)]
pub struct ApiError(pub String);

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

fn md5_hex(input: &str) -> String {
    let mut h = Md5::new();
    h.update(input.as_bytes());
    format!("{:x}", h.finalize())
}

/// 对已排序的参数做签名：k=v&k=v 按 key 字典序拼接，末尾追加密钥，整体 MD5。
fn sign(params: &mut Map<String, Value>) {
    let ts = chrono::Utc::now().timestamp();
    params.insert("ts".into(), Value::from(ts));
    let mut pairs: Vec<(&String, &Value)> = params.iter().collect();
    pairs.sort_by(|a, b| a.0.cmp(b.0));
    let mut qs = String::new();
    for (i, (k, v)) in pairs.iter().enumerate() {
        if i > 0 {
            qs.push('&');
        }
        let vs = match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        qs.push_str(&format!("{k}={vs}"));
    }
    qs.push_str(&format!("&key={SIGN_SECRET}"));
    params.insert("sign".into(), Value::from(md5_hex(&qs)));
}

fn client() -> Result<reqwest::blocking::Client, ApiError> {
    reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(12))
        // 登录请求体和 query 中含有凭据，禁止重定向到其他地址。
        .redirect(reqwest::redirect::Policy::none())
        .https_only(true)
        .build()
        .map_err(|e| network_error("构建 HTTP 客户端失败", e))
}

fn network_error(context: &str, error: reqwest::Error) -> ApiError {
    // reqwest 的默认错误文本会附带完整 URL，其中可能包含 account_token。
    ApiError(format!("{context}: {}", error.without_url()))
}

fn parse_json(text: &str) -> Result<Value, ApiError> {
    serde_json::from_str::<Value>(text).map_err(|e| {
        // HTML 响应一般是 WAF 拦截页或接口已下线，给出可读提示
        let hint = if text.trim_start().starts_with('<') {
            if text.contains("CloudWAF") || text.contains("访问被拦截") {
                "服务器防火墙拦截（接口可能已变更或被风控）"
            } else {
                "服务器返回了网页而不是数据（接口可能已变更）"
            }
        } else {
            "响应格式异常"
        };
        // 不截取或记录响应正文：正文可能含凭据，按字节截取中文也会 panic。
        ApiError(format!(
            "{hint}（第 {} 行，第 {} 列）",
            e.line(),
            e.column()
        ))
    })
}

fn post(path: &str, body: &Map<String, Value>) -> Result<Value, ApiError> {
    let url = format!("{HOST}{path}");
    let resp = client()?
        .post(&url)
        .json(body)
        .send()
        .map_err(|e| network_error("网络请求失败", e))?;
    let text = resp.text().map_err(|e| network_error("读取响应失败", e))?;
    parse_json(&text)
}

/// 官网登录后接口的统一形状（逆向自官网 axios 拦截器）：
/// account_token 与元信息放 **query string**，body 为 {}，**不签名**。
/// 暂停/恢复/查询均如此；签名只用于登录类接口。
fn post_authed(path: &str, token: &str) -> Result<Value, ApiError> {
    let resp = authed_request(&client()?, path, token)
        .send()
        .map_err(|e| network_error("网络请求失败", e))?;
    let text = resp.text().map_err(|e| network_error("读取响应失败", e))?;
    parse_json(&text)
}

fn authed_request(
    client: &reqwest::blocking::Client,
    path: &str,
    token: &str,
) -> reqwest::blocking::RequestBuilder {
    client
        .post(format!("{HOST}{path}"))
        // 使用 URL 编码，避免 token 内的 &、+ 等字符改变 query 参数。
        .query(&[
            ("os_type", "4"),
            ("account_token", token),
            ("region_code", "1"),
            ("src_channel", "guanwang"),
            ("lang", "zh_CN"),
        ])
        .json(&Map::new())
}

/// 判断错误是否为 token 失效（400006 账号未登录），只有这种情况才值得清 token 重登
pub fn is_token_err(e: &ApiError) -> bool {
    e.0.contains("code=400006") || e.0.contains("未登录") || e.0.contains("过期")
}

/// 提取业务错误信息
fn check_ok(v: &Value) -> Result<(), ApiError> {
    let code = v.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
    if code == 0 {
        Ok(())
    } else {
        let msg = v
            .get("msg")
            .and_then(|m| m.as_str())
            .unwrap_or("未知错误")
            .to_string();
        Err(ApiError(format!("服务器返回错误 code={code}: {msg}")))
    }
}

/// 用 手机号 + MD5(密码) 登录，返回 account_token。
/// 官网当前走 /api/auth/login/v2（旧 v1 已被 WAF 拦截下线）。
/// 官网密码登录流程：先弹极验 v4（captchaId f3b7531c…，与发短信的不同），
/// body 为 {country_code, mobile_num, username, password:MD5, ...v4凭据}。
/// proof 为极验 v4 凭据（首次不带，服务器报"验证码不能为空"后由 UI 弹窗补齐重试）。
pub fn login_with_hash(
    username: &str,
    password_md5: &str,
    proof: Option<&CaptchaProof>,
) -> Result<String, ApiError> {
    let mut body = Map::new();
    body.insert("username".into(), Value::from(username));
    body.insert("mobile_num".into(), Value::from(username));
    body.insert("password".into(), Value::from(password_md5));
    body.insert("country_code".into(), Value::from(86));
    body.insert("lang".into(), Value::from("zh_CN"));
    if let Some(p) = proof {
        p.apply(&mut body);
    }
    sign(&mut body);
    let v = post("/api/auth/login/v2", &body)?;
    check_ok(&v)?;
    v.pointer("/data/login_info/account_token")
        .or_else(|| v.pointer("/data/account_token"))
        .and_then(|t| t.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| ApiError("登录成功但未找到账户凭据，请稍后重试".into()))
}

pub fn password_md5(plain: &str) -> String {
    md5_hex(plain)
}

/// 暂停计时（官网形状：token 走 query、body {}、不签名）
/// 400803「账号已经停止加速」说明已是目标状态，按成功处理（幂等）
pub fn pause(token: &str) -> Result<String, ApiError> {
    let v = post_authed(PAUSE_PATH, token)?;
    match check_ok(&v) {
        Ok(()) => Ok(v
            .get("msg")
            .and_then(|m| m.as_str())
            .unwrap_or("已暂停")
            .to_string()),
        Err(e) if e.0.contains("code=400803") => Ok("已是暂停状态".to_string()),
        Err(e) => Err(e),
    }
}

/// 恢复计时（官网形状：token 走 query、body {}、不签名）
/// 400804「账号已经恢复加速」说明已是目标状态，按成功处理（幂等）
pub fn recover(token: &str) -> Result<String, ApiError> {
    let v = post_authed(RECOVER_PATH, token)?;
    match check_ok(&v) {
        Ok(()) => Ok(v
            .get("msg")
            .and_then(|m| m.as_str())
            .unwrap_or("已恢复")
            .to_string()),
        Err(e) if e.0.contains("code=400804") => Ok("已是计时状态".to_string()),
        Err(e) => Err(e),
    }
}

/// 发送短信登录验证码，返回 smscode_key（登录时需回传）。
/// 请求体对齐官网前端 LoginPhone 的实际形状（state=4 为登录用途，
/// 同时携带 mobile_num 与 phone 两个字段）；proof 为极验 v4 凭据
///（官网流程：发送前先弹行为验证，结果直接 spread 进请求体）。
pub fn send_sms_code(phone: &str, proof: Option<&CaptchaProof>) -> Result<String, ApiError> {
    let mut body = Map::new();
    body.insert("country_code".into(), Value::from(86));
    body.insert("mobile_num".into(), Value::from(phone));
    body.insert("phone".into(), Value::from(phone));
    body.insert("smscode".into(), Value::from(""));
    body.insert("state".into(), Value::from(4));
    body.insert("lang".into(), Value::from("zh_CN"));
    if let Some(p) = proof {
        p.apply(&mut body);
    }
    // 该接口挂在 host 根路径（/tools/...），不在 /api 下
    let v = post("/tools/smscode", &body)?;
    check_ok(&v)?;
    v.pointer("/data/smscode_key")
        .and_then(|k| k.as_str())
        .map(String::from)
        .or_else(|| v.get("data").and_then(|d| d.as_str()).map(String::from))
        .ok_or_else(|| ApiError("验证码已请求但未找到短信登录凭据，请稍后重试".into()))
}

/// 手机号 + 短信验证码登录。
/// 官网当前走 /api/auth/login/code（旧 /api/auth/login 已被 WAF 拦截下线）。
/// 实测服务端逐字段校验：phone 必填 → smscode_key 必填 → 再校验验证码本身。
pub fn login_with_code(phone: &str, code: &str, smscode_key: &str) -> Result<String, ApiError> {
    let mut body = Map::new();
    body.insert("phone".into(), Value::from(phone));
    body.insert("mobile_num".into(), Value::from(phone));
    body.insert("smscode".into(), Value::from(code));
    body.insert("smscode_key".into(), Value::from(smscode_key));
    body.insert("country_code".into(), Value::from(86));
    body.insert("lang".into(), Value::from("zh_CN"));
    let v = post("/api/auth/login/code", &body)?;
    check_ok(&v)?;
    v.pointer("/data/login_info/account_token")
        .or_else(|| v.pointer("/data/account_token"))
        .and_then(|t| t.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| ApiError("登录成功但未找到账户凭据，请稍后重试".into()))
}

/// 查询账户信息（剩余时长等），失败不影响主流程（官网形状：query token、body {}、不签名）
pub fn user_info(token: &str) -> Result<Value, ApiError> {
    let v = post_authed(USER_INFO_PATH, token)?;
    check_ok(&v)?;
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_unicode_response_is_safe_and_private() {
        // 150 bytes falls inside a Chinese character after this one-byte prefix.
        let response = format!("x{} account_token=private-test-value", "界".repeat(60));
        let error = parse_json(&response).unwrap_err();
        assert!(error.0.contains("响应格式异常"));
        assert!(!error.0.contains("private-test-value"));
        assert!(!error.0.contains('界'));
    }

    #[test]
    fn waf_error_keeps_hint_without_response_body() {
        let error = parse_json("<html>CloudWAF private-test-value</html>").unwrap_err();
        assert!(error.0.contains("服务器防火墙拦截"));
        assert!(!error.0.contains("private-test-value"));
    }

    #[test]
    fn invalid_captcha_fields_are_not_echoed() {
        let error = CaptchaProof::parse(
            r#"{"lot_number":"a","captcha_output":"b","pass_token":"c","gen_time":"d","server_status":"private-test-value"}"#,
        )
        .err()
        .expect("string server_status should fail parsing");
        assert!(error.0.contains("验证结果解析失败"));
        assert!(!error.0.contains("private-test-value"));
    }

    #[test]
    fn network_errors_remove_account_token_url() {
        let client = client().unwrap();
        // Build an error entirely offline and attach the same URL reqwest would attach.
        let error = client.post("not a URL").build().unwrap_err().with_url(
            reqwest::Url::parse("https://example.invalid/api?account_token=private-test-value")
                .unwrap(),
        );
        assert!(error.to_string().contains("private-test-value"));
        let error = network_error("网络请求失败", error);
        assert!(!error.0.contains("private-test-value"));
        assert!(!error.0.contains("account_token"));
    }

    #[test]
    fn authenticated_query_preserves_token_as_one_value() {
        let token = "private+test&value=with#special?characters";
        let request = authed_request(&client().unwrap(), PAUSE_PATH, token)
            .build()
            .unwrap();
        let pairs: Vec<_> = request.url().query_pairs().collect();
        assert_eq!(pairs.len(), 5);
        assert_eq!(
            pairs
                .iter()
                .find(|(key, _)| key == "account_token")
                .unwrap()
                .1,
            token
        );
        assert_eq!(request.url().fragment(), None);
    }
}
