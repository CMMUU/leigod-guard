//! Shared runtime brand assets. Update the source and run scripts/generate-brand.py.
pub const WINDOW_SIDE: usize = 256;
pub const WINDOW_RGBA: &[u8; WINDOW_SIDE * WINDOW_SIDE * 4] =
    include_bytes!("../assets/app-icon-256.rgba");
pub const TRAY_SIDE: usize = 32;
pub const TRAY_RGBA: &[u8; TRAY_SIDE * TRAY_SIDE * 4] =
    include_bytes!("../assets/app-icon-32.rgba");

pub fn window_icon() -> (Vec<u8>, u32, u32) {
    (WINDOW_RGBA.to_vec(), WINDOW_SIDE as u32, WINDOW_SIDE as u32)
}

pub fn tray_icon() -> (Vec<u8>, u32, u32) {
    (TRAY_RGBA.to_vec(), TRAY_SIDE as u32, TRAY_SIDE as u32)
}

/// The viewport icon is ICON_SMALL on Windows; install_taskbar_icon sets ICON_BIG.
pub fn native_options(mut options: eframe::NativeOptions) -> eframe::NativeOptions {
    let (rgba, width, height) = window_icon();
    options.viewport = options.viewport.with_icon(egui::IconData {
        rgba,
        width,
        height,
    });
    options
}

/// eframe 0.31 does not expose winit's separate Windows taskbar icon option.
/// Copy the just-created, full-resolution brand window icon onto ICON_BIG.
/// The subclass owns the copy until the HWND is destroyed; egui owns the small icon.
pub fn install_taskbar_icon(cc: &eframe::CreationContext<'_>) -> Result<(), String> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::Shell::SetWindowSubclass;
    use windows::Win32::UI::WindowsAndMessaging::{
        CopyIcon, DestroyIcon, SendMessageW, HICON, ICON_BIG, ICON_SMALL, WM_GETICON, WM_SETICON,
    };
    let RawWindowHandle::Win32(handle) = cc.window_handle().map_err(|e| e.to_string())?.as_raw()
    else {
        return Err("Expected Windows window handle".into());
    };
    let hwnd = HWND(handle.hwnd.get() as *mut _);
    unsafe {
        let source = SendMessageW(hwnd, WM_GETICON, WPARAM(ICON_SMALL as usize), LPARAM(0));
        if source.0 == 0 {
            return Err("Brand window icon is unavailable".into());
        }
        let icon = CopyIcon(HICON(source.0 as *mut _)).map_err(|e| e.to_string())?;
        if !SetWindowSubclass(
            hwnd,
            Some(taskbar_icon_proc),
            TASKBAR_SUBCLASS_ID,
            icon.0 as usize,
        )
        .as_bool()
        {
            let _ = DestroyIcon(icon);
            return Err("Could not attach taskbar icon lifetime".into());
        }
        SendMessageW(
            hwnd,
            WM_SETICON,
            WPARAM(ICON_BIG as usize),
            LPARAM(icon.0 as isize),
        );
    }
    Ok(())
}

const TASKBAR_SUBCLASS_ID: usize = 0x4c49;

unsafe extern "system" fn taskbar_icon_proc(
    hwnd: windows::Win32::Foundation::HWND,
    message: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
    _id: usize,
    icon: usize,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass};
    use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, HICON, WM_NCDESTROY};
    if message == WM_NCDESTROY {
        let _ = RemoveWindowSubclass(hwnd, Some(taskbar_icon_proc), TASKBAR_SUBCLASS_ID);
        let result = DefSubclassProc(hwnd, message, wparam, lparam);
        let _ = DestroyIcon(HICON(icon as *mut _));
        result
    } else {
        DefSubclassProc(hwnd, message, wparam, lparam)
    }
}
