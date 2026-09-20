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
    let session = api.login(" demo ", "password-fixture-123").unwrap();
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
        assert_eq!(api.login("demo", "some-password").err(), Some(expected));
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
        api.login("demo", "some-password").err(),
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
        cookie().replace("Max-Age=86400", "Max-Age=999999"),
        cookie().replace(&"a".repeat(64), "bad"),
    ] {
        let (api, _, thread) = mock(vec![(200, header, "{\"ok\":true}".into())]);
        assert_eq!(api.login("demo", "password").err(), Some(Error::Protocol));
        thread.join().unwrap();
    }
    for body in ["{}".to_owned(), "x".repeat(MAX_RESPONSE as usize + 1)] {
        let (api, _, thread) = mock(vec![(200, cookie(), "{\"ok\":true}".into()), ok(&body)]);
        assert_eq!(api.login("demo", "password").err(), Some(Error::Protocol));
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
        api.login("demo", "some-password").err(),
        Some(Error::Network)
    );
    thread.join().unwrap();
}
