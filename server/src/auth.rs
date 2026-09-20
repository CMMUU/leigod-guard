use crate::*;
use argon2::{
    password_hash::{PasswordHash, SaltString},
    Argon2, PasswordHasher, PasswordVerifier,
};
use rand::{rngs::OsRng, RngCore};
use sha2::{Digest, Sha256};

pub fn secret() -> String {
    let mut b = [0u8; 32];
    OsRng.fill_bytes(&mut b);
    hex::encode(b)
}
pub fn digest(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}
pub fn valid_password(s: &str) -> bool {
    s.len() >= 12 && s.len() <= 128
}
pub fn valid_username(s: &str) -> bool {
    (3..=100).contains(&s.len())
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"@._-+".contains(&c))
}
pub fn hash_password(p: &str) -> Result<String, String> {
    Argon2::default()
        .hash_password(p.as_bytes(), &SaltString::generate(&mut OsRng))
        .map(|h| h.to_string())
        .map_err(|_| "password hashing failed".into())
}
pub fn check_password(p: &str, hash: &str) -> bool {
    PasswordHash::new(hash)
        .is_ok_and(|h| Argon2::default().verify_password(p.as_bytes(), &h).is_ok())
}
pub fn same_origin(s: &AppState, h: &HeaderMap) -> ApiResult<()> {
    if h.get("origin").and_then(|v| v.to_str().ok()) != Some(s.origin.as_str()) {
        return Err(ApiError(StatusCode::FORBIDDEN, "请求来源不匹配"));
    }
    Ok(())
}
pub fn cookie_token(s: &AppState, h: &HeaderMap) -> Option<String> {
    h.get("cookie")?.to_str().ok()?.split(';').find_map(|item| {
        let (k, v) = item.trim().split_once('=')?;
        (k == s.cookie_name && v.len() == 64).then(|| v.to_owned())
    })
}
pub async fn user(s: &AppState, h: &HeaderMap, write: bool) -> ApiResult<SessionUser> {
    let token = cookie_token(s, h).ok_or(ApiError(StatusCode::UNAUTHORIZED, "请先登录"))?;
    let u=sqlx::query_as::<_,SessionUser>("SELECT u.id,u.username,u.display_name,u.role,s.csrf FROM sessions s JOIN users u ON u.id=s.user_id WHERE s.token_hash=$1 AND s.expires_at>now() AND NOT u.disabled")
        .bind(digest(&token)).fetch_optional(&s.db).await?.ok_or(ApiError(StatusCode::UNAUTHORIZED,"会话已过期，请重新登录"))?;
    if write {
        same_origin(s, h)?;
        if h.get("x-csrf-token").and_then(|v| v.to_str().ok()) != Some(u.csrf.as_str()) {
            return Err(ApiError(
                StatusCode::FORBIDDEN,
                "操作校验失败，请刷新后重试",
            ));
        }
    }
    sqlx::query("UPDATE sessions SET last_seen=now() WHERE token_hash=$1 AND last_seen<now()-interval '30 seconds'").bind(digest(&token)).execute(&s.db).await?;
    Ok(u)
}
pub async fn admin(s: &AppState, h: &HeaderMap, write: bool) -> ApiResult<SessionUser> {
    let u = user(s, h, write).await?;
    if u.role != "admin" {
        return Err(ApiError(StatusCode::FORBIDDEN, "需要管理员权限"));
    }
    Ok(u)
}
pub fn throttle(s: &AppState, key: String, limit: u32, window: u64) -> ApiResult<()> {
    let now = std::time::Instant::now();
    let mut map = s
        .limits
        .lock()
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "稍后重试"))?;
    map.retain(|_, (_, expires)| *expires > now);
    if map.len() >= 10000 && !map.contains_key(&key) {
        return Err(ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "请求过多，请稍后再试",
        ));
    }
    let entry = map
        .entry(key)
        .or_insert((0, now + std::time::Duration::from_secs(window)));
    entry.0 += 1;
    if entry.0 > limit {
        return Err(ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "请求过多，请稍后再试",
        ));
    }
    Ok(())
}
pub fn client_key(h: &HeaderMap) -> String {
    // Service binds loopback; the trusted local reverse proxy overwrites X-Real-IP.
    h.get("x-real-ip")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("local")
        .chars()
        .take(64)
        .collect()
}
pub fn session_cookie(s: &AppState, value: &str, seconds: u32) -> String {
    format!(
        "{}={}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}{}",
        s.cookie_name,
        value,
        seconds,
        if s.secure { "; Secure" } else { "" }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn password_hash_roundtrip() {
        let h = hash_password("test-password-123!").unwrap();
        assert!(check_password("test-password-123!", &h));
        assert!(!check_password("wrong", &h));
        assert!(!h.contains("test-password"));
    }
    #[test]
    fn credential_bounds() {
        assert!(!valid_password("short"));
        assert!(!valid_password(&"x".repeat(129)));
        assert!(valid_username("owner@example.com"));
        assert!(!valid_username("<script>"));
        assert!(!valid_username("a b"));
    }
    #[test]
    fn tokens_are_distinct() {
        let a = secret();
        let b = secret();
        assert_ne!(a, b);
        assert_eq!(a.len(), 64);
        assert_ne!(digest(&a), a);
    }
}
