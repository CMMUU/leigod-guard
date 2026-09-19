//! Real eframe regression, isolated from account/config/network/tray and OS shutdown.
//! Messages go ONLY to this process's off-screen HWND, never HWND_BROADCAST.
#[path = "../src/session_end.rs"]
mod session_end;
#[path = "../src/window_visibility.rs"]
mod window_visibility;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    IsWindowVisible, SendMessageTimeoutW, ShowWindow, SMTO_ABORTIFHUNG, SW_SHOWNOACTIVATE,
    WM_ENDSESSION, WM_QUERYENDSESSION,
};

struct Probe;
impl eframe::App for Probe {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_millis(20));
    }
}

fn require(ok: bool, message: &str) {
    if !ok {
        eprintln!("FAIL: {message}");
        std::process::exit(1);
    }
}

fn main() -> eframe::Result {
    let args: Vec<_> = std::env::args().collect();
    let manual = args.iter().any(|arg| arg == "--manual");
    let opened = args.iter().any(|arg| arg == "--opened");
    let legacy = args
        .iter()
        .any(|arg| arg == "--legacy-missing-main-handler");
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(25));
        require(false, "session-end probe timed out");
    });
    eframe::run_native(
        "LeigodGuard isolated session-end probe",
        eframe::NativeOptions {
            renderer: eframe::Renderer::Wgpu,
            persist_window: false,
            viewport: egui::ViewportBuilder::default()
                .with_position(egui::pos2(-32000.0, -32000.0))
                .with_inner_size([64.0, 64.0])
                .with_decorations(false)
                .with_visible(manual)
                .with_active(false),
            ..Default::default()
        },
        Box::new(move |cc| {
            let RawWindowHandle::Win32(handle) = cc.window_handle()?.as_raw() else {
                panic!("Windows only")
            };
            let hwnd = HWND(handle.hwnd.get() as *mut _);
            let calls = Arc::new(AtomicUsize::new(0));
            let counted = Arc::clone(&calls);
            let handler = session_end::SessionEnd::new(
                move |_| {
                    std::thread::sleep(Duration::from_millis(50));
                    counted.fetch_add(1, Ordering::SeqCst);
                },
                |_| {},
            );
            if !legacy {
                session_end::install(hwnd, Arc::clone(&handler))?;
            }
            if !manual {
                window_visibility::install(cc)?;
            }
            let raw = hwnd.0 as usize;
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(500));
                let hwnd = HWND(raw as *mut _);
                unsafe {
                    require(
                        IsWindowVisible(hwnd).as_bool() == manual,
                        "unexpected initial visibility",
                    );
                    if opened {
                        require(window_visibility::allow_show(hwnd), "open request failed");
                        require(
                            !window_visibility::startup_hidden(),
                            "visibility guard was not released",
                        );
                        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                        require(IsWindowVisible(hwnd).as_bool(), "explicit open failed");
                    }
                    let send = |message, confirmed| {
                        let mut result = 0usize;
                        require(
                            SendMessageTimeoutW(
                                hwnd,
                                message,
                                WPARAM(confirmed),
                                LPARAM(0),
                                SMTO_ABORTIFHUNG,
                                5000,
                                Some(&mut result),
                            )
                            .0 != 0,
                            "message timed out",
                        );
                        result
                    };
                    require(
                        send(WM_QUERYENDSESSION, 0) == 1,
                        "query must allow shutdown",
                    );
                    send(WM_ENDSESSION, 0);
                    require(
                        calls.load(Ordering::SeqCst) == 0,
                        "canceled shutdown must not pause",
                    );
                    send(WM_QUERYENDSESSION, 0);
                    send(WM_ENDSESSION, 1);
                    require(
                        calls.load(Ordering::SeqCst) == 1,
                        "main window did not complete pause before returning",
                    );
                    handler.handle(WM_ENDSESSION, WPARAM(1));
                    send(WM_ENDSESSION, 1);
                    require(
                        calls.load(Ordering::SeqCst) == 1,
                        "duplicate notification paused again",
                    );
                    require(
                        IsWindowVisible(hwnd).as_bool() == (manual || opened),
                        "shutdown changed visibility",
                    );
                }
                println!("PASS: actual eframe session end (manual={manual}, opened={opened}); cancel, confirmed, duplicate");
                std::process::exit(0);
            });
            Ok(Box::new(Probe))
        }),
    )
}
