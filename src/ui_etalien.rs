//! Nonblocking account panel. Background requests never persist credentials.
use crate::{
    config::{Config, Etalien},
    dpapi, etalien_api as api, etalien_auth as auth,
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

#[derive(Debug)]
enum Event {
    SmsSent(Result<Duration, auth::Error>),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sms_result_extends_official_cooldown_without_changing_saved_account_or_guard() {
        let config = Arc::new(Mutex::new(Config::default()));
        config.lock().unwrap().etalien.username = "existing-account".into();
        let platform = crate::ui_platform::Panel::default();
        let mut panel = Panel::default();
        panel
            .shared
            .lock()
            .unwrap()
            .set_token(Some("existing-token".into()));
        panel.extend_cooldown(auth::DEFAULT_COOLDOWN);
        let (tx, rx) = mpsc::channel();
        panel.events = Some(rx);
        tx.send(Ok(Event::SmsSent(Ok(Duration::from_secs(120)))))
            .unwrap();
        panel.poll(&config, &platform);
        assert!((119..=120).contains(&panel.retry_seconds()));
        assert_eq!(config.lock().unwrap().etalien.username, "existing-account");
        let state = panel.shared.lock().unwrap();
        assert_eq!(state.token.as_deref(), Some("existing-token"));
        assert!(state.manual_cmd.is_none());
    }

    #[test]
    fn failed_login_keeps_current_credentials_and_resend_delay_survives_tab_changes() {
        let config = Arc::new(Mutex::new(Config::default()));
        let platform = crate::ui_platform::Panel::default();
        let mut panel = Panel::default();
        panel
            .shared
            .lock()
            .unwrap()
            .set_token(Some("existing-token".into()));
        panel.extend_cooldown(auth::DEFAULT_COOLDOWN);
        panel.login_mode = LoginMode::Password;
        panel.login_mode = LoginMode::Sms;
        let (tx, rx) = mpsc::channel();
        panel.events = Some(rx);
        tx.send(Err("验证码无效".into())).unwrap();
        panel.poll(&config, &platform);
        assert!(panel.events.is_none());
        assert!(panel.retry_seconds() > 0);
        assert_eq!(
            panel.shared.lock().unwrap().token.as_deref(),
            Some("existing-token")
        );
        assert_eq!(panel.message, "验证码无效");
    }
}

#[derive(Clone, Copy, Default, PartialEq)]
enum LoginMode {
    #[default]
    Sms,
    Password,
    Token,
}

pub struct Panel {
    pub shared: Arc<Mutex<Shared>>,
    user: String,
    password: String,
    token_input: String,
    login_mode: LoginMode,
    sms_phone: String,
    sms_code: String,
    sms_retry_at: Option<Instant>,
    login_device: String,
    events: Option<Receiver<Result<Event, String>>>,
    started: Instant,
    message: String,
    query_failed: bool,
}

impl Default for Panel {
    fn default() -> Self {
        Self {
            shared: Arc::new(Mutex::new(Shared::default())),
            user: String::new(),
            password: String::new(),
            token_input: String::new(),
            login_mode: LoginMode::default(),
            sms_phone: String::new(),
            sms_code: String::new(),
            sms_retry_at: None,
            login_device: String::new(),
            events: None,
            started: Instant::now(),
            message: String::new(),
            query_failed: false,
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

    fn device(&mut self, account: &Etalien) -> Result<String, String> {
        if self.login_device.is_empty() {
            self.login_device = if account.device_id.is_empty() {
                api::device_id()?
            } else {
                account.device_id.clone()
            };
        }
        Ok(self.login_device.clone())
    }

    fn retry_seconds(&self) -> u64 {
        self.sms_retry_at
            .map(|deadline| {
                deadline
                    .saturating_duration_since(Instant::now())
                    .as_secs_f64()
                    .ceil() as u64
            })
            .unwrap_or(0)
    }

    fn extend_cooldown(&mut self, duration: Duration) {
        let deadline = Instant::now() + duration;
        self.sms_retry_at = Some(self.sms_retry_at.map_or(deadline, |old| old.max(deadline)));
    }

    pub fn poll(&mut self, config: &Arc<Mutex<Config>>, platform: &crate::ui_platform::Panel) {
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
            Event::SmsSent(result) => match result {
                Ok(cooldown) => {
                    self.extend_cooldown(cooldown);
                    Ok("验证码已发送，请输入手机收到的最新 6 位验证码。有效期以外星仔官方为准。".into())
                }
                Err(error) => {
                    if let Some(cooldown) = error.retry_after { self.extend_cooldown(cooldown); }
                    Err(error.to_string())
                }
            },
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
                // Failed SMS/password attempts must not revoke an existing grant.
                // A verified replacement account still requires fresh calibration/consent.
                platform.revoke_etalien();
                save(config, account)?;
                let mut s = self.shared.lock().map_err(|_| "无法读取账号")?;
                s.set_token(Some(token));
                s.account_status = "已登录，等待首次暂停状态校准".into();
                self.password.clear();
                self.token_input.clear();
                self.sms_code.clear();
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
                let mut state = self.shared.lock().map_err(|_| "无法读取账号")?;
                state.account_status = describe(&info, account.paused_state);
                state.set_etalien_info(&token, info);
                Ok(if calibrate {
                    "暂停状态已校准，可以开启外星仔自动暂停。"
                } else {
                    "状态已刷新"
                }
                .into())
            }
        });
        self.query_failed = outcome.is_err();
        self.message = outcome.unwrap_or_else(|error| error);
    }

    pub fn balance(&self, s: &Shared) -> crate::ui_home::TimeBalance {
        crate::ui_home::TimeBalance {
            free_seconds: s
                .etalien_info
                .as_ref()
                .map(|info| info.free_duration_second.max(0) as u64),
            seconds: s
                .etalien_info
                .as_ref()
                .map(|info| info.vip_duration_second.max(0) as u64),
            checked_at: s
                .account_info_updated_at
                .map(|t| t.format("%H:%M:%S").to_string()),
            logged_in: s.token.is_some(),
            refreshing: self.events.is_some(),
            query_failed: self.query_failed,
        }
    }
    pub fn refresh(&mut self, config: &Arc<Mutex<Config>>, ctx: &egui::Context) {
        if self.events.is_some() {
            return;
        }
        let Some(token) = self.shared.lock().ok().and_then(|s| s.token.clone()) else {
            return;
        };
        let device = config
            .lock()
            .map(|c| c.etalien.device_id.clone())
            .unwrap_or_default();
        self.start(ctx.clone(), move || {
            Ok(Event::Info {
                info: api::info(&token, &device)?,
                token,
                calibrate: false,
            })
        });
    }
    pub fn auto_refresh(&mut self, config: &Arc<Mutex<Config>>, ctx: &egui::Context, active: bool) {
        if config
            .lock()
            .is_ok_and(|c| c.is_active(crate::platform_api::Provider::Etalien))
            && self.events.is_none()
            && self.started.elapsed() >= Duration::from_secs(60)
            && self
                .shared
                .lock()
                .is_ok_and(|s| s.token.is_some() && (active || s.etalien_info.is_none()))
        {
            self.refresh(config, ctx);
        }
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        config: &Arc<Mutex<Config>>,
        platform: &crate::ui_platform::Panel,
    ) {
        self.poll(config, platform);
        let account = config.lock().map(|c| c.etalien.clone()).unwrap_or_default();
        let compact = ui.available_width() < 520.0;
        let busy = self.events.is_some();
        ui.add_enabled_ui(!busy, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.selectable_value(&mut self.login_mode, LoginMode::Sms, "短信验证码");
                ui.selectable_value(&mut self.login_mode, LoginMode::Password, "密码登录");
                ui.selectable_value(&mut self.login_mode, LoginMode::Token, "已有 Token");
            });
            if self.login_mode == LoginMode::Sms {
                if !compact { ui.label("外星仔账号手机号"); }
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut self.sms_phone)
                            .id_salt("etalien-sms-phone")
                            .hint_text("11 位手机号，可带 +86")
                            .char_limit(14)
                            .desired_width(260.0),
                    )
                    .changed()
                {
                    self.sms_code.clear();
                }
                ui.horizontal_wrapped(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.sms_code)
                            .id_salt("etalien-sms-code")
                            .hint_text("6 位短信验证码")
                            .char_limit(6)
                            .desired_width(160.0),
                    );
                    let remaining = self.retry_seconds();
                    let label = if remaining > 0 {
                        format!("{remaining} 秒后重发")
                    } else {
                        "发送验证码".into()
                    };
                    if ui
                        .add_enabled(
                            remaining == 0 && auth::phone(&self.sms_phone).is_ok(),
                            egui::Button::new(label),
                        )
                        .clicked()
                    {
                        match self.device(&account) {
                            Ok(device) => {
                                let phone = self.sms_phone.clone();
                                self.sms_code.clear();
                                // Start a conservative delay even when the request times out:
                                // the official service may already have sent the SMS.
                                self.extend_cooldown(auth::DEFAULT_COOLDOWN);
                                self.start(ui.ctx().clone(), move || {
                                    Ok(Event::SmsSent(auth::send_code(&phone, &device)))
                                });
                                self.message = "正在请求外星仔发送短信验证码…".into();
                            }
                            Err(error) => self.message = error,
                        }
                    }
                });
            } else if self.login_mode == LoginMode::Token {
                ui.label("粘贴你本人账号的 Authorization（仅单独授权远程保护后上传平台）：");
                ui.add(
                    egui::TextEdit::singleline(&mut self.token_input)
                        .id_salt("etalien-token")
                        .password(true)
                        .desired_width(340.0),
                );
            } else {
                ui.horizontal_wrapped(|ui| {
                    ui.label("手机号");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.user)
                            .id_salt("etalien-password-phone")
                            .hint_text(&account.username)
                            .desired_width(160.0),
                    );
                    ui.label("密码");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.password)
                            .id_salt("etalien-password")
                            .password(true)
                            .desired_width(160.0),
                    );
                });
            }
            let valid_login = match self.login_mode {
                LoginMode::Sms => {
                    auth::phone(&self.sms_phone).is_ok() && auth::valid_code(&self.sms_code)
                }
                LoginMode::Password => {
                    (!self.user.trim().is_empty() || !account.username.is_empty())
                        && !self.password.is_empty()
                }
                LoginMode::Token => !self.token_input.trim().is_empty(),
            };
            if ui
                .add_enabled_ui(valid_login && self.events.is_none(), |ui| {
                    theme::primary(ui, "登录外星仔")
                })
                .inner
                .clicked()
            {
                let user = if self.login_mode == LoginMode::Sms {
                    self.sms_phone.trim().to_owned()
                } else if self.user.trim().is_empty() {
                    account.username.clone()
                } else {
                    self.user.trim().to_owned()
                };
                let password = std::mem::take(&mut self.password);
                let imported = std::mem::take(&mut self.token_input).trim().to_owned();
                let code = std::mem::take(&mut self.sms_code);
                let mode = self.login_mode;
                let device = self.device(&account);
                self.start(ui.ctx().clone(), move || {
                    let device = device?;
                    let token = match mode {
                        LoginMode::Sms => {
                            auth::login(&user, &code, &device).map_err(|e| e.to_string())?
                        }
                        LoginMode::Token => imported,
                        LoginMode::Password => api::login(&user, &password, &device)?,
                    };
                    api::info(&token, &device)?;
                    Ok(Event::Login {
                        token,
                        device,
                        user,
                    })
                });
            }
            if self.login_mode == LoginMode::Sms {
                ui.label(egui::RichText::new("验证码由外星仔官方发送和校验；未注册的手机号可能按官方规则创建账号。").small().color(theme::MUTED));
            }
            ui.label(egui::RichText::new("登录令牌由 Windows 加密保存；密码和短信验证码不保存。").small().color(theme::MUTED));
            ui.collapsing("首次使用与服务器保护", |ui| {
                ui.label("外星仔加速器 · 本机自动暂停（试验性接入）");
                ui.label("使用游戏名单、游戏退出宽限期和启动保护。服务器失联保护可在“平台账号”页单独授权开启。");
                ui.label("首次使用请核对外星仔官方计时状态。");
            });
            ui.separator();
            let token = self.shared.lock().ok().and_then(|s| s.token.clone());
            ui.add_enabled_ui(token.is_some(), |ui| {
                ui.label("首次配置：先在官方客户端点击暂停并刷新，确认已经暂停，再读取状态。");
                ui.horizontal_wrapped(|ui| {
                    let calibrate = ui.button("我已在官方暂停，读取并校准").clicked();
                    let refresh = ui.button("刷新状态").clicked();
                    if calibrate || refresh {
                        if calibrate {
                            platform.revoke_etalien();
                        }
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
            platform.revoke_etalien();
            self.events = None;
            self.password.clear();
            self.sms_code.clear();
            self.token_input.clear();
            self.user.clear();
            self.sms_phone.clear();
            self.login_device.clear();
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
        if self.login_mode == LoginMode::Sms && self.retry_seconds() > 0 {
            ui.ctx().request_repaint_after(Duration::from_secs(1));
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
