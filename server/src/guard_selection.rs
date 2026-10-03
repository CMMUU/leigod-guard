//! Device-scoped, revisioned selection. Never authorizes or uploads credentials.
use crate::provider::Kind;
use crate::*;

#[derive(Deserialize)]
pub struct Change {
    provider: Kind,
    revision: i64,
    run_generation: i64,
}

pub async fn status(State(s): State<AppState>, h: HeaderMap) -> ApiResult<Json<Value>> {
    let (id, _) = remote::device(&s, &h).await?;
    let r = sqlx::query("SELECT guard_provider,guard_revision FROM devices WHERE id=?")
        .bind(id)
        .fetch_one(&s.db)
        .await?;
    Ok(Json(
        json!({"provider":r.get::<Option<String>,_>("guard_provider"),"revision":r.get::<i64,_>("guard_revision"),"committed":true,"reason":""}),
    ))
}

pub async fn change(
    State(s): State<AppState>,
    h: HeaderMap,
    Json(p): Json<Change>,
) -> ApiResult<Json<Value>> {
    let (id, uid) = remote::device(&s, &h).await?;
    if p.revision < 0 || p.revision == i64::MAX || p.run_generation <= 0 {
        return Err(ApiError(StatusCode::BAD_REQUEST, "守护选择参数无效"));
    }
    let mut tx = storage::begin(&s.db).await?;
    // Keep the same device -> scheduler lock order as authorize/heartbeat.
    let d = sqlx::query("SELECT d.guard_provider,d.guard_revision,d.run_generation FROM devices d JOIN users u ON u.id=d.user_id WHERE d.id=? AND d.token_hash=? AND NOT d.revoked AND NOT u.disabled FOR UPDATE").bind(id).bind(auth::digest(h.get("authorization").unwrap().to_str().unwrap().strip_prefix("Bearer ").unwrap()))
        .fetch_optional(&mut *tx).await?.ok_or(ApiError(StatusCode::UNAUTHORIZED,"设备授权已失效"))?;
    let revision: i64 = d.get("guard_revision");
    if revision != p.revision || p.run_generation < d.get::<i64, _>("run_generation") {
        return Err(ApiError(StatusCode::CONFLICT, "守护选择或运行代次已变化"));
    }
    remote::lock(&mut tx).await?;
    let other = if p.provider == Kind::Leigod {
        Kind::Etalien
    } else {
        Kind::Leigod
    };
    // Cafe mode belongs to an account, possibly shared by multiple PCs. A
    // device selection must not silently disable it. The web owner can close it.
    let cafe: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM remote_grants g JOIN cafe_policies c ON c.account_id=g.account_id WHERE g.device_id=? AND g.provider=? AND c.enabled)").bind(id).bind(other.as_str()).fetch_one(&mut *tx).await?;
    let busy: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM remote_grants g JOIN remote_jobs j ON j.account_id=g.account_id WHERE g.device_id=? AND j.lease_until>UTC_TIMESTAMP(6))").bind(id).fetch_one(&mut *tx).await?;
    if cafe || busy {
        return Ok(Json(
            json!({"provider":d.get::<Option<String>,_>("guard_provider"),"revision":revision,"committed":false,"reason":if cafe {"cafe_mode"} else {"in_flight"}}),
        ));
    }
    let changed =
        d.get::<Option<String>, _>("guard_provider").as_deref() != Some(p.provider.as_str());
    // Only this device's old grant is revoked; other devices keep their consent.
    // Cancel queued jobs even if the old grant was already disabled.
    if changed {
        storage::revoke_provider(&mut tx, id, other).await?;
    }
    if changed || p.run_generation > d.get::<i64, _>("run_generation") {
        // A selected provider may retain a legacy dual-provider grant. This
        // authenticated, revisioned connection ends its old offline episode.
        // Keep the existing armed/unarmed state: a previously protected device
        // must still time out if it crashes before the next heartbeat, while a
        // never-confirmed grant must still await its first heartbeat.
        // Independent cafe deadlines are not reset by a device selection.
        storage::cancel_device_account(&mut tx, id, p.provider).await?;
        sqlx::query("UPDATE remote_grants SET last_seen=UTC_TIMESTAMP(6),prepare_until=UTC_TIMESTAMP(6),run_generation=?,updated_at=UTC_TIMESTAMP(6) WHERE device_id=? AND provider=? AND enabled").bind(p.run_generation).bind(id).bind(p.provider.as_str()).execute(&mut *tx).await?;
    }
    let peers: i64 = sqlx::query_scalar("SELECT count(DISTINCT g.device_id) FROM remote_grants g WHERE g.enabled AND g.device_id<>? AND g.provider=? AND g.account_id IN (SELECT account_id FROM remote_grants WHERE device_id=? AND provider=?)").bind(id).bind(other.as_str()).bind(id).bind(other.as_str()).fetch_one(&mut *tx).await?;
    let revision = revision + i64::from(changed);
    sqlx::query("UPDATE devices SET guard_provider=?,guard_revision=?,run_generation=?,game_running=CASE WHEN ? THEN NULL ELSE game_running END,prepare_until=CASE WHEN ? THEN UTC_TIMESTAMP(6) ELSE prepare_until END WHERE id=?").bind(p.provider.as_str()).bind(revision).bind(p.run_generation).bind(changed).bind(changed).bind(id).execute(&mut *tx).await?;
    if changed {
        routes::audit(
            &mut tx,
            uid,
            Some(id),
            "guard_selection_changed",
            if p.provider == Kind::Leigod {
                "本机守护对象切换为雷神；已撤销本机外星仔保护"
            } else {
                "本机守护对象切换为外星仔；已撤销本机雷神保护"
            },
        )
        .await?;
    }
    tx.commit().await?;
    Ok(Json(
        json!({"provider":p.provider.as_str(),"revision":revision,"committed":true,"reason":"","other_devices":peers}),
    ))
}
