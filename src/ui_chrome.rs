//! App-owned light window chrome. Rendering only emits viewport commands;
//! native hide-to-tray is applied by App after rendering, never by UI fixtures.
use crate::ui_theme::{self as theme, Icon};
use egui::{pos2, vec2, Align2, Context, Rect, ResizeDirection, Sense, Stroke, ViewportCommand};

pub fn render(ctx: &Context, backdrop: &egui::TextureHandle) -> bool {
    let mut hide = false;
    let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
    egui::TopBottomPanel::top("title-bar")
        .exact_height(50.0)
        .frame(egui::Frame::new())
        .show(ctx, |ui| {
            let r = ui.max_rect();
            theme::paint_backdrop(ui, backdrop, r);
            ui.painter().line_segment(
                [r.left_bottom(), r.right_bottom()],
                Stroke::new(1.0_f32, theme::BORDER),
            );
            let drag_rect =
                Rect::from_min_max(r.min + vec2(5.0, 5.0), r.right_bottom() - vec2(155.0, 1.0));
            let drag = ui.interact(drag_rect, ui.id().with("drag"), Sense::click_and_drag());
            if drag.double_clicked() {
                ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
            } else if drag.drag_started_by(egui::PointerButton::Primary) {
                ctx.send_viewport_cmd(ViewportCommand::StartDrag);
            }
            theme::icon(
                ui,
                Icon::Shield,
                Rect::from_center_size(pos2(r.left() + 29.0, r.center().y), vec2(21.0, 21.0)),
                theme::TEXT,
            );
            ui.painter().text(
                pos2(r.left() + 51.0, r.center().y),
                Align2::LEFT_CENTER,
                "雷神守护",
                theme::heading_font(17.0),
                theme::TEXT,
            );
            for (index, label) in [
                "最小化",
                if maximized {
                    "还原窗口"
                } else {
                    "最大化"
                },
                "关闭并隐藏到托盘",
            ]
            .into_iter()
            .enumerate()
            {
                let button = Rect::from_min_size(
                    pos2(r.right() - 150.0 + index as f32 * 50.0, r.top() + 5.0),
                    vec2(45.0, 40.0),
                );
                let response = ui.interact(button, ui.id().with(label), Sense::click());
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label)
                });
                if response.hovered() || response.has_focus() {
                    ui.painter().rect_filled(button, 6, theme::glass_tint(210));
                    ui.painter().rect_stroke(
                        button,
                        6,
                        Stroke::new(1.0_f32, theme::BORDER),
                        egui::StrokeKind::Inside,
                    );
                }
                let c = button.center();
                let stroke = Stroke::new(1.3_f32, theme::TEXT);
                match index {
                    0 => {
                        ui.painter()
                            .line_segment([c - vec2(5.0, 0.0), c + vec2(5.0, 0.0)], stroke);
                    }
                    1 => {
                        if maximized {
                            ui.painter().rect_stroke(
                                Rect::from_center_size(c + vec2(2.0, -2.0), vec2(9.0, 9.0)),
                                1,
                                stroke,
                                egui::StrokeKind::Inside,
                            );
                        }
                        ui.painter().rect(
                            Rect::from_center_size(c, vec2(10.0, 10.0)),
                            1,
                            theme::BACKGROUND,
                            stroke,
                            egui::StrokeKind::Inside,
                        );
                    }
                    _ => {
                        ui.painter()
                            .line_segment([c - vec2(5.0, 5.0), c + vec2(5.0, 5.0)], stroke);
                        ui.painter()
                            .line_segment([c + vec2(-5.0, 5.0), c + vec2(5.0, -5.0)], stroke);
                    }
                }
                if response.clicked() {
                    match index {
                        0 => ctx.send_viewport_cmd(ViewportCommand::Minimized(true)),
                        1 => ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized)),
                        _ => hide = true,
                    }
                }
                response.on_hover_text(label);
            }
        });
    egui::TopBottomPanel::bottom("window-footer")
        .exact_height(36.0)
        .frame(egui::Frame::new())
        .show(ctx, |ui| {
            let r = ui.max_rect();
            theme::paint_backdrop(ui, backdrop, r);
            ui.painter().line_segment(
                [r.left_top(), r.right_top()],
                Stroke::new(1.0_f32, theme::BORDER),
            );
            ui.painter().text(
                r.center(),
                Align2::CENTER_CENTER,
                "暂停是否生效，请在雷神官方微信小程序下拉刷新核对。",
                egui::FontId::proportional(12.0),
                theme::MUTED,
            );
        });
    // Native resize gestures for an undecorated window, including its corners.
    if !maximized {
        if let Some((direction, pressed)) = ctx.input(|i| {
            let p = i.pointer.hover_pos()?;
            let r = i.screen_rect;
            let left = p.x < r.left() + 5.0;
            let right = p.x > r.right() - 5.0;
            let top = p.y < r.top() + 5.0;
            let bottom = p.y > r.bottom() - 5.0;
            let direction = match (left, right, top, bottom) {
                (true, _, true, _) => ResizeDirection::NorthWest,
                (_, true, true, _) => ResizeDirection::NorthEast,
                (true, _, _, true) => ResizeDirection::SouthWest,
                (_, true, _, true) => ResizeDirection::SouthEast,
                (true, _, _, _) => ResizeDirection::West,
                (_, true, _, _) => ResizeDirection::East,
                (_, _, true, _) => ResizeDirection::North,
                (_, _, _, true) => ResizeDirection::South,
                _ => return None,
            };
            Some((direction, i.pointer.primary_pressed()))
        }) {
            let cursor = match direction {
                ResizeDirection::North | ResizeDirection::South => egui::CursorIcon::ResizeVertical,
                ResizeDirection::East | ResizeDirection::West => egui::CursorIcon::ResizeHorizontal,
                ResizeDirection::NorthWest | ResizeDirection::SouthEast => {
                    egui::CursorIcon::ResizeNwSe
                }
                _ => egui::CursorIcon::ResizeNeSw,
            };
            ctx.set_cursor_icon(cursor);
            if pressed {
                ctx.send_viewport_cmd(ViewportCommand::BeginResize(direction));
            }
        }
    }
    hide
}
