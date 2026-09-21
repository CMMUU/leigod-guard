//! Stable installation credentials bind one installation to one platform owner.
use crate::*;
pub fn valid_key(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|c| c.is_ascii_hexdigit())
}
#[derive(Deserialize)]
pub struct Register {
    installation_key: String,
    name: String,
    version: String,
    #[serde(default)]
    reactivate: bool,
}
pub async fn register(
    State(s): State<AppState>,
    h: HeaderMap,
    Json(p): Json<Register>,
) -> ApiResult<Json<Value>> {
    let u = auth::user(&s, &h, true).await?;
    auth::throttle(&s, format!("register:{}", u.id), 20, 600)?;
    let mut tx = s.db.begin().await?;
    let response = bind(
        &mut tx,
        u.id,
        Some(&p.installation_key),
        &p.name,
        &p.version,
        p.reactivate,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(response))
}
pub async fn bind(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    uid: Uuid,
    installation: Option<&str>,
    name: &str,
    version: &str,
    reactivate: bool,
) -> ApiResult<Value> {
    if name.trim().is_empty()
        || name.len() > 100
        || name.chars().any(char::is_control)
        || version.len() > 32
        || installation.is_some_and(|key| !valid_key(key))
    {
        return Err(ApiError(StatusCode::BAD_REQUEST, "设备参数无效"));
    }
    let owner = sqlx::query(
        "SELECT disabled,COALESCE(email,username) AS username FROM users WHERE id=$1 FOR UPDATE",
    )
    .bind(uid)
    .fetch_one(&mut **tx)
    .await?;
    if owner.get::<bool, _>("disabled") {
        return Err(ApiError(StatusCode::FORBIDDEN, "账号已停用"));
    }
    let token = auth::secret();
    let hash = installation
        .map(auth::digest)
        .unwrap_or_else(|| auth::digest(&token));
    if installation.is_some() {
        // Serialize the global installation identity even before its first row exists.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(&hash)
            .execute(&mut **tx)
            .await?;
        if let Some(existing) = sqlx::query(
            "SELECT id,user_id,revoked,sequence FROM devices WHERE installation_hash=$1 FOR UPDATE",
        )
        .bind(&hash)
        .fetch_optional(&mut **tx)
        .await?
        {
            if existing.get::<Uuid, _>("user_id") != uid {
                return Err(ApiError(
                    StatusCode::CONFLICT,
                    "本机已属于其他平台账号，请先在原账号解除设备",
                ));
            }
            let revoked = existing.get::<bool, _>("revoked");
            if revoked && !reactivate {
                return Err(ApiError(StatusCode::CONFLICT, "设备已撤销，请手动重新绑定"));
            }
            if revoked {
                enforce_limit(tx, uid).await?;
            }
            let id: Uuid = existing.get("id");
            sqlx::query(
                "UPDATE devices SET name=$2,version=$3,revoked=false,token_hash=$4,run_generation=0 WHERE id=$1",
            )
            .bind(id)
            .bind(name.trim())
            .bind(version)
            .bind(auth::digest(&token))
            .execute(&mut **tx)
            .await?;
            if revoked {
                routes::audit(tx, uid, Some(id), "device_rebound", "用户手动重新授权设备").await?;
            }
            return Ok(
                json!({"device_id":id,"device_token":token,"owner_id":uid,"owner":owner.get::<String,_>("username"),"sequence":existing.get::<i64,_>("sequence"),"heartbeat_seconds":15,"mode":"observe"}),
            );
        }
    }
    enforce_limit(tx, uid).await?;
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO devices(id,user_id,name,token_hash,installation_hash,version) VALUES($1,$2,$3,$4,$5,$6)")
        .bind(id).bind(uid).bind(name.trim()).bind(auth::digest(&token)).bind(installation.map(|_|hash.clone())).bind(version).execute(&mut **tx).await?;
    routes::audit(
        tx,
        uid,
        Some(id),
        "device_paired",
        "设备已绑定，等待首次心跳",
    )
    .await?;
    Ok(
        json!({"device_id":id,"device_token":token,"owner_id":uid,"owner":owner.get::<String,_>("username"),"sequence":-1,"heartbeat_seconds":15,"mode":"observe"}),
    )
}
async fn enforce_limit(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, uid: Uuid) -> ApiResult<()> {
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM devices WHERE user_id=$1 AND NOT revoked")
            .bind(uid)
            .fetch_one(&mut **tx)
            .await?;
    if count >= 50 {
        return Err(ApiError(StatusCode::FORBIDDEN, "已达到 50 台设备数量限制"));
    }
    Ok(())
}
// Removing a revoked registration is an explicit ownership action, unlike passive logout.
pub async fn forget(
    State(s): State<AppState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    let u = auth::user(&s, &h, true).await?;
    let mut tx = s.db.begin().await?;
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(u.id)
        .fetch_one(&mut *tx)
        .await?;
    let changed=sqlx::query("UPDATE devices SET revoked=true,installation_hash=NULL,token_hash=$3 WHERE id=$1 AND user_id=$2")
        .bind(id).bind(u.id).bind(auth::digest(&auth::secret())).execute(&mut *tx).await?;
    if changed.rows_affected() != 1 {
        return Err(ApiError(StatusCode::NOT_FOUND, "设备不存在"));
    }
    routes::audit(
        &mut tx,
        u.id,
        Some(id),
        "device_unbound",
        "解除设备绑定，旧凭据已失效",
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"ok":true})))
}
