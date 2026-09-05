//! egui 配置面板 + 系统托盘。
#[cfg(test)]
#[path = "ui_render_tests.rs"]
mod presentation_tests;
use crate::autostart;
use crate::config::{AccelPlan, Config, GameEntry};
use crate::dpapi;
use crate::game_presets::{self, PRESETS};
use crate::leigod_api as api;
use crate::osd;
use crate::shared::{ManualCmd, Shared};
use crate::ui_home::{HomeAction, HomeState};
use crate::ui_theme::{self as theme, Icon};
use crate::updater::{DownloadProgress, DownloadedUpdate, PackageKind, ReleaseInfo, UpdateSource};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Games,
    Plans,
    Account,
    Strategy,
    Logs,
    Updates,
}

enum UpdateEvent {
    Checked(UpdateSource, Result<Option<ReleaseInfo>, String>),
    Progress(DownloadProgress),
    Downloaded(Result<DownloadedUpdate, String>),
    PreparationFailed(String),
}

/// 已保存密码的占位显示（密码框里展示，不代表真实密码）
const PWD_PLACEHOLDER: &str = "••••••••";

pub struct App {
    shared: Arc<Mutex<Shared>>,
    config: Arc<Mutex<Config>>,
    /// 托盘图标本体（Rc 类型，必须留在 UI 线程；事件处理在独立托盘线程）
    _tray: Option<TrayIcon>,

    page: Page,
    brand: egui::TextureHandle,
    show_add_game: bool,
    dirty: bool,
    status_msg: String,

    update_events: Option<Receiver<UpdateEvent>>,
    update_release: Option<ReleaseInfo>,
    update_kind: Result<PackageKind, String>,
    update_busy: bool,
    update_preparing: Arc<AtomicBool>,
    update_progress: Option<DownloadProgress>,
    update_message: String,
    update_error: bool,

    // 添加游戏表单
    new_name: String,
    new_exe: String,
    new_preset: Option<usize>,
    new_plan: String,
    show_proc_picker: bool,
    proc_filter: String,
    proc_list: Vec<String>,

    // 账户表单
    acc_user: String,
    acc_pwd: String,
    login_mode: u8, // 0=密码 1=短信验证码 2=手动token
    remember_pwd: bool,
    sms_phone: String,
    sms_code: String,
    sms_key: String,
    sms_sent_at: Option<std::time::Instant>,
    token_input: String,

    // 人机验证等待状态：0=无 1=等验证后重试发短信 2=等验证后重试密码登录
    pending_captcha: u8,
    pending_user: String,
    pending_md5: String,

    // OSD 排除配置提权后的状态轮询截止时间
    osd_poll_until: Option<std::time::Instant>,

    // 新方案表单
    plan_name: String,
    plan_game: String,
    plan_region: String,
    plan_node: String,
    plan_mode: String,
    plan_note: String,
}

pub fn make_icon_rgba() -> (Vec<u8>, u32, u32) {
    // 与 EXE/安装器的 ICO 共用设计；内嵌 RGBA，无需运行时解码或读取文件。
    const PIXELS: &[u8; 256 * 256 * 4] = include_bytes!("../assets/app-icon-256.rgba");
    (PIXELS.to_vec(), 256, 256)
}

fn make_tray_icon_rgba() -> (Vec<u8>, u32, u32) {
    const PIXELS: &[u8; 32 * 32 * 4] = include_bytes!("../assets/app-icon-32.rgba");
    (PIXELS.to_vec(), 32, 32)
}

fn load_cjk_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    for path in [
        r"C:\Windows\Fonts\msyh.ttc",
        r"C:\Windows\Fonts\msyh.ttf",
        r"C:\Windows\Fonts\simhei.ttf",
        r"C:\Windows\Fonts\simsun.ttc",
    ] {
        if let Ok(bytes) = std::fs::read(path) {
            fonts.font_data.insert(
                "cjk".into(),
                std::sync::Arc::new(egui::FontData::from_owned(bytes)),
            );
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "cjk".into());
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .push("cjk".into());
            break;
        }
    }
    // Prefer Segoe UI for Latin text and keep system CJK fonts as fallbacks.
    if let Ok(bytes) = std::fs::read(r"C:\Windows\Fonts\segoeui.ttf") {
        fonts
            .font_data
            .insert("latin".into(), Arc::new(egui::FontData::from_owned(bytes)));
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "latin".into());
    }
    let mut headings = fonts.families[&egui::FontFamily::Proportional].clone();
    for (key, path) in [
        ("cjk-bold", r"C:\Windows\Fonts\msyhbd.ttc"),
        ("latin-bold", r"C:\Windows\Fonts\segoeuib.ttf"),
    ] {
        if let Ok(bytes) = std::fs::read(path) {
            fonts
                .font_data
                .insert(key.into(), Arc::new(egui::FontData::from_owned(bytes)));
            headings.insert(0, key.into());
        }
    }
    fonts
        .families
        .insert(egui::FontFamily::Name("heading".into()), headings);
    ctx.set_fonts(fonts);
    theme::install(ctx);
}

/// 追加写运行日志，单文件上限约 1 MiB，保留一份历史日志。
pub fn dbglog(msg: &str) {
    static LOG_LOCK: Mutex<()> = Mutex::new(());
    let Ok(_guard) = LOG_LOCK.lock() else { return };
    let path = dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("leigod-guard")
        .join("debug.log");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if std::fs::metadata(&path).is_ok_and(|m| m.len() >= 1024 * 1024) {
        let previous = path.with_extension("log.1");
        let _ = std::fs::remove_file(&previous);
        let _ = std::fs::rename(&path, previous);
    }
    let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        use std::io::Write;
        let _ = writeln!(f, "[{ts}] {msg}");
    }
}

/// Windows 原生警告弹窗（用于暂停失败等必须引起注意的场景）
pub fn msgbox_warn(title: &str, text: &str) {
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONWARNING, MB_OK, MB_TOPMOST};
    let t: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let c: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(t.as_ptr()),
            PCWSTR(c.as_ptr()),
            MB_OK | MB_ICONWARNING | MB_TOPMOST,
        );
    }
}

/// 是/否 确认弹窗，返回 true=是
fn msgbox_yesno(title: &str, text: &str, warning: bool) -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, IDYES, MB_ICONQUESTION, MB_ICONWARNING, MB_TOPMOST, MB_YESNO,
    };
    let t: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let c: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    let icon = if warning {
        MB_ICONWARNING
    } else {
        MB_ICONQUESTION
    };
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(t.as_ptr()),
            PCWSTR(c.as_ptr()),
            MB_YESNO | icon | MB_TOPMOST,
        ) == IDYES
    }
}

/// 退出选择弹窗：加速中给出三选（暂停并退出/直接退出/取消），否则简单确认
enum ExitChoice {
    PauseThenExit,
    ExitNow,
    Cancel,
}

fn msgbox_exit(accelerating: bool) -> ExitChoice {
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, IDNO, IDYES, MB_ICONWARNING, MB_TOPMOST, MB_YESNOCANCEL,
    };
    if accelerating {
        let t: Vec<u16> = "当前已登录，计时可能仍在继续。\n\n【是】先暂停计时再退出（推荐）\n【否】直接退出（计时继续）\n【取消】不退出"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let c: Vec<u16> = "退出雷神守护"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let r = unsafe {
            MessageBoxW(
                None,
                PCWSTR(t.as_ptr()),
                PCWSTR(c.as_ptr()),
                MB_YESNOCANCEL | MB_ICONWARNING | MB_TOPMOST,
            )
        };
        match r {
            IDYES => ExitChoice::PauseThenExit,
            IDNO => ExitChoice::ExitNow,
            _ => ExitChoice::Cancel,
        }
    } else if msgbox_yesno(
        "退出雷神守护",
        "确定要退出吗？退出后将不再自动暂停计时。",
        false,
    ) {
        ExitChoice::ExitNow
    } else {
        ExitChoice::Cancel
    }
}

/// 托盘菜单项 ID 集合（MenuId 内部是 String，可跨线程）
struct TrayIds {
    open: tray_icon::menu::MenuId,
    defer_startup: tray_icon::menu::MenuId,
    pause: tray_icon::menu::MenuId,
    quit: tray_icon::menu::MenuId,
}

/// Both entry points only queue a request. The worker owns timing and account actions.
fn request_startup_defer(shared: &mut Shared, requested_at: Instant) -> bool {
    if !shared.startup_pause_status.pending {
        return false;
    }
    shared.startup_defer_requested_at = Some(requested_at);
    shared.log("准备游戏：请求将启动检查延后至至少10分钟后");
    true
}

/// 查找主窗口句柄（按窗口标题）
fn find_main_hwnd() -> Option<windows::Win32::Foundation::HWND> {
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::FindWindowW;
    let title: Vec<u16> = "雷神守护 - LeigodGuard\0".encode_utf16().collect();
    let h = unsafe { FindWindowW(PCWSTR::null(), PCWSTR(title.as_ptr())) };
    match h {
        Ok(h) if !h.is_invalid() => Some(h),
        _ => None,
    }
}

/// A second shortcut launch brings the running instance to the foreground.
pub fn activate_existing_window() {
    use windows::Win32::UI::WindowsAndMessaging::{SetForegroundWindow, ShowWindow, SW_RESTORE};
    if let Some(hwnd) = find_main_hwnd() {
        unsafe {
            let _ = ShowWindow(hwnd, SW_RESTORE);
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

/// 原生隐藏主窗口。winit 0.30 在 Windows 下 set_visible(false) 不会真正隐藏窗口
/// （apply_diff 只处理"变可见"，漏了 SW_HIDE 分支），导致 egui 以为窗口已隐藏而
/// 停止重绘、窗口却还停在屏幕上"假死"。所以隐藏必须走原生 ShowWindow(SW_HIDE)。
fn hide_window_native() {
    use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
    if let Some(h) = find_main_hwnd() {
        dbglog(&format!("hide_window_native hwnd={:?}", h.0));
        unsafe {
            let _ = ShowWindow(h, SW_HIDE);
        }
    }
}

/// 主窗口当前是否可见（调试用）
fn is_main_visible() -> Option<bool> {
    use windows::Win32::UI::WindowsAndMessaging::IsWindowVisible;
    find_main_hwnd().map(|h| unsafe { IsWindowVisible(h).as_bool() })
}

/// 显示并聚焦主窗口：优先用原生 Win32（egui 的 Visible 命令在事件循环休眠时不生效），
/// 同时发 egui 视口命令 + 重绘请求做双保险。
fn show_window(ctx: &egui::Context, hwnd: &mut Option<windows::Win32::Foundation::HWND>) {
    use windows::Win32::System::Threading::AttachThreadInput;
    use windows::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId, SetForegroundWindow,
        ShowWindow, SW_RESTORE,
    };
    if hwnd.is_none() {
        *hwnd = find_main_hwnd();
    }
    dbglog(&format!("show_window called, hwnd={:?}", hwnd.map(|h| h.0)));
    if let Some(h) = *hwnd {
        unsafe {
            let _ = ShowWindow(h, SW_RESTORE); // 隐藏/最小化状态一并恢复
                                               // 抢前台：附加到当前前台线程的输入队列
            let fg = GetForegroundWindow();
            let fg_tid = GetWindowThreadProcessId(fg, None);
            let cur_tid = windows::Win32::System::Threading::GetCurrentThreadId();
            let _ = AttachThreadInput(cur_tid, fg_tid, true);
            let _ = BringWindowToTop(h);
            let _ = SetForegroundWindow(h);
            let _ = AttachThreadInput(cur_tid, fg_tid, false);
        }
    }
    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    ctx.request_repaint();
}

/// 托盘事件轮询线程：窗口隐藏时 egui 可能暂停重绘，
/// 因此托盘点击/菜单/告警必须脱离 update() 独立处理。
fn tray_event_loop(
    ctx: egui::Context,
    shared: Arc<Mutex<Shared>>,
    config: Arc<Mutex<Config>>,
    update_preparing: Arc<AtomicBool>,
    ids: TrayIds,
) {
    dbglog("tray event loop started");
    let mut hwnd = find_main_hwnd();
    loop {
        // 告警弹窗（暂停/恢复失败等）
        let alert = shared.lock().ok().and_then(|mut s| s.alert.take());
        if let Some(msg) = alert {
            show_window(&ctx, &mut hwnd);
            msgbox_warn("雷神守护 - 警告", &msg);
        }

        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            // 悬停移动事件太频繁，不记日志
            if !matches!(
                event,
                TrayIconEvent::Move { .. }
                    | TrayIconEvent::Enter { .. }
                    | TrayIconEvent::Leave { .. }
            ) {
                dbglog(&format!("tray event: {event:?}"));
            }
            match event {
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Down,
                    ..
                } => show_window(&ctx, &mut hwnd),
                TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                } => show_window(&ctx, &mut hwnd),
                _ => {}
            }
        }
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            dbglog(&format!("menu event: {:?}", event.id));
            let id = event.id;
            if update_preparing.load(Ordering::Acquire) && id == ids.defer_startup {
                continue;
            }
            if update_preparing.load(Ordering::Acquire) && (id == ids.pause || id == ids.quit) {
                show_window(&ctx, &mut hwnd);
                continue;
            }
            if id == ids.open {
                show_window(&ctx, &mut hwnd);
            } else if id == ids.defer_startup {
                if let Ok(mut s) = shared.lock() {
                    request_startup_defer(&mut s, Instant::now());
                }
                // Keep the current game in the foreground; no window or popup.
                ctx.request_repaint();
            } else if id == ids.pause {
                if let Ok(mut s) = shared.lock() {
                    s.manual_cmd = Some(ManualCmd::Pause);
                    s.log("托盘指令：立即暂停");
                }
            } else if id == ids.quit {
                dbglog("tray quit -> direct exit");
                do_exit(&config);
            }
        }
        std::thread::sleep(Duration::from_millis(120));
    }
}

/// 账户页「退出程序」：确认对话框；若正在加速，可先自动暂停再退出
fn try_exit(
    shared: &Arc<Mutex<Shared>>,
    config: &Arc<Mutex<Config>>,
    update_preparing: &AtomicBool,
) {
    if update_preparing.load(Ordering::Acquire) {
        return;
    }
    dbglog("try_exit enter");
    let accelerating = shared
        .lock()
        .map(|s| s.token.is_some() || s.status.contains("加速中"))
        .unwrap_or(false);
    let choice = msgbox_exit(accelerating);
    // An exit confirmation may remain open while an update is pending.
    if update_preparing.load(Ordering::Acquire) {
        return;
    }
    match choice {
        ExitChoice::Cancel => dbglog("exit cancelled"),
        ExitChoice::ExitNow => do_exit(config),
        ExitChoice::PauseThenExit => {
            if let Ok(mut s) = shared.lock() {
                s.status = "正在暂停计时…".into();
                s.manual_pause_result = None;
                s.manual_cmd = Some(ManualCmd::Pause);
                s.log("退出前自动暂停计时…");
            }
            // 等待工作线程完成暂停（轮询状态），最多 12 秒
            let deadline = std::time::Instant::now() + Duration::from_secs(12);
            let mut paused = false;
            while std::time::Instant::now() < deadline {
                if update_preparing.load(Ordering::Acquire) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(300));
                let result = shared.lock().ok().and_then(|s| s.manual_pause_result);
                if let Some(success) = result {
                    paused = success;
                    break;
                }
            }
            if update_preparing.load(Ordering::Acquire) {
                return;
            }
            if paused {
                do_exit(config);
            } else if msgbox_yesno(
                "暂停未确认",
                "暂停计时未能确认成功（可能已失败，详见日志）。\n仍要退出吗？退出后工具不会再尝试暂停，时长可能继续消耗。",
                true,
            ) && !update_preparing.load(Ordering::Acquire)
            {
                do_exit(config);
            }
        }
    }
}

/// 真正退出：保存配置、结束进程（进程结束后托盘图标由系统回收）
fn do_exit(config: &Arc<Mutex<Config>>) -> ! {
    if let Ok(c) = config.lock() {
        let _ = c.save();
    }
    dbglog("process exit");
    std::process::exit(0);
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        shared: Arc<Mutex<Shared>>,
        config: Arc<Mutex<Config>>,
    ) -> Self {
        load_cjk_fonts(&cc.egui_ctx);
        let update_preparing = Arc::new(AtomicBool::new(false));

        // 托盘
        let menu = Menu::new();
        let menu_open = MenuItem::new("打开面板", true, None);
        let menu_defer_startup = MenuItem::new("准备游戏：延后启动检查10分钟", true, None);
        let menu_pause = MenuItem::new("立即暂停计时", true, None);
        // 延后入口只保护待处理的启动检查，不会开启或恢复加速。
        let menu_quit = MenuItem::new("退出", true, None);
        let _ = menu.append_items(&[
            &menu_open,
            &PredefinedMenuItem::separator(),
            &menu_defer_startup,
            &menu_pause,
            &PredefinedMenuItem::separator(),
            &menu_quit,
        ]);
        let (rgba, w, h) = make_tray_icon_rgba();
        let tray = tray_icon::Icon::from_rgba(rgba, w, h)
            .ok()
            .and_then(|icon| {
                TrayIconBuilder::new()
                    .with_menu(Box::new(menu))
                    .with_tooltip("雷神守护 - 自动暂停")
                    .with_icon(icon)
                    .build()
                    .ok()
            });

        // 托盘事件独立线程轮询：窗口隐藏时 egui 可能暂停重绘，
        // 若事件放在 update() 里处理会全部堆积无响应。
        {
            let ids = TrayIds {
                open: menu_open.id().clone(),
                defer_startup: menu_defer_startup.id().clone(),
                pause: menu_pause.id().clone(),
                quit: menu_quit.id().clone(),
            };
            let ctx = cc.egui_ctx.clone();
            let shared = Arc::clone(&shared);
            let config = Arc::clone(&config);
            let update_preparing = Arc::clone(&update_preparing);
            std::thread::spawn(move || tray_event_loop(ctx, shared, config, update_preparing, ids));
        }

        let mut app = Self::from_state(
            &cc.egui_ctx,
            shared,
            config,
            tray,
            crate::update_apply::detect_package_kind(),
            update_preparing,
        );
        let check_on_startup = app
            .config
            .lock()
            .map(|c| c.updates.check_on_startup)
            .unwrap_or(false);
        if check_on_startup {
            app.start_update_check(cc.egui_ctx.clone());
        }
        app
    }

    /// Shared initializer for the live app and isolated renderer fixtures.
    /// It uses only the supplied state; no disk configuration, tray, worker or API.
    fn from_state(
        ctx: &egui::Context,
        shared: Arc<Mutex<Shared>>,
        config: Arc<Mutex<Config>>,
        tray: Option<TrayIcon>,
        update_kind: Result<PackageKind, String>,
        update_preparing: Arc<AtomicBool>,
    ) -> Self {
        let (acc_user, has_saved_pwd) = config
            .lock()
            .map(|c| (c.account.username.clone(), !c.account.cred_enc.is_empty()))
            .unwrap_or_default();

        Self {
            shared,
            config,
            _tray: tray,
            page: Page::Games,
            brand: ctx.load_texture(
                "leigod-brand",
                egui::ColorImage::from_rgba_unmultiplied(
                    [256, 256],
                    include_bytes!("../assets/app-icon-256.rgba"),
                ),
                egui::TextureOptions::LINEAR,
            ),
            show_add_game: false,
            dirty: false,
            status_msg: String::new(),
            update_events: None,
            update_release: None,
            update_kind,
            update_busy: false,
            update_preparing,
            update_progress: None,
            update_message: "尚未检查更新".into(),
            update_error: false,
            new_name: String::new(),
            new_exe: String::new(),
            new_preset: None,
            new_plan: String::new(),
            show_proc_picker: false,
            proc_filter: String::new(),
            proc_list: Vec::new(),
            acc_user,
            acc_pwd: if has_saved_pwd {
                PWD_PLACEHOLDER.to_string()
            } else {
                String::new()
            },
            login_mode: 0,
            remember_pwd: has_saved_pwd,
            sms_phone: String::new(),
            sms_code: String::new(),
            sms_key: String::new(),
            sms_sent_at: None,
            token_input: String::new(),
            pending_captcha: 0,
            pending_user: String::new(),
            pending_md5: String::new(),
            osd_poll_until: None,
            plan_name: String::new(),
            plan_game: String::new(),
            plan_region: String::new(),
            plan_node: String::new(),
            plan_mode: String::new(),
            plan_note: String::new(),
        }
    }

    fn start_update_check(&mut self, ctx: egui::Context) {
        if self.update_busy {
            return;
        }
        let source = match self.config.lock() {
            Ok(config) => config.updates.source,
            Err(_) => {
                self.update_message = "无法读取更新来源，请稍后重试。".into();
                self.update_error = true;
                return;
            }
        };
        let (sender, receiver) = mpsc::channel();
        self.update_events = Some(receiver);
        self.update_busy = true;
        self.update_progress = None;
        self.update_release = None;
        self.update_message = format!("正在通过 {} 检查新版本…", source.label());
        self.update_error = false;
        std::thread::spawn(move || {
            let result = crate::updater::check_latest(env!("CARGO_PKG_VERSION"), source);
            let _ = sender.send(UpdateEvent::Checked(source, result));
            ctx.request_repaint();
        });
    }

    fn start_update_download(&mut self, ctx: egui::Context) {
        if self.update_busy {
            return;
        }
        let kind = match &self.update_kind {
            Ok(kind) => *kind,
            Err(message) => {
                self.update_message =
                    format!("无法确认当前安装方式：{message}。请使用手动下载入口。");
                self.update_error = true;
                return;
            }
        };
        let Some(release) = self.update_release.clone() else {
            return;
        };
        if self.pending_captcha != 0 {
            self.update_message = "请先完成或关闭当前验证码窗口，再开始更新。".into();
            self.update_error = true;
            return;
        }
        let (sender, receiver) = mpsc::channel();
        self.update_events = Some(receiver);
        self.update_busy = true;
        self.update_progress = None;
        self.update_message = format!("正在下载 v{}，下载完成后将校验并更新…", release.version);
        self.update_error = false;
        std::thread::spawn(move || {
            let result = crate::update_apply::create_staging().and_then(|staging| {
                crate::updater::download_update(&release, kind, &staging, &|progress| {
                    let _ = sender.send(UpdateEvent::Progress(progress));
                    ctx.request_repaint();
                })
            });
            let succeeded = result.is_ok();
            if sender.send(UpdateEvent::Downloaded(result)).is_ok() && succeeded {
                // Hidden windows can stop egui polling. The user requested an
                // update, so show its progress and let the UI perform handoff.
                activate_existing_window();
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            }
            ctx.request_repaint();
        });
    }

    fn poll_update_events(&mut self, ctx: &egui::Context) {
        let events: Vec<_> = self
            .update_events
            .as_ref()
            .map(|receiver| receiver.try_iter().collect())
            .unwrap_or_default();
        for event in events {
            match event {
                UpdateEvent::Checked(source, result) => {
                    self.update_events = None;
                    self.update_busy = false;
                    match result {
                        Ok(Some(release)) => {
                            self.update_message =
                                format!("{} 发现新版本 v{}", source.label(), release.version);
                            self.update_release = Some(release);
                            self.update_error = false;
                        }
                        Ok(None) => {
                            self.update_message = format!("{} 暂无更高的正式版本。各来源同步可能有延迟，可切换来源后重新检查。", source.label());
                            self.update_release = None;
                            self.update_error = false;
                        }
                        Err(message) => {
                            self.update_message = message;
                            self.update_error = true;
                        }
                    }
                }
                UpdateEvent::Progress(progress) => self.update_progress = Some(progress),
                UpdateEvent::Downloaded(result) => {
                    self.update_events = None;
                    self.update_busy = false;
                    self.update_progress = None;
                    match result {
                        Ok(downloaded) => {
                            if self.pending_captcha != 0 {
                                self.update_message =
                                    "请先完成或关闭当前验证码窗口，再尝试更新。".into();
                                self.update_error = true;
                                continue;
                            }
                            self.update_busy = true;
                            self.update_preparing.store(true, Ordering::Release);
                            self.update_message = "文件已校验，正在保存配置并准备更新…".into();
                            let (sender, receiver) = mpsc::channel();
                            self.update_events = Some(receiver);
                            let ctx = ctx.clone();
                            let config = Arc::clone(&self.config);
                            let preparing = Arc::clone(&self.update_preparing);
                            std::thread::spawn(move || {
                                let prepare = || -> Result<(), String> {
                                    // Keep the successfully saved state locked through handoff.
                                    // This also prevents process exit interrupting a worker save.
                                    // The preparing UI below never acquires this lock.
                                    let saved_config = config
                                        .lock()
                                        .map_err(|_| "无法读取待保存的配置".to_string())?;
                                    saved_config.save()?;
                                    crate::update_apply::prepare_and_launch_helper(&downloaded)?;
                                    dbglog("Starting verified application update");
                                    // Do not use do_exit: it would write the configuration a
                                    // second time, ignore a failure, and drop the guard first.
                                    std::process::exit(0)
                                };
                                if let Err(message) = prepare() {
                                    // The closure has already released the configuration lock.
                                    preparing.store(false, Ordering::Release);
                                    let _ = sender.send(UpdateEvent::PreparationFailed(message));
                                    ctx.request_repaint();
                                }
                            });
                        }
                        Err(message) => {
                            self.update_message = message;
                            self.update_error = true;
                        }
                    }
                }
                UpdateEvent::PreparationFailed(message) => {
                    self.update_events = None;
                    self.update_busy = false;
                    self.update_preparing.store(false, Ordering::Release);
                    self.update_message =
                        format!("未能开始更新，程序仍在运行：{message}。可重试或手动下载。");
                    self.update_error = true;
                }
            }
        }
    }

    /// 触发风控时：取极验 server_status 并以独立子进程弹出 v4 验证窗口，结果回来后自动重试。
    /// kind: 1=重试发短信 2=重试密码登录（两者用不同的极验 captchaId，与官网一致）
    fn start_captcha(&mut self, kind: u8, user: &str, md5: &str) {
        if self.update_preparing.load(Ordering::Acquire) {
            self.status_msg = "正在准备更新，请等待更新完成后再登录。".into();
            return;
        }
        let captcha_id = if kind == 2 {
            api::GEETEST_V4_CAPTCHA_ID_PWD
        } else {
            api::GEETEST_V4_CAPTCHA_ID
        };
        match api::geetest_server_status() {
            Ok(ss) => match crate::captcha::spawn_subprocess(captcha_id, ss) {
                Ok(()) => {
                    self.pending_captcha = kind;
                    self.pending_user = user.to_string();
                    self.pending_md5 = md5.to_string();
                    self.status_msg = "触发风控：请在弹出窗口完成人机验证，通过后将自动重试".into();
                    if let Ok(mut s) = self.shared.lock() {
                        s.log("触发风控，已弹出人机验证窗口");
                    }
                }
                Err(e) => {
                    self.status_msg =
                        format!("启动人机验证窗口失败: {e}，请改用“验证码登录”或“手动 Token”");
                }
            },
            Err(e) => {
                self.status_msg =
                    format!("获取人机验证配置失败: {e}，请改用“验证码登录”或“手动 Token”");
            }
        }
    }

    /// 轮询人机验证结果（子进程写入的结果文件），拿到后自动重试原操作
    fn poll_captcha(&mut self) {
        let Some(json) = crate::captcha::take_result_file() else {
            return;
        };
        let kind = std::mem::take(&mut self.pending_captcha);
        if kind == 0 {
            return; // 残留结果，忽略
        }
        if json.is_empty() {
            self.status_msg = "已取消人机验证，操作未执行".into();
            return;
        }
        let proof = match api::CaptchaProof::parse(&json) {
            Ok(p) => p,
            Err(e) => {
                self.status_msg = e.to_string();
                return;
            }
        };
        match kind {
            // 重试发送短信验证码
            1 => {
                let phone = self.pending_user.clone();
                match api::send_sms_code(&phone, Some(&proof)) {
                    Ok(key) => {
                        self.sms_key = key;
                        self.sms_sent_at = Some(std::time::Instant::now());
                        self.status_msg = "验证通过，验证码已发送，请查收短信".into();
                        if let Ok(mut s) = self.shared.lock() {
                            s.log("人机验证通过，短信验证码已重新发送");
                        }
                    }
                    Err(e) => {
                        self.status_msg = format!("验证通过后重试仍失败: {e}");
                    }
                }
            }
            // 重试密码登录
            2 => {
                let user = self.pending_user.clone();
                let md5pwd = self.pending_md5.clone();
                match api::login_with_hash(&user, &md5pwd, Some(&proof)) {
                    Ok(token) => {
                        self.finish_pwd_login(&user, token, md5pwd);
                    }
                    Err(e) => {
                        self.status_msg = format!("验证通过后登录仍失败: {e}");
                    }
                }
            }
            _ => {}
        }
    }

    /// 密码登录成功后的收尾：按“记住密码”开关决定凭据存留
    fn finish_pwd_login(&mut self, user: &str, token: String, md5pwd: String) {
        let md5_to_save = if self.remember_pwd {
            Some(md5pwd)
        } else {
            None
        };
        if !self.save_token(user, token, md5_to_save) {
            return;
        }
        if self.remember_pwd {
            self.acc_pwd = PWD_PLACEHOLDER.to_string();
        } else {
            // 明确不记住：清掉已保存凭据
            if let Ok(mut c) = self.config.lock() {
                if !c.account.cred_enc.is_empty() {
                    c.account.cred_enc.clear();
                    self.dirty = true;
                }
            }
            self.acc_pwd.clear();
        }
        self.status_msg = "登录成功".into();
    }
}

impl App {
    fn render_shell(&mut self, ctx: &egui::Context) {
        let screen = ctx.screen_rect();
        let short = screen.height() < 620.0;
        let sidebar_width = if screen.width() >= 1000.0 {
            208.0
        } else {
            170.0
        };
        egui::SidePanel::left("nav")
            .resizable(false)
            .exact_width(sidebar_width)
            .frame(egui::Frame::new())
            .show(ctx, |ui| {
                theme::sidebar_background(ui);
                ui.add_space(if short { 14.0 } else { 42.0 });
                ui.vertical_centered(|ui| {
                    let size = if short { 44.0 } else { 76.0 };
                    ui.add(egui::Image::new(&self.brand).fit_to_exact_size(egui::vec2(size, size)));
                    ui.add_space(if short { 0.0 } else { 4.0 });
                    ui.label(theme::title("雷神守护", if short { 19.0 } else { 23.0 }));
                    if !short {
                        ui.label(
                            egui::RichText::new("Leigod Guard")
                                .size(14.0)
                                .color(theme::MUTED),
                        );
                    }
                });
                ui.add_space(if short { 8.0 } else { 22.0 });
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(16, 0))
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = if short { 3.0 } else { 10.0 };
                        for (page, icon, label) in [
                            (Page::Games, Icon::Home, "首页与游戏"),
                            (Page::Account, Icon::Account, "账户"),
                            (Page::Strategy, Icon::Shield, "策略"),
                            (Page::Logs, Icon::Logs, "日志"),
                        ] {
                            if theme::navigation(ui, icon, label, self.page == page).clicked() {
                                self.page = page;
                                self.status_msg.clear();
                            }
                        }
                    });
                let bottom = ui.max_rect().bottom();
                let footer = egui::Rect::from_min_max(
                    egui::pos2(ui.max_rect().left() + 16.0, bottom - 129.0),
                    egui::pos2(ui.max_rect().right() - 16.0, bottom - 12.0),
                );
                ui.scope_builder(egui::UiBuilder::new().max_rect(footer), |ui| {
                    ui.separator();
                    let updates =
                        theme::navigation(ui, Icon::Info, "关于与更新", self.page == Page::Updates);
                    if self.update_release.is_some() {
                        ui.painter().circle_filled(
                            updates.rect.right_top() + egui::vec2(-9.0, 9.0),
                            3.0,
                            theme::BLUE,
                        );
                    }
                    if updates.clicked() {
                        self.page = Page::Updates;
                    }
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "Windows 版 · {}",
                                env!("CARGO_PKG_VERSION")
                            ))
                            .size(11.0)
                            .color(theme::MUTED),
                        );
                        if ui.small_button("隐藏到托盘").clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                            hide_window_native();
                        }
                    });
                });
            });
        let margin = if screen.width() >= 1000.0 { 28 } else { 20 };
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(theme::BACKGROUND)
                    .inner_margin(egui::Margin::symmetric(margin, 24)),
            )
            .show(ctx, |ui| {
                let content = egui::ScrollArea::vertical()
                    .id_salt(self.page as u8)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        match self.page {
                            Page::Games => self.page_games(ui),
                            Page::Plans => self.page_plans(ui),
                            Page::Account => {
                                page_header(ui, "账户", "管理雷神账号与本机保存的登录凭据。");
                                theme::card().show(ui, |ui| {
                                    ui.set_min_width(ui.available_width());
                                    self.page_account(ui);
                                });
                            }
                            Page::Strategy => {
                                page_header(ui, "策略", "按你的游戏习惯，安排自动暂停。");
                                theme::card().show(ui, |ui| {
                                    ui.set_min_width(ui.available_width());
                                    self.page_strategy(ui);
                                });
                            }
                            Page::Logs => {
                                page_header(ui, "日志", "查看检测结果与暂停请求，了解每一次守护。");
                                theme::card().show(ui, |ui| {
                                    ui.set_min_width(ui.available_width());
                                    self.page_logs(ui);
                                });
                            }
                            Page::Updates => {
                                page_header(
                                    ui,
                                    "关于与更新",
                                    "选择合适的下载来源，让守护保持更新。",
                                );
                                self.page_updates(ui);
                            }
                        }
                    });
                #[cfg(test)]
                assert!(
                    content.content_size.x <= content.inner_rect.width() + 1.0,
                    "page {} overflows horizontally: {:?}",
                    self.page as u8,
                    content.content_size
                );
                #[cfg(not(test))]
                let _ = content;
            });
        if self.show_add_game {
            let mut open = self.show_add_game;
            egui::Window::new("添加游戏")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .default_pos(screen.center() - egui::vec2(237.0, 200.0))
                .default_width(430.0)
                .default_height(290.0)
                .frame(theme::card())
                .max_width((screen.width() - 70.0).max(280.0))
                .max_height((screen.height() - 80.0).max(220.0))
                .vscroll(true)
                .show(ctx, |ui| self.game_add_form(ui));
            self.show_add_game &= open;
        }
    }
}

fn page_header(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    ui.label(theme::title(title, 27.0));
    ui.label(egui::RichText::new(subtitle).color(theme::MUTED));
    ui.add_space(18.0);
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 关闭按钮 → 隐藏到托盘而不是退出
        if ctx.input(|i| i.viewport().close_requested()) {
            dbglog("close_requested -> CancelClose + hide");
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            hide_window_native();
        }

        if !self.update_preparing.load(Ordering::Acquire) {
            self.poll_captcha();
        }
        self.poll_update_events(ctx);

        if self.update_preparing.load(Ordering::Acquire) {
            // Handoff holds the config mutex. Do not render normal pages,
            // account actions, process pickers, or autosave until it finishes.
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(70.0);
                    ui.heading("正在准备更新");
                    ui.add_space(12.0);
                    ui.spinner();
                    ui.label("正在保存配置并检查更新文件，请稍候。");
                    ui.label("监控将短暂停止，更新完成后程序会重新打开。");
                    ui.label("更新程序本身不暂停计时；重新打开后会按启动设置等待并检查。");
                });
            });
            ctx.request_repaint_after(Duration::from_millis(200));
            return;
        }

        // OSD 排除配置提权写入后，轮询一段时间等待配置文件落盘以刷新页面状态
        if let Some(deadline) = self.osd_poll_until {
            if std::time::Instant::now() < deadline {
                ctx.request_repaint_after(std::time::Duration::from_millis(500));
            } else {
                self.osd_poll_until = None;
            }
        }

        self.render_shell(ctx);

        // 进程选择弹窗
        if self.show_proc_picker {
            self.proc_picker_window(ctx);
        }

        if self.dirty {
            if let Ok(c) = self.config.lock() {
                match c.save() {
                    Ok(()) => self.dirty = false,
                    Err(e) => self.status_msg = format!("保存配置失败: {e}"),
                }
            }
        }

        ctx.request_repaint_after(Duration::from_millis(500));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GameFormAction {
    None,
    Add,
    PickProcess,
}

/// Pure form rendering: it needs no account, configuration file, or live process.
fn game_entry_form(
    ui: &mut egui::Ui,
    selected: &mut Option<usize>,
    name: &mut String,
    exe: &mut String,
) -> GameFormAction {
    ui.label("常用游戏");
    let previous = *selected;
    let label = selected
        .and_then(|index| PRESETS.get(index))
        .map(|preset| preset.name)
        .unwrap_or("自定义 / 手动填写");
    egui::ComboBox::from_id_salt("game_preset")
        .selected_text(label)
        .width(ui.available_width().min(380.0))
        .height(220.0)
        .wrap_mode(egui::TextWrapMode::Truncate)
        .show_ui(ui, |ui| {
            ui.selectable_value(selected, None, "自定义 / 手动填写");
            for (index, preset) in PRESETS.iter().enumerate() {
                ui.selectable_value(
                    selected,
                    Some(index),
                    format!("{}\n{}", preset.name, preset.exe),
                );
            }
        });
    if *selected != previous {
        if let Some(preset) = selected.and_then(|index| PRESETS.get(index)) {
            *name = preset.name.to_string();
            *exe = preset.exe.to_string();
        }
    }
    ui.label(
        egui::RichText::new(
            "选择后自动填入名称和进程名，下面仍可修改。不同区服或版本可从运行进程中确认。",
        )
        .weak()
        .small(),
    );
    if let Some(preset) = selected.and_then(|index| PRESETS.get(index)) {
        ui.hyperlink_to("进程名参考", preset.source);
        if preset.exe == "League of Legends.exe" {
            ui.label(
                egui::RichText::new("英雄联盟此项仅识别对局；返回大厅后会进入退出宽限期。")
                    .weak()
                    .small(),
            );
        }
    }
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.label("名称:");
        if ui
            .add(
                egui::TextEdit::singleline(&mut *name)
                    .hint_text("游戏显示名称")
                    .desired_width(ui.available_width().min(240.0)),
            )
            .changed()
        {
            *selected = None;
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.label("进程名:");
        if ui
            .add(
                egui::TextEdit::singleline(&mut *exe)
                    .hint_text("如 game.exe")
                    .desired_width(ui.available_width().min(360.0)),
            )
            .changed()
        {
            *selected = None;
        }
    });
    let mut action = GameFormAction::None;
    ui.horizontal_wrapped(|ui| {
        if ui.button("从运行进程选择…").clicked() {
            action = GameFormAction::PickProcess;
        }
        if ui
            .add_enabled(!exe.trim().is_empty(), egui::Button::new("添加到名单"))
            .clicked()
        {
            action = GameFormAction::Add;
        }
    });
    action
}

#[cfg(test)]
mod ui_tests {
    use super::{
        game_entry_form, request_startup_defer, GameFormAction, ManualCmd, Shared, PRESETS,
    };
    use std::time::{Duration, Instant};

    #[test]
    fn startup_defer_only_queues_for_a_pending_check() {
        let now = Instant::now();
        let mut shared = Shared::default();
        shared.startup_pause_status.pending = false;
        assert!(!request_startup_defer(&mut shared, now));
        assert!(shared.startup_defer_requested_at.is_none());

        shared.startup_pause_status.pending = true;
        shared.manual_cmd = Some(ManualCmd::Pause);
        assert!(request_startup_defer(&mut shared, now));
        let next_click = now + Duration::from_secs(30);
        assert!(request_startup_defer(&mut shared, next_click));
        assert_eq!(shared.startup_defer_requested_at, Some(next_click));
        assert!(matches!(shared.manual_cmd, Some(ManualCmd::Pause)));
        assert!(shared.alert.is_none(), "deferral must not show a popup");

        shared.startup_pause_status.pending = false;
        shared.startup_defer_requested_at = None;
        assert!(!request_startup_defer(&mut shared, next_click));
        assert!(shared.startup_defer_requested_at.is_none());
    }

    #[test]
    fn add_game_form_fits_narrow_content_area_without_native_app() {
        // 440 px approximates the content left by navigation/window chrome in
        // the minimum 680x460 window. This only runs egui's in-memory layout.
        for preset in [None, Some(3), Some(5)] {
            let context = egui::Context::default();
            let mut selected = preset;
            let mut name = preset
                .map(|index| PRESETS[index].name)
                .unwrap_or("自定义游戏")
                .to_owned();
            let mut exe = preset
                .map(|index| PRESETS[index].exe)
                .unwrap_or("custom-game.exe")
                .to_owned();
            let before = (name.clone(), exe.clone());
            let output = context.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(440.0, 430.0),
                    )),
                    ..Default::default()
                },
                |context| {
                    egui::CentralPanel::default().show(context, |ui| {
                        let available = ui.max_rect();
                        assert_eq!(
                            game_entry_form(ui, &mut selected, &mut name, &mut exe),
                            GameFormAction::None
                        );
                        assert!(
                            ui.min_rect().right() <= available.right() + 1.0,
                            "form overflows horizontally"
                        );
                        assert!(
                            ui.min_rect().bottom() <= available.bottom() + 1.0,
                            "form overflows vertically"
                        );
                    });
                },
            );
            assert!(!output.shapes.is_empty());
            assert_eq!(
                (name, exe),
                before,
                "rendering must not overwrite custom or edited values"
            );
        }
    }
}

impl App {
    fn page_games(&mut self, ui: &mut egui::Ui) {
        let config = match self.config.lock() {
            Ok(config) => config.clone(),
            Err(_) => {
                ui.label("暂时无法读取配置，请稍后重试。");
                return;
            }
        };
        let snapshot = self.shared.lock().ok().map(|s| {
            (
                s.startup_pause_status,
                s.startup_defer_requested_at.is_some(),
                s.process_snapshot.clone(),
                s.status.clone(),
            )
        });
        let state = HomeState {
            startup: snapshot.as_ref().map(|s| (s.0, s.1)),
            strategy: &config.strategy,
            games: &config.games,
            processes: snapshot.as_ref().and_then(|s| s.2.as_deref()),
            status: snapshot
                .as_ref()
                .map(|s| s.3.as_str())
                .unwrap_or("暂时无法读取监控状态"),
        };
        let mut enabled = config.strategy.enabled;
        let action = crate::ui_home::render(ui, &state, &mut enabled);
        if enabled != config.strategy.enabled {
            if let Ok(mut config) = self.config.lock() {
                config.strategy.enabled = enabled;
                self.dirty = true;
            }
        }
        match action {
            HomeAction::None => {}
            HomeAction::Defer => {
                if let Ok(mut shared) = self.shared.lock() {
                    request_startup_defer(&mut shared, Instant::now());
                }
                ui.ctx().request_repaint();
            }
            HomeAction::Pause => self.request_manual_pause(),
            HomeAction::Strategy => self.page = Page::Strategy,
            HomeAction::AddGame => self.show_add_game = true,
            HomeAction::RemoveGame(index) => {
                if let Ok(mut config) = self.config.lock() {
                    if index < config.games.len() {
                        config.games.remove(index);
                        self.dirty = true;
                    }
                }
            }
        }
        if !self.status_msg.is_empty() {
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(&self.status_msg)
                    .size(13.0)
                    .color(theme::MUTED),
            );
        }
    }

    fn request_manual_pause(&mut self) {
        if let Ok(mut shared) = self.shared.lock() {
            shared.manual_pause_result = None;
            shared.manual_cmd = Some(ManualCmd::Pause);
            shared.log("界面指令：立即暂停计时");
        }
        self.status_msg = "暂停指令已发送，请在雷神官方微信小程序下拉刷新核对计时状态。".into();
    }

    fn game_add_form(&mut self, ui: &mut egui::Ui) {
        match game_entry_form(
            ui,
            &mut self.new_preset,
            &mut self.new_name,
            &mut self.new_exe,
        ) {
            GameFormAction::PickProcess => {
                self.proc_list = crate::monitor::running_process_names();
                self.proc_list.sort();
                self.proc_list.dedup();
                self.show_proc_picker = true;
            }
            GameFormAction::Add => {
                let exe = self.new_exe.trim().to_string();
                if !crate::config::valid_game_executable(&exe) {
                    self.status_msg =
                        "请填写单个游戏进程文件名（如 game.exe），不要填写路径、通配符或启动命令。"
                            .into();
                    return;
                }
                let name = if self.new_name.trim().is_empty() {
                    exe[..exe.len() - 4].to_string()
                } else {
                    self.new_name.trim().to_string()
                };
                let added = match self.config.lock() {
                    Ok(mut c) => {
                        if game_presets::contains_executable(
                            c.games.iter().map(|game| game.exe.as_str()),
                            &exe,
                        ) {
                            self.status_msg = format!("{exe} 已在名单中，无需重复添加。");
                            false
                        } else {
                            c.games.push(GameEntry {
                                name,
                                exe,
                                plan: self.new_plan.clone(),
                            });
                            self.dirty = true;
                            self.status_msg.clear();
                            true
                        }
                    }
                    Err(_) => {
                        self.status_msg = "暂时无法保存游戏名单，请重试。".into();
                        false
                    }
                };
                if added {
                    self.show_add_game = false;
                    self.new_name.clear();
                    self.new_exe.clear();
                    self.new_preset = None;
                }
            }
            GameFormAction::None => {}
        }
        if !self.status_msg.is_empty() {
            ui.colored_label(egui::Color32::from_rgb(230, 60, 60), &self.status_msg);
        }
    }

    fn proc_picker_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_proc_picker;
        let mut picked = None;
        egui::Window::new("选择正在运行的进程")
            .open(&mut open)
            .default_size([360.0, 420.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("过滤:");
                    ui.text_edit_singleline(&mut self.proc_filter);
                });
                ui.separator();
                let filter = self.proc_filter.to_lowercase();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    let list = self.proc_list.clone();
                    for p in list {
                        if !filter.is_empty() && !p.to_lowercase().contains(&filter) {
                            continue;
                        }
                        if ui.button(&p).clicked() {
                            picked = Some(p);
                        }
                    }
                });
            });
        self.show_proc_picker = open;
        if let Some(process) = picked {
            self.new_exe = process.clone();
            self.new_preset = None;
            if self.new_name.is_empty() {
                self.new_name = process.trim_end_matches(".exe").to_string();
            }
            self.show_proc_picker = false;
        }
    }

    fn page_plans(&mut self, ui: &mut egui::Ui) {
        ui.heading("预设加速方案");
        ui.label("此页仅保存方案备注，当前版本不会应用区服或节点，也不会自动开始加速或恢复计时。请在雷神客户端手动操作。");
        ui.add_space(8.0);

        let mut to_delete: Option<usize> = None;
        egui::Grid::new("plans_grid")
            .num_columns(7)
            .striped(true)
            .show(ui, |ui| {
                ui.strong("方案名");
                ui.strong("游戏");
                ui.strong("区服");
                ui.strong("节点");
                ui.strong("模式");
                ui.strong("备注");
                ui.strong("操作");
                ui.end_row();
                if let Ok(mut c) = self.config.lock() {
                    for (i, p) in c.plans.iter_mut().enumerate() {
                        if ui.text_edit_singleline(&mut p.name).changed() {
                            self.dirty = true;
                        }
                        if ui.text_edit_singleline(&mut p.game).changed() {
                            self.dirty = true;
                        }
                        if ui
                            .add(egui::TextEdit::singleline(&mut p.region).desired_width(70.0))
                            .changed()
                        {
                            self.dirty = true;
                        }
                        if ui
                            .add(egui::TextEdit::singleline(&mut p.node).desired_width(70.0))
                            .changed()
                        {
                            self.dirty = true;
                        }
                        if ui
                            .add(egui::TextEdit::singleline(&mut p.mode).desired_width(70.0))
                            .changed()
                        {
                            self.dirty = true;
                        }
                        if ui
                            .add(egui::TextEdit::singleline(&mut p.note).desired_width(90.0))
                            .changed()
                        {
                            self.dirty = true;
                        }
                        if ui.button("删除").clicked() {
                            to_delete = Some(i);
                        }
                        ui.end_row();
                    }
                }
            });
        if let Some(i) = to_delete {
            if let Ok(mut c) = self.config.lock() {
                c.plans.remove(i);
                self.dirty = true;
            }
        }

        ui.add_space(12.0);
        ui.separator();
        ui.strong("新增方案");
        ui.horizontal_wrapped(|ui| {
            ui.label("方案名:");
            ui.add(egui::TextEdit::singleline(&mut self.plan_name).desired_width(90.0));
            ui.label("游戏:");
            ui.add(egui::TextEdit::singleline(&mut self.plan_game).desired_width(90.0));
            ui.label("区服:");
            ui.add(egui::TextEdit::singleline(&mut self.plan_region).desired_width(70.0));
            ui.label("节点:");
            ui.add(egui::TextEdit::singleline(&mut self.plan_node).desired_width(70.0));
            ui.label("模式:");
            ui.add(egui::TextEdit::singleline(&mut self.plan_mode).desired_width(70.0));
            ui.label("备注:");
            ui.add(egui::TextEdit::singleline(&mut self.plan_note).desired_width(90.0));
            if ui.button("添加").clicked() && !self.plan_name.trim().is_empty() {
                if let Ok(mut c) = self.config.lock() {
                    c.plans.push(AccelPlan {
                        name: self.plan_name.trim().to_string(),
                        game: self.plan_game.trim().to_string(),
                        region: self.plan_region.trim().to_string(),
                        node: self.plan_node.trim().to_string(),
                        mode: self.plan_mode.trim().to_string(),
                        note: self.plan_note.trim().to_string(),
                    });
                    self.dirty = true;
                }
                self.plan_name.clear();
                self.plan_game.clear();
                self.plan_region.clear();
                self.plan_node.clear();
                self.plan_mode.clear();
                self.plan_note.clear();
            }
        });
        ui.add_space(8.0);
        ui.colored_label(
            egui::Color32::GRAY,
            "提示：方案导入与自动应用尚未实现，当前保存的内容仅作备注。",
        );
    }

    /// 登录成功后的统一处理：token/凭据加密落盘 + 更新共享状态
    fn save_token(&mut self, user: &str, token: String, md5pwd: Option<String>) -> bool {
        let token_enc = match dpapi::protect(&token) {
            Ok(value) => value,
            Err(_) => {
                self.status_msg = "无法加密保存登录凭据，请检查当前 Windows 用户环境".into();
                return false;
            }
        };
        let cred_enc = match md5pwd.as_deref().map(dpapi::protect).transpose() {
            Ok(value) => value.unwrap_or_default(),
            Err(_) => {
                self.status_msg = "无法加密保存密码凭据，请重试或取消记住密码".into();
                return false;
            }
        };
        if let Ok(mut c) = self.config.lock() {
            c.account = crate::config::Account {
                username: user.to_string(),
                token_enc,
                cred_enc,
            };
            self.dirty = true;
        }
        self.acc_user = user.to_string();
        self.remember_pwd = md5pwd.is_some();
        self.acc_pwd = if self.remember_pwd {
            PWD_PLACEHOLDER.into()
        } else {
            String::new()
        };
        if let Ok(mut s) = self.shared.lock() {
            s.token = Some(token);
            s.account_status =
                format!("已登录（{}）", if user.is_empty() { "token" } else { user });
            s.log("账户登录成功，token 已加密保存");
        }
        // 登录成功后立即拉取账户信息展示
        self.refresh_account_info();
        true
    }

    /// 拉取账户信息并展示（登录后自动调用 + “刷新账户状态”按钮）
    fn refresh_account_info(&mut self) {
        let token = self.shared.lock().ok().and_then(|s| s.token.clone());
        let Some(t) = token else {
            self.status_msg = "尚未登录".into();
            return;
        };
        let user = self
            .config
            .lock()
            .map(|c| c.account.username.clone())
            .unwrap_or_default();
        match api::user_info(&t) {
            Ok(v) => {
                let paused = v.pointer("/data/pause_status_id").and_then(|x| x.as_i64()) == Some(1);
                let state = if paused { "已暂停" } else { "计时中" };
                if let Ok(mut s) = self.shared.lock() {
                    s.account_info = Some(v);
                    s.account_status = if user.is_empty() {
                        format!("已登录 · {state}")
                    } else {
                        format!("已登录（{user}）· {state}")
                    };
                }
            }
            Err(e) => {
                dbglog(&format!("[ui] user_info failed: {}", e.0));
                if let Ok(mut s) = self.shared.lock() {
                    s.account_status = format!("查询失败: {e}");
                }
            }
        }
    }

    fn page_account(&mut self, ui: &mut egui::Ui) {
        ui.label(
            egui::RichText::new(
                "登录凭据加密保存在本机，仅当前 Windows 用户可读取。登录失效后，请重新登录。",
            )
            .color(theme::MUTED),
        );
        ui.add_space(6.0);

        // 登录方式切换
        ui.horizontal_wrapped(|ui| {
            for (mode, label) in [(0u8, "密码登录"), (1, "验证码登录"), (2, "手动 Token")]
            {
                if ui
                    .selectable_label(self.login_mode == mode, label)
                    .clicked()
                {
                    self.login_mode = mode;
                }
            }
        });
        ui.separator();

        match self.login_mode {
            // ---- 密码登录（可能受极验验证码限制）----
            0 => {
                ui.horizontal_wrapped(|ui| {
                    ui.label("手机号:");
                    ui.add(egui::TextEdit::singleline(&mut self.acc_user).desired_width(140.0));
                    ui.label("密码:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.acc_pwd)
                            .password(true)
                            .desired_width(140.0),
                    );
                    ui.checkbox(&mut self.remember_pwd, "记住密码");
                });
                ui.horizontal_wrapped(|ui| {
                    if theme::primary(ui,"登录并保存").clicked() {
                        let user = self.acc_user.trim().to_string();
                        let pwd = self.acc_pwd.clone();
                        if user.is_empty() {
                            self.status_msg = "请输入手机号".into();
                        } else {
                            // 占位符或空密码 → 尝试使用已保存的凭据
                            let md5pwd = if pwd.is_empty() || pwd == PWD_PLACEHOLDER {
                                let enc = self
                                    .config
                                    .lock()
                                    .map(|c| c.account.cred_enc.clone())
                                    .unwrap_or_default();
                                if enc.is_empty() {
                                    self.status_msg = "请输入密码".into();
                                    None
                                } else {
                                    match dpapi::unprotect(&enc) {
                                        Ok(m) => Some(m),
                                        Err(e) => {
                                            self.status_msg =
                                                format!("读取已保存密码失败: {e}，请重新输入密码");
                                            None
                                        }
                                    }
                                }
                            } else {
                                Some(api::password_md5(&pwd))
                            };
                            if let Some(md5pwd) = md5pwd {
                                self.status_msg = "登录中…".into();
                                match api::login_with_hash(&user, &md5pwd, None) {
                                    Ok(token) => {
                                        self.finish_pwd_login(&user, token, md5pwd);
                                    }
                                    Err(e) => {
                                        if api::is_captcha_err(&e) {
                                            self.start_captcha(2, &user, &md5pwd);
                                        } else {
                                            self.status_msg = format!(
                                                "登录失败: {e}（也可改用“验证码登录”或“手动 Token”）"
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                    ui.colored_label(
                        egui::Color32::GRAY,
                        "勾选“记住密码”后保存加密凭据，方便下次登录。",
                    );
                });
            }
            // ---- 短信验证码登录 ----
            1 => {
                ui.horizontal_wrapped(|ui| {
                    ui.label("手机号:");
                    ui.add(egui::TextEdit::singleline(&mut self.sms_phone).desired_width(140.0));
                    let cooldown = self
                        .sms_sent_at
                        .map(|t| t.elapsed().as_secs() < 60)
                        .unwrap_or(false);
                    let label = if cooldown {
                        let left = 60 - self.sms_sent_at.unwrap().elapsed().as_secs();
                        format!("{left} 秒后重发")
                    } else {
                        "发送验证码".to_string()
                    };
                    if ui
                        .add_enabled(!cooldown, egui::Button::new(label))
                        .clicked()
                    {
                        let phone = self.sms_phone.trim().to_string();
                        if phone.len() != 11 {
                            self.status_msg = "请输入 11 位手机号".into();
                        } else {
                            match api::send_sms_code(&phone, None) {
                                Ok(key) => {
                                    self.sms_key = key;
                                    self.sms_sent_at = Some(std::time::Instant::now());
                                    self.status_msg = "验证码已发送，请查收短信".into();
                                    if let Ok(mut s) = self.shared.lock() {
                                        s.log("已请求发送短信验证码");
                                    }
                                }
                                Err(e) => {
                                    if api::is_captcha_err(&e) {
                                        self.start_captcha(1, &phone, "");
                                    } else {
                                        self.status_msg = format!("发送失败: {e}");
                                    }
                                }
                            }
                        }
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("验证码:");
                    ui.add(egui::TextEdit::singleline(&mut self.sms_code).desired_width(100.0));
                    if theme::primary(ui, "登录并保存").clicked() {
                        let phone = self.sms_phone.trim().to_string();
                        let code = self.sms_code.trim().to_string();
                        if code.is_empty() || self.sms_key.is_empty() {
                            self.status_msg = "请先发送验证码并填写收到的短信验证码".into();
                        } else {
                            match api::login_with_code(&phone, &code, &self.sms_key.clone()) {
                                Ok(token) => {
                                    let phone2 = phone.clone();
                                    if self.save_token(&phone2, token, None) {
                                        self.status_msg = "登录成功（验证码方式）".into();
                                        self.sms_code.clear();
                                    }
                                }
                                Err(e) => self.status_msg = format!("登录失败: {e}"),
                            }
                        }
                    }
                });
                ui.colored_label(
                    egui::Color32::GRAY,
                    "说明：验证码登录不保存密码，token 过期后需重新收码登录。",
                );
            }
            // ---- 手动粘贴 token ----
            _ => {
                ui.label("在浏览器登录雷神官网(vip.leigod.com)，F12 → Network，找任意请求的 account_token 参数，粘贴到下面：");
                ui.horizontal_wrapped(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.token_input)
                            .password(true)
                            .desired_width(ui.available_width().min(360.0)),
                    );
                    if theme::primary(ui, "保存并使用").clicked() {
                        let t = self.token_input.trim().to_string();
                        if t.len() < 20 {
                            self.status_msg = "token 看起来太短，请确认复制完整".into();
                        } else {
                            if self.save_token("", t, None) {
                                self.status_msg = "token 已保存".into();
                                self.token_input.clear();
                            }
                        }
                    }
                });
                ui.colored_label(
                    egui::Color32::GRAY,
                    "说明：token 有效期由雷神服务决定，失效后请重新登录并更新。",
                );
            }
        }

        ui.add_space(10.0);
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            if ui.button("立即暂停计时").clicked() {
                if let Ok(mut s) = self.shared.lock() {
                    s.manual_cmd = Some(ManualCmd::Pause);
                }
                self.status_msg =
                    "暂停指令已发送；成功提示仅为请求反馈，请在雷神官方微信小程序刷新核对计时状态"
                        .into();
            }
            // 二期功能：手动恢复入口暂时隐藏（代码保留，见 worker ManualCmd::Resume）
            if ui.button("刷新账户状态").clicked() {
                self.status_msg = "正在查询账户信息…".into();
                self.refresh_account_info();
            }
        });
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            if ui.button("退出登录").clicked() {
                // 清空内存 token、本地加密凭据与界面状态
                if let Ok(mut s) = self.shared.lock() {
                    s.token = None;
                    s.account_info = None;
                    s.account_status = "未登录".into();
                    s.log("已退出登录，本地 token 与凭据已清除");
                }
                if let Ok(mut c) = self.config.lock() {
                    c.account.token_enc.clear();
                    c.account.cred_enc.clear();
                    c.account.username.clear();
                    self.dirty = true;
                }
                self.acc_pwd.clear();
                self.remember_pwd = false;
                self.sms_code.clear();
                self.sms_key.clear();
                self.sms_sent_at = None;
                self.token_input.clear();
                self.status_msg = "已退出登录".into();
            }
            if ui.button("退出程序").clicked() {
                // 账户页保留退出选择；托盘菜单直接退出。
                try_exit(&self.shared, &self.config, &self.update_preparing);
            }
            ui.label(
                egui::RichText::new(
                    "点关闭按钮只是最小化到托盘；要彻底退出请用「退出程序」或托盘菜单",
                )
                .weak()
                .small(),
            );
        });

        let acc_status = self
            .shared
            .lock()
            .map(|s| s.account_status.clone())
            .unwrap_or_default();
        ui.label(format!("状态：{acc_status}"));
        if !self.status_msg.is_empty() {
            ui.colored_label(egui::Color32::from_rgb(230, 60, 60), &self.status_msg);
        }

        // 账户信息展示：计时状态高亮 + 完整信息折叠区
        let info = self.shared.lock().ok().and_then(|s| s.account_info.clone());
        if let Some(v) = info {
            ui.add_space(8.0);
            ui.separator();
            let data = v.get("data").cloned().unwrap_or(v.clone());
            let paused = data.get("pause_status_id").and_then(|x| x.as_i64()) == Some(1);
            let (txt, color) = if paused {
                ("⏸ 计时状态：已暂停", egui::Color32::from_rgb(80, 180, 80))
            } else {
                ("⏱ 计时状态：计时中", egui::Color32::from_rgb(230, 160, 40))
            };
            ui.colored_label(color, txt);
            ui.label("成功提示仅为请求反馈，最终以小程序刷新后的计时状态为准。\n核对：雷神官方微信小程序 → 登录同一账号 → 下拉刷新。\n暂停计时不代表已停止加速。账户资料和凭据不在此展示。");
        }
    }

    fn page_updates(&mut self, ui: &mut egui::Ui) {
        theme::card().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.add_space(8.0);
            ui.label(format!("雷神守护 v{} · Windows x64", env!("CARGO_PKG_VERSION")));
            match &self.update_kind {
                Ok(PackageKind::Installer) => { ui.label("当前使用方式：安装版"); }
                Ok(PackageKind::Portable) => { ui.label("当前使用方式：绿色免安装版"); }
                Err(_) => { ui.label("当前使用方式：未能确认，请使用手动下载"); }
            }
            ui.label("个人维护的第三方开源工具，与雷神加速器官方无隶属关系。");
            ui.add_space(12.0);
            ui.separator();
            ui.add_space(8.0);
            if let Ok(mut config) = self.config.lock() {
                let previous_source = config.updates.source;
                ui.add_enabled_ui(!self.update_busy, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label("更新来源：");
                        egui::ComboBox::from_id_salt("update_source")
                            .selected_text(match config.updates.source {
                                UpdateSource::GitHub => "GitHub",
                                UpdateSource::Gitee => "Gitee（国内）",
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut config.updates.source, UpdateSource::GitHub, "GitHub");
                                ui.selectable_value(&mut config.updates.source, UpdateSource::Gitee, "Gitee（国内）");
                            });
                    });
                });
                if config.updates.source != previous_source {
                    self.dirty = true;
                    self.update_release = None;
                    self.update_progress = None;
                    self.update_error = false;
                    self.update_message = format!("已切换到 {}，请点击“检查更新”。", config.updates.source.label());
                }
                if ui.checkbox(&mut config.updates.check_on_startup, "启动时自动检查更新").changed() {
                    self.dirty = true;
                }
            }
            ui.label(egui::RichText::new(
                "国内网络可选择 Gitee。版本检查、安装包和校验文件均使用所选来源；启动检查默认关闭，只有点击更新才会下载和安装。"
            ).weak().small());
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled(!self.update_busy, egui::Button::new("检查更新")).clicked() {
                    self.start_update_check(ui.ctx().clone());
                }
                ui.hyperlink_to("Gitee 下载", crate::updater::GITEE_RELEASES_PAGE);
                ui.hyperlink_to("GitHub 下载", crate::updater::RELEASES_PAGE);
                ui.hyperlink_to("项目使用说明", "https://github.com/CMMUU/leigod-guard#readme");
            });
            ui.add_space(8.0);
            if self.update_error {
                ui.colored_label(egui::Color32::from_rgb(230, 100, 70), &self.update_message);
            } else {
                ui.label(&self.update_message);
            }
            if self.update_busy {
                if let Some(progress) = self.update_progress {
                    if let Some(total) = progress.total.filter(|total| *total > 0) {
                        ui.add(egui::ProgressBar::new(
                            (progress.downloaded as f32 / total as f32).clamp(0.0, 1.0)
                        ).show_percentage().desired_width(ui.available_width().min(340.0)));
                    } else {
                        ui.spinner();
                    }
                    ui.label(format!("已下载 {:.1} MiB", progress.downloaded as f64 / 1_048_576.0));
                } else {
                    ui.spinner();
                }
            }
            if let Some(release) = self.update_release.clone() {
                ui.add_space(12.0);
                ui.separator();
                ui.add_space(8.0);
                ui.heading(format!("可更新至 v{}", release.version));
                ui.label(format!("此次下载来源：{}", release.source.label()));
                ui.label("更新会保留配置和当前使用方式：安装版继续使用安装版，绿色版继续免安装。");
                ui.label("点击后会下载并校验文件，再关闭当前程序完成更新并重新打开。监控会短暂停止；更新程序本身不暂停计时，重新打开后按启动设置等待并检查。");
                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    if ui.add_enabled(!self.update_busy && self.update_kind.is_ok(),
                        egui::Button::new("下载并更新")).clicked() {
                        self.start_update_download(ui.ctx().clone());
                    }
                    ui.hyperlink_to("查看此版本说明", &release.page_url);
                });
                if !release.notes.trim().is_empty() {
                    ui.add_space(8.0);
                    ui.collapsing("更新内容", |ui| { ui.label(&release.notes); });
                }
            }
            if self.status_msg.starts_with("保存配置失败") {
                ui.add_space(8.0);
                ui.colored_label(egui::Color32::from_rgb(230, 100, 70), &self.status_msg);
            }
        });
    }

    fn page_strategy(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        if let Ok(mut c) = self.config.lock() {
            if ui
                .checkbox(&mut c.strategy.enabled, "启用自动暂停（总开关）")
                .changed()
            {
                self.dirty = true;
            }
            ui.label(
                egui::RichText::new("控制启动等待后的检查，以及游戏全部退出后的暂停。")
                    .weak()
                    .small(),
            );
            ui.add_space(4.0);
            if ui
                .checkbox(
                    &mut c.strategy.pause_on_startup,
                    "启动时无游戏运行则暂停计时",
                )
                .changed()
            {
                self.dirty = true;
            }
            ui.label(egui::RichText::new("默认开启。启动后先等待，再检查名单中的游戏。关闭会结束本次等待，重新开启要下次启动才检查；空名单或检测失败不会被当作无游戏。").weak().small());
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.label("启动等待（秒）:");
                if ui
                    .add(egui::DragValue::new(&mut c.strategy.startup_grace_secs).range(0..=3600))
                    .changed()
                {
                    self.dirty = true;
                }
            });
            ui.label(egui::RichText::new("默认180秒（3分钟），调整会影响尚未完成的启动等待。正在准备游戏时，可在首页或托盘延后10分钟；检测到游戏就结束本次启动检查。").weak().small());
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                ui.label("游戏退出宽限期（秒）:");
                if ui
                    .add(egui::DragValue::new(&mut c.strategy.grace_secs).range(0..=3600))
                    .changed()
                {
                    self.dirty = true;
                }
            });
            ui.label(egui::RichText::new("默认90秒。名单中的游戏全部退出后，连续等待这段时间再暂停，给切换游戏留出余地。").weak().small());
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                ui.label("进程检测间隔（秒）:");
                if ui
                    .add(egui::DragValue::new(&mut c.strategy.check_interval_secs).range(1..=60))
                    .changed()
                {
                    self.dirty = true;
                }
            });
            // 二期功能：最短运行时间与自动恢复配套，暂时隐藏（字段保留在配置中）
            ui.add_space(8.0);
            let mut auto = autostart::is_enabled();
            if ui
                .checkbox(&mut auto, "开机自动启动（最小化到托盘）")
                .changed()
            {
                match autostart::set_enabled(auto) {
                    Ok(()) => {
                        c.strategy.autostart = auto;
                        self.dirty = true;
                        self.status_msg = if auto {
                            "已开启开机自启".into()
                        } else {
                            "已关闭开机自启".into()
                        };
                    }
                    Err(e) => self.status_msg = format!("设置自启失败: {e}"),
                }
            }
            ui.add_space(4.0);
            if ui
                .checkbox(&mut c.strategy.pause_on_shutdown, "关机/注销前自动暂停计时")
                .changed()
            {
                self.dirty = true;
            }
            ui.label(
                egui::RichText::new(
                    "开启后，收到 Windows 关机或注销通知时尝试暂停计时；断电或强制结束无法保证。",
                )
                .weak()
                .small(),
            );

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(6.0);
            ui.heading("OSD 与注入屏蔽");
            ui.add_space(4.0);
            ui.strong("微星小飞机（RTSS）");
            ui.label("本工具窗口使用 DirectX 渲染，微星小飞机（RTSS）会把它误判成 3D 游戏并叠加 OSD。\
                      这里可以写入 RTSS 官方排除配置（与其自带的非游戏应用模板一致），让它不再注入本工具。");
            ui.add_space(4.0);
            if osd::rtss_excluded() {
                ui.colored_label(
                    egui::Color32::from_rgb(80, 180, 80),
                    "✔ 已屏蔽：微星小飞机不会再向本工具窗口叠加 OSD（如仍显示请重启小飞机）",
                );
            } else if osd::rtss_installed() {
                ui.label("状态：未屏蔽");
                if ui.button("一键屏蔽微星小飞机 OSD").clicked() {
                    match osd::apply_rtss_exclusion() {
                        Ok(m) => {
                            self.status_msg = m;
                            // 提权复制是异步的，轮询最多 60 秒等配置文件落盘
                            self.osd_poll_until = Some(
                                std::time::Instant::now() + std::time::Duration::from_secs(60),
                            );
                        }
                        Err(e) => self.status_msg = e,
                    }
                }
            } else {
                ui.label("未检测到微星小飞机（RTSS）安装，无需处理");
            }

            ui.add_space(10.0);
            ui.strong("游戏加加");
            let protection_active = osd::gamepp_protection_active();
            let protection_error = osd::gamepp_protection_error();
            let gamepp_modules_loaded = osd::gamepp_modules_loaded();
            if ui
                .checkbox(
                    &mut c.strategy.block_gamepp_injection,
                    "阻止游戏加加向本工具注入（完全退出并重新打开后生效）",
                )
                .changed()
            {
                self.dirty = true;
            }
            ui.label(
                egui::RichText::new(
                    "默认关闭。更改后须从托盘完全退出并重新打开；关闭保护也要重启才能撤销。",
                )
                .weak()
                .small(),
            );
            ui.label(
                egui::RichText::new(
                    "严格策略只作用于新启动的雷神守护主进程，不改变游戏进程；不会关闭游戏加加或修改它的设置，无需管理员权限。",
                )
                .weak()
                .small(),
            );
            ui.label(
                egui::RichText::new(
                    "它也会阻止其他不属于 Microsoft、Microsoft Store 或 WHQL 信任范围的 DLL，可能影响其他 OSD、录屏或输入法插件；如有异常，请关闭后完全退出并重新打开。",
                )
                .weak()
                .small(),
            );
            ui.add_space(4.0);
            match (c.strategy.block_gamepp_injection, protection_active) {
                (false, false) => {
                    ui.label("状态：未启用（默认关闭）");
                    if gamepp_modules_loaded == Some(true) {
                        ui.colored_label(
                            egui::Color32::from_rgb(220, 150, 50),
                            "当前进程检测到游戏加加模块；如需屏蔽，请开启后完全退出并重新打开。",
                        );
                    }
                }
                (true, false) => {
                    if let Some(error) = protection_error {
                        ui.colored_label(
                            egui::Color32::from_rgb(230, 60, 60),
                            format!("状态：本次启动保护失败，当前未受保护：{error}"),
                        );
                        ui.label("可关闭此选项继续普通使用，或完全退出后重试。");
                    } else {
                        ui.colored_label(
                            egui::Color32::from_rgb(220, 150, 50),
                            "状态：当前未生效。设置已保存；从托盘完全退出并重新打开后尝试启用。",
                        );
                    }
                }
                (false, true) => {
                    ui.colored_label(
                        egui::Color32::from_rgb(220, 150, 50),
                        "状态：已保存关闭；当前进程仍受保护，完全退出后重新打开即可撤销。",
                    );
                }
                (true, true) if gamepp_modules_loaded == Some(true) => {
                    ui.colored_label(
                        egui::Color32::from_rgb(220, 120, 50),
                        "Windows 保护策略已启用，但仍检测到游戏加加模块，本次未完全屏蔽；完全退出后再开或关闭兼容软件后重试。",
                    );
                }
                (true, true) if gamepp_modules_loaded.is_none() => {
                    ui.colored_label(
                        egui::Color32::from_rgb(220, 150, 50),
                        "Windows 保护策略已启用，但无法读取本进程模块列表；请以窗口实际效果为准。",
                    );
                }
                (true, true) => {
                    ui.colored_label(
                        egui::Color32::from_rgb(80, 180, 80),
                        "✔ Windows 报告保护策略已启用；当前模块扫描未发现游戏加加模块。",
                    );
                }
            }
        }
        if !self.status_msg.is_empty() {
            ui.colored_label(egui::Color32::from_rgb(230, 60, 60), &self.status_msg);
        }
        ui.add_space(12.0);
        ui.separator();
        ui.label("说明：工具仅观察进程名，不注入、不读写游戏内存；无法保证所有游戏及反作弊系统的兼容性。");
    }

    fn page_logs(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(theme::title("运行记录", 18.0));
            if ui.button("清空").clicked() {
                if let Ok(mut s) = self.shared.lock() {
                    s.logs.clear();
                }
            }
        });
        ui.separator();
        let text = self
            .shared
            .lock()
            .map(|s| s.logs.iter().cloned().collect::<Vec<_>>().join("\n"))
            .unwrap_or_default();
        if text.is_empty() {
            ui.add_space(12.0);
            ui.label(
                egui::RichText::new("暂无运行记录，检测与暂停结果会显示在这里。")
                    .color(theme::MUTED),
            );
            ui.add_space(12.0);
            return;
        }
        egui::ScrollArea::vertical()
            .stick_to_bottom(true)
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut text.as_str())
                        .font(egui::TextStyle::Monospace)
                        .frame(false)
                        .desired_rows(16)
                        .desired_width(f32::INFINITY)
                        .interactive(false),
                );
            });
    }
}
