//! Nonblocking account panel. Background requests never persist credentials.
use crate::{
    config::{Config, Etalien},
    dpapi, etalien_api as api,
    shared::{ManualCmd, Shared},
    ui_theme as theme,
};
use std::{
    sync::{
        mpsc::{self, Receiver},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

enum Event {
    Login {
        token: String,
        device: String,
        user: String,
    },
    Info {
        token: String,
        info: api::AccountInfo,
        calibrate: bool,
    },
}

pub struct Panel {
    pub shared: Arc<Mutex<Shared>>,
    user: String,
    password: String,
    token_input: String,
    token_mode: bool,
    events: Option<Receiver<Result<Event, String>>>,
    started: Instant,
    message: String,
}

impl Default for Panel {
    fn default() -> Self {
        Self {
            shared: Arc::new(Mutex::new(Shared::default())),
            user: String::new(),
            password: String::new(),
            token_input: String::new(),
            token_mode: false,
            events: None,
            started: Instant::now(),
            message: String::new(),
        }
    }
}

pub fn describe(info: &api::AccountInfo, paused: Option<i64>) -> String {
    let state = if paused.is_some() && info.pause_state == paused {
        "已暂停"
    } else if info.pause_state.unwrap_or(0) == 0 {
        "未暂停"
    } else {
        "待校准"
    };
    format!(
        "官方状态：{state} · 可暂停时长 {} 分钟 · 免费时长 {} 分钟",
        info.vip_duration_second / 60,
        info.free_duration_second / 60
    )
}

fn save(config: &Arc<Mutex<Config>>, account: Etalien) -> Result<(), String> {
    let mut c = config.lock().map_err(|_| "无法读取配置")?;
    let previous = std::mem::replace(&mut c.etalien, account);
    if let Err(error) = c.save() {
        c.etalien = previous;
        return Err(format!("无法保存外星仔配置：{error}"));
    }
    Ok(())
}

impl Panel {
    fn start(
        &mut self,
        ctx: egui::Context,
        action: impl FnOnce() -> Result<Event, String> + Send + 'static,
    ) {
        let (tx, rx) = mpsc::channel();
        self.events = Some(rx);
        self.started = Instant::now();
        self.message = "正在连接外星仔…".into();
        std::thread::spawn(move || {
            let _ = tx.send(action());
            ctx.request_repaint();
        });
    }

    pub fn poll(&mut self, config: &Arc<Mutex<Config>>) {
        let result = self.events.as_ref().and_then(|rx| match rx.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Disconnected) => Some(Err("外星仔查询中断，请重试".into())),
            Err(_) if self.started.elapsed() > Duration::from_secs(28) => {
                Some(Err("外星仔查询超时，请重试".into()))
            }
            Err(_) => None,
        });
        let Some(result) = result else {
            return;
        };
        self.events = None;
        let outcome = result.and_then(|event| match event {
            Event::Login {
                token,
                device,
                user,
            } => {
                let account = Etalien {
                    username: user,
                    token_enc: dpapi::protect(&token)?,
                    device_id: device,
                    ..Default::default()
                };
                save(config, account)?;
                let mut s = self.shared.lock().map_err(|_| "无法读取账号")?;
                s.set_token(Some(token));
                s.account_status = "已登录，等待首次暂停状态校准".into();
                self.password.clear();
                self.token_input.clear();
                Ok("登录成功。先在外星仔官方客户端暂停，再点击下方校准按钮。".into())
            }
            Event::Info {
                token,
                info,
                calibrate,
            } => {
                if self
                    .shared
                    .lock()
                    .map_err(|_| "无法读取账号")?
                    .token
                    .as_deref()
                    != Some(&token)
                {
                    return Err("账号已变化，已忽略旧查询".into());
                }
                let mut account = config.lock().map_err(|_| "无法读取配置")?.etalien.clone();
                if calibrate {
                    account.paused_state = Some(api::calibrated_state(&info)?);
                    // Calibration is read-only and never enables automation itself.
                    save(config, account.clone())?;
                }
                self.shared
                    .lock()
                    .map_err(|_| "无法读取账号")?
                    .account_status = describe(&info, account.paused_state);
                Ok(if calibrate {
                    "暂停状态已校准，可以开启外星仔自动暂停。"
                } else {
                    "状态已刷新"
                }
                .into())
            }
        });
        self.message = outcome.unwrap_or_else(|error| error);
    }

    pub fn show(&mut self, ui: &mut egui::Ui, config: &Arc<Mutex<Config>>) {
        self.poll(config);
        let account = config.lock().map(|c| c.etalien.clone()).unwrap_or_default();
        ui.label(egui::RichText::new("外星仔加速器 · 本机自动暂停（试验性接入）").strong());
        ui.label("使用游戏名单、游戏退出宽限期和启动保护。平台服务器失联保护目前仅支持雷神。");
        ui.label(
            egui::RichText::new(
                "登录令牌由 Windows 加密保存；密码不保存。接口与真实账号兼容性仍需验收。",
            )
            .color(theme::MUTED),
        );
        ui.add_space(8.0);
        let busy = self.events.is_some();
        ui.add_enabled_ui(!busy, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.selectable_value(&mut self.token_mode, false, "密码登录");
                ui.selectable_value(&mut self.token_mode, true, "已有 Token");
            });
            if self.token_mode {
                ui.label("粘贴你本人账号的 Authorization（不会发送给守护平台）：");
                ui.add(
                    egui::TextEdit::singleline(&mut self.token_input)
                        .password(true)
                        .desired_width(340.0),
                );
            } else {
                ui.horizontal_wrapped(|ui| {
                    ui.label("手机号");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.user)
                            .hint_text(&account.username)
                            .desired_width(160.0),
                    );
                    ui.label("密码");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.password)
                            .password(true)
                            .desired_width(160.0),
                    );
                });
            }
            if theme::primary(ui, "登录外星仔").clicked() {
                let user = self.user.trim().to_owned();
                let password = std::mem::take(&mut self.password);
                let imported = std::mem::take(&mut self.token_input).trim().to_owned();
                let token_mode = self.token_mode;
                let device = if account.device_id.is_empty() {
                    api::device_id()
                } else {
                    Ok(account.device_id.clone())
                };
                self.start(ui.ctx().clone(), move || {
                    let device = device?;
                    let token = if token_mode {
                        imported
                    } else {
                        api::login(&user, &password, &device)?
                    };
                    api::info(&token, &device)?;
                    Ok(Event::Login {
                        token,
                        device,
                        user,
                    })
                });
            }
            ui.separator();
            let token = self.shared.lock().ok().and_then(|s| s.token.clone());
            ui.add_enabled_ui(token.is_some(), |ui| {
                ui.label("首次配置：先在官方客户端点击暂停并刷新，确认已经暂停，再读取状态。");
                ui.horizontal_wrapped(|ui| {
                    let calibrate = ui.button("我已在官方暂停，读取并校准").clicked();
                    let refresh = ui.button("刷新状态").clicked();
                    if calibrate || refresh {
                        let token = token.clone().unwrap_or_default();
                        let device = account.device_id.clone();
                        self.start(ui.ctx().clone(), move || {
                            Ok(Event::Info {
                                info: api::info(&token, &device)?,
                                token,
                                calibrate,
                            })
                        });
                    }
                });
            });
            let mut enabled = account.enabled;
            if ui
                .add_enabled(
                    account.paused_state.is_some() && token.is_some(),
                    egui::Checkbox::new(&mut enabled, "启用外星仔自动暂停"),
                )
                .changed()
            {
                let mut next = account.clone();
                next.enabled = enabled;
                self.message = save(config, next)
                    .map(|_| {
                        if enabled {
                            "外星仔守护已开启"
                        } else {
                            "外星仔守护已关闭"
                        }
                        .to_string()
                    })
                    .unwrap_or_else(|e| e);
            }
            if ui
                .add_enabled(
                    account.ready() && token.is_some(),
                    egui::Button::new("立即暂停外星仔"),
                )
                .clicked()
            {
                if let Ok(mut s) = self.shared.lock() {
                    s.manual_cmd = Some(ManualCmd::Pause);
                }
            }
        });
        if ui.button("退出外星仔账号并清除保存").clicked() {
            self.events = None;
            self.message = match save(config, Etalien::default()) {
                Ok(()) => {
                    if let Ok(mut s) = self.shared.lock() {
                        s.set_token(None);
                        s.manual_cmd = None;
                        s.account_status = "未登录".into();
                    }
                    "已清除外星仔账号".into()
                }
                Err(error) => error,
            };
        }
        if busy {
            ui.spinner();
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
        if !self.message.is_empty() {
            ui.label(&self.message);
        }
        if let Ok(s) = self.shared.lock() {
            ui.separator();
            ui.label(&s.account_status);
            ui.label(s.status_at(Instant::now()));
            for line in s.logs.iter().rev().take(3) {
                ui.label(egui::RichText::new(line).small().color(theme::MUTED));
            }
        }
    }
}
