//! Add a verified email to an existing password account without changing its identity.
use crate::*;
use rand::Rng;

#[derive(Deserialize)]
pub struct SendCode {
    email: String,
}

pub async fn send_code(
    State(s): State<AppState>,
    h: HeaderMap,
    Json(input): Json<SendCode>,
) -> ApiResult<Json<Value>> {
    let user = auth::user(&s, &h, true).await?;
    let email = email::normalize(&input.email)
        .ok_or(ApiError(StatusCode::BAD_REQUEST, "请输入有效邮箱"))?;
    if user.email.is_some() || !user.password_enabled {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "此账号已绑定邮箱，不能重复绑定或直接更换",
        ));
    }
    let mail = s.mailer.as_ref().ok_or(ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "邮件服务尚未配置",
    ))?;
    email::budget(&s, format!("bind-send:{}", user.id), 6, 3600.).await?;
    email::delivery_budget(&s, &h, &email).await?;
    let id = Uuid::new_v4();
    let code = format!("{:06}", rand::rngs::OsRng.gen_range(0..1_000_000u32));
    let mut tx = storage::begin(&s.db).await?;
    let eligible: bool = sqlx::query_scalar("SELECT email IS NULL AND NOT disabled AND password_hash<>'!' FROM users WHERE id=? FOR UPDATE")
        .bind(user.id).fetch_one(&mut *tx).await?;
    if !eligible {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "账号状态已变化，请刷新后重试",
        ));
    }
    ensure_available(&mut tx, &email).await?;
    let recent: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM email_binding_challenges WHERE (user_id=? OR email=?) AND requested_at>=UTC_TIMESTAMP(6)-INTERVAL 60 SECOND) OR EXISTS(SELECT 1 FROM email_challenges WHERE email=? AND requested_at>=UTC_TIMESTAMP(6)-INTERVAL 60 SECOND)")
        .bind(user.id).bind(&email).bind(&email).fetch_one(&mut *tx).await?;
    if recent {
        return Err(ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "请等待 60 秒后重新获取验证码",
        ));
    }
    sqlx::query("INSERT INTO email_binding_challenges(user_id,email,request_id,code_hash,expires_at) VALUES(?,?,?,?,UTC_TIMESTAMP(6)+INTERVAL 10 MINUTE) ON DUPLICATE KEY UPDATE email=?,request_id=?,code_hash=?,requested_at=UTC_TIMESTAMP(6),expires_at=UTC_TIMESTAMP(6)+INTERVAL 10 MINUTE,attempts=0,ready=false")
        .bind(user.id).bind(&email).bind(id).bind(mail.hash(&email,id,&code))
        .bind(&email).bind(id).bind(mail.hash(&email,id,&code)).execute(&mut *tx).await?;
    tx.commit().await?;
    // Delivery never holds database/coordinator locks or grants account ownership.
    if let Err(error) = mail.send(&email, id, &code, true).await {
        sqlx::query("DELETE FROM email_binding_challenges WHERE user_id=? AND request_id=?")
            .bind(user.id)
            .bind(id)
            .execute(&s.db)
            .await?;
        return Err(error);
    }
    sqlx::query("UPDATE email_binding_challenges SET ready=true WHERE user_id=? AND request_id=?")
        .bind(user.id)
        .bind(id)
        .execute(&s.db)
        .await?;
    Ok(Json(
        json!({"ok":true,"request_id":id,"expires_in":600,"retry_after":60}),
    ))
}

async fn ensure_available(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    email: &str,
) -> ApiResult<()> {
    let occupied: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE email=?)")
        .bind(email)
        .fetch_one(&mut **tx)
        .await?;
    if occupied {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "该邮箱已关联平台账号，请使用该邮箱登录；不能合并或覆盖账号",
        ));
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct BindEmail {
    email: String,
    request_id: Uuid,
    code: String,
    current_password: String,
}

pub async fn bind(
    State(s): State<AppState>,
    h: HeaderMap,
    Json(input): Json<BindEmail>,
) -> ApiResult<Json<Value>> {
    let user = auth::user(&s, &h, true).await?;
    let invalid = || {
        ApiError(
            StatusCode::BAD_REQUEST,
            "绑定验证码无效、已使用或已过期，请重新获取",
        )
    };
    let email = email::normalize(&input.email).ok_or_else(invalid)?;
    if input.code.len() != 6 || !input.code.bytes().all(|c| c.is_ascii_digit()) {
        return Err(invalid());
    }
    if user.email.is_some() || !user.password_enabled {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "此账号已绑定邮箱，不能重复绑定或直接更换",
        ));
    }
    email::budget(&s, format!("bind-verify:{}", user.id), 15, 900.).await?;
    if input.current_password.is_empty() || input.current_password.len() > 128 {
        return Err(ApiError(StatusCode::BAD_REQUEST, "当前密码不正确"));
    }
    let mail = s.mailer.as_ref().ok_or(ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "邮件服务尚未配置",
    ))?;
    let expected: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id=?")
        .bind(user.id)
        .fetch_one(&s.db)
        .await?;
    let hash = expected.clone();
    let permit = s
        .hashes
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError(StatusCode::TOO_MANY_REQUESTS, "操作繁忙，请稍后重试"))?;
    let valid = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        auth::check_password(&input.current_password, &hash)
    })
    .await
    .unwrap_or(false);
    if !valid {
        return Err(ApiError(StatusCode::BAD_REQUEST, "当前密码不正确"));
    }
    let mut tx = storage::begin(&s.db).await?;
    // Recheck credentials and session under the same coordinator as password changes,
    // user disable, login and competing binds; a preflight read alone is insufficient.
    sqlx::query("SELECT token_hash FROM sessions WHERE user_id=? AND token_hash=? AND expires_at>UTC_TIMESTAMP(6) FOR UPDATE")
        .bind(user.id).bind(auth::digest(&auth::cookie_token(&s,&h).unwrap_or_default()))
        .fetch_optional(&mut *tx).await?
        .ok_or(ApiError(StatusCode::UNAUTHORIZED,"会话已过期，请重新登录"))?;
    let eligible: bool = sqlx::query_scalar("SELECT email IS NULL AND NOT disabled AND password_hash=? AND EXISTS(SELECT 1 FROM sessions WHERE user_id=users.id AND token_hash=? AND expires_at>UTC_TIMESTAMP(6)) FROM users WHERE id=? FOR UPDATE")
        .bind(&expected).bind(auth::digest(&auth::cookie_token(&s,&h).unwrap_or_default()))
        .bind(user.id).fetch_one(&mut *tx).await?;
    if !eligible {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "账号或会话状态已变化，请重新登录",
        ));
    }
    let row=sqlx::query("SELECT code_hash,attempts FROM email_binding_challenges WHERE user_id=? AND email=? AND request_id=? AND ready AND expires_at>UTC_TIMESTAMP(6) FOR UPDATE")
        .bind(user.id).bind(&email).bind(input.request_id).fetch_optional(&mut *tx).await?.ok_or_else(invalid)?;
    if row.get::<i32, _>("attempts") >= 5 {
        return Err(invalid());
    }
    if !mail.matches(
        &email,
        input.request_id,
        &input.code,
        &row.get::<Vec<u8>, _>("code_hash"),
    ) {
        sqlx::query("UPDATE email_binding_challenges SET attempts=attempts+1 WHERE user_id=?")
            .bind(user.id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        return Err(invalid());
    }
    ensure_available(&mut tx, &email).await?;
    sqlx::query("UPDATE users SET email=?,email_verified_at=UTC_TIMESTAMP(6) WHERE id=?")
        .bind(&email)
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM email_binding_challenges WHERE user_id=? OR email=?")
        .bind(user.id)
        .bind(&email)
        .execute(&mut *tx)
        .await?;
    // A login code issued before ownership changed must not gain access afterwards.
    sqlx::query("DELETE FROM email_challenges WHERE email=?")
        .bind(&email)
        .execute(&mut *tx)
        .await?;
    routes::audit(
        &mut tx,
        user.id,
        None,
        "email_bound",
        "验证并绑定登录邮箱；账号与设备归属保持不变",
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"ok":true,"email":email})))
}
