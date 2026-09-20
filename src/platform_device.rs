//! Device worker is independent of window visibility and never receives Leigod secrets.
use crate::platform_api::{AccountLink, Api, DeviceBinding, Error, Heartbeat, Session, ORIGIN_URL};
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
        })
    }
    fn valid(&self) -> bool {
        self.origin == ORIGIN_URL
            && self.installation_key.len() == 64
            && self.installation_key.bytes().all(|b| b.is_ascii_hexdigit())
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
}
enum Command {
    Session(Session, bool),
    Bind(Session),
    Pair(String, Option<Session>),
    Stop,
}
pub(crate) struct Agent {
    sender: mpsc::Sender<Command>,
    pub view: Arc<Mutex<View>>,
}
impl Agent {
    pub fn start(
        shared: Arc<Mutex<crate::shared::Shared>>,
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
            .spawn(move || run(receiver, status, shared, config, ctx));
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
    pub fn stop(&self) {
        let _ = self.sender.send(Command::Stop);
    }
}
fn publish(view: &Arc<Mutex<View>>, ctx: &egui::Context, saved: &Saved, message: &str, busy: bool) {
    if let Ok(mut v) = view.lock() {
        v.message = message.into();
        v.active = saved.active;
        v.busy = busy;
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
    let mut sequence = saved.reserved_until;
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
        let wait = if saved.active {
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
                Command::Stop => {
                    last_session = None;
                    saved.active = false;
                    saved.paused = true;
                    let message = store.save(&saved).err().unwrap_or_else(|| {
                        "已停止本机上报；平台中的设备记录保留，可在网页撤销或解除。".into()
                    });
                    publish(&view, &ctx, &saved, &message, false);
                    continue;
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
        let payload = snapshot(&shared, &config, sequence);
        sequence += 1;
        match api.heartbeat(&binding, &payload) {
            Ok(()) => {
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
                if e.invalidates_session() || e == Error::Conflict {
                    saved.active = false;
                    blocked = true;
                    let _ = store.save(&saved);
                }
                publish(
                    &view,
                    &ctx,
                    &saved,
                    if !saved.active {
                        "设备授权失效或序号冲突，已停止上报。请手动重新绑定。"
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
            if e == Error::Conflict || e.invalidates_session() {
                *blocked = true;
            }
            let _ = store.save(saved);
            publish(view, ctx, saved, e.message(), false);
        }
    }
}
fn snapshot(
    shared: &Arc<Mutex<crate::shared::Shared>>,
    config: &Arc<Mutex<crate::config::Config>>,
    sequence: i64,
) -> Heartbeat {
    let mut value = Heartbeat {
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
#[cfg(test)]
mod tests {
    use super::*;
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
        store.save(&s).unwrap();
        let read = store.load().unwrap();
        assert_eq!(read.installation_key, original);
        assert_eq!(read.reserved_until, 256);
        assert!(read.paused);
        assert!(!std::fs::read_to_string(&store.0)
            .unwrap()
            .contains(&original));
        std::fs::remove_dir_all(root).unwrap();
    }
}
