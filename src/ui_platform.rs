//! Native platform account panel. Rendering emits actions; only the live update
//! loop executes them. Offscreen fixtures never read credentials or use the network.
use crate::platform_api::{Api, CodeChallenge, Error, Session, ORIGIN_URL};
use crate::platform_store::Store;
use crate::ui_theme as theme;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Login,
    SendCode,
    Bind,
    Pair,
    StopDevice,
    Refresh,
    Logout,
}
#[derive(Clone, Copy)]
enum Kind {
    SendCode,
    Login,
    Validate,
    Logout,
}
enum Completed {
    Code(String, CodeChallenge),
    Identity(Session),
    Logout,
}
struct Pending {
    kind: Kind,
    started: Instant,
    receiver: Receiver<Result<Completed, Error>>,
}

pub(crate) struct Panel {
    pub(crate) username: String,
    pub(crate) password: String,
    pub(crate) email_mode: bool,
    code: String,
    challenge: Option<(String, CodeChallenge)>,
    next_code: Instant,
    pair_code: String,
    agent: Option<crate::platform_device::Agent>,
    pub(crate) remember: bool,
    pub(crate) message: String,
    pub(crate) error: bool,
    pub(crate) session: Option<Session>,
    pub(crate) verified: bool,
    pub(crate) action: Option<Action>,
    pending: Option<Pending>,
    logout_retry: Option<Session>,
    cleanup_failed: bool,
    next_check: Instant,
}
impl Default for Panel {
    fn default() -> Self {
        Self {
            username: String::new(),
            password: String::new(),
            email_mode: true,
            code: String::new(),
            challenge: None,
            next_code: Instant::now(),
            pair_code: String::new(),
            agent: None,
            remember: false,
            message: "尚未登录平台；本地守护可正常使用。".into(),
            error: false,
            session: None,
            verified: false,
            action: None,
            pending: None,
            logout_retry: None,
            cleanup_failed: false,
            next_check: Instant::now(),
        }
    }
}
impl Panel {
    pub(crate) fn start_device_agent(
        &mut self,
        shared: std::sync::Arc<std::sync::Mutex<crate::shared::Shared>>,
        config: std::sync::Arc<std::sync::Mutex<crate::config::Config>>,
        ctx: egui::Context,
    ) {
        self.agent = Some(crate::platform_device::Agent::start(shared, config, ctx));
    }

    pub(crate) fn restore(&mut self, store: &impl Store, ctx: &egui::Context) {
        match store.load() {
            Ok(Some(session)) => {
                self.username = session.user.username.clone();
                self.remember = true;
                self.session = Some(session);
                self.dispatch(Action::Refresh, store, ctx);
            }
            Ok(None) => {}
            Err(message) => {
                self.cleanup_failed = store.clear().is_err();
                self.message = message;
                if self.cleanup_failed {
                    self.message
                        .push_str(" 本机旧登录文件未能清除，请点击退出重试。");
                }
                self.error = true;
            }
        }
    }
    fn start(
        &mut self,
        kind: Kind,
        ctx: &egui::Context,
        work: impl FnOnce() -> Result<Completed, Error> + Send + 'static,
    ) {
        let (sender, receiver) = mpsc::channel();
        let ctx = ctx.clone();
        let handle = std::thread::Builder::new()
            .name("platform-account".into())
            .spawn(move || {
                let result = work();
                // If the application discarded a late login, revoke it best-effort.
                if let Err(mpsc::SendError(Ok(Completed::Identity(session)))) = sender.send(result)
                {
                    if matches!(kind, Kind::Login) {
                        if let Ok(api) = Api::new() {
                            let _ = api.logout(&session);
                        }
                    }
                }
                ctx.request_repaint();
            });
        if handle.is_ok() {
            self.pending = Some(Pending {
                kind,
                receiver,
                started: Instant::now(),
            });
        } else {
            self.message = "无法开始平台请求，请稍后重试。".into();
            self.error = true;
        }
    }
    pub(crate) fn dispatch(&mut self, action: Action, store: &impl Store, ctx: &egui::Context) {
        if self.pending.is_some() {
            return;
        }
        self.error = false;
        match action {
            Action::SendCode => {
                if Instant::now() < self.next_code {
                    return;
                }
                let email = self.username.trim().to_ascii_lowercase();
                if !Api::valid_email(&email) {
                    self.message = "请输入有效邮箱。".into();
                    self.error = true;
                    return;
                }
                self.message = "正在发送验证码…".into();
                self.start(Kind::SendCode, ctx, move || {
                    Api::new()?
                        .send_code(&email)
                        .map(|c| Completed::Code(email, c))
                });
            }
            Action::Bind => {
                if self.verified {
                    if let (Some(agent), Some(session)) = (&self.agent, &self.session) {
                        agent.bind(session.clone());
                    }
                }
            }
            Action::Pair => {
                let code = self.pair_code.trim().to_owned();
                if code.len() != 32 || !code.bytes().all(|b| b.is_ascii_hexdigit()) {
                    self.error = true;
                    self.message = "请粘贴网页生成的完整配对码。".into();
                    return;
                }
                if let Some(agent) = &self.agent {
                    agent.pair(code, self.session.clone().filter(|_| self.verified));
                    self.pair_code.clear();
                }
            }
            Action::StopDevice => {
                if let Some(agent) = &self.agent {
                    agent.stop();
                }
            }
            Action::Login => {
                let valid = if self.email_mode {
                    Api::valid_email(&self.username)
                        && self.code.len() == 6
                        && self.code.bytes().all(|b| b.is_ascii_digit())
                        && self.challenge.as_ref().is_some_and(|(email, _)| {
                            email == &self.username.trim().to_ascii_lowercase()
                        })
                } else {
                    Api::valid_input(&self.username, &self.password)
                };
                if !valid {
                    self.message = Error::InvalidInput.message().into();
                    self.error = true;
                    return;
                }
                // Never leave an older account on disk when replacing its session.
                if let Err(message) = store.clear() {
                    self.cleanup_failed = true;
                    self.message = message;
                    self.error = true;
                    return;
                }
                self.cleanup_failed = false;
                self.logout_retry = None;
                let username = self.username.trim().to_owned();
                let password = std::mem::take(&mut self.password);
                self.message = "正在登录上海平台…".into();
                let email_mode = self.email_mode;
                let request_id = self
                    .challenge
                    .as_ref()
                    .map(|(_, c)| c.request_id.clone())
                    .unwrap_or_default();
                let code = std::mem::take(&mut self.code);
                let remember = self.remember;
                self.start(Kind::Login, ctx, move || {
                    let api = Api::new()?;
                    if email_mode {
                        api.login_code(&username, &request_id, &code, remember)
                    } else {
                        api.login(&username, &password, remember)
                    }
                    .map(Completed::Identity)
                });
            }
            Action::Refresh => {
                let Some(session) = self.session.clone() else {
                    return;
                };
                self.verified = false;
                self.message = "正在校验平台登录状态…".into();
                self.start(Kind::Validate, ctx, move || {
                    Api::new()?.validate(&session).map(Completed::Identity)
                });
            }
            Action::Logout => {
                if let Some(agent) = &self.agent {
                    agent.stop();
                }
                self.code.clear();
                self.challenge = None;
                let clearing = store.clear();
                self.cleanup_failed = clearing.is_err();
                self.password.clear();
                self.verified = false;
                self.logout_retry = self.session.take().or(self.logout_retry.take());
                if let Some(session) = self.logout_retry.clone() {
                    self.message = "正在撤销平台会话…".into();
                    self.start(Kind::Logout, ctx, move || {
                        Api::new()?.logout(&session).map(|()| Completed::Logout)
                    });
                } else {
                    self.message = clearing.err().unwrap_or_else(|| "已退出平台账号。".into());
                    self.error = self.cleanup_failed;
                }
            }
        }
    }
    fn complete(
        &mut self,
        kind: Kind,
        result: Result<Completed, Error>,
        store: &impl Store,
        now: Instant,
    ) {
        self.next_check = now + Duration::from_secs(60);
        match result {
            Ok(Completed::Code(email, challenge)) => {
                self.next_code = now + Duration::from_secs(challenge.retry_after.into());
                self.challenge = Some((email, challenge));
                self.message = "验证码已发送，10 分钟内有效，请检查收件箱和垃圾邮件。".into();
                self.error = false;
            }
            Ok(Completed::Identity(session)) => {
                self.username = session.user.username.clone();
                self.message = "平台登录有效。".into();
                self.error = false;
                if self.remember {
                    if let Err(message) = store.save(&session) {
                        self.cleanup_failed = store.clear().is_err();
                        self.message = if self.cleanup_failed {
                            "无法更新或清除本机登录文件，请退出后重试。".into()
                        } else {
                            message
                        };
                        self.error = true;
                    }
                }
                if let Some(agent) = &self.agent {
                    agent.session(session.clone(), matches!(kind, Kind::Login));
                }
                self.session = Some(session);
                self.verified = true;
            }
            Ok(Completed::Logout) => {
                self.logout_retry = None;
                self.error = self.cleanup_failed;
                self.message = if self.cleanup_failed {
                    "平台会话已撤销，但本机文件未能清除，请点击退出重试。"
                } else {
                    "已退出平台账号，本地守护保持原有状态。"
                }
                .into();
            }
            Err(error) => {
                self.error = true;
                self.verified = false;
                if matches!(kind, Kind::Logout) {
                    self.message = if self.cleanup_failed {
                        "尚未确认退出：本机文件清除和服务器会话撤销均未完成，请重试退出。"
                    } else {
                        "本机已退出，服务器撤销暂未确认。可重试退出；原会话最多 30 天后过期。"
                    }
                    .into();
                } else if error.invalidates_session() {
                    self.session = None;
                    self.cleanup_failed = store.clear().is_err();
                    self.message = error.message().into();
                    if self.cleanup_failed {
                        self.message
                            .push_str(" 本机旧登录文件未能清除，请点击退出重试。");
                    }
                } else {
                    // Network errors retain the encrypted session for an explicit
                    // retry; they never claim that a cached identity is verified.
                    self.message = error.message().into();
                }
            }
        }
    }
    pub(crate) fn tick(&mut self, store: &impl Store, ctx: &egui::Context, active: bool) {
        let now = Instant::now();
        if let Some(pending) = &self.pending {
            let result = match pending.receiver.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Disconnected) => Some(Err(Error::Network)),
                Err(mpsc::TryRecvError::Empty)
                    if pending.started.elapsed() >= Duration::from_secs(25) =>
                {
                    Some(Err(Error::Network))
                }
                Err(mpsc::TryRecvError::Empty) => None,
            };
            if let Some(result) = result {
                let kind = self.pending.take().unwrap().kind;
                self.complete(kind, result, store, now);
            }
        }
        if self.pending.is_none()
            && self
                .session
                .as_ref()
                .is_some_and(|s| s.expires_at <= chrono::Utc::now().timestamp())
        {
            self.complete(Kind::Validate, Err(Error::Unauthorized), store, now);
        }
        if self.pending.is_none() && self.session.is_some() && active && now >= self.next_check {
            self.dispatch(Action::Refresh, store, ctx);
        }
    }
    pub(crate) fn render(&mut self, ui: &mut egui::Ui) {
        let busy = self.pending.is_some();
        ui.label(theme::title("守护平台账号", 20.0));
        ui.label(
            egui::RichText::new("邮箱验证码登录后自动绑定本机；首次登录自动创建普通用户账号。")
                .color(theme::MUTED),
        );
        ui.add_space(16.0);
        if let Some(session) = &self.session {
            ui.label(theme::title(
                if self.verified {
                    "已登录平台"
                } else {
                    "登录状态待确认"
                },
                17.0,
            ));
            ui.label(format!("账号：{}", session.user.username));
            ui.label(format!("显示名称：{}", session.user.display_name));
            if self.verified {
                ui.label(if session.user.role == "admin" {
                    "角色：管理员"
                } else {
                    "角色：普通用户"
                });
            }
            if let Some(expires) = chrono::DateTime::from_timestamp(session.expires_at, 0) {
                ui.label(format!(
                    "会话到期：{}",
                    expires.with_timezone(&chrono::Local).format("%m-%d %H:%M")
                ));
            }
            ui.add_space(12.0);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(!busy, egui::Button::new("刷新登录状态"))
                    .clicked()
                {
                    self.action = Some(Action::Refresh);
                }
                if ui
                    .add_enabled(!busy, egui::Button::new("退出平台账号"))
                    .clicked()
                {
                    self.action = Some(Action::Logout);
                }
            });
        } else {
            ui.add_enabled_ui(!busy, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.email_mode, true, "邮箱验证码");
                    ui.selectable_value(&mut self.email_mode, false, "账号密码");
                });
                ui.add_space(8.0);
                ui.label(if self.email_mode {
                    "邮箱"
                } else {
                    "平台账号"
                });
                ui.add(
                    egui::TextEdit::singleline(&mut self.username)
                        .id_salt("platform-user")
                        .hint_text(if self.email_mode {
                            "用于接收验证码的邮箱"
                        } else {
                            "已有账号或管理员账号"
                        })
                        .desired_width(ui.available_width().min(400.0))
                        .char_limit(254),
                );
                ui.add_space(8.0);
                let credential = if self.email_mode {
                    let remaining = self
                        .next_code
                        .saturating_duration_since(Instant::now())
                        .as_secs();
                    if ui
                        .add_enabled(
                            remaining == 0,
                            egui::Button::new(if remaining == 0 {
                                "发送验证码".into()
                            } else {
                                format!("{} 秒后重发", remaining + 1)
                            }),
                        )
                        .clicked()
                    {
                        self.action = Some(Action::SendCode);
                    }
                    if remaining > 0 {
                        ui.ctx().request_repaint_after(Duration::from_secs(1));
                    }
                    ui.label("6 位验证码");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.code)
                            .id_salt("platform-code")
                            .char_limit(6)
                            .desired_width(180.0),
                    )
                } else {
                    ui.label("平台密码");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.password)
                            .id_salt("platform-password")
                            .password(true)
                            .desired_width(ui.available_width().min(400.0))
                            .char_limit(128),
                    )
                };
                ui.add_space(8.0);
                ui.checkbox(&mut self.remember, "记住登录状态（30 天）");
                ui.label(
                    egui::RichText::new("仅在当前 Windows 用户下加密保存会话，不保存平台密码。")
                        .size(12.0)
                        .color(theme::MUTED),
                );
                ui.add_space(12.0);
                if ui.button("登录平台").clicked()
                    || (credential.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                {
                    self.action = Some(Action::Login);
                }
                if (self.logout_retry.is_some() || self.cleanup_failed)
                    && ui.button("重试退出平台账号").clicked()
                {
                    self.action = Some(Action::Logout);
                }
            });
        }
        ui.add_space(12.0);
        if busy {
            ui.spinner();
        }
        ui.label(egui::RichText::new(&self.message).color(if self.error {
            egui::Color32::from_rgb(179, 49, 59)
        } else {
            theme::MUTED
        }));
        ui.add_space(18.0);
        ui.separator();
        ui.label(theme::title("本机设备", 17.0));
        let view = self
            .agent
            .as_ref()
            .and_then(|a| a.view.lock().ok().map(|v| v.clone()))
            .unwrap_or_default();
        if !view.owner.is_empty() {
            ui.label(format!("所属账号：{}", view.owner));
        }
        if !view.device_id.is_empty() {
            ui.label(format!("设备 ID：{}", view.device_id));
        }
        if !view.account.is_empty() {
            ui.label(format!("已关联：{}", view.account));
        }
        ui.label(if view.message.is_empty() {
            "登录平台后自动绑定，也可从网页获取配对码。"
        } else {
            &view.message
        });
        ui.add_enabled_ui(!busy && !view.busy, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(self.verified, egui::Button::new("重新绑定本机"))
                    .clicked()
                {
                    self.action = Some(Action::Bind);
                }
                if ui
                    .add_enabled(view.active, egui::Button::new("停止本机上报"))
                    .clicked()
                {
                    self.action = Some(Action::StopDevice);
                }
            });
            ui.add_space(8.0);
            ui.label("手动添加：在网页“配对设备”生成配对码，然后粘贴到这里。");
            ui.add(
                egui::TextEdit::singleline(&mut self.pair_code)
                    .id_salt("platform-pair")
                    .hint_text("一次性配对码")
                    .char_limit(32)
                    .desired_width(ui.available_width().min(400.0)),
            );
            if ui.button("配对本机").clicked() {
                self.action = Some(Action::Pair);
            }
        });
        ui.label(
            egui::RichText::new(
                "设备授权独立保存；退出平台账号会停止本机上报。可在网页撤销或解除设备绑定。",
            )
            .size(12.0)
            .color(theme::MUTED),
        );
        ui.add_space(18.0);
        ui.separator();
        ui.add_space(10.0);
        ui.horizontal_wrapped(|ui| {
            ui.label("上海节点");
            ui.hyperlink_to("打开后台", ORIGIN_URL);
        });
        ui.label(
            egui::RichText::new(
                "普通用户使用邮箱验证码注册／登录；管理员仍可使用账号密码。平台登录可选。",
            )
            .size(13.0)
            .color(theme::MUTED),
        );
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new("设备绑定后上报在线和游戏运行状态，并关联当前雷神账号的脱敏标识。自动暂停仍由本机执行。")
                .size(13.0)
                .color(theme::MUTED),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    #[derive(Default)]
    struct MemoryStore {
        session: RefCell<Option<Session>>,
        fail_write: bool,
        fail_clear: bool,
    }
    impl Store for MemoryStore {
        fn load(&self) -> Result<Option<Session>, String> {
            Ok(self.session.borrow().clone())
        }
        fn save(&self, s: &Session) -> Result<(), String> {
            if self.fail_write {
                Err("保存失败".into())
            } else {
                *self.session.borrow_mut() = Some(s.clone());
                Ok(())
            }
        }
        fn clear(&self) -> Result<(), String> {
            if self.fail_clear {
                Err("清除失败".into())
            } else {
                *self.session.borrow_mut() = None;
                Ok(())
            }
        }
    }
    fn session() -> Session {
        Session {
            origin: ORIGIN_URL.into(),
            token: "a".repeat(64),
            expires_at: chrono::Utc::now().timestamp() + 3600,
            user: crate::platform_api::User {
                id: "test-user".into(),
                username: "test-user".into(),
                display_name: "测试用户".into(),
                role: "user".into(),
                csrf: "b".repeat(64),
            },
        }
    }
    #[test]
    fn remember_is_opt_in_and_keeps_password_out_of_storage() {
        let store = MemoryStore::default();
        let mut panel = Panel::default();
        panel.complete(
            Kind::Login,
            Ok(Completed::Identity(session())),
            &store,
            Instant::now(),
        );
        assert!(panel.verified);
        assert!(store.session.borrow().is_none());
        panel.remember = true;
        panel.complete(
            Kind::Login,
            Ok(Completed::Identity(session())),
            &store,
            Instant::now(),
        );
        assert!(store.session.borrow().is_some());
        let json = serde_json::to_string(store.session.borrow().as_ref().unwrap()).unwrap();
        assert!(!json.contains("password"));
    }
    #[test]
    fn network_failure_preserves_session_but_revocation_clears_it() {
        let store = MemoryStore::default();
        let mut panel = Panel {
            remember: true,
            ..Panel::default()
        };
        panel.complete(
            Kind::Login,
            Ok(Completed::Identity(session())),
            &store,
            Instant::now(),
        );
        panel.complete(Kind::Validate, Err(Error::Network), &store, Instant::now());
        assert!(!panel.verified);
        assert!(panel.session.is_some());
        assert!(store.session.borrow().is_some());
        panel.complete(
            Kind::Validate,
            Err(Error::Unauthorized),
            &store,
            Instant::now(),
        );
        assert!(panel.session.is_none());
        assert!(store.session.borrow().is_none());
    }
    #[test]
    fn storage_failure_is_reported_without_claiming_remembered_login() {
        let store = MemoryStore {
            fail_write: true,
            ..MemoryStore::default()
        };
        let mut panel = Panel {
            remember: true,
            ..Panel::default()
        };
        panel.complete(
            Kind::Login,
            Ok(Completed::Identity(session())),
            &store,
            Instant::now(),
        );
        assert!(panel.verified);
        assert!(panel.error);
        assert!(store.session.borrow().is_none());
    }
    #[test]
    fn failed_logout_never_restores_local_identity_and_can_retry_revocation() {
        let store = MemoryStore::default();
        let mut panel = Panel {
            logout_retry: Some(session()),
            ..Panel::default()
        };
        panel.complete(Kind::Logout, Err(Error::Network), &store, Instant::now());
        assert!(panel.session.is_none());
        assert!(!panel.verified);
        assert!(panel.logout_retry.is_some());
        assert!(panel.message.contains("撤销暂未确认"));
        panel.complete(Kind::Logout, Ok(Completed::Logout), &store, Instant::now());
        assert!(panel.logout_retry.is_none());
    }
    #[test]
    fn lost_or_late_worker_result_cannot_restore_a_timed_out_login() {
        let store = MemoryStore::default();
        let (sender, receiver) = mpsc::channel();
        let mut panel = Panel {
            pending: Some(Pending {
                kind: Kind::Login,
                started: Instant::now() - Duration::from_secs(30),
                receiver,
            }),
            ..Panel::default()
        };
        panel.tick(&store, &egui::Context::default(), false);
        assert!(panel.pending.is_none());
        assert!(sender.send(Ok(Completed::Identity(session()))).is_err());
        assert!(panel.session.is_none());
        assert!(!panel.verified);
    }
    #[test]
    fn restoring_without_saved_session_performs_no_request() {
        let mut panel = Panel::default();
        panel.restore(&MemoryStore::default(), &egui::Context::default());
        assert!(panel.pending.is_none());
        assert!(panel.session.is_none());
    }
}
