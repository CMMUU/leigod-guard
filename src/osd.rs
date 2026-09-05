//! OSD 工具（微星小飞机 / RTSS、游戏加加）兼容处理
//!
//! Windows 没有"声明自己不是游戏"的系统机制：RTSS / 游戏加加这类 OSD 工具
//! 是靠向使用 DirectX 的进程注入钩子来判断的。RTSS 官方支持进程级排除——
//! 在 Profiles 目录放一个与 exe 同名的 .cfg（与 RTSS 自带模板 7zFM.exe.cfg、
//! AcroRd32.exe.cfg 完全一致），内容 EnableHooking=0 即可不再注入本进程。
//!
//! 游戏加加没有公开的进程级排除接口。用户明确开启严格屏蔽后，主程序在拿
//! 单实例锁和初始化 DirectX 之前，以 Windows 进程创建缓解策略重新启动自身。
//! 该策略只作用于新建的雷神守护进程，但会阻止所有不属于 Microsoft、Store
//! 或 WHQL 信任范围的 DLL，而不仅是游戏加加模块。

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows::core::{HRESULT, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, ERROR_BAD_LENGTH, ERROR_NO_MORE_FILES, HANDLE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Module32FirstW, Module32NextW, MODULEENTRY32W, TH32CS_SNAPMODULE,
    TH32CS_SNAPMODULE32,
};
use windows::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, GetCurrentProcess, GetCurrentProcessId,
    GetProcessMitigationPolicy, InitializeProcThreadAttributeList, ProcessSignaturePolicy,
    UpdateProcThreadAttribute, EXTENDED_STARTUPINFO_PRESENT, LPPROC_THREAD_ATTRIBUTE_LIST,
    PROCESS_INFORMATION, PROC_THREAD_ATTRIBUTE_MITIGATION_POLICY, STARTUPINFOEXW,
};

const RTSS_PROFILE_DIR: &str = r"C:\Program Files (x86)\RivaTuner Statistics Server\Profiles";
const PROFILE_NAME: &str = "leigod-guard.exe.cfg";
const PROFILE_BODY: &str = "[Hooking]\r\nEnableHooking\t\t= 0\r\n";
const GAMEPP_PROTECTED_ARG: &str = "--gamepp-protected";
const GAMEPP_DISABLE_VULKAN_ENV: &str = "DISABLE_GAMEPP_LAYER";
static GAMEPP_PROTECTION_ERROR: OnceLock<String> = OnceLock::new();

// WinBase.h: PROCESS_CREATION_MITIGATION_POLICY_BLOCK_NON_MICROSOFT_BINARIES_ALLOW_STORE.
// Windows exposes the resulting policy as StoreSignedOnly + MitigationOptIn, allowing
// Microsoft, Store and WHQL images while rejecting ordinary vendor-signed/unsigned DLLs.
const BLOCK_NON_MICROSOFT_BINARIES_ALLOW_STORE: u64 = 3u64 << 44;

struct AttributeList {
    _storage: Vec<usize>,
    raw: LPPROC_THREAD_ATTRIBUTE_LIST,
}

impl AttributeList {
    fn with_mitigation_policy(policy: &u64) -> Result<Self, String> {
        let mut byte_len = 0usize;
        // The first call is a documented size query and normally reports
        // ERROR_INSUFFICIENT_BUFFER. Only the returned non-zero size matters.
        unsafe {
            let _ = InitializeProcThreadAttributeList(
                LPPROC_THREAD_ATTRIBUTE_LIST::default(),
                1,
                0,
                &mut byte_len,
            );
        }
        if byte_len == 0 {
            return Err("Windows 未返回进程保护属性所需空间".into());
        }

        let word_len = byte_len.div_ceil(std::mem::size_of::<usize>());
        let mut storage = vec![0usize; word_len];
        let raw = LPPROC_THREAD_ATTRIBUTE_LIST(storage.as_mut_ptr().cast());
        unsafe {
            InitializeProcThreadAttributeList(raw, 1, 0, &mut byte_len)
                .map_err(|error| format!("初始化 Windows 进程保护属性失败: {error}"))?;
            if let Err(error) = UpdateProcThreadAttribute(
                raw,
                0,
                PROC_THREAD_ATTRIBUTE_MITIGATION_POLICY as usize,
                Some((policy as *const u64).cast()),
                std::mem::size_of::<u64>(),
                None,
                None,
            ) {
                DeleteProcThreadAttributeList(raw);
                return Err(format!("设置 Windows 进程保护属性失败: {error}"));
            }
        }
        Ok(Self {
            _storage: storage,
            raw,
        })
    }
}

impl Drop for AttributeList {
    fn drop(&mut self) {
        unsafe { DeleteProcThreadAttributeList(self.raw) };
    }
}

struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

/// Prepare the optional strict overlay protection before the main instance starts.
///
/// `Ok(true)` means a protected replacement was started and this bootstrap process
/// must return immediately. `Ok(false)` means the current process is already protected.
pub fn prepare_gamepp_protection(args: &[String]) -> GameppProtectionPreparation {
    // This is the official Vulkan implicit-layer opt-out declared by the local
    // GamePP manifest. It complements the Windows DLL policy; by itself it would
    // not block GamePP's DirectX/general injection modules.
    std::env::set_var(GAMEPP_DISABLE_VULKAN_ENV, "1");

    if gamepp_protection_active() {
        return GameppProtectionPreparation::Active;
    }
    if args.iter().any(|arg| arg == GAMEPP_PROTECTED_ARG) {
        return GameppProtectionPreparation::Abort(
            "受保护进程已经启动，但 Windows 未确认严格 DLL 签名策略。为避免绕过保护，本次不会继续启动；请按正常方式重新打开本工具。"
                .into(),
        );
    }

    let minimized = args.iter().any(|arg| arg == "--minimized");
    match relaunch_with_gamepp_protection(minimized) {
        Ok(()) => GameppProtectionPreparation::Relaunched,
        Err(message) => {
            let _ = GAMEPP_PROTECTION_ERROR.set(message.clone());
            GameppProtectionPreparation::ContinueUnprotected(message)
        }
    }
}

pub enum GameppProtectionPreparation {
    Relaunched,
    Active,
    /// The bootstrap could not create a child. Continue so the user can turn
    /// the option off instead of being locked out of the application.
    ContinueUnprotected(String),
    /// A process claiming to be the protected child is not actually protected.
    /// Do not run it unprotected, or the internal marker would become a bypass.
    Abort(String),
}

pub fn gamepp_protection_error() -> Option<&'static str> {
    GAMEPP_PROTECTION_ERROR.get().map(String::as_str)
}

fn relaunch_with_gamepp_protection(minimized: bool) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;

    let executable =
        std::env::current_exe().map_err(|error| format!("无法定位当前程序文件: {error}"))?;
    let mut executable_wide: Vec<u16> = executable.as_os_str().encode_wide().collect();
    executable_wide.push(0);

    // lpApplicationName is passed separately, so the mutable command line only
    // needs a correctly quoted argv[0] plus the two fixed internal switches.
    let mut command_line = vec![b'"' as u16];
    command_line.extend(executable.as_os_str().encode_wide());
    command_line.extend("\" ".encode_utf16());
    command_line.extend(GAMEPP_PROTECTED_ARG.encode_utf16());
    if minimized {
        command_line.extend(" --minimized".encode_utf16());
    }
    command_line.push(0);

    let policy = BLOCK_NON_MICROSOFT_BINARIES_ALLOW_STORE;
    let attributes = AttributeList::with_mitigation_policy(&policy)?;
    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    startup.lpAttributeList = attributes.raw;
    let mut process_info = PROCESS_INFORMATION::default();

    unsafe {
        CreateProcessW(
            PCWSTR(executable_wide.as_ptr()),
            PWSTR(command_line.as_mut_ptr()),
            None,
            None,
            false,
            EXTENDED_STARTUPINFO_PRESENT,
            None,
            PCWSTR::null(),
            &startup.StartupInfo,
            &mut process_info,
        )
        .map_err(|error| format!("启动受保护的雷神守护进程失败: {error}"))?;
    }

    let _process = OwnedHandle(process_info.hProcess);
    let _thread = OwnedHandle(process_info.hThread);
    Ok(())
}

/// Whether Windows reports an enforcing binary-signature policy for this process.
pub fn gamepp_protection_active() -> bool {
    let mut flags = 0u32;
    let result = unsafe {
        GetProcessMitigationPolicy(
            GetCurrentProcess(),
            ProcessSignaturePolicy,
            (&mut flags as *mut u32).cast(),
            std::mem::size_of::<u32>(),
        )
    };
    // MitigationOptIn alone (bit 2) is advisory and did not enforce loading in
    // validation. MicrosoftSignedOnly or StoreSignedOnly confirms enforcement.
    result.is_ok() && flags & 0b11 != 0
}

/// Check whether GamePP modules were already present in this process.
/// `None` means Windows did not allow the current module list to be inspected.
pub fn gamepp_modules_loaded() -> Option<bool> {
    static CACHE: OnceLock<Mutex<Option<(Instant, Option<bool>)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    if let Ok(mut cached) = cache.lock() {
        if let Some((checked_at, loaded)) = *cached {
            if checked_at.elapsed() < Duration::from_secs(2) {
                return loaded;
            }
        }
        let loaded = scan_gamepp_modules();
        *cached = Some((Instant::now(), loaded));
        return loaded;
    }
    scan_gamepp_modules()
}

fn scan_gamepp_modules() -> Option<bool> {
    let process_id = unsafe { GetCurrentProcessId() };
    let mut snapshot = None;
    for _ in 0..4 {
        match unsafe {
            CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, process_id)
        } {
            Ok(handle) => {
                snapshot = Some(OwnedHandle(handle));
                break;
            }
            Err(error) if error.code() == HRESULT::from_win32(ERROR_BAD_LENGTH.0) => continue,
            Err(_) => return None,
        }
    }
    let snapshot = snapshot?;
    let mut module = MODULEENTRY32W {
        dwSize: std::mem::size_of::<MODULEENTRY32W>() as u32,
        ..Default::default()
    };
    if unsafe { Module32FirstW(snapshot.0, &mut module) }.is_err() {
        return None;
    }
    loop {
        let end = module
            .szExePath
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(module.szExePath.len());
        let path = String::from_utf16_lossy(&module.szExePath[..end]).to_ascii_lowercase();
        if is_gamepp_module_path(&path) {
            return Some(true);
        }
        if let Err(error) = unsafe { Module32NextW(snapshot.0, &mut module) } {
            return if error.code() == HRESULT::from_win32(ERROR_NO_MORE_FILES.0) {
                Some(false)
            } else {
                None
            };
        }
    }
}

fn is_gamepp_module_path(path: &str) -> bool {
    let normalized = path.replace('/', "\\").to_ascii_lowercase();
    normalized.contains("\\gamepp\\")
        || normalized.contains("\\gameppsdk\\")
        || normalized.contains("\\programdata\\gameppsdk\\")
}

#[cfg(test)]
mod tests {
    use super::is_gamepp_module_path;

    #[test]
    fn recognizes_gamepp_modules_without_treating_unrelated_dlls_as_gamepp() {
        for path in [
            r"C:\Users\Test\AppData\Local\GamePPSDK\1.2\GPP64.dll",
            r"C:\ProgramData\GamePPSdk\1.2\vulkan\GPP_VKLayer64.dll",
            r"C:/Program Files (x86)/GamePP/OverlayClient64.node",
        ] {
            assert!(is_gamepp_module_path(path), "missed {path}");
        }
        for path in [
            r"C:\Windows\System32\d3d12.dll",
            r"C:\Windows\System32\DriverStore\FileRepository\nvwgf2umx.dll",
            r"C:\Tools\my-gamepp-notes\plugin.dll",
        ] {
            assert!(!is_gamepp_module_path(path), "false positive {path}");
        }
    }
}

fn profile_path() -> PathBuf {
    Path::new(RTSS_PROFILE_DIR).join(PROFILE_NAME)
}

/// 是否检测到 RTSS（微星小飞机）安装
pub fn rtss_installed() -> bool {
    Path::new(RTSS_PROFILE_DIR).is_dir()
}

/// 排除配置是否已存在
pub fn rtss_excluded() -> bool {
    profile_path().exists()
}

/// 写入 RTSS 排除配置。直接写失败（无管理员权限）时通过 UAC 提权复制。
/// 返回给用户看的结果描述。
pub fn apply_rtss_exclusion() -> Result<String, String> {
    if !rtss_installed() {
        return Err("未检测到微星小飞机（RTSS）安装目录".into());
    }
    let dst = profile_path();
    // 先尝试直接写（少数机器 Profiles 目录 ACL 较宽松）
    if std::fs::write(&dst, PROFILE_BODY).is_ok() {
        return Ok("已写入排除配置，重启微星小飞机后生效".into());
    }
    // 落到自己的配置目录，再提权复制过去
    let src = dirs::config_dir()
        .ok_or("无法定位配置目录")?
        .join("leigod-guard")
        .join(PROFILE_NAME);
    if let Some(parent) = src.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&src, PROFILE_BODY).map_err(|e| format!("临时配置写入失败: {e}"))?;
    runas_copy(&src, &dst)?;
    Ok("已弹出管理员授权窗口，点击「是」后生效（生效后本页状态会自动刷新）".into())
}

/// 以管理员权限执行 cmd /c copy（触发 UAC 授权弹窗）
fn runas_copy(src: &Path, dst: &Path) -> Result<(), String> {
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

    let verb: Vec<u16> = "runas\0".encode_utf16().collect();
    let file: Vec<u16> = "cmd.exe\0".encode_utf16().collect();
    let params = format!("/c copy /Y \"{}\" \"{}\"", src.display(), dst.display());
    let mut params16: Vec<u16> = params.encode_utf16().collect();
    params16.push(0);

    // 返回值 <= 32 表示失败（含用户取消 UAC）
    let r = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(verb.as_ptr()),
            PCWSTR(file.as_ptr()),
            PCWSTR(params16.as_ptr()),
            PCWSTR::null(),
            SW_HIDE,
        )
    };
    if (r.0 as usize) <= 32 {
        return Err("提权被取消或失败".into());
    }
    Ok(())
}
