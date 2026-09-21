//! Durable device consent and account-level protection. Secrets never enter API views.
use crate::*;
use sqlx::{Postgres, Transaction};
pub async fn lock(tx: &mut Transaction<'_, Postgres>) -> ApiResult<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(73940128)")
        .execute(&mut **tx)
        .await?;
    Ok(())
}
async fn device(s: &AppState, h: &HeaderMap) -> ApiResult<(Uuid, Uuid)> {
    let token = h
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .filter(|v| devices::valid_key(v))
        .ok_or(ApiError(StatusCode::UNAUTHORIZED, "设备凭据无效"))?;
    let r=sqlx::query("SELECT d.id,d.user_id FROM devices d JOIN users u ON u.id=d.user_id WHERE d.token_hash=$1 AND NOT d.revoked AND NOT u.disabled")
        .bind(auth::digest(token)).fetch_optional(&s.db).await?.ok_or(ApiError(StatusCode::UNAUTHORIZED,"设备授权已失效"))?;
    Ok((r.get("id"), r.get("user_id")))
}
pub async fn status(State(s): State<AppState>, h: HeaderMap) -> ApiResult<Json<Value>> {
    let (id, _) = device(&s, &h).await?;
    Ok(Json(view(&s, id).await?))
}
pub async fn service(s: &AppState) -> ApiResult<Value> {
    let r=sqlx::query("SELECT blocked,reason,ingress_ok,warmup_until,last_tick,now()<warmup_until AS warming,now()-last_tick>interval '20 seconds' AS stale FROM remote_service WHERE singleton").fetch_one(&s.db).await?;
    let state = if s.provider.is_none() {
        "disabled"
    } else if r.get::<bool, _>("blocked") {
        "blocked"
    } else if !r.get::<bool, _>("ingress_ok") || r.get::<bool, _>("stale") {
        "unhealthy"
    } else if r.get::<bool, _>("warming") {
        "warming"
    } else {
        "ready"
    };
    Ok(
        json!({"state":state,"reason":r.get::<String,_>("reason"),"warmup_until":r.get::<DateTime<Utc>,_>("warmup_until"),"last_tick":r.get::<DateTime<Utc>,_>("last_tick")}),
    )
}
pub async fn view(s: &AppState, id: Uuid) -> ApiResult<Value> {
    let service = service(s).await?;
    let row=sqlx::query("SELECT g.revision,g.enabled,g.armed_at,g.last_seen,g.run_generation,a.provider_key,a.label,a.credential_state,j.state AS job_state,j.result,j.updated_at AS result_at,(j.epoch=a.epoch) AS current_job FROM remote_grants g JOIN remote_accounts a ON a.id=g.account_id LEFT JOIN LATERAL(SELECT * FROM remote_jobs WHERE account_id=a.id ORDER BY created_at DESC LIMIT 1) j ON true WHERE g.device_id=$1").bind(id).fetch_optional(&s.db).await?;
    let Some(r) = row else {
        let revision: i64 = sqlx::query_scalar("SELECT remote_revision FROM devices WHERE id=$1")
            .bind(id)
            .fetch_one(&s.db)
            .await?;
        return Ok(
            json!({"available":s.provider.is_some(),"enabled":false,"revision":revision,"protection":"off","credential":"none","last_result":"none","service":service}),
        );
    };
    let enabled = r.get::<bool, _>("enabled");
    let credential: String = r.get("credential_state");
    let armed: Option<DateTime<Utc>> = r.get("armed_at");
    let seen: Option<DateTime<Utc>> = r.get("last_seen");
    let job: Option<String> = r.get("job_state");
    let protection = if !enabled {
        "off"
    } else if credential != "valid" {
        "reauthorize"
    } else if service["state"] != "ready" {
        "service_abnormal"
    } else if armed.is_none() {
        "awaiting_heartbeat"
    } else if r.get::<Option<bool>, _>("current_job") == Some(true) {
        match job.as_deref() {
            Some("running") => "executing",
            Some("confirmed") => "confirmed",
            Some("unconfirmed") => "unconfirmed",
            _ => "waiting",
        }
    } else if seen.is_none_or(|t| Utc::now() - t > chrono::Duration::seconds(45)) {
        "waiting"
    } else {
        "armed"
    };
    Ok(
        json!({"available":s.provider.is_some(),"enabled":enabled,"revision":r.get::<i64,_>("revision"),"run_generation":r.get::<i64,_>("run_generation"),"label":r.get::<String,_>("label"),"account_key":r.get::<String,_>("provider_key"),"credential":credential,"protection":protection,"last_result":r.get::<Option<String>,_>("result").unwrap_or("none".into()),"last_result_at":r.get::<Option<DateTime<Utc>>,_>("result_at"),"service":service}),
    )
}
#[derive(Deserialize)]
pub struct Authorize {
    account_token: String,
    revision: i64,
    run_generation: i64,
    #[serde(default)]
    refresh: bool,
}
pub async fn authorize(
    State(s): State<AppState>,
    h: HeaderMap,
    Json(p): Json<Authorize>,
) -> ApiResult<Json<Value>> {
    let (id, uid) = device(&s, &h).await?;
    auth::throttle(&s, format!("remote-authorize:{uid}"), 10, 60)?;
    if p.account_token.is_empty()
        || p.account_token.len() > 4096
        || p.account_token.chars().any(char::is_control)
        || p.revision < 0
        || p.revision == i64::MAX
        || p.run_generation <= 0
    {
        return Err(ApiError(StatusCode::BAD_REQUEST, "远程授权参数无效"));
    }
    let provider = s.provider.as_ref().ok_or(ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "服务器远程暂停能力尚未开放",
    ))?;
    let _permit = s
        .remote_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError(StatusCode::TOO_MANY_REQUESTS, "授权服务繁忙，请稍后重试"))?;
    let info = provider.info(&p.account_token).await.map_err(|e| match e {
        provider::Failure::Credential => ApiError(
            StatusCode::BAD_REQUEST,
            "雷神登录已失效，请在客户端重新登录",
        ),
        provider::Failure::InvalidAccount => ApiError(
            StatusCode::BAD_REQUEST,
            "无法验证雷神账号唯一身份，未开启保护",
        ),
        _ => ApiError(StatusCode::BAD_GATEWAY, "雷神账号验证失败，请稍后重试"),
    })?;
    let cipher = provider
        .seal(&info.key, &p.account_token)
        .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "凭据保存失败"))?;
    let mut tx = s.db.begin().await?;
    // Revalidate after the external call; concurrent logout/rebind wins.
    let d=sqlx::query("SELECT d.run_generation,d.remote_revision FROM devices d JOIN users u ON u.id=d.user_id WHERE d.id=$1 AND d.user_id=$2 AND d.token_hash=$3 AND NOT d.revoked AND NOT u.disabled FOR UPDATE OF d")
        .bind(id).bind(uid).bind(auth::digest(h.get("authorization").unwrap().to_str().unwrap().strip_prefix("Bearer ").unwrap())).fetch_optional(&mut *tx).await?.ok_or(ApiError(StatusCode::UNAUTHORIZED,"设备授权已失效"))?;
    if d.get::<i64, _>("run_generation") != p.run_generation
        || d.get::<i64, _>("remote_revision") != p.revision
    {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "客户端运行代次已变化，请重新连接",
        ));
    }
    lock(&mut tx).await?;
    let old =
        sqlx::query("SELECT revision,account_id,enabled FROM remote_grants WHERE device_id=$1")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    if p.refresh && old.as_ref().is_none_or(|r| !r.get::<bool, _>("enabled")) {
        return Err(ApiError(StatusCode::CONFLICT, "远程授权已变化，请重新确认"));
    }
    let account = sqlx::query("SELECT id,user_id FROM remote_accounts WHERE provider_key=$1")
        .bind(&info.key)
        .fetch_optional(&mut *tx)
        .await?;
    let aid = if let Some(account) = account {
        if account.get::<Uuid, _>("user_id") != uid {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "该雷神账号已属于其他平台账号",
            ));
        }
        account.get::<Uuid, _>("id")
    } else {
        let a = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO remote_accounts(id,user_id,provider_key,label) VALUES($1,$2,$3,$4)",
        )
        .bind(a)
        .bind(uid)
        .bind(&info.key)
        .bind(&info.label)
        .execute(&mut *tx)
        .await?;
        a
    };
    if let Some(ref old) = old {
        if p.refresh && old.get::<Uuid, _>("account_id") != aid {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "雷神账号已切换，请关闭旧保护后重新开启",
            ));
        }
        sqlx::query("SELECT remote_revoke_device($1)")
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    let revision = p.revision + 1;
    sqlx::query("UPDATE devices SET remote_revision=$2 WHERE id=$1")
        .bind(id)
        .bind(revision)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO remote_grants(device_id,account_id,revision,run_generation) VALUES($1,$2,$3,$4) ON CONFLICT(device_id) DO UPDATE SET account_id=$2,revision=$3,run_generation=$4,enabled=true,armed_at=NULL,last_seen=NULL,prepare_until=NULL,updated_at=now()")
        .bind(id).bind(aid).bind(revision).bind(p.run_generation).execute(&mut *tx).await?;
    sqlx::query("SELECT remote_cancel($1)")
        .bind(aid)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE remote_accounts SET credential=$2,credential_state='valid',credential_version=credential_version+1,label=$3,updated_at=now() WHERE id=$1").bind(aid).bind(cipher).bind(info.label).execute(&mut *tx).await?;
    routes::audit(
        &mut tx,
        uid,
        Some(id),
        "remote_authorized",
        "用户授权上海服务器保存加密雷神凭据并执行失联暂停；等待首次心跳",
    )
    .await?;
    tx.commit().await?;
    Ok(Json(view(&s, id).await?))
}
#[derive(Deserialize)]
pub struct Disable {
    revision: i64,
}
pub async fn disable(
    State(s): State<AppState>,
    h: HeaderMap,
    Json(p): Json<Disable>,
) -> ApiResult<Json<Value>> {
    let (id, uid) = device(&s, &h).await?;
    revoke(&s, id, uid, Some(p.revision)).await?;
    Ok(Json(view(&s, id).await?))
}
pub async fn web_disable(
    State(s): State<AppState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    let u = auth::user(&s, &h, true).await?;
    revoke(&s, id, u.id, None).await?;
    Ok(Json(json!({"ok":true})))
}
async fn revoke(s: &AppState, id: Uuid, uid: Uuid, revision: Option<i64>) -> ApiResult<()> {
    let mut tx = s.db.begin().await?;
    sqlx::query("SELECT id FROM devices WHERE id=$1 AND user_id=$2 FOR UPDATE")
        .bind(id)
        .bind(uid)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ApiError(StatusCode::NOT_FOUND, "设备不存在"))?;
    lock(&mut tx).await?;
    let old = sqlx::query("SELECT revision,enabled FROM remote_grants WHERE device_id=$1")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
    if let Some(old) = old {
        if old.get::<bool, _>("enabled")
            && revision.is_some_and(|v| v != old.get::<i64, _>("revision"))
        {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "远程授权已变化，请刷新后重试",
            ));
        }
    }
    sqlx::query("SELECT remote_revoke_device($1)")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    routes::audit(
        &mut tx,
        uid,
        Some(id),
        "remote_disabled",
        "已关闭本机远程保护；未发送的旧任务已取消，已发出的请求无法撤回",
    )
    .await?;
    tx.commit().await?;
    Ok(())
}
// Called within the device heartbeat transaction, after locking the device row.
pub async fn heartbeat(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    run: i64,
    revision: Option<i64>,
    clear: bool,
    prepare: u32,
) -> ApiResult<()> {
    lock(tx).await?;
    let grant =
        sqlx::query("SELECT account_id,revision,enabled FROM remote_grants WHERE device_id=$1")
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?;
    if let Some(g) = grant {
        if g.get::<bool, _>("enabled") {
            if revision != Some(g.get::<i64, _>("revision")) || run <= 0 {
                return Err(ApiError(
                    StatusCode::CONFLICT,
                    "远程授权版本已变化，请刷新状态",
                ));
            }
            if clear {
                sqlx::query("SELECT remote_revoke_device($1)")
                    .bind(id)
                    .execute(&mut **tx)
                    .await?;
            } else {
                sqlx::query("UPDATE remote_grants SET armed_at=COALESCE(armed_at,now()),last_seen=now(),prepare_until=now()+make_interval(secs=>$2),run_generation=$3,updated_at=now() WHERE device_id=$1")
                    .bind(id).bind(prepare as f64).bind(run).execute(&mut **tx).await?;
                sqlx::query("SELECT remote_cancel($1)")
                    .bind(g.get::<Uuid, _>("account_id"))
                    .execute(&mut **tx)
                    .await?;
            }
        }
    }
    Ok(())
}
pub async fn listing(State(s): State<AppState>, h: HeaderMap) -> ApiResult<Json<Value>> {
    let u = auth::user(&s, &h, false).await?;
    let rows:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('device_id',d.id,'device_name',d.name,'owner',COALESCE(u.email,u.username),'user_id',d.user_id,'label',a.label,'enabled',g.enabled,'credential',a.credential_state,'armed_at',g.armed_at,'last_seen',g.last_seen,'revision',g.revision,'connection',CASE WHEN d.revoked THEN 'revoked' WHEN g.last_seen>now()-interval '45 seconds' THEN 'online' WHEN g.last_seen>now()-interval '120 seconds' THEN 'waiting' ELSE 'offline' END,'last_result',j.result,'protection',CASE WHEN NOT g.enabled THEN 'off' WHEN a.credential_state<>'valid' THEN 'reauthorize' WHEN g.armed_at IS NULL THEN 'awaiting_heartbeat' WHEN j.epoch=a.epoch AND j.state='running' THEN 'executing' WHEN j.epoch=a.epoch AND j.state='confirmed' THEN 'confirmed' WHEN j.epoch=a.epoch AND j.state='unconfirmed' THEN 'unconfirmed' WHEN g.last_seen<now()-interval '45 seconds' THEN 'waiting' ELSE 'armed' END) FROM remote_grants g JOIN devices d ON d.id=g.device_id JOIN users u ON u.id=d.user_id JOIN remote_accounts a ON a.id=g.account_id LEFT JOIN LATERAL(SELECT epoch,state,result FROM remote_jobs WHERE account_id=a.id ORDER BY created_at DESC LIMIT 1) j ON true WHERE ($1 OR d.user_id=$2) ORDER BY g.updated_at DESC LIMIT 500").bind(u.role=="admin").bind(u.id).fetch_all(&s.db).await?;
    let jobs:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('id',j.id,'label',a.label,'state',j.state,'result',j.result,'attempts',j.attempts,'created_at',j.created_at,'updated_at',j.updated_at) FROM remote_jobs j JOIN remote_accounts a ON a.id=j.account_id WHERE ($1 OR a.user_id=$2) ORDER BY j.created_at DESC LIMIT 100").bind(u.role=="admin").bind(u.id).fetch_all(&s.db).await?;
    Ok(Json(
        json!({"grants":rows,"jobs":jobs,"service":service(&s).await?}),
    ))
}
pub async fn acknowledge(State(s): State<AppState>, h: HeaderMap) -> ApiResult<Json<Value>> {
    let u = auth::admin(&s, &h, true).await?;
    let mut tx = s.db.begin().await?;
    lock(&mut tx).await?;
    // Acknowledgement discards old offline episodes, never releases a stale batch.
    sqlx::query("UPDATE remote_jobs SET state='cancelled',result='incident_acknowledged',updated_at=now() WHERE state IN ('queued','running')").execute(&mut *tx).await?;
    sqlx::query("UPDATE remote_grants SET armed_at=NULL WHERE enabled")
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE remote_service SET blocked=false,reason='reconnect',warmup_until=now()+interval '120 seconds' WHERE singleton").execute(&mut *tx).await?;
    routes::audit(
        &mut tx,
        u.id,
        None,
        "remote_incident_acknowledged",
        "管理员确认批量失联；所有设备需要新心跳，观察 120 秒后恢复",
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"ok":true})))
}
