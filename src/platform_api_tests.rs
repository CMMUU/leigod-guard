use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

fn fixture_user() -> User {
    User {
        id: "user-1".into(),
        username: "demo".into(),
        display_name: "演示用户".into(),
        role: "user".into(),
        csrf: "b".repeat(64),
    }
}
fn mock(
    responses: Vec<(u16, String, String)>,
) -> (Api, Arc<Mutex<Vec<String>>>, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let captured = Arc::new(Mutex::new(Vec::new()));
    let requests = captured.clone();
    let task = std::thread::spawn(move || {
        for (status, extra, body) in responses {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut bytes = Vec::new();
            let mut buf = [0u8; 1024];
            loop {
                let n = socket.read(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
                bytes.extend_from_slice(&buf[..n]);
                if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..end]);
                    let len = headers
                        .lines()
                        .filter_map(|s| s.split_once(':'))
                        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                        .map(|(_, v)| v.trim().parse::<usize>().unwrap())
                        .unwrap_or(0);
                    if bytes.len() >= end + 4 + len {
                        break;
                    }
                }
            }
            requests
                .lock()
                .unwrap()
                .push(String::from_utf8(bytes).unwrap());
            write!(socket,"HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n{extra}\r\n{body}", body.len()).unwrap();
        }
    });
    (
        Api::build(&origin, Duration::from_secs(3)).unwrap(),
        captured,
        task,
    )
}
fn cookie() -> String {
    format!("Set-Cookie: {COOKIE_NAME}={}; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age=86400\r\n", "a".repeat(64))
}
fn ok(body: &str) -> (u16, String, String) {
    (200, String::new(), body.into())
}
#[test]
fn login_validate_logout_use_fixed_origin_cookie_and_csrf() {
    let user = serde_json::to_string(&fixture_user()).unwrap();
    let (api, requests, thread) = mock(vec![
        (200, cookie(), "{\"ok\":true}".into()),
        ok(&user),
        ok(&user),
        ok("{\"ok\":true}"),
    ]);
    let session = api.login(" demo ", "password-fixture-123", false).unwrap();
    assert_eq!(session.user.username, "demo");
    let restored = api.validate(&session).unwrap();
    assert_eq!(restored.expires_at, session.expires_at); // validation cannot extend server TTL
    api.logout(&restored).unwrap();
    thread.join().unwrap();
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 4);
    assert!(requests[0].contains("POST /api/login"));
    assert!(requests[0].contains(&format!("origin: {}", api.origin)));
    assert!(!requests[0].contains("cookie:"));
    for request in &requests[1..] {
        assert!(request.contains(&format!("cookie: {COOKIE_NAME}={}", "a".repeat(64))));
        assert!(!request.contains("password-fixture"));
    }
    assert!(requests[3].contains(&format!("x-csrf-token: {}", "b".repeat(64))));
}
#[test]
fn authentication_network_and_service_errors_stay_distinct() {
    for (status, expected) in [
        (401, Error::Unauthorized),
        (403, Error::Forbidden),
        (429, Error::RateLimited),
        (503, Error::Unavailable),
    ] {
        let (api, _, thread) = mock(vec![(
            status,
            String::new(),
            "{\"error\":\"untrusted echoed secret\"}".into(),
        )]);
        assert_eq!(
            api.login("demo", "some-password", false).err(),
            Some(expected)
        );
        thread.join().unwrap();
        assert!(!expected.message().contains("secret"));
    }
}
#[test]
fn redirects_are_not_followed_with_credentials() {
    let (api, requests, thread) = mock(vec![(
        307,
        "Location: http://127.0.0.1:1/steal\r\n".into(),
        String::new(),
    )]);
    assert_eq!(
        api.login("demo", "some-password", false).err(),
        Some(Error::Protocol)
    );
    thread.join().unwrap();
    assert_eq!(requests.lock().unwrap().len(), 1);
}
#[test]
fn malformed_cookie_identity_and_oversized_body_are_rejected() {
    for header in [
        cookie().replace("Secure; ", ""),
        cookie().replace("Path=/;", "Path=/; Domain=example.com;"),
        cookie().replace("Max-Age=86400", "Max-Age=2592001"),
        cookie().replace(&"a".repeat(64), "bad"),
    ] {
        let (api, _, thread) = mock(vec![(200, header, "{\"ok\":true}".into())]);
        assert_eq!(
            api.login("demo", "password", false).err(),
            Some(Error::Protocol)
        );
        thread.join().unwrap();
    }
    for body in ["{}".to_owned(), "x".repeat(MAX_RESPONSE as usize + 1)] {
        let (api, _, thread) = mock(vec![(200, cookie(), "{\"ok\":true}".into()), ok(&body)]);
        assert_eq!(
            api.login("demo", "password", false).err(),
            Some(Error::Protocol)
        );
        thread.join().unwrap();
    }
}
#[test]
fn foreign_expired_or_corrupt_sessions_are_rejected_before_network() {
    let (api, requests, thread) = mock(vec![]);
    thread.join().unwrap();
    let base = Session {
        origin: api.origin.clone(),
        token: "a".repeat(64),
        user: fixture_user(),
        expires_at: chrono::Utc::now().timestamp() + 3600,
    };
    let mut foreign = base.clone();
    foreign.origin = "https://other.invalid".into();
    assert_eq!(api.validate(&foreign).err(), Some(Error::Unauthorized));
    let mut expired = base.clone();
    expired.expires_at = 0;
    assert_eq!(api.validate(&expired).err(), Some(Error::Unauthorized));
    let mut corrupt = base;
    corrupt.token = "bad\r\nHeader: value".into();
    assert_eq!(api.validate(&corrupt).err(), Some(Error::Unauthorized));
    assert!(requests.lock().unwrap().is_empty());
}
#[test]
fn input_bounds_and_revoked_logout_are_handled_without_echoing_passwords() {
    assert!(!Api::valid_input("ab", "password"));
    assert!(!Api::valid_input("demo", &"密".repeat(50)));
    assert!(!Api::valid_input("<bad>", "password"));
    let (api, _, thread) = mock(vec![(401, String::new(), "{}".into())]);
    let session = Session {
        origin: api.origin.clone(),
        token: "a".repeat(64),
        user: fixture_user(),
        expires_at: chrono::Utc::now().timestamp() + 3600,
    };
    assert_eq!(api.logout(&session), Ok(()));
    thread.join().unwrap();
}

#[test]
fn transport_timeout_returns_network_error_without_waiting_for_the_server() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let thread = std::thread::spawn(move || {
        let (_socket, _) = listener.accept().unwrap();
        std::thread::sleep(Duration::from_millis(250));
    });
    let api = Api::build(&origin, Duration::from_millis(50)).unwrap();
    assert_eq!(
        api.login("demo", "some-password", false).err(),
        Some(Error::Network)
    );
    thread.join().unwrap();
}

#[test]
fn email_login_thirty_day_cookie_and_device_headers() {
    let id = "12345678-1234-1234-1234-123456789abc";
    let mut identity = fixture_user();
    identity.id = id.into();
    let user = serde_json::to_string(&identity).unwrap();
    let binding=serde_json::json!({"device_id":id,"device_token":"d".repeat(64),"owner_id":id,"owner":"user@example.com","sequence":-1}).to_string();
    let (api, requests, thread) = mock(vec![
        ok(&serde_json::json!({"request_id":id,"expires_in":600,"retry_after":60}).to_string()),
        (
            200,
            cookie().replace("Max-Age=86400", "Max-Age=2592000"),
            "{\"ok\":true}".into(),
        ),
        ok(&user),
        ok(&binding),
        ok("{\"ok\":true,\"remote_execution\":false}"),
    ]);
    let challenge = api.send_code(" USER@example.com ").unwrap();
    let session = api
        .login_code("user@example.com", &challenge.request_id, "123456", true)
        .unwrap();
    assert!(session.expires_at - chrono::Utc::now().timestamp() > 2591900);
    let device = api
        .register_device(&session, &"e".repeat(64), "Test device", false)
        .unwrap();
    api.heartbeat(
        &device,
        &Heartbeat {
            run_generation: 1,
            remote_revision: None,
            sequence: 0,
            game_running: None,
            prepare_seconds: 0,
            version: "0.14.0".into(),
            account_action: "clear".into(),
            leigod_account: None,
        },
    )
    .unwrap();
    thread.join().unwrap();
    let requests = requests.lock().unwrap();
    assert!(requests[0].contains("user@example.com"));
    assert!(requests[1].contains("\"remember\":true"));
    assert!(requests[3].contains("x-csrf-token:"));
    assert!(requests[4].contains(&format!("authorization: Bearer {}", "d".repeat(64))));
    assert!(!requests[4].contains("cookie:"));
    assert!(!requests[4].contains(&"e".repeat(64)));
}

#[test]
fn remote_consent_uses_body_and_device_bearer_with_versioned_revocation() {
    let id = "12345678-1234-1234-1234-123456789abc";
    let binding = DeviceBinding {
        device_id: id.into(),
        device_token: "d".repeat(64),
        owner_id: id.into(),
        owner: "fixture".into(),
        sequence: 0,
    };
    let status=serde_json::json!({"available":true,"enabled":true,"revision":7,"account_key":"e".repeat(64),"label":"masked","protection":"awaiting_heartbeat","credential":"valid","last_result":"none"}).to_string();
    let off=serde_json::json!({"available":true,"enabled":false,"revision":8,"protection":"off","credential":"deleted","last_result":"none"}).to_string();
    let (api, requests, thread) = mock(vec![ok(&status), ok(&status), ok(&off)]);
    let authorized = api
        .remote_authorize(&binding, "fixture-provider-token", 6, 22, false)
        .unwrap();
    assert_eq!(authorized.revision, 7);
    assert_ne!(authorized.message(), "服务器已确认远程保护生效");
    api.remote_status(&binding).unwrap();
    assert!(!api.remote_disable(&binding, 7).unwrap().enabled);
    thread.join().unwrap();
    let r = requests.lock().unwrap();
    assert!(r[0].starts_with("POST /api/device/remote/authorize HTTP"));
    assert!(r[0].contains("\"account_token\":\"fixture-provider-token\""));
    assert!(r[0].contains("\"run_generation\":22"));
    assert!(!r[0]
        .lines()
        .next()
        .unwrap()
        .contains("fixture-provider-token"));
    assert!(!r[1].contains("fixture-provider-token"));
    assert!(!r[2].contains("fixture-provider-token"));
    assert!(r[2].contains("\"revision\":7"));
    for r in r.iter() {
        assert!(r.contains("authorization: Bearer"));
        assert!(!r.contains("cookie:"));
    }
}

#[test]
fn remote_errors_explain_known_reasons_without_echoing_response_secrets() {
    let id = "12345678-1234-1234-1234-123456789abc";
    let binding = DeviceBinding {
        device_id: id.into(),
        device_token: "d".repeat(64),
        owner_id: id.into(),
        owner: "fixture".into(),
        sequence: 0,
    };
    for (status, message, expected) in [
        (
            400,
            "雷神登录已失效，请在客户端重新登录",
            Error::LeigodCredential,
        ),
        (
            400,
            "无法验证雷神账号唯一身份，未开启保护",
            Error::LeigodIdentity,
        ),
        (
            409,
            "该雷神账号已属于其他平台账号",
            Error::RemoteAccountOwned,
        ),
        (
            409,
            "客户端运行代次已变化，请重新连接",
            Error::RemoteStateChanged,
        ),
        (409, "远程授权已变化，请重新确认", Error::RemoteStateChanged),
        (
            409,
            "雷神账号已切换，请关闭旧保护后重新开启",
            Error::RemoteStateChanged,
        ),
        (
            409,
            "远程授权已变化，请刷新后重试",
            Error::RemoteStateChanged,
        ),
        (400, "untrusted echoed secret", Error::InvalidInput),
        (409, "untrusted echoed secret", Error::Conflict),
        (
            401,
            "无法验证雷神账号唯一身份，未开启保护",
            Error::Unauthorized,
        ),
    ] {
        let body =
            serde_json::json!({"error":message,"token":"untrusted echoed secret"}).to_string();
        let (api, _, thread) = mock(vec![(status, String::new(), body)]);
        assert_eq!(
            api.remote_authorize(&binding, "fixture-provider-token", 0, 1, false)
                .err(),
            Some(expected)
        );
        assert!(!expected.message().contains("secret"));
        assert_eq!(expected.is_conflict(), status == 409);
        thread.join().unwrap();
    }
    for (status, body, expected) in [
        (400, "not json".into(), Error::InvalidInput),
        (
            400,
            "x".repeat(MAX_RESPONSE as usize + 1),
            Error::InvalidInput,
        ),
        (409, "not json".into(), Error::Conflict),
        (409, "x".repeat(MAX_RESPONSE as usize + 1), Error::Conflict),
    ] {
        let (api, _, thread) = mock(vec![(status, String::new(), body)]);
        assert_eq!(
            api.remote_authorize(&binding, "fixture-provider-token", 0, 1, false)
                .err(),
            Some(expected)
        );
        thread.join().unwrap();
    }
}
