//! One-time email authentication. Codes never enter logs, response bodies or plaintext storage.
use crate::*;
use hmac::{Hmac, Mac};
use rand::Rng;
use sha2::Sha256;

#[derive(Clone)]
pub struct Mailer {
    client: reqwest::Client,
    endpoint: String,
    key: String,
    from: String,
    pepper: String,
}
impl Mailer {
    pub fn from_env(origin: &str) -> Result<Option<Self>, Box<dyn std::error::Error>> {
        let Ok(key) = std::env::var("RESEND_API_KEY") else {
            return Ok(None);
        };
        let from = std::env::var("EMAIL_FROM")?;
        let pepper = std::env::var("EMAIL_CODE_SECRET")?;
        if key.is_empty() || from.contains(['\r', '\n']) || !from.contains('@') || pepper.len() < 32
        {
            return Err("invalid email configuration".into());
        }
        let endpoint = match std::env::var("EMAIL_TEST_ENDPOINT") {
            Ok(url)
                if origin.starts_with("http://127.0.0.1:")
                    && url.starts_with("http://127.0.0.1:") =>
            {
                url
            }
            Ok(_) => return Err("email test endpoint requires isolated loopback origin".into()),
            Err(_) => "https://api.resend.com/emails".into(),
        };
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(8))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Some(Self {
            client,
            endpoint,
            key,
            from,
            pepper,
        }))
    }
    fn hash(&self, email: &str, id: Uuid, code: &str) -> Vec<u8> {
        let mut mac = Hmac::<Sha256>::new_from_slice(self.pepper.as_bytes())
            .expect("HMAC accepts any key length");
        mac.update(format!("guard-email-v1:{id}:{email}:{code}").as_bytes());
        mac.finalize().into_bytes().to_vec()
    }
    fn matches(&self, email: &str, id: Uuid, code: &str, expected: &[u8]) -> bool {
        let mut mac = Hmac::<Sha256>::new_from_slice(self.pepper.as_bytes())
            .expect("HMAC accepts any key length");
        mac.update(format!("guard-email-v1:{id}:{email}:{code}").as_bytes());
        mac.verify_slice(expected).is_ok()
    }
    async fn send(&self, email: &str, id: Uuid, code: &str) -> ApiResult<()> {
        let mut response = self.client.post(&self.endpoint).bearer_auth(&self.key)
            .header("Idempotency-Key", format!("guard-login-{id}"))
            .json(&json!({"from":self.from,"to":[email],"subject":"加速器守护登录验证码",
                "text":format!("你的加速器守护验证码是：{code}\n\n10 分钟内有效，仅可使用一次。首次验证成功会创建普通平台账号。请勿向他人提供验证码。\n如果不是你本人操作，请忽略此邮件。\n加速器守护是独立开源工具，与雷神加速器官方无隶属关系。") }))
            .send().await.map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE,"验证码发送暂不可用，请稍后重试"))?;
        if !response.status().is_success() {
            tracing::warn!(
                status = response.status().as_u16(),
                "mail provider rejected delivery"
            );
            return Err(ApiError(
                StatusCode::SERVICE_UNAVAILABLE,
                "验证码发送暂不可用，请稍后重试",
            ));
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| {
            ApiError(
                StatusCode::SERVICE_UNAVAILABLE,
                "验证码发送状态未确认，请稍后重试",
            )
        })? {
            if body.len() + chunk.len() > 8192 {
                return Err(ApiError(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "验证码发送状态未确认，请稍后重试",
                ));
            }
            body.extend_from_slice(&chunk);
        }
        let data: Value = serde_json::from_slice(&body).map_err(|_| {
            ApiError(
                StatusCode::SERVICE_UNAVAILABLE,
                "验证码发送状态未确认，请稍后重试",
            )
        })?;
        if data
            .get("id")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        {
            return Err(ApiError(
                StatusCode::SERVICE_UNAVAILABLE,
                "验证码发送状态未确认，请稍后重试",
            ));
        }
        Ok(())
    }
}
pub fn normalize(input: &str) -> Option<String> {
    let value = input.trim().to_ascii_lowercase();
    let (local, domain) = value.split_once('@')?;
    if value.len() > 254
        || local.is_empty()
        || local.len() > 64
        || local.starts_with('.')
        || local.ends_with('.')
        || local.contains("..")
        || !local
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._%+-".contains(&c))
        || !domain.contains('.')
        || domain.len() > 253
        || domain.split('.').any(|label| {
            label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        })
    {
        return None;
    }
    Some(value)
}
async fn budget(s: &AppState, key: String, max: i32, seconds: f64) -> ApiResult<()> {
    let hits: i32 = sqlx::query_scalar("INSERT INTO email_rate_limits(key,hits,expires_at) VALUES($1,1,now()+make_interval(secs=>$2)) ON CONFLICT(key) DO UPDATE SET hits=CASE WHEN email_rate_limits.expires_at<=now() THEN 1 ELSE email_rate_limits.hits+1 END,expires_at=CASE WHEN email_rate_limits.expires_at<=now() THEN EXCLUDED.expires_at ELSE email_rate_limits.expires_at END RETURNING hits")
        .bind(key).bind(seconds).fetch_one(&s.db).await?;
    if hits > max {
        return Err(ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "验证码请求过于频繁，请稍后再试",
        ));
    }
    Ok(())
}
#[derive(Deserialize)]
pub struct SendCode {
    email: String,
}
pub async fn send_code(
    State(s): State<AppState>,
    h: HeaderMap,
    Json(input): Json<SendCode>,
) -> ApiResult<Json<Value>> {
    auth::same_origin(&s, &h)?;
    let email =
        normalize(&input.email).ok_or(ApiError(StatusCode::BAD_REQUEST, "请输入有效邮箱"))?;
    let mail = s.mailer.as_ref().ok_or(ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "邮件服务尚未配置",
    ))?;
    // Persist limits before sending. Restarts and multiple workers cannot reset the mail budget.
    budget(
        &s,
        format!("send-ip:{}", auth::digest(&auth::client_key(&h))),
        10,
        900.,
    )
    .await?;
    budget(&s, format!("send-email:{}", auth::digest(&email)), 6, 3600.).await?;
    budget(&s, "send-global-day".into(), 100, 86400.).await?;
    let id = Uuid::new_v4();
    let code = format!("{:06}", rand::rngs::OsRng.gen_range(0..1_000_000u32));
    let changed = sqlx::query("INSERT INTO email_challenges(email,request_id,code_hash,expires_at) VALUES($1,$2,$3,now()+interval '10 minutes') ON CONFLICT(email) DO UPDATE SET request_id=EXCLUDED.request_id,code_hash=EXCLUDED.code_hash,requested_at=now(),expires_at=EXCLUDED.expires_at,attempts=0,ready=false WHERE email_challenges.requested_at<now()-interval '60 seconds'")
        .bind(&email).bind(id).bind(mail.hash(&email,id,&code)).execute(&s.db).await?;
    if changed.rows_affected() != 1 {
        return Err(ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "请等待 60 秒后重新获取验证码",
        ));
    }
    // No database transaction or row lock is held across the external delivery request.
    if let Err(error) = mail.send(&email, id, &code).await {
        sqlx::query("DELETE FROM email_challenges WHERE email=$1 AND request_id=$2")
            .bind(&email)
            .bind(id)
            .execute(&s.db)
            .await?;
        return Err(error);
    }
    sqlx::query("UPDATE email_challenges SET ready=true WHERE email=$1 AND request_id=$2")
        .bind(&email)
        .bind(id)
        .execute(&s.db)
        .await?;
    Ok(Json(
        json!({"ok":true,"request_id":id,"expires_in":600,"retry_after":60}),
    ))
}
#[derive(Deserialize)]
pub struct VerifyCode {
    email: String,
    request_id: Uuid,
    code: String,
    #[serde(default)]
    remember: bool,
}
pub async fn verify_code(
    State(s): State<AppState>,
    h: HeaderMap,
    Json(input): Json<VerifyCode>,
) -> ApiResult<Response> {
    auth::same_origin(&s, &h)?;
    let invalid = || ApiError(StatusCode::UNAUTHORIZED, "验证码无效、已使用或已过期");
    let email = normalize(&input.email).ok_or_else(invalid)?;
    if input.code.len() != 6 || !input.code.bytes().all(|c| c.is_ascii_digit()) {
        return Err(invalid());
    }
    auth::throttle(
        &s,
        format!("email-verify:{}", auth::client_key(&h)),
        30,
        900,
    )?;
    let mail = s.mailer.as_ref().ok_or(ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "邮件服务尚未配置",
    ))?;
    let mut tx = s.db.begin().await?;
    let row = sqlx::query("SELECT code_hash,attempts FROM email_challenges WHERE email=$1 AND request_id=$2 AND ready AND expires_at>now() FOR UPDATE")
        .bind(&email).bind(input.request_id).fetch_optional(&mut *tx).await?.ok_or_else(invalid)?;
    if row.get::<i32, _>("attempts") >= 5 {
        return Err(invalid());
    }
    if !mail.matches(
        &email,
        input.request_id,
        &input.code,
        &row.get::<Vec<u8>, _>("code_hash"),
    ) {
        sqlx::query("UPDATE email_challenges SET attempts=attempts+1 WHERE email=$1")
            .bind(&email)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        return Err(invalid());
    }
    // Never infer verified ownership or administrator privileges from a legacy username.
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO users(id,username,display_name,password_hash,role,email,email_verified_at) VALUES($1,$2,$3,'!','user',$4,now()) ON CONFLICT(email) DO NOTHING")
        .bind(id).bind(format!("email-{}",id.simple())).bind("邮箱用户").bind(&email).execute(&mut *tx).await?;
    let user = sqlx::query("SELECT id,disabled,role FROM users WHERE email=$1 FOR UPDATE")
        .bind(&email)
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM email_challenges WHERE email=$1")
        .bind(&email)
        .execute(&mut *tx)
        .await?;
    if user.get::<bool, _>("disabled") || user.get::<String, _>("role") != "user" {
        tx.commit().await?;
        return Err(invalid());
    }
    let uid: Uuid = user.get("id");
    let (token, seconds) = auth::issue_session(&mut tx, uid, input.remember).await?;
    routes::audit(&mut tx, uid, None, "email_login", "邮箱验证登录平台").await?;
    tx.commit().await?;
    Ok((
        [(
            axum::http::header::SET_COOKIE,
            auth::session_cookie(&s, &token, seconds),
        )],
        Json(json!({"ok":true})),
    )
        .into_response())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn email_addresses_are_normalized_and_bounded() {
        assert_eq!(
            normalize(" User+tag@Example.com ").as_deref(),
            Some("user+tag@example.com")
        );
        for bad in [
            "a@@b.com",
            "a@localhost",
            "a\r\nb@c.com",
            "a@-example.com",
            ".a@b.com",
            "a..b@c.com",
        ] {
            assert!(normalize(bad).is_none());
        }
    }
}
