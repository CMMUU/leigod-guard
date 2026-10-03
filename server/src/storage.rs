//! MySQL transaction boundary. All authoritative mutations take the coordinator
//! row first. Never hold this transaction across an external HTTP request.
use crate::*;
use sqlx::mysql::{MySqlConnectOptions, MySqlPoolOptions, MySqlRow, MySqlSslMode};
use sqlx::{Column, MySql, MySqlPool, Transaction, TypeInfo, ValueRef};
use std::str::FromStr;

pub async fn connect(url: &str) -> Result<MySqlPool, Box<dyn std::error::Error>> {
    let options = MySqlConnectOptions::from_str(url)?;
    let local = matches!(options.get_host(), "localhost" | "127.0.0.1" | "::1")
        || (options.get_host() == "mysql"
            && std::env::var("MYSQL_COMPOSE_NETWORK").as_deref() == Ok("true"));
    let mut options = options.ssl_mode(if local {
        MySqlSslMode::Preferred
    } else {
        MySqlSslMode::VerifyIdentity
    });
    if let Ok(ca) = std::env::var("MYSQL_SSL_CA") {
        options = options.ssl_ca(ca);
    }
    Ok(MySqlPoolOptions::new()
        .max_connections(8)
        .acquire_timeout(Duration::from_secs(5))
        .after_connect(|conn, _| {
            Box::pin(async move {
                sqlx::query("SET time_zone = '+00:00'")
                    .execute(&mut *conn)
                    .await?;
                sqlx::query("SET SESSION TRANSACTION ISOLATION LEVEL READ COMMITTED")
                    .execute(&mut *conn)
                    .await?;
                sqlx::query("SET SESSION innodb_lock_wait_timeout = 5")
                    .execute(&mut *conn)
                    .await?;
                Ok(())
            })
        })
        .connect_with(options)
        .await?)
}

pub async fn lock(tx: &mut Transaction<'_, MySql>) -> ApiResult<()> {
    sqlx::query("SELECT id FROM guard_lock WHERE id=1 FOR UPDATE")
        .fetch_one(&mut **tx)
        .await?;
    Ok(())
}

pub async fn begin(db: &MySqlPool) -> ApiResult<Transaction<'_, MySql>> {
    let mut tx = db.begin().await?;
    lock(&mut tx).await?;
    Ok(tx)
}

pub async fn cancel(tx: &mut Transaction<'_, MySql>, account: Uuid) -> ApiResult<()> {
    sqlx::query("UPDATE remote_accounts SET epoch=epoch+1,updated_at=UTC_TIMESTAMP(6) WHERE id=?")
        .bind(account)
        .execute(&mut **tx)
        .await?;
    sqlx::query("UPDATE remote_jobs SET state='cancelled',result='authorization_or_heartbeat_changed',updated_at=UTC_TIMESTAMP(6) WHERE account_id=? AND state IN ('queued','running')").bind(account).execute(&mut **tx).await?;
    sqlx::query("UPDATE remote_accounts SET credential=NULL,credential_state='deleted',credential_version=credential_version+1 WHERE id=? AND NOT EXISTS(SELECT 1 FROM remote_grants WHERE account_id=? AND enabled) AND NOT EXISTS(SELECT 1 FROM cafe_policies WHERE account_id=? AND enabled)").bind(account).bind(account).bind(account).execute(&mut **tx).await?;
    Ok(())
}

pub async fn revoke_provider(
    tx: &mut Transaction<'_, MySql>,
    device: Uuid,
    kind: provider::Kind,
) -> ApiResult<()> {
    let column = kind.revision_column();
    sqlx::query(&format!(
        "UPDATE devices SET {column}={column}+1 WHERE id=?"
    ))
    .bind(device)
    .execute(&mut **tx)
    .await?;
    let account: Option<Uuid> =
        sqlx::query_scalar("SELECT account_id FROM remote_grants WHERE device_id=? AND provider=?")
            .bind(device)
            .bind(kind.as_str())
            .fetch_optional(&mut **tx)
            .await?;
    sqlx::query(&format!("UPDATE remote_grants g JOIN devices d ON d.id=g.device_id SET g.enabled=false,g.revision=d.{column},g.armed_at=NULL,g.updated_at=UTC_TIMESTAMP(6) WHERE g.device_id=? AND g.provider=?")).bind(device).bind(kind.as_str()).execute(&mut **tx).await?;
    if let Some(account) = account {
        cancel(tx, account).await?;
    }
    Ok(())
}

pub async fn revoke_device(tx: &mut Transaction<'_, MySql>, id: Uuid) -> ApiResult<()> {
    revoke_provider(tx, id, provider::Kind::Leigod).await?;
    revoke_provider(tx, id, provider::Kind::Etalien).await
}

pub async fn cancel_device_account(
    tx: &mut Transaction<'_, MySql>,
    id: Uuid,
    kind: provider::Kind,
) -> ApiResult<()> {
    let accounts: Vec<Uuid> = sqlx::query_scalar("SELECT g.account_id FROM remote_grants g WHERE g.device_id=? AND g.provider=? AND g.enabled AND NOT EXISTS(SELECT 1 FROM cafe_policies c WHERE c.account_id=g.account_id AND c.enabled)").bind(id).bind(kind.as_str()).fetch_all(&mut **tx).await?;
    for account in accounts {
        cancel(tx, account).await?;
    }
    Ok(())
}

pub async fn disable_user(tx: &mut Transaction<'_, MySql>, uid: Uuid) -> ApiResult<()> {
    sqlx::query("UPDATE cafe_policies c JOIN remote_accounts a ON a.id=c.account_id SET c.enabled=false,c.revision=c.revision+1,c.started_at=NULL,c.observed_state='unknown',c.poll_lease=NULL,c.poll_until=NULL,c.updated_at=UTC_TIMESTAMP(6) WHERE a.user_id=?").bind(uid).execute(&mut **tx).await?;
    let devices: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM devices WHERE user_id=? ORDER BY id")
            .bind(uid)
            .fetch_all(&mut **tx)
            .await?;
    for id in devices {
        revoke_device(tx, id).await?;
    }
    let accounts: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM remote_accounts WHERE user_id=? ORDER BY id")
            .bind(uid)
            .fetch_all(&mut **tx)
            .await?;
    for id in accounts {
        cancel(tx, id).await?;
    }
    sqlx::query("UPDATE remote_accounts SET credential=NULL,credential_state='deleted',credential_version=credential_version+1 WHERE user_id=?").bind(uid).execute(&mut **tx).await?;
    Ok(())
}

/// API projection only: queries explicitly select public columns, never `*`.
/// Preserve JSON booleans, UUID strings and UTC timestamps across DB engines.
pub fn row_json(row: MySqlRow) -> ApiResult<Value> {
    let mut out = serde_json::Map::new();
    for col in row.columns() {
        let i = col.ordinal();
        let name = col.name();
        let raw = row.try_get_raw(i)?;
        let value = if raw.is_null() {
            Value::Null
        } else if matches!(
            name,
            "enabled" | "disabled" | "cafe_enabled" | "password_enabled"
        ) {
            json!(row.try_get::<bool, _>(i)?)
        } else {
            match col.type_info().name() {
                "BINARY" | "VARBINARY"
                    if matches!(name, "id" | "user_id" | "device_id" | "account_id") =>
                {
                    json!(row.try_get::<Uuid, _>(i)?)
                }
                "TIMESTAMP" | "DATETIME" => json!(row.try_get::<DateTime<Utc>, _>(i)?),
                "TINYINT" | "SMALLINT" | "INT" | "BIGINT" | "MEDIUMINT" => {
                    json!(row.try_get::<i64, _>(i)?)
                }
                "TINYINT UNSIGNED" | "SMALLINT UNSIGNED" | "INT UNSIGNED" | "BIGINT UNSIGNED"
                | "MEDIUMINT UNSIGNED" => json!(row.try_get::<u64, _>(i)?),
                _ => json!(row.try_get::<String, _>(i)?),
            }
        };
        out.insert(name.into(), value);
    }
    Ok(Value::Object(out))
}
pub fn json_rows(rows: Vec<MySqlRow>) -> ApiResult<Vec<Value>> {
    rows.into_iter().map(row_json).collect()
}
