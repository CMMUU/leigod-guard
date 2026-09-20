use crate::*;
pub async fn run(s: AppState) {
    let mut interval = tokio::time::interval(Duration::from_secs(15));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        if tick(&s).await.is_err() {
            tracing::error!("observation scheduler failed");
        }
    }
}
async fn tick(s: &AppState) -> ApiResult<()> {
    let mut tx = s.db.begin().await?;
    let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(73940127)")
        .fetch_one(&mut *tx)
        .await?;
    if !locked {
        return Ok(());
    }
    // No credentials or pause calls exist in this release: offline is an observation only.
    sqlx::query("WITH expired AS (UPDATE devices SET observed_offline=true WHERE NOT revoked AND NOT observed_offline AND last_seen<now()-interval '120 seconds' RETURNING id,user_id) INSERT INTO events(user_id,device_id,kind,detail) SELECT user_id,id,'device_offline','设备超过 120 秒未上报；仅记录观察，不执行暂停' FROM expired").execute(&mut *tx).await?;
    sqlx::query("INSERT INTO metrics(bucket,online_devices,online_users,web_users) SELECT date_trunc('minute',now()),(SELECT count(*) FROM devices WHERE NOT revoked AND last_seen>now()-interval '45 seconds'),(SELECT count(DISTINCT user_id) FROM devices WHERE NOT revoked AND last_seen>now()-interval '45 seconds'),(SELECT count(DISTINCT user_id) FROM sessions WHERE expires_at>now() AND last_seen>now()-interval '5 minutes') ON CONFLICT(bucket) DO UPDATE SET online_devices=EXCLUDED.online_devices,online_users=EXCLUDED.online_users,web_users=EXCLUDED.web_users").execute(&mut *tx).await?;
    sqlx::query("DELETE FROM sessions WHERE expires_at<now()")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM pairing_codes WHERE expires_at<now()")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM email_challenges WHERE expires_at<now()")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM email_rate_limits WHERE expires_at<now()")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM metrics WHERE bucket<now()-interval '7 days'")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM events WHERE id IN (SELECT id FROM events WHERE created_at<now()-interval '30 days' ORDER BY created_at LIMIT 1000)").execute(&mut *tx).await?;
    sqlx::query("INSERT INTO service_state(key,updated_at) VALUES('scheduler',now()) ON CONFLICT(key) DO UPDATE SET updated_at=EXCLUDED.updated_at").execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}
