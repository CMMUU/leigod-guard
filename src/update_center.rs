//! Public download-center catalog adapter. No account data or mutable latest-file URL.
use super::*;

pub(super) const CATALOG: &str = "https://downloads.cmmuu.com/api/catalog";
const MAX_CATALOG_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Deserialize)]
struct Catalog {
    files: Vec<CenterFile>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CenterFile {
    id: String,
    filename: String,
    version: String,
    size: u64,
    sha256: String,
    download_url: String,
    status: String,
    source: String,
    project: Option<String>,
    category: String,
    platform: String,
    architecture: String,
    #[serde(default)]
    description: String,
}

fn valid_id(id: &str) -> bool {
    let hex = |s: &str| {
        s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    };
    (id.len() == 64 && hex(id))
        || (id.len() == 36
            && id.split('-').map(str::len).eq([8, 4, 4, 4, 12])
            && id.split('-').all(hex))
}

pub(super) fn valid_file_url(value: &str, filename: &str) -> bool {
    let Some(rest) = value.strip_prefix("https://files.cmmuu.com/d/") else {
        return false;
    };
    let Some((id, name)) = rest.split_once('/') else {
        return false;
    };
    valid_id(id) && name == filename
}

pub(super) fn validate(asset: &Asset, name: &str, limit: u64) -> Result<(), String> {
    if asset.name != name
        || asset.url != asset.browser_download_url
        || !valid_file_url(&asset.url, name)
        || asset.state != "uploaded"
        || asset.size == 0
        || asset.size > limit
    {
        return Err("下载中心文件地址、大小或状态异常，已停止更新。".into());
    }
    parse_api_digest(asset.digest.as_deref().ok_or("下载中心缺少文件校验值。")?)?;
    Ok(())
}

pub(super) fn catalog_release(
    bytes: &[u8],
    current: &str,
    tag: Option<&str>,
) -> Result<Option<ReleaseInfo>, String> {
    if bytes.len() as u64 > MAX_CATALOG_BYTES {
        return Err("下载中心目录过大，请稍后重试。".into());
    }
    let current = parse_version(current)?;
    let catalog: Catalog = serde_json::from_slice(bytes)
        .map_err(|_| "下载中心返回的目录格式异常，请稍后重试。".to_string())?;
    let entries: Vec<_> = catalog
        .files
        .iter()
        .filter(|f| {
            f.status == "published"
                && matches!(f.source.as_str(), "manual" | "release")
                && (f.project.as_deref() == Some("leigod-guard")
                    || (f.project.is_none() && f.category == "加速器守护"))
                && f.version
                    .strip_prefix('v')
                    .is_some_and(|v| parse_version(v).is_ok())
        })
        .collect();
    let selected = if let Some(tag) = tag {
        entries.iter().find(|f| f.version == tag).copied()
    } else {
        entries
            .iter()
            .max_by_key(|f| parse_version(&f.version[1..]).unwrap())
            .copied()
    };
    let selected =
        selected.ok_or("下载中心尚无可用正式版，无法确认更新，请稍后重试或手动切换来源。")?;
    let tag = &selected.version;
    let version = &tag[1..];
    let get = |name: &str, package: bool| -> Result<Asset, String> {
        let matching: Vec<_> = entries
            .iter()
            .filter(|f| f.version == *tag && f.filename == name)
            .collect();
        if matching.len() != 1 {
            return Err("下载中心此版本文件不完整或存在重复条目，请稍后重试。".into());
        }
        let file = matching[0];
        if (package && (file.platform != "windows" || file.architecture != "x64"))
            || !valid_id(&file.id)
            || file.download_url != format!("https://files.cmmuu.com/d/{}/{name}", file.id)
            || !valid_hash(&file.sha256)
        {
            return Err("下载中心的版本、平台或文件身份异常，已停止更新。".into());
        }
        let asset = Asset {
            id: 0,
            url: file.download_url.clone(),
            browser_download_url: file.download_url.clone(),
            name: name.into(),
            size: file.size,
            state: "uploaded".into(),
            digest: Some(format!("sha256:{}", file.sha256)),
        };
        validate(
            &asset,
            name,
            if package {
                MAX_PACKAGE_BYTES
            } else {
                MAX_CHECKSUM_BYTES
            },
        )?;
        Ok(asset)
    };
    let installer = get(&package_name(tag, PackageKind::Installer), true)?;
    let portable = get(&package_name(tag, PackageKind::Portable), true)?;
    let checksums = get("SHA256SUMS.txt", false)?;
    if parse_version(version)? <= current {
        return Ok(None);
    }
    Ok(Some(ReleaseInfo {
        source: UpdateSource::Center,
        version: version.into(),
        tag: tag.clone(),
        notes: entries
            .iter()
            .find(|f| f.version == *tag && f.filename == installer.name)
            .map(|f| f.description.chars().take(24_000).collect())
            .unwrap_or_default(),
        page_url: CENTER_RELEASES_PAGE.into(),
        release_id: 0,
        installer,
        portable,
        checksums,
    }))
}

pub(super) fn check(
    current: &str,
    tag: Option<&str>,
    deadline: Instant,
) -> Result<Option<ReleaseInfo>, String> {
    let bytes = fetch_catalog(&client(UpdateSource::Center)?, CATALOG, deadline)?;
    catalog_release(&bytes, current, tag)
}

fn fetch_catalog(client: &Client, url: &str, deadline: Instant) -> Result<Vec<u8>, String> {
    let response = client
        .get(url)
        .timeout(remaining(deadline)?)
        .send()
        .map_err(network_error)?;
    read_response(response, MAX_CATALOG_BYTES, None)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use serde_json::{json, Value};

    #[test]
    fn slow_catalog_survives_old_eight_second_limit_but_remains_bounded() {
        use std::net::TcpListener;
        for expires in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/catalog", listener.local_addr().unwrap());
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    stream.read_exact(&mut byte).unwrap();
                    request.push(byte[0]);
                    assert!(request.len() < 8192);
                }
                std::thread::sleep(if expires {
                    Duration::from_millis(400)
                } else {
                    Duration::from_secs(9)
                });
                let body = fixture("v99.0.0");
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream
                    .write_all(header.as_bytes())
                    .and_then(|_| stream.write_all(&body));
            });
            let client = Client::builder()
                .no_proxy()
                .timeout(METADATA_TIMEOUT)
                .build()
                .unwrap();
            let budget = if expires {
                Duration::from_millis(150)
            } else {
                update_sources::CHECK_TIMEOUT
            };
            let start = Instant::now();
            let result = fetch_catalog(&client, &url, start + budget);
            if expires {
                assert!(result.is_err());
                assert!(start.elapsed() < Duration::from_secs(2));
            } else {
                assert_eq!(
                    catalog_release(&result.unwrap(), "0.0.0", None)
                        .unwrap()
                        .unwrap()
                        .tag,
                    "v99.0.0"
                );
            }
            server.join().unwrap();
        }
    }

    pub(in crate::updater) fn fixture(tag: &str) -> Vec<u8> {
        let files: Vec<_> = [
            package_name(tag, PackageKind::Installer),
            package_name(tag, PackageKind::Portable),
            "SHA256SUMS.txt".into(),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, name)| {
            let id = format!("00000000-0000-0000-0000-00000000000{i}");
            json!({"id": id, "filename": name, "version": tag, "size": 212,
                "sha256": "a".repeat(64), "status": "published", "source": "manual",
                "project": null, "category": "加速器守护", "platform": "windows",
                "architecture": "x64", "description": "fixture release",
                "downloadUrl": format!("https://files.cmmuu.com/d/{id}/{name}")})
        })
        .collect();
        serde_json::to_vec(&json!({"files": files})).unwrap()
    }

    fn parse(value: &Value) -> Result<Option<ReleaseInfo>, String> {
        catalog_release(&serde_json::to_vec(value).unwrap(), "0.0.0", None)
    }

    #[test]
    fn latest_is_numeric_scoped_and_never_downgrades() {
        let mut catalog: Value = serde_json::from_slice(&fixture("v0.9.0")).unwrap();
        let newer: Value = serde_json::from_slice(&fixture("v0.10.0")).unwrap();
        catalog["files"]
            .as_array_mut()
            .unwrap()
            .extend(newer["files"].as_array().unwrap().clone());
        for (tag, key, value) in [
            ("v99.0.0", "project", "other-project"),
            ("v98.0.0", "status", "draft"),
            ("v97.0.0-beta", "category", "加速器守护"),
            ("v096.0.0", "category", "加速器守护"),
        ] {
            let mut ignored: Value = serde_json::from_slice(&fixture(tag)).unwrap();
            for f in ignored["files"].as_array_mut().unwrap() {
                f[key] = json!(value);
            }
            catalog["files"]
                .as_array_mut()
                .unwrap()
                .extend(ignored["files"].as_array().unwrap().clone());
        }
        assert_eq!(parse(&catalog).unwrap().unwrap().tag, "v0.10.0");
        let bytes = serde_json::to_vec(&catalog).unwrap();
        assert_eq!(
            catalog_release(&bytes, "0.0.0", Some("v0.9.0"))
                .unwrap()
                .unwrap()
                .tag,
            "v0.9.0"
        );
        assert!(catalog_release(&bytes, "0.10.0", None).unwrap().is_none());
        assert!(catalog_release(&bytes, "1.0.0", None).unwrap().is_none());
        assert!(catalog_release(&bytes, "0.0.0", Some("v0.11.0")).is_err());
    }

    #[test]
    fn empty_incomplete_duplicate_or_bad_catalog_is_failure() {
        assert!(catalog_release(b"broken", "0.0.0", None).is_err());
        assert!(parse(&json!({"files": []})).is_err());
        let mut catalog: Value = serde_json::from_slice(&fixture("v0.20.0")).unwrap();
        catalog["files"].as_array_mut().unwrap().pop();
        assert!(parse(&catalog).is_err());
        // Do not silently offer the older version while a newer publication is incomplete.
        let old: Value = serde_json::from_slice(&fixture("v0.19.0")).unwrap();
        catalog["files"]
            .as_array_mut()
            .unwrap()
            .extend(old["files"].as_array().unwrap().clone());
        assert!(parse(&catalog).is_err());
        let mut duplicate: Value = serde_json::from_slice(&fixture("v0.20.0")).unwrap();
        let extra = duplicate["files"][0].clone();
        duplicate["files"].as_array_mut().unwrap().push(extra);
        assert!(parse(&duplicate).is_err());
        assert!(
            catalog_release(&vec![b' '; MAX_CATALOG_BYTES as usize + 1], "0.0.0", None).is_err()
        );
    }

    #[test]
    fn rejects_mismatched_urls_platforms_sizes_and_digests() {
        let base: Value = serde_json::from_slice(&fixture("v0.20.0")).unwrap();
        for (key, bad) in [
            ("downloadUrl", json!("https://evil.example/setup.exe")),
            (
                "downloadUrl",
                json!("https://files.cmmuu.com.evil.example/d/id/setup.exe"),
            ),
            (
                "downloadUrl",
                json!(format!(
                    "{}?redirect=other",
                    base["files"][0]["downloadUrl"].as_str().unwrap()
                )),
            ),
            ("id", json!("00000000-0000-0000-0000-000000000099")),
            ("platform", json!("macos")),
            ("architecture", json!("arm64")),
            ("size", json!(0)),
            ("size", json!(MAX_PACKAGE_BYTES + 1)),
            ("sha256", json!("invalid")),
            ("sha256", Value::Null),
        ] {
            let mut value = base.clone();
            value["files"][0][key] = bad;
            assert!(parse(&value).is_err(), "accepted invalid {key}");
        }
        let mut oversized_checksum = base;
        oversized_checksum["files"][2]["size"] = json!(MAX_CHECKSUM_BYTES + 1);
        assert!(parse(&oversized_checksum).is_err());
        for url in [
            CATALOG,
            "https://files.cmmuu.com/",
            RELEASES_PAGE,
            GITEE_RELEASES_PAGE,
        ] {
            assert!(!trusted_source_redirect(
                UpdateSource::Center,
                &Url::parse(url).unwrap()
            ));
        }
    }

    #[test]
    fn release_imports_and_manual_uploads_share_identity_checks() {
        let mut catalog: Value = serde_json::from_slice(&fixture("v0.20.0")).unwrap();
        for f in catalog["files"].as_array_mut().unwrap() {
            let id = "b".repeat(64);
            f["source"] = json!("release");
            f["project"] = json!("leigod-guard");
            f["category"] = json!("项目发行");
            f["id"] = json!(id);
            f["downloadUrl"] = json!(format!(
                "https://files.cmmuu.com/d/{id}/{}",
                f["filename"].as_str().unwrap()
            ));
        }
        let release = parse(&catalog).unwrap().unwrap();
        assert_eq!(release.source, UpdateSource::Center);
        assert_eq!(release.page_url, CENTER_RELEASES_PAGE);
        assert!(verify_api_digest(b"corrupted download", &release.installer).is_err());
        assert!(verify_api_digest(b"corrupted manifest", &release.checksums).is_err());
    }

    #[test]
    #[ignore = "explicit anonymous download-center package verification; no executable is run"]
    fn public_center_packages_download_and_verify() {
        let report = check_for_updates("0.0.0", UpdateMode::default()).unwrap();
        let plan = report.plan.expect("published stable release");
        assert_eq!(plan.release.source, UpdateSource::Center);
        assert!(!plan.automatic());
        let exact = check_tag(&plan.release.tag, UpdateSource::Center)
            .unwrap()
            .unwrap();
        assert_eq!(exact.installer.digest, plan.release.installer.digest);
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "guard-center-check-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&dir).unwrap();
        for kind in [PackageKind::Installer, PackageKind::Portable] {
            let downloaded =
                download_planned_update(&plan, kind, &dir, &|_| {}, &|source, fallback| {
                    assert_eq!(source, UpdateSource::Center);
                    assert!(!fallback);
                })
                .unwrap();
            assert_eq!(downloaded.kind, kind);
            assert_eq!(downloaded.version, plan.release.version);
            assert_eq!(
                fs::metadata(&downloaded.path).unwrap().len(),
                downloaded.size
            );
            println!(
                "Download center verified {}: {} bytes, SHA-256 {}",
                downloaded.path.file_name().unwrap().to_string_lossy(),
                downloaded.size,
                downloaded.sha256
            );
            fs::remove_file(downloaded.path).unwrap();
        }
        fs::remove_dir(dir).unwrap();
    }
}
