//! Best-effort shutdown pause with a main-window handler and a fallback window.
use crate::{config::Config, leigod_api as api, session_end::SessionEnd, shared::Shared};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::sync::{mpsc, Arc, Mutex, OnceLock, TryLockError};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};

static HANDLER: OnceLock<Arc<SessionEnd>> = OnceLock::new();
static TRACE: OnceLock<mpsc::SyncSender<String>> = OnceLock::new();

// Window procedures must not wait for the UI's shared/configuration mutexes or
// disk logging. Diagnostics are queued to a separate thread and contain no token.
fn trace(message: &str) {
    if let Some(sender) = TRACE.get() {
        let _ = sender.try_send(message.to_owned());
    }
}

pub fn start(shared: Arc<Mutex<Shared>>, config: Arc<Mutex<Config>>) {
    let (sender, receiver) = mpsc::sync_channel::<String>(64);
    let _ = TRACE.set(sender);
    let _ = std::thread::Builder::new()
        .name("shutdown-log".into())
        .spawn(move || {
            for message in receiver {
                crate::ui::dbglog(&format!("[shutdown] {message}"));
            }
        });
    let handler = SessionEnd::new(
        move |deadline| {
            let result = pause_before_deadline(&shared, &config, deadline, api::pause_for_shutdown);
            // Finish writing the outcome before releasing the waiting windows.
            // A slow disk writer is still bounded by SessionEnd's outer deadline.
            crate::ui::dbglog(&format!("[shutdown] {result}"));
        },
        trace,
    );
    if HANDLER.set(handler).is_err() {
        return;
    }
    // Higher levels are processed first; normal applications use 0x280.
    // No shutdown cancellation, registry timeout changes or administrator right.
    unsafe {
        if windows::Win32::System::Threading::SetProcessShutdownParameters(0x3ff, 0).is_err() {
            trace("无法调整关机通知顺序，继续使用系统默认顺序");
        }
    }
    if std::thread::Builder::new()
        .name("shutdown-window".into())
        .spawn(|| unsafe { window_thread() })
        .is_err()
    {
        trace("独立关机监听线程启动失败，将依赖主窗口监听");
    }
}

pub fn install_main_window(cc: &eframe::CreationContext<'_>) -> Result<(), String> {
    let handle = cc.window_handle().map_err(|_| "无法取得主窗口句柄")?;
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return Err("无法取得 Windows 主窗口句柄".into());
    };
    let handler = HANDLER.get().ok_or("关机监听尚未初始化")?;
    crate::session_end::install(HWND(handle.hwnd.get() as *mut _), Arc::clone(handler))?;
    trace("主窗口关机监听已安装（显示、隐藏及静默启动均生效）");
    Ok(())
}

fn snapshot<T, R>(lock: &Mutex<T>, deadline: Instant, read: impl FnOnce(&T) -> R) -> Option<R> {
    loop {
        match lock.try_lock() {
            Ok(value) => return Some(read(&value)),
            Err(TryLockError::Poisoned(_)) => return None,
            Err(TryLockError::WouldBlock) => {
                if Instant::now() >= deadline {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
}

fn pause_before_deadline(
    shared: &Mutex<Shared>,
    config: &Mutex<Config>,
    deadline: Instant,
    pause: impl FnOnce(&str, Duration) -> Result<String, api::ApiError>,
) -> String {
    // Windows can synchronously invoke the UI subclass while UI code owns config.
    // Never wait indefinitely or guess that a busy/poisoned policy is enabled.
    let snapshot_deadline = deadline.min(Instant::now() + Duration::from_millis(250));
    match snapshot(config, snapshot_deadline, |c| c.strategy.pause_on_shutdown) {
        Some(false) => return "关机暂停选项已关闭，跳过".into(),
        None => return "无法及时读取关机策略，暂停未确认".into(),
        Some(true) => {}
    }
    let token = match snapshot(shared, snapshot_deadline, |s| s.token.clone()) {
        Some(Some(token)) if !token.is_empty() => token,
        Some(_) => return "未登录，关机暂停未确认".into(),
        None => return "无法及时读取登录状态，关机暂停未确认".into(),
    };
    // Leave time for all receiving windows to get the result. No account query
    // or interactive re-login is attempted during shutdown.
    let remaining = deadline
        .saturating_duration_since(Instant::now())
        .saturating_sub(Duration::from_millis(250))
        .min(Duration::from_secs(3));
    if remaining.is_zero() {
        return "关机处理时间已耗尽，暂停未确认".into();
    }
    match pause(&token, remaining) {
        Ok(_) => "关机暂停请求返回成功；最终计时状态请在官方小程序刷新核对".into(),
        Err(error) if api::is_token_err(&error) => {
            "登录状态失效，关机暂停未确认；下次启动请重新登录".into()
        }
        Err(_) => "关机暂停请求失败或超时，暂停未确认；下次启动仍按启动检查规则补查".into(),
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if let Some(result) = HANDLER
        .get()
        .and_then(|handler| handler.handle(msg, wparam))
    {
        return result;
    }
    windows::Win32::UI::WindowsAndMessaging::DefWindowProcW(hwnd, msg, wparam, lparam)
}

unsafe fn window_thread() {
    use windows::core::w;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DispatchMessageW, GetMessageW, RegisterClassW, TranslateMessage, MSG,
        WINDOW_EX_STYLE, WNDCLASSW, WS_OVERLAPPED,
    };
    let class = w!("LeigodGuardShutdown");
    let wc = WNDCLASSW {
        lpfnWndProc: Some(wndproc),
        lpszClassName: class,
        ..Default::default()
    };
    let _ = RegisterClassW(&wc);
    // A real top-level window: HWND_MESSAGE windows don't receive this broadcast.
    if CreateWindowExW(
        WINDOW_EX_STYLE::default(),
        class,
        w!("LeigodGuard shutdown listener"),
        WS_OVERLAPPED,
        0,
        0,
        0,
        0,
        None,
        None,
        None,
        None,
    )
    .is_err()
    {
        trace("独立关机监听窗口创建失败，将依赖主窗口监听");
        return;
    }
    trace("独立关机监听窗口已就绪");
    let mut msg = MSG::default();
    loop {
        let result = GetMessageW(&mut msg, None, 0, 0).0;
        if result <= 0 {
            if result == -1 {
                trace("独立关机监听消息循环发生错误");
            }
            break;
        }
        let _ = TranslateMessage(&msg);
        let _ = DispatchMessageW(&msg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opt_out_and_missing_login_never_send_a_pause() {
        let mut config = Config::default();
        config.strategy.pause_on_shutdown = false;
        let config = Mutex::new(config);
        let shared = Mutex::new(Shared::default());
        let forbidden =
            |_: &str, _: Duration| -> Result<String, api::ApiError> { panic!("must not send") };
        assert!(pause_before_deadline(
            &shared,
            &config,
            Instant::now() + Duration::from_secs(1),
            forbidden
        )
        .contains("已关闭"));
        config.lock().unwrap().strategy.pause_on_shutdown = true;
        assert!(pause_before_deadline(
            &shared,
            &config,
            Instant::now() + Duration::from_secs(1),
            forbidden
        )
        .contains("未登录"));
    }

    #[test]
    fn locked_ui_config_and_expired_budget_cannot_hang_or_send() {
        let config = Mutex::new(Config::default());
        let shared = Mutex::new(Shared::default());
        let held = config.lock().unwrap();
        let start = Instant::now();
        let result = pause_before_deadline(
            &shared,
            &config,
            start + Duration::from_millis(30),
            |_, _| panic!("must not send"),
        );
        assert!(start.elapsed() < Duration::from_secs(1));
        assert!(result.contains("无法及时读取关机策略"));
        drop(held);
        shared.lock().unwrap().set_token(Some("fixture".into()));
        let result = pause_before_deadline(&shared, &config, Instant::now(), |_, _| {
            panic!("must not send")
        });
        assert!(result.contains("时间已耗尽"));
    }

    #[test]
    fn pause_uses_short_budget_and_redacts_failure_details() {
        let config = Mutex::new(Config::default());
        let shared = Mutex::new(Shared::default());
        shared
            .lock()
            .unwrap()
            .set_token(Some("private-test-token".into()));
        let result = pause_before_deadline(
            &shared,
            &config,
            Instant::now() + Duration::from_secs(4),
            |token, budget| {
                assert_eq!(token, "private-test-token");
                assert!(budget <= Duration::from_secs(3));
                Err(api::ApiError("upstream echoed private-test-token".into()))
            },
        );
        assert!(result.contains("暂停未确认"));
        assert!(!result.contains("private-test-token"));
    }
}
