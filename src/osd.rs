//! OSD 工具（微星小飞机 / RTSS）排除配置
//!
//! Windows 没有"声明自己不是游戏"的系统机制：RTSS / 游戏加加这类 OSD 工具
//! 是靠向使用 DirectX 的进程注入钩子来判断的。RTSS 官方支持进程级排除——
//! 在 Profiles 目录放一个与 exe 同名的 .cfg（与 RTSS 自带模板 7zFM.exe.cfg、
//! AcroRd32.exe.cfg 完全一致），内容 EnableHooking=0 即可不再注入本进程。

use std::path::{Path, PathBuf};

const RTSS_PROFILE_DIR: &str = r"C:\Program Files (x86)\RivaTuner Statistics Server\Profiles";
const PROFILE_NAME: &str = "leigod-guard.exe.cfg";
const PROFILE_BODY: &str = "[Hooking]\r\nEnableHooking\t\t= 0\r\n";

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
