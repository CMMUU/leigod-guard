//! Pure presentation: actions are applied by App, never by the renderer.
use crate::config::{valid_game_executable, GameEntry, Strategy};
use crate::shared::StartupPauseStatus;
use crate::ui_theme::{self as theme, Icon};
use egui::{vec2, Color32, FontId, Rect, RichText, Stroke, Ui};

pub struct HomeState<'a> {
    pub startup: Option<(StartupPauseStatus, bool)>,
    pub strategy: &'a Strategy,
    pub games: &'a [GameEntry],
    pub processes: Option<&'a [String]>,
    pub status: &'a str,
}

#[derive(Default, Debug, PartialEq, Eq)]
pub enum HomeAction {
    #[default]
    None,
    Defer,
    Pause,
    Strategy,
    AddGame,
    RemoveGame(usize),
}

struct Presentation {
    title: String,
    detail: String,
    value: String,
    caption: &'static str,
    progress: f32,
    can_defer: bool,
    color: Color32,
}

fn presentation(state: &HomeState<'_>) -> Presentation {
    let mut p = Presentation {
        title: "本次启动检查已结束".into(),
        detail: "本次不再重复启动检查，游戏退出监控继续按策略执行。".into(),
        value: "待命".into(),
        caption: "游戏退出监控",
        progress: 1.0,
        can_defer: false,
        color: theme::TEAL,
    };
    if !state.strategy.enabled {
        p.title = "自动暂停已停用".into();
        p.detail =
            "开启总开关后继续监控游戏退出；启动检查已结束时，需要下次启动才重新检查。".into();
        p.value = "已关闭".into();
        p.caption = "手动暂停仍可用";
        p.color = theme::MUTED;
    } else if state.games.is_empty() {
        p.title = "先添加要守护的游戏".into();
        p.detail = "名单为空时不会自动暂停。添加游戏后，重新打开本工具可执行启动检查。".into();
        p.value = "—".into();
        p.caption = "等待配置";
        p.color = theme::MUTED;
    } else if state.games.iter().any(|g| !valid_game_executable(&g.exe)) {
        p.title = "请检查游戏进程名".into();
        p.detail = "名单包含无效进程名，自动暂停已暂缓；请修正对应条目。".into();
        p.value = "待检查".into();
        p.caption = "名单需要调整";
        p.color = theme::AMBER;
    } else if state.processes.is_none() {
        p.title = "等待有效检测".into();
        p.detail = "暂时无法确认游戏状态，不会把检测失败当作游戏退出。".into();
        p.value = "—".into();
        p.caption = "检测尚未就绪";
        p.color = theme::AMBER;
        p.can_defer = state
            .startup
            .is_some_and(|(s, requested)| s.pending && !requested);
    } else if state
        .games
        .iter()
        .any(|g| game_running(state.processes, &g.exe) == Some(true))
    {
        p.title = "游戏运行中，安心畅玩".into();
        p.detail = "检测到名单中的游戏。游戏全部退出后，再按退出宽限期检查。".into();
        p.value = "守护中".into();
        p.caption = "游戏正在运行";
    } else if let Some((startup, requested)) = state.startup.filter(|(s, _)| s.pending) {
        p.can_defer = !requested;
        if requested {
            p.title = "正在延后启动检查…".into();
            p.detail = "请求已交给监控处理，不会开启或恢复加速。".into();
            p.value = "处理中".into();
            p.caption = "准备游戏";
        } else if let Some(seconds) = startup.remaining_secs {
            p.value = format!("{:02}:{:02}", seconds / 60, seconds % 60);
            let total = if startup.preparing_game {
                state.strategy.startup_grace_secs.max(600)
            } else {
                state.strategy.startup_grace_secs.max(seconds).max(1)
            };
            p.progress = (seconds as f32 / total as f32).clamp(0.0, 1.0);
            p.caption = "剩余等待";
            if seconds == 0 {
                p.title = "正在复核游戏与账户状态".into();
                p.detail = "确认名单中的游戏未运行后，才会尝试暂停计时。此时仍可延后。".into();
                p.caption = "复核中";
            } else if startup.preparing_game {
                p.title = "正在为你预留准备时间".into();
                p.detail = "准备游戏保护已生效；检测到游戏后，结束本次启动检查。".into();
            } else {
                p.title = "暂未检测到游戏".into();
                p.detail = "等待结束后再次检查，无游戏运行时尝试暂停计时。".into();
            }
        } else {
            p.title = "等待有效检测".into();
            p.detail = "启动检测尚未就绪，暂不据此暂停计时。".into();
            p.value = "—".into();
            p.caption = "等待启动检查";
        }
    } else if state.status.contains("游戏已退出") {
        p.title = "游戏退出宽限期".into();
        p.detail = state.status.into();
        p.value = "等待中".into();
        p.caption = "重开游戏可取消";
    } else if state.status.contains("失败") || state.status.contains("未确认") {
        p.title = "暂停尚未确认".into();
        p.detail = state.status.into();
        p.value = "待核对".into();
        p.caption = "请查看日志";
        p.color = theme::AMBER;
    } else if state.status.starts_with("暂停请求返回成功") {
        p.title = "暂停请求返回成功".into();
        p.detail = "请在雷神官方微信小程序下拉刷新，核对实际计时状态。".into();
        p.value = "待核对".into();
        p.caption = "以小程序状态为准";
    }
    p
}

pub fn game_running(processes: Option<&[String]>, exe: &str) -> Option<bool> {
    if !valid_game_executable(exe) {
        return None;
    }
    processes.map(|list| list.iter().any(|p| p.eq_ignore_ascii_case(exe.trim())))
}

pub fn render(ui: &mut Ui, state: &HomeState<'_>, enabled: &mut bool) -> HomeAction {
    ui.spacing_mut().item_spacing.y = 8.0;
    let mut action = HomeAction::None;
    let width = ui.available_width();
    let header = |ui: &mut Ui| {
        ui.label(theme::title("守护概览", 27.0));
        ui.label(RichText::new("让加速时长，留给真正开玩的时刻。").color(theme::MUTED));
    };
    let controls = |ui: &mut Ui, enabled: &mut bool| {
        ui.horizontal(|ui| {
            let ready = *enabled
                && state.processes.is_some()
                && !state.games.is_empty()
                && state.games.iter().all(|g| valid_game_executable(&g.exe));
            let (label, color) = if !*enabled {
                ("已停用", theme::MUTED)
            } else if ready {
                ("监控中", theme::GREEN)
            } else {
                ("待就绪", theme::AMBER)
            };
            let (dot, _) = ui.allocate_exact_size(vec2(9.0, 9.0), egui::Sense::hover());
            ui.painter().circle_filled(dot.center(), 4.5, color);
            ui.label(RichText::new(label).size(14.0).color(color));
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(6.0);
            theme::toggle(ui, enabled, "自动暂停");
        });
    };
    if width >= 760.0 {
        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(
                vec2(width - 252.0, 65.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_min_width(width - 252.0);
                    header(ui);
                },
            );
            controls(ui, enabled);
        });
    } else {
        header(ui);
        ui.add_space(2.0);
        controls(ui, enabled);
    }
    ui.add_space(10.0);
    let p = presentation(state);
    if width >= 810.0 {
        ui.horizontal_top(|ui| {
            let left = (width - 20.0) * 0.64;
            ui.allocate_ui_with_layout(
                vec2(left, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    action = protection_card(ui, &p);
                },
            );
            ui.add_space(10.0);
            ui.allocate_ui_with_layout(
                vec2(width - left - 20.0, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    if rules_card(ui, state.strategy) {
                        action = HomeAction::Strategy;
                    }
                },
            );
        });
    } else {
        action = protection_card(ui, &p);
        ui.add_space(10.0);
        if rules_card(ui, state.strategy) {
            action = HomeAction::Strategy;
        }
    }
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label(theme::title("游戏名单", 20.0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("＋ 添加游戏").clicked() {
                action = HomeAction::AddGame;
            }
        });
    });
    ui.add_space(2.0);
    if let Some(index) = game_list(ui, state.games, state.processes) {
        action = HomeAction::RemoveGame(index);
    }
    ui.add_space(8.0);
    egui::Frame::new()
        .fill(Color32::from_rgb(243, 247, 255))
        .stroke(Stroke::new(1.0_f32, Color32::from_rgb(184, 211, 253)))
        .corner_radius(12)
        .inner_margin(12)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(26.0, 26.0), egui::Sense::hover());
                theme::icon(ui, Icon::Info, r, theme::BLUE);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 5.0;
                    ui.label(theme::title("重启后，安心处理其他任务", 15.0));
                    ui.label(
                        RichText::new("无需游戏加速时，自动暂停计时，减少闲置消耗。")
                            .size(13.0)
                            .color(theme::MUTED),
                    );
                });
            });
        });
    ui.add_space(2.0);
    ui.label(
        RichText::new("暂停是否生效：在雷神官方微信小程序下拉刷新核对。")
            .size(12.0)
            .color(theme::MUTED),
    );
    ui.scope(|ui| {
      ui.spacing_mut().interact_size.y = 22.0;
      ui.collapsing(RichText::new("生效条件与异常处理").size(12.0).color(theme::MUTED), |ui| {
        ui.label("自动暂停须开启总开关，游戏名单非空且进程名有效；启动检查还须开启对应策略。检测失败不会被当作游戏退出，恢复后重新累计等待时间。");
        ui.label("准备游戏只延后尚未完成的启动检查，至少等到最后一次点击满10分钟；重复点击不累加，也不会开启或恢复加速。检查完成或跳过后，本次运行不再补做。");
        ui.label("暂停失败后冷却60秒并重新复核。关机暂停是独立开关，断电或强制退出不能保证；已消耗的时长无法追回。");
        ui.label(format!("当前后台状态：{}", state.status));
      });
    });
    action
}

fn protection_card(ui: &mut Ui, p: &Presentation) -> HomeAction {
    let mut action = HomeAction::None;
    theme::card().show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.set_min_height(190.0);
        let width = ui.available_width();
        let ring = if width >= 430.0 { 130.0 } else { 106.0 };
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                vec2((width - ring - 18.0).max(130.0), ring),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_min_width((width - ring - 18.0).max(130.0));
                    ui.label(RichText::new("本次启动保护").size(13.0).color(p.color));
                    ui.add_space(7.0);
                    ui.label(theme::title(&p.title, 22.0));
                    ui.label(RichText::new(&p.detail).size(13.0).color(theme::MUTED));
                },
            );
            countdown(ui, p, ring);
        });
        ui.add_space(10.0);
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled_ui(p.can_defer, |ui| theme::primary(ui, "准备游戏，延后10分钟"))
                .inner
                .clicked()
            {
                action = HomeAction::Defer;
            }
            if ui
                .add(egui::Button::new("立即暂停计时").min_size(vec2(128.0, 40.0)))
                .clicked()
            {
                action = HomeAction::Pause;
            }
        });
        ui.add_space(1.0);
        ui.label(
            RichText::new(if p.can_defer {
                "准备开玩？先延后，再启动游戏。"
            } else {
                "仅在本次启动检查尚未结束时可延后。"
            })
            .size(12.0)
            .color(theme::MUTED),
        );
    });
    action
}

fn countdown(ui: &mut Ui, p: &Presentation, size: f32) {
    let (r, _) = ui.allocate_exact_size(vec2(size, size), egui::Sense::hover());
    let radius = size / 2.0 - 5.0;
    ui.painter().circle_stroke(
        r.center(),
        radius,
        Stroke::new(5.0_f32, Color32::from_rgb(220, 245, 237)),
    );
    if p.progress > 0.0 {
        let start = -std::f32::consts::FRAC_PI_2;
        let count = (100.0 * p.progress).ceil() as usize;
        let points = (0..=count)
            .map(|i| {
                let angle =
                    start + std::f32::consts::TAU * p.progress * i as f32 / count.max(1) as f32;
                r.center() + vec2(angle.cos(), angle.sin()) * radius
            })
            .collect();
        ui.painter()
            .add(egui::Shape::line(points, Stroke::new(5.0_f32, p.color)));
    }
    let font = if p.value.contains(':') {
        FontId::proportional(size * 0.245)
    } else {
        FontId::proportional(size * 0.17)
    };
    ui.painter().text(
        r.center() - vec2(0.0, 10.0),
        egui::Align2::CENTER_CENTER,
        &p.value,
        font,
        theme::TEXT,
    );
    ui.painter().text(
        r.center() + vec2(0.0, 21.0),
        egui::Align2::CENTER_CENTER,
        p.caption,
        FontId::proportional(11.0),
        theme::MUTED,
    );
}

fn wait_label(seconds: u64) -> String {
    if seconds > 0 && seconds.is_multiple_of(60) {
        format!("等待 {} 分钟", seconds / 60)
    } else {
        format!("等待 {seconds} 秒")
    }
}

fn rules_card(ui: &mut Ui, strategy: &Strategy) -> bool {
    theme::card()
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.set_min_height(190.0);
            ui.label(theme::title("自动暂停规则", 16.0));
            ui.add_space(6.0);
            for (kind, label, value) in [
                (
                    Icon::Clock,
                    "启动检查",
                    if strategy.pause_on_startup {
                        wait_label(strategy.startup_grace_secs)
                    } else {
                        "已关闭".into()
                    },
                ),
                (Icon::Exit, "游戏退出", wait_label(strategy.grace_secs)),
            ] {
                ui.separator();
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(20.0, 26.0), egui::Sense::hover());
                    theme::icon(ui, kind, r.shrink2(vec2(0.0, 3.0)), theme::MUTED);
                    ui.label(RichText::new(label).size(14.0));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(value).size(12.0).color(theme::MUTED));
                    });
                });
            }
            ui.separator();
            ui.label(
                RichText::new("游戏重新运行，将取消退出倒计时。")
                    .size(12.0)
                    .color(theme::MUTED),
            );
            ui.link(RichText::new("调整策略 ›").color(theme::BLUE))
                .clicked()
        })
        .inner
}

fn game_list(ui: &mut Ui, games: &[GameEntry], processes: Option<&[String]>) -> Option<usize> {
    let mut remove = None;
    theme::card()
        .inner_margin(egui::Margin::symmetric(16, 4))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 6.0;
            if games.is_empty() {
                ui.add_space(18.0);
                ui.label(theme::title("名单还是空的", 16.0));
                ui.label(
                    RichText::new("点击“添加游戏”，选择热门游戏或填写自定义进程名。")
                        .size(13.0)
                        .color(theme::MUTED),
                );
                ui.add_space(18.0);
            }
            for (index, game) in games.iter().enumerate() {
                ui.push_id(index, |ui| {
                    if index > 0 {
                        ui.separator();
                    }
                    let width = ui.available_width();
                    ui.horizontal(|ui| {
                        let (icon_rect, _) =
                            ui.allocate_exact_size(vec2(42.0, 42.0), egui::Sense::hover());
                        game_icon(ui, game, icon_rect);
                        ui.allocate_ui_with_layout(
                            vec2((width - 175.0).max(40.0), 49.0),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                ui.spacing_mut().item_spacing.y = 3.0;
                                ui.add(
                                    egui::Label::new(RichText::new(&game.name).size(15.0))
                                        .truncate(),
                                )
                                .on_hover_text(&game.name);
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(&game.exe).size(12.0).color(theme::MUTED),
                                    )
                                    .truncate(),
                                )
                                .on_hover_text(&game.exe);
                            },
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let menu = ui.menu_button("   ", |ui| {
                                if ui.button("从名单移除").clicked() {
                                    remove = Some(index);
                                    ui.close_menu();
                                }
                            });
                            theme::icon(
                                ui,
                                Icon::More,
                                Rect::from_center_size(
                                    menu.response.rect.center(),
                                    vec2(17.0, 17.0),
                                ),
                                theme::MUTED,
                            );
                            menu.response.widget_info(|| {
                                egui::WidgetInfo::labeled(
                                    egui::WidgetType::Button,
                                    ui.is_enabled(),
                                    "更多操作",
                                )
                            });
                            menu.response.on_hover_text("更多操作");
                            let (text, color) = if !valid_game_executable(&game.exe) {
                                ("进程无效", theme::AMBER)
                            } else {
                                match game_running(processes, &game.exe) {
                                    Some(true) => ("运行中", theme::GREEN),
                                    Some(false) => ("未运行", theme::MUTED),
                                    None => ("待检测", theme::AMBER),
                                }
                            };
                            theme::badge(ui, text, color);
                        });
                    });
                });
            }
        });
    remove
}

fn game_icon(ui: &Ui, game: &GameEntry, rect: Rect) {
    let exe = game.exe.to_ascii_lowercase();
    let (text, color) = match exe.as_str() {
        "cs2.exe" => ("CS2".into(), Color32::from_rgb(224, 156, 38)),
        "tslgame.exe" => ("PUBG".into(), Color32::from_rgb(46, 49, 49)),
        "r5apex.exe" => ("APEX".into(), Color32::from_rgb(203, 51, 65)),
        "dota2.exe" => ("D2".into(), Color32::from_rgb(155, 66, 58)),
        _ => (
            game.name.chars().take(2).collect::<String>(),
            Color32::from_rgb(96, 128, 181),
        ),
    };
    ui.painter().rect_filled(rect, 9, color);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        &text,
        theme::heading_font(if text.len() > 3 { 12.0 } else { 15.0 }),
        Color32::WHITE,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_status_uses_exact_executables_and_preserves_unknown() {
        let processes = vec!["TSLGAME.EXE".into(), "cs2.exe.bak".into()];
        assert_eq!(game_running(Some(&processes), "tslgame.exe"), Some(true));
        assert_eq!(game_running(Some(&processes), "cs2.exe"), Some(false));
        assert_eq!(game_running(None, "cs2.exe"), None);
        assert_eq!(game_running(Some(&processes), "C:\\cs2.exe"), None);
    }

    #[test]
    fn unavailable_and_completed_checks_never_claim_a_pause_or_offer_deferral() {
        let strategy = Strategy::default();
        let games = vec![GameEntry {
            name: "Test".into(),
            exe: "test.exe".into(),
            plan: String::new(),
        }];
        let mut state = HomeState {
            startup: Some((StartupPauseStatus::default(), false)),
            strategy: &strategy,
            games: &games,
            processes: None,
            status: "初始化…",
        };
        assert_eq!(presentation(&state).title, "等待有效检测");
        assert!(!presentation(&state).can_defer);
        state.processes = Some(&[]);
        assert_eq!(presentation(&state).title, "本次启动检查已结束");
        assert!(!presentation(&state).detail.contains("已暂停"));
        state.startup = Some((
            StartupPauseStatus {
                pending: true,
                remaining_secs: Some(156),
                preparing_game: false,
            },
            false,
        ));
        assert_eq!(presentation(&state).value, "02:36");
        assert!(presentation(&state).can_defer);
        state.processes = None;
        assert!(
            presentation(&state).can_defer,
            "pending checks can be deferred during scan failures"
        );
        assert_eq!(presentation(&state).title, "等待有效检测");
        state.startup.as_mut().unwrap().1 = true;
        assert!(!presentation(&state).can_defer);
    }
}
