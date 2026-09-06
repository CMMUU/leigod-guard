//! Shared native design tokens and controls for the approved light interface.
use egui::{pos2, vec2, Color32, FontFamily, FontId, Rect, Response, RichText, Sense, Stroke, Ui};

pub const BACKGROUND: Color32 = Color32::from_rgb(245, 248, 253);
pub const TEXT: Color32 = Color32::from_rgb(23, 27, 37);
pub const MUTED: Color32 = Color32::from_rgb(99, 108, 125);
pub const BORDER: Color32 = Color32::from_rgb(229, 232, 237);
pub const BLUE: Color32 = Color32::from_rgb(0, 122, 255);
pub const TEAL: Color32 = Color32::from_rgb(44, 201, 173);
pub const GREEN: Color32 = Color32::from_rgb(35, 173, 113);
pub const AMBER: Color32 = Color32::from_rgb(174, 112, 26);

/// eframe's Unorm wgpu target blends in gamma space. Premultiply the white
/// tint in that same space so layered glass does not saturate to solid white.
pub fn glass_tint(alpha: u8) -> Color32 {
    Color32::from_rgba_premultiplied(alpha, alpha, alpha, alpha)
}

pub fn install(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Light);
    let mut style = (*ctx.style()).clone();
    style
        .text_styles
        .insert(egui::TextStyle::Heading, heading_font(20.0));
    style
        .text_styles
        .insert(egui::TextStyle::Body, FontId::proportional(15.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, FontId::proportional(15.0));
    style
        .text_styles
        .insert(egui::TextStyle::Small, FontId::proportional(12.0));
    style
        .text_styles
        .insert(egui::TextStyle::Monospace, FontId::monospace(13.0));
    style.spacing.item_spacing = vec2(10.0, 10.0);
    style.spacing.button_padding = vec2(14.0, 9.0);
    style.spacing.interact_size.y = 34.0;
    style.spacing.combo_width = 230.0;
    style.spacing.text_edit_width = 300.0;
    style.visuals = egui::Visuals::light();
    style.visuals.panel_fill = BACKGROUND;
    style.visuals.window_fill = Color32::WHITE;
    style.visuals.extreme_bg_color = Color32::WHITE;
    style.visuals.faint_bg_color = Color32::from_rgb(244, 246, 250);
    style.visuals.hyperlink_color = BLUE;
    style.visuals.selection.bg_fill = Color32::from_rgb(222, 236, 255);
    style.visuals.selection.stroke = Stroke::new(1.0_f32, BLUE);
    style.visuals.window_corner_radius = 16.into();
    style.visuals.menu_corner_radius = 10.into();
    style.visuals.window_stroke = Stroke::new(1.0_f32, BORDER);
    style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    for widget in [
        &mut style.visuals.widgets.inactive,
        &mut style.visuals.widgets.hovered,
        &mut style.visuals.widgets.active,
        &mut style.visuals.widgets.open,
    ] {
        widget.corner_radius = 9.into();
        widget.expansion = 0.0;
        widget.fg_stroke = Stroke::new(1.3_f32, TEXT);
        widget.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(211, 216, 225));
        widget.bg_fill = Color32::WHITE;
        widget.weak_bg_fill = Color32::WHITE;
    }
    style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(238, 244, 255);
    style.visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(238, 244, 255);
    style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, BLUE);
    style.visuals.widgets.active.bg_fill = Color32::from_rgb(225, 237, 255);
    style.visuals.widgets.active.weak_bg_fill = Color32::from_rgb(225, 237, 255);
    ctx.set_style(style);
}

pub fn heading_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("heading".into()))
}

pub fn title(text: impl Into<String>, size: f32) -> RichText {
    RichText::new(text).font(heading_font(size)).color(TEXT)
}

pub fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(glass_tint(180))
        .stroke(Stroke::new(1.0_f32, glass_tint(240)))
        .corner_radius(18)
        .inner_margin(22)
        .shadow(egui::epaint::Shadow {
            offset: [0, 4],
            blur: 18,
            spread: 0,
            color: Color32::from_rgba_unmultiplied(48, 73, 112, 15),
        })
}

/// A real Gaussian blur of an app-owned color field, uploaded once per App.
/// Glass surfaces reveal this cached texture. No desktop capture, per-frame
/// convolution or animated backdrop competes with games for GPU time.
pub fn glass_backdrop(ctx: &egui::Context) -> egui::TextureHandle {
    let mut pixels = image::RgbaImage::from_pixel(384, 256, image::Rgba([245, 248, 253, 255]));
    for (x, y, pixel) in pixels.enumerate_pixels_mut() {
        let x = x as f32 / 384.0;
        let y = y as f32 / 256.0;
        for (cx, cy, rx, ry, color) in [
            (0.28, 0.18, 0.34, 0.26, [187, 215, 249]),
            (0.87, 0.29, 0.29, 0.32, [175, 228, 216]),
            (0.38, 0.92, 0.39, 0.30, [220, 205, 241]),
        ] {
            if ((x - cx) / rx).powi(2) + ((y - cy) / ry).powi(2) < 1.0 {
                *pixel = image::Rgba([color[0], color[1], color[2], 255]);
            }
        }
    }
    let blurred = image::imageops::blur(&pixels, 20.0);
    ctx.load_texture(
        "glass-backdrop",
        egui::ColorImage::from_rgba_unmultiplied([384, 256], blurred.as_raw()),
        egui::TextureOptions::LINEAR,
    )
}

pub fn paint_backdrop(ui: &Ui, texture: &egui::TextureHandle, rect: Rect) {
    let screen = ui.ctx().screen_rect();
    let uv = Rect::from_min_max(
        pos2(
            (rect.left() - screen.left()) / screen.width(),
            (rect.top() - screen.top()) / screen.height(),
        ),
        pos2(
            (rect.right() - screen.left()) / screen.width(),
            (rect.bottom() - screen.top()) / screen.height(),
        ),
    );
    ui.painter()
        .with_clip_rect(rect)
        .image(texture.id(), rect, uv, Color32::WHITE);
}

pub fn primary(ui: &mut Ui, text: &str) -> Response {
    ui.add(
        egui::Button::new(RichText::new(text).color(Color32::WHITE))
            .fill(if ui.is_enabled() {
                BLUE
            } else {
                Color32::from_rgb(167, 193, 229)
            })
            .stroke(Stroke::NONE)
            .min_size(vec2(0.0, 40.0)),
    )
}

pub fn badge(ui: &mut Ui, text: &str, color: Color32) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.08))
        .corner_radius(12)
        .inner_margin(egui::Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(12.0).color(color));
        });
}

pub fn toggle(ui: &mut Ui, value: &mut bool, label: &str) -> Response {
    let text = ui
        .painter()
        .layout_no_wrap(label.to_string(), FontId::proportional(14.0), TEXT);
    let (rect, mut response) =
        ui.allocate_exact_size(vec2(text.size().x + 64.0, 30.0), Sense::click());
    if response.clicked() {
        *value = !*value;
        response.mark_changed();
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), *value, label)
    });
    let switch =
        Rect::from_center_size(pos2(rect.right() - 23.0, rect.center().y), vec2(46.0, 26.0));
    let color = if *value {
        BLUE
    } else {
        Color32::from_rgb(201, 206, 214)
    };
    ui.painter().rect_filled(switch, 13, color);
    let x = if *value {
        switch.right() - 13.0
    } else {
        switch.left() + 13.0
    };
    ui.painter()
        .circle_filled(pos2(x, switch.center().y), 10.5, Color32::WHITE);
    ui.painter().galley(
        pos2(rect.left(), rect.center().y - text.size().y / 2.0),
        text,
        TEXT,
    );
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect.expand(3.0),
            6,
            Stroke::new(1.5_f32, BLUE),
            egui::StrokeKind::Inside,
        );
    }
    response
}

#[derive(Clone, Copy)]
pub enum Icon {
    Home,
    Account,
    Shield,
    Logs,
    Info,
    Clock,
    Exit,
    More,
}

pub fn icon(ui: &Ui, kind: Icon, rect: Rect, color: Color32) {
    let p = ui.painter();
    let at = |x: f32, y: f32| {
        pos2(
            rect.left() + rect.width() * x / 24.0,
            rect.top() + rect.height() * y / 24.0,
        )
    };
    let stroke = Stroke::new(1.6_f32, color);
    let line = |points: &[(f32, f32)]| {
        p.add(egui::Shape::line(
            points.iter().map(|(x, y)| at(*x, *y)).collect(),
            stroke,
        ));
    };
    match kind {
        Icon::Home => {
            p.add(egui::Shape::convex_polygon(
                vec![
                    at(2., 11.),
                    at(12., 2.),
                    at(22., 11.),
                    at(19., 11.),
                    at(19., 22.),
                    at(14., 22.),
                    at(14., 15.),
                    at(10., 15.),
                    at(10., 22.),
                    at(5., 22.),
                    at(5., 11.),
                ],
                color,
                Stroke::NONE,
            ));
        }
        Icon::Account => {
            p.circle_stroke(at(12., 7.), rect.width() * 0.2, stroke);
            line(&[
                (7., 12.),
                (3., 16.),
                (3., 22.),
                (21., 22.),
                (21., 16.),
                (17., 12.),
            ]);
        }
        Icon::Shield => {
            line(&[
                (12., 2.),
                (21., 6.),
                (20., 15.),
                (17., 19.),
                (12., 23.),
                (7., 19.),
                (4., 15.),
                (3., 6.),
                (12., 2.),
            ]);
            line(&[(12., 7.), (12., 16.)]);
            line(&[(8., 12.), (12., 16.), (16., 12.)]);
        }
        Icon::Logs => {
            p.rect_stroke(
                Rect::from_min_max(at(4., 2.), at(20., 22.)),
                3,
                stroke,
                egui::StrokeKind::Inside,
            );
            for (y, end) in [(7., 16.), (11., 16.), (15., 12.)] {
                line(&[(8., y), (end, y)]);
            }
        }
        Icon::Clock => {
            p.circle_stroke(at(12., 12.), rect.width() * 0.44, stroke);
            line(&[(12., 5.), (12., 12.), (17., 15.)]);
        }
        Icon::Info => {
            p.circle_stroke(at(12., 12.), rect.width() * 0.44, stroke);
            p.circle_filled(at(12., 7.), rect.width() * 0.055, color);
            line(&[(12., 11.), (12., 18.)]);
        }
        Icon::Exit => {
            line(&[(15., 3.), (4., 3.), (4., 21.), (15., 21.)]);
            line(&[(10., 12.), (23., 12.), (18., 7.)]);
            line(&[(23., 12.), (18., 17.)]);
        }
        Icon::More => {
            for x in [4., 12., 20.] {
                p.circle_filled(at(x, 12.), rect.width() * 0.075, color);
            }
        }
    }
}

pub fn navigation(ui: &mut Ui, kind: Icon, label: &str, selected: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            ui.is_enabled(),
            selected,
            label,
        )
    });
    let fill = if selected {
        Color32::from_rgb(222, 233, 248)
    } else if response.hovered() {
        Color32::from_white_alpha(155)
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, 11, fill);
    let color = if selected { BLUE } else { TEXT };
    icon(
        ui,
        kind,
        Rect::from_center_size(pos2(rect.left() + 26.0, rect.center().y), vec2(21.0, 21.0)),
        color,
    );
    ui.painter().text(
        pos2(rect.left() + 50.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        FontId::proportional(15.0),
        color,
    );
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect,
            11,
            Stroke::new(1.0_f32, BLUE),
            egui::StrokeKind::Inside,
        );
    }
    response
}

pub fn sidebar_background(ui: &Ui) {
    let r = ui.max_rect();
    ui.painter().rect_filled(r, 0, glass_tint(95));
    ui.painter().line_segment(
        [r.right_top(), r.right_bottom()],
        Stroke::new(1.0_f32, glass_tint(190)),
    );
}
