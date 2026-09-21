//! PostgreSQL queue. Transactions only cover decisions; all HTTP happens after commit.
use crate::*;
const ELIGIBLE: &str = "a.credential_state='valid' AND a.credential IS NOT NULL AND EXISTS(SELECT 1 FROM remote_grants g WHERE g.account_id=a.id AND g.enabled) AND NOT EXISTS(SELECT 1 FROM remote_grants g WHERE g.account_id=a.id AND g.enabled AND (g.armed_at IS NULL OR g.last_seen IS NULL OR g.last_seen>now()-interval '120 seconds' OR g.prepare_until>now()))";
struct Job {
    id: Uuid,
    account: Uuid,
    lease: Uuid,
    epoch: i64,
    credential_version: i64,
    key: String,
    cipher: Vec<u8>,
    attempts: i32,
}
pub async fn run(s: AppState) {
    if s.provider.is_none() {
        return;
    }
    if sqlx::query("UPDATE remote_service SET warmup_until=now()+interval '120 seconds',ingress_ok=false,reason=CASE WHEN blocked THEN reason ELSE 'startup' END WHERE singleton").execute(&s.db).await.is_err(){return;}
    let probe = match reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(3))
        .build()
    {
        Ok(c) => c,
        Err(_) => return,
    };
    let mut interval = tokio::time::interval(Duration::from_secs(5));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        let healthy = probe
            .get(format!("{}/api/health", s.origin))
            .send()
            .await
            .is_ok_and(|r| r.status().is_success());
        if tick(&s, healthy).await.is_err() {
            tracing::error!("remote scheduler failed; no new work dispatched");
            continue;
        }
        for _ in 0..2 {
            let Ok(permit) = s.remote_slots.clone().try_acquire_owned() else {
                break;
            };
            match claim(&s).await {
                Ok(Some(job)) => {
                    let state = s.clone();
                    tokio::spawn(async move {
                        let _permit = permit;
                        if execute(&state, &job).await.is_err() {
                            tracing::error!("remote task persistence failed; lease recovery will query before retry");
                        }
                    });
                }
                Ok(None) => break,
                Err(_) => {
                    tracing::error!("remote task claim failed");
                    break;
                }
            }
        }
    }
}
async fn tick(s: &AppState, healthy: bool) -> ApiResult<()> {
    let mut tx = s.db.begin().await?;
    remote::lock(&mut tx).await?;
    // An interrupted scheduler or public ingress failure gets a full reconnect window.
    sqlx::query("UPDATE remote_service SET warmup_until=CASE WHEN NOT $1 OR NOT ingress_ok OR last_tick<now()-interval '20 seconds' THEN now()+interval '120 seconds' ELSE warmup_until END,ingress_ok=$1,last_tick=now(),reason=CASE WHEN blocked THEN reason WHEN NOT $1 THEN 'ingress_unavailable' WHEN NOT ingress_ok THEN 'reconnect' WHEN warmup_until<=now() THEN 'ready' ELSE reason END WHERE singleton").bind(healthy).execute(&mut *tx).await?;
    // Five or more protected accounts lost in the same 30-second band, >=60% of
    // armed accounts, is an incident requiring explicit admin acknowledgement.
    let mass:bool=sqlx::query_scalar("WITH seen AS(SELECT a.id,max(g.last_seen) AS seen FROM remote_accounts a JOIN remote_grants g ON g.account_id=a.id AND g.enabled WHERE a.credential_state='valid' GROUP BY a.id HAVING bool_and(g.armed_at IS NOT NULL)), recent AS(SELECT count(*) FILTER(WHERE seen<now()-interval '120 seconds' AND seen>now()-interval '150 seconds') AS lost,count(*) AS total FROM seen) SELECT lost>=5 AND lost*5>=total*3 FROM recent").fetch_one(&mut *tx).await?;
    if mass {
        sqlx::query(
            "UPDATE remote_service SET blocked=true,reason='mass_disconnect' WHERE singleton",
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE remote_jobs SET state='cancelled',result='mass_disconnect',updated_at=now() WHERE state IN ('queued','running')").execute(&mut *tx).await?;
    }
    sqlx::query("UPDATE remote_jobs SET state='unconfirmed',result='retry_window_expired',updated_at=now() WHERE state IN ('queued','running') AND expires_at<=now() AND (lease_until IS NULL OR lease_until<=now())").execute(&mut *tx).await?;
    sqlx::query(&format!("INSERT INTO remote_jobs(id,account_id,epoch,credential_version) SELECT gen_random_uuid(),a.id,a.epoch,a.credential_version FROM remote_accounts a,remote_service s WHERE {ELIGIBLE} AND s.singleton AND s.ingress_ok AND NOT s.blocked AND s.warmup_until<=now() ON CONFLICT(account_id,epoch) DO NOTHING")).execute(&mut *tx).await?;
    // Keep terminal rows for current epochs to preserve deduplication; old history 30d.
    sqlx::query("DELETE FROM remote_jobs j USING remote_accounts a WHERE j.account_id=a.id AND j.epoch<>a.epoch AND j.state NOT IN ('queued','running') AND j.updated_at<now()-interval '30 days' AND (j.lease_until IS NULL OR j.lease_until<now())").execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}
async fn claim(s: &AppState) -> ApiResult<Option<Job>> {
    let mut tx = s.db.begin().await?;
    remote::lock(&mut tx).await?;
    let row=sqlx::query(&format!("SELECT j.id,j.account_id,j.epoch,j.credential_version,j.attempts,a.provider_key,a.credential FROM remote_jobs j JOIN remote_accounts a ON a.id=j.account_id,remote_service s WHERE j.state IN ('queued','running') AND j.next_attempt<=now() AND j.expires_at>now()+interval '40 seconds' AND j.attempts<3 AND (j.lease_until IS NULL OR j.lease_until<now()) AND j.epoch=a.epoch AND j.credential_version=a.credential_version AND {ELIGIBLE} AND s.ingress_ok AND NOT s.blocked AND s.warmup_until<=now() AND s.last_tick>now()-interval '20 seconds' AND (SELECT count(*) FROM remote_jobs WHERE lease_until>now())<2 AND NOT EXISTS(SELECT 1 FROM remote_jobs k WHERE k.account_id=a.id AND k.lease_until>now()) ORDER BY j.next_attempt FOR UPDATE OF j SKIP LOCKED LIMIT 1")).fetch_optional(&mut *tx).await?;
    let Some(r) = row else {
        return Ok(None);
    };
    let job = Job {
        id: r.get("id"),
        account: r.get("account_id"),
        epoch: r.get("epoch"),
        credential_version: r.get("credential_version"),
        lease: Uuid::new_v4(),
        key: r.get("provider_key"),
        cipher: r.get("credential"),
        attempts: r.get::<i32, _>("attempts") + 1,
    };
    sqlx::query("UPDATE remote_jobs SET state='running',lease_id=$2,lease_until=now()+interval '90 seconds',attempts=attempts+1,result='querying',updated_at=now() WHERE id=$1").bind(job.id).bind(job.lease).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Some(job))
}
async fn current(s: &AppState, j: &Job) -> ApiResult<bool> {
    let mut tx = s.db.begin().await?;
    remote::lock(&mut tx).await?;
    let ok:bool=sqlx::query_scalar(&format!("SELECT EXISTS(SELECT 1 FROM remote_jobs j JOIN remote_accounts a ON a.id=j.account_id,remote_service s WHERE j.id=$1 AND j.lease_id=$2 AND j.state='running' AND j.lease_until>now()+interval '15 seconds' AND j.expires_at>now() AND j.epoch=a.epoch AND j.credential_version=a.credential_version AND {ELIGIBLE} AND s.ingress_ok AND NOT s.blocked AND s.warmup_until<=now() AND s.last_tick>now()-interval '20 seconds')")).bind(j.id).bind(j.lease).fetch_one(&mut *tx).await?;
    tx.commit().await?;
    Ok(ok)
}
async fn execute(s: &AppState, j: &Job) -> ApiResult<()> {
    let Some(p) = s.provider.as_ref() else {
        return Ok(());
    };
    if !current(s, j).await? {
        return finish(s, j, "cancelled", "conditions_changed").await;
    }
    let token = match p.open(&j.key, &j.cipher) {
        Ok(t) => t,
        Err(_) => return finish(s, j, "reauthorize", "credential_unreadable").await,
    };
    let before = p.info(&token).await;
    match before {
        Ok(info) if info.key != j.key => {
            return finish(s, j, "reauthorize", "credential_account_mismatch").await
        }
        Ok(info) if info.paused == Some(true) => {
            return finish(s, j, "confirmed", "already_paused").await
        }
        Ok(info) if info.paused == Some(false) => {}
        Err(provider::Failure::Credential) => {
            return finish(s, j, "reauthorize", "credential_expired").await
        }
        _ => return retry(s, j, "status_unknown").await,
    }
    if !current(s, j).await? {
        return finish(s, j, "cancelled", "conditions_changed").await;
    }
    // There is no provider fencing/idempotency contract. Cancellation after this
    // dispatch is best effort, explicitly surfaced in the audit/status contract.
    let sent = p.pause(&token).await;
    if sent == Err(provider::Failure::Credential) {
        return finish(s, j, "reauthorize", "credential_expired").await;
    }
    // Always query after dispatch, including timeouts that may have applied remotely.
    match p.info(&token).await {
        Ok(info) if info.key == j.key && info.paused == Some(true) => {
            finish(s, j, "confirmed", "pause_confirmed").await
        }
        Err(provider::Failure::Credential) => {
            finish(s, j, "reauthorize", "credential_expired").await
        }
        Ok(info) if info.key != j.key => {
            finish(s, j, "reauthorize", "credential_account_mismatch").await
        }
        _ => retry(s, j, "pause_not_confirmed").await,
    }
}
async fn retry(s: &AppState, j: &Job, reason: &str) -> ApiResult<()> {
    if j.attempts >= 3 {
        return finish(s, j, "unconfirmed", reason).await;
    }
    if !current(s, j).await? {
        return finish(s, j, "cancelled", "conditions_changed").await;
    }
    let delay = if j.attempts == 1 { 15.0 } else { 45.0 };
    sqlx::query("UPDATE remote_jobs SET state='queued',result=$3,next_attempt=now()+make_interval(secs=>$4),lease_until=NULL,lease_id=NULL,updated_at=now() WHERE id=$1 AND lease_id=$2 AND state='running'").bind(j.id).bind(j.lease).bind(reason).bind(delay).execute(&s.db).await?;
    Ok(())
}
async fn finish(s: &AppState, j: &Job, state: &str, result: &str) -> ApiResult<()> {
    let mut tx = s.db.begin().await?;
    remote::lock(&mut tx).await?;
    let changed=sqlx::query("UPDATE remote_jobs SET state=CASE WHEN state='cancelled' THEN state ELSE $3 END,result=CASE WHEN state='cancelled' AND $3='confirmed' THEN 'confirmed_after_cancel' WHEN state='cancelled' THEN result ELSE $4 END,lease_until=NULL,lease_id=NULL,updated_at=now() WHERE id=$1 AND lease_id=$2 RETURNING account_id,state")
        .bind(j.id).bind(j.lease).bind(state).bind(result).fetch_optional(&mut *tx).await?;
    if let Some(row) = changed {
        if state == "reauthorize" {
            sqlx::query("UPDATE remote_accounts SET credential_state='reauthorize',credential=NULL WHERE id=$1 AND credential_version=$2 AND epoch=$3").bind(j.account).bind(j.credential_version).bind(j.epoch).execute(&mut *tx).await?;
            sqlx::query("UPDATE remote_jobs SET state='reauthorize',result='credential_expired',updated_at=now() WHERE account_id=$1 AND credential_version=$2 AND state='queued'").bind(j.account).bind(j.credential_version).execute(&mut *tx).await?;
        }
        let uid: Uuid = sqlx::query_scalar("SELECT user_id FROM remote_accounts WHERE id=$1")
            .bind(j.account)
            .fetch_one(&mut *tx)
            .await?;
        let detail = match row.get::<String, _>("state").as_str() {
            "confirmed" => "远程暂停已通过雷神状态查询确认",
            "reauthorize" => "雷神凭据已失效，请在客户端重新授权",
            "unconfirmed" => "远程暂停未确认；有限重试已结束",
            _ => "远程任务已取消；已发出请求的结果请查看任务记录",
        };
        routes::audit(&mut tx, uid, None, "remote_task_result", detail).await?;
    }
    tx.commit().await?;
    Ok(())
}
