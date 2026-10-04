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
    let mut tx = storage::begin(&s.db).await?;
    // Shared transaction coordinator keeps observation and consent mutations ordered.
    sqlx::query("INSERT INTO events(user_id,device_id,kind,detail) SELECT user_id,id,'device_offline','设备超过 120 秒未上报；远程暂停取决于单独授权及账号全部设备状态' FROM devices WHERE NOT revoked AND NOT observed_offline AND last_seen<UTC_TIMESTAMP(6)-INTERVAL 120 SECOND").execute(&mut *tx).await?;
    sqlx::query("UPDATE devices SET observed_offline=true WHERE NOT revoked AND NOT observed_offline AND last_seen<UTC_TIMESTAMP(6)-INTERVAL 120 SECOND").execute(&mut *tx).await?;
    sqlx::query("INSERT INTO metrics(bucket,online_devices,online_users,web_users) SELECT CAST(DATE_FORMAT(UTC_TIMESTAMP(6),'%Y-%m-%d %H:%i:00') AS DATETIME),(SELECT count(*) FROM devices WHERE NOT revoked AND last_seen>UTC_TIMESTAMP(6)-INTERVAL 45 SECOND),(SELECT count(DISTINCT user_id) FROM devices WHERE NOT revoked AND last_seen>UTC_TIMESTAMP(6)-INTERVAL 45 SECOND),(SELECT count(DISTINCT user_id) FROM sessions WHERE expires_at>UTC_TIMESTAMP(6) AND last_seen>UTC_TIMESTAMP(6)-INTERVAL 5 MINUTE) ON DUPLICATE KEY UPDATE online_devices=VALUES(online_devices),online_users=VALUES(online_users),web_users=VALUES(web_users)").execute(&mut *tx).await?;
    sqlx::query("DELETE FROM sessions WHERE expires_at<UTC_TIMESTAMP(6)")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM pairing_codes WHERE expires_at<UTC_TIMESTAMP(6)")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM email_challenges WHERE expires_at<UTC_TIMESTAMP(6)")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM email_binding_challenges WHERE expires_at<UTC_TIMESTAMP(6)")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM email_rate_limits WHERE expires_at<UTC_TIMESTAMP(6)")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM metrics WHERE bucket<UTC_TIMESTAMP(6)-INTERVAL 7 DAY")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM events WHERE created_at<UTC_TIMESTAMP(6)-INTERVAL 30 DAY ORDER BY created_at LIMIT 1000").execute(&mut *tx).await?;
    sqlx::query("INSERT INTO service_state(`key`,updated_at) VALUES('scheduler',UTC_TIMESTAMP(6)) ON DUPLICATE KEY UPDATE updated_at=VALUES(updated_at)").execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}
