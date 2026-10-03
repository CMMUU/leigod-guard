//! Durable device consent and account-level protection. Secrets never enter API views.
use crate::provider::Kind;
use crate::*;
use axum::extract::Query;
use sqlx::{MySql, Transaction};
#[derive(Default, Deserialize)]
pub struct Selection {
    #[serde(default)]
    provider: Kind,
}
pub async fn lock(tx: &mut Transaction<'_, MySql>) -> ApiResult<()> {
    storage::lock(tx).await
}
pub(crate) async fn device(s: &AppState, h: &HeaderMap) -> ApiResult<(Uuid, Uuid)> {
    let token = h
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .filter(|v| devices::valid_key(v))
        .ok_or(ApiError(StatusCode::UNAUTHORIZED, "设备凭据无效"))?;
    let r=sqlx::query("SELECT d.id,d.user_id FROM devices d JOIN users u ON u.id=d.user_id WHERE d.token_hash=? AND NOT d.revoked AND NOT u.disabled").bind(auth::digest(token)).fetch_optional(&s.db).await?.ok_or(ApiError(StatusCode::UNAUTHORIZED,"设备授权已失效"))?;
    Ok((r.get("id"), r.get("user_id")))
}
pub async fn status(
    State(s): State<AppState>,
    Query(q): Query<Selection>,
    h: HeaderMap,
) -> ApiResult<Json<Value>> {
    let (id, _) = device(&s, &h).await?;
    Ok(Json(view(&s, id, q.provider).await?))
}
pub async fn service(s: &AppState) -> ApiResult<Value> {
    let r=sqlx::query("SELECT blocked,reason,ingress_ok,warmup_until,last_tick,UTC_TIMESTAMP(6)<warmup_until AS warming,last_tick<UTC_TIMESTAMP(6)-INTERVAL 20 SECOND AS stale FROM remote_service WHERE singleton").fetch_one(&s.db).await?;
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
        json!({"state":state,"stage":s.stage,"reason":r.get::<String,_>("reason"),"warmup_until":r.get::<DateTime<Utc>,_>("warmup_until"),"last_tick":r.get::<DateTime<Utc>,_>("last_tick")}),
    )
}
pub async fn view(s: &AppState, id: Uuid, kind: Kind) -> ApiResult<Value> {
    let service = service(s).await?;
    let row=sqlx::query("SELECT g.revision,g.enabled,g.armed_at,g.last_seen,g.run_generation,a.provider_key,a.label,a.credential_state,j.state AS job_state,j.result,j.updated_at AS result_at,(j.epoch=a.epoch) AS current_job FROM remote_grants g JOIN remote_accounts a ON a.id=g.account_id LEFT JOIN remote_jobs j ON j.id=(SELECT newest.id FROM remote_jobs newest WHERE newest.account_id=a.id ORDER BY newest.created_at DESC,newest.id DESC LIMIT 1) WHERE g.device_id=? AND g.provider=?").bind(id).bind(kind.as_str()).fetch_optional(&s.db).await?;
    let Some(r) = row else {
        let revision: i64 = sqlx::query_scalar(&format!(
            "SELECT {} FROM devices WHERE id=?",
            kind.revision_column()
        ))
        .bind(id)
        .fetch_one(&s.db)
        .await?;
        return Ok(
            json!({"provider":kind.as_str(),"available":s.provider.is_some(),"enabled":false,"revision":revision,"protection":"off","credential":"none","last_result":"none","service":service}),
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
        json!({"provider":kind.as_str(),"available":s.provider.is_some(),"enabled":enabled,"revision":r.get::<i64,_>("revision"),"run_generation":r.get::<i64,_>("run_generation"),"label":r.get::<String,_>("label"),"account_key":r.get::<String,_>("provider_key"),"credential":credential,"protection":protection,"last_result":r.get::<Option<String>,_>("result").unwrap_or("none".into()),"last_result_at":r.get::<Option<DateTime<Utc>>,_>("result_at"),"service":service}),
    )
}
#[derive(Deserialize)]
pub struct Authorize {
    account_token: String,
    revision: i64,
    run_generation: i64,
    #[serde(default)]
    refresh: bool,
    #[serde(default)]
    device_id: String,
    #[serde(default)]
    paused_state: i64,
}
pub async fn authorize(
    State(s): State<AppState>,
    Query(q): Query<Selection>,
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
    let credential = if q.provider == Kind::Etalien {
        let value = serde_json::to_string(&crate::etalien::Credential {
            token: p.account_token.clone(),
            device_id: p.device_id.clone(),
            paused_state: p.paused_state,
        })
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "外星仔凭据无效"))?;
        crate::etalien::Credential::parse(&value)
            .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "请先登录并校准外星仔暂停状态"))?;
        value
    } else {
        p.account_token.clone()
    };
    let info = provider
        .info(q.provider, &credential)
        .await
        .map_err(|e| match e {
            provider::Failure::Credential => ApiError(
                StatusCode::BAD_REQUEST,
                if q.provider == Kind::Leigod {
                    "雷神登录已失效，请在客户端重新登录"
                } else {
                    "加速器登录已失效，请在客户端重新登录"
                },
            ),
            provider::Failure::InvalidAccount => ApiError(
                StatusCode::BAD_REQUEST,
                if q.provider == Kind::Leigod {
                    "无法验证雷神账号唯一身份，未开启保护"
                } else {
                    "无法验证加速器账号唯一身份，未开启保护"
                },
            ),
            _ => ApiError(StatusCode::BAD_GATEWAY, "加速器账号验证失败，请稍后重试"),
        })?;
    let cipher = provider
        .seal(&info.key, &credential)
        .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "凭据保存失败"))?;
    let mut tx = storage::begin(&s.db).await?;
    // Revalidate after the external call; concurrent logout/rebind wins.
    let d=sqlx::query(&format!("SELECT d.guard_provider,d.run_generation,d.{} AS remote_revision FROM devices d JOIN users u ON u.id=d.user_id WHERE d.id=? AND d.user_id=? AND d.token_hash=? AND NOT d.revoked AND NOT u.disabled FOR UPDATE", q.provider.revision_column())).bind(id).bind(uid).bind(auth::digest(h.get("authorization").unwrap().to_str().unwrap().strip_prefix("Bearer ").unwrap())).fetch_optional(&mut *tx).await?.ok_or(ApiError(StatusCode::UNAUTHORIZED,"设备授权已失效"))?;
    if d.get::<Option<String>, _>("guard_provider")
        .is_some_and(|selected| selected != q.provider.as_str())
    {
        return Err(ApiError(StatusCode::CONFLICT, "只能授权当前选择的加速器"));
    }
    if d.get::<i64, _>("run_generation") != p.run_generation
        || d.get::<i64, _>("remote_revision") != p.revision
    {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "客户端运行代次已变化，请重新连接",
        ));
    }
    lock(&mut tx).await?;
    let old = sqlx::query(
        "SELECT revision,account_id,enabled FROM remote_grants WHERE device_id=? AND provider=?",
    )
    .bind(id)
    .bind(q.provider.as_str())
    .fetch_optional(&mut *tx)
    .await?;
    if p.refresh && old.as_ref().is_none_or(|r| !r.get::<bool, _>("enabled")) {
        return Err(ApiError(StatusCode::CONFLICT, "远程授权已变化，请重新确认"));
    }
    let account = sqlx::query(
        "SELECT id,user_id,credential FROM remote_accounts WHERE provider_key=? AND provider=?",
    )
    .bind(&info.key)
    .bind(q.provider.as_str())
    .fetch_optional(&mut *tx)
    .await?;
    // The first ETAlien grant must observe the user-calibrated paused state.
    // Refresh may run while timing, but cannot replace the validated state with a guessed enum.
    if q.provider == Kind::Etalien && info.paused != Some(true) {
        let previous = account
            .as_ref()
            .and_then(|r| r.get::<Option<Vec<u8>>, _>("credential"));
        let unchanged = p.refresh
            && previous
                .as_deref()
                .and_then(|b| provider.open(&info.key, b).ok())
                .and_then(|v| crate::etalien::Credential::parse(&v).ok())
                .is_some_and(|c| c.paused_state == p.paused_state);
        if !unchanged || info.paused != Some(false) {
            return Err(ApiError(
                StatusCode::BAD_REQUEST,
                "请先在外星仔官方客户端暂停并校准，再授权服务器保护",
            ));
        }
    }
    let aid = if let Some(account) = account {
        if account.get::<Uuid, _>("user_id") != uid {
            return Err(ApiError(
                StatusCode::CONFLICT,
                if q.provider == Kind::Leigod {
                    "该雷神账号已属于其他平台账号"
                } else {
                    "该加速器账号已属于其他平台账号"
                },
            ));
        }
        account.get::<Uuid, _>("id")
    } else {
        let a = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO remote_accounts(id,user_id,provider_key,label,provider) VALUES(?,?,?,?,?)",
        )
        .bind(a)
        .bind(uid)
        .bind(&info.key)
        .bind(&info.label)
        .bind(q.provider.as_str())
        .execute(&mut *tx)
        .await?;
        a
    };
    if let Some(ref old) = old {
        if p.refresh && old.get::<Uuid, _>("account_id") != aid {
            return Err(ApiError(
                StatusCode::CONFLICT,
                if q.provider == Kind::Leigod {
                    "雷神账号已切换，请关闭旧保护后重新开启"
                } else {
                    "加速器账号已切换，请关闭旧保护后重新开启"
                },
            ));
        }
        storage::revoke_provider(&mut tx, id, q.provider).await?;
    }
    let revision = p.revision + 1;
    sqlx::query(&format!(
        "UPDATE devices SET {}=? WHERE id=?",
        q.provider.revision_column()
    ))
    .bind(revision)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO remote_grants(device_id,account_id,revision,run_generation,provider) VALUES(?,?,?,?,?) ON DUPLICATE KEY UPDATE account_id=?,revision=?,run_generation=?,enabled=true,armed_at=NULL,last_seen=NULL,prepare_until=NULL,updated_at=UTC_TIMESTAMP(6)").bind(id).bind(aid).bind(revision).bind(p.run_generation).bind(q.provider.as_str()).bind(aid).bind(revision).bind(p.run_generation).execute(&mut *tx).await?;
    storage::cancel(&mut tx, aid).await?;
    sqlx::query("UPDATE remote_accounts SET credential=?,credential_state='valid',credential_version=credential_version+1,label=?,updated_at=UTC_TIMESTAMP(6) WHERE id=?").bind(&(cipher)).bind(&(info.label)).bind(aid).execute(&mut *tx).await?;
    routes::audit(
        &mut tx,
        uid,
        Some(id),
        "remote_authorized",
        "用户授权服务器保存所选加速器的加密凭据并执行失联暂停；等待首次心跳",
    )
    .await?;
    tx.commit().await?;
    Ok(Json(view(&s, id, q.provider).await?))
}
#[derive(Deserialize)]
pub struct Disable {
    revision: i64,
}
pub async fn disable(
    State(s): State<AppState>,
    Query(q): Query<Selection>,
    h: HeaderMap,
    Json(p): Json<Disable>,
) -> ApiResult<Json<Value>> {
    let (id, uid) = device(&s, &h).await?;
    revoke(&s, id, uid, Some(p.revision), q.provider).await?;
    Ok(Json(view(&s, id, q.provider).await?))
}
pub async fn web_disable(
    State(s): State<AppState>,
    Query(q): Query<Selection>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    let u = auth::user(&s, &h, true).await?;
    revoke(&s, id, u.id, None, q.provider).await?;
    Ok(Json(json!({"ok":true})))
}
async fn revoke(
    s: &AppState,
    id: Uuid,
    uid: Uuid,
    revision: Option<i64>,
    kind: Kind,
) -> ApiResult<()> {
    let mut tx = storage::begin(&s.db).await?;
    sqlx::query("SELECT id FROM devices WHERE id=? AND user_id=? FOR UPDATE")
        .bind(id)
        .bind(uid)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ApiError(StatusCode::NOT_FOUND, "设备不存在"))?;
    lock(&mut tx).await?;
    let old =
        sqlx::query("SELECT revision,enabled FROM remote_grants WHERE device_id=? AND provider=?")
            .bind(id)
            .bind(kind.as_str())
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
    storage::revoke_provider(&mut tx, id, kind).await?;
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
    tx: &mut Transaction<'_, MySql>,
    id: Uuid,
    run: i64,
    revision: Option<i64>,
    clear: bool,
    prepare: u32,
    kind: Kind,
) -> ApiResult<()> {
    lock(tx).await?;
    let grant = sqlx::query(
        "SELECT account_id,revision,enabled FROM remote_grants WHERE device_id=? AND provider=?",
    )
    .bind(id)
    .bind(kind.as_str())
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(g) = grant {
        if g.get::<bool, _>("enabled") {
            // A downgraded client cannot keep an ET grant alive. Revoke it rather
            // than interpreting a healthy legacy heartbeat as accelerator loss.
            if kind == Kind::Etalien && revision.is_none() {
                storage::revoke_provider(tx, id, kind).await?;
                return Ok(());
            }
            if revision != Some(g.get::<i64, _>("revision")) || run <= 0 {
                return Err(ApiError(
                    StatusCode::CONFLICT,
                    "远程授权版本已变化，请刷新状态",
                ));
            }
            if clear {
                storage::revoke_provider(tx, id, kind).await?;
            } else {
                sqlx::query("UPDATE remote_grants SET armed_at=COALESCE(armed_at,UTC_TIMESTAMP(6)),last_seen=UTC_TIMESTAMP(6),prepare_until=UTC_TIMESTAMP(6)+INTERVAL ? SECOND,run_generation=?,updated_at=UTC_TIMESTAMP(6) WHERE device_id=? AND provider=?").bind(prepare as f64).bind(run).bind(id).bind(kind.as_str()).execute(&mut **tx).await?;
                storage::cancel_device_account(tx, id, kind).await?;
            }
        }
    }
    Ok(())
}
pub async fn listing(State(s): State<AppState>, h: HeaderMap) -> ApiResult<Json<Value>> {
    let u = auth::user(&s, &h, false).await?;
    let rows:Vec<Value>=storage::json_rows(sqlx::query("SELECT d.id AS device_id,g.provider AS provider,d.name AS device_name,COALESCE(u.email,u.username) AS owner,d.user_id AS user_id,a.label AS label,EXISTS(SELECT 1 FROM cafe_policies c WHERE c.account_id=a.id AND c.enabled) AS cafe_enabled,g.enabled AS enabled,a.credential_state AS credential,g.armed_at AS armed_at,g.last_seen AS last_seen,g.revision AS revision,CASE WHEN d.revoked THEN 'revoked' WHEN g.last_seen>UTC_TIMESTAMP(6)-INTERVAL 45 SECOND THEN 'online' WHEN g.last_seen>UTC_TIMESTAMP(6)-INTERVAL 120 SECOND THEN 'waiting' ELSE 'offline' END AS connection,j.result AS last_result,CASE WHEN NOT g.enabled THEN 'off' WHEN a.credential_state<>'valid' THEN 'reauthorize' WHEN g.armed_at IS NULL THEN 'awaiting_heartbeat' WHEN j.epoch=a.epoch AND j.state='running' THEN 'executing' WHEN j.epoch=a.epoch AND j.state='confirmed' THEN 'confirmed' WHEN j.epoch=a.epoch AND j.state='unconfirmed' THEN 'unconfirmed' WHEN g.last_seen<UTC_TIMESTAMP(6)-INTERVAL 45 SECOND THEN 'waiting' ELSE 'armed' END AS protection FROM remote_grants g JOIN devices d ON d.id=g.device_id JOIN users u ON u.id=d.user_id JOIN remote_accounts a ON a.id=g.account_id LEFT JOIN remote_jobs j ON j.id=(SELECT newest.id FROM remote_jobs newest WHERE newest.account_id=a.id ORDER BY newest.created_at DESC,newest.id DESC LIMIT 1) WHERE (? OR d.user_id=?) ORDER BY g.updated_at DESC LIMIT 500").bind(u.role=="admin").bind(u.id).fetch_all(&s.db).await?)?;
    let jobs:Vec<Value>=storage::json_rows(sqlx::query("SELECT j.id AS id,a.provider AS provider,a.label AS label,j.state AS state,j.trigger_kind AS trigger_kind,j.result AS result,j.attempts AS attempts,j.created_at AS created_at,j.updated_at AS updated_at FROM remote_jobs j JOIN remote_accounts a ON a.id=j.account_id WHERE (? OR a.user_id=?) ORDER BY j.created_at DESC LIMIT 100").bind(u.role=="admin").bind(u.id).fetch_all(&s.db).await?)?;
    Ok(Json(
        json!({"grants":rows,"jobs":jobs,"cafe":cafe::listing(&s,&u).await?,"service":service(&s).await?}),
    ))
}
pub async fn acknowledge(State(s): State<AppState>, h: HeaderMap) -> ApiResult<Json<Value>> {
    let u = auth::admin(&s, &h, true).await?;
    let mut tx = storage::begin(&s.db).await?;
    lock(&mut tx).await?;
    // Acknowledgement discards old offline episodes, never releases a stale batch.
    sqlx::query("UPDATE remote_jobs SET state='cancelled',result='incident_acknowledged',updated_at=UTC_TIMESTAMP(6) WHERE state IN ('queued','running')").execute(&mut *tx).await?;
    sqlx::query("UPDATE cafe_policies SET started_at=NULL,observed_at=NULL,observed_state='unknown',revision=revision+1,poll_lease=NULL,poll_until=NULL,next_poll=UTC_TIMESTAMP(6) WHERE enabled")
        .execute(&mut *tx).await?;
    sqlx::query("UPDATE remote_grants SET armed_at=NULL WHERE enabled")
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE remote_service SET blocked=false,reason='reconnect',warmup_until=UTC_TIMESTAMP(6)+INTERVAL 120 SECOND WHERE singleton").execute(&mut *tx).await?;
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
