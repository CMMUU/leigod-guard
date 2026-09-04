//! 后台守护线程：进程监控 + 状态机 + 雷神 API 调用。
use crate::config::Config;
use crate::dpapi;
use crate::leigod_api as api;
use crate::monitor;
use crate::shared::{ManualCmd, Shared, StartupPauseStatus};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// 失败重试次数
const MAX_RETRY: usize = 3;
/// 失败后的冷却时间（秒），避免每个 tick 都打 API
const FAIL_COOLDOWN_SECS: u64 = 60;
const PREPARING_GAME_SECS: u64 = 600;

#[derive(Debug, PartialEq, Eq)]
enum PauseDecision {
    Idle,
    Running,
    GraceStarted,
    Waiting(u64),
    Pause,
    StartupPause,
    StartupWaiting {
        remaining_secs: u64,
        preparing_game: bool,
    },
    RetryWaiting,
}

/// Startup recovery is a once-per-launch opportunity, separate from the normal
/// observed-game/exit grace period. Disabling it never arms a later surprise pause.
struct AutoPauseWatch {
    exit: PauseWatch,
    startup_pending: bool,
    startup_empty_since: Option<Instant>,
    startup_deferred_at: Option<Instant>,
    startup_grace_secs: u64,
    active: bool,
    next_retry: Option<Instant>,
}

impl Default for AutoPauseWatch {
    fn default() -> Self {
        Self {
            exit: PauseWatch::default(),
            startup_pending: true,
            startup_empty_since: None,
            startup_deferred_at: None,
            startup_grace_secs: crate::config::DEFAULT_STARTUP_GRACE_SECS,
            active: false,
            next_retry: None,
        }
    }
}

impl AutoPauseWatch {
    fn configure(&mut self, enabled: bool, startup_enabled: bool, valid_watch: bool) {
        self.active = enabled && valid_watch;
        if !self.active {
            self.exit.reset();
            self.next_retry = None;
            self.disable_startup();
        } else if !startup_enabled {
            self.disable_startup();
        }
    }

    fn disable_startup(&mut self) {
        if self.startup_pending {
            self.next_retry = None;
        }
        self.startup_pending = false;
        self.startup_empty_since = None;
        self.startup_deferred_at = None;
    }

    fn observation_failed(&mut self) {
        self.exit.observation_failed();
        self.startup_empty_since = None;
    }

    fn pause_succeeded(&mut self) {
        self.exit.reset();
        self.disable_startup();
        self.next_retry = None;
    }

    fn pause_failed(&mut self, now: Instant) {
        self.next_retry = Some(now + Duration::from_secs(FAIL_COOLDOWN_SECS));
    }

    fn defer_startup(&mut self, clicked_at: Instant) {
        if self.active && self.startup_pending {
            self.startup_deferred_at = Some(
                self.startup_deferred_at
                    .map_or(clicked_at, |previous| previous.max(clicked_at)),
            );
        }
    }

    fn deferral_remaining(&self, now: Instant) -> Duration {
        self.startup_deferred_at
            .map_or(Duration::ZERO, |clicked_at| {
                Duration::from_secs(PREPARING_GAME_SECS)
                    .saturating_sub(now.saturating_duration_since(clicked_at))
            })
    }

    fn startup_protection_remaining(&self, now: Instant) -> Option<Duration> {
        self.startup_empty_since.map(|since| {
            let grace = Duration::from_secs(self.startup_grace_secs)
                .saturating_sub(now.saturating_duration_since(since));
            grace.max(self.deferral_remaining(now))
        })
    }

    fn startup_remaining(&self, now: Instant) -> Option<Duration> {
        self.startup_protection_remaining(now).map(|protection| {
            let retry = self
                .next_retry
                .map_or(Duration::ZERO, |retry| retry.saturating_duration_since(now));
            protection.max(retry)
        })
    }

    fn startup_status(&self, now: Instant) -> StartupPauseStatus {
        if !self.active || !self.startup_pending {
            return StartupPauseStatus::default();
        }
        StartupPauseStatus {
            pending: true,
            remaining_secs: self.startup_remaining(now).map(ceil_secs),
            preparing_game: !self.deferral_remaining(now).is_zero(),
        }
    }

    fn observe(&mut self, now: Instant, has_games: bool, grace_secs: u64) -> PauseDecision {
        if !self.active {
            return PauseDecision::Idle;
        }
        if has_games {
            self.disable_startup();
            self.next_retry = None;
            return self.exit.observe(now, true, grace_secs);
        }
        let decision = if self.startup_pending {
            self.startup_empty_since.get_or_insert(now);
            let remaining = self.startup_protection_remaining(now).unwrap_or_default();
            if !remaining.is_zero() {
                return PauseDecision::StartupWaiting {
                    remaining_secs: ceil_secs(remaining),
                    preparing_game: !self.deferral_remaining(now).is_zero(),
                };
            }
            PauseDecision::StartupPause
        } else {
            self.exit.observe(now, false, grace_secs)
        };
        if matches!(decision, PauseDecision::Pause | PauseDecision::StartupPause)
            && self.next_retry.is_some_and(|retry| now < retry)
        {
            PauseDecision::RetryWaiting
        } else {
            decision
        }
    }
}

fn ceil_secs(duration: Duration) -> u64 {
    duration
        .as_secs()
        .saturating_add(u64::from(duration.subsec_nanos() != 0))
}

fn publish_startup_status(shared: &Arc<Mutex<Shared>>, watch: &AutoPauseWatch, now: Instant) {
    if let Ok(mut shared) = shared.lock() {
        shared.startup_pause_status = watch.startup_status(now);
    }
}

fn consume_startup_request(
    shared: &Arc<Mutex<Shared>>,
    watch: &mut AutoPauseWatch,
    now: Instant,
) -> Result<bool, AutoPauseBlock> {
    let mut shared = shared
        .lock()
        .map_err(|_| AutoPauseBlock::ControlUnavailable)?;
    if let Some(clicked_at) = shared.startup_defer_requested_at.take() {
        watch.defer_startup(clicked_at);
    }
    shared.startup_pause_status = watch.startup_status(now);
    Ok(matches!(shared.manual_cmd, Some(ManualCmd::Pause)))
}

fn checked_watch(cfg: &Config) -> Option<Vec<(String, String)>> {
    if cfg.games.is_empty()
        || cfg
            .games
            .iter()
            .any(|game| !crate::config::valid_game_executable(&game.exe))
    {
        return None;
    }
    Some(
        cfg.games
            .iter()
            .map(|game| (game.name.clone(), game.exe.trim().to_string()))
            .collect(),
    )
}

#[derive(Debug)]
enum AutoPauseBlock {
    Disabled,
    StartupDisabled,
    InvalidWatch,
    Running(Vec<String>),
    ObservationFailed(String),
    ConfigUnavailable,
    ControlUnavailable,
    Protected(PauseDecision),
    ManualPausePending,
}

#[derive(Debug)]
enum CheckedCallError {
    Request(String),
    Blocked(AutoPauseBlock),
}

/// Called after ensure_token (which may wait for network/login), before EVERY
/// actual automatic pause attempt, including an internal expired-token retry.
fn auto_pause_guard(
    cfg: &Arc<Mutex<Config>>,
    shared: &Arc<Mutex<Shared>>,
    watch: &mut AutoPauseWatch,
    startup: bool,
) -> Result<(), AutoPauseBlock> {
    auto_pause_guard_with_snapshot(cfg, shared, watch, startup, Instant::now(), || {
        monitor::try_running_process_names().map_err(|error| error.to_string())
    })
}

fn auto_pause_guard_with_snapshot(
    cfg: &Arc<Mutex<Config>>,
    shared: &Arc<Mutex<Shared>>,
    pause_watch: &mut AutoPauseWatch,
    startup: bool,
    now: Instant,
    snapshot: impl FnOnce() -> Result<Vec<String>, String>,
) -> Result<(), AutoPauseBlock> {
    let cfg = cfg
        .lock()
        .map_err(|_| AutoPauseBlock::ConfigUnavailable)?
        .clone();
    let watch = checked_watch(&cfg);
    pause_watch.configure(
        cfg.strategy.enabled,
        cfg.strategy.pause_on_startup,
        watch.is_some(),
    );
    pause_watch.startup_grace_secs = cfg.strategy.startup_grace_secs;
    if !cfg.strategy.enabled {
        return Err(AutoPauseBlock::Disabled);
    }
    if startup && !cfg.strategy.pause_on_startup {
        return Err(AutoPauseBlock::StartupDisabled);
    }
    let watch = watch.ok_or(AutoPauseBlock::InvalidWatch)?;
    let processes = match snapshot() {
        Ok(processes) => processes,
        Err(error) => {
            pause_watch.observation_failed();
            publish_startup_status(shared, pause_watch, now);
            return Err(AutoPauseBlock::ObservationFailed(error));
        }
    };
    let matched = monitor::match_games(&processes, &watch);
    if !matched.is_empty() {
        pause_watch.observe(now, true, cfg.strategy.grace_secs);
        // Discard a stale deferral if a game has already settled startup recovery.
        consume_startup_request(shared, pause_watch, now)?;
        return Err(AutoPauseBlock::Running(matched));
    }
    // Do this after the potentially slow login AND the fresh snapshot, as close
    // as possible to the API request. A UI/tray click during login must win.
    if consume_startup_request(shared, pause_watch, now)? {
        return Err(AutoPauseBlock::ManualPausePending);
    }
    let decision = pause_watch.observe(now, false, cfg.strategy.grace_secs);
    publish_startup_status(shared, pause_watch, now);
    if (startup && decision == PauseDecision::StartupPause)
        || (!startup && decision == PauseDecision::Pause)
    {
        Ok(())
    } else {
        Err(AutoPauseBlock::Protected(decision))
    }
}

/// 仅在观察到游戏运行后才可触发自动暂停；时钟和观察结果由调用方传入，便于离线验证。
#[derive(Default)]
struct PauseWatch {
    observed_running: bool,
    empty_since: Option<Instant>,
}

impl PauseWatch {
    fn reset(&mut self) {
        *self = Self::default();
    }

    fn observation_failed(&mut self) {
        // 检测失败不代表退出，也不能计入连续无游戏的宽限期。
        self.empty_since = None;
    }

    fn observe(&mut self, now: Instant, has_games: bool, grace_secs: u64) -> PauseDecision {
        if has_games {
            self.observed_running = true;
            self.empty_since = None;
            return PauseDecision::Running;
        }
        if !self.observed_running {
            return PauseDecision::Idle;
        }
        let started = self.empty_since.is_none();
        let since = *self.empty_since.get_or_insert(now);
        // 使用持续时间计算，避免截止时间溢出和跨过截止瞬间时 Instant 相减 panic。
        let left =
            Duration::from_secs(grace_secs).saturating_sub(now.saturating_duration_since(since));
        if left.is_zero() {
            PauseDecision::Pause
        } else if started {
            PauseDecision::GraceStarted
        } else {
            PauseDecision::Waiting(left.as_secs())
        }
    }
}

fn log(shared: &Arc<Mutex<Shared>>, msg: &str) {
    if let Ok(mut s) = shared.lock() {
        s.log(msg);
    }
}

fn alert(shared: &Arc<Mutex<Shared>>, msg: &str) {
    if let Ok(mut s) = shared.lock() {
        s.log(&format!("⚠ {msg}"));
        s.alert = Some(msg.to_string());
    }
}

/// 确保拿到 token：内存 → 本地加密存储 → 用凭据静默重登
fn ensure_token(shared: &Arc<Mutex<Shared>>, cfg: &Arc<Mutex<Config>>) -> Option<String> {
    if let Ok(s) = shared.lock() {
        if let Some(t) = &s.token {
            return Some(t.clone());
        }
    }
    let (username, cred_enc) = {
        let c = cfg.lock().ok()?;
        (c.account.username.clone(), c.account.cred_enc.clone())
    };
    if username.is_empty() || cred_enc.is_empty() {
        return None;
    }
    let md5pwd = match dpapi::unprotect(&cred_enc) {
        Ok(v) => v,
        Err(e) => {
            log(shared, &format!("凭据解密失败: {e}"));
            return None;
        }
    };
    match api::login_with_hash(&username, &md5pwd, None) {
        Ok(token) => {
            log(shared, "已自动登录并获取 token");
            if let Ok(enc) = dpapi::protect(&token) {
                if let Ok(mut c) = cfg.lock() {
                    c.account.token_enc = enc;
                    let _ = c.save();
                }
            }
            if let Ok(mut s) = shared.lock() {
                s.token = Some(token.clone());
            }
            Some(token)
        }
        Err(e) => {
            log(shared, &format!("自动登录失败: {e}"));
            None
        }
    }
}

/// 启动时尝试恢复上次保存的 token
fn restore_token(shared: &Arc<Mutex<Shared>>, cfg: &Arc<Mutex<Config>>) {
    let token_enc = cfg
        .lock()
        .map(|c| c.account.token_enc.clone())
        .unwrap_or_default();
    if token_enc.is_empty() {
        return;
    }
    match dpapi::unprotect(&token_enc) {
        Ok(t) if !t.is_empty() => {
            if let Ok(mut s) = shared.lock() {
                s.token = Some(t);
            }
            log(shared, "已恢复本地保存的 token");
        }
        _ => log(shared, "本地 token 解密失败，将在需要时重新登录"),
    }
}

pub fn run(shared: Arc<Mutex<Shared>>, cfg: Arc<Mutex<Config>>) {
    log(&shared, "守护线程已启动");
    restore_token(&shared, &cfg);

    // 自动暂停包含一次启动补查和原有的游戏退出宽限期；不自动恢复计时。
    let mut pause_watch = AutoPauseWatch::default();
    let mut monitor_failed = false;

    loop {
        let (interval, enabled, startup_enabled, startup_grace_secs, grace_secs, watch) = {
            let c = match cfg.lock() {
                Ok(c) => c.clone(),
                Err(_) => {
                    pause_watch.observation_failed();
                    publish_startup_status(&shared, &pause_watch, Instant::now());
                    std::thread::sleep(Duration::from_secs(3));
                    continue;
                }
            };
            let watch = checked_watch(&c);
            (
                c.strategy.check_interval_secs.max(1),
                c.strategy.enabled,
                c.strategy.pause_on_startup,
                c.strategy.startup_grace_secs,
                c.strategy.grace_secs,
                watch,
            )
        };
        pause_watch.configure(enabled, startup_enabled, watch.is_some());
        pause_watch.startup_grace_secs = startup_grace_secs;
        let _ = consume_startup_request(&shared, &mut pause_watch, Instant::now());

        // 处理 UI 手动指令
        let cmd = shared.lock().ok().and_then(|mut s| s.manual_cmd.take());
        if let Some(cmd) = cmd {
            match cmd {
                ManualCmd::Pause => match call_with_retry(&shared, &cfg, api::pause, "暂停") {
                    Ok(msg) => {
                        pause_watch.pause_succeeded();
                        publish_startup_status(&shared, &pause_watch, Instant::now());
                        log(&shared, &format!("手动暂停请求返回成功: {msg}。最终以雷神官方微信小程序登录同一账号、下拉刷新后的计时状态为准。"));
                        if let Ok(mut s) = shared.lock() {
                            s.manual_pause_result = Some(true);
                        }
                        set_status(&shared, "暂停请求返回成功，请在小程序刷新核对计时状态");
                        refresh_account_info(&shared, &cfg);
                    }
                    Err(e) => {
                        if let Ok(mut s) = shared.lock() {
                            s.manual_pause_result = Some(false);
                        }
                        alert(
                            &shared,
                            &format!("暂停未确认：{e}。请打开雷神官方微信小程序，登录同一账号并下拉刷新，核对计时是否已暂停；仍在计时请手动暂停。"),
                        );
                    }
                },
                // 手动恢复仍保留（供调试/二期使用），UI 已隐藏入口
                ManualCmd::Resume => match call_with_retry(&shared, &cfg, api::recover, "恢复") {
                    Ok(msg) => {
                        log(&shared, &format!("手动恢复成功: {msg}"));
                        set_status(&shared, "已恢复计时");
                        refresh_account_info(&shared, &cfg);
                    }
                    Err(e) => alert(&shared, &format!("恢复计时失败：{e}")),
                },
            }
        }

        // 进程检测：失败时保留最后一次已知游戏列表，取消倒计时并跳过自动暂停。
        let processes = match monitor::try_running_process_names() {
            Ok(processes) => {
                if monitor_failed {
                    log(&shared, "进程检测已恢复");
                    monitor_failed = false;
                }
                processes
            }
            Err(e) => {
                pause_watch.observation_failed();
                publish_startup_status(&shared, &pause_watch, Instant::now());
                if !monitor_failed {
                    log(&shared, &format!("进程检测失败，暂缓自动暂停：{e}"));
                    monitor_failed = true;
                }
                set_status(&shared, "进程检测失败，等待下次检测");
                std::thread::sleep(Duration::from_secs(interval));
                continue;
            }
        };
        let matched = watch
            .as_ref()
            .map(|watch| monitor::match_games(&processes, watch))
            .unwrap_or_default();
        if let Ok(mut s) = shared.lock() {
            s.running_games = matched.clone();
        }

        if !enabled {
            set_status(&shared, "自动暂停已停用");
            std::thread::sleep(Duration::from_secs(interval));
            continue;
        }
        if watch.is_none() {
            set_status(&shared, "名单为空或含无效进程名，自动暂停已暂缓");
            std::thread::sleep(Duration::from_secs(interval));
            continue;
        }

        let now = Instant::now();
        let decision = pause_watch.observe(now, !matched.is_empty(), grace_secs);
        publish_startup_status(&shared, &pause_watch, now);
        match decision {
            PauseDecision::Running => {
                // 即使在 API 冷却期，也必须观察游戏重新启动并取消旧倒计时。
                set_status(
                    &shared,
                    &format!("游戏运行中（{}），自动暂停待命中", matched.join("、")),
                );
            }
            PauseDecision::GraceStarted => {
                log(&shared, &format!("游戏已退出，进入 {grace_secs} 秒宽限期"));
                set_status(&shared, "游戏已退出，宽限期中…");
            }
            PauseDecision::Waiting(left) => {
                set_status(&shared, &format!("游戏已退出，{left} 秒后自动暂停"));
            }
            PauseDecision::RetryWaiting => {
                set_status(&shared, "暂停失败，等待重试");
            }
            PauseDecision::StartupWaiting {
                remaining_secs,
                preparing_game,
            } => {
                set_startup_waiting_status(&shared, remaining_secs, preparing_game);
            }
            decision @ (PauseDecision::Pause | PauseDecision::StartupPause) => {
                let startup = decision == PauseDecision::StartupPause;
                let result = call_with_retry_checked(&shared, &cfg, api::pause, "暂停", || {
                    auto_pause_guard(&cfg, &shared, &mut pause_watch, startup)
                });
                match result {
                    Ok(msg) => {
                        pause_watch.pause_succeeded();
                        publish_startup_status(&shared, &pause_watch, Instant::now());
                        let reason = if startup {
                            "启动检查确认无名单游戏运行，暂停请求返回成功"
                        } else {
                            "宽限期结束，自动暂停请求返回成功"
                        };
                        log(&shared, &format!("{reason}: {msg}。最终以雷神官方微信小程序登录同一账号、下拉刷新后的计时状态为准。"));
                        crate::ui::dbglog(&format!("[worker] auto-pause ok: {msg}"));
                        set_status(&shared, "暂停请求返回成功，请在小程序刷新核对计时状态");
                        refresh_account_info(&shared, &cfg);
                    }
                    Err(CheckedCallError::Request(e)) => {
                        alert(&shared, &format!("自动暂停未确认：{e}。请检查网络或登录；打开雷神官方微信小程序，登录同一账号并下拉刷新，核对计时状态。工具稍后重试。"));
                        pause_watch.pause_failed(Instant::now());
                        set_status(&shared, "暂停失败，等待重试");
                    }
                    Err(CheckedCallError::Blocked(block)) => match block {
                        AutoPauseBlock::Disabled => {
                            pause_watch.configure(false, startup_enabled, true);
                            set_status(&shared, "自动暂停已停用");
                        }
                        AutoPauseBlock::StartupDisabled => {
                            pause_watch.disable_startup();
                            set_status(&shared, "本次启动补暂停已取消");
                        }
                        AutoPauseBlock::InvalidWatch => {
                            pause_watch.configure(enabled, startup_enabled, false);
                            set_status(&shared, "名单为空或含无效进程名，自动暂停已暂缓");
                        }
                        AutoPauseBlock::Running(games) => {
                            pause_watch.observe(Instant::now(), true, grace_secs);
                            if let Ok(mut s) = shared.lock() {
                                s.running_games = games.clone();
                            }
                            log(&shared, "暂停前复查发现游戏运行，已取消本次自动暂停");
                            set_status(
                                &shared,
                                &format!("游戏运行中（{}），自动暂停待命中", games.join("、")),
                            );
                        }
                        AutoPauseBlock::ObservationFailed(error) => {
                            pause_watch.observation_failed();
                            if !monitor_failed {
                                log(
                                    &shared,
                                    &format!("暂停前进程复查失败，暂缓自动暂停：{error}"),
                                );
                                monitor_failed = true;
                            }
                            set_status(&shared, "进程检测失败，等待下次检测");
                        }
                        AutoPauseBlock::ConfigUnavailable => {
                            pause_watch.observation_failed();
                            set_status(&shared, "暂时无法读取策略，等待下次检测");
                        }
                        AutoPauseBlock::ControlUnavailable => {
                            pause_watch.observation_failed();
                            set_status(&shared, "暂时无法读取启动保护请求，等待下次检测");
                        }
                        AutoPauseBlock::Protected(decision) => match decision {
                            PauseDecision::StartupWaiting {
                                remaining_secs,
                                preparing_game,
                            } => {
                                set_startup_waiting_status(&shared, remaining_secs, preparing_game);
                            }
                            PauseDecision::Waiting(left) => {
                                set_status(&shared, &format!("游戏已退出，{left} 秒后自动暂停"));
                            }
                            PauseDecision::RetryWaiting => {
                                set_status(&shared, "暂停未确认，等待重试")
                            }
                            _ => set_status(&shared, "本次自动暂停已暂缓"),
                        },
                        AutoPauseBlock::ManualPausePending => {
                            set_status(&shared, "正在处理手动暂停请求…");
                        }
                    },
                }
            }
            PauseDecision::Idle => set_status(&shared, "空闲（无名单游戏运行）"),
        }
        publish_startup_status(&shared, &pause_watch, Instant::now());

        std::thread::sleep(Duration::from_secs(interval));
    }
}

fn set_status(shared: &Arc<Mutex<Shared>>, status: &str) {
    if let Ok(mut s) = shared.lock() {
        s.status = status.to_string();
    }
}

fn set_startup_waiting_status(
    shared: &Arc<Mutex<Shared>>,
    remaining_secs: u64,
    preparing_game: bool,
) {
    let description = if preparing_game {
        "准备游戏保护中"
    } else {
        "启动检查宽限期中"
    };
    set_status(
        shared,
        &format!("{description}，{remaining_secs} 秒后复查；不会启动或恢复加速"),
    );
}

/// 暂停/恢复成功后刷新账户信息，让账户页展示同步更新
fn refresh_account_info(shared: &Arc<Mutex<Shared>>, cfg: &Arc<Mutex<Config>>) {
    if let Some(t) = ensure_token(shared, cfg) {
        match api::user_info(&t) {
            Ok(v) => {
                if let Ok(mut s) = shared.lock() {
                    s.account_info = Some(v);
                }
            }
            Err(e) => crate::ui::dbglog(&format!("[worker] refresh user_info failed: {}", e.0)),
        }
    }
}

/// 带重试的 API 调用；只有明确是 token 失效（400006）才清内存 token 并重登再试，
/// 其它错误直接上报真实原因（避免把真实错误掩盖成"token 缺失"）。
fn call_with_retry(
    shared: &Arc<Mutex<Shared>>,
    cfg: &Arc<Mutex<Config>>,
    f: fn(&str) -> Result<String, api::ApiError>,
    action: &str,
) -> Result<String, String> {
    call_with_retry_checked(shared, cfg, f, action, || Ok(())).map_err(|error| match error {
        CheckedCallError::Request(message) => message,
        CheckedCallError::Blocked(_) => "请求已取消".to_string(),
    })
}

fn call_with_retry_checked(
    shared: &Arc<Mutex<Shared>>,
    cfg: &Arc<Mutex<Config>>,
    f: fn(&str) -> Result<String, api::ApiError>,
    action: &str,
    mut before_request: impl FnMut() -> Result<(), AutoPauseBlock>,
) -> Result<String, CheckedCallError> {
    let mut last_err = String::new();
    for attempt in 1..=MAX_RETRY {
        match request_after_token(|| ensure_token(shared, cfg), &mut before_request, f)? {
            Ok(msg) => return Ok(msg),
            Err(e) => {
                last_err = e.0.clone();
                log(shared, &format!("{action}第 {attempt} 次失败: {}", e.0));
                crate::ui::dbglog(&format!(
                    "[worker] {action} attempt {attempt} failed: {}",
                    e.0
                ));
                if api::is_token_err(&e) {
                    // token 失效：清掉内存 token，下一轮 ensure_token 会尝试重登
                    if let Ok(mut s) = shared.lock() {
                        s.token = None;
                    }
                    std::thread::sleep(Duration::from_secs(2));
                } else {
                    // 非 token 类错误重试无意义，直接报真实原因
                    break;
                }
            }
        }
    }
    Err(CheckedCallError::Request(last_err))
}

/// Keep token resolution and the fresh guard in one tested call path: login can
/// take long enough for a game to start or the user to disable the strategy.
fn request_after_token<R>(
    resolve_token: impl FnOnce() -> Option<String>,
    before_request: impl FnOnce() -> Result<(), AutoPauseBlock>,
    request: impl FnOnce(&str) -> R,
) -> Result<R, CheckedCallError> {
    let token = resolve_token().ok_or_else(|| {
        CheckedCallError::Request(
            "token 缺失且无法自动重登：请在“账户”页重新登录（验证码方式）或重新粘贴 token"
                .to_string(),
        )
    })?;
    before_request().map_err(CheckedCallError::Blocked)?;
    Ok(request(&token))
}

#[cfg(test)]
mod tests {
    use super::{
        auto_pause_guard_with_snapshot, checked_watch, consume_startup_request,
        request_after_token, AutoPauseBlock, AutoPauseWatch, CheckedCallError, PauseDecision,
        PauseWatch,
    };
    use crate::config::{Config, GameEntry};
    use crate::shared::{ManualCmd, Shared};
    use std::cell::Cell;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    #[test]
    fn ordinary_exit_watch_never_pauses_without_observing_a_game() {
        let now = Instant::now();
        let mut watch = PauseWatch::default();
        assert_eq!(watch.observe(now, false, 0), PauseDecision::Idle);
        assert_eq!(
            watch.observe(now + Duration::from_secs(600), false, 90),
            PauseDecision::Idle
        );
    }

    #[test]
    fn waits_for_complete_grace_period_and_handles_the_boundary() {
        let now = Instant::now();
        let mut watch = PauseWatch::default();
        assert_eq!(watch.observe(now, true, 90), PauseDecision::Running);
        assert_eq!(watch.observe(now, false, 90), PauseDecision::GraceStarted);
        assert_eq!(
            watch.observe(now + Duration::from_secs(89), false, 90),
            PauseDecision::Waiting(1)
        );
        assert_eq!(
            watch.observe(now + Duration::from_secs(90), false, 90),
            PauseDecision::Pause
        );
        // 越过截止时刻也不会因 Instant 相减发生 panic。
        assert_eq!(
            watch.observe(now + Duration::from_secs(91), false, 90),
            PauseDecision::Pause
        );
    }

    #[test]
    fn restarted_game_requires_a_new_complete_grace_period() {
        let now = Instant::now();
        let mut watch = PauseWatch::default();
        watch.observe(now, true, 90);
        watch.observe(now, false, 90);
        assert_eq!(
            watch.observe(now + Duration::from_secs(89), true, 90),
            PauseDecision::Running
        );
        assert_eq!(
            watch.observe(now + Duration::from_secs(90), false, 90),
            PauseDecision::GraceStarted
        );
        assert_eq!(
            watch.observe(now + Duration::from_secs(179), false, 90),
            PauseDecision::Waiting(1)
        );
    }

    #[test]
    fn failed_observation_cannot_count_as_time_without_games() {
        let now = Instant::now();
        let mut watch = PauseWatch::default();
        watch.observe(now, true, 90);
        watch.observe(now, false, 90);
        watch.observation_failed();
        assert_eq!(
            watch.observe(now + Duration::from_secs(120), false, 90),
            PauseDecision::GraceStarted
        );
        assert_eq!(
            watch.observe(now + Duration::from_secs(209), false, 90),
            PauseDecision::Waiting(1)
        );
    }

    #[test]
    fn disabling_or_successfully_pausing_clears_the_old_game_session() {
        let now = Instant::now();
        let mut watch = PauseWatch::default();
        watch.observe(now, true, 90);
        watch.observe(now, false, 90);
        watch.reset();
        assert_eq!(
            watch.observe(now + Duration::from_secs(120), false, 90),
            PauseDecision::Idle
        );
    }

    #[test]
    fn zero_grace_pauses_on_first_confirmed_exit_and_large_values_do_not_overflow() {
        let now = Instant::now();
        let mut watch = PauseWatch::default();
        watch.observe(now, true, 0);
        assert_eq!(watch.observe(now, false, 0), PauseDecision::Pause);
        watch.reset();
        watch.observe(now, true, u64::MAX);
        assert_eq!(
            watch.observe(now, false, u64::MAX),
            PauseDecision::GraceStarted
        );
        assert_eq!(
            watch.observe(now + Duration::from_secs(1), false, u64::MAX),
            PauseDecision::Waiting(u64::MAX - 1)
        );
    }

    #[test]
    fn startup_recovery_requires_180_seconds_then_never_rearms_after_success() {
        let now = Instant::now();
        let mut watch = AutoPauseWatch::default();
        watch.configure(true, true, true);
        assert_eq!(watch.observe(now, false, 90), startup_wait(180, false));
        assert_eq!(
            watch.observe(now + Duration::from_secs(179), false, 90),
            startup_wait(1, false)
        );
        assert_eq!(
            watch.observe(now + Duration::from_secs(180), false, 90),
            PauseDecision::StartupPause
        );
        // The same success path is used by startup, exit, and manual pauses.
        watch.pause_succeeded();
        assert_eq!(watch.observe(now, false, 90), PauseDecision::Idle);
        watch.configure(true, true, true);
        watch.defer_startup(now + Duration::from_secs(300));
        assert_eq!(
            watch.observe(now + Duration::from_secs(600), false, 90),
            PauseDecision::Idle
        );
    }

    #[test]
    fn startup_snapshot_failure_restarts_the_continuous_idle_grace() {
        let now = Instant::now();
        let mut watch = AutoPauseWatch::default();
        watch.configure(true, true, true);
        assert_eq!(watch.observe(now, false, 90), startup_wait(180, false));
        assert_eq!(
            watch.observe(now + Duration::from_secs(100), false, 90),
            startup_wait(80, false)
        );
        watch.observation_failed();
        watch.observation_failed();
        assert!(watch.startup_pending);
        assert_eq!(
            watch.observe(now + Duration::from_secs(120), false, 90),
            startup_wait(180, false)
        );
        assert_eq!(
            watch.observe(now + Duration::from_secs(299), false, 90),
            startup_wait(1, false)
        );
        assert_eq!(
            watch.observe(now + Duration::from_secs(300), false, 90),
            PauseDecision::StartupPause
        );
    }

    #[test]
    fn failed_startup_pause_keeps_pending_and_retries_after_the_bounded_cooldown() {
        let now = Instant::now();
        let mut watch = AutoPauseWatch::default();
        watch.configure(true, true, true);
        watch.observe(now, false, 90);
        let now = now + Duration::from_secs(180);
        assert_eq!(watch.observe(now, false, 90), PauseDecision::StartupPause);
        watch.pause_failed(now);
        assert_eq!(
            watch.observe(now + Duration::from_secs(59), false, 90),
            PauseDecision::RetryWaiting
        );
        // A snapshot failure cannot consume the pending startup pause.
        watch.observation_failed();
        assert_eq!(
            watch.observe(now + Duration::from_secs(60), false, 90),
            startup_wait(180, false)
        );
        assert_eq!(
            watch.observe(now + Duration::from_secs(240), false, 90),
            PauseDecision::StartupPause
        );
        watch.pause_succeeded();
        assert_eq!(
            watch.observe(now + Duration::from_secs(300), false, 90),
            PauseDecision::Idle
        );
    }

    #[test]
    fn game_start_during_login_or_retry_cancels_startup_and_requires_normal_exit_grace() {
        let now = Instant::now();
        let mut watch = AutoPauseWatch::default();
        watch.configure(true, true, true);
        watch.observe(now, false, 90);
        let now = now + Duration::from_secs(180);
        assert_eq!(watch.observe(now, false, 90), PauseDecision::StartupPause);
        watch.pause_failed(now);
        // This fresh observation also represents a game found by the final
        // pre-request guard after a potentially slow login/token refresh.
        assert_eq!(
            watch.observe(now + Duration::from_secs(5), true, 90),
            PauseDecision::Running
        );
        assert!(!watch.startup_pending);
        assert!(watch.next_retry.is_none());
        assert_eq!(
            watch.observe(now + Duration::from_secs(6), false, 90),
            PauseDecision::GraceStarted
        );
        assert_eq!(
            watch.observe(now + Duration::from_secs(95), false, 90),
            PauseDecision::Waiting(1)
        );
        assert_eq!(
            watch.observe(now + Duration::from_secs(96), false, 90),
            PauseDecision::Pause
        );
    }

    #[test]
    fn disabled_or_empty_initial_config_never_arms_a_later_startup_pause() {
        let now = Instant::now();
        for (enabled, startup, valid) in [
            (false, true, true),
            (true, false, true),
            (true, true, false),
        ] {
            let mut watch = AutoPauseWatch::default();
            watch.configure(enabled, startup, valid);
            assert_eq!(watch.observe(now, false, 0), PauseDecision::Idle);
            watch.configure(true, true, true);
            watch.defer_startup(now);
            assert_eq!(watch.observe(now, false, 0), PauseDecision::Idle);
        }
    }

    #[test]
    fn preparing_game_extends_to_latest_click_plus_ten_minutes_without_accumulating() {
        let now = Instant::now();
        let mut watch = AutoPauseWatch::default();
        watch.configure(true, true, true);
        watch.observe(now, false, 90);
        watch.defer_startup(now + Duration::from_secs(10));
        assert_eq!(
            watch.observe(now + Duration::from_secs(20), false, 90),
            startup_wait(590, true)
        );
        watch.defer_startup(now + Duration::from_secs(100));
        // Out-of-order/stale UI requests must not shorten the latest protection.
        watch.defer_startup(now + Duration::from_secs(50));
        assert_eq!(
            watch.observe(now + Duration::from_secs(100), false, 90),
            startup_wait(600, true)
        );
        assert_eq!(
            watch.observe(now + Duration::from_secs(699), false, 90),
            startup_wait(1, true)
        );
        assert_eq!(
            watch.observe(now + Duration::from_secs(700), false, 90),
            PauseDecision::StartupPause
        );
        assert!(
            !watch
                .startup_status(now + Duration::from_secs(700))
                .preparing_game
        );
    }

    #[test]
    fn failed_scan_preserves_deferral_but_requires_new_continuous_idle_after_recovery() {
        let now = Instant::now();
        let mut watch = AutoPauseWatch::default();
        watch.configure(true, true, true);
        watch.observe(now, false, 90);
        watch.defer_startup(now + Duration::from_secs(10));
        watch.observation_failed();
        assert!(
            watch
                .startup_status(now + Duration::from_secs(600))
                .preparing_game
        );
        assert_eq!(
            watch
                .startup_status(now + Duration::from_secs(600))
                .remaining_secs,
            None
        );
        assert_eq!(
            watch.observe(now + Duration::from_secs(601), false, 90),
            startup_wait(180, true)
        );
        assert_eq!(
            watch.observe(now + Duration::from_secs(610), false, 90),
            startup_wait(171, false)
        );
        assert_eq!(
            watch.observe(now + Duration::from_secs(781), false, 90),
            PauseDecision::StartupPause
        );
    }

    #[test]
    fn game_during_startup_countdown_settles_deferral_and_restores_normal_exit_grace() {
        let now = Instant::now();
        let mut watch = AutoPauseWatch::default();
        watch.configure(true, true, true);
        watch.observe(now, false, 90);
        watch.defer_startup(now + Duration::from_secs(10));
        assert_eq!(
            watch.observe(now + Duration::from_secs(30), true, 90),
            PauseDecision::Running
        );
        assert!(!watch.startup_status(now + Duration::from_secs(30)).pending);
        assert!(watch.startup_deferred_at.is_none());
        assert_eq!(
            watch.observe(now + Duration::from_secs(40), false, 90),
            PauseDecision::GraceStarted
        );
        assert_eq!(
            watch.observe(now + Duration::from_secs(129), false, 90),
            PauseDecision::Waiting(1)
        );
        assert_eq!(
            watch.observe(now + Duration::from_secs(130), false, 90),
            PauseDecision::Pause
        );
    }

    #[test]
    fn clearing_or_disabling_pending_watch_discards_old_exit_and_retry_state() {
        let now = Instant::now();
        for valid in [false, true] {
            let mut watch = AutoPauseWatch::default();
            watch.configure(true, true, true);
            watch.observe(now, true, 90);
            watch.observe(now, false, 90);
            watch.pause_failed(now);
            watch.configure(!valid, true, valid);
            watch.configure(true, true, true);
            assert_eq!(
                watch.observe(now + Duration::from_secs(120), false, 0),
                PauseDecision::Idle
            );
            assert!(watch.next_retry.is_none());
        }
    }

    #[test]
    fn one_invalid_configured_game_blocks_the_entire_watch_instead_of_dropping_it() {
        let mut cfg = Config::default();
        assert!(checked_watch(&cfg).is_none());
        cfg.games.push(GameEntry {
            name: "Fixture".into(),
            exe: " fixture.exe ".into(),
            plan: String::new(),
        });
        assert_eq!(checked_watch(&cfg).unwrap()[0].1, "fixture.exe");
        for invalid in ["", "other*.exe", "C:\\games\\other.exe"] {
            cfg.games.push(GameEntry {
                name: "Invalid".into(),
                exe: invalid.into(),
                plan: String::new(),
            });
            assert!(checked_watch(&cfg).is_none());
            cfg.games.pop();
        }
    }

    fn configured_fixture() -> Arc<Mutex<Config>> {
        let mut cfg = Config::default();
        cfg.games.push(GameEntry {
            name: "Fixture".into(),
            exe: "fixture.exe".into(),
            plan: String::new(),
        });
        Arc::new(Mutex::new(cfg))
    }

    fn startup_wait(remaining_secs: u64, preparing_game: bool) -> PauseDecision {
        PauseDecision::StartupWaiting {
            remaining_secs,
            preparing_game,
        }
    }

    #[test]
    fn valid_empty_snapshot_executes_one_pause_and_normal_exit_ignores_startup_opt_out() {
        for startup in [true, false] {
            let cfg = configured_fixture();
            // Disabling startup recovery must leave normal game-exit pauses enabled.
            cfg.lock().unwrap().strategy.pause_on_startup = startup;
            let shared = Arc::new(Mutex::new(Shared::default()));
            let now = Instant::now();
            let mut watch = AutoPauseWatch::default();
            watch.configure(true, startup, true);
            if !startup {
                watch.observe(now, true, 90);
            }
            watch.observe(now, false, 90);
            let ready_at = now + Duration::from_secs(180);
            let calls = Cell::new(0);
            let snapshots = Cell::new(0);
            let result = request_after_token(
                || Some("offline-fixture-token".to_string()),
                || {
                    auto_pause_guard_with_snapshot(
                        &cfg,
                        &shared,
                        &mut watch,
                        startup,
                        ready_at,
                        || {
                            snapshots.set(snapshots.get() + 1);
                            Ok(Vec::new())
                        },
                    )
                },
                |token| {
                    assert_eq!(token, "offline-fixture-token");
                    calls.set(calls.get() + 1);
                    "fixture pause succeeded"
                },
            );
            assert_eq!(result.unwrap(), "fixture pause succeeded");
            assert_eq!(snapshots.get(), 1);
            assert_eq!(calls.get(), 1);
        }
    }

    #[test]
    fn actual_request_guard_scans_after_token_resolution_and_blocks_a_new_game() {
        let cfg = configured_fixture();
        let shared = Arc::new(Mutex::new(Shared::default()));
        let mut watch = AutoPauseWatch::default();
        let game_started = Cell::new(false);
        let calls = Cell::new(0);
        let result = request_after_token(
            || {
                // Simulate the game launching while a slow login completes.
                game_started.set(true);
                Some("offline-fixture-token".to_string())
            },
            || {
                auto_pause_guard_with_snapshot(
                    &cfg,
                    &shared,
                    &mut watch,
                    true,
                    Instant::now(),
                    || {
                        Ok(if game_started.get() {
                            vec!["FIXTURE.EXE".into()]
                        } else {
                            Vec::new()
                        })
                    },
                )
            },
            |_| calls.set(calls.get() + 1),
        );
        assert!(matches!(
            result,
            Err(CheckedCallError::Blocked(AutoPauseBlock::Running(_)))
        ));
        assert_eq!(calls.get(), 0);
    }

    #[test]
    fn actual_request_guard_reloads_changed_config_and_never_treats_failed_scan_as_empty() {
        for change in 0..4 {
            let cfg = configured_fixture();
            let shared = Arc::new(Mutex::new(Shared::default()));
            let mut watch = AutoPauseWatch::default();
            let calls = Cell::new(0);
            let result = request_after_token(
                || {
                    let mut cfg = cfg.lock().unwrap();
                    match change {
                        0 => cfg.strategy.enabled = false,
                        1 => cfg.strategy.pause_on_startup = false,
                        2 => cfg.games.clear(),
                        _ => {}
                    }
                    Some("offline-fixture-token".to_string())
                },
                || {
                    auto_pause_guard_with_snapshot(
                        &cfg,
                        &shared,
                        &mut watch,
                        true,
                        Instant::now(),
                        || Err("fixture scan failed".into()),
                    )
                },
                |_| calls.set(calls.get() + 1),
            );
            let expected = match (change, result) {
                (0, Err(CheckedCallError::Blocked(AutoPauseBlock::Disabled)))
                | (1, Err(CheckedCallError::Blocked(AutoPauseBlock::StartupDisabled)))
                | (2, Err(CheckedCallError::Blocked(AutoPauseBlock::InvalidWatch)))
                | (3, Err(CheckedCallError::Blocked(AutoPauseBlock::ObservationFailed(_)))) => true,
                _ => false,
            };
            assert!(
                expected,
                "change {change} was not blocked by the fresh guard"
            );
            assert_eq!(calls.get(), 0);
        }
    }

    #[test]
    fn startup_grace_changed_during_login_protects_the_still_pending_check() {
        let cfg = configured_fixture();
        let shared = Arc::new(Mutex::new(Shared::default()));
        let mut watch = AutoPauseWatch::default();
        let now = Instant::now();
        watch.configure(true, true, true);
        watch.observe(now, false, 90);
        let calls = Cell::new(0);
        let result = request_after_token(
            || {
                cfg.lock().unwrap().strategy.startup_grace_secs = 300;
                Some("offline-fixture-token".to_string())
            },
            || {
                auto_pause_guard_with_snapshot(
                    &cfg,
                    &shared,
                    &mut watch,
                    true,
                    now + Duration::from_secs(180),
                    || Ok(Vec::new()),
                )
            },
            |_| calls.set(calls.get() + 1),
        );
        assert!(matches!(
            result,
            Err(CheckedCallError::Blocked(AutoPauseBlock::Protected(
                PauseDecision::StartupWaiting {
                    remaining_secs: 120,
                    preparing_game: false
                }
            )))
        ));
        assert_eq!(calls.get(), 0);
        // Reducing the configurable grace cannot shorten explicit preparation protection.
        watch.defer_startup(now + Duration::from_secs(180));
        watch.startup_grace_secs = 0;
        assert_eq!(
            watch.observe(now + Duration::from_secs(181), false, 90),
            startup_wait(599, true)
        );
    }

    #[test]
    fn deferral_clicked_during_token_resolution_blocks_pause_until_its_real_expiry() {
        let cfg = configured_fixture();
        let shared = Arc::new(Mutex::new(Shared::default()));
        let mut watch = AutoPauseWatch::default();
        let now = Instant::now();
        watch.configure(true, true, true);
        watch.observe(now, false, 90);
        let clicked_at = now + Duration::from_secs(181);
        let calls = Cell::new(0);
        let result = request_after_token(
            || {
                shared.lock().unwrap().startup_defer_requested_at = Some(clicked_at);
                Some("offline-fixture-token".to_string())
            },
            || {
                auto_pause_guard_with_snapshot(
                    &cfg,
                    &shared,
                    &mut watch,
                    true,
                    clicked_at + Duration::from_secs(1),
                    || Ok(Vec::new()),
                )
            },
            |_| calls.set(calls.get() + 1),
        );
        assert!(matches!(
            result,
            Err(CheckedCallError::Blocked(AutoPauseBlock::Protected(
                PauseDecision::StartupWaiting {
                    remaining_secs: 599,
                    preparing_game: true
                }
            )))
        ));
        assert_eq!(calls.get(), 0);
        assert_eq!(
            shared.lock().unwrap().startup_pause_status.remaining_secs,
            Some(599)
        );
        assert!(shared.lock().unwrap().startup_defer_requested_at.is_none());
        let result = request_after_token(
            || Some("offline-fixture-token".to_string()),
            || {
                auto_pause_guard_with_snapshot(
                    &cfg,
                    &shared,
                    &mut watch,
                    true,
                    clicked_at + Duration::from_secs(600),
                    || Ok(Vec::new()),
                )
            },
            |_| {
                calls.set(calls.get() + 1);
                "fixture pause succeeded"
            },
        );
        assert_eq!(result.unwrap(), "fixture pause succeeded");
        assert_eq!(calls.get(), 1);
        watch.pause_succeeded();
        shared.lock().unwrap().startup_defer_requested_at =
            Some(clicked_at + Duration::from_secs(601));
        consume_startup_request(&shared, &mut watch, clicked_at + Duration::from_secs(601))
            .unwrap();
        assert!(!shared.lock().unwrap().startup_pause_status.pending);
        assert_eq!(
            watch.observe(clicked_at + Duration::from_secs(1800), false, 90),
            PauseDecision::Idle
        );
    }

    #[test]
    fn manual_pause_has_priority_over_startup_even_when_requested_during_login() {
        let cfg = configured_fixture();
        let shared = Arc::new(Mutex::new(Shared::default()));
        let mut watch = AutoPauseWatch::default();
        let now = Instant::now();
        watch.configure(true, true, true);
        watch.observe(now, false, 90);
        let calls = Cell::new(0);
        let result = request_after_token(
            || {
                let mut shared = shared.lock().unwrap();
                shared.manual_cmd = Some(ManualCmd::Pause);
                shared.startup_defer_requested_at = Some(now + Duration::from_secs(180));
                Some("offline-fixture-token".to_string())
            },
            || {
                auto_pause_guard_with_snapshot(
                    &cfg,
                    &shared,
                    &mut watch,
                    true,
                    now + Duration::from_secs(180),
                    || Ok(Vec::new()),
                )
            },
            |_| calls.set(calls.get() + 1),
        );
        assert!(matches!(
            result,
            Err(CheckedCallError::Blocked(
                AutoPauseBlock::ManualPausePending
            ))
        ));
        assert_eq!(calls.get(), 0);
        assert!(matches!(
            shared.lock().unwrap().manual_cmd,
            Some(ManualCmd::Pause)
        ));
        watch.pause_succeeded();
        assert!(!watch.startup_status(now + Duration::from_secs(181)).pending);
        assert!(watch.startup_deferred_at.is_none());
    }
}
