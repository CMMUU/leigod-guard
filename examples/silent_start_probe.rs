//! Isolated real-eframe regression: no account, config, tray, guard or network.
//! Run once normally and once with --manual. The tiny window stays off-screen.
#[path = "../src/window_visibility.rs"]
mod window_visibility;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, IsWindowVisible, ShowWindow, SW_HIDE, SW_SHOWNOACTIVATE,
};

struct Probe {
    frames: Arc<AtomicUsize>,
}

impl eframe::App for Probe {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.frames.fetch_add(1, Ordering::Release);
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.label("Startup probe");
        });
        ctx.request_repaint_after(Duration::from_millis(50));
    }
}

fn require(ok: bool, message: &str) {
    if !ok {
        eprintln!("FAIL: {message}");
        std::process::exit(1);
    }
}

fn main() -> eframe::Result {
    let manual = std::env::args().any(|arg| arg == "--manual");
    let frames = Arc::new(AtomicUsize::new(0));
    // A stuck or failed renderer must fail CI, never hang indefinitely.
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(30));
        require(false, "native startup probe timed out");
    });
    eframe::run_native(
        "LeigodGuard isolated startup probe",
        eframe::NativeOptions {
            renderer: eframe::Renderer::Wgpu,
            persist_window: false,
            viewport: egui::ViewportBuilder::default()
                .with_position(egui::pos2(-32000.0, -32000.0))
                .with_inner_size([64.0, 64.0])
                .with_decorations(false)
                .with_active(false)
                .with_visible(manual),
            ..Default::default()
        },
        Box::new(move |cc| {
            if !manual {
                window_visibility::install(cc)?;
            }
            let RawWindowHandle::Win32(handle) = cc.window_handle()?.as_raw() else {
                panic!("Expected Windows");
            };
            let raw = handle.hwnd.get() as usize;
            let observed_frames = Arc::clone(&frames);
            let ctx = cc.egui_ctx.clone();
            std::thread::spawn(move || {
                let hwnd = HWND(raw as *mut _);
                let start = Instant::now();
                while start.elapsed() < Duration::from_secs(2) {
                    unsafe {
                        require(
                            GetForegroundWindow() != hwnd,
                            "probe stole foreground focus",
                        );
                        if !manual {
                            require(
                                !IsWindowVisible(hwnd).as_bool(),
                                "first paint made startup window visible",
                            );
                        }
                    }
                    ctx.request_repaint();
                    std::thread::sleep(Duration::from_millis(10));
                }
                require(
                    observed_frames.load(Ordering::Acquire) >= 3,
                    "actual eframe repaint loop did not run",
                );
                unsafe {
                    if manual {
                        require(
                            IsWindowVisible(hwnd).as_bool(),
                            "manual launch failed to show",
                        );
                    } else {
                        require(
                            window_visibility::startup_hidden(),
                            "startup guard released without user action",
                        );
                        require(
                            window_visibility::allow_show(hwnd),
                            "cross-thread open timed out",
                        );
                        require(
                            !window_visibility::startup_hidden(),
                            "explicit open did not release guard",
                        );
                        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                        require(IsWindowVisible(hwnd).as_bool(), "explicit open failed");
                    }
                    let _ = ShowWindow(hwnd, SW_HIDE);
                    require(!IsWindowVisible(hwnd).as_bool(), "hide failed");
                    require(window_visibility::allow_show(hwnd), "second open timed out");
                    let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                    require(IsWindowVisible(hwnd).as_bool(), "reopen failed");
                }
                println!("PASS: actual eframe startup (manual={manual}), hide and reopen");
                // This isolated probe owns no persistent state. End it here;
                // an off-screen/hidden window need not receive another paint
                // to process an egui Close command. Windows reclaims its HWND.
                std::process::exit(0);
            });
            Ok(Box::new(Probe { frames }))
        }),
    )
}
