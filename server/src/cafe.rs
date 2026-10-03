//! Cloud time limits use observed billing, never the absence of a browser heartbeat.
use crate::*;

#[derive(Deserialize)]
pub struct Settings {
    enabled: bool,
    #[serde(default = "default_hours")]
    max_hours: i32,
    revision: i64,
}
fn default_hours() -> i32 {
    24
}

pub async fn save(
    State(s): State<AppState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
    Json(p): Json<Settings>,
) -> ApiResult<Json<Value>> {
    let user = auth::user(&s, &h, true).await?;
    if !(1..=168).contains(&p.max_hours) || p.revision < 0 || p.revision == i64::MAX {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "暂停时长须为 1–168 的整数小时",
        ));
    }
    if p.enabled && s.provider.is_none() {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "服务器远程暂停能力尚未开放",
        ));
    }
    let mut tx = storage::begin(&s.db).await?;
    // Same user -> global lock order as user disabling. Recheck after session auth.
    sqlx::query("SELECT id FROM users WHERE id=? AND NOT disabled FOR UPDATE")
        .bind(user.id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ApiError(StatusCode::UNAUTHORIZED, "账号已停用"))?;
    remote::lock(&mut tx).await?;
    let account = sqlx::query("SELECT credential_state,credential IS NOT NULL AS has_credential FROM remote_accounts WHERE id=? AND user_id=?").bind(id).bind(user.id).fetch_optional(&mut *tx).await?
        .ok_or(ApiError(StatusCode::NOT_FOUND, "加速器账号不存在"))?;
    let old = sqlx::query("SELECT revision,enabled FROM cafe_policies WHERE account_id=?")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
    let revision = old.as_ref().map_or(0, |r| r.get::<i64, _>("revision"));
    if p.revision != revision {
        return Err(ApiError(StatusCode::CONFLICT, "设置已变化，请刷新后重试"));
    }
    if p.enabled
        && (account.get::<String, _>("credential_state") != "valid"
            || !account.get::<bool, _>("has_credential"))
    {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "请先在客户端重新授权对应加速器，再开启网吧模式",
        ));
    }
    let was_enabled = old.as_ref().is_some_and(|r| r.get::<bool, _>("enabled"));
    if old.is_some() {
        sqlx::query("UPDATE cafe_policies SET enabled=?,max_hours=?,revision=revision+1,started_at=CASE WHEN ? THEN started_at ELSE NULL END,observed_at=CASE WHEN ? THEN observed_at ELSE NULL END,observed_state=CASE WHEN ? THEN observed_state ELSE 'unknown' END,next_poll=UTC_TIMESTAMP(6),poll_lease=NULL,poll_until=NULL,failures=0,updated_at=UTC_TIMESTAMP(6) WHERE account_id=?").bind(p.enabled).bind(p.max_hours).bind(was_enabled && p.enabled).bind(was_enabled && p.enabled).bind(was_enabled && p.enabled).bind(id).execute(&mut *tx).await?;
    } else {
        sqlx::query("INSERT INTO cafe_policies(account_id,enabled,max_hours) VALUES(?,?,?)")
            .bind(id)
            .bind(p.enabled)
            .bind(p.max_hours)
            .execute(&mut *tx)
            .await?;
    }
    storage::cancel(&mut tx, id).await?;
    if was_enabled && !p.enabled {
        // Leaving cafe mode must not immediately unleash stale home-device jobs.
        sqlx::query("UPDATE remote_grants SET armed_at=NULL WHERE account_id=? AND enabled")
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    routes::audit(
        &mut tx,
        user.id,
        None,
        "cafe_settings_changed",
        &format!(
            "网吧模式{}，连续未暂停上限 {} 小时；开启时优先于设备失联规则",
            if p.enabled { "开启" } else { "关闭" },
            p.max_hours
        ),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"ok":true,"revision":revision+1})))
}

pub async fn listing(s: &AppState, user: &SessionUser) -> ApiResult<Vec<Value>> {
    storage::json_rows(sqlx::query("SELECT a.id AS id,a.user_id AS user_id,COALESCE(u.email,u.username) AS owner,a.provider AS provider,a.label AS label,a.credential_state AS credential,COALESCE(c.enabled,false) AS enabled,COALESCE(c.max_hours,24) AS max_hours,COALESCE(c.revision,0) AS revision,c.started_at AS started_at,c.started_at+INTERVAL c.max_hours HOUR AS deadline,c.observed_at AS observed_at,CASE WHEN c.observed_at<UTC_TIMESTAMP(6)-INTERVAL 150 SECOND THEN 'unknown' ELSE COALESCE(c.observed_state,'unknown') END AS observed_state,c.next_poll AS next_poll,j.state AS job_state,j.result AS last_result FROM remote_accounts a JOIN users u ON u.id=a.user_id LEFT JOIN cafe_policies c ON c.account_id=a.id LEFT JOIN remote_jobs j ON j.account_id=a.id AND j.epoch=a.epoch AND j.trigger_kind='cafe' WHERE (? OR a.user_id=?) ORDER BY a.updated_at DESC LIMIT 500").bind(user.role=="admin").bind(user.id).fetch_all(&s.db).await?)
}

pub struct Poll {
    id: Uuid,
    lease: Uuid,
    revision: i64,
    epoch: i64,
    credential_version: i64,
    key: String,
    kind: provider::Kind,
    cipher: Vec<u8>,
}
pub async fn claim(s: &AppState) -> ApiResult<Option<Poll>> {
    let mut tx = storage::begin(&s.db).await?;
    remote::lock(&mut tx).await?;
    let row = sqlx::query("SELECT c.account_id,c.revision,a.epoch,a.credential_version,a.provider_key,a.provider,a.credential FROM cafe_policies c JOIN remote_accounts a ON a.id=c.account_id JOIN users u ON u.id=a.user_id WHERE c.enabled AND NOT u.disabled AND a.credential_state='valid' AND a.credential IS NOT NULL AND c.next_poll<=UTC_TIMESTAMP(6) AND (c.poll_until IS NULL OR c.poll_until<=UTC_TIMESTAMP(6)) AND NOT EXISTS(SELECT 1 FROM remote_jobs j WHERE j.account_id=a.id AND j.lease_until>UTC_TIMESTAMP(6)) AND (SELECT count(*) FROM remote_jobs WHERE lease_until>UTC_TIMESTAMP(6))+(SELECT count(*) FROM cafe_policies WHERE poll_until>UTC_TIMESTAMP(6))<2 ORDER BY c.next_poll LIMIT 1 FOR UPDATE SKIP LOCKED")
        .fetch_optional(&mut *tx).await?;
    let Some(r) = row else {
        return Ok(None);
    };
    let poll = Poll {
        id: r.get("account_id"),
        revision: r.get("revision"),
        epoch: r.get("epoch"),
        credential_version: r.get("credential_version"),
        lease: Uuid::new_v4(),
        key: r.get("provider_key"),
        kind: provider::Kind::parse(r.get("provider"))
            .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "未知加速器"))?,
        cipher: r.get("credential"),
    };
    sqlx::query("UPDATE cafe_policies SET poll_lease=?,poll_until=UTC_TIMESTAMP(6)+INTERVAL 60 SECOND WHERE account_id=?").bind(poll.lease).bind(poll.id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Some(poll))
}

pub async fn observe(s: &AppState, p: &Poll) -> ApiResult<()> {
    let Some(provider) = &s.provider else {
        return Ok(());
    };
    let result = match provider.open(&p.key, &p.cipher) {
        Ok(token) => provider.info(p.kind, &token).await,
        Err(e) => Err(e),
    };
    let mut tx = storage::begin(&s.db).await?;
    remote::lock(&mut tx).await?;
    let row = sqlx::query("SELECT c.started_at,c.observed_at,c.failures FROM cafe_policies c JOIN remote_accounts a ON a.id=c.account_id JOIN users u ON u.id=a.user_id WHERE c.account_id=? AND c.enabled AND c.revision=? AND c.poll_lease=? AND c.poll_until>UTC_TIMESTAMP(6) AND a.epoch=? AND a.credential_version=? AND NOT u.disabled").bind(p.id).bind(p.revision).bind(p.lease).bind(p.epoch).bind(p.credential_version).fetch_optional(&mut *tx).await?;
    let Some(r) = row else {
        sqlx::query("UPDATE cafe_policies SET poll_lease=NULL,poll_until=NULL WHERE account_id=? AND poll_lease=?").bind(p.id).bind(p.lease).execute(&mut *tx).await?;
        tx.commit().await?;
        return Ok(());
    };
    let started: Option<DateTime<Utc>> = r.get("started_at");
    let observed: Option<DateTime<Utc>> = r.get("observed_at");
    let now = Utc::now();
    let paused = match &result {
        Ok(info) if info.key == p.key => info.paused,
        _ => None,
    };
    if let Some(paused) = paused {
        // A long observation gap cannot prove this is the same uninterrupted run.
        let next = next_start(started, observed, paused, now);
        if next != started {
            storage::cancel(&mut tx, p.id).await?;
        }
        sqlx::query("UPDATE cafe_policies SET started_at=?,observed_at=UTC_TIMESTAMP(6),observed_state=?,failures=0,next_poll=UTC_TIMESTAMP(6)+INTERVAL 60 SECOND,poll_lease=NULL,poll_until=NULL,updated_at=UTC_TIMESTAMP(6) WHERE account_id=?").bind(next).bind(if paused {"paused"} else {"running"}).bind(p.id).execute(&mut *tx).await?;
    } else {
        let invalid = matches!(
            &result,
            Err(provider::Failure::Credential | provider::Failure::InvalidAccount)
        ) || matches!(&result, Ok(info) if info.key != p.key);
        if invalid {
            storage::cancel(&mut tx, p.id).await?;
            sqlx::query("UPDATE remote_accounts SET credential=NULL,credential_state='reauthorize' WHERE id=?").bind(p.id).execute(&mut *tx).await?;
        }
        let failures = r.get::<i32, _>("failures").saturating_add(1).min(10);
        let delay = (60 * (1_i32 << failures.min(3))).min(300) as f64;
        sqlx::query("UPDATE cafe_policies SET observed_state='unknown',failures=?,next_poll=UTC_TIMESTAMP(6)+INTERVAL ? SECOND,poll_lease=NULL,poll_until=NULL,updated_at=UTC_TIMESTAMP(6) WHERE account_id=?").bind(failures).bind(delay).bind(p.id).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

fn next_start(
    started: Option<DateTime<Utc>>,
    observed: Option<DateTime<Utc>>,
    paused: bool,
    now: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    if paused {
        None
    } else if observed.is_none_or(|t| now - t > chrono::Duration::minutes(5)) {
        Some(now)
    } else {
        Some(started.unwrap_or(now))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn observations_preserve_end_and_restart_a_cycle() {
        let now = Utc::now();
        let start = now - chrono::Duration::hours(23);
        assert_eq!(
            next_start(
                Some(start),
                Some(now - chrono::Duration::minutes(1)),
                false,
                now
            ),
            Some(start)
        );
        assert_eq!(next_start(Some(start), Some(now), true, now), None);
        assert_eq!(next_start(None, Some(now), false, now), Some(now));
        assert_eq!(
            next_start(
                Some(start),
                Some(now - chrono::Duration::minutes(6)),
                false,
                now
            ),
            Some(now)
        );
    }
    #[test]
    fn omitted_duration_defaults_to_twenty_four_hours() {
        let p: Settings = serde_json::from_value(json!({"enabled":true,"revision":0})).unwrap();
        assert_eq!(p.max_hours, 24);
    }
}
