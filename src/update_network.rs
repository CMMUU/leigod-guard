//! Updater-only Windows manual proxy support. Account/heartbeat clients keep
//! their existing transport; never change the machine's proxy or proxy settings.
use reqwest::blocking::ClientBuilder;
use reqwest::{NoProxy, Proxy, Url};
use winreg::{enums::HKEY_CURRENT_USER, RegKey};

const SETTINGS: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings";

pub(super) fn configure(builder: ClientBuilder) -> Result<ClientBuilder, String> {
    // Keep reqwest's existing environment-proxy precedence and NO_PROXY handling.
    if environment_proxy_present(|name| std::env::var_os(name).is_some()) {
        return Ok(builder);
    }
    let Ok(settings) = RegKey::predef(HKEY_CURRENT_USER).open_subkey(SETTINGS) else {
        return Ok(builder);
    };
    if settings.get_value::<u32, _>("ProxyEnable").unwrap_or(0) == 0 {
        return Ok(builder);
    }
    let server: String = settings
        .get_value("ProxyServer")
        .map_err(|_| "Windows 系统代理已启用，但无法读取其地址，请检查系统代理设置。")?;
    let bypass: String = settings.get_value("ProxyOverride").unwrap_or_default();
    let no_proxy = std::env::var("NO_PROXY")
        .or_else(|_| std::env::var("no_proxy"))
        .ok();
    apply_manual_proxy(builder, &server, &bypass, no_proxy.as_deref())
}

fn environment_proxy_present(present: impl Fn(&str) -> bool) -> bool {
    ["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"]
        .iter()
        .any(|name| present(name))
}

fn https_proxy(server: &str) -> Result<Option<Url>, String> {
    let server = server.trim();
    let target = if server.contains('=') {
        // Windows can store separate HTTP and HTTPS proxy endpoints. An HTTP-only
        // entry must not route our HTTPS requests through the wrong proxy.
        server.split(';').find_map(|entry| {
            let (scheme, endpoint) = entry.trim().split_once('=')?;
            scheme
                .trim()
                .eq_ignore_ascii_case("https")
                .then_some(endpoint.trim())
        })
    } else {
        Some(server)
    };
    let Some(target) = target else {
        return Ok(None);
    };
    let url = if target.contains("://") {
        target.to_string()
    } else {
        format!("http://{target}")
    };
    let invalid = || "Windows HTTPS 代理地址无效或不受支持，请检查系统代理设置。".to_string();
    let url = Url::parse(&url).map_err(|_| invalid())?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid());
    }
    Ok(Some(url))
}

fn apply_manual_proxy(
    builder: ClientBuilder,
    server: &str,
    bypass: &str,
    no_proxy: Option<&str>,
) -> Result<ClientBuilder, String> {
    let Some(endpoint) = https_proxy(server)? else {
        return Ok(builder);
    };
    let bypass = bypass.to_ascii_lowercase();
    let proxy = Proxy::custom(move |url| {
        (url.scheme() == "https" && !bypassed(url, &bypass)).then(|| endpoint.clone())
    })
    .no_proxy(no_proxy.and_then(NoProxy::from_string));
    // Explicit proxy disables reqwest's automatic proxy resolver for this client
    // only. No fallback to direct traffic when the configured proxy is unavailable.
    Ok(builder.proxy(proxy))
}

fn bypassed(url: &Url, bypass: &str) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    bypass.split(';').map(str::trim).any(|entry| {
        if entry.eq_ignore_ascii_case("<local>") {
            return !host.contains('.') && !host.contains(':');
        }
        // Windows exceptions may include scheme and port, e.g. https://*.test:443.
        let entry = match entry.split_once("://") {
            Some((scheme, entry)) if scheme == url.scheme() => entry,
            Some(_) => return false,
            None => entry,
        };
        wildcard(entry.as_bytes(), host.as_bytes())
            || url
                .port_or_known_default()
                .is_some_and(|port| wildcard(entry.as_bytes(), format!("{host}:{port}").as_bytes()))
    })
}

fn wildcard(pattern: &[u8], text: &[u8]) -> bool {
    // Greedy glob match. An asterisk is the Windows host wildcard; exact
    // host exceptions must not bypass unrelated suffixes such as example.com.evil.
    let (mut p, mut t, mut star, mut retry) = (0, 0, None, 0);
    while t < text.len() {
        if p < pattern.len() && pattern[p] == b'*' {
            star = Some(p);
            p += 1;
            retry = t;
        } else if p < pattern.len() && pattern[p] == text[t] {
            p += 1;
            t += 1;
        } else if let Some(index) = star {
            retry += 1;
            t = retry;
            p = index + 1;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == b'*' {
        p += 1;
    }
    p == pattern.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        time::Duration,
    };

    #[test]
    fn manual_proxy_formats_and_environment_precedence() {
        assert_eq!(
            https_proxy("127.0.0.1:7890").unwrap().unwrap().as_str(),
            "http://127.0.0.1:7890/"
        );
        assert_eq!(
            https_proxy("http=a:80; HTTPS=b:81;ftp=c:82")
                .unwrap()
                .unwrap()
                .as_str(),
            "http://b:81/"
        );
        assert!(https_proxy("http=a:80").unwrap().is_none());
        for invalid in [
            "",
            "socks5://localhost:1080",
            "http://proxy/path",
            "bad proxy",
            "https=",
        ] {
            assert!(https_proxy(invalid).is_err(), "{invalid}");
        }
        for key in ["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"] {
            assert!(environment_proxy_present(|name| name == key));
        }
        assert!(!environment_proxy_present(
            |name| name == "HTTP_PROXY" || name == "NO_PROXY"
        ));
    }

    #[test]
    fn manual_proxy_exceptions_match_only_intended_destinations() {
        let url = |s| Url::parse(s).unwrap();
        assert!(bypassed(
            &url("https://downloads.cmmuu.com"),
            "*.cmmuu.com;localhost"
        ));
        assert!(!bypassed(
            &url("https://downloads.cmmuu.com.evil"),
            "*.cmmuu.com"
        ));
        assert!(!bypassed(&url("https://sub.example.com"), "example.com"));
        assert!(bypassed(&url("https://intranet"), "<local>"));
        assert!(!bypassed(&url("https://downloads.cmmuu.com"), "<local>"));
        assert!(bypassed(
            &url("https://downloads.cmmuu.com"),
            "https://*.cmmuu.com:443"
        ));
        assert!(!bypassed(
            &url("https://downloads.cmmuu.com"),
            "http://*.cmmuu.com:80"
        ));
        assert!(bypassed(&url("https://downloads.cmmuu.com"), "*"));
        assert!(!bypassed(&url("https://downloads.cmmuu.com"), ""));
    }

    #[test]
    fn https_update_request_reaches_manual_proxy_without_external_network() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let worker = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                if let Ok((stream, _)) = listener.accept() {
                    break stream;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "update request did not reach proxy"
                );
                std::thread::sleep(Duration::from_millis(10));
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut bytes = Vec::new();
            while !bytes.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                bytes.push(byte[0]);
                assert!(bytes.len() < 8192);
            }
            let request = String::from_utf8_lossy(&bytes);
            assert!(request.starts_with("CONNECT update.invalid:443 HTTP/1.1\r\n"));
            assert!(!request.contains("Authorization:"));
            stream
                .write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n")
                .unwrap();
        });
        let client = apply_manual_proxy(
            reqwest::blocking::Client::builder().no_proxy(),
            &address.to_string(),
            "<local>",
            None,
        )
        .unwrap()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
        assert!(client.get("https://update.invalid/").send().is_err());
        worker.join().unwrap();
    }
}
