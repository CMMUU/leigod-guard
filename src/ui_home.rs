//! Pure presentation: actions are applied by App, never by the renderer.
use crate::config::{valid_game_executable, GameEntry, Strategy};
use crate::platform_api::Provider;
use crate::shared::StartupPauseStatus;
use crate::ui_theme::{self as theme, Icon};
use egui::{vec2, Color32, Rect, RichText, Stroke, Ui};

pub struct HomeState<'a> {
    pub provider: Provider,
    pub guard_ready: bool,
    pub account_ready: bool,
    pub observation: Option<&'a crate::game_lifecycle::Observation>,
    pub startup: Option<(StartupPauseStatus, bool)>,
    pub strategy: &'a Strategy,
    pub games: &'a [GameEntry],
    pub processes: Option<&'a [String]>,
    pub status: &'a str,
    pub balance: Option<&'a TimeBalance>,
}

#[derive(Default)]
pub struct TimeBalance {
    pub seconds: Option<u64>,
    pub free_seconds: Option<u64>,
    pub checked_at: Option<String>,
    pub logged_in: bool,
    pub refreshing: bool,
    pub query_failed: bool,
}

fn duration_label(seconds: u64) -> String {
    format!(
        "{} 时 {:02} 分 {:02} 秒",
        seconds / 3600,
        seconds % 3600 / 60,
        seconds % 60
    )
}

#[derive(Default, Debug, PartialEq, Eq)]
pub enum HomeAction {
    #[default]
    None,
    Select(Provider),
    Defer,
    Pause,
    Strategy,
    Account,
    RefreshAccount,
    AddGame,
    RemoveGame(usize),
}

struct Presentation {
    title: String,
    detail: String,
    value: String,
    caption: &'static str,
    can_defer: bool,
    color: Color32,
}

fn presentation(state: &HomeState<'_>) -> Presentation {
    let mut p = Presentation {
        title: "本次启动检查已结束".into(),
        detail: "本次不再重复启动检查，游戏退出监控继续按策略执行。".into(),
        value: "待命".into(),
        caption: "游戏退出监控",
        can_defer: false,
        color: theme::TEAL,
    };
    if !state.guard_ready {
        p.title = "正在确认守护对象…".into();
        p.detail = "本机操作已暂缓；云端确认旧保护关闭后继续。".into();
        p.value = "切换中".into();
        p.caption = "等待确认";
        p.color = theme::AMBER;
        return p;
    }
    if !state.account_ready {
        p.title = format!("请先配置{}账号", state.provider.name());
        p.detail = if state.provider == Provider::Etalien {
            "请在账户页登录、读取并校准暂停状态，然后启用。"
        } else {
            "请在账户页登录当前加速器。"
        }
        .into();
        p.value = "待配置".into();
        p.caption = "尚未开始守护";
        p.color = theme::MUTED;
        return p;
    }
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
        .observation
        .is_some_and(|o| o.phase == crate::game_lifecycle::Phase::Launching)
    {
        let o = state.observation.unwrap();
        p.title = "游戏正在启动／准备中".into();
        p.detail = "本轮启动受到保护；检测到游戏后继续守护，旧的退出倒计时已取消。".into();
        p.value = format!("{} 秒", o.remaining(std::time::Instant::now()));
        p.caption = "启动保护剩余";
        p.can_defer = true;
    } else if state
        .games
        .iter()
        .any(|g| game_running(state.processes, &g.exe) == Some(true))
    {
        p.title = "游戏运行中，安心畅玩".into();
        p.detail = "检测到名单中的游戏。游戏全部退出后，再按退出宽限期检查。".into();
        p.value = "守护中".into();
        p.caption = "游戏正在运行";
        p.color = theme::GREEN;
    } else if let Some((startup, requested)) = state.startup.filter(|(s, _)| s.pending) {
        p.can_defer = !requested;
        if requested {
            p.title = "正在延后启动检查…".into();
            p.detail = "请求已交给监控处理，不会开启或恢复加速。".into();
            p.value = "处理中".into();
            p.caption = "准备游戏";
        } else if let Some(seconds) = startup.remaining_secs {
            p.value = format!("{:02}:{:02}", seconds / 60, seconds % 60);
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
    } else if state.provider == Provider::Etalien && state.status.contains("已确认暂停") {
        p.title = "计时已暂停".into();
        p.detail = "外星仔官方状态查询已确认暂停。".into();
        p.value = "已暂停".into();
        p.caption = "官方状态已确认";
    } else if state.status.starts_with("暂停请求返回成功") {
        p.title = "暂停请求返回成功".into();
        p.detail = "请在雷神官方微信小程序下拉刷新，核对实际计时状态。".into();
        p.value = "待核对".into();
        p.caption = "以小程序状态为准";
    }
    if state
        .observation
        .is_some_and(|o| o.phase == crate::game_lifecycle::Phase::Absent)
        && state.strategy.enabled
        && state.processes.is_some()
    {
        p.can_defer = true;
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
    theme::aero_header(ui);
    ui.horizontal_wrapped(|ui| {
        ui.label(theme::title("守护概览", 32.0));
        ui.add_space(12.0);
        ui.add_enabled_ui(state.guard_ready, |ui| {
            let mut selected = state.provider;
            egui::ComboBox::from_id_salt("guard-provider")
                .selected_text(format!("{}加速器", selected.name()))
                .width(150.0)
                .show_ui(ui, |ui| {
                    for provider in Provider::ALL {
                        ui.selectable_value(
                            &mut selected,
                            provider,
                            format!("{}加速器", provider.name()),
                        );
                    }
                });
            if selected != state.provider {
                action = HomeAction::Select(selected);
            }
        });
    });
    if !state.guard_ready {
        ui.disable();
    }
    ui.label(RichText::new("让加速时长，留给真正开玩的时刻。").color(theme::MUTED));
    ui.add_space(12.0);
    let p = presentation(state);
    theme::cut_panel(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 0.0;
        status_row(ui, |ui| {
            ui.horizontal(|ui| {
                let label = ui.add(
                    egui::Label::new(theme::title("自动暂停", 16.0)).sense(egui::Sense::click()),
                );
                if label.clicked() {
                    *enabled = !*enabled;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let ready = *enabled
                        && state.guard_ready
                        && state.account_ready
                        && state.processes.is_some()
                        && !state.games.is_empty()
                        && state.games.iter().all(|g| valid_game_executable(&g.exe));
                    let text = if !state.guard_ready {
                        "切换中"
                    } else if !state.account_ready {
                        "待配置"
                    } else if !*enabled {
                        "已停用"
                    } else if ready {
                        "监控中"
                    } else {
                        "待就绪"
                    };
                    ui.label(RichText::new(text).color(theme::MUTED));
                    theme::toggle(ui, enabled, "自动暂停");
                });
            });
        });
        group_separator(ui);
        status_row(ui, |ui| {
            let width = ui.available_width();
            if width >= 710.0 {
                let left_width = width * 0.55;
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 22.0;
                    let left = ui.allocate_ui_with_layout(
                        vec2(left_width, 0.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_min_width(left_width);
                            ui.set_max_width(left_width);
                            guard_status(ui, state, &p);
                        },
                    );
                    let right = ui.allocate_ui_with_layout(
                        vec2(width - left_width - 22.0, 0.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_min_width(width - left_width - 22.0);
                            ui.set_max_width(width - left_width - 22.0);
                            ui.label(
                                RichText::new(format!("{}账户剩余时长", state.provider.name()))
                                    .size(15.0)
                                    .color(theme::MUTED),
                            );
                            let next = time_balance(ui, state.balance, state.provider);
                            if next != HomeAction::None {
                                action = next;
                            }
                        },
                    );
                    let x = left.response.rect.right() + 11.0;
                    ui.painter().line_segment(
                        [
                            egui::pos2(x, left.response.rect.top()),
                            egui::pos2(
                                x,
                                left.response
                                    .rect
                                    .bottom()
                                    .max(right.response.rect.bottom()),
                            ),
                        ],
                        Stroke::new(1.0, theme::BORDER),
                    );
                });
            } else {
                guard_status(ui, state, &p);
                ui.add_space(10.0);
                group_separator(ui);
                ui.add_space(10.0);
                ui.label(
                    RichText::new(format!("{}账户剩余时长", state.provider.name()))
                        .size(13.0)
                        .color(theme::MUTED),
                );
                let next = time_balance(ui, state.balance, state.provider);
                if next != HomeAction::None {
                    action = next;
                }
            }
        });
    });
    ui.add_space(10.0);
    let width = ui.available_width();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 16.0;
        let button_width = (width - 16.0) / 2.0;
        if ui
            .add_enabled_ui(p.can_defer, |ui| {
                theme::action_button(
                    ui,
                    "准备游戏，保护10分钟",
                    Icon::Protect,
                    true,
                    button_width,
                )
            })
            .inner
            .clicked()
        {
            action = HomeAction::Defer;
        }
        if ui
            .add_enabled_ui(state.account_ready, |ui| {
                theme::action_button(
                    ui,
                    &format!("立即暂停{}", state.provider.name()),
                    Icon::Pause,
                    false,
                    button_width,
                )
            })
            .inner
            .clicked()
        {
            action = HomeAction::Pause;
        }
    });
    ui.label(
        RichText::new(if p.can_defer {
            "准备开玩？先保护启动，再启动游戏。"
        } else if p.title == "游戏运行中，安心畅玩" {
            "已检测到游戏，保持守护；游戏退出后再按策略检查。"
        } else {
            "准备保护会在可用时开启，不会启动或恢复加速。"
        })
        .size(12.0)
        .color(theme::MUTED),
    );
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label(theme::title("游戏名单", 20.0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add(theme::outline_button("＋ 添加游戏", true)).clicked() {
                action = HomeAction::AddGame;
            }
        });
    });
    if let Some(index) = game_list(ui, state.games, state.processes, state.observation) {
        action = HomeAction::RemoveGame(index);
    }
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().interact_size.y = 20.0;
        let startup = if state.strategy.pause_on_startup {
            wait_label(state.strategy.startup_grace_secs)
        } else {
            "已关闭".into()
        };
        ui.label(
            RichText::new(format!(
                "自动暂停规则：启动检查{startup} · 游戏退出{}",
                wait_label(state.strategy.grace_secs)
            ))
            .size(12.0)
            .color(theme::MUTED),
        );
        if ui.link(RichText::new("调整策略 ›").size(12.0)).clicked() {
            action = HomeAction::Strategy;
        }
    });
    ui.scope(|ui| {
        ui.spacing_mut().interact_size.y = 22.0;
        ui.collapsing(RichText::new("生效条件与异常处理").size(12.0).color(theme::MUTED), |ui| {
            ui.label("自动暂停须开启总开关，游戏名单非空且进程名有效；启动检查还须开启对应策略。检测失败不会被当作游戏退出，恢复后重新累计等待时间。");
            ui.label("准备游戏可保护首次启动和重新启动，至少等到最后一次点击满10分钟；重复点击不累加，检测到主进程后转入正常守护，不会开启或恢复加速。");
            ui.label("暂停失败后冷却60秒并重新复核。关机暂停是独立开关，断电或强制退出不能保证；已消耗的时长无法追回。");
            ui.label(format!("当前后台状态：{}", state.status));
        });
    });
    action
}

fn guard_status(ui: &mut Ui, state: &HomeState<'_>, p: &Presentation) {
    let running = p.title == "游戏运行中，安心畅玩";
    let width = ui.available_width();
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 14.0;
        let (symbol, _) = ui.allocate_exact_size(vec2(42.0, 46.0), egui::Sense::hover());
        theme::status_symbol(ui, symbol.shrink2(vec2(0.0, 2.0)), running, p.color);
        ui.allocate_ui_with_layout(
            vec2((width - 56.0).max(80.0), 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_min_width((width - 56.0).max(80.0));
                ui.set_max_width((width - 56.0).max(80.0));
                ui.spacing_mut().item_spacing.y = 7.0;
                ui.label(theme::title(
                    &p.title,
                    if width >= 430.0 { 22.0 } else { 18.0 },
                ));
                if running {
                    let names = state
                        .games
                        .iter()
                        .filter(|game| game_running(state.processes, &game.exe) == Some(true))
                        .map(|game| game.name.as_str())
                        .collect::<Vec<_>>()
                        .join("、");
                    ui.label(
                        RichText::new(format!("已检测到 {names}"))
                            .size(14.0)
                            .color(theme::TEXT),
                    );
                    ui.label(
                        RichText::new(format!(
                            "游戏全部退出后等待 {} 秒，再复查并暂停。",
                            state.strategy.grace_secs
                        ))
                        .size(12.0)
                        .color(theme::MUTED),
                    );
                } else {
                    ui.label(RichText::new(&p.detail).size(13.0).color(theme::MUTED));
                    ui.horizontal_wrapped(|ui| {
                        ui.label(theme::title(
                            &p.value,
                            if p.value.contains(':') { 24.0 } else { 16.0 },
                        ));
                        ui.label(RichText::new(p.caption).size(12.0).color(theme::MUTED));
                    });
                }
            },
        );
    });
}

fn status_row(ui: &mut Ui, contents: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(22, 16))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            contents(ui);
        });
}

fn group_separator(ui: &mut Ui) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter().line_segment(
        [r.left_center(), r.right_center()],
        Stroke::new(1.0_f32, theme::BORDER),
    );
}

fn time_balance(ui: &mut Ui, balance: Option<&TimeBalance>, provider: Provider) -> HomeAction {
    let fallback = TimeBalance::default();
    let balance = balance.unwrap_or(&fallback);
    let mut action = HomeAction::None;
    ui.spacing_mut().item_spacing.y = 3.0;
    // Always describe a server snapshot, never simulate billing by subtracting
    // the protection countdown or assuming another device has not paused it.
    let value = if !balance.logged_in {
        "登录后查看".into()
    } else if let Some(seconds) = balance.seconds {
        duration_label(seconds)
    } else if balance.refreshing {
        "正在查询…".into()
    } else {
        "暂不可用".into()
    };
    let size = if ui.available_width() < 290.0 {
        20.0
    } else if ui.available_width() < 360.0 {
        24.0
    } else {
        30.0
    };
    ui.label(theme::title(value, size));
    if let Some(free) = balance.free_seconds.filter(|_| balance.logged_in) {
        ui.label(
            RichText::new(format!("免费时长 {}（单独计）", duration_label(free)))
                .size(11.0)
                .color(theme::MUTED),
        );
    }
    ui.spacing_mut().interact_size.y = 19.0;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 7.0;
        ui.spacing_mut().interact_size.y = 19.0;
        let note = if !balance.logged_in {
            format!("登录{}账号后显示", provider.name())
        } else if balance.refreshing {
            if balance.seconds.is_some() {
                "上次结果 · 正在刷新…".into()
            } else {
                "最多等待 15 秒".into()
            }
        } else if let Some(time) = &balance.checked_at {
            if balance.seconds.is_some() {
                format!("上次查询 {time}")
            } else {
                "未返回可识别的时长，请刷新重试".into()
            }
        } else if balance.query_failed {
            "查询失败，请重试或重新登录".into()
        } else {
            "等待查询账户信息".into()
        };
        let right_aligned = ui.layout().main_dir() == egui::Direction::RightToLeft;
        if !right_aligned {
            ui.label(RichText::new(&note).size(11.0).color(theme::MUTED));
        }
        if balance.logged_in {
            if ui
                .add_enabled(
                    !balance.refreshing,
                    egui::Link::new(RichText::new("刷新时长").size(11.0)),
                )
                .clicked()
            {
                action = HomeAction::RefreshAccount;
            }
        } else {
            let response = ui.link(RichText::new("去登录").size(11.0));
            if response.clicked() {
                action = HomeAction::Account;
            }
        }
        if right_aligned {
            ui.label(RichText::new(note).size(11.0).color(theme::MUTED));
        }
    });
    action
}

fn wait_label(seconds: u64) -> String {
    if seconds > 0 && seconds.is_multiple_of(60) {
        format!("等待 {} 分钟", seconds / 60)
    } else {
        format!("等待 {seconds} 秒")
    }
}

fn game_list(
    ui: &mut Ui,
    games: &[GameEntry],
    processes: Option<&[String]>,
    observation: Option<&crate::game_lifecycle::Observation>,
) -> Option<usize> {
    let mut remove = None;
    theme::card()
        .inner_margin(egui::Margin::symmetric(16, 8))
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
                            vec2((width - 175.0).max(40.0), 42.0),
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
                            } else if observation.is_some_and(|o| o.launching.contains(&game.name))
                            {
                                ("启动中", theme::AMBER)
                            } else {
                                match game_running(processes, &game.exe) {
                                    Some(true) => ("运行中", theme::GREEN),
                                    Some(false) => ("未运行", theme::MUTED),
                                    None => ("待检测", theme::AMBER),
                                }
                            };
                            ui.label(RichText::new(text).size(14.0).color(color));
                            let (dot, _) =
                                ui.allocate_exact_size(vec2(8.0, 8.0), egui::Sense::hover());
                            ui.painter().circle_filled(dot.center(), 4.0, color);
                        });
                    });
                });
            }
        });
    remove
}

fn game_icon(ui: &Ui, game: &GameEntry, rect: Rect) {
    let exe = game.exe.to_ascii_lowercase();
    let text = match exe.as_str() {
        "cs2.exe" => "CS2".into(),
        "tslgame.exe" => "PUBG".into(),
        "r5apex.exe" => "APEX".into(),
        "dota2.exe" => "D2".into(),
        _ => game.name.chars().take(2).collect::<String>(),
    };
    ui.painter()
        .rect_filled(rect, 4, Color32::from_rgb(234, 238, 245));
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        &text,
        theme::heading_font(if text.len() > 3 { 12.0 } else { 15.0 }),
        theme::MUTED,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subminute_balance_is_not_reported_as_zero() {
        assert_eq!(duration_label(0), "0 时 00 分 00 秒");
        assert_eq!(duration_label(1), "0 时 00 分 01 秒");
        assert_eq!(duration_label(59), "0 时 00 分 59 秒");
        assert_eq!(duration_label(60), "0 时 01 分 00 秒");
    }

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
            provider: Provider::Leigod,
            guard_ready: true,
            account_ready: true,
            observation: None,
            startup: Some((StartupPauseStatus::default(), false)),
            strategy: &strategy,
            games: &games,
            processes: None,
            status: "初始化…",
            balance: None,
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
