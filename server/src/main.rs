mod auth;
mod devices;
mod email;
mod provider;
mod remote;
mod remote_worker;
mod routes;
mod scheduler;
use axum::{
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{postgres::PgPoolOptions, PgPool, Row};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;
use uuid::Uuid;

#[derive(Clone)]
pub struct AppState {
    db: PgPool,
    origin: String,
    secure: bool,
    cookie_name: &'static str,
    limits: Arc<Mutex<HashMap<String, (u32, Instant)>>>,
    hashes: Arc<Semaphore>,
    dummy_hash: String,
    mailer: Option<email::Mailer>,
    provider: Option<provider::Provider>,
    remote_slots: Arc<Semaphore>,
}
#[derive(Serialize, sqlx::FromRow)]
pub struct SessionUser {
    id: Uuid,
    username: String,
    display_name: String,
    role: String,
    csrf: String,
    password_enabled: bool,
}
pub struct ApiError(StatusCode, &'static str);
type ApiResult<T> = Result<T, ApiError>;
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error":self.1}))).into_response()
    }
}
impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        tracing::error!(error_kind=?e.as_database_error().map(|e|e.code()),"database operation failed");
        Self(
            StatusCode::INTERNAL_SERVER_ERROR,
            "服务暂时不可用，请稍后重试",
        )
    }
}
pub async fn event(
    s: &AppState,
    user_id: Uuid,
    device: Option<Uuid>,
    kind: &str,
    detail: &str,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO events(user_id,device_id,kind,detail) VALUES($1,$2,$3,$4)")
        .bind(user_id)
        .bind(device)
        .bind(kind)
        .bind(detail)
        .execute(&s.db)
        .await?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter("leigod_guard_server=info")
        .init();
    let db = PgPoolOptions::new()
        .max_connections(8)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&std::env::var("DATABASE_URL")?)
        .await?;
    sqlx::migrate!().run(&db).await?;
    if std::env::args().nth(1).as_deref() == Some("create-admin") {
        let username = std::env::var("ADMIN_USERNAME")?.to_lowercase();
        let password = std::env::var("ADMIN_PASSWORD")?;
        if !auth::valid_username(&username) || !auth::valid_password(&password) {
            return Err("invalid administrator credentials".into());
        }
        let hash = auth::hash_password(&password)?;
        sqlx::query("INSERT INTO users(id,username,display_name,password_hash,role) VALUES($1,$2,'管理员',$3,'admin')")
            .bind(Uuid::new_v4()).bind(username).bind(hash).execute(&db).await?;
        println!("Administrator created.");
        return Ok(());
    }
    let origin = std::env::var("PUBLIC_ORIGIN")?;
    let secure = origin.starts_with("https://");
    if !secure && !origin.starts_with("http://127.0.0.1:") {
        return Err("HTTPS origin required".into());
    }
    let mailer = email::Mailer::from_env(&origin)?;
    let provider = provider::Provider::from_env(&origin)?;
    let state = AppState {
        provider,
        remote_slots: Arc::new(Semaphore::new(2)),
        mailer,
        db,
        origin,
        secure,
        cookie_name: if secure {
            "__Host-guard_session"
        } else {
            "guard_session"
        },
        limits: Default::default(),
        hashes: Arc::new(Semaphore::new(2)),
        dummy_hash: auth::hash_password(&auth::secret())?,
    };
    let api = Router::new()
        .route("/health", get(routes::health))
        .route("/device/remote", get(remote::status))
        .route("/device/remote/authorize", post(remote::authorize))
        .route("/device/remote/disable", post(remote::disable))
        .route("/devices/{id}/remote/disable", post(remote::web_disable))
        .route("/remote", get(remote::listing))
        .route("/admin/remote/acknowledge", post(remote::acknowledge))
        .route("/login", post(routes::login))
        .route("/email/code", post(email::send_code))
        .route("/email/login", post(email::verify_code))
        .route("/devices/register", post(devices::register))
        .route("/logout", post(routes::logout))
        .route("/me", get(routes::me))
        .route("/password", post(routes::password))
        .route("/dashboard", get(routes::dashboard))
        .route("/devices", get(routes::devices))
        .route("/devices/{id}/revoke", post(routes::revoke))
        .route("/devices/{id}/unbind", post(devices::forget))
        .route("/pairings", post(routes::pairing))
        .route("/device/pair", post(routes::pair_device))
        .route("/device/heartbeat", post(routes::heartbeat))
        .route("/events", get(routes::events))
        .route("/admin/overview", get(routes::overview))
        .route("/admin/users", get(routes::users).post(routes::create_user))
        .route("/admin/users/{id}/status", post(routes::user_status))
        .route("/admin/devices", get(routes::all_devices))
        .route("/admin/events", get(routes::all_events));
    let static_dir = std::env::var("STATIC_DIR").unwrap_or("static".into());
    let app = Router::new()
        .nest("/api", api)
        .fallback_service(tower_http::services::ServeDir::new(static_dir))
        .layer(DefaultBodyLimit::max(8192))
        .layer(tower_http::set_header::SetResponseHeaderLayer::overriding(
            axum::http::header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ))
        .layer(tower_http::set_header::SetResponseHeaderLayer::overriding(
            axum::http::header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .with_state(state.clone());
    tokio::spawn(remote_worker::run(state.clone()));
    tokio::spawn(scheduler::run(state));
    let port = std::env::var("PORT").unwrap_or("3088".into());
    let address: std::net::IpAddr = std::env::var("LISTEN_ADDRESS")
        .unwrap_or_else(|_| "127.0.0.1".into())
        .parse()?;
    let listener = tokio::net::TcpListener::bind((address, port.parse::<u16>()?)).await?;
    tracing::info!(%address, %port, "guard server listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
