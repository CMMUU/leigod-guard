//! 关机/注销前自动暂停计时。
//!
//! Windows 关机或注销时，系统会向所有顶层窗口广播 WM_QUERYENDSESSION /
//! WM_ENDSESSION。这里在独立线程创建一个永不显示的顶层窗口来接收广播：
//! - WM_QUERYENDSESSION → 返回 TRUE（不阻止关机）
//! - WM_ENDSESSION(TRUE) → 使用内存凭据立即尝试暂停（已暂停时接口按成功处理）
//!
//! 注意：消息循环结束后 drop 窗口可能触发问题，窗口与类直接泄漏（进程级一次性资源）。
use crate::config::Config;
use crate::leigod_api as api;
use crate::shared::Shared;
use std::sync::{Arc, Mutex, OnceLock};

static SHARED: OnceLock<Arc<Mutex<Shared>>> = OnceLock::new();
static CONFIG: OnceLock<Arc<Mutex<Config>>> = OnceLock::new();

/// 启动关机监听线程（进程内调用一次）
pub fn start(shared: Arc<Mutex<Shared>>, config: Arc<Mutex<Config>>) {
    let _ = SHARED.set(shared);
    let _ = CONFIG.set(config);
    std::thread::spawn(|| unsafe { window_thread() });
}

fn log_line(msg: &str) {
    crate::ui::dbglog(&format!("[shutdown] {msg}"));
    if let Some(s) = SHARED.get() {
        if let Ok(mut s) = s.lock() {
            s.log(&format!("[关机检测] {msg}"));
        }
    }
}

/// 关机/注销时尽力暂停；系统终止进程或断网可能使请求无法完成。
fn on_session_ending() {
    log_line("收到系统关机/注销广播");
    let Some(cfg) = CONFIG.get() else { return };
    let enabled = match cfg.lock() {
        Ok(c) => c.strategy.pause_on_shutdown,
        Err(_) => return,
    };
    if !enabled {
        log_line("关机前自动暂停已被用户关闭，跳过");
        return;
    }
    // token 只存在于内存（Shared），不落盘明文
    let token = match SHARED.get().and_then(|s| s.lock().ok()) {
        Some(s) => s.token.clone().unwrap_or_default(),
        None => String::new(),
    };
    if token.is_empty() {
        log_line("未登录，无法自动暂停");
        return;
    }
    // 关机窗口期有限，直接调用幂等暂停接口，不再先花一次请求查询状态。
    match api::pause(&token) {
        Ok(m) => log_line(&format!("关机前自动暂停成功: {m}")),
        Err(e) => log_line(&format!("关机前自动暂停失败: {e}")),
    }
}

unsafe extern "system" fn wndproc(
    hwnd: windows::Win32::Foundation::HWND,
    msg: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::Foundation::LRESULT;
    use windows::Win32::UI::WindowsAndMessaging::DefWindowProcW;
    const WM_QUERYENDSESSION: u32 = 0x0011;
    const WM_ENDSESSION: u32 = 0x0016;
    match msg {
        // 允许关机（返回 TRUE）
        WM_QUERYENDSESSION => LRESULT(1),
        // 会话正在结束（wparam=TRUE）
        WM_ENDSESSION if wparam.0 != 0 => {
            on_session_ending();
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn window_thread() {
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DispatchMessageW, GetMessageW, RegisterClassW, TranslateMessage, MSG,
        WINDOW_EX_STYLE, WNDCLASSW, WS_OVERLAPPED,
    };
    let class: Vec<u16> = "LeigodGuardShutdown\0".encode_utf16().collect();
    let wc = WNDCLASSW {
        lpfnWndProc: Some(wndproc),
        lpszClassName: PCWSTR(class.as_ptr()),
        ..Default::default()
    };
    let _ = RegisterClassW(&wc);
    // 普通顶层窗口（不能用 HWND_MESSAGE 消息窗口：收不到关机广播），永不显示
    let hwnd = CreateWindowExW(
        WINDOW_EX_STYLE::default(),
        PCWSTR(class.as_ptr()),
        PCWSTR::null(),
        WS_OVERLAPPED,
        0,
        0,
        0,
        0,
        None,
        None,
        None,
        None,
    );
    match hwnd {
        Ok(h) if !h.is_invalid() => {
            crate::ui::dbglog("[shutdown] hidden window created, listening");
        }
        _ => {
            crate::ui::dbglog("[shutdown] 创建关机监听窗口失败");
            return;
        }
    }
    let mut msg = MSG::default();
    loop {
        let result = GetMessageW(&mut msg, None, 0, 0).0;
        if result <= 0 {
            if result == -1 {
                crate::ui::dbglog("[shutdown] 消息循环发生错误，关机监听已停止");
            }
            break;
        }
        let _ = TranslateMessage(&msg);
        let _ = DispatchMessageW(&msg);
    }
}
