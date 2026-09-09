//! Scrollable surfaces without a visible track. egui retains its native smooth
//! wheel/touchpad input and bounded offsets; no extra timer or inertia loop.
use crate::ui_theme as theme;
use egui::{pos2, Key, Modifiers, Rect, ScrollArea, Ui};

pub fn show<R>(
    ui: &mut Ui,
    area: ScrollArea,
    keyboard_enabled: bool,
    contents: impl FnOnce(&mut Ui) -> R,
) -> egui::scroll_area::ScrollAreaOutput<R> {
    let output = area
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .show_viewport(ui, |ui, viewport| {
            let visible =
                Rect::from_min_size(ui.max_rect().min + viewport.min.to_vec2(), viewport.size())
                    .intersect(ui.clip_rect());
            // Register beneath children so clicking a control still reaches it.
            let focus = ui.interact(visible, ui.id().with("scroll-focus"), egui::Sense::click());
            if focus.clicked() {
                focus.request_focus();
            }
            let result = contents(ui);
            let overflow = (ui.min_rect().height() - viewport.height()).max(0.0);
            let editing = ui
                .ctx()
                .memory(|m| m.focused())
                .is_some_and(|id| egui::text_edit::TextEditState::load(ui.ctx(), id).is_some());
            if keyboard_enabled
                && ui.is_enabled()
                && overflow > 0.5
                && ui.input(|i| i.focused && !i.modifiers.any())
                && !editing
                && !ui.memory(|m| m.any_popup_open())
                && (focus.has_focus() || ui.rect_contains_pointer(visible))
            {
                let delta = ui.input_mut(|i| {
                    if i.consume_key(Modifiers::NONE, Key::PageDown) {
                        Some(-viewport.height() * 0.85)
                    } else if i.consume_key(Modifiers::NONE, Key::PageUp) {
                        Some(viewport.height() * 0.85)
                    } else if i.consume_key(Modifiers::NONE, Key::Home) {
                        Some(viewport.top())
                    } else if i.consume_key(Modifiers::NONE, Key::End) {
                        Some(viewport.top() - overflow)
                    } else {
                        None
                    }
                });
                if let Some(delta) = delta {
                    // Use the wheel path so a keyboard move also releases a
                    // log area's stick-to-bottom state. Scroll targets alone
                    // do not release that state in egui 0.31.
                    ui.input_mut(|i| i.smooth_scroll_delta.y += delta);
                }
            }
            result
        });
    let (above, below) = edges(
        output.state.offset.y,
        output.content_size.y,
        output.inner_rect.height(),
    );
    let painter = ui
        .painter()
        .with_clip_rect(output.inner_rect.intersect(ui.clip_rect()));
    let height = 10.0_f32.min(output.inner_rect.height() / 3.0);
    for (show, top) in [(above, true), (below, false)] {
        if !show || height <= 0.0 {
            continue;
        }
        let r = output.inner_rect;
        let (y0, y1) = if top {
            (r.top(), r.top() + height)
        } else {
            (r.bottom() - height, r.bottom())
        };
        let (near, far) = (theme::glass_tint(90), egui::Color32::TRANSPARENT);
        let (upper, lower) = if top { (near, far) } else { (far, near) };
        let mut mesh = egui::Mesh::default();
        for (p, color) in [
            (pos2(r.left(), y0), upper),
            (pos2(r.right(), y0), upper),
            (pos2(r.right(), y1), lower),
            (pos2(r.left(), y1), lower),
        ] {
            mesh.colored_vertex(p, color);
        }
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        painter.add(egui::Shape::mesh(mesh));
    }
    output
}

fn edges(offset: f32, content: f32, viewport: f32) -> (bool, bool) {
    let max = (content - viewport).max(0.0);
    (max > 0.5 && offset > 0.5, max > 0.5 && max - offset > 0.5)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{vec2, Context, Event, MouseWheelUnit, Pos2};

    fn key(key: Key) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }
    }

    fn frame(
        ctx: &Context,
        rows: usize,
        sticky: bool,
        edit: bool,
        events: Vec<Event>,
    ) -> egui::scroll_area::ScrollAreaOutput<()> {
        let mut result = None;
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(480.0, 300.0))),
                focused: true,
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    result = Some(show(
                        ui,
                        ScrollArea::vertical()
                            .id_salt("test-scroll")
                            .auto_shrink([false, false])
                            .stick_to_bottom(sticky),
                        true,
                        |ui| {
                            if edit {
                                let mut value = "editable field".to_string();
                                ui.text_edit_singleline(&mut value).request_focus();
                            }
                            for index in 0..rows {
                                ui.label(format!("record {index}"));
                            }
                        },
                    ));
                });
            },
        );
        result.unwrap()
    }

    #[test]
    fn hidden_tracks_keep_full_width_wheel_keyboard_and_correct_edge_hints() {
        let ctx = Context::default();
        let top = frame(&ctx, 60, false, false, vec![]);
        assert!((top.inner_rect.width() - 464.0).abs() < 1.0);
        assert_eq!(
            edges(
                top.state.offset.y,
                top.content_size.y,
                top.inner_rect.height()
            ),
            (false, true)
        );
        let wheel = frame(
            &ctx,
            60,
            false,
            false,
            vec![
                Event::PointerMoved(pos2(220.0, 150.0)),
                Event::MouseWheel {
                    unit: MouseWheelUnit::Point,
                    delta: vec2(0.0, -80.0),
                    modifiers: Modifiers::NONE,
                },
            ],
        );
        assert!(wheel.state.offset.y > 0.0);
        for _ in 0..30 {
            frame(&ctx, 60, false, false, vec![]);
        }
        let before = frame(&ctx, 60, false, false, vec![]).state.offset.y;
        let down = frame(&ctx, 60, false, false, vec![key(Key::PageDown)]);
        assert!(down.state.offset.y > before);
        let up = frame(&ctx, 60, false, false, vec![key(Key::PageUp)]);
        assert!(up.state.offset.y < down.state.offset.y);
        let bottom = frame(&ctx, 60, false, false, vec![key(Key::End)]);
        assert_eq!(
            edges(
                bottom.state.offset.y,
                bottom.content_size.y,
                bottom.inner_rect.height()
            ),
            (true, false)
        );
        let top = frame(&ctx, 60, false, false, vec![key(Key::Home)]);
        assert!(top.state.offset.y < 0.5);
        assert_eq!(edges(0.0, 80.0, 200.0), (false, false));
    }

    #[test]
    fn log_history_stays_put_after_keyboard_or_wheel_scroll_and_can_follow_again() {
        let ctx = Context::default();
        frame(
            &ctx,
            60,
            true,
            false,
            vec![Event::PointerMoved(pos2(220.0, 150.0))],
        );
        let old = frame(&ctx, 60, true, false, vec![]);
        let appended = frame(&ctx, 65, true, false, vec![]);
        assert!(appended.state.offset.y > old.state.offset.y);
        let history = frame(&ctx, 65, true, false, vec![key(Key::PageUp)]);
        assert!(history.state.offset.y < appended.state.offset.y);
        let appended = frame(&ctx, 70, true, false, vec![]);
        assert!((appended.state.offset.y - history.state.offset.y).abs() < 0.5);
        frame(&ctx, 70, true, false, vec![key(Key::End)]);
        let following = frame(&ctx, 75, true, false, vec![]);
        assert_eq!(
            edges(
                following.state.offset.y,
                following.content_size.y,
                following.inner_rect.height()
            ),
            (true, false)
        );
        frame(
            &ctx,
            75,
            true,
            false,
            vec![Event::MouseWheel {
                unit: MouseWheelUnit::Point,
                delta: vec2(0.0, 100.0),
                modifiers: Modifiers::NONE,
            }],
        );
        for _ in 0..30 {
            frame(&ctx, 75, true, false, vec![]);
        }
        let history = frame(&ctx, 75, true, false, vec![]);
        let appended = frame(&ctx, 80, true, false, vec![]);
        assert!((appended.state.offset.y - history.state.offset.y).abs() < 0.5);
    }

    #[test]
    fn page_shortcuts_do_not_steal_input_from_a_focused_text_field() {
        let ctx = Context::default();
        frame(
            &ctx,
            60,
            false,
            true,
            vec![Event::PointerMoved(pos2(220.0, 150.0))],
        );
        for k in [Key::PageDown, Key::End, Key::Home, Key::PageUp] {
            let output = frame(&ctx, 60, false, true, vec![key(k)]);
            assert!(output.state.offset.y < 0.5);
        }
    }
}
