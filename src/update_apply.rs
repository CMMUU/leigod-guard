//! Apply a verified update from a separate process, without touching account data.
use crate::updater::{DownloadedUpdate, PackageKind};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::windows::fs::MetadataExt;
use std::os::windows::process::CommandExt;
use std::path::{Component, Path, PathBuf, Prefix};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetProcessTimes, OpenProcess, WaitForSingleObject, CREATE_NO_WINDOW,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
};
use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY};
use winreg::RegKey;

const APP_EXE: &str = "leigod-guard.exe";
const HELPER_EXE: &str = "update-helper.exe";
const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{6ABF5F53-AFD4-4D24-A930-174CF6B21B7A}_is1";
const MAX_ARCHIVE: u64 = 512 * 1024 * 1024;
const MAX_UNPACKED: u64 = 1024 * 1024 * 1024;
const MAX_FILES: usize = 2048;
const MANUAL_URL: &str = "Gitee：https://gitee.com/cmmuu/leigod-guard/releases\nGitHub：https://github.com/CMMUU/leigod-guard/releases/latest";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdatePlan {
    schema: u32,
    kind: PackageKind,
    version: String,
    previous_version: String,
    parent_pid: u32,
    parent_created: u64,
    destination: PathBuf,
    artifact: String,
    artifact_size: u64,
    sha256: String,
    previous_sha256: String,
}

struct ProcessHandle(HANDLE);
impl Drop for ProcessHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn io_error(context: &str, error: impl std::fmt::Display) -> String {
    format!("{context}：{error}")
}

fn same_path(left: &Path, right: &Path) -> bool {
    left.as_os_str()
        .to_string_lossy()
        .eq_ignore_ascii_case(&right.as_os_str().to_string_lossy())
}

/// Reject junctions and symlinks before canonicalization, including ancestors.
fn safe_path(path: &Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("更新路径必须是绝对路径".into());
    }
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => {
                if !matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)) {
                    return Err("自动更新只支持本机磁盘路径，请手动更新网络共享中的程序".into());
                }
                current.push(component);
                continue;
            }
            Component::ParentDir | Component::CurDir => return Err("更新路径包含无效层级".into()),
            _ => current.push(component),
        }
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if metadata.file_attributes() & 0x400 != 0 {
                    return Err(format!(
                        "更新路径含链接或重解析点，已停止：{}",
                        current.display()
                    ));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error("无法检查更新路径", error)),
        }
    }
    Ok(())
}

fn canonical_safe(path: &Path) -> Result<PathBuf, String> {
    safe_path(path)?;
    fs::canonicalize(path).map_err(|error| io_error("无法解析更新路径", error))
}

fn updates_root() -> Result<PathBuf, String> {
    let root = dirs::data_local_dir()
        .ok_or("无法找到本机应用数据目录")?
        .join("LeigodGuard")
        .join("updates");
    safe_path(&root)?;
    fs::create_dir_all(&root).map_err(|error| io_error("无法创建更新缓存目录", error))?;
    canonical_safe(&root)
}

fn unique_directory(parent: &Path, prefix: &str) -> Result<PathBuf, String> {
    safe_path(parent)?;
    let ticks = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| io_error("无法读取系统时间", error))?
        .as_nanos();
    for counter in 0..20 {
        let candidate = parent.join(format!("{prefix}{}-{ticks}-{counter}", std::process::id()));
        match fs::create_dir(&candidate) {
            Ok(()) => return canonical_safe(&candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(io_error("无法创建更新暂存目录", error)),
        }
    }
    Err("无法创建唯一的更新暂存目录，请重试".into())
}

pub fn create_staging() -> Result<PathBuf, String> {
    unique_directory(&updates_root()?, "update-")
}

fn validate_staging(path: &Path) -> Result<PathBuf, String> {
    let stage = canonical_safe(path)?;
    let root = updates_root()?;
    if !stage
        .parent()
        .is_some_and(|parent| same_path(parent, &root))
        || !stage.file_name().is_some_and(|name| {
            let name = name.to_string_lossy();
            name.starts_with("update-")
                && name.len() < 100
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
    {
        return Err("更新暂存目录不属于本程序".into());
    }
    Ok(stage)
}

fn kind_for_directory(directory: &Path) -> Result<PackageKind, String> {
    let current_user = RegKey::predef(HKEY_CURRENT_USER);
    for view in [KEY_WOW64_64KEY, KEY_WOW64_32KEY] {
        match current_user.open_subkey_with_flags(UNINSTALL_KEY, KEY_READ | view) {
            Ok(key) => {
                let location: String = key
                    .get_value("InstallLocation")
                    .map_err(|error| io_error("无法读取已安装版本的位置", error))?;
                if let Ok(installed) = canonical_safe(Path::new(&location)) {
                    if same_path(&installed, directory) {
                        return Ok(PackageKind::Installer);
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error("无法确认当前安装方式", error)),
        }
    }
    Ok(PackageKind::Portable)
}

fn application_exe() -> Result<PathBuf, String> {
    let executable = canonical_safe(
        &std::env::current_exe().map_err(|error| io_error("无法找到当前程序", error))?,
    )?;
    if executable.file_name().and_then(|value| value.to_str()) != Some(APP_EXE) {
        return Err("请保持程序文件名为 leigod-guard.exe 后再使用自动更新".into());
    }
    Ok(executable)
}

pub fn detect_package_kind() -> Result<PackageKind, String> {
    let executable = application_exe()?;
    kind_for_directory(executable.parent().ok_or("无法找到程序目录")?)
}

fn version_tuple(value: &str) -> Result<(u64, u64, u64), String> {
    let numbers: Vec<_> = value.split('.').collect();
    if numbers.len() != 3
        || numbers.iter().any(|number| {
            number.is_empty()
                || !number.bytes().all(|byte| byte.is_ascii_digit())
                || (number.len() > 1 && number.starts_with('0'))
        })
    {
        return Err("更新版本号无效".into());
    }
    let parse = |number: &str| {
        number
            .parse::<u64>()
            .map_err(|_| "更新版本号过大".to_owned())
    };
    Ok((parse(numbers[0])?, parse(numbers[1])?, parse(numbers[2])?))
}

fn artifact_name(version: &str, kind: PackageKind) -> String {
    let suffix = match kind {
        PackageKind::Installer => "-setup.exe",
        PackageKind::Portable => ".zip",
    };
    format!("leigod-guard-v{version}-windows-x64{suffix}")
}

fn file_sha256(path: &Path) -> Result<String, String> {
    safe_path(path)?;
    let mut input = File::open(path).map_err(|error| io_error("无法读取校验文件", error))?;
    if !input
        .metadata()
        .map_err(|error| error.to_string())?
        .is_file()
    {
        return Err("更新文件类型无效".into());
    }
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 65536];
    loop {
        let bytes = input
            .read(&mut buffer)
            .map_err(|error| io_error("无法校验文件", error))?;
        if bytes == 0 {
            break;
        }
        digest.update(&buffer[..bytes]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn verify_artifact(path: &Path, size: u64, expected_hash: &str) -> Result<(), String> {
    safe_path(path)?;
    if size == 0
        || size > MAX_ARCHIVE
        || expected_hash.len() != 64
        || !expected_hash.bytes().all(|value| value.is_ascii_hexdigit())
    {
        return Err("更新包的校验信息无效".into());
    }
    let metadata = fs::symlink_metadata(path).map_err(|error| io_error("更新包不存在", error))?;
    if !metadata.is_file() || metadata.len() != size {
        return Err("更新包大小不一致，请重新下载".into());
    }
    if !file_sha256(path)?.eq_ignore_ascii_case(expected_hash) {
        return Err("更新包 SHA-256 校验不通过，请重新下载".into());
    }
    Ok(())
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    safe_path(path)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| io_error("无法写入更新文件", error))?;
    output
        .write_all(bytes)
        .and_then(|_| output.sync_all())
        .map_err(|error| io_error("无法保存更新文件", error))
}

fn copy_new(source: &Path, destination: &Path) -> Result<(), String> {
    safe_path(source)?;
    safe_path(destination)?;
    let mut input = File::open(source).map_err(|error| io_error("无法读取程序文件", error))?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|error| io_error("无法创建程序副本", error))?;
    std::io::copy(&mut input, &mut output)
        .and_then(|_| output.sync_all())
        .map_err(|error| io_error("无法复制程序文件", error))
}

fn process_created(handle: HANDLE) -> Result<u64, String> {
    let (mut created, mut exited, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    unsafe { GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) }
        .map_err(|error| io_error("无法核实待退出的进程", error))?;
    Ok((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
}

pub fn prepare_and_launch_helper(update: &DownloadedUpdate) -> Result<(), String> {
    let executable = application_exe()?;
    let destination = executable.parent().ok_or("无法找到程序目录")?.to_path_buf();
    if kind_for_directory(&destination)? != update.kind {
        return Err("更新包类型与当前安装方式不一致，请重新检查更新".into());
    }
    if version_tuple(&update.version)? <= version_tuple(env!("CARGO_PKG_VERSION"))? {
        return Err("更新包版本必须高于当前版本".into());
    }
    let stage = validate_staging(update.path.parent().ok_or("更新包路径无效")?)?;
    let name = artifact_name(&update.version, update.kind);
    if !same_path(&canonical_safe(&update.path)?, &stage.join(&name)) {
        return Err("更新包文件名或位置无效".into());
    }
    verify_artifact(&update.path, update.size, &update.sha256)?;
    let helper = stage.join(HELPER_EXE);
    copy_new(&executable, &helper)?;
    // GNU developer builds need their existing loader beside the copied helper.
    let loader = destination.join("WebView2Loader.dll");
    if loader.is_file() {
        copy_new(&loader, &stage.join("WebView2Loader.dll"))?;
    }
    let plan = UpdatePlan {
        schema: 1,
        kind: update.kind,
        version: update.version.clone(),
        previous_version: env!("CARGO_PKG_VERSION").to_owned(),
        parent_pid: std::process::id(),
        parent_created: process_created(unsafe { GetCurrentProcess() })?,
        destination,
        artifact: name,
        artifact_size: update.size,
        sha256: update.sha256.clone(),
        previous_sha256: file_sha256(&executable)?,
    };
    let plan_path = stage.join("plan.json");
    write_new(
        &plan_path,
        &serde_json::to_vec(&plan).map_err(|error| error.to_string())?,
    )?;
    let mut child = Command::new(helper)
        .arg("--apply-update")
        .arg(&plan_path)
        .current_dir(&stage)
        .creation_flags(CREATE_NO_WINDOW.0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| io_error("无法启动更新程序", error))?;
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if stage.join("ready").is_file() {
            write_new(&stage.join("proceed"), b"apply\n")?;
            return Ok(());
        }
        if let Ok(error) = fs::read_to_string(stage.join("error.txt")) {
            return Err(error);
        }
        if child
            .try_wait()
            .map_err(|error| error.to_string())?
            .is_some()
        {
            return Err("更新程序提前退出，请手动下载新版后重试".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err("更新程序未能及时准备就绪，当前程序继续运行，请稍后重试".into())
}

fn safe_relative(name: &str) -> Result<PathBuf, String> {
    if name.is_empty() || name.len() > 240 || name.contains('\\') || name.starts_with('/') {
        return Err("ZIP 中存在无效路径".into());
    }
    let trimmed = name.strip_suffix('/').unwrap_or(name);
    for part in trimmed.split('/') {
        let stem = part
            .split('.')
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        if part.is_empty()
            || part == "."
            || part == ".."
            || part.ends_with(['.', ' '])
            || part
                .chars()
                .any(|value| value.is_control() || ":<>\"|?*".contains(value))
            || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && matches!(stem.as_bytes()[3], b'1'..=b'9'))
        {
            return Err(format!("ZIP 路径不安全：{name}"));
        }
    }
    let path = PathBuf::from(trimmed);
    if path
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err("ZIP 路径越出了程序目录".into());
    }
    Ok(path)
}

fn allowed_file(path: &Path) -> bool {
    let normalized = path
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    if matches!(
        normalized.as_str(),
        "leigod-guard.exe"
            | "webview2loader.dll"
            | "readme.md"
            | "license"
            | "third_party_notices.txt"
            | "changelog.md"
            | "config.example.toml"
            | "docs/privacy.md"
            | "docs/releasing.md"
    ) {
        return true;
    }
    if !normalized.starts_with("licenses/") {
        return false;
    }
    let filename = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    matches!(
        filename.as_str(),
        "authors" | "license" | "license-mit" | "license-apache" | "license.chromium"
    ) || [".txt", ".md", ".html", ".nuspec"]
        .iter()
        .any(|extension| filename.ends_with(extension))
}

fn make_parent(root: &Path, relative: &Path) -> Result<PathBuf, String> {
    let destination = root.join(relative);
    if !destination.starts_with(root) || relative.is_absolute() {
        return Err("更新目标越出了允许目录".into());
    }
    safe_path(&destination)?;
    fs::create_dir_all(destination.parent().ok_or("无效文件目标")?)
        .map_err(|error| io_error("无法创建更新子目录", error))?;
    safe_path(&destination)?;
    Ok(destination)
}

fn extract_portable(archive_path: &Path, destination: &Path) -> Result<Vec<PathBuf>, String> {
    safe_path(destination)?;
    fs::create_dir(destination).map_err(|error| io_error("无法准备解压目录", error))?;
    let input = File::open(archive_path).map_err(|error| io_error("无法打开绿色更新包", error))?;
    let mut archive =
        zip::ZipArchive::new(input).map_err(|error| io_error("绿色更新包格式无效", error))?;
    if archive.len() == 0 || archive.len() > MAX_FILES {
        return Err("绿色更新包的文件数量无效".into());
    }
    let mut seen = HashSet::new();
    let mut files = Vec::new();
    let mut total = 0_u64;
    // Validate the complete manifest before creating any payload file.
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|error| io_error("无法读取 ZIP 文件目录", error))?;
        let relative = safe_relative(entry.name())?;
        let normalized = relative
            .to_string_lossy()
            .replace('\\', "/")
            .to_ascii_lowercase();
        if !seen.insert(normalized)
            || entry.is_symlink()
            || entry.encrypted()
            || entry.unix_mode().is_some_and(|mode| {
                mode & 0o170000 != 0 && mode & 0o170000 != 0o100000 && mode & 0o170000 != 0o040000
            })
        {
            return Err("绿色更新包含重复文件、链接或特殊文件".into());
        }
        if entry.is_dir() {
            if relative != Path::new("docs")
                && relative != Path::new("licenses")
                && !relative.starts_with("licenses")
            {
                return Err("绿色更新包包含非发布目录".into());
            }
            continue;
        }
        if !allowed_file(&relative) {
            return Err(format!("绿色更新包包含非发布文件：{}", relative.display()));
        }
        total = total.checked_add(entry.size()).ok_or("绿色更新包过大")?;
        if entry.size() > MAX_ARCHIVE || total > MAX_UNPACKED {
            return Err("绿色更新包解压后大小超出限制".into());
        }
        files.push(relative);
    }
    if !files.iter().any(|path| path == Path::new(APP_EXE)) {
        return Err("绿色更新包缺少主程序".into());
    }
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| error.to_string())?;
        if entry.is_dir() {
            continue;
        }
        let relative = safe_relative(entry.name())?;
        let output = make_parent(destination, &relative)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(output)
            .map_err(|error| io_error("无法解压更新文件", error))?;
        let limit = entry.size();
        let bytes = std::io::copy(&mut (&mut entry).take(limit + 1), &mut file)
            .map_err(|error| io_error("解压或 CRC 校验失败", error))?;
        if bytes != limit {
            return Err("ZIP 文件实际大小与清单不一致".into());
        }
        file.sync_all()
            .map_err(|error| io_error("无法保存解压文件", error))?;
    }
    // Update the executable last, after its support files and notices.
    files.sort_by_key(|path| path == Path::new(APP_EXE));
    Ok(files)
}

fn verify_executable_version(executable: &Path, expected: &str) -> Result<(), String> {
    safe_path(executable)?;
    let mut child = Command::new(executable)
        .arg("--version")
        .current_dir(executable.parent().ok_or("无效程序目录")?)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW.0)
        .spawn()
        .map_err(|error| io_error("无法验证新版程序能否启动", error))?;
    let stdout = child.stdout.take().ok_or("无法读取新版程序版本")?;
    let reader = std::thread::spawn(move || {
        let mut output = Vec::new();
        stdout.take(4096).read_to_end(&mut output).map(|_| output)
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            break status;
        }
        if Instant::now() >= deadline {
            // This is only the isolated --version probe, never the user's app.
            let _ = child.kill();
            let _ = child.wait();
            return Err("新版程序未能通过限时启动检查，已停止更新".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let output = reader
        .join()
        .map_err(|_| "版本检查线程异常退出")?
        .map_err(|error| io_error("无法读取新版程序版本", error))?;
    let actual = String::from_utf8_lossy(&output);
    if !status.success() || actual.trim() != format!("leigod-guard {expected}") {
        return Err("新版程序的版本或启动检查不通过".into());
    }
    Ok(())
}

/// A rename-based transaction stays on the destination volume. Backups are never
/// recursively deleted; they remain available for manual recovery after a crash.
fn replace_portable(
    destination: &Path,
    unpacked: &Path,
    files: &[PathBuf],
) -> Result<PathBuf, String> {
    let transaction = unique_directory(destination, ".leigod-update-")?;
    let incoming = transaction.join("new");
    let backup = transaction.join("backup");
    fs::create_dir(&incoming)
        .and_then(|_| fs::create_dir(&backup))
        .map_err(|error| io_error("无法准备回滚目录", error))?;
    for relative in files {
        let target = destination.join(relative);
        safe_path(&target)?;
        if target.exists()
            && !fs::metadata(&target)
                .map_err(|error| error.to_string())?
                .is_file()
        {
            return Err(format!("目标文件已被同名目录占用：{}", relative.display()));
        }
        copy_new(&unpacked.join(relative), &make_parent(&incoming, relative)?)?;
    }
    let mut changed: Vec<(PathBuf, bool, bool)> = Vec::new();
    let apply = (|| -> Result<(), String> {
        for relative in files {
            let target = make_parent(destination, relative)?;
            let old = make_parent(&backup, relative)?;
            let had_original = target.exists();
            if had_original {
                safe_path(&target)?;
                fs::rename(&target, &old)
                    .map_err(|error| io_error("无法备份旧程序，请关闭占用文件的软件", error))?;
            }
            changed.push((relative.clone(), had_original, false));
            safe_path(&target)?;
            fs::rename(incoming.join(relative), &target)
                .map_err(|error| io_error("无法替换程序文件", error))?;
            changed.last_mut().unwrap().2 = true;
        }
        Ok(())
    })();
    if let Err(error) = apply {
        let mut recovery_errors = Vec::new();
        for (relative, had_original, installed) in changed.iter().rev() {
            let target = destination.join(relative);
            let restore = (|| -> Result<(), String> {
                safe_path(&target)?;
                if *installed {
                    fs::rename(&target, incoming.join(relative))
                        .map_err(|error| error.to_string())?;
                }
                if *had_original {
                    let original = backup.join(relative);
                    safe_path(&original)?;
                    fs::rename(original, target).map_err(|error| error.to_string())?;
                }
                Ok(())
            })();
            if let Err(error) = restore {
                recovery_errors.push(error);
            }
        }
        if !recovery_errors.is_empty() {
            return Err(format!(
                "{error}\n部分文件未能自动恢复。旧文件保留于：{}\n{}",
                backup.display(),
                recovery_errors.join("\n")
            ));
        }
        return Err(format!("{error}\n已恢复原有程序文件。"));
    }
    Ok(transaction)
}

fn restart(executable: &Path) -> Result<(), String> {
    safe_path(executable)?;
    Command::new(executable)
        .current_dir(executable.parent().ok_or("程序目录无效")?)
        .creation_flags(CREATE_NO_WINDOW.0)
        .spawn()
        .map(|_| ())
        .map_err(|error| io_error("无法重新打开程序，请手动启动", error))
}

fn validate_plan(plan_path: &Path) -> Result<(PathBuf, UpdatePlan), String> {
    let plan_path = canonical_safe(plan_path)?;
    let stage = validate_staging(plan_path.parent().ok_or("更新计划目录无效")?)?;
    if plan_path.file_name().and_then(|value| value.to_str()) != Some("plan.json") {
        return Err("更新计划文件名无效".into());
    }
    let current = canonical_safe(&std::env::current_exe().map_err(|error| error.to_string())?)?;
    if !same_path(&current, &stage.join(HELPER_EXE)) {
        return Err("更新助手必须从专用暂存目录运行".into());
    }
    if fs::metadata(&plan_path)
        .map_err(|error| io_error("无法读取更新计划", error))?
        .len()
        > 16_384
    {
        return Err("更新计划过大".into());
    }
    let mut bytes = Vec::new();
    File::open(&plan_path)
        .map_err(|error| io_error("无法读取更新计划", error))?
        .take(16_385)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > 16_384 {
        return Err("更新计划过大".into());
    }
    let plan: UpdatePlan =
        serde_json::from_slice(&bytes).map_err(|error| io_error("更新计划格式无效", error))?;
    if plan.schema != 1
        || plan.previous_version != env!("CARGO_PKG_VERSION")
        || version_tuple(&plan.version)? <= version_tuple(&plan.previous_version)?
        || plan.artifact != artifact_name(&plan.version, plan.kind)
        || plan.parent_pid == 0
        || plan.parent_pid == std::process::id()
    {
        return Err("更新计划与当前程序不匹配".into());
    }
    let destination = canonical_safe(&plan.destination)?;
    if !same_path(&destination, &plan.destination)
        || destination.parent().is_none()
        || same_path(&destination, &stage)
        || stage.starts_with(&destination)
        || kind_for_directory(&destination)? != plan.kind
    {
        return Err("更新目标或安装方式发生变化，已停止更新".into());
    }
    let original = destination.join(APP_EXE);
    if file_sha256(&original)? != plan.previous_sha256
        || file_sha256(&current)? != plan.previous_sha256
    {
        return Err("更新助手或原程序已经变化，已停止更新".into());
    }
    verify_artifact(
        &stage.join(&plan.artifact),
        plan.artifact_size,
        &plan.sha256,
    )?;
    Ok((stage, plan))
}

fn run_inner(plan_path: &Path) -> Result<(), String> {
    let (stage, plan) = validate_plan(plan_path)?;
    let parent = ProcessHandle(
        unsafe {
            OpenProcess(
                PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                false,
                plan.parent_pid,
            )
        }
        .map_err(|error| io_error("无法确认旧程序是否仍在运行", error))?,
    );
    if process_created(parent.0)? != plan.parent_created {
        return Err("待退出的程序身份不匹配，已停止更新".into());
    }
    let artifact = stage.join(&plan.artifact);
    let unpacked = stage.join("unpacked");
    let files = if plan.kind == PackageKind::Portable {
        let files = extract_portable(&artifact, &unpacked)?;
        verify_executable_version(&unpacked.join(APP_EXE), &plan.version)?;
        Some(files)
    } else {
        None
    };
    write_new(&stage.join("ready"), b"verified\n")?;
    let deadline = Instant::now() + Duration::from_secs(20);
    while !stage.join("proceed").is_file() {
        if Instant::now() >= deadline {
            return Err("主程序没有确认开始更新，程序文件未被改动".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if unsafe { WaitForSingleObject(parent.0, 60_000) } != WAIT_OBJECT_0 {
        return Err("旧程序尚未退出，已停止更新。请从托盘退出后再试。".into());
    }
    // Keep another portable instance from opening during replacement. Setup has
    // its own AppMutex check, so the installer branch must not hold this mutex.
    let portable_guard = if plan.kind == PackageKind::Portable {
        Some(
            crate::instance::InstanceGuard::acquire()
                .map_err(|error| io_error("无法保护更新过程", error))?
                .ok_or("程序已被重新打开，请退出后再次更新")?,
        )
    } else {
        None
    };
    let executable = plan.destination.join(APP_EXE);
    let outcome = (|| -> Result<(), String> {
        // Recheck immediately before the first write to the application directory.
        safe_path(&plan.destination)?;
        verify_artifact(&artifact, plan.artifact_size, &plan.sha256)?;
        if file_sha256(&executable)? != plan.previous_sha256 {
            return Err("旧程序在准备更新期间发生变化，已停止更新".into());
        }
        if let Some(files) = files {
            replace_portable(&plan.destination, &unpacked, &files).map(|backup| {
                let _ = write_new(
                    &stage.join("recovery-path.txt"),
                    backup.to_string_lossy().as_bytes(),
                );
            })
        } else {
            // Inno Setup keeps the user's existing tasks and uses its own rollback.
            // skipifsilent in the installer prevents it from racing our restart.
            let status = Command::new(&artifact)
                .args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/SP-"])
                .arg(format!("/DIR={}", ordinary_path(&plan.destination)))
                .arg(format!("/LOG={}", ordinary_path(&stage.join("setup.log"))))
                .current_dir(&stage)
                .creation_flags(CREATE_NO_WINDOW.0)
                .status()
                .map_err(|error| io_error("无法运行安装版更新", error));
            match status {
                Ok(status) if status.success() => {
                    verify_executable_version(&executable, &plan.version)
                }
                Ok(status) => Err(format!(
                    "安装版更新未完成（退出代码 {}），详情见 {}",
                    status.code().unwrap_or(-1),
                    stage.join("setup.log").display()
                )),
                Err(error) => Err(error),
            }
        }
    })();
    drop(portable_guard);
    if let Err(error) = outcome {
        // Only restart an unchanged/restored old executable after a failed apply.
        if file_sha256(&executable).is_ok_and(|hash| hash == plan.previous_sha256) {
            if let Err(restart_error) = restart(&executable) {
                return Err(format!("{error}\n旧版文件已保留，但没有重新打开，请手动启动雷神守护。\n{restart_error}"));
            }
            return Err(format!("{error}\n已重新打开原版本。"));
        }
        return Err(format!(
            "{error}\n程序未自动重新打开，请按提示恢复或手动安装后启动。"
        ));
    }
    let _ = write_new(
        &stage.join("complete.txt"),
        format!("Updated to {}\n", plan.version).as_bytes(),
    );
    restart(&executable)
}

fn ordinary_path(path: &Path) -> String {
    let display = path.to_string_lossy();
    display.strip_prefix(r"\\?\").unwrap_or(&display).to_owned()
}

pub fn run_helper(plan_path: &Path) -> Result<(), String> {
    run_inner(plan_path).map_err(|error| {
        let message = format!("{error}\n\n账号和偏好设置不会被删除。手动下载：{MANUAL_URL}");
        if let Some(parent) = plan_path.parent() {
            if let Ok(stage) = validate_staging(parent) {
                let _ = write_new(&stage.join("error.txt"), message.as_bytes());
            }
        }
        message
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use zip::write::SimpleFileOptions;

    fn fixture() -> PathBuf {
        unique_directory(
            &fs::canonicalize(std::env::temp_dir()).unwrap(),
            "leigod-apply-test-",
        )
        .unwrap()
    }

    fn archive(path: &Path, entries: &[(&str, &[u8])]) {
        let mut archive = zip::ZipWriter::new(File::create(path).unwrap());
        for (name, bytes) in entries {
            archive
                .start_file(*name, SimpleFileOptions::default())
                .unwrap();
            archive.write_all(bytes).unwrap();
        }
        archive.finish().unwrap();
    }

    #[test]
    fn archive_paths_reject_traversal_windows_devices_and_ads() {
        for name in [
            "../evil.exe",
            "C:/evil.exe",
            "/evil.exe",
            "a\\evil.exe",
            "licenses/../x",
            "licenses/a.txt:payload",
            "licenses/CON.txt",
            "licenses/a. ",
            "licenses//a.txt",
        ] {
            assert!(safe_relative(name).is_err(), "accepted {name}");
        }
        assert!(safe_relative("licenses/library/LICENSE-MIT").is_ok());
        assert!(!allowed_file(Path::new("config.toml")));
        assert!(!allowed_file(Path::new("licenses/install.exe")));
        assert!(!allowed_file(Path::new("licenses/LICENSE-payload.exe")));
    }

    #[test]
    fn malformed_archive_cannot_write_payload_files() {
        let root = fixture();
        for (index, bad) in [
            "../escape.txt",
            "config.toml",
            "README.md:ads",
            "LEIGOD-GUARD.exe",
        ]
        .iter()
        .enumerate()
        {
            let package = root.join(format!("bad-{index}.zip"));
            let unpacked = root.join(format!("bad-{index}"));
            archive(&package, &[(APP_EXE, b"new"), (bad, b"bad")]);
            assert!(extract_portable(&package, &unpacked).is_err());
            assert!(!unpacked.join(APP_EXE).exists());
        }
        assert!(!root.join("escape.txt").exists());
    }

    #[test]
    fn archive_symlink_is_rejected_before_any_payload_is_written() {
        let root = fixture();
        let package = root.join("symlink.zip");
        let unpacked = root.join("unpacked");
        let mut zip = zip::ZipWriter::new(File::create(&package).unwrap());
        zip.start_file(APP_EXE, SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"fixture executable").unwrap();
        zip.add_symlink("README.md", "../outside.txt", SimpleFileOptions::default())
            .unwrap();
        zip.finish().unwrap();
        assert!(extract_portable(&package, &unpacked).is_err());
        assert!(!unpacked.join(APP_EXE).exists());
    }

    #[test]
    fn portable_apply_keeps_user_files_and_records_old_program() {
        let root = fixture();
        let destination = root.join("portable");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join(APP_EXE), b"old executable").unwrap();
        fs::write(destination.join("config.toml"), b"fixture only").unwrap();
        fs::write(destination.join("my-notes.txt"), b"keep me").unwrap();
        let package = root.join("good.zip");
        archive(
            &package,
            &[(APP_EXE, b"new executable"), ("README.md", b"new readme")],
        );
        let unpacked = root.join("unpacked");
        let files = extract_portable(&package, &unpacked).unwrap();
        let transaction = replace_portable(&destination, &unpacked, &files).unwrap();
        assert_eq!(
            fs::read(destination.join(APP_EXE)).unwrap(),
            b"new executable"
        );
        assert_eq!(
            fs::read(transaction.join("backup").join(APP_EXE)).unwrap(),
            b"old executable"
        );
        assert_eq!(
            fs::read(destination.join("config.toml")).unwrap(),
            b"fixture only"
        );
        assert_eq!(
            fs::read(destination.join("my-notes.txt")).unwrap(),
            b"keep me"
        );
    }

    #[test]
    fn locked_executable_rolls_back_preceding_files() {
        use std::os::windows::fs::OpenOptionsExt;
        let root = fixture();
        let destination = root.join("portable");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join(APP_EXE), b"old executable").unwrap();
        fs::write(destination.join("README.md"), b"old readme").unwrap();
        let package = root.join("update.zip");
        archive(
            &package,
            &[("README.md", b"new readme"), (APP_EXE, b"new executable")],
        );
        let unpacked = root.join("unpacked");
        let files = extract_portable(&package, &unpacked).unwrap();
        let _locked = OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(destination.join(APP_EXE))
            .unwrap();
        assert!(replace_portable(&destination, &unpacked, &files).is_err());
        assert_eq!(
            fs::read(destination.join(APP_EXE)).unwrap(),
            b"old executable"
        );
        assert_eq!(
            fs::read(destination.join("README.md")).unwrap(),
            b"old readme"
        );
    }

    #[test]
    fn corrupt_download_is_rejected_before_apply() {
        let root = fixture();
        let artifact = root.join("package.zip");
        fs::write(&artifact, b"fixture").unwrap();
        let hash = file_sha256(&artifact).unwrap();
        assert!(verify_artifact(&artifact, 7, &hash).is_ok());
        assert!(verify_artifact(&artifact, 8, &hash).is_err());
        fs::write(&artifact, b"changed").unwrap();
        assert!(verify_artifact(&artifact, 7, &hash).is_err());
    }
}
