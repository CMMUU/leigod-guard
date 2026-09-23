//! Device heartbeats and explicitly consented remote protection run independently of windows.
use crate::platform_api::{
    AccountLink, Api, DeviceBinding, Error, Heartbeat, Provider, RemoteCredential, RemoteStatus,
    Session, ORIGIN_URL,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Serialize, Deserialize)]
struct Saved {
    origin: String,
    installation_key: String,
    binding: Option<DeviceBinding>,
    active: bool,
    #[serde(default)]
    paused: bool,
    reserved_until: i64,
    #[serde(flatten)]
    leigod: RemoteSaved,
    #[serde(default)]
    etalien: RemoteSaved,
}
#[derive(Clone, Default, Serialize, Deserialize)]
struct RemoteSaved {
    #[serde(default)]
    remote_consent: bool,
    #[serde(default)]
    pending_disable: bool,
    #[serde(default)]
    remote_revision: i64,
    #[serde(default)]
    remote_token_digest: String,
}
impl std::ops::Index<Provider> for Saved {
    type Output = RemoteSaved;
    fn index(&self, p: Provider) -> &RemoteSaved {
        match p {
            Provider::Leigod => &self.leigod,
            Provider::Etalien => &self.etalien,
        }
    }
}
impl std::ops::IndexMut<Provider> for Saved {
    fn index_mut(&mut self, p: Provider) -> &mut RemoteSaved {
        match p {
            Provider::Leigod => &mut self.leigod,
            Provider::Etalien => &mut self.etalien,
        }
    }
}

impl Saved {
    fn fresh() -> Result<Self, String> {
        let mut bytes = [0u8; 32];
        getrandom::getrandom(&mut bytes).map_err(|_| "无法生成设备身份。")?;
        Ok(Self {
            origin: ORIGIN_URL.into(),
            installation_key: bytes.iter().map(|b| format!("{b:02x}")).collect(),
            binding: None,
            active: false,
            paused: false,
            reserved_until: 0,
            leigod: RemoteSaved::default(),
            etalien: RemoteSaved::default(),
        })
    }
    fn valid(&self) -> bool {
        self.origin == ORIGIN_URL
            && self.installation_key.len() == 64
            && self.installation_key.bytes().all(|b| b.is_ascii_hexdigit())
            && Provider::ALL.into_iter().all(|p| {
                self[p].remote_revision >= 0
                    && self[p].remote_revision < i64::MAX
                    && self[p].remote_token_digest.len() <= 64
            })
            && self.reserved_until >= 0
            && self.reserved_until < i64::MAX - 1024
            && self.binding.as_ref().is_none_or(DeviceBinding::valid)
    }
}
struct Store(PathBuf);
impl Store {
    fn current() -> Self {
        Self(crate::config::Config::path().with_file_name("platform-device.dat"))
    }
    fn load(&self) -> Result<Saved, String> {
        use std::io::Read;
        match std::fs::File::open(&self.0) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Saved::fresh(),
            Err(_) => Err("无法读取本机设备身份。".into()),
            Ok(file) => {
                let mut text = String::new();
                file.take(32769)
                    .read_to_string(&mut text)
                    .map_err(|_| "无法读取本机设备身份。")?;
                if text.len() > 32768 {
                    return Err("设备身份文件无效。".into());
                }
                let plain = crate::dpapi::unprotect(&text)
                    .map_err(|_| "设备身份无法解密，请使用原 Windows 用户。")?;
                let saved: Saved =
                    serde_json::from_str(&plain).map_err(|_| "设备身份文件无效。")?;
                if !saved.valid() {
                    return Err("设备身份文件无效。".into());
                }
                Ok(saved)
            }
        }
    }
    fn save(&self, saved: &Saved) -> Result<(), String> {
        use std::io::Write;
        if !saved.valid() {
            return Err("设备身份无效，未保存。".into());
        }
        let cipher = crate::dpapi::protect(
            &serde_json::to_string(saved).map_err(|_| "设备身份序列化失败。")?,
        )
        .map_err(|_| "无法加密设备身份。")?;
        let temp = self.0.with_extension("tmp");
        let result = (|| -> std::io::Result<()> {
            if let Some(p) = self.0.parent() {
                std::fs::create_dir_all(p)?;
            }
            let mut f = std::fs::File::create(&temp)?;
            f.write_all(cipher.as_bytes())?;
            f.sync_all()?;
            drop(f);
            std::fs::rename(&temp, &self.0)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(temp);
            return Err("无法保存设备身份，已停止本机上报。".into());
        }
        Ok(())
    }
}
#[derive(Clone, Default)]
pub(crate) struct View {
    pub message: String,
    pub owner: String,
    pub device_id: String,
    pub account: String,
    pub active: bool,
    pub busy: bool,
    pub remote: [RemoteView; 2],
}
#[derive(Clone, Default)]
pub(crate) struct RemoteView {
    pub status: RemoteStatus,
    pub message: String,
    pub error: String,
    pub pending_disable: bool,
}
enum Command {
    Session(Session, bool),
    Bind(Session),
    Pair(String, Option<Session>),
    Stop,
    Remote(Provider, bool),
}
pub(crate) struct Agent {
    sender: mpsc::Sender<Command>,
    pub view: Arc<Mutex<View>>,
}
impl Agent {
    #[cfg(test)]
    pub(crate) fn fixture(view: View) -> Self {
        let (sender, _) = mpsc::channel();
        Self {
            sender,
            view: Arc::new(Mutex::new(view)),
        }
    }

    pub fn start(
        shared: Arc<Mutex<crate::shared::Shared>>,
        etalien_shared: Arc<Mutex<crate::shared::Shared>>,
        config: Arc<Mutex<crate::config::Config>>,
        ctx: egui::Context,
    ) -> Self {
        let (sender, receiver) = mpsc::channel();
        let view = Arc::new(Mutex::new(View {
            message: "本机尚未绑定平台。".into(),
            ..View::default()
        }));
        let status = view.clone();
        let handle = std::thread::Builder::new()
            .name("platform-device".into())
            .spawn(move || run(receiver, status, shared, etalien_shared, config, ctx));
        if handle.is_err() {
            if let Ok(mut v) = view.lock() {
                v.message = "设备连接线程未能启动。".into();
            }
        }
        Self { sender, view }
    }
    pub fn session(&self, session: Session, fresh_login: bool) {
        let _ = self.sender.send(Command::Session(session, fresh_login));
    }
    pub fn bind(&self, session: Session) {
        let _ = self.sender.send(Command::Bind(session));
    }
    pub fn pair(&self, code: String, session: Option<Session>) {
        let _ = self.sender.send(Command::Pair(code, session));
    }
    fn persist_disable(&self, kind: Provider) {
        let result = write_disable_marker(kind);
        if let Ok(mut view) = self.view.lock() {
            view.remote[kind.index()].pending_disable = true;
            view.remote[kind.index()].message = if result.is_ok() {
                "服务器关闭未确认，可能仍会超时暂停；正在重试。"
            } else {
                "无法保存关闭请求，请在网页关闭保护或撤销设备。"
            }
            .into();
        }
    }
    pub fn remote(&self, kind: Provider, enabled: bool) {
        if !enabled {
            self.persist_disable(kind);
        }
        let _ = self.sender.send(Command::Remote(kind, enabled));
    }
    pub fn stop(&self) {
        for kind in Provider::ALL {
            self.persist_disable(kind);
        }
        let _ = self.sender.send(Command::Stop);
    }
}
fn publish(view: &Arc<Mutex<View>>, ctx: &egui::Context, saved: &Saved, message: &str, busy: bool) {
    if let Ok(mut v) = view.lock() {
        v.message = message.into();
        v.active = saved.active;
        v.busy = busy;
        for kind in Provider::ALL {
            v.remote[kind.index()].pending_disable = saved[kind].pending_disable;
            if saved[kind].pending_disable {
                v.remote[kind.index()].message =
                    "服务器关闭未确认，可能仍会超时暂停；正在重试。".into();
            }
        }
        v.owner = saved
            .binding
            .as_ref()
            .map(|b| b.owner.clone())
            .unwrap_or_default();
        v.device_id = saved
            .binding
            .as_ref()
            .map(|b| b.device_id.clone())
            .unwrap_or_default();
    }
    ctx.request_repaint();
}
fn run(
    receiver: mpsc::Receiver<Command>,
    view: Arc<Mutex<View>>,
    shared: Arc<Mutex<crate::shared::Shared>>,
    etalien_shared: Arc<Mutex<crate::shared::Shared>>,
    config: Arc<Mutex<crate::config::Config>>,
    ctx: egui::Context,
) {
    let store = Store::current();
    let mut saved = match store.load() {
        Ok(s) => s,
        Err(e) => {
            if let Ok(mut v) = view.lock() {
                v.message = e;
            }
            ctx.request_repaint();
            return;
        }
    };
    for kind in Provider::ALL {
        if read_disable_marker(kind).is_some() {
            saved[kind].pending_disable = true;
            saved[kind].remote_consent = false;
        }
    }
    let mut sequence = saved.reserved_until;
    let run_generation = sequence.saturating_add(1);
    saved.reserved_until = run_generation.saturating_add(256);
    let mut enable_requested = [false; 2];
    let mut run_confirmed = false;
    // Persist a new run generation before emitting any heartbeat. Older instances cannot report.
    // Persist the installation identity before the first registration request.
    if let Err(e) = store.save(&saved) {
        publish(&view, &ctx, &saved, &e, false);
        return;
    }
    let api = match Api::new() {
        Ok(a) => a,
        Err(e) => {
            publish(&view, &ctx, &saved, e.message(), false);
            return;
        }
    };
    let name = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Windows 设备".into());
    let name: String = name.chars().filter(|c| !c.is_control()).take(30).collect();
    let mut next = Instant::now();
    let mut blocked = false;
    let mut last_session: Option<Session> = None;
    publish(
        &view,
        &ctx,
        &saved,
        if saved.active {
            "正在恢复设备连接…"
        } else {
            "登录平台后自动绑定，也可使用配对码。"
        },
        false,
    );
    loop {
        let wait = if saved.active || Provider::ALL.into_iter().any(|p| saved[p].pending_disable) {
            next.saturating_duration_since(Instant::now())
                .min(Duration::from_secs(15))
        } else {
            Duration::from_secs(60)
        };
        let mut command = match receiver.recv_timeout(wait) {
            Ok(c) => Some(c),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        if command.is_none() && !saved.active && !saved.paused && !blocked {
            command = last_session
                .clone()
                .map(|session| Command::Session(session, false));
        }
        if let Some(command) = command {
            match command {
                Command::Remote(kind, enabled) => {
                    if enabled && !saved.active {
                        publish(
                            &view,
                            &ctx,
                            &saved,
                            "请先恢复本机设备连接，再开启远程保护。",
                            false,
                        );
                        continue;
                    }
                    if let Ok(mut v) = view.lock() {
                        v.remote[kind.index()].error.clear();
                    }
                    enable_requested[kind.index()] = enabled;
                    if !enabled {
                        saved[kind].remote_consent = false;
                        saved[kind].pending_disable = true;
                    }
                    let message = store.save(&saved).err().unwrap_or_else(|| {
                        if enabled {
                            "正在请求远程保护授权…".into()
                        } else {
                            "正在关闭服务器远程保护…".into()
                        }
                    });
                    publish(&view, &ctx, &saved, &message, enabled);
                }
                Command::Stop => {
                    enable_requested = [false; 2];
                    for kind in Provider::ALL {
                        saved[kind].remote_consent = false;
                        saved[kind].pending_disable = true;
                    }
                    last_session = None;
                    saved.active = false;
                    saved.paused = true;
                    let message = store.save(&saved).err().unwrap_or_else(|| {
                        "已停止本机上报；正在确认关闭服务器保护，未确认前可能仍超时暂停。".into()
                    });
                    publish(&view, &ctx, &saved, &message, false);
                }
                Command::Session(session, fresh_login) => {
                    if !session.valid_for(ORIGIN_URL, chrono::Utc::now().timestamp()) {
                        last_session = None;
                        continue;
                    }
                    last_session = Some(session.clone());
                    if fresh_login {
                        saved.paused = false;
                    }
                    if saved
                        .binding
                        .as_ref()
                        .is_some_and(|b| b.owner_id != session.user.id)
                    {
                        saved.active = false;
                        for kind in Provider::ALL {
                            saved[kind].remote_consent = false;
                            saved[kind].pending_disable = true;
                            let _ = write_disable_marker(kind);
                        }
                        let _ = store.save(&saved);
                        publish(
                            &view,
                            &ctx,
                            &saved,
                            "本机属于其他平台账号；请在原账号网页撤销并解除，再手动绑定。",
                            false,
                        );
                        continue;
                    }
                    if saved.active || blocked || saved.paused {
                        continue;
                    }
                    publish(&view, &ctx, &saved, "正在自动绑定本机…", true);
                    apply_binding(
                        &api,
                        &store,
                        &mut saved,
                        &mut sequence,
                        &mut blocked,
                        &view,
                        &ctx,
                        |a, s| a.register_device(&session, &s.installation_key, &name, false),
                    );
                }
                Command::Bind(session) => {
                    saved.paused = false;
                    publish(&view, &ctx, &saved, "正在重新绑定本机…", true);
                    apply_binding(
                        &api,
                        &store,
                        &mut saved,
                        &mut sequence,
                        &mut blocked,
                        &view,
                        &ctx,
                        |a, s| a.register_device(&session, &s.installation_key, &name, true),
                    );
                }
                Command::Pair(code, session) => {
                    saved.paused = false;
                    publish(&view, &ctx, &saved, "正在手动配对设备…", true);
                    apply_binding(
                        &api,
                        &store,
                        &mut saved,
                        &mut sequence,
                        &mut blocked,
                        &view,
                        &ctx,
                        |a, s| {
                            a.pair_device(code.trim(), &s.installation_key, &name, session.as_ref())
                        },
                    );
                }
            }
            next = Instant::now();
        }
        if Instant::now() >= next
            && (saved.active || Provider::ALL.into_iter().any(|p| saved[p].pending_disable))
        {
            if let Some(binding) = saved.binding.clone() {
                for kind in Provider::ALL {
                    sync_remote(
                        &api,
                        &store,
                        &mut saved,
                        &binding,
                        &view,
                        &ctx,
                        &shared,
                        if run_confirmed { run_generation } else { 0 },
                        &config,
                        kind,
                    );
                }
            } else {
                for kind in Provider::ALL {
                    saved[kind].pending_disable = false;
                }
                let _ = store.save(&saved);
            }
            if !saved.active {
                next = Instant::now() + Duration::from_secs(15);
            }
        }
        if !saved.active || Instant::now() < next {
            continue;
        }
        next = Instant::now() + Duration::from_secs(15);
        let Some(binding) = saved.binding.clone() else {
            saved.active = false;
            continue;
        };
        // Reserve a range durably before sending. Crashes skip counters, never reuse them.
        if sequence >= saved.reserved_until {
            saved.reserved_until = sequence.saturating_add(256);
            if let Err(e) = store.save(&saved) {
                saved.active = false;
                blocked = true;
                publish(&view, &ctx, &saved, &e, false);
                continue;
            }
        }
        let mut payload = snapshot(&shared, &etalien_shared, &config, sequence);
        payload.run_generation = run_generation;
        payload.remote_revision = Some(saved[Provider::Leigod].remote_revision);
        payload.etalien_revision = Some(saved[Provider::Etalien].remote_revision);
        // During startup/re-login, missing local credentials do not silently revoke
        // an existing consent. Explicit logout/switch dispatches Remote(false).
        if saved[Provider::Leigod].remote_consent && payload.account_action == "clear" {
            payload.account_action = "keep".into();
        }
        sequence += 1;
        match api.heartbeat(&binding, &payload) {
            Ok(()) => {
                run_confirmed = true;
                for kind in Provider::ALL {
                    if enable_requested[kind.index()] {
                        enable_requested[kind.index()] = false;
                        enable_remote(
                            &api,
                            &store,
                            &mut saved,
                            &binding,
                            &view,
                            &ctx,
                            &shared,
                            run_generation,
                            &config,
                            kind,
                        );
                    }
                }
                if let Ok(mut v) = view.lock() {
                    if payload.account_action == "clear" {
                        v.account.clear();
                    }
                    if let Some(account) = payload.leigod_account {
                        v.account = account.label;
                    }
                }
                publish(&view, &ctx, &saved, "设备在线，心跳已确认。", false);
            }
            Err(e) => {
                if e.invalidates_session() {
                    remote_invalid(&view);
                    saved.active = false;
                    blocked = true;
                    let _ = store.save(&saved);
                }
                publish(
                    &view,
                    &ctx,
                    &saved,
                    if !saved.active {
                        "设备授权失效，已停止上报。请手动重新绑定。"
                    } else {
                        "设备上报失败，将自动重试；本地守护继续运行。"
                    },
                    false,
                );
            }
        }
    }
}
#[allow(clippy::too_many_arguments)]
fn apply_binding(
    api: &Api,
    store: &Store,
    saved: &mut Saved,
    sequence: &mut i64,
    blocked: &mut bool,
    view: &Arc<Mutex<View>>,
    ctx: &egui::Context,
    request: impl FnOnce(&Api, &Saved) -> Result<DeviceBinding, Error>,
) {
    match request(api, saved) {
        Ok(binding) => {
            for kind in Provider::ALL {
                saved[kind].remote_consent = false;
                saved[kind].remote_token_digest.clear();
            }
            *sequence = saved.reserved_until.max(binding.sequence.saturating_add(1));
            saved.binding = Some(binding);
            saved.active = true;
            *blocked = false;
            if let Err(e) = store.save(saved) {
                saved.active = false;
                *blocked = true;
                publish(view, ctx, saved, &e, false);
            } else {
                publish(view, ctx, saved, "设备已绑定，正在发送首次心跳…", false);
            }
        }
        Err(e) => {
            saved.active = false;
            if e.is_conflict() || e.invalidates_session() {
                *blocked = true;
            }
            let _ = store.save(saved);
            publish(view, ctx, saved, e.message(), false);
        }
    }
}
fn snapshot(
    shared: &Arc<Mutex<crate::shared::Shared>>,
    etalien_shared: &Arc<Mutex<crate::shared::Shared>>,
    config: &Arc<Mutex<crate::config::Config>>,
    sequence: i64,
) -> Heartbeat {
    let mut value = Heartbeat {
        run_generation: 0,
        remote_revision: None,
        etalien_revision: None,
        sequence,
        game_running: None,
        prepare_seconds: 0,
        version: env!("CARGO_PKG_VERSION").into(),
        account_action: "keep".into(),
        leigod_account: None,
    };
    // Copy configuration first; never hold both state locks or perform HTTP under a lock.
    let username = config
        .lock()
        .ok()
        .map(|c| c.account.username.clone())
        .unwrap_or_default();
    if let Ok(s) = shared.lock() {
        value.game_running = s
            .process_snapshot
            .as_ref()
            .map(|_| !s.running_games.is_empty());
        if s.startup_pause_status.preparing_game {
            value.prepare_seconds =
                s.startup_pause_status.remaining_secs.unwrap_or(0).min(600) as u32;
        }
        if s.token.is_none() {
            value.account_action = "clear".into();
        } else if let Some(info) = s.account_info.as_ref() {
            if let Some(link) = account_link(&username, info) {
                value.account_action = "link".into();
                value.leigod_account = Some(link);
            }
        }
    }
    if let Ok(et) = etalien_shared.lock() {
        if et.process_snapshot.is_some() {
            value.game_running =
                Some(value.game_running.unwrap_or(false) || !et.running_games.is_empty());
        }
        if et.startup_pause_status.preparing_game {
            value.prepare_seconds = value
                .prepare_seconds
                .max(et.startup_pause_status.remaining_secs.unwrap_or(0).min(600) as u32);
        }
    }
    value
}
fn account_link(username: &str, info: &serde_json::Value) -> Option<AccountLink> {
    let id = info
        .pointer("/data/user_id")
        .or_else(|| info.pointer("/data/id"))
        .and_then(|v| {
            v.as_str()
                .map(str::to_owned)
                .or_else(|| v.as_u64().map(|n| n.to_string()))
        })
        .filter(|v| !v.is_empty() && v.len() <= 100);
    let (kind, identifier) = if let Some(id) = id {
        ("id", id)
    } else if !username.trim().is_empty() && username.len() <= 100 {
        ("name", username.trim().to_ascii_lowercase())
    } else {
        return None;
    };
    let key = format!(
        "{:x}",
        Sha256::digest(format!("leigod:{kind}:{identifier}").as_bytes())
    );
    let tail: String = identifier
        .chars()
        .rev()
        .take(4)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    Some(AccountLink {
        key,
        label: format!("雷神账号 · ***{tail}"),
    })
}

fn remote_invalid(view: &Arc<Mutex<View>>) {
    if let Ok(mut v) = view.lock() {
        for r in &mut v.remote {
            r.status = RemoteStatus::default();
            r.pending_disable = false;
            r.message = "设备凭据已失效，远程保护状态请在网页核对。".into();
        }
    }
}
fn remote_view(
    view: &Arc<Mutex<View>>,
    ctx: &egui::Context,
    status: RemoteStatus,
    pending: bool,
    kind: Provider,
) {
    if let Ok(mut v) = view.lock() {
        v.remote[kind.index()].pending_disable = pending;
        v.remote[kind.index()].message = status.message().into();
        v.remote[kind.index()].status = status;
    }
    ctx.request_repaint();
}
fn token(
    shared: &Arc<Mutex<crate::shared::Shared>>,
    config: &Arc<Mutex<crate::config::Config>>,
    kind: Provider,
) -> Option<RemoteCredential> {
    match kind {
        Provider::Leigod => Some(RemoteCredential {
            account_token: shared.lock().ok()?.token.clone()?,
            device_id: String::new(),
            paused_state: 0,
        }),
        Provider::Etalien => {
            let c = config.lock().ok()?.etalien.clone();
            Some(RemoteCredential {
                account_token: crate::dpapi::unprotect(&c.token_enc)
                    .ok()
                    .filter(|t| !t.is_empty())?,
                device_id: c.device_id,
                paused_state: c.paused_state.filter(|s| *s > 0)?,
            })
        }
    }
}
fn token_digest(token: &RemoteCredential) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(token).unwrap_or_default())
    )
}
#[allow(clippy::too_many_arguments)]
fn sync_remote(
    api: &Api,
    store: &Store,
    saved: &mut Saved,
    binding: &DeviceBinding,
    view: &Arc<Mutex<View>>,
    ctx: &egui::Context,
    shared: &Arc<Mutex<crate::shared::Shared>>,
    run: i64,
    config: &Arc<Mutex<crate::config::Config>>,
    kind: Provider,
) {
    let marker = read_disable_marker(kind);
    if kind == Provider::Etalien
        && saved[kind].remote_consent
        && token(shared, config, kind).is_none()
    {
        saved[kind].pending_disable = true;
        saved[kind].remote_consent = false;
    }
    if marker.is_some() {
        saved[kind].pending_disable = true;
        saved[kind].remote_consent = false;
    }
    match api.remote_status_for(binding, kind) {
        Ok(mut status) => {
            saved[kind].remote_revision = status.revision;
            if saved[kind].pending_disable || (!saved[kind].remote_consent && status.enabled) {
                saved[kind].pending_disable = true;
                if store.save(saved).is_err() {
                    publish(
                        view,
                        ctx,
                        saved,
                        "关闭请求无法保存，请在网页撤销设备。",
                        false,
                    );
                    return;
                }
                // This also advances the server revision when already off, cancelling
                // any authorization request whose response was lost in transit.
                match api.remote_disable_for(binding, status.revision, kind) {
                    Ok(s) => {
                        status = s;
                        saved[kind].pending_disable = false;
                        saved[kind].remote_consent = false;
                        saved[kind].remote_revision = status.revision;
                        saved[kind].remote_token_digest.clear();
                    }
                    Err(e) if e.invalidates_session() => {
                        saved[kind].pending_disable = false;
                        saved[kind].remote_consent = false;
                    }
                    Err(_) => {
                        publish(
                            view,
                            ctx,
                            saved,
                            "服务器关闭未确认，可能仍会超时暂停；正在重试。",
                            false,
                        );
                        return;
                    }
                }
            } else if saved[kind].remote_consent {
                if !status.enabled {
                    saved[kind].remote_consent = false;
                    saved[kind].remote_token_digest.clear();
                } else if let Some(token) = token(shared, config, kind) {
                    let digest = token_digest(&token);
                    if run > 0 && digest != saved[kind].remote_token_digest {
                        match api.remote_authorize_for(
                            binding,
                            kind,
                            &token,
                            status.revision,
                            run,
                            true,
                        ) {
                            Ok(s) => {
                                status = s;
                                saved[kind].remote_revision = status.revision;
                                saved[kind].remote_token_digest = digest;
                            }
                            Err(e) if e.is_conflict() => {
                                saved[kind].remote_consent = false;
                                saved[kind].pending_disable = true;
                            }
                            Err(_) => {
                                if let Ok(mut v) = view.lock() {
                                    v.remote[kind.index()].message =
                                        "加速器凭据同步失败，保护状态待确认；请检查登录。".into();
                                }
                                let _ = store.save(saved);
                                ctx.request_repaint();
                                return;
                            }
                        }
                    }
                }
            }
            remote_view(view, ctx, status, saved[kind].pending_disable, kind);
            if let Err(e) = store.save(saved) {
                saved[kind].remote_consent = false;
                saved[kind].pending_disable = true;
                publish(view, ctx, saved, &e, false);
            } else if !saved[kind].pending_disable {
                clear_disable_marker(marker.as_deref(), kind);
            }
        }
        Err(e) if e.invalidates_session() => {
            remote_invalid(view);
            saved[kind].pending_disable = false;
            saved[kind].remote_consent = false;
            saved.active = false;
            if store.save(saved).is_ok() {
                clear_disable_marker(marker.as_deref(), kind);
            }
            publish(
                view,
                ctx,
                saved,
                "设备授权已失效，服务器远程保护已撤销。",
                false,
            );
        }
        Err(_) => {
            if let Ok(mut v) = view.lock() {
                v.remote[kind.index()].message = if saved[kind].pending_disable {
                    "服务器关闭未确认，可能仍会超时暂停；正在重试。"
                } else {
                    "无法确认服务器远程保护状态，正在重试。"
                }
                .into();
            }
            ctx.request_repaint();
        }
    }
}
#[allow(clippy::too_many_arguments)]
fn enable_remote(
    api: &Api,
    store: &Store,
    saved: &mut Saved,
    binding: &DeviceBinding,
    view: &Arc<Mutex<View>>,
    ctx: &egui::Context,
    shared: &Arc<Mutex<crate::shared::Shared>>,
    run: i64,
    config: &Arc<Mutex<crate::config::Config>>,
    kind: Provider,
) {
    let Some(token) = token(shared, config, kind) else {
        publish(
            view,
            ctx,
            saved,
            "请先登录对应加速器；外星仔还需完成暂停状态校准。",
            false,
        );
        return;
    };
    // Durable compensation intent precedes uploading any token. Crash/timeout will
    // revoke an uncertain grant on the next run, never silently enable it.
    saved[kind].pending_disable = true;
    if let Err(e) = store.save(saved) {
        publish(view, ctx, saved, &e, false);
        return;
    }
    match api.remote_authorize_for(
        binding,
        kind,
        &token,
        saved[kind].remote_revision,
        run,
        false,
    ) {
        Ok(status) => {
            saved[kind].remote_revision = status.revision;
            saved[kind].remote_consent = true;
            saved[kind].pending_disable = false;
            saved[kind].remote_token_digest = token_digest(&token);
            remote_view(view, ctx, status, saved[kind].pending_disable, kind);
        }
        Err(e) => {
            saved[kind].remote_consent = false;
            if let Ok(mut v) = view.lock() {
                v.remote[kind.index()].error = match e {
                    Error::InvalidInput | Error::Conflict => {
                        "授权失败：请检查加速器登录、账号归属及设备状态。外星仔首次授权前请在官方客户端暂停并校准。".into()
                    }
                    _ => format!("授权未完成：{}", e.message()),
                };
            }
            publish(view, ctx, saved, e.message(), false);
        }
    }
    if let Err(e) = store.save(saved) {
        saved[kind].remote_consent = false;
        saved[kind].pending_disable = true;
        publish(view, ctx, saved, &e, false);
    }
}

static REMOTE_MARKER_LOCK: Mutex<()> = Mutex::new(());
fn disable_marker_path(kind: Provider) -> PathBuf {
    crate::config::Config::path().with_file_name(match kind {
        Provider::Leigod => "platform-remote-disable.pending",
        Provider::Etalien => "platform-etalien-disable.pending",
    })
}
fn read_disable_marker(kind: Provider) -> Option<String> {
    let _guard = REMOTE_MARKER_LOCK.lock().ok()?;
    std::fs::read_to_string(disable_marker_path(kind)).ok()
}
fn write_disable_marker(kind: Provider) -> Result<(), String> {
    use std::io::Write;
    let _guard = REMOTE_MARKER_LOCK
        .lock()
        .map_err(|_| "无法保存远程关闭请求")?;
    let path = disable_marker_path(kind);
    let result = (|| -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::File::create(&path)?;
        file.write_all(
            chrono::Utc::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
                .to_string()
                .as_bytes(),
        )?;
        file.sync_all()
    })();
    result.map_err(|_| "无法保存远程关闭请求".into())
}
fn clear_disable_marker(expected: Option<&str>, kind: Provider) {
    let Ok(_guard) = REMOTE_MARKER_LOCK.lock() else {
        return;
    };
    if expected.is_some()
        && std::fs::read_to_string(disable_marker_path(kind))
            .ok()
            .as_deref()
            == expected
    {
        let _ = std::fs::remove_file(disable_marker_path(kind));
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_device_never_keeps_a_stale_protected_view() {
        let view = Arc::new(Mutex::new(View {
            remote: [
                RemoteView {
                    status: RemoteStatus {
                        available: true,
                        enabled: true,
                        protection: "armed".into(),
                        ..Default::default()
                    },
                    message: "服务器已确认远程保护生效".into(),
                    ..Default::default()
                },
                RemoteView::default(),
            ],
            ..Default::default()
        }));
        remote_invalid(&view);
        let v = view.lock().unwrap();
        assert!(!v.remote[0].status.enabled);
        assert!(!v.remote[0].status.available);
        assert!(!v.remote[0].message.contains("保护生效"));
    }

    #[test]
    fn account_payload_contains_only_a_digest_and_masked_label() {
        let link = account_link(
            "13812345678",
            &serde_json::json!({"data":{"user_id":123456},"token":"must-not-leave-client"}),
        )
        .unwrap();
        let text = serde_json::to_string(&link).unwrap();
        assert!(!text.contains("13812345678"));
        assert!(!text.contains("must-not-leave-client"));
        assert!(!text.contains("123456"));
        assert_eq!(link.key.len(), 64);
        assert!(account_link("", &serde_json::json!({"data":{}})).is_none());
    }
    #[test]
    fn device_store_protects_identity_and_counter_across_restart() {
        let root = std::env::temp_dir().join(format!(
            "guard-device-test-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        ));
        let store = Store(root.join("platform-device.dat"));
        let mut s = store.load().unwrap();
        store.save(&s).unwrap();
        let original = s.installation_key.clone();
        s.reserved_until = 256;
        s.paused = true;
        s[Provider::Leigod].pending_disable = true;
        s[Provider::Leigod].remote_revision = 4;
        store.save(&s).unwrap();
        let read = store.load().unwrap();
        assert_eq!(read.installation_key, original);
        assert_eq!(read.reserved_until, 256);
        assert!(read.paused);
        assert!(read[Provider::Leigod].pending_disable);
        assert_eq!(read[Provider::Leigod].remote_revision, 4);
        assert!(!std::fs::read_to_string(&store.0)
            .unwrap()
            .contains(&original));
        std::fs::remove_dir_all(root).unwrap();
    }
}
