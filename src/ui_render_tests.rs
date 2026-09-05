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
                Some(Rect::from_min_size(text.pos, text.galley.size()))
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
    click(&ctx, &mut app, "GitHub");
    click(&ctx, &mut app, "Gitee（国内）");
    assert_eq!(
        app.config.lock().unwrap().updates.source,
        UpdateSource::Gitee
    );
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
        gpu.save(
            &ctx,
            &mut app,
            size,
            scale,
            &output.join(format!("{name}.png")),
        );
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
