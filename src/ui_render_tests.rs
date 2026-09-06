//! In-memory UI tests and explicit offscreen screenshots. No native app is started.
use super::*;
use crate::shared::StartupPauseStatus;
use egui::{vec2, Event, FullOutput, Pos2, Rect};

fn fixture() -> (egui::Context, App) {
    let ctx = egui::Context::default();
    load_cjk_fonts(&ctx);
    // Capture the settled UI rather than a partially faded opening animation.
    ctx.style_mut(|style| style.animation_time = 0.0);
    let mut config = Config::default();
    config.strategy = crate::config::Strategy::default();
    config.games = [
        ("Counter-Strike 2", "cs2.exe"),
        ("绝地求生", "TslGame.exe"),
        ("Apex Legends", "r5apex.exe"),
    ]
    .into_iter()
    .map(|(name, exe)| GameEntry {
        name: name.into(),
        exe: exe.into(),
        plan: String::new(),
    })
    .collect();
    let shared = Shared {
        process_snapshot: Some(vec![]),
        startup_pause_status: StartupPauseStatus {
            pending: true,
            remaining_secs: Some(156),
            preparing_game: false,
        },
        status: "启动等待中".into(),
        ..Shared::default()
    };
    let app = App::from_state(
        &ctx,
        Arc::new(Mutex::new(shared)),
        Arc::new(Mutex::new(config)),
        None,
        Ok(PackageKind::Portable),
        Arc::new(AtomicBool::new(false)),
    );
    (ctx, app)
}

fn frame(ctx: &egui::Context, app: &mut App, size: [f32; 2], events: Vec<Event>) -> FullOutput {
    ctx.run(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(size[0], size[1]))),
            events,
            focused: true,
            ..Default::default()
        },
        |ctx| {
            app.render_shell(ctx);
        },
    )
}

fn text_rect(shapes: &[egui::epaint::ClippedShape], wanted: &str) -> Rect {
    fn find(shape: &egui::Shape, wanted: &str) -> Option<Rect> {
        match shape {
            egui::Shape::Text(text) if text.galley.job.text == wanted => {
                // Wrapped horizontal labels carry first-row indentation in
                // their row bounds; galley.size() includes that blank prefix.
                let bounds = text
                    .galley
                    .rows
                    .iter()
                    .fold(Rect::NOTHING, |r, row| r.union(row.rect));
                Some(bounds.translate(text.pos.to_vec2()))
            }
            egui::Shape::Vec(shapes) => shapes.iter().find_map(|s| find(s, wanted)),
            _ => None,
        }
    }
    shapes
        .iter()
        .find_map(|s| find(&s.shape, wanted))
        .unwrap_or_else(|| panic!("missing visible control: {wanted}"))
}

fn click(ctx: &egui::Context, app: &mut App, label: &str) {
    let size = [1180.0, 780.0];
    let _ = frame(ctx, app, size, vec![]);
    let output = frame(ctx, app, size, vec![]);
    let point = text_rect(&output.shapes, label).center();
    for pressed in [true, false] {
        frame(
            ctx,
            app,
            size,
            vec![
                Event::PointerMoved(point),
                Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
}

#[test]
fn home_controls_update_real_in_memory_strategy_and_commands() {
    let (ctx, mut app) = fixture();
    click(&ctx, &mut app, "准备游戏，延后10分钟");
    assert!(app
        .shared
        .lock()
        .unwrap()
        .startup_defer_requested_at
        .is_some());
    assert!(app.shared.lock().unwrap().manual_cmd.is_none());
    click(&ctx, &mut app, "立即暂停计时");
    assert!(matches!(
        app.shared.lock().unwrap().manual_cmd,
        Some(ManualCmd::Pause)
    ));
    assert!(app.shared.lock().unwrap().manual_pause_result.is_none());
    click(&ctx, &mut app, "自动暂停");
    assert!(!app.config.lock().unwrap().strategy.enabled);
    assert!(app.dirty);
}

#[test]
fn navigation_and_custom_game_submission_use_the_live_form() {
    let (ctx, mut app) = fixture();
    click(&ctx, &mut app, "账户");
    assert!(app.page == Page::Account);
    click(&ctx, &mut app, "登录并保存");
    assert_eq!(app.status_msg, "请输入手机号"); // no credential or network request
    click(&ctx, &mut app, "首页与游戏");
    click(&ctx, &mut app, "＋ 添加游戏");
    assert!(app.show_add_game);
    app.new_name = "我的自定义游戏".into();
    app.new_exe = "custom-game.exe".into();
    click(&ctx, &mut app, "添加到名单");
    assert!(!app.show_add_game);
    let config = app.config.lock().unwrap();
    assert_eq!(config.games.len(), 4);
    assert_eq!(config.games[3].exe, "custom-game.exe");
}

#[test]
fn game_menu_and_update_source_preserve_existing_actions() {
    let (ctx, mut app) = fixture();
    click(&ctx, &mut app, "   ");
    click(&ctx, &mut app, "从名单移除");
    assert_eq!(app.config.lock().unwrap().games[0].exe, "TslGame.exe");
    click(&ctx, &mut app, "关于与更新");
    click(&ctx, &mut app, "自动选择（国内优先）");
    click(&ctx, &mut app, "仅 Gitee（国内）");
    assert_eq!(app.config.lock().unwrap().updates.source, UpdateMode::Gitee);
    assert!(
        !app.update_busy,
        "source changes must not start network requests"
    );
    click(&ctx, &mut app, "启动时自动检查更新");
    assert!(app.config.lock().unwrap().updates.check_on_startup);
}

#[test]
fn all_pages_render_at_minimum_and_standard_window_sizes() {
    let (ctx, mut app) = fixture();
    for size in [[680.0, 460.0], [940.0, 660.0], [1180.0, 780.0]] {
        for page in [
            Page::Games,
            Page::Account,
            Page::Strategy,
            Page::Logs,
            Page::Updates,
        ] {
            app.page = page;
            for _ in 0..2 {
                assert!(!frame(&ctx, &mut app, size, vec![]).shapes.is_empty());
            }
        }
    }
}

// Controlled responses exercise the live completion and rendering paths without
// reading account files or connecting to the Leigod service.
fn pending_account_query(app: &mut App) -> mpsc::Sender<Result<serde_json::Value, api::ApiError>> {
    app.page = Page::Account;
    app.shared.lock().unwrap().token = Some("fixture-account-token".into());
    let (sender, events) = mpsc::channel();
    app.account_query = Some(AccountQuery {
        events,
        started_at: Instant::now(),
        token: "fixture-account-token".into(),
        user: "demo".into(),
    });
    app.account_query_message = "正在查询账户信息…（最多等待 15 秒）".into();
    sender
}

#[test]
fn account_query_success_replaces_pending_text_and_preserves_other_actions() {
    let (ctx, mut app) = fixture();
    let sender = pending_account_query(&mut app);
    app.status_msg = "暂停指令已发送".into();
    app.shared.lock().unwrap().manual_cmd = Some(ManualCmd::Pause);
    sender
        .send(Ok(serde_json::json!({"data": {"pause_status_id": 1}})))
        .unwrap();
    app.poll_account_query(Instant::now());

    assert!(app.account_query.is_none());
    assert!(!app.account_query_error);
    assert!(app.account_query_message.starts_with("账户状态已刷新（"));
    assert!(!app.account_query_message.contains("正在查询"));
    assert_eq!(
        app.shared.lock().unwrap().account_status,
        "已登录（demo）· 已暂停"
    );
    assert_eq!(app.status_msg, "暂停指令已发送");
    assert!(matches!(
        app.shared.lock().unwrap().manual_cmd,
        Some(ManualCmd::Pause)
    ));
    let _ = frame(&ctx, &mut app, [1180.0, 780.0], vec![]);
    let output = frame(&ctx, &mut app, [1180.0, 780.0], vec![]);
    text_rect(&output.shapes, &app.account_query_message);
    text_rect(&output.shapes, "⏸ 计时状态：已暂停");
}

#[test]
fn account_query_errors_end_wait_and_remove_stale_success() {
    for (error, expected) in [
        ("网络请求失败", "查询失败"),
        ("服务器返回错误 code=400006: 未登录", "请重新登录"),
    ] {
        let (_, mut app) = fixture();
        let sender = pending_account_query(&mut app);
        app.shared.lock().unwrap().account_info =
            Some(serde_json::json!({"data": {"pause_status_id": 1}}));
        sender.send(Err(api::ApiError(error.into()))).unwrap();
        app.poll_account_query(Instant::now());
        assert!(app.account_query.is_none());
        assert!(app.account_query_error);
        assert!(app.account_query_message.contains(expected));
        assert!(!app.account_query_message.contains("正在查询"));
        assert!(app.shared.lock().unwrap().account_info.is_none());
        assert!(!app.shared.lock().unwrap().account_status.contains("已暂停"));
    }
}

#[test]
fn account_query_timeout_allows_retry_and_discards_late_response() {
    let (_, mut app) = fixture();
    let old_sender = pending_account_query(&mut app);
    let deadline = app.account_query.as_ref().unwrap().started_at + ACCOUNT_QUERY_TIMEOUT;
    app.poll_account_query(deadline - Duration::from_millis(1));
    assert!(app.account_query.is_some());
    app.poll_account_query(deadline);
    assert!(app.account_query.is_none());
    assert!(app.account_query_message.contains("查询超时"));
    let sender = pending_account_query(&mut app);
    assert!(old_sender
        .send(Ok(serde_json::json!({"data": {"pause_status_id": 1}})))
        .is_err());
    sender
        .send(Ok(serde_json::json!({"data": {"pause_status_id": 0}})))
        .unwrap();
    app.poll_account_query(Instant::now());
    assert!(!app.account_query_error);
    assert_eq!(
        app.shared.lock().unwrap().account_status,
        "已登录（demo）· 计时中"
    );
}

#[test]
fn account_query_disconnected_worker_does_not_leave_a_spinner() {
    let (_, mut app) = fixture();
    drop(pending_account_query(&mut app));
    app.poll_account_query(Instant::now());
    assert!(app.account_query.is_none());
    assert!(app.account_query_error);
    assert!(app.account_query_message.contains("查询意外中断"));
}

#[test]
fn account_query_logout_and_token_replacement_ignore_old_results() {
    let (ctx, mut app) = fixture();
    let sender = pending_account_query(&mut app);
    click(&ctx, &mut app, "退出登录");
    assert!(app.account_query.is_none());
    assert!(app.account_query_message.is_empty());
    assert!(sender
        .send(Ok(serde_json::json!({"data": {"pause_status_id": 1}})))
        .is_err());
    assert!(app.shared.lock().unwrap().token.is_none());

    let sender = pending_account_query(&mut app);
    {
        let mut shared = app.shared.lock().unwrap();
        shared.token = Some("new-fixture-account-token".into());
        shared.account_status = "新账户".into();
    }
    sender
        .send(Ok(serde_json::json!({"data": {"pause_status_id": 1}})))
        .unwrap();
    app.poll_account_query(Instant::now());
    assert!(app.account_query.is_none());
    assert_eq!(app.shared.lock().unwrap().account_status, "新账户");
    assert!(app.shared.lock().unwrap().account_info.is_none());
}

#[test]
fn account_query_runs_off_ui_thread_and_blocks_duplicate_refreshes() {
    let (ctx, mut app) = fixture();
    app.page = Page::Account;
    app.shared.lock().unwrap().token = Some("fixture-account-token".into());
    let ui_thread = std::thread::current().id();
    let (started, started_rx) = mpsc::channel();
    let (release, release_rx) = mpsc::channel();
    app.start_account_query(move |_| {
        started.send(std::thread::current().id()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        Ok(serde_json::json!({"data": {"pause_status_id": 1}}))
    });
    assert_ne!(
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
        ui_thread
    );
    app.start_account_query(|_| panic!("duplicate request must not execute"));
    click(&ctx, &mut app, "刷新账户状态"); // disabled; must not reach the real API
    app.poll_account_query(Instant::now());
    assert!(app.account_query.is_some());
    assert!(app.account_query_message.contains("正在查询"));
    click(&ctx, &mut app, "首页与游戏"); // navigation remains responsive
    assert!(app.page == Page::Games);
    release.send(()).unwrap();
}

#[test]
fn account_query_without_login_or_known_timer_state_is_not_success() {
    let (ctx, mut app) = fixture();
    app.page = Page::Account;
    click(&ctx, &mut app, "刷新账户状态");
    assert!(app.account_query.is_none());
    assert!(app.account_query_message.contains("尚未登录"));
    let sender = pending_account_query(&mut app);
    sender.send(Ok(serde_json::json!({"data": {}}))).unwrap();
    app.poll_account_query(Instant::now());
    assert!(app.account_query_error);
    assert_eq!(
        app.shared.lock().unwrap().account_status,
        "已登录（demo）· 计时状态未知"
    );
    let _ = frame(&ctx, &mut app, [1180.0, 780.0], vec![]);
    let output = frame(&ctx, &mut app, [1180.0, 780.0], vec![]);
    text_rect(&output.shapes, "计时状态：未知，请在小程序刷新核对");
}

fn set_demo_balance(app: &mut App, seconds: u64) {
    let mut shared = app.shared.lock().unwrap();
    shared.set_token(Some("fixture-account-token".into()));
    shared.set_account_info(
        "fixture-account-token",
        serde_json::json!({"data": {"pause_status_id": 1, "expiry_time_samp": seconds, "experience_time": 0}}),
    );
}

#[test]
fn home_balance_and_protection_countdown_are_independent_and_fit_small_windows() {
    let (ctx, mut app) = fixture();
    set_demo_balance(&mut app, 128 * 3600 + 42 * 60);
    for size in [[680.0, 460.0], [940.0, 660.0], [1180.0, 780.0]] {
        let _ = frame(&ctx, &mut app, size, vec![]);
        let output = frame(&ctx, &mut app, size, vec![]);
        let balance = text_rect(&output.shapes, "128 小时 42 分钟");
        let countdown = text_rect(&output.shapes, "02:36");
        assert!(!balance.intersects(countdown));
        text_rect(&output.shapes, "账户剩余时长");
        text_rect(&output.shapes, "刷新时长");
    }
    set_demo_balance(&mut app, 999_999 * 3600 + 59 * 60);
    for _ in 0..2 {
        frame(&ctx, &mut app, [680.0, 460.0], vec![]);
    }
}

#[test]
fn home_balance_refresh_keeps_old_label_then_handles_success_failure_and_zero() {
    let (ctx, mut app) = fixture();
    set_demo_balance(&mut app, 3600);
    let sender = pending_account_query(&mut app);
    app.page = Page::Games;
    let _ = frame(&ctx, &mut app, [1180.0, 780.0], vec![]);
    let output = frame(&ctx, &mut app, [1180.0, 780.0], vec![]);
    text_rect(&output.shapes, "1 小时 00 分钟");
    text_rect(&output.shapes, "上次结果 · 正在刷新…");
    click(&ctx, &mut app, "刷新时长"); // disabled: must not launch a real query
    assert!(app.account_query.is_some());
    sender
        .send(Ok(
            serde_json::json!({"data": {"pause_status_id": 1, "expiry_time_samp": 0}}),
        ))
        .unwrap();
    app.poll_account_query(Instant::now());
    let output = frame(&ctx, &mut app, [1180.0, 780.0], vec![]);
    text_rect(&output.shapes, "0 小时 00 分钟");
    assert!(app.shared.lock().unwrap().account_info_updated_at.is_some());

    let sender = pending_account_query(&mut app);
    app.page = Page::Games;
    sender
        .send(Err(api::ApiError("网络请求失败".into())))
        .unwrap();
    app.poll_account_query(Instant::now());
    let output = frame(&ctx, &mut app, [1180.0, 780.0], vec![]);
    text_rect(&output.shapes, "暂不可用");
    text_rect(&output.shapes, "查询失败，请重试或重新登录");
    assert!(app.shared.lock().unwrap().account_info_updated_at.is_none());
    app.shared.lock().unwrap().set_token(None);
    let output = frame(&ctx, &mut app, [1180.0, 780.0], vec![]);
    text_rect(&output.shapes, "登录后查看");
    click(&ctx, &mut app, "去登录");
    assert!(app.page == Page::Account);
}

#[test]
fn automatic_home_queries_are_focus_gated_freshness_checked_and_rate_limited() {
    let (_, mut app) = fixture();
    let now = Instant::now();
    app.next_home_refresh = now;
    assert!(!app.home_refresh_due(now, true));
    app.shared
        .lock()
        .unwrap()
        .set_token(Some("fixture-account-token".into()));
    assert!(app.home_refresh_due(now, true));
    assert!(!app.home_refresh_due(now, false));
    app.page = Page::Account;
    assert!(!app.home_refresh_due(now, true));
    app.page = Page::Games;
    set_demo_balance(&mut app, 60);
    assert!(!app.home_refresh_due(Instant::now(), true));
    let later = Instant::now() + HOME_REFRESH_INTERVAL;
    assert!(app.home_refresh_due(later, true));
    app.next_home_refresh = later + HOME_REFRESH_INTERVAL;
    assert!(!app.home_refresh_due(later, true));
    app.shared
        .lock()
        .unwrap()
        .set_token(Some("different-fixture-token".into()));
    let shared = app.shared.lock().unwrap();
    assert!(shared.account_info.is_none());
    assert!(shared.account_info_updated_at.is_none());
}

struct Offscreen {
    device: eframe::wgpu::Device,
    queue: eframe::wgpu::Queue,
}

impl Offscreen {
    fn new() -> Self {
        use eframe::wgpu;
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::DX12,
            ..Default::default()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            force_fallback_adapter: true,
            compatible_surface: None,
        }))
        .expect("DirectX offscreen adapter required; no native window is created");
        println!("Offscreen adapter: {}", adapter.get_info().name);
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default(), None))
                .unwrap();
        Self { device, queue }
    }

    fn save(
        &self,
        ctx: &egui::Context,
        app: &mut App,
        size: [f32; 2],
        scale: f32,
        path: &std::path::Path,
    ) {
        use eframe::{egui_wgpu, wgpu};
        ctx.set_pixels_per_point(scale);
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let mut renderer = egui_wgpu::Renderer::new(&self.device, format, None, 1, false);
        let mut output = None;
        for _ in 0..3 {
            let current = frame(ctx, app, size, vec![]);
            for (id, delta) in &current.textures_delta.set {
                renderer.update_texture(&self.device, &self.queue, *id, delta);
            }
            for id in &current.textures_delta.free {
                renderer.free_texture(id);
            }
            output = Some(current);
        }
        let output = output.unwrap();
        let scale = output.pixels_per_point;
        let width = (size[0] * scale).round() as u32;
        let height = (size[1] * scale).round() as u32;
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen native UI"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let jobs = ctx.tessellate(output.shapes, scale);
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [width, height],
            pixels_per_point: scale,
        };
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let commands =
            renderer.update_buffers(&self.device, &self.queue, &mut encoder, &jobs, &screen);
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: None,
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                })
                .forget_lifetime();
            renderer.render(&mut pass, &jobs, &screen);
        }
        let stride = (width * 4).div_ceil(256) * 256;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (stride * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.queue
            .submit(commands.into_iter().chain(Some(encoder.finish())));
        let (sender, receiver) = std::sync::mpsc::channel();
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
        self.device.poll(wgpu::Maintain::Wait);
        receiver.recv().unwrap().unwrap();
        let mapped = buffer.slice(..).get_mapped_range();
        let bytes: Vec<u8> = mapped
            .chunks(stride as usize)
            .flat_map(|row| row[..width as usize * 4].iter().copied())
            .collect();
        image::save_buffer(path, &bytes, width, height, image::ColorType::Rgba8).unwrap();
        drop(mapped);
        buffer.unmap();
        println!("Rendered {}x{}: {}", width, height, path.display());
    }
}

#[test]
#[ignore = "explicit offscreen screenshot generation; no account, file configuration, native window or monitor"]
fn render_apple_preview() {
    let output = std::env::var_os("LEIGOD_UI_PREVIEW_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("target/ui-preview"));
    std::fs::create_dir_all(&output).unwrap();
    let gpu = Offscreen::new();
    for (name, page, size, scale) in [
        ("home", Page::Games, [1180.0, 780.0], 1.0),
        ("home-narrow", Page::Games, [680.0, 460.0], 1.0),
        ("home-hidpi", Page::Games, [1180.0, 780.0], 1.5),
        ("account", Page::Account, [1180.0, 780.0], 1.0),
        ("account-narrow", Page::Account, [680.0, 460.0], 1.0),
        ("strategy", Page::Strategy, [1180.0, 780.0], 1.0),
        ("strategy-narrow", Page::Strategy, [680.0, 460.0], 1.0),
        ("updates", Page::Updates, [1180.0, 780.0], 1.0),
        ("updates-narrow", Page::Updates, [680.0, 460.0], 1.0),
        ("logs", Page::Logs, [1180.0, 780.0], 1.0),
    ] {
        let (ctx, mut app) = fixture();
        app.page = page;
        if page == Page::Games {
            set_demo_balance(&mut app, 128 * 3600 + 42 * 60);
        }
        gpu.save(
            &ctx,
            &mut app,
            size,
            scale,
            &output.join(format!("{name}.png")),
        );
    }
    for state in ["logged-out", "pending", "unavailable", "zero"] {
        let (ctx, mut app) = fixture();
        let _pending = if state == "pending" {
            set_demo_balance(&mut app, 128 * 3600 + 42 * 60);
            Some(pending_account_query(&mut app))
        } else {
            None
        };
        if state == "zero" {
            set_demo_balance(&mut app, 0);
        } else if state == "unavailable" {
            app.shared
                .lock()
                .unwrap()
                .set_token(Some("fixture-account-token".into()));
            app.account_query_error = true;
        }
        app.page = Page::Games;
        gpu.save(
            &ctx,
            &mut app,
            [1180.0, 780.0],
            1.0,
            &output.join(format!("home-{state}.png")),
        );
    }
    for state in ["pending", "success", "failure"] {
        for (suffix, size) in [("", [1180.0, 780.0]), ("-narrow", [680.0, 460.0])] {
            let (ctx, mut app) = fixture();
            let sender = pending_account_query(&mut app);
            app.shared.lock().unwrap().account_info =
                Some(serde_json::json!({"data": {"pause_status_id": 1}}));
            if state == "success" {
                sender
                    .send(Ok(serde_json::json!({"data": {"pause_status_id": 1}})))
                    .unwrap();
                app.poll_account_query(Instant::now());
            } else if state == "failure" {
                sender
                    .send(Err(api::ApiError(
                        "查询超时，请检查网络后重新刷新。".into(),
                    )))
                    .unwrap();
                app.poll_account_query(Instant::now());
            }
            gpu.save(
                &ctx,
                &mut app,
                size,
                1.0,
                &output.join(format!("account-{state}{suffix}.png")),
            );
        }
    }
    let (ctx, mut app) = fixture();
    app.show_add_game = true;
    gpu.save(
        &ctx,
        &mut app,
        [1180.0, 780.0],
        1.0,
        &output.join("add-game.png"),
    );
}
