use crate::*;

pub async fn health(State(s): State<AppState>) -> ApiResult<Json<Value>> {
    sqlx::query("SELECT 1").execute(&s.db).await?;
    Ok(Json(
        json!({"status":"ok","version":env!("CARGO_PKG_VERSION"),"mode":if s.provider.is_some(){"remote"}else{"observe"},"remote_execution":s.provider.is_some()}),
    ))
}
#[derive(Deserialize)]
pub struct Login {
    username: String,
    password: String,
    #[serde(default)]
    remember: bool,
}
pub async fn login(
    State(s): State<AppState>,
    h: HeaderMap,
    Json(input): Json<Login>,
) -> ApiResult<Response> {
    auth::same_origin(&s, &h)?;
    let username = input.username.trim().to_lowercase();
    auth::throttle(&s, format!("login:{}", auth::client_key(&h)), 20, 900)?;
    auth::throttle(&s, format!("account:{}", auth::digest(&username)), 15, 900)?;
    if input.password.len() > 128 || username.len() > 100 {
        return Err(ApiError(StatusCode::UNAUTHORIZED, "账号或密码不正确"));
    }
    let row = sqlx::query("SELECT id,password_hash,disabled FROM users WHERE username=$1")
        .bind(&username)
        .fetch_optional(&s.db)
        .await?;
    let hash = row
        .as_ref()
        .map(|r| r.get::<String, _>("password_hash"))
        .unwrap_or(s.dummy_hash.clone());
    let expected_hash = hash.clone();
    let permit = s
        .hashes
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError(StatusCode::TOO_MANY_REQUESTS, "登录繁忙，请稍后重试"))?;
    let valid = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        auth::check_password(&input.password, &hash)
    })
    .await
    .unwrap_or(false);
    if !valid || row.as_ref().is_none_or(|r| r.get::<bool, _>("disabled")) {
        return Err(ApiError(StatusCode::UNAUTHORIZED, "账号或密码不正确"));
    }
    let id: Uuid = row.unwrap().get("id");
    let mut tx = s.db.begin().await?;
    // Serialize session creation with user disable/password changes.
    let enabled: bool = sqlx::query_scalar(
        "SELECT NOT disabled AND password_hash=$2 FROM users WHERE id=$1 FOR UPDATE",
    )
    .bind(id)
    .bind(expected_hash)
    .fetch_one(&mut *tx)
    .await?;
    if !enabled {
        return Err(ApiError(StatusCode::UNAUTHORIZED, "账号或密码不正确"));
    }
    let (token, seconds) = auth::issue_session(&mut tx, id, input.remember).await?;
    audit(&mut tx, id, None, "login", "登录平台").await?;
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
pub async fn logout(State(s): State<AppState>, h: HeaderMap) -> ApiResult<Response> {
    let _ = auth::user(&s, &h, true).await?;
    sqlx::query("DELETE FROM sessions WHERE token_hash=$1")
        .bind(auth::digest(
            &auth::cookie_token(&s, &h).unwrap_or_default(),
        ))
        .execute(&s.db)
        .await?;
    Ok((
        [(
            axum::http::header::SET_COOKIE,
            auth::session_cookie(&s, "", 0),
        )],
        Json(json!({"ok":true})),
    )
        .into_response())
}
pub async fn me(State(s): State<AppState>, h: HeaderMap) -> ApiResult<Json<SessionUser>> {
    Ok(Json(auth::user(&s, &h, false).await?))
}
#[derive(Deserialize)]
pub struct PasswordChange {
    current_password: String,
    new_password: String,
}
pub async fn password(
    State(s): State<AppState>,
    h: HeaderMap,
    Json(p): Json<PasswordChange>,
) -> ApiResult<Response> {
    let u = auth::user(&s, &h, true).await?;
    auth::throttle(&s, format!("password:{}", u.id), 5, 300)?;
    if !auth::valid_password(&p.new_password) || p.current_password.len() > 128 {
        return Err(ApiError(StatusCode::BAD_REQUEST, "新密码需为 12–128 字节"));
    }
    let old: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id=$1")
        .bind(u.id)
        .fetch_one(&s.db)
        .await?;
    let old_copy = old.clone();
    let permit = s
        .hashes
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError(StatusCode::TOO_MANY_REQUESTS, "操作繁忙，请稍后重试"))?;
    let new = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        if auth::check_password(&p.current_password, &old_copy) {
            auth::hash_password(&p.new_password).ok()
        } else {
            None
        }
    })
    .await
    .ok()
    .flatten()
    .ok_or(ApiError(StatusCode::BAD_REQUEST, "当前密码不正确"))?;
    let mut tx = s.db.begin().await?;
    let changed = sqlx::query(
        "UPDATE users SET password_hash=$1 WHERE id=$2 AND password_hash=$3 AND NOT disabled",
    )
    .bind(new)
    .bind(u.id)
    .bind(old)
    .execute(&mut *tx)
    .await?;
    if changed.rows_affected() != 1 {
        return Err(ApiError(StatusCode::CONFLICT, "账号状态已变化，请重新登录"));
    }
    sqlx::query("DELETE FROM sessions WHERE user_id=$1")
        .bind(u.id)
        .execute(&mut *tx)
        .await?;
    audit(
        &mut tx,
        u.id,
        None,
        "password_changed",
        "修改密码并撤销全部网页会话",
    )
    .await?;
    tx.commit().await?;
    Ok((
        [(
            axum::http::header::SET_COOKIE,
            auth::session_cookie(&s, "", 0),
        )],
        Json(json!({"ok":true})),
    )
        .into_response())
}
pub async fn audit(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    uid: Uuid,
    d: Option<Uuid>,
    kind: &str,
    detail: &str,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO events(user_id,device_id,kind,detail) VALUES($1,$2,$3,$4)")
        .bind(uid)
        .bind(d)
        .bind(kind)
        .bind(detail)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
#[derive(Serialize, sqlx::FromRow)]
struct Device {
    id: Uuid,
    user_id: Uuid,
    name: String,
    version: String,
    last_seen: Option<DateTime<Utc>>,
    game_running: Option<bool>,
    prepare_until: Option<DateTime<Utc>>,
    revoked: bool,
    status: String,
    username: String,
    leigod_account_key: Option<String>,
    leigod_account_label: Option<String>,
    account_updated_at: Option<DateTime<Utc>>,
}
async fn device_list(s: &AppState, owner: Option<Uuid>) -> ApiResult<Json<Value>> {
    let rows=sqlx::query_as::<_,Device>("SELECT d.id,d.user_id,d.name,d.version,d.last_seen,d.game_running,d.prepare_until,d.revoked,COALESCE(u.email,u.username) AS username,d.leigod_account_key,d.leigod_account_label,d.account_updated_at,CASE WHEN d.revoked THEN 'revoked' WHEN d.last_seen IS NULL THEN 'unknown' WHEN d.last_seen>now()-interval '45 seconds' THEN 'online' WHEN d.last_seen>now()-interval '120 seconds' THEN 'waiting' ELSE 'offline' END AS status FROM devices d JOIN users u ON u.id=d.user_id WHERE ($1::uuid IS NULL OR d.user_id=$1) ORDER BY d.revoked,d.created_at DESC LIMIT 500").bind(owner).fetch_all(&s.db).await?;
    Ok(Json(json!({"devices":rows,"limit":500})))
}
pub async fn devices(State(s): State<AppState>, h: HeaderMap) -> ApiResult<Json<Value>> {
    let u = auth::user(&s, &h, false).await?;
    device_list(&s, Some(u.id)).await
}
pub async fn all_devices(State(s): State<AppState>, h: HeaderMap) -> ApiResult<Json<Value>> {
    auth::admin(&s, &h, false).await?;
    device_list(&s, None).await
}
pub async fn revoke(
    State(s): State<AppState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    let u = auth::user(&s, &h, true).await?;
    let mut tx = s.db.begin().await?;
    let result =
        sqlx::query("UPDATE devices SET revoked=true WHERE id=$1 AND user_id=$2 AND NOT revoked")
            .bind(id)
            .bind(u.id)
            .execute(&mut *tx)
            .await?;
    if result.rows_affected() != 1 {
        return Err(ApiError(StatusCode::NOT_FOUND, "设备不存在或已撤销"));
    }
    audit(&mut tx, u.id, Some(id), "device_revoked", "撤销设备授权").await?;
    tx.commit().await?;
    Ok(Json(json!({"ok":true})))
}
pub async fn pairing(State(s): State<AppState>, h: HeaderMap) -> ApiResult<Json<Value>> {
    let u = auth::user(&s, &h, true).await?;
    auth::throttle(&s, format!("pairing:{}", u.id), 10, 600)?;
    let code = auth::secret()[..32].to_string();
    let mut tx = s.db.begin().await?;
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(u.id)
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM pairing_codes WHERE user_id=$1")
        .bind(u.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO pairing_codes(code_hash,user_id,expires_at) VALUES($1,$2,now()+interval '10 minutes')").bind(auth::digest(&code)).bind(u.id).execute(&mut *tx).await?;
    audit(
        &mut tx,
        u.id,
        None,
        "pairing_created",
        "生成配对码，有效期 10 分钟",
    )
    .await?;
    tx.commit().await?;
    Ok(Json(
        json!({"code":code,"expires_in":600,"server":s.origin}),
    ))
}
#[derive(Deserialize)]
pub struct Pair {
    code: String,
    name: String,
    version: String,
    #[serde(default)]
    installation_key: Option<String>,
}
pub async fn pair_device(
    State(s): State<AppState>,
    h: HeaderMap,
    Json(p): Json<Pair>,
) -> ApiResult<Json<Value>> {
    auth::throttle(&s, format!("pair:{}", auth::client_key(&h)), 20, 600)?;
    if p.code.len() != 32 || p.name.trim().is_empty() || p.name.len() > 100 || p.version.len() > 32
    {
        return Err(ApiError(StatusCode::BAD_REQUEST, "配对参数无效"));
    }
    // Authenticate before reserving a pool connection for the transaction.
    let current = if auth::cookie_token(&s, &h).is_some() {
        Some(auth::user(&s, &h, true).await?)
    } else {
        None
    };
    let mut tx = s.db.begin().await?;
    let hash = auth::digest(&p.code);
    let uid: Uuid = sqlx::query_scalar(
        "SELECT user_id FROM pairing_codes WHERE code_hash=$1 AND expires_at>now()",
    )
    .bind(&hash)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(ApiError(
        StatusCode::BAD_REQUEST,
        "配对码无效、已使用或已过期",
    ))?;
    // Lock the owner first, matching pairing creation and administrator disable.
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(uid)
        .fetch_one(&mut *tx)
        .await?;
    if let Some(current) = current {
        if current.id != uid {
            return Err(ApiError(StatusCode::CONFLICT, "配对码不属于当前平台账号"));
        }
    }
    let used = sqlx::query(
        "DELETE FROM pairing_codes WHERE code_hash=$1 AND user_id=$2 AND expires_at>now()",
    )
    .bind(hash)
    .bind(uid)
    .execute(&mut *tx)
    .await?;
    if used.rows_affected() != 1 {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "配对码无效、已使用或已过期",
        ));
    }
    let response = devices::bind(
        &mut tx,
        uid,
        p.installation_key.as_deref(),
        &p.name,
        &p.version,
        true,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(response))
}

#[derive(Deserialize)]
pub struct Heartbeat {
    #[serde(default)]
    run_generation: i64,
    #[serde(default)]
    remote_revision: Option<i64>,
    sequence: i64,
    game_running: Option<bool>,
    #[serde(default)]
    prepare_seconds: u32,
    version: String,
    #[serde(default = "keep_account")]
    account_action: String,
    #[serde(default)]
    leigod_account: Option<LeigodLink>,
}
fn keep_account() -> String {
    "keep".into()
}
#[derive(Deserialize)]
pub struct LeigodLink {
    key: String,
    label: String,
}
pub async fn heartbeat(
    State(s): State<AppState>,
    h: HeaderMap,
    Json(p): Json<Heartbeat>,
) -> ApiResult<Json<Value>> {
    let token = h
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .filter(|v| v.len() == 64)
        .ok_or(ApiError(StatusCode::UNAUTHORIZED, "设备凭据无效"))?;
    auth::throttle(&s, format!("heartbeat:{}", auth::digest(token)), 10, 30)?;
    if p.sequence < 0
        || p.sequence == i64::MAX
        || p.run_generation < 0
        || p.prepare_seconds > 600
        || p.version.len() > 32
    {
        return Err(ApiError(StatusCode::BAD_REQUEST, "心跳参数无效"));
    }
    if !matches!(p.account_action.as_str(), "keep" | "clear" | "link")
        || (p.account_action == "link"
            && p.leigod_account.as_ref().is_none_or(|a| {
                !devices::valid_key(&a.key)
                    || a.label.is_empty()
                    || a.label.len() > 100
                    || a.label.chars().any(char::is_control)
            }))
    {
        return Err(ApiError(StatusCode::BAD_REQUEST, "雷神账号关联参数无效"));
    }
    let mut tx = s.db.begin().await?;
    let row=sqlx::query("SELECT d.id,d.user_id,d.sequence,d.run_generation,d.last_seen,d.observed_offline,d.leigod_account_key FROM devices d JOIN users u ON u.id=d.user_id WHERE d.token_hash=$1 AND NOT d.revoked AND NOT u.disabled FOR UPDATE OF d")
        .bind(auth::digest(token)).fetch_optional(&mut *tx).await?.ok_or(ApiError(StatusCode::UNAUTHORIZED,"设备已撤销或账号已停用"))?;
    if p.sequence <= row.get::<i64, _>("sequence")
        || p.run_generation < row.get::<i64, _>("run_generation")
    {
        return Err(ApiError(StatusCode::CONFLICT, "心跳序号已使用；必须递增"));
    }
    let id: Uuid = row.get("id");
    let uid: Uuid = row.get("user_id");
    sqlx::query("UPDATE devices SET last_seen=now(),sequence=$2,game_running=$3,prepare_until=now()+make_interval(secs=>$4),version=$5,run_generation=$6,observed_offline=false WHERE id=$1")
        .bind(id).bind(p.sequence).bind(p.game_running).bind(p.prepare_seconds as f64).bind(p.version).bind(p.run_generation).execute(&mut *tx).await?;
    remote::heartbeat(
        &mut tx,
        id,
        p.run_generation,
        p.remote_revision,
        p.account_action == "clear",
        p.prepare_seconds,
    )
    .await?;
    if p.account_action != "keep" {
        let key = if p.account_action == "link" {
            p.leigod_account.as_ref().map(|a| a.key.as_str())
        } else {
            None
        };
        let label = if p.account_action == "link" {
            p.leigod_account.as_ref().map(|a| a.label.as_str())
        } else {
            None
        };
        if row
            .get::<Option<String>, _>("leigod_account_key")
            .as_deref()
            != key
        {
            sqlx::query("UPDATE devices SET leigod_account_key=$2,leigod_account_label=$3,account_updated_at=now() WHERE id=$1")
                .bind(id).bind(key).bind(label).execute(&mut *tx).await?;
            audit(
                &mut tx,
                uid,
                Some(id),
                "account_link_changed",
                if key.is_some() {
                    "设备已关联本机雷神账号标识"
                } else {
                    "设备已解除雷神账号关联"
                },
            )
            .await?;
        }
    }
    if row.get::<Option<DateTime<Utc>>, _>("last_seen").is_none()
        || row.get::<bool, _>("observed_offline")
    {
        audit(
            &mut tx,
            uid,
            Some(id),
            "device_online",
            "收到有效心跳，设备已连接",
        )
        .await?;
    }
    tx.commit().await?;
    Ok(Json(
        json!({"ok":true,"server_time":Utc::now(),"mode":"observe","remote_execution":false}),
    ))
}
async fn event_list(s: &AppState, owner: Option<Uuid>) -> ApiResult<Json<Value>> {
    let rows:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('id',e.id,'kind',e.kind,'detail',e.detail,'created_at',e.created_at,'username',COALESCE(u.email,u.username),'device_name',d.name) FROM events e LEFT JOIN users u ON u.id=e.user_id LEFT JOIN devices d ON d.id=e.device_id WHERE ($1::uuid IS NULL OR e.user_id=$1) ORDER BY e.created_at DESC,e.id DESC LIMIT 100").bind(owner).fetch_all(&s.db).await?;
    Ok(Json(json!({"events":rows,"limit":100})))
}
pub async fn events(State(s): State<AppState>, h: HeaderMap) -> ApiResult<Json<Value>> {
    let u = auth::user(&s, &h, false).await?;
    event_list(&s, Some(u.id)).await
}
pub async fn all_events(State(s): State<AppState>, h: HeaderMap) -> ApiResult<Json<Value>> {
    auth::admin(&s, &h, false).await?;
    event_list(&s, None).await
}
pub async fn dashboard(State(s): State<AppState>, h: HeaderMap) -> ApiResult<Json<Value>> {
    let u = auth::user(&s, &h, false).await?;
    let stats:Value=sqlx::query_scalar("SELECT jsonb_build_object('devices',count(*),'online',count(*) FILTER(WHERE last_seen>now()-interval '45 seconds'),'waiting',count(*) FILTER(WHERE last_seen<=now()-interval '45 seconds' AND last_seen>now()-interval '120 seconds')) FROM devices WHERE user_id=$1 AND NOT revoked").bind(u.id).fetch_one(&s.db).await?;
    Ok(Json(
        json!({"stats":stats,"mode":"observe","remote_execution":false,"updated_at":Utc::now(),"client_integration":"v0.14.0 支持自动绑定、手动配对和设备心跳"}),
    ))
}
pub async fn overview(State(s): State<AppState>, h: HeaderMap) -> ApiResult<Json<Value>> {
    auth::admin(&s, &h, false).await?;
    let stats:Value=sqlx::query_scalar("SELECT jsonb_build_object('users',(SELECT count(*) FROM users WHERE NOT disabled),'disabled_users',(SELECT count(*) FROM users WHERE disabled),'web_users',(SELECT count(DISTINCT s.user_id) FROM sessions s JOIN users u ON u.id=s.user_id WHERE NOT u.disabled AND s.expires_at>now() AND s.last_seen>now()-interval '5 minutes'),'online_users',(SELECT count(DISTINCT user_id) FROM devices WHERE NOT revoked AND last_seen>now()-interval '45 seconds'),'online_devices',(SELECT count(*) FROM devices WHERE NOT revoked AND last_seen>now()-interval '45 seconds'),'devices',(SELECT count(*) FROM devices WHERE NOT revoked),'scheduler_at',(SELECT updated_at FROM service_state WHERE key='scheduler'))").fetch_one(&s.db).await?;
    let metrics:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('time',bucket,'devices',online_devices,'users',online_users,'web_users',web_users) FROM metrics WHERE bucket>now()-interval '24 hours' ORDER BY bucket").fetch_all(&s.db).await?;
    Ok(Json(
        json!({"stats":stats,"metrics":metrics,"mode":"observe","updated_at":Utc::now()}),
    ))
}
pub async fn users(State(s): State<AppState>, h: HeaderMap) -> ApiResult<Json<Value>> {
    auth::admin(&s, &h, false).await?;
    let rows:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('id',u.id,'username',COALESCE(u.email,u.username),'display_name',u.display_name,'role',u.role,'disabled',u.disabled,'created_at',u.created_at,'devices',(SELECT count(*) FROM devices d WHERE d.user_id=u.id AND NOT d.revoked)) FROM users u ORDER BY u.created_at DESC LIMIT 500").fetch_all(&s.db).await?;
    Ok(Json(json!({"users":rows,"limit":500})))
}
#[derive(Deserialize)]
pub struct NewUser {
    username: String,
    display_name: String,
    password: String,
}
pub async fn create_user(
    State(s): State<AppState>,
    h: HeaderMap,
    Json(p): Json<NewUser>,
) -> ApiResult<Json<Value>> {
    let admin = auth::admin(&s, &h, true).await?;
    auth::throttle(&s, format!("create:{}", admin.id), 20, 600)?;
    let username = p.username.trim().to_lowercase();
    if !auth::valid_username(&username)
        || !auth::valid_password(&p.password)
        || p.display_name.trim().is_empty()
        || p.display_name.len() > 100
    {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "账号格式不正确，密码需为 12–128 字节",
        ));
    }
    let permit = s
        .hashes
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError(StatusCode::TOO_MANY_REQUESTS, "操作繁忙"))?;
    let hash = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        auth::hash_password(&p.password)
    })
    .await
    .ok()
    .and_then(Result::ok)
    .ok_or(ApiError(StatusCode::INTERNAL_SERVER_ERROR, "创建失败"))?;
    let id = Uuid::new_v4();
    let mut tx = s.db.begin().await?;
    let inserted=sqlx::query("INSERT INTO users(id,username,display_name,password_hash) VALUES($1,$2,$3,$4) ON CONFLICT(username) DO NOTHING")
        .bind(id).bind(&username).bind(p.display_name.trim()).bind(hash).execute(&mut *tx).await?;
    if inserted.rows_affected() != 1 {
        return Err(ApiError(StatusCode::CONFLICT, "该账号已存在"));
    }
    audit(
        &mut tx,
        admin.id,
        None,
        "user_created",
        &format!("创建用户 {username}"),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"id":id,"ok":true})))
}
#[derive(Deserialize)]
pub struct UserStatus {
    disabled: bool,
}
pub async fn user_status(
    State(s): State<AppState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
    Json(p): Json<UserStatus>,
) -> ApiResult<Json<Value>> {
    let admin = auth::admin(&s, &h, true).await?;
    if id == admin.id {
        return Err(ApiError(StatusCode::BAD_REQUEST, "不能停用当前管理员"));
    }
    let mut tx = s.db.begin().await?;
    let changed = sqlx::query("UPDATE users SET disabled=$1 WHERE id=$2 AND role='user'")
        .bind(p.disabled)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    if changed.rows_affected() != 1 {
        return Err(ApiError(StatusCode::NOT_FOUND, "普通用户不存在"));
    }
    if p.disabled {
        sqlx::query("DELETE FROM sessions WHERE user_id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM pairing_codes WHERE user_id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE devices SET revoked=true WHERE user_id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    audit(
        &mut tx,
        admin.id,
        None,
        "user_status_changed",
        &format!(
            "用户 {id} {}",
            if p.disabled {
                "已停用；会话和设备授权已撤销"
            } else {
                "已启用；设备需重新配对"
            }
        ),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"ok":true})))
}
