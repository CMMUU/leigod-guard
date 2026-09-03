//! 开机自启：写 HKCU\...\Run，无需管理员权限。
use winreg::enums::*;
use winreg::RegKey;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "LeigodGuard";

pub fn is_enabled() -> bool {
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(RUN_KEY)
        .and_then(|k| k.get_value::<String, _>(VALUE_NAME))
        .is_ok()
}

pub fn set_enabled(enable: bool) -> Result<(), String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu
        .create_subkey(RUN_KEY)
        .map_err(|e| format!("打开注册表失败: {e}"))?;
    if enable {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let cmd = format!("\"{}\" --minimized", exe.display());
        key.set_value(VALUE_NAME, &cmd)
            .map_err(|e| format!("写入注册表失败: {e}"))?;
    } else {
        // 不存在时删除会报错，忽略
        let _ = key.delete_value(VALUE_NAME);
    }
    Ok(())
}
