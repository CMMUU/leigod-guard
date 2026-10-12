//! Account-scoped v2 coordination. No provider HTTP inside database transactions.
use crate::cycle_protocol::{CycleRequest, CycleView};
use crate::*;
use sqlx::{MySql, Transaction};
type Tx<'a> = Transaction<'a, MySql>;

pub async fn listing(s: &AppState, user: &SessionUser) -> ApiResult<Vec<Value>> {
    storage::json_rows(sqlx::query("SELECT d.id AS device_id,d.name AS device_name,r.provider AS provider,r.label AS label,c.id AS id,c.state AS state,c.started_at AS started_at,c.completed_at AS completed_at,o.phase AS phase,o.observed_at AS observed_at,o.executor_at AS executor_at,o.idle_until AS deadline,CASE WHEN p.enabled THEN 'cafe' ELSE 'coordinated' END AS mode,CASE WHEN o.observed_at>UTC_TIMESTAMP(6)-INTERVAL 22 SECOND AND o.phase<>'unknown' THEN 'healthy' ELSE 'unknown' END AS observer_health,CASE WHEN o.executor_at>UTC_TIMESTAMP(6)-INTERVAL 60 SECOND THEN 'healthy' ELSE 'stalled' END AS executor_health,x.state AS action_state,a.startup_seconds AS startup_seconds,a.exit_seconds AS exit_seconds,a.policy_revision AS policy_revision FROM guard_observers o JOIN devices d ON d.id=o.device_id JOIN remote_accounts r ON r.id=o.account_id JOIN guard_accounts a ON a.account_id=r.id LEFT JOIN guard_cycles c ON c.id=a.cycle_id LEFT JOIN cafe_policies p ON p.account_id=r.id LEFT JOIN guard_actions x ON x.id=(SELECT y.id FROM guard_actions y WHERE y.account_id=r.id AND y.cycle_id=c.id ORDER BY y.created_at DESC LIMIT 1) WHERE o.enabled AND (? OR r.user_id=?) ORDER BY d.name LIMIT 500").bind(user.role=="admin").bind(user.id).fetch_all(&s.db).await?)
}

fn conflict() -> ApiError {
    ApiError(StatusCode::CONFLICT, "周期或授权已变化，请重新同步")
}
fn uuid(value: &Option<String>) -> ApiResult<Uuid> {
    value
        .as_deref()
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(conflict)
}
pub async fn invalidate(tx: &mut Tx<'_>, account: Uuid) -> ApiResult<()> {
    sqlx::query("UPDATE guard_cycles c JOIN guard_accounts a ON a.cycle_id=c.id SET c.revision=c.revision+1 WHERE a.account_id=?").bind(account).execute(&mut **tx).await?;
    sqlx::query("UPDATE guard_actions SET state='cancelled',result='conditions_changed',updated_at=UTC_TIMESTAMP(6) WHERE account_id=? AND state='leased'").bind(account).execute(&mut **tx).await?;
    Ok(())
}
pub async fn revoke(tx: &mut Tx<'_>, device: Uuid, kind: provider::Kind) -> ApiResult<()> {
    let account: Option<Uuid> = sqlx::query_scalar(
        "SELECT account_id FROM guard_observers WHERE device_id=? AND provider=? AND enabled",
    )
    .bind(device)
    .bind(kind.as_str())
    .fetch_optional(&mut **tx)
    .await?;
    sqlx::query("UPDATE guard_observers SET enabled=false,phase='unknown',idle_until=NULL WHERE device_id=? AND provider=?").bind(device).bind(kind.as_str()).execute(&mut **tx).await?;
    if let Some(a) = account {
        invalidate(tx, a).await?;
    }
    Ok(())
}

pub async fn endpoint(
    State(s): State<AppState>,
    h: HeaderMap,
    Json(q): Json<CycleRequest>,
) -> ApiResult<Json<CycleView>> {
    let (device, _) = remote::device(&s, &h).await?;
    let kind = provider::Kind::parse(&q.provider).map_err(|_| conflict())?;
    if q.sequence < 0
        || q.run_generation <= 0
        || q.prepare_seconds > 600
        || q.startup_seconds > 3600
        || q.exit_seconds > 3600
        || q.request_id.len() > 64
    {
        return Err(ApiError(StatusCode::BAD_REQUEST, "周期参数无效"));
    }
    auth::throttle(&s, format!("cycle:{device}"), 120, 60)?;
    let mut tx = storage::begin(&s.db).await?;
    if q.op == "complete" {
        let id = uuid(&q.action_id)?;
        let r=sqlx::query("SELECT account_id,cycle_id,state FROM guard_actions WHERE id=? AND device_id=? AND run_generation=?").bind(id).bind(device).bind(q.run_generation).fetch_optional(&mut *tx).await?.ok_or_else(conflict)?;
        let account: Uuid = r.get("account_id");
        let state: String = r.get("state");
        if matches!(state.as_str(), "leased" | "sent" | "unknown" | "restricted") {
            let result = match q.result.as_str() {
                "confirmed" => "confirmed",
                "cancelled" => "cancelled",
                "restricted" => "restricted",
                _ => "unknown",
            };
            sqlx::query(
                "UPDATE guard_actions SET state=?,result=?,updated_at=UTC_TIMESTAMP(6) WHERE id=?",
            )
            .bind(result)
            .bind(result)
            .bind(id)
            .execute(&mut *tx)
            .await?;
            if result == "confirmed" {
                complete_cycle(&mut tx, r.get("cycle_id")).await?;
            }
        }
        let response = view(&mut tx, account).await?;
        tx.commit().await?;
        return Ok(Json(response));
    }

    let grant=sqlx::query("SELECT g.account_id,a.credential_version FROM remote_grants g JOIN devices d ON d.id=g.device_id JOIN remote_accounts a ON a.id=g.account_id WHERE g.device_id=? AND g.provider=? AND g.enabled AND g.revision=? AND g.run_generation=? AND d.run_generation=? AND d.guard_provider=g.provider AND NOT d.revoked AND a.credential_state='valid'").bind(device).bind(kind.as_str()).bind(q.grant_revision).bind(q.run_generation).bind(q.run_generation).fetch_optional(&mut *tx).await?.ok_or_else(conflict)?;
    let account: Uuid = grant.get("account_id");
    let credential_version: i64 = grant.get("credential_version");
    if q.op == "enable" && !sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM guard_observers WHERE device_id=? AND provider=? AND account_id=? AND enabled AND grant_revision=? AND run_generation=?)").bind(device).bind(&q.provider).bind(account).bind(q.grant_revision).bind(q.run_generation).fetch_one(&mut *tx).await? {
        sqlx::query("INSERT INTO guard_accounts(account_id,startup_seconds,exit_seconds,startup_enabled) VALUES(?,?,?,?) ON DUPLICATE KEY UPDATE account_id=guard_accounts.account_id").bind(account).bind(q.startup_seconds).bind(q.exit_seconds).bind(q.startup_enabled).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO guard_observers(device_id,provider,account_id,grant_revision,run_generation) VALUES(?,?,?,?,?) ON DUPLICATE KEY UPDATE account_id=VALUES(account_id),grant_revision=VALUES(grant_revision),run_generation=VALUES(run_generation),enabled=true,sequence=-1,phase='unknown',idle_until=NULL,cycle_id=NULL,seen_at=NULL").bind(device).bind(kind.as_str()).bind(account).bind(q.grant_revision).bind(q.run_generation).execute(&mut *tx).await?;
        storage::cancel(&mut tx, account).await?;
        invalidate(&mut tx, account).await?;
    }
    let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM guard_observers WHERE device_id=? AND provider=? AND enabled AND account_id=? AND grant_revision=? AND run_generation=?)").bind(device).bind(kind.as_str()).bind(account).bind(q.grant_revision).bind(q.run_generation).fetch_one(&mut *tx).await?;
    if !valid {
        return Err(conflict());
    }
    let mut action = None;
    match q.op.as_str() {
        "enable" | "status" => {}
        "observe" => observe(&mut tx, account, device, &q).await?,
        "policy" => {
            let revision: i64 =
                sqlx::query_scalar("SELECT policy_revision FROM guard_accounts WHERE account_id=?")
                    .bind(account)
                    .fetch_one(&mut *tx)
                    .await?;
            if q.policy_revision != revision {
                return Err(conflict());
            }
            sqlx::query("UPDATE guard_accounts SET startup_seconds=?,exit_seconds=?,startup_enabled=?,policy_revision=policy_revision+1 WHERE account_id=?").bind(q.startup_seconds).bind(q.exit_seconds).bind(q.startup_enabled).bind(account).execute(&mut *tx).await?;
            sqlx::query("UPDATE guard_observers SET idle_until=NULL WHERE account_id=?")
                .bind(account)
                .execute(&mut *tx)
                .await?;
            invalidate(&mut tx, account).await?;
        }
        "claim" => {
            if q.request_id.len() < 16 {
                return Err(conflict());
            }
            let previous:Option<Uuid>=sqlx::query_scalar("SELECT id FROM guard_actions WHERE account_id=? AND request_id=? AND device_id=? AND run_generation=? AND state='leased' AND lease_until>UTC_TIMESTAMP(6)+INTERVAL 10 SECOND").bind(account).bind(&q.request_id).bind(device).bind(q.run_generation).fetch_optional(&mut *tx).await?;
            if previous.is_some() {
                action = previous;
            } else if !sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM guard_actions WHERE account_id=? AND request_id=?)",
            )
            .bind(account)
            .bind(&q.request_id)
            .fetch_one(&mut *tx)
            .await?
                && eligible(&mut tx, account, false, q.manual).await?
            {
                let cycle = uuid(&q.cycle_id)?;
                let revision:Option<i64>=sqlx::query_scalar("SELECT c.revision FROM guard_cycles c JOIN guard_accounts a ON a.cycle_id=c.id WHERE a.account_id=? AND c.id=? AND c.state='running'").bind(account).bind(cycle).fetch_optional(&mut *tx).await?;
                if let Some(revision) = revision {
                    let id = Uuid::new_v4();
                    sqlx::query("INSERT INTO guard_actions(id,account_id,cycle_id,device_id,run_generation,credential_version,cycle_revision,request_id,is_manual,lease_until) VALUES(?,?,?,?,?,?,?,?,?,UTC_TIMESTAMP(6)+INTERVAL 45 SECOND)").bind(id).bind(account).bind(cycle).bind(device).bind(q.run_generation).bind(credential_version).bind(revision).bind(&q.request_id).bind(q.manual).execute(&mut *tx).await?;
                    action = Some(id);
                }
            }
        }
        "dispatch" => {
            let id = uuid(&q.action_id)?;
            let r=sqlx::query("SELECT is_manual,cycle_id,cycle_revision,credential_version FROM guard_actions WHERE id=? AND account_id=? AND device_id=? AND run_generation=? AND state='leased' AND lease_until>UTC_TIMESTAMP(6)+INTERVAL 10 SECOND").bind(id).bind(account).bind(device).bind(q.run_generation).fetch_optional(&mut *tx).await?.ok_or_else(conflict)?;
            let still:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM guard_accounts a JOIN guard_cycles c ON c.id=a.cycle_id WHERE a.account_id=? AND c.id=? AND c.revision=? AND c.state='running')").bind(account).bind(r.get::<Uuid,_>("cycle_id")).bind(r.get::<i64,_>("cycle_revision")).fetch_one(&mut *tx).await?;
            if !still
                || r.get::<i64, _>("credential_version") != credential_version
                || !conditions(&mut tx, account, false, r.get("is_manual")).await?
            {
                return Err(conflict());
            }
            sqlx::query("UPDATE guard_actions SET state='sent',result='awaiting_confirmation',updated_at=UTC_TIMESTAMP(6) WHERE id=?").bind(id).execute(&mut *tx).await?;
            action = Some(id);
        }
        _ => return Err(ApiError(StatusCode::BAD_REQUEST, "未知周期操作")),
    }
    let mut view = view(&mut tx, account).await?;
    view.action_id = action.map(|id| id.to_string());
    if action.is_some() {
        view.lease_seconds = 30;
    }
    tx.commit().await?;
    Ok(Json(view))
}

async fn complete_cycle(tx: &mut Tx<'_>, cycle: Uuid) -> ApiResult<()> {
    sqlx::query("UPDATE guard_cycles SET state='completed',revision=revision+1,completed_at=UTC_TIMESTAMP(6) WHERE id=? AND state='running'").bind(cycle).execute(&mut **tx).await?;
    sqlx::query("UPDATE guard_actions SET state='cancelled',result='cycle_completed' WHERE cycle_id=? AND state='leased'").bind(cycle).execute(&mut **tx).await?;
    sqlx::query("UPDATE guard_actions SET state='confirmed',result='cycle_confirmed',updated_at=UTC_TIMESTAMP(6) WHERE cycle_id=? AND state IN ('sent','unknown','restricted')").bind(cycle).execute(&mut **tx).await?;
    Ok(())
}
async fn observe(tx: &mut Tx<'_>, account: Uuid, device: Uuid, q: &CycleRequest) -> ApiResult<()> {
    if !matches!(
        q.phase.as_str(),
        "unknown" | "absent" | "running" | "launching"
    ) {
        return Err(conflict());
    }
    let old=sqlx::query("SELECT sequence,phase,cycle_id,activity,idle_until,game_generation FROM guard_observers WHERE device_id=? AND provider=?").bind(device).bind(&q.provider).fetch_one(&mut **tx).await?;
    if q.sequence <= old.get::<i64, _>("sequence") {
        return Ok(());
    }
    let row=sqlx::query("SELECT a.cycle_id,a.startup_seconds,a.exit_seconds,a.startup_enabled,c.state,c.started_at,c.completed_at FROM guard_accounts a LEFT JOIN guard_cycles c ON c.id=a.cycle_id WHERE a.account_id=?").bind(account).fetch_one(&mut **tx).await?;
    let mut cycle: Option<Uuid> = row.get("cycle_id");
    let completed: Option<DateTime<Utc>> = row.get("completed_at");
    let billing_at = Utc::now() - chrono::Duration::seconds(q.billing_age.min(86400) as i64);
    let billing_fresh = q.billing_paused.is_some() && q.billing_age <= 45;
    if billing_fresh
        && q.billing_paused == Some(false)
        && (cycle.is_none()
            || completed.is_some_and(|at| billing_at > at + chrono::Duration::seconds(2)))
    {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO guard_cycles(id,account_id) VALUES(?,?)")
            .bind(id)
            .bind(account)
            .execute(&mut **tx)
            .await?;
        sqlx::query("UPDATE guard_accounts SET cycle_id=? WHERE account_id=?")
            .bind(id)
            .bind(account)
            .execute(&mut **tx)
            .await?;
        cycle = Some(id);
    } else if billing_fresh
        && q.billing_paused == Some(true)
        && cycle.is_some()
        && q.cycle_id.as_deref() == cycle.map(|v| v.to_string()).as_deref()
        && row
            .get::<Option<DateTime<Utc>>, _>("started_at")
            .is_some_and(|at| billing_at >= at)
    {
        complete_cycle(tx, cycle.unwrap()).await?;
    }
    let phase = if q.observer_age <= 7 {
        q.phase.as_str()
    } else {
        "unknown"
    };
    let changed = old.get::<u64, _>("game_generation") != q.game_generation
        || old.get::<String, _>("phase") != phase
        || old.get::<Option<Uuid>, _>("cycle_id") != cycle;
    if changed {
        invalidate(tx, account).await?;
    }
    let same_cycle = old.get::<Option<Uuid>, _>("cycle_id") == cycle;
    let activity = matches!(phase, "running" | "launching")
        || (same_cycle
            && old.get::<String, _>("phase") != "unknown"
            && old.get::<bool, _>("activity"));
    let idle: Option<DateTime<Utc>> = old.get("idle_until");
    let deadline = if phase == "absent"
        && cycle.is_some()
        && (activity || row.get::<bool, _>("startup_enabled"))
    {
        if !changed && idle.is_some() {
            idle
        } else {
            Some(
                Utc::now()
                    + chrono::Duration::seconds(i64::from(row.get::<i32, _>(if activity {
                        "exit_seconds"
                    } else {
                        "startup_seconds"
                    }))),
            )
        }
    } else {
        None
    };
    sqlx::query("UPDATE guard_observers SET game_generation=?,sequence=?,phase=?,cycle_id=?,activity=?,observed_at=UTC_TIMESTAMP(6)-INTERVAL ? SECOND,executor_at=UTC_TIMESTAMP(6)-INTERVAL ? SECOND,seen_at=UTC_TIMESTAMP(6),idle_until=?,prepare_until=UTC_TIMESTAMP(6)+INTERVAL ? SECOND,billing_at=CASE WHEN ? THEN ? ELSE billing_at END WHERE device_id=? AND provider=?").bind(q.game_generation).bind(q.sequence).bind(phase).bind(cycle).bind(activity).bind(q.observer_age.min(86400)).bind(q.executor_age.min(86400)).bind(deadline).bind(q.prepare_seconds).bind(billing_fresh).bind(billing_at).bind(device).bind(&q.provider).execute(&mut **tx).await?;
    Ok(())
}

async fn conditions(tx: &mut Tx<'_>, account: Uuid, cloud: bool, manual: bool) -> ApiResult<bool> {
    let healthy:bool=sqlx::query_scalar("SELECT ingress_ok AND NOT blocked AND warmup_until<=UTC_TIMESTAMP(6) AND last_tick>UTC_TIMESTAMP(6)-INTERVAL 20 SECOND FROM remote_service WHERE singleton").fetch_one(&mut **tx).await?;
    if !healthy {
        return Ok(false);
    }
    let cafe: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM cafe_policies WHERE account_id=? AND enabled)",
    )
    .bind(account)
    .fetch_one(&mut **tx)
    .await?;
    if cafe && !manual {
        return Ok(false);
    }
    let rows=sqlx::query("SELECT g.armed_at,GREATEST(g.last_seen,COALESCE(o.seen_at,g.last_seen)) AS last_seen,GREATEST(g.prepare_until,COALESCE(o.prepare_until,g.prepare_until)) AS prepare_until,o.enabled,o.grant_revision,g.revision,o.run_generation,g.run_generation AS grant_run,o.phase,o.observed_at,o.executor_at,o.idle_until,o.cycle_id,a.cycle_id AS current_cycle FROM remote_grants g JOIN guard_accounts a ON a.account_id=g.account_id LEFT JOIN guard_observers o ON o.device_id=g.device_id AND o.provider=g.provider AND o.account_id=g.account_id WHERE g.account_id=? AND g.enabled").bind(account).fetch_all(&mut **tx).await?;
    if rows.is_empty() {
        return Ok(false);
    }
    let now = Utc::now();
    for r in rows {
        // An old client can still write directly. Never advertise full coordination.
        if r.get::<Option<bool>, _>("enabled") != Some(true)
            || r.get::<Option<i64>, _>("grant_revision") != Some(r.get("revision"))
            || r.get::<Option<i64>, _>("run_generation") != Some(r.get("grant_run"))
        {
            return Ok(false);
        }
        if manual {
            continue;
        }
        if r.get::<Option<Uuid>, _>("cycle_id") != r.get::<Option<Uuid>, _>("current_cycle") {
            return Ok(false);
        }
        if r.get::<Option<DateTime<Utc>>, _>("prepare_until")
            .is_some_and(|t| t > now)
        {
            return Ok(false);
        }
        let fresh = r
            .get::<Option<DateTime<Utc>>, _>("observed_at")
            .is_some_and(|t| t + chrono::Duration::seconds(22) > now);
        if fresh
            && matches!(
                r.get::<Option<String>, _>("phase").as_deref(),
                Some("running" | "launching")
            )
        {
            return Ok(false);
        }
        let offline = r
            .get::<Option<DateTime<Utc>>, _>("last_seen")
            .is_some_and(|t| t + chrono::Duration::seconds(120) <= now)
            && r.get::<Option<DateTime<Utc>>, _>("armed_at").is_some();
        let idle = r.get::<Option<DateTime<Utc>>, _>("idle_until");
        if offline {
            if idle.is_some_and(|t| t > now) {
                return Ok(false);
            }
        } else {
            if r.get::<Option<String>, _>("phase").as_deref() != Some("absent")
                || !r
                    .get::<Option<DateTime<Utc>>, _>("observed_at")
                    .is_some_and(|t| t + chrono::Duration::seconds(22) > now)
                || !idle.is_some_and(|t| t <= now)
            {
                return Ok(false);
            }
            if cloud
                && r.get::<Option<DateTime<Utc>>, _>("executor_at")
                    .is_some_and(|t| t + chrono::Duration::seconds(60) > now)
            {
                return Ok(false);
            }
        }
    }
    Ok(true)
}
async fn eligible(tx: &mut Tx<'_>, account: Uuid, cloud: bool, manual: bool) -> ApiResult<bool> {
    sqlx::query("UPDATE guard_actions SET state='cancelled',result='unused_permit_expired' WHERE account_id=? AND state='leased' AND lease_until<=UTC_TIMESTAMP(6)").bind(account).execute(&mut **tx).await?;
    let busy:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM guard_actions WHERE account_id=? AND state IN ('leased','sent','unknown','restricted')) OR EXISTS(SELECT 1 FROM remote_jobs WHERE account_id=? AND (state IN ('queued','running') OR lease_until>UTC_TIMESTAMP(6))) OR EXISTS(SELECT 1 FROM cafe_policies WHERE account_id=? AND poll_until>UTC_TIMESTAMP(6))").bind(account).bind(account).bind(account).fetch_one(&mut **tx).await?;
    Ok(!busy && conditions(tx, account, cloud, manual).await?)
}
async fn view(tx: &mut Tx<'_>, account: Uuid) -> ApiResult<CycleView> {
    let r=sqlx::query("SELECT a.cycle_id,a.policy_revision,a.startup_seconds,a.exit_seconds,a.startup_enabled,c.revision,c.state FROM guard_accounts a LEFT JOIN guard_cycles c ON c.id=a.cycle_id WHERE a.account_id=?").bind(account).fetch_one(&mut **tx).await?;
    let cafe: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM cafe_policies WHERE account_id=? AND enabled)",
    )
    .bind(account)
    .fetch_one(&mut **tx)
    .await?;
    let remaining:Option<i64>=sqlx::query_scalar("SELECT CAST(GREATEST(0,TIMESTAMPDIFF(SECOND,UTC_TIMESTAMP(6),MAX(idle_until))+1) AS SIGNED) FROM guard_observers WHERE account_id=? AND enabled").bind(account).fetch_one(&mut **tx).await?;
    let state: String = r
        .get::<Option<String>, _>("state")
        .unwrap_or_else(|| "awaiting_billing".into());
    let ready = state == "running" && eligible(tx, account, false, false).await?;
    let pending:Option<String>=sqlx::query_scalar("SELECT state FROM guard_actions WHERE account_id=? AND state IN ('sent','unknown','restricted') ORDER BY created_at DESC LIMIT 1").bind(account).fetch_optional(&mut **tx).await?;
    let message = if cafe {
        "网吧限时已开启，本机自动暂停已抑制"
    } else if pending.as_deref() == Some("restricted") {
        "官方暂停受限，等待计时状态变化"
    } else if pending.is_some() {
        "暂停结果待确认，不重复发送"
    } else if state == "completed" {
        "本轮暂停已确认，等待再次计时"
    } else if ready {
        "本轮已到期，等待客户端复查执行"
    } else {
        "协同守护中，等待所有设备与安全期限满足"
    };
    Ok(CycleView {
        enabled: true,
        cycle_id: r.get::<Option<Uuid>, _>("cycle_id").map(|v| v.to_string()),
        revision: r.get::<Option<i64>, _>("revision").unwrap_or(0),
        policy_revision: r.get("policy_revision"),
        startup_seconds: r.get::<i32, _>("startup_seconds") as u32,
        startup_enabled: r.get("startup_enabled"),
        exit_seconds: r.get::<i32, _>("exit_seconds") as u32,
        mode: if cafe { "cafe" } else { "coordinated" }.into(),
        state: if ready { "ready".into() } else { state },
        remaining_seconds: remaining.unwrap_or(0) as u64,
        message: message.into(),
        ..Default::default()
    })
}

/// Called after the existing ingress/cohort health check. Two shared slots bound HTTP.
pub async fn tick(s: &AppState) -> ApiResult<()> {
    sqlx::query("DELETE FROM guard_actions WHERE state IN ('confirmed','cancelled') AND updated_at<UTC_TIMESTAMP(6)-INTERVAL 30 DAY").execute(&s.db).await?;
    sqlx::query("DELETE c FROM guard_cycles c LEFT JOIN guard_accounts a ON a.cycle_id=c.id WHERE a.account_id IS NULL AND c.completed_at<UTC_TIMESTAMP(6)-INTERVAL 30 DAY AND NOT EXISTS(SELECT 1 FROM guard_actions x WHERE x.cycle_id=c.id)").execute(&s.db).await?;

    let accounts:Vec<Uuid>=sqlx::query_scalar("SELECT a.account_id FROM guard_accounts a JOIN remote_accounts r ON r.id=a.account_id WHERE r.credential_state='valid' AND (EXISTS(SELECT 1 FROM guard_observers o WHERE o.account_id=a.account_id AND o.enabled) OR EXISTS(SELECT 1 FROM guard_actions x WHERE x.account_id=a.account_id AND x.state IN ('sent','unknown','restricted'))) ORDER BY r.updated_at LIMIT 100").fetch_all(&s.db).await?;
    for account in accounts {
        let Ok(slot) = s.remote_slots.clone().try_acquire_owned() else {
            break;
        };
        let mut tx = storage::begin(&s.db).await?;
        let pending=sqlx::query("SELECT id,cycle_id FROM guard_actions WHERE account_id=? AND state IN ('sent','unknown','restricted') AND lease_until<UTC_TIMESTAMP(6) ORDER BY created_at LIMIT 1").bind(account).fetch_optional(&mut *tx).await?;
        let query_only = pending.is_some();
        let (id, cycle) = if let Some(r) = pending {
            (r.get("id"), r.get("cycle_id"))
        } else {
            if !eligible(&mut tx, account, true, false).await? {
                continue;
            }
            let Some(r)=sqlx::query("SELECT c.id,c.revision,r.credential_version FROM guard_accounts a JOIN guard_cycles c ON c.id=a.cycle_id JOIN remote_accounts r ON r.id=a.account_id WHERE a.account_id=? AND c.state='running'").bind(account).fetch_optional(&mut *tx).await?else{continue;};
            let id = Uuid::new_v4();
            let cycle: Uuid = r.get("id");
            sqlx::query("INSERT INTO guard_actions(id,account_id,cycle_id,run_generation,credential_version,cycle_revision,request_id,state,lease_until) VALUES(?,?,?,0,?,?,?,'sent',UTC_TIMESTAMP(6)+INTERVAL 60 SECOND)").bind(id).bind(account).bind(cycle).bind(r.get::<i64,_>("credential_version")).bind(r.get::<i64,_>("revision")).bind(id.to_string()).execute(&mut *tx).await?;
            (id, cycle)
        };
        sqlx::query(
            "UPDATE guard_actions SET lease_until=UTC_TIMESTAMP(6)+INTERVAL 60 SECOND WHERE id=?",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        let state = s.clone();
        tokio::spawn(async move {
            let _slot = slot;
            if execute(&state, account, id, cycle, query_only)
                .await
                .is_err()
            {
                tracing::error!("cycle action persistence unavailable; confirmation will recover");
            }
        });
    }
    Ok(())
}
async fn execute(
    s: &AppState,
    account: Uuid,
    id: Uuid,
    cycle: Uuid,
    query_only: bool,
) -> ApiResult<()> {
    let Some(p) = s.provider.as_ref() else {
        return Ok(());
    };
    let Some(r)=sqlx::query("SELECT provider,provider_key,credential,credential_version FROM remote_accounts WHERE id=? AND credential_state='valid' AND credential IS NOT NULL").bind(account).fetch_optional(&s.db).await?else{return Ok(());};
    let key: String = r.get("provider_key");
    let kind = provider::Kind::parse(r.get("provider")).map_err(|_| conflict())?;
    let token = match p.open(&key, &r.get::<Vec<u8>, _>("credential")) {
        Ok(t) => t,
        Err(_) => return Ok(()),
    };
    let before = p.info(kind, &token).await;
    let mut result = "unknown";
    if before
        .as_ref()
        .is_ok_and(|v| v.key == key && v.paused == Some(true))
    {
        result = "confirmed";
    } else if !query_only
        && before
            .as_ref()
            .is_ok_and(|v| v.key == key && v.paused == Some(false))
    {
        let mut tx = storage::begin(&s.db).await?;
        let current:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM guard_accounts a JOIN guard_cycles c ON c.id=a.cycle_id JOIN guard_actions x ON x.cycle_id=c.id JOIN remote_accounts r ON r.id=a.account_id WHERE x.id=? AND c.id=? AND c.state='running' AND x.cycle_revision=c.revision AND x.credential_version=r.credential_version AND r.credential_state='valid')").bind(id).bind(cycle).fetch_one(&mut *tx).await?;
        let safe = current && conditions(&mut tx, account, true, false).await?;
        tx.commit().await?;
        if safe {
            let sent = p.pause(kind, &token).await;
            if p.info(kind, &token)
                .await
                .is_ok_and(|v| v.key == key && v.paused == Some(true))
            {
                result = "confirmed";
            } else if sent == Err(provider::Failure::Restricted) {
                result = "restricted";
            }
        } else {
            result = "cancelled";
        }
    }
    let mut tx = storage::begin(&s.db).await?;
    sqlx::query("UPDATE guard_actions SET result=IF(state='restricted' AND ?='unknown',result,?),state=IF(state='restricted' AND ?='unknown',state,?),updated_at=UTC_TIMESTAMP(6),lease_until=UTC_TIMESTAMP(6)+INTERVAL 60 SECOND WHERE id=? AND state IN ('sent','unknown','restricted')").bind(result).bind(result).bind(result).bind(result).bind(id).execute(&mut *tx).await?;
    if result == "confirmed" {
        complete_cycle(&mut tx, cycle).await?;
    }
    tx.commit().await?;
    Ok(())
}
