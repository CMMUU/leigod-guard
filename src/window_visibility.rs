//! Keep the initial tray launch hidden even when eframe shows its first frame.
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::sync::atomic::{AtomicBool, Ordering};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowLongW, SendMessageTimeoutW, SetWindowLongW, GWL_EXSTYLE, GWL_STYLE, SMTO_ABORTIFHUNG,
    STYLESTRUCT, SWP_SHOWWINDOW, WINDOWPOS, WM_APP, WM_NCDESTROY, WM_STYLECHANGING,
    WM_WINDOWPOSCHANGING, WS_EX_NOACTIVATE, WS_VISIBLE,
};

const SUBCLASS_ID: usize = 0x4c47;
const OPEN_REQUEST: u32 = WM_APP + 0x147;
static STARTUP_HIDDEN: AtomicBool = AtomicBool::new(false);

pub fn startup_hidden() -> bool {
    STARTUP_HIDDEN.load(Ordering::Acquire)
}

pub fn install(cc: &eframe::CreationContext<'_>) -> Result<(), String> {
    let handle = cc
        .window_handle()
        .map_err(|_| "Window handle unavailable")?;
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return Err("Expected a Windows window handle".into());
    };
    install_on_window(HWND(handle.hwnd.get() as *mut std::ffi::c_void))
}

fn install_on_window(hwnd: HWND) -> Result<(), String> {
    // Called on the UI thread before eframe's first paint. Do not poll for a
    // visible window and hide it later: that would flash and steal focus.
    let original_noactivate =
        unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as u32 & WS_EX_NOACTIVATE.0;
    if unsafe {
        SetWindowSubclass(
            hwnd,
            Some(silent_start_proc),
            SUBCLASS_ID,
            original_noactivate as usize,
        )
    }
    .as_bool()
    {
        unsafe {
            let style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            SetWindowLongW(hwnd, GWL_EXSTYLE, (style | WS_EX_NOACTIVATE.0) as i32);
        }
        STARTUP_HIDDEN.store(true, Ordering::Release);
        Ok(())
    } else {
        Err("Could not install the silent-start window handler".into())
    }
}

/// The tray thread and a second process both release the handler on its owning
/// UI thread. Ordinary launches have no handler and accept the same message.
pub fn allow_show(hwnd: HWND) -> bool {
    unsafe {
        SendMessageTimeoutW(
            hwnd,
            OPEN_REQUEST,
            WPARAM(0),
            LPARAM(0),
            SMTO_ABORTIFHUNG,
            1500,
            None,
        )
        .0 != 0
    }
}

unsafe extern "system" fn silent_start_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    match message {
        WM_STYLECHANGING if lparam.0 != 0 => {
            // winit writes WS_VISIBLE after ShowWindow, then refreshes the
            // frame. A WINDOWPOS-only guard is bypassed by that style write.
            // Apply the invariant after downstream handlers as well.
            let result = DefSubclassProc(hwnd, message, wparam, lparam);
            let style = &mut *(lparam.0 as *mut STYLESTRUCT);
            if wparam.0 as i32 == GWL_STYLE.0 {
                style.styleNew &= !WS_VISIBLE.0;
            } else if wparam.0 as i32 == GWL_EXSTYLE.0 {
                style.styleNew |= WS_EX_NOACTIVATE.0;
            }
            return result;
        }
        WM_WINDOWPOSCHANGING if lparam.0 != 0 => {
            // Windows explicitly permits changing WINDOWPOS flags here.
            // https://learn.microsoft.com/windows/win32/winmsg/wm-windowposchanging
            let position = &mut *(lparam.0 as *mut WINDOWPOS);
            position.flags &= !SWP_SHOWWINDOW;
        }
        OPEN_REQUEST => {
            if RemoveWindowSubclass(hwnd, Some(silent_start_proc), SUBCLASS_ID).as_bool() {
                let style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
                SetWindowLongW(
                    hwnd,
                    GWL_EXSTYLE,
                    ((style & !WS_EX_NOACTIVATE.0) | data as u32) as i32,
                );
                STARTUP_HIDDEN.store(false, Ordering::Release);
            }
            return LRESULT(1);
        }
        WM_NCDESTROY => {
            let _ = RemoveWindowSubclass(hwnd, Some(silent_start_proc), SUBCLASS_ID);
            STARTUP_HIDDEN.store(false, Ordering::Release);
        }
        _ => {}
    }
    DefSubclassProc(hwnd, message, wparam, lparam)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::core::w;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, DispatchMessageW, GetWindowLongW, IsWindowVisible,
        PeekMessageW, SetWindowLongW, SetWindowPos, ShowWindow, GWL_STYLE, MSG, PM_REMOVE,
        SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SW_HIDE, SW_SHOW, SW_SHOWNOACTIVATE,
        WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP, WS_VISIBLE,
    };

    struct TestWindow(HWND);
    impl Drop for TestWindow {
        fn drop(&mut self) {
            unsafe {
                let _ = DestroyWindow(self.0);
            }
        }
    }

    #[test]
    fn framework_show_stays_hidden_until_explicit_open_then_can_hide_and_reopen() {
        // A one-pixel, off-screen, non-activating test window. No real app,
        // account, configuration, tray, or monitor is started.
        let window = TestWindow(unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                w!("STATIC"),
                w!("LeigodGuard silent-start test"),
                WS_POPUP,
                -32000,
                -32000,
                1,
                1,
                None,
                None,
                None,
                None,
            )
            .unwrap()
        });
        let hwnd = window.0;
        install_on_window(hwnd).unwrap();
        assert!(startup_hidden());
        unsafe {
            assert_ne!(
                GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_NOACTIVATE.0,
                0
            );
            for _ in 0..3 {
                let _ = ShowWindow(hwnd, SW_SHOW);
                assert!(
                    !IsWindowVisible(hwnd).as_bool(),
                    "first-frame show must be suppressed"
                );
                SetWindowPos(
                    hwnd,
                    None,
                    0,
                    0,
                    0,
                    0,
                    SWP_SHOWWINDOW | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                )
                .unwrap();
                assert!(!IsWindowVisible(hwnd).as_bool());
                // winit 0.30 applies the entire style after ShowWindow. Its
                // WS_VISIBLE write bypasses a WINDOWPOS-only startup guard.
                let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
                SetWindowLongW(hwnd, GWL_STYLE, (style | WS_VISIBLE.0) as i32);
                assert!(
                    !IsWindowVisible(hwnd).as_bool(),
                    "winit's style update must not bypass silent startup"
                );
                SetWindowLongW(hwnd, GWL_EXSTYLE, WS_EX_TOOLWINDOW.0 as i32);
                assert_ne!(
                    GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_NOACTIVATE.0,
                    0
                );
            }
            // Tray clicks call from another thread; subclass removal must run
            // on the window's owning thread, with a bounded wait if it is hung.
            let handle = hwnd.0 as usize;
            let open = std::thread::spawn(move || allow_show(HWND(handle as *mut _)));
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            while !open.is_finished() && std::time::Instant::now() < deadline {
                let mut message = MSG::default();
                while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                    DispatchMessageW(&message);
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            assert!(open.join().unwrap());
            assert!(!startup_hidden());
            assert_eq!(
                GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_NOACTIVATE.0,
                0
            );
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            assert!(IsWindowVisible(hwnd).as_bool());
            let _ = ShowWindow(hwnd, SW_HIDE);
            assert!(!IsWindowVisible(hwnd).as_bool());
            assert!(allow_show(hwnd));
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            assert!(
                IsWindowVisible(hwnd).as_bool(),
                "later user opens must still work"
            );
        }
    }
}
