//! 外星仔 PC 时长接口。只访问官方 HTTPS 主机，不调用加速、奖励或广告接口。
//! 协议依据与首次校准限制见 docs/外星仔接入说明.md。
use prost::Message;
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};

const HOST: &str = "https://api.et-api.com";
const REMAIN: &str = "/v2/account/remain/duration";
const PAUSE: &str = "/v2/account/update/pause/state";

#[derive(Clone, PartialEq, Message)]
struct LoginRequest {
    #[prost(string, tag = "1")]
    phone_number: String,
    #[prost(string, tag = "2")]
    password: String,
}

#[derive(Clone, PartialEq, Message)]
struct LoginResponse {
    #[prost(int64, tag = "1")]
    user_id: i64,
    #[prost(string, tag = "2")]
    authorization: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct AccountInfo {
    #[prost(int64, tag = "1")]
    pub vip_duration_second: i64,
    #[prost(int64, tag = "2")]
    pub free_duration_second: i64,
    #[prost(int64, tag = "3")]
    pub timestamp: i64,
    // Presence matters: a missing/changed field must never become "paused".
    #[prost(int64, optional, tag = "4")]
    pub pause_state: Option<i64>,
}

#[derive(Clone, PartialEq, Message)]
struct PauseRequest {
    #[prost(int64, tag = "1")]
    pause_state: i64,
}

pub fn device_id() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).map_err(|_| "无法生成设备标识".to_string())?;
    Ok(bytes.iter().map(|b| format!("{b:02X}")).collect())
}

fn signed_url(path: &str, nonce: &str, timestamp: i64) -> String {
    let query = format!("nonce={nonce}&ts={timestamp}&ver=2023-08-28");
    let sign = Sha256::digest(format!("POSTapi.et-api.com{path}?{query}").as_bytes());
    format!("{HOST}{path}?{query}&sig={sign:x}")
}

fn request(
    path: &str,
    device: &str,
    token: &str,
    body: Vec<u8>,
    timeout: Duration,
) -> Result<Vec<u8>, String> {
    if timeout.is_zero() || device.len() != 32 || !device.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("设备标识无效或请求已超时，请重新登录".into());
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(timeout)
        .connect_timeout(timeout.min(Duration::from_secs(5)))
        .redirect(reqwest::redirect::Policy::none())
        .https_only(true)
        .build()
        .map_err(|_| "无法建立外星仔连接".to_string())?;
    let mut req = client
        .post(signed_url(
            path,
            &device_id()?,
            chrono::Utc::now().timestamp(),
        ))
        .header("Content-Type", "application/x-protobuf")
        .header("Accept", "application/x-protobuf")
        .header("x-eta", format!("os=1&ver=1.26.11&dvc={device}&ch=default"))
        .body(body);
    if !token.is_empty() {
        req = req.header("Authorization", token);
    }
    let response = req
        .send()
        .map_err(|_| "外星仔请求失败或超时，请检查网络".to_string())?;
    if !response.status().is_success() {
        // Never display raw error bodies, URLs, credentials or tokens.
        return Err(format!(
            "外星仔返回 HTTP {}；请检查账号或重新登录",
            response.status().as_u16()
        ));
    }
    if response.content_length().is_some_and(|len| len > 65536) {
        return Err("外星仔响应过大".into());
    }
    use std::io::Read;
    let mut bytes = Vec::new();
    response
        .take(65537)
        .read_to_end(&mut bytes)
        .map_err(|_| "无法读取外星仔响应".to_string())?;
    if bytes.len() > 65536 {
        return Err("外星仔响应过大".into());
    }
    Ok(bytes)
}

pub fn login(phone: &str, password: &str, device: &str) -> Result<String, String> {
    let phone = phone.trim();
    let phone = if phone.starts_with('+') {
        phone.to_string()
    } else {
        format!("+86{phone}")
    };
    if password.is_empty() || phone.len() < 8 || !phone[1..].bytes().all(|b| b.is_ascii_digit()) {
        return Err("请输入正确的手机号与密码".into());
    }
    let data = request(
        "/v2/account/login",
        device,
        "",
        LoginRequest {
            phone_number: phone,
            password: password.into(),
        }
        .encode_to_vec(),
        Duration::from_secs(12),
    )?;
    let result =
        LoginResponse::decode(data.as_slice()).map_err(|_| "登录响应格式发生变化".to_string())?;
    if result.user_id <= 0 || result.authorization.is_empty() {
        return Err("登录未返回有效凭据".into());
    }
    Ok(result.authorization)
}

pub fn info(token: &str, device: &str) -> Result<AccountInfo, String> {
    read_info(token, device, Duration::from_secs(12))
}

fn read_info(token: &str, device: &str, timeout: Duration) -> Result<AccountInfo, String> {
    if token.is_empty() {
        return Err("请先登录外星仔账号".into());
    }
    let bytes = request(REMAIN, device, token, Vec::new(), timeout)?;
    let info =
        AccountInfo::decode(bytes.as_slice()).map_err(|_| "时长响应格式发生变化".to_string())?;
    if info.timestamp <= 0 || info.vip_duration_second < 0 || info.free_duration_second < 0 {
        return Err("未取得有效的外星仔计时状态".into());
    }
    Ok(info)
}

/// Do not guess the official pause enum. The user first pauses in the official
/// client, then explicitly reads that state. Re-login invalidates calibration.
pub fn calibrated_state(info: &AccountInfo) -> Result<i64, String> {
    info.pause_state
        .filter(|state| *state > 0)
        .ok_or_else(|| "当前未返回可确认的暂停状态；请先在外星仔官方客户端暂停并刷新后重试".into())
}

/// The guard runs after the slow read and immediately before the mutating call.
/// Successful HTTP alone does not prove the time is paused: always read back.
pub fn pause(
    token: &str,
    device: &str,
    paused_state: i64,
    timeout: Duration,
    guard: impl FnOnce() -> Result<(), String>,
) -> Result<AccountInfo, String> {
    pause_checked(
        paused_state,
        timeout,
        guard,
        |remaining| read_info(token, device, remaining),
        |state, remaining| {
            request(
                PAUSE,
                device,
                token,
                PauseRequest { pause_state: state }.encode_to_vec(),
                remaining,
            )
            .map(|_| ())
        },
    )
}

fn pause_checked(
    paused_state: i64,
    timeout: Duration,
    guard: impl FnOnce() -> Result<(), String>,
    mut read: impl FnMut(Duration) -> Result<AccountInfo, String>,
    write: impl FnOnce(i64, Duration) -> Result<(), String>,
) -> Result<AccountInfo, String> {
    if paused_state <= 0 || timeout.is_zero() {
        return Err("请先完成官方暂停状态校准，或重试超时请求".into());
    }
    let deadline = Instant::now() + timeout;
    let current = read(deadline.saturating_duration_since(Instant::now()))?;
    guard()?;
    if current.pause_state == Some(paused_state) {
        return Ok(current);
    }
    if current.pause_state.is_some_and(|state| state != 0) {
        return Err("外星仔返回未知计时状态，已暂缓暂停，请重新校准".into());
    }
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err("查询超时，未发送暂停请求".into());
    }
    write(paused_state, remaining)?;
    let confirmed = read(deadline.saturating_duration_since(Instant::now()))?;
    if confirmed.pause_state != Some(paused_state) {
        return Err("暂停请求已发送，但官方状态尚未确认；请在官方客户端核对".into());
    }
    Ok(confirmed)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pause_fails_closed_and_reads_back_after_write() {
        use std::cell::Cell;
        let writes = Cell::new(0);
        let reads = Cell::new(0);
        let read = |_: Duration| {
            reads.set(reads.get() + 1);
            Ok(AccountInfo {
                pause_state: Some(0),
                ..Default::default()
            })
        };
        assert!(pause_checked(
            1,
            Duration::from_secs(1),
            || Err("game started during query".into()),
            read,
            |_, _| {
                writes.set(writes.get() + 1);
                Ok(())
            }
        )
        .is_err());
        assert_eq!(reads.get(), 1);
        assert_eq!(writes.get(), 0);
        assert!(pause_checked(
            1,
            Duration::from_secs(1),
            || Ok(()),
            read,
            |_, _| {
                writes.set(writes.get() + 1);
                Ok(())
            }
        )
        .is_err());
        assert_eq!(writes.get(), 1);
        assert_eq!(reads.get(), 3);
        let mut states = [Some(0), Some(2)].into_iter();
        assert!(pause_checked(
            2,
            Duration::from_secs(1),
            || Ok(()),
            |_| Ok(AccountInfo {
                pause_state: states.next().unwrap(),
                ..Default::default()
            }),
            |state, _| {
                assert_eq!(state, 2);
                Ok(())
            }
        )
        .is_ok());
    }

    #[test]
    fn already_paused_is_idempotent_and_unknown_state_is_never_mutated() {
        for state in [1, 9, -1] {
            let result = pause_checked(
                1,
                Duration::from_secs(1),
                || Ok(()),
                |_| {
                    Ok(AccountInfo {
                        pause_state: Some(state),
                        ..Default::default()
                    })
                },
                |_, _| panic!("must not send a state update"),
            );
            assert_eq!(result.is_ok(), state == 1);
        }
    }

    #[test]
    fn calibration_rejects_default_missing_and_negative_states() {
        for state in [None, Some(0), Some(-1)] {
            assert!(calibrated_state(&AccountInfo {
                pause_state: state,
                ..Default::default()
            })
            .is_err());
        }
        assert_eq!(
            calibrated_state(&AccountInfo {
                pause_state: Some(2),
                ..Default::default()
            })
            .unwrap(),
            2
        );
        assert_eq!(PauseRequest { pause_state: 2 }.encode_to_vec(), vec![8, 2]);
    }
    #[test]
    fn protobuf_fixture_reads_pc_time_and_state() {
        let value = AccountInfo::decode(&[8, 120, 16, 60, 24, 100, 32, 1][..]).unwrap();
        assert_eq!(value.vip_duration_second, 120);
        assert_eq!(value.free_duration_second, 60);
        assert_eq!(value.pause_state, Some(1));
    }
}
