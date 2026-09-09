//! Shared native design tokens and controls for the approved light interface.
use egui::{pos2, vec2, Color32, FontFamily, FontId, Rect, Response, RichText, Sense, Stroke, Ui};

pub const BACKGROUND: Color32 = Color32::from_rgb(249, 250, 255);
pub const TEXT: Color32 = Color32::from_rgb(23, 37, 65);
pub const MUTED: Color32 = Color32::from_rgb(103, 119, 153);
pub const BORDER: Color32 = Color32::from_rgb(216, 222, 240);
pub const BLUE: Color32 = Color32::from_rgb(79, 94, 255);
pub const TEAL: Color32 = BLUE;
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
    style.visuals.selection.bg_fill = Color32::from_rgb(234, 235, 252);
    style.visuals.selection.stroke = Stroke::new(1.0_f32, BLUE);
    style.visuals.window_corner_radius = 10.into();
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
        widget.corner_radius = 8.into();
        widget.expansion = 0.0;
        widget.fg_stroke = Stroke::new(1.3_f32, TEXT);
        widget.bg_stroke = Stroke::new(1.0_f32, BORDER);
        widget.bg_fill = Color32::from_rgb(252, 252, 255);
        widget.weak_bg_fill = glass_tint(90);
    }
    style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(238, 240, 255);
    style.visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(238, 240, 255);
    style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, BLUE);
    style.visuals.widgets.active.bg_fill = Color32::from_rgb(230, 233, 255);
    style.visuals.widgets.active.weak_bg_fill = Color32::from_rgb(230, 233, 255);
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
        .fill(glass_tint(125))
        .stroke(Stroke::new(1.0_f32, BORDER))
        .corner_radius(10)
        .inner_margin(22)
        .shadow(egui::epaint::Shadow {
            offset: [0, 3],
            blur: 12,
            spread: 0,
            color: Color32::from_rgba_unmultiplied(52, 63, 115, 5),
        })
}

/// A real Gaussian blur of an app-owned color field, uploaded once per App.
/// Glass surfaces reveal this cached texture. No desktop capture, per-frame
/// convolution or animated backdrop competes with games for GPU time.
pub fn glass_backdrop(ctx: &egui::Context) -> egui::TextureHandle {
    let mut pixels = image::RgbaImage::new(384, 256);
    for (x, y, pixel) in pixels.enumerate_pixels_mut() {
        let x = x as f32 / 384.0;
        let y = y as f32 / 256.0;
        let tint = y.powf(1.3) * (0.9 + 0.1 * (1.0 - x));
        *pixel = image::Rgba([
            (251.0 - 16.0 * tint) as u8,
            (252.0 - 15.0 * tint) as u8,
            255,
            255,
        ]);
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
    ui.add(outline_button(text, true).min_size(vec2(0.0, 40.0)))
}

pub fn outline_button(text: &str, primary: bool) -> egui::Button<'_> {
    let color = if primary { BLUE } else { MUTED };
    egui::Button::new(RichText::new(text).color(color))
        .fill(glass_tint(65))
        .stroke(Stroke::new(1.0_f32, if primary { BLUE } else { BORDER }))
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
    let (rect, mut response) = ui.allocate_exact_size(vec2(46.0, 30.0), Sense::click());
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

/// A complete settings row: wrapping field name on the left, switch on the right.
pub fn switch_row(ui: &mut Ui, value: &mut bool, label: &str) -> Response {
    let old = *value;
    let width = ui.available_width();
    let mut response = ui
        .horizontal_top(|ui| {
            let label_response = ui
                .allocate_ui_with_layout(
                    vec2((width - 62.0).max(80.0), 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_min_width((width - 62.0).max(80.0));
                        ui.add_space(4.0);
                        ui.add(egui::Label::new(title(label, 15.0)).sense(Sense::click()))
                    },
                )
                .inner;
            if label_response.clicked() {
                *value = !*value;
            }
            toggle(ui, value, label).union(label_response)
        })
        .inner;
    if *value != old {
        response.mark_changed();
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
            line(&[(2., 11.), (12., 2.), (22., 11.)]);
            line(&[
                (5., 9.),
                (5., 22.),
                (10., 22.),
                (10., 15.),
                (14., 15.),
                (14., 22.),
                (19., 22.),
                (19., 9.),
            ]);
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
        Icon::Info => {
            p.circle_stroke(at(12., 12.), rect.width() * 0.44, stroke);
            p.circle_filled(at(12., 7.), rect.width() * 0.055, color);
            line(&[(12., 11.), (12., 18.)]);
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
        Color32::from_rgb(236, 238, 250)
    } else if response.hovered() {
        Color32::from_white_alpha(155)
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, 9, fill);
    if selected {
        ui.painter().rect_filled(
            Rect::from_min_max(
                rect.left_top() + vec2(-4.0, 5.0),
                rect.left_bottom() + vec2(-1.0, -5.0),
            ),
            2,
            BLUE,
        );
    }
    let color = TEXT;
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
        if selected {
            heading_font(16.0)
        } else {
            FontId::proportional(16.0)
        },
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
        Stroke::new(1.0_f32, BORDER),
    );
}
