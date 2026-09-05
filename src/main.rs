#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(not(target_os = "windows"))]
compile_error!("LeigodGuard currently supports Windows only.");

mod autostart;
mod captcha;
mod config;
mod dpapi;
mod game_presets;
mod instance;
mod leigod_api;
mod monitor;
mod osd;
mod shared;
mod shutdown;
mod ui;
mod ui_home;
mod ui_theme;
mod update_apply;
mod updater;
mod worker;

use std::sync::{Arc, Mutex};

fn main() {
    // 子进程模式：独立进程承载人机验证窗口，结果写文件后退出
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--version" || arg == "-V") {
        println!("leigod-guard {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!(
            "LeigodGuard {} (Windows)\nUsage: leigod-guard [--minimized] [--version] [--help]",
            env!("CARGO_PKG_VERSION")
        );
        return;
    }
    // Helpers must not read account settings, start monitoring, or hold the
    // main application's mutex while waiting for its process to exit.
    if args.get(1).map(String::as_str) == Some("--apply-update") {
        if args.len() != 3 {
            ui::msgbox_warn("更新失败", "缺少有效的更新计划，请从应用内重新检查更新。");
            std::process::exit(2);
        }
        if let Err(message) = update_apply::run_helper(std::path::Path::new(&args[2])) {
            ui::msgbox_warn("更新失败", &message);
            std::process::exit(1);
        }
        return;
    }
    if args.len() >= 4 && args[1] == "--captcha" {
        let server_status: i64 = args[3].parse().unwrap_or(1);
        std::process::exit(captcha::run_subprocess(args[2].clone(), server_status));
    }

    let minimized = args.iter().any(|a| a == "--minimized");

    // Load the strategy before the instance mutex and graphics initialization.
    // Strict GamePP blocking must be present at process creation time; the small
    // bootstrap exits after starting the protected replacement.
    let loaded_config = config::Config::load();
    if loaded_config.strategy.block_gamepp_injection {
        match osd::prepare_gamepp_protection(&args) {
            osd::GameppProtectionPreparation::Relaunched => return,
            osd::GameppProtectionPreparation::Active => {}
            osd::GameppProtectionPreparation::ContinueUnprotected(message) => {
                ui::msgbox_warn("游戏加加屏蔽未生效", &message)
            }
            osd::GameppProtectionPreparation::Abort(message) => {
                ui::msgbox_warn("游戏加加屏蔽未生效", &message);
                return;
            }
        }
    }

    let _instance = match instance::InstanceGuard::acquire() {
        Ok(Some(guard)) => guard,
        Ok(None) => {
            if !minimized {
                ui::activate_existing_window();
            }
            return;
        }
        Err(_) => {
            ui::msgbox_warn("启动失败", "无法确认程序运行状态，请退出已有实例后重试。");
            std::process::exit(1);
        }
    };

    let config = Arc::new(Mutex::new(loaded_config));
    let shared = Arc::new(Mutex::new(shared::Shared::default()));

    // 后台守护线程
    {
        let shared = Arc::clone(&shared);
        let config = Arc::clone(&config);
        std::thread::spawn(move || worker::run(shared, config));
    }

    // 关机/注销监听线程（收到 WM_ENDSESSION 时自动暂停计时）
    shutdown::start(Arc::clone(&shared), Arc::clone(&config));

    // 主窗口
    let (rgba, w, h) = ui::make_icon_rgba();
    let options = eframe::NativeOptions {
        // wgpu(DX12) 渲染：规避部分机器 OpenGL 新窗口白屏问题
        renderer: eframe::Renderer::Wgpu,
        // 关闭窗口几何持久化：eframe persistence 会把调试期间的残废窗口尺寸
        // 存进 app.ron 并在之后每次启动恢复，导致窗口越开越小。
        // 关掉后按设计默认尺寸打开，仍允许缩放到紧凑布局。
        persist_window: false,
        viewport: egui::ViewportBuilder::default()
            // 默认打开 1180x780（winit 0.30 在 Windows 下 with_resizable(false)
            // 和 min=max 都会把窗口压成 70x90 的残废尺寸，只能用默认大小约束）
            .with_inner_size([1180.0, 780.0])
            .with_min_inner_size([680.0, 460.0])
            .with_visible(!minimized)
            .with_icon(egui::IconData {
                rgba,
                width: w,
                height: h,
            }),
        ..Default::default()
    };

    let shared_ui = Arc::clone(&shared);
    let config_ui = Arc::clone(&config);
    if let Err(e) = eframe::run_native(
        "雷神守护 - LeigodGuard",
        options,
        Box::new(move |cc| {
            Ok(Box::new(ui::App::new(cc, shared_ui, config_ui)) as Box<dyn eframe::App>)
        }),
    ) {
        eprintln!("GUI 启动失败: {e}");
        std::process::exit(1);
    }
}
