//! Anonymous, bounded GitHub release checks and verified downloads.
//!
//! This module never reads account configuration or executes an update. The UI
//! decides when to call it; the separate helper applies a verified package only
//! after an explicit user action.

use reqwest::blocking::{Client, Response};
use reqwest::redirect::Policy;
use reqwest::{StatusCode, Url};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::Duration;
use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::MoveFileW;

pub const RELEASES_PAGE: &str = "https://github.com/CMMUU/leigod-guard/releases/latest";
const LATEST_API: &str = "https://api.github.com/repos/CMMUU/leigod-guard/releases/latest";
const ASSET_API_PREFIX: &str = "https://api.github.com/repos/CMMUU/leigod-guard/releases/assets/";
const REPOSITORY: &str = "https://github.com/CMMUU/leigod-guard";
const MAX_METADATA_BYTES: u64 = 1024 * 1024;
const MAX_CHECKSUM_BYTES: u64 = 64 * 1024;
const MAX_PACKAGE_BYTES: u64 = 384 * 1024 * 1024;
const METADATA_TIMEOUT: Duration = Duration::from_secs(30);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum PackageKind {
    Installer,
    Portable,
}

#[derive(Clone, Debug)]
pub struct ReleaseInfo {
    pub version: String,
    pub tag: String,
    pub notes: String,
    pub page_url: String,
    installer: Asset,
    portable: Asset,
    checksums: Asset,
}

#[derive(Clone, Debug)]
pub struct DownloadedUpdate {
    pub kind: PackageKind,
    pub version: String,
    pub path: PathBuf,
    pub sha256: String,
    pub size: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct DownloadProgress {
    pub downloaded: u64,
    pub total: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
struct Asset {
    id: u64,
    url: String,
    name: String,
    browser_download_url: String,
    size: u64,
    state: String,
    #[serde(default)]
    digest: Option<String>,
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    html_url: String,
    draft: bool,
    prerelease: bool,
    #[serde(default)]
    body: Option<String>,
    assets: Vec<Asset>,
}

/// Checks the public repository without credentials. `None` includes equal,
/// older, draft and prerelease versions; malformed metadata is an error.
pub fn check_latest(current_version: &str) -> Result<Option<ReleaseInfo>, String> {
    (|| {
        parse_version(current_version)?;
        let response = client()?
            .get(LATEST_API)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .timeout(METADATA_TIMEOUT)
            .send()
            .map_err(network_error)?;
        // A new public repository can legitimately have no release yet.
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let bytes = read_response(response, MAX_METADATA_BYTES, None)?;
        release_from_json(&bytes, current_version)
    })()
    .map_err(with_manual_fallback)
}

/// Downloads into an existing, unique directory created by the caller. Existing
/// files are never reused or overwritten. Only a fully verified file is returned.
pub fn download_update(
    release: &ReleaseInfo,
    kind: PackageKind,
    staging_dir: &Path,
    progress: &dyn Fn(DownloadProgress),
) -> Result<DownloadedUpdate, String> {
    download_inner(release, kind, staging_dir, progress).map_err(with_manual_fallback)
}

fn download_inner(
    release: &ReleaseInfo,
    kind: PackageKind,
    staging_dir: &Path,
    progress: &dyn Fn(DownloadProgress),
) -> Result<DownloadedUpdate, String> {
    // Recheck the binding because public display fields may have been changed by
    // a caller after the initial release check.
    parse_version(&release.version)?;
    if release.tag != format!("v{}", release.version) {
        return Err("更新版本信息不一致，请重新检查更新。".into());
    }
    let asset = match kind {
        PackageKind::Installer => &release.installer,
        PackageKind::Portable => &release.portable,
    };
    validate_asset(
        asset,
        &release.tag,
        &package_name(&release.tag, kind),
        MAX_PACKAGE_BYTES,
    )?;
    validate_asset(
        &release.checksums,
        &release.tag,
        "SHA256SUMS.txt",
        MAX_CHECKSUM_BYTES,
    )?;

    let metadata = fs::symlink_metadata(staging_dir)
        .map_err(|_| "无法访问更新临时目录，请检查磁盘空间和目录权限。".to_string())?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("更新临时目录无效，请重新尝试更新。".into());
    }
    let staging_dir = fs::canonicalize(staging_dir)
        .map_err(|_| "无法确认更新临时目录，请重新尝试更新。".to_string())?;
    let final_path = staging_dir.join(&asset.name);
    let part_path = staging_dir.join(format!("{}.part", asset.name));
    if final_path
        .try_exists()
        .map_err(|_| "无法检查更新文件路径。")?
    {
        return Err("更新临时目录中已有同名文件，请重新尝试更新。".into());
    }

    let client = client()?;
    let checksum_response = asset_response(&client, &release.checksums, METADATA_TIMEOUT)?;
    let checksum_bytes = read_response(
        checksum_response,
        MAX_CHECKSUM_BYTES,
        Some(release.checksums.size),
    )?;
    verify_api_digest(&checksum_bytes, &release.checksums)?;
    let checksum_text = std::str::from_utf8(&checksum_bytes)
        .map_err(|_| "更新校验清单不是有效文本，已停止更新。".to_string())?;
    let expected_hash = checksum_for(checksum_text, &asset.name)?;
    if let Some(digest) = &asset.digest {
        let api_hash = parse_api_digest(digest)?;
        verify_hash(&api_hash, &expected_hash)?;
    }

    let mut response = asset_response(&client, asset, DOWNLOAD_TIMEOUT)?;
    check_response(&response, MAX_PACKAGE_BYTES, Some(asset.size))?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&part_path)
        .map_err(|_| "无法创建更新文件，请检查磁盘空间和目录权限。".to_string())?;

    let result = (|| {
        let actual_hash = copy_verified(
            &mut response,
            &mut file,
            asset.size,
            &expected_hash,
            progress,
        )?;
        file.sync_all()
            .map_err(|_| "更新文件未能完整写入磁盘，请检查磁盘空间。".to_string())?;
        Ok(actual_hash)
    })();
    // Close the handle before renaming/removing a file on Windows.
    drop(file);
    let actual_hash = match result {
        Ok(hash) => hash,
        Err(error) => {
            let _ = fs::remove_file(&part_path);
            return Err(error);
        }
    };
    // MoveFileW fails atomically if the destination already exists. Rust's
    // fs::rename would replace an existing file even on Windows.
    if rename_new(&part_path, &final_path).is_err() {
        let _ = fs::remove_file(&part_path);
        return Err("无法保存已校验的更新文件，请检查目录权限后重试。".into());
    }
    Ok(DownloadedUpdate {
        kind,
        version: release.version.clone(),
        path: final_path,
        sha256: actual_hash,
        size: asset.size,
    })
}

fn copy_verified(
    reader: &mut impl Read,
    writer: &mut impl Write,
    expected_size: u64,
    expected_hash: &str,
    progress: &dyn Fn(DownloadProgress),
) -> Result<String, String> {
    if expected_size == 0 || expected_size > MAX_PACKAGE_BYTES {
        return Err("更新文件大小异常，已停止更新。".into());
    }
    let mut hasher = Sha256::new();
    let mut downloaded = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    progress(DownloadProgress {
        downloaded,
        total: Some(expected_size),
    });
    loop {
        let length = reader
            .read(&mut buffer)
            .map_err(|_| "下载中断或超时，请检查网络后重试。".to_string())?;
        if length == 0 {
            break;
        }
        downloaded += length as u64;
        if downloaded > expected_size {
            return Err("更新文件大小与发布信息不符，已停止更新。".into());
        }
        writer
            .write_all(&buffer[..length])
            .map_err(|_| "无法写入更新文件，请检查磁盘空间和目录权限。".to_string())?;
        hasher.update(&buffer[..length]);
        progress(DownloadProgress {
            downloaded,
            total: Some(expected_size),
        });
    }
    if downloaded != expected_size {
        return Err("更新文件下载不完整，请重试。".into());
    }
    let actual_hash = format!("{:x}", hasher.finalize());
    verify_hash(&actual_hash, expected_hash)?;
    Ok(actual_hash)
}

fn rename_new(from: &Path, to: &Path) -> std::io::Result<()> {
    let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    // Both paths come from a canonicalized directory and fixed ASCII filenames.
    // The NUL-terminated allocations remain alive for the complete Win32 call.
    unsafe { MoveFileW(PCWSTR(from.as_ptr()), PCWSTR(to.as_ptr())) }
        .map_err(|_| std::io::Error::last_os_error())
}

fn client() -> Result<Client, String> {
    Client::builder()
        .user_agent(concat!("LeigodGuard/", env!("CARGO_PKG_VERSION")))
        .https_only(true)
        .connect_timeout(Duration::from_secs(15))
        .timeout(METADATA_TIMEOUT)
        .redirect(Policy::custom(|attempt| {
            if attempt.previous().len() >= 5 {
                return attempt.error("too many update redirects");
            }
            if trusted_redirect(attempt.url()) {
                attempt.follow()
            } else {
                attempt.error("untrusted update redirect")
            }
        }))
        .build()
        .map_err(|_| "无法初始化安全的更新连接，请稍后重试。".into())
}

fn asset_response(client: &Client, asset: &Asset, timeout: Duration) -> Result<Response, String> {
    if asset.id == 0 || asset.url != format!("{ASSET_API_PREFIX}{}", asset.id) {
        return Err("更新文件的 GitHub API 地址或编号不符，已停止自动更新。".into());
    }
    // GitHub officially supports anonymous binary downloads for public release
    // assets through this API. It either streams the bytes (200) or redirects to
    // its release storage (302). This avoids requiring a connection to the web
    // frontend at github.com when that route is unavailable.
    // https://docs.github.com/en/rest/releases/assets#get-a-release-asset
    client
        .get(&asset.url)
        .header("Accept", "application/octet-stream")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .timeout(timeout)
        .send()
        .map_err(network_error)
}

fn network_error(error: reqwest::Error) -> String {
    if error.is_timeout() {
        "连接 GitHub 超时，请检查网络后重试。".into()
    } else if error.is_redirect() {
        "更新下载地址发生了不受信任或过多的跳转，已停止连接。".into()
    } else {
        "无法安全连接 GitHub，请检查网络、代理设置或系统时间后重试。".into()
    }
}

fn with_manual_fallback(message: String) -> String {
    format!("{message}\n也可前往发布页手动更新：{RELEASES_PAGE}")
}

fn check_response(response: &Response, limit: u64, expected: Option<u64>) -> Result<(), String> {
    match response.status() {
        StatusCode::OK => {}
        StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS => {
            return Err("GitHub 暂时限制了请求，请稍后重试。".into());
        }
        StatusCode::NOT_FOUND => return Err("该版本的更新文件暂不可用，请稍后重试。".into()),
        _ => return Err("GitHub 未能提供完整的更新数据，请稍后重试。".into()),
    }
    if let Some(length) = response.content_length() {
        if length > limit || expected.is_some_and(|expected| length != expected) {
            return Err("更新文件大小与发布信息不符，已停止更新。".into());
        }
    }
    Ok(())
}

fn read_response(response: Response, limit: u64, expected: Option<u64>) -> Result<Vec<u8>, String> {
    check_response(&response, limit, expected)?;
    let mut bytes = Vec::new();
    response
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "获取更新信息时网络中断或超时，请重试。".to_string())?;
    if bytes.len() as u64 > limit || expected.is_some_and(|expected| bytes.len() as u64 != expected)
    {
        return Err("更新数据大小异常或下载不完整，已停止更新。".into());
    }
    Ok(bytes)
}

fn release_from_json(bytes: &[u8], current_version: &str) -> Result<Option<ReleaseInfo>, String> {
    if bytes.len() as u64 > MAX_METADATA_BYTES {
        return Err("更新信息过大，已停止读取。".into());
    }
    let current = parse_version(current_version)?;
    let release: ApiRelease = serde_json::from_slice(bytes)
        .map_err(|_| "GitHub 返回的版本信息格式异常，请稍后重试。".to_string())?;
    if release.draft || release.prerelease {
        return Ok(None);
    }
    let version = release
        .tag_name
        .strip_prefix('v')
        .ok_or_else(|| "发布版本号格式异常，已停止自动更新。".to_string())?;
    let latest = parse_version(version)?;
    if latest <= current {
        return Ok(None);
    }
    let expected_page = format!("{REPOSITORY}/releases/tag/{}", release.tag_name);
    if release.html_url != expected_page {
        return Err("发布页面与本项目不符，已停止自动更新。".into());
    }
    let installer = unique_asset(
        &release.assets,
        &release.tag_name,
        &package_name(&release.tag_name, PackageKind::Installer),
        MAX_PACKAGE_BYTES,
    )?;
    let portable = unique_asset(
        &release.assets,
        &release.tag_name,
        &package_name(&release.tag_name, PackageKind::Portable),
        MAX_PACKAGE_BYTES,
    )?;
    let checksums = unique_asset(
        &release.assets,
        &release.tag_name,
        "SHA256SUMS.txt",
        MAX_CHECKSUM_BYTES,
    )?;
    if [installer.id, portable.id, checksums.id]
        .into_iter()
        .collect::<HashSet<_>>()
        .len()
        != 3
    {
        return Err("发布文件的 GitHub 资产编号重复，已停止自动更新。".into());
    }
    let notes = release
        .body
        .unwrap_or_default()
        .chars()
        .take(24_000)
        .collect();
    Ok(Some(ReleaseInfo {
        version: version.to_string(),
        tag: release.tag_name,
        notes,
        page_url: expected_page,
        installer,
        portable,
        checksums,
    }))
}

fn parse_version(version: &str) -> Result<[u64; 3], String> {
    let parts: Vec<_> = version.split('.').collect();
    let invalid = || "版本号格式异常，暂时无法自动比较版本。".to_string();
    if parts.len() != 3 {
        return Err(invalid());
    }
    let mut parsed = [0; 3];
    for (index, part) in parts.into_iter().enumerate() {
        if part.is_empty()
            || !part.bytes().all(|byte| byte.is_ascii_digit())
            || (part.len() > 1 && part.starts_with('0'))
        {
            return Err(invalid());
        }
        parsed[index] = part.parse().map_err(|_| invalid())?;
    }
    Ok(parsed)
}

fn package_name(tag: &str, kind: PackageKind) -> String {
    let suffix = match kind {
        PackageKind::Installer => "-setup.exe",
        PackageKind::Portable => ".zip",
    };
    format!("leigod-guard-{tag}-windows-x64{suffix}")
}

fn unique_asset(assets: &[Asset], tag: &str, name: &str, limit: u64) -> Result<Asset, String> {
    let mut matches = assets.iter().filter(|asset| asset.name == name);
    let asset = matches
        .next()
        .ok_or_else(|| "新版本的安装版、绿色版或校验清单尚未完整发布，请稍后重试。".to_string())?;
    if matches.next().is_some() {
        return Err("发布文件存在重复名称，已停止自动更新。".into());
    }
    validate_asset(asset, tag, name, limit)?;
    Ok(asset.clone())
}

fn validate_asset(asset: &Asset, tag: &str, name: &str, limit: u64) -> Result<(), String> {
    let expected_url = format!("{REPOSITORY}/releases/download/{tag}/{name}");
    if asset.name != name || asset.browser_download_url != expected_url {
        return Err("更新文件地址与本项目不符，已停止自动更新。".into());
    }
    if asset.id == 0 || asset.url != format!("{ASSET_API_PREFIX}{}", asset.id) {
        return Err("更新文件的 GitHub API 地址或编号不符，已停止自动更新。".into());
    }
    if asset.state != "uploaded" || asset.size == 0 || asset.size > limit {
        return Err("更新文件尚未上传完成或大小异常，请稍后重试。".into());
    }
    if let Some(digest) = &asset.digest {
        parse_api_digest(digest)?;
    }
    Ok(())
}

fn trusted_redirect(url: &Url) -> bool {
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port_or_known_default() != Some(443)
        || url.fragment().is_some()
    {
        return false;
    }
    match url.host_str() {
        Some("api.github.com") => {
            url.as_str() == LATEST_API
                || url
                    .as_str()
                    .strip_prefix(ASSET_API_PREFIX)
                    .and_then(|id| id.parse::<u64>().ok())
                    .is_some_and(|id| id > 0 && url.as_str() == format!("{ASSET_API_PREFIX}{id}"))
        }
        Some("github.com") => {
            url.query().is_none()
                && url
                    .path()
                    .starts_with("/CMMUU/leigod-guard/releases/download/")
        }
        // Exact hosts, not a wildcard suffix. Signed query strings on GitHub's
        // release storage are expected; arbitrary githubusercontent subdomains
        // and user-controlled GitHub Pages are not accepted.
        Some("release-assets.githubusercontent.com")
        | Some("objects.githubusercontent.com")
        | Some("github-releases.githubusercontent.com") => true,
        _ => false,
    }
}

fn parse_api_digest(digest: &str) -> Result<String, String> {
    let hash = digest
        .strip_prefix("sha256:")
        .filter(|hash| valid_hash(hash))
        .ok_or_else(|| "发布文件的 SHA-256 信息异常，已停止自动更新。".to_string())?;
    Ok(hash.to_ascii_lowercase())
}

fn verify_api_digest(bytes: &[u8], asset: &Asset) -> Result<(), String> {
    if let Some(digest) = &asset.digest {
        let actual = format!("{:x}", Sha256::digest(bytes));
        verify_hash(&actual, &parse_api_digest(digest)?)?;
    }
    Ok(())
}

fn valid_hash(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn verify_hash(actual: &str, expected: &str) -> Result<(), String> {
    if !valid_hash(actual) || !valid_hash(expected) || !actual.eq_ignore_ascii_case(expected) {
        return Err("更新文件的 SHA-256 校验失败，已停止更新；请重新下载。".into());
    }
    Ok(())
}

fn checksum_for(manifest: &str, filename: &str) -> Result<String, String> {
    if manifest.len() as u64 > MAX_CHECKSUM_BYTES {
        return Err("更新校验清单过大，已停止更新。".into());
    }
    let mut found = None;
    let mut names = HashSet::new();
    for line in manifest.trim_start_matches('\u{feff}').lines() {
        if line.is_empty() {
            continue;
        }
        let bytes = line.as_bytes();
        if bytes.len() < 67
            || !bytes[..64].iter().all(|byte| byte.is_ascii_hexdigit())
            || bytes[64] != b' '
            || !matches!(bytes[65], b' ' | b'*')
        {
            return Err("更新校验清单格式异常，已停止更新。".into());
        }
        // The preceding 66 bytes were all checked as ASCII, so this is a valid
        // UTF-8 boundary even for an untrusted manifest.
        let name = &line[66..];
        if name.contains(['/', '\\', '\0']) || !names.insert(name) {
            return Err("更新校验清单的文件名不明确或重复，已停止更新。".into());
        }
        if name == filename {
            found = Some(line[..64].to_ascii_lowercase());
        }
    }
    found.ok_or_else(|| "校验清单中缺少该更新文件的精确记录，已停止更新。".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn fixture_release(tag: &str) -> Value {
        let names = [
            package_name(tag, PackageKind::Installer),
            package_name(tag, PackageKind::Portable),
            "SHA256SUMS.txt".to_string(),
        ];
        json!({
            "tag_name": tag,
            "html_url": format!("{REPOSITORY}/releases/tag/{tag}"),
            "draft": false,
            "prerelease": false,
            "body": "fixture notes",
            "assets": names.into_iter().enumerate().map(|(index, name)| {
                let id = index as u64 + 1;
                json!({
                    "id": id,
                    "url": format!("{ASSET_API_PREFIX}{id}"),
                    "browser_download_url": format!("{REPOSITORY}/releases/download/{tag}/{name}"),
                    "name": name,
                    "size": 100,
                    "state": "uploaded",
                })
            }).collect::<Vec<_>>()
        })
    }

    fn parse_fixture(release: &Value, current: &str) -> Result<Option<ReleaseInfo>, String> {
        release_from_json(&serde_json::to_vec(release).unwrap(), current)
    }

    #[test]
    fn version_order_is_numeric_and_never_downgrades() {
        assert!(parse_version("0.10.0").unwrap() > parse_version("0.9.99").unwrap());
        assert!(parse_version("1.0.0").unwrap() > parse_version("0.999.999").unwrap());
        assert!(parse_fixture(&fixture_release("v0.10.0"), "0.9.9")
            .unwrap()
            .is_some());
        assert!(parse_fixture(&fixture_release("v0.10.0"), "0.10.0")
            .unwrap()
            .is_none());
        assert!(parse_fixture(&fixture_release("v0.10.0"), "1.0.0")
            .unwrap()
            .is_none());
    }

    #[test]
    fn rejects_ambiguous_versions_and_nonstable_tags() {
        for invalid in [
            "1.2",
            "1.2.3.4",
            "01.2.3",
            "1.02.3",
            "1.2.3-beta",
            "1.2.3+build",
            "+1.2.3",
            "1.2.-3",
            "1.2. 3",
            "v1.2.3",
            "１.2.3",
            "18446744073709551616.0.0",
        ] {
            assert!(parse_version(invalid).is_err(), "accepted {invalid}");
        }
        assert!(parse_fixture(&fixture_release("1.2.3"), "0.1.0").is_err());
        for field in ["draft", "prerelease"] {
            let mut fixture = fixture_release("v0.10.0");
            fixture[field] = json!(true);
            assert!(parse_fixture(&fixture, "0.1.0").unwrap().is_none());
        }
    }

    #[test]
    fn requires_both_packages_and_one_unambiguous_manifest() {
        for missing in 0..3 {
            let mut fixture = fixture_release("v0.10.0");
            fixture["assets"].as_array_mut().unwrap().remove(missing);
            assert!(parse_fixture(&fixture, "0.1.0").is_err());
        }
        let mut fixture = fixture_release("v0.10.0");
        let duplicate = fixture["assets"][2].clone();
        fixture["assets"].as_array_mut().unwrap().push(duplicate);
        assert!(parse_fixture(&fixture, "0.1.0").is_err());
    }

    #[test]
    fn rejects_foreign_urls_and_unbounded_or_incomplete_assets() {
        let mut fixture = fixture_release("v0.10.0");
        fixture["assets"][0]["browser_download_url"] =
            json!("https://github.com/attacker/leigod-guard/releases/download/v0.10.0/setup.exe");
        assert!(parse_fixture(&fixture, "0.1.0").is_err());
        for size in [0, MAX_PACKAGE_BYTES + 1] {
            let mut fixture = fixture_release("v0.10.0");
            fixture["assets"][0]["size"] = json!(size);
            assert!(parse_fixture(&fixture, "0.1.0").is_err());
        }
        let mut fixture = fixture_release("v0.10.0");
        fixture["assets"][0]["state"] = json!("starter");
        assert!(parse_fixture(&fixture, "0.1.0").is_err());
        let mut fixture = fixture_release("v0.10.0");
        fixture["html_url"] = json!("https://evil.example/releases/tag/v0.10.0");
        assert!(parse_fixture(&fixture, "0.1.0").is_err());
    }

    #[test]
    fn asset_api_requires_exact_repository_and_canonical_matching_id() {
        for url in [
            "https://api.github.com/repos/attacker/leigod-guard/releases/assets/1",
            "https://api.github.com/repos/CMMUU/leigod-guard/releases/assets/2",
            "https://api.github.com/repos/CMMUU/leigod-guard/releases/assets/01",
            "https://api.github.com/repos/CMMUU/leigod-guard/releases/assets/1?download=1",
            "https://api.github.com/repos/CMMUU/leigod-guard/releases/assets/1/../2",
            "http://api.github.com/repos/CMMUU/leigod-guard/releases/assets/1",
            "https://api.github.com.evil.example/repos/CMMUU/leigod-guard/releases/assets/1",
        ] {
            let mut fixture = fixture_release("v0.10.0");
            fixture["assets"][0]["url"] = json!(url);
            assert!(parse_fixture(&fixture, "0.1.0").is_err(), "accepted {url}");
        }
        for id in [json!(0), json!(-1), json!("1"), json!(1.5)] {
            let mut fixture = fixture_release("v0.10.0");
            fixture["assets"][0]["id"] = id;
            assert!(parse_fixture(&fixture, "0.1.0").is_err());
        }
        let mut fixture = fixture_release("v0.10.0");
        fixture["assets"][1]["id"] = json!(1);
        fixture["assets"][1]["url"] = json!(format!("{ASSET_API_PREFIX}1"));
        assert!(parse_fixture(&fixture, "0.1.0").is_err());
    }

    #[test]
    fn redirects_allow_only_exact_https_github_storage_hosts() {
        for url in [
            "https://release-assets.githubusercontent.com/github-production-release-asset/abc?sig=123",
            "https://objects.githubusercontent.com/github-production-release-asset/abc?sig=123",
            "https://github-releases.githubusercontent.com/abc?sig=123",
            "https://api.github.com/repos/CMMUU/leigod-guard/releases/assets/12345",
            LATEST_API,
        ] {
            assert!(trusted_redirect(&Url::parse(url).unwrap()), "rejected {url}");
        }
        for url in [
            "http://release-assets.githubusercontent.com/a",
            "https://release-assets.githubusercontent.com.evil.example/a",
            "https://evil.example/github.com/a",
            "https://raw.githubusercontent.com/CMMUU/leigod-guard/main/a",
            "https://cmmuu.github.io/a",
            "https://github.com/attacker/leigod-guard/releases/download/a",
            "https://api.github.com/repos/attacker/leigod-guard/releases/latest",
            "https://api.github.com/repos/attacker/leigod-guard/releases/assets/12345",
            "https://api.github.com/repos/CMMUU/leigod-guard/releases/assets/0",
            "https://api.github.com/repos/CMMUU/leigod-guard/releases/assets/01",
            "https://api.github.com/repos/CMMUU/leigod-guard/releases/assets/1?redirect=anything",
            "https://user@objects.githubusercontent.com/a",
            "https://objects.githubusercontent.com:444/a",
            "https://objects.githubusercontent.com/a#fragment",
        ] {
            assert!(
                !trusted_redirect(&Url::parse(url).unwrap()),
                "accepted {url}"
            );
        }
    }

    #[test]
    fn checksum_requires_exact_name_and_unique_well_formed_record() {
        let hash = "a".repeat(64);
        assert_eq!(
            checksum_for(&format!("{hash}  app.zip\r\n"), "app.zip").unwrap(),
            hash
        );
        assert_eq!(
            checksum_for(&format!("{hash} *app.zip\n"), "app.zip").unwrap(),
            hash
        );
        for manifest in [
            format!("{hash}  app.zip.bak\n"),
            format!("{hash}  ./app.zip\n"),
            format!("{hash}  app.zip \n"),
            format!("{hash}  app.zip\n{hash}  app.zip\n"),
            format!("{}  app.zip\n", "g".repeat(64)),
            format!("{hash} app.zip\n"),
            "bad line".to_string(),
        ] {
            assert!(checksum_for(&manifest, "app.zip").is_err());
        }
    }

    #[test]
    fn hash_mismatch_and_malformed_api_digest_are_rejected() {
        let actual = format!("{:x}", Sha256::digest(b"abc"));
        assert!(verify_hash(
            &actual,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        )
        .is_ok());
        assert!(verify_hash(&actual, &"0".repeat(64)).is_err());
        assert!(verify_hash("", "").is_err());
        assert!(parse_api_digest(&format!("md5:{}", "a".repeat(64))).is_err());
        assert!(parse_api_digest("sha256:abcd").is_err());
        assert!(parse_api_digest(&format!("sha256:{}", "A".repeat(64))).is_ok());
    }

    #[test]
    fn streamed_download_rejects_truncation_overflow_and_modified_content() {
        let hash = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        let mut output = Vec::new();
        let events = std::cell::RefCell::new(Vec::new());
        let result = copy_verified(&mut &b"abc"[..], &mut output, 3, hash, &|progress| {
            events.borrow_mut().push(progress)
        });
        assert_eq!(result.unwrap(), hash);
        assert_eq!(output, b"abc");
        assert_eq!(events.borrow().first().unwrap().downloaded, 0);
        assert_eq!(events.borrow().last().unwrap().downloaded, 3);
        assert!(events.borrow().iter().all(|event| event.total == Some(3)));

        for (data, size) in [(&b"ab"[..], 3), (&b"abcd"[..], 3), (&b"abd"[..], 3)] {
            assert!(copy_verified(&mut &*data, &mut Vec::new(), size, hash, &|_| {}).is_err());
        }
    }

    #[test]
    fn verified_file_rename_never_overwrites_an_existing_destination() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "LeigodGuard-update-rename-test-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&dir).unwrap();
        let source = dir.join("fixture.part");
        let target = dir.join("fixture.zip");
        fs::write(&source, b"new bytes").unwrap();
        fs::write(&target, b"existing bytes").unwrap();
        assert!(rename_new(&source, &target).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"existing bytes");
        assert_eq!(fs::read(&source).unwrap(), b"new bytes");
        fs::remove_file(&target).unwrap();
        rename_new(&source, &target).unwrap();
        assert!(!source.exists());
        assert_eq!(fs::read(&target).unwrap(), b"new bytes");
        fs::remove_file(&target).unwrap();
        fs::remove_dir(&dir).unwrap();
    }

    #[test]
    fn old_release_without_installer_does_not_report_an_error() {
        let mut fixture = fixture_release("v0.5.0");
        fixture["assets"] = json!([]);
        assert!(parse_fixture(&fixture, "0.6.0").unwrap().is_none());
    }

    /// Opt-in network smoke test; normal local/CI tests remain fully offline.
    /// Reads only public metadata and its small checksum manifest. No packages,
    /// user configuration, account credentials, or executable processes are used.
    #[test]
    #[ignore = "explicit public GitHub network smoke test; metadata and checksums only"]
    fn public_github_asset_api_metadata_and_checksum_smoke() {
        let release = check_latest("0.0.0")
            .expect("public release metadata should be reachable")
            .expect("the public repository should have a stable release");
        let response = asset_response(
            &client().expect("HTTPS client"),
            &release.checksums,
            METADATA_TIMEOUT,
        )
        .expect("public asset API should return the checksum manifest");
        let bytes = read_response(response, MAX_CHECKSUM_BYTES, Some(release.checksums.size))
            .expect("checksum response size should match public release metadata");
        verify_api_digest(&bytes, &release.checksums)
            .expect("checksum manifest should match GitHub's digest when present");
        let manifest = std::str::from_utf8(&bytes).expect("checksum manifest should be UTF-8");
        for asset in [&release.installer, &release.portable] {
            let checksum = checksum_for(manifest, &asset.name)
                .expect("manifest should name each validated release package exactly once");
            if let Some(api_digest) = &asset.digest {
                verify_hash(&checksum, &parse_api_digest(api_digest).unwrap())
                    .expect("manifest hash should match the public asset digest");
            }
        }
        println!(
            "Public GitHub API smoke passed: {}, {} checksum bytes, both package records verified.",
            release.tag,
            bytes.len()
        );
    }
}
