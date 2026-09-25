//! 后台守护线程：进程监控 + 状态机 + 雷神 API 调用。
use crate::config::Config;
use crate::dpapi;
use crate::game_lifecycle::{Observation, Phase};
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
    LaunchWaiting(u64),
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
    observed_generation: Option<u64>,
    launch_status: Option<Instant>,
    exit_reason: &'static str,
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
            observed_generation: None,
            launch_status: None,
            exit_reason: "game_exit",
        }
    }
}

impl AutoPauseWatch {
    fn configure(&mut self, enabled: bool, startup_enabled: bool, valid_watch: bool) {
        self.active = enabled && valid_watch;
        if !self.active {
            self.launch_status = None;
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
        self.launch_status = None;
    }

    fn pause_succeeded(&mut self) {
        self.launch_status = None;
        self.exit.reset();
        self.disable_startup();
        self.next_retry = None;
    }

    fn pause_failed(&mut self, now: Instant) {
        self.next_retry = Some(now + Duration::from_secs(FAIL_COOLDOWN_SECS));
    }

    fn defer_startup(&mut self, clicked_at: Instant) {
        if self.active {
            self.exit.empty_since = None;
            if !self.startup_pending {
                self.exit.observed_running = true;
            }
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
        if let Some(until) = self.launch_status.filter(|_| self.active) {
            return StartupPauseStatus {
                pending: true,
                remaining_secs: Some(ceil_secs(until.saturating_duration_since(now))),
                preparing_game: !self.deferral_remaining(now).is_zero(),
            };
        }
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
        self.launch_status = None;
        if !self.active {
            return PauseDecision::Idle;
        }
        if has_games {
            self.exit_reason = "game_exit";
            self.disable_startup();
            self.next_retry = None;
            return self.exit.observe(now, true, grace_secs);
        }
        if !self.startup_pending && !self.deferral_remaining(now).is_zero() {
            self.exit.empty_since = None;
            self.exit_reason = "manual_preparation_elapsed";
            self.launch_status = Some(now + self.deferral_remaining(now));
            return PauseDecision::LaunchWaiting(ceil_secs(self.deferral_remaining(now)));
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

    fn observe_game(
        &mut self,
        now: Instant,
        observation: &Observation,
        grace_secs: u64,
    ) -> PauseDecision {
        if observation.phase == Phase::Unknown {
            self.observation_failed();
            return PauseDecision::Idle;
        }
        let changed = self
            .observed_generation
            .replace(observation.generation)
            .is_some_and(|old| old != observation.generation);
        if changed && self.active {
            // Even a complete new game cycle during slow login invalidates the
            // old exit countdown. It must receive a fresh confirmation period.
            let explicit_preparation = self.startup_deferred_at;
            self.disable_startup();
            self.startup_deferred_at = explicit_preparation;
            self.exit.reset();
            self.exit.observed_running = observation.activity_seen;
            self.next_retry = None;
        }
        match observation.phase {
            Phase::Launching if self.active => {
                let explicit_preparation = self.startup_deferred_at;
                self.disable_startup();
                self.startup_deferred_at = explicit_preparation;
                self.exit.reset();
                self.exit.observed_running = true;
                self.next_retry = None;
                self.exit_reason = "launch_protection_elapsed";
                let remaining = observation
                    .remaining(now)
                    .max(ceil_secs(self.deferral_remaining(now)));
                self.launch_status = Some(now + Duration::from_secs(remaining));
                PauseDecision::LaunchWaiting(remaining)
            }
            Phase::Unknown => {
                self.observation_failed();
                self.launch_status = None;
                PauseDecision::Idle
            }
            _ => self.observe(now, observation.phase == Phase::Running, grace_secs),
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

fn publish_process_snapshot(shared: &Arc<Mutex<Shared>>, processes: Option<&[String]>) {
    if let Ok(mut shared) = shared.lock() {
        shared.process_snapshot = processes.map(<[String]>::to_vec);
    }
}

fn game_observation(shared: &Arc<Mutex<Shared>>) -> Result<Observation, String> {
    let observer = shared
        .lock()
        .map_err(|_| "游戏观察不可用")?
        .game_monitor
        .clone()
        .ok_or("游戏观察尚未启动")?;
    let observation = observer.latest();
    if observation.phase == Phase::Unknown {
        return Err("游戏观察失败或已过期".into());
    }
    Ok(observation)
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
    auto_pause_guard_with_observation(cfg, shared, watch, startup, Instant::now(), || {
        let observer = shared
            .lock()
            .map_err(|_| "游戏观察状态不可用")?
            .game_monitor
            .clone()
            .ok_or("游戏观察尚未启动")?;
        let config = cfg.lock().map_err(|_| "配置不可用")?.clone();
        observer.observe_now(&config)
    })
}

#[cfg(test)]
fn auto_pause_guard_with_snapshot(
    cfg: &Arc<Mutex<Config>>,
    shared: &Arc<Mutex<Shared>>,
    pause_watch: &mut AutoPauseWatch,
    startup: bool,
    now: Instant,
    snapshot: impl FnOnce() -> Result<Vec<String>, String>,
) -> Result<(), AutoPauseBlock> {
    auto_pause_guard_with_observation(cfg, shared, pause_watch, startup, now, || {
        let processes = snapshot()?;
        let config = cfg.lock().map_err(|_| "配置不可用")?;
        let watch = checked_watch(&config).unwrap_or_default();
        let mut observation = Observation::unknown(now);
        observation.running = monitor::match_games(&processes, &watch);
        observation.phase = if observation.running.is_empty() {
            Phase::Absent
        } else {
            Phase::Running
        };
        observation.processes = processes;
        Ok(observation)
    })
}

fn auto_pause_guard_with_observation(
    cfg: &Arc<Mutex<Config>>,
    shared: &Arc<Mutex<Shared>>,
    pause_watch: &mut AutoPauseWatch,
    startup: bool,
    now: Instant,
    snapshot: impl FnOnce() -> Result<Observation, String>,
) -> Result<(), AutoPauseBlock> {
    let source_config = cfg;
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
    watch.ok_or(AutoPauseBlock::InvalidWatch)?;
    let observation = match snapshot() {
        Ok(observation) if observation.phase != Phase::Unknown => observation,
        Ok(_) => {
            pause_watch.observation_failed();
            publish_process_snapshot(shared, None);
            return Err(AutoPauseBlock::ObservationFailed("游戏观察尚未就绪".into()));
        }
        Err(error) => {
            publish_process_snapshot(shared, None);
            pause_watch.observation_failed();
            publish_startup_status(shared, pause_watch, now);
            return Err(AutoPauseBlock::ObservationFailed(error));
        }
    };
    let still_current = source_config
        .lock()
        .map_err(|_| AutoPauseBlock::ConfigUnavailable)?;
    if still_current.games != cfg.games || still_current.strategy != cfg.strategy {
        return Err(AutoPauseBlock::ConfigUnavailable);
    }
    drop(still_current);
    publish_process_snapshot(shared, Some(&observation.processes));
    if observation.phase == Phase::Running {
        pause_watch.observe_game(now, &observation, cfg.strategy.grace_secs);
        // Discard a stale deferral if a game has already settled startup recovery.
        shared
            .lock()
            .map_err(|_| AutoPauseBlock::ControlUnavailable)?
            .startup_defer_requested_at = None;
        return Err(AutoPauseBlock::Running(observation.running));
    }
    // Do this after the potentially slow login AND the fresh snapshot, as close
    // as possible to the API request. A UI/tray click during login must win.
    if consume_startup_request(shared, pause_watch, now)? {
        return Err(AutoPauseBlock::ManualPausePending);
    }
    let decision = pause_watch.observe_game(now, &observation, cfg.strategy.grace_secs);
    publish_startup_status(shared, pause_watch, now);
    if (startup && decision == PauseDecision::StartupPause)
        || (!startup && decision == PauseDecision::Pause)
    {
        let provider = shared.lock().map(|s| s.provider).unwrap_or("unknown");
        crate::ui::dbglog(&format!("[pause] source=local provider={provider} reason={} generation={} observation_age_ms={} evidence={:?} action=send",
            if startup { "startup_idle" } else { pause_watch.exit_reason }, observation.generation,
            now.saturating_duration_since(observation.at).as_millis(), observation.evidence));
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
    /// Last observed policy, also used by the UI countdown after a fresh guard check.
    grace_secs: u64,
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
        self.grace_secs = grace_secs;
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
                s.set_token(Some(token.clone()));
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
                s.set_token(Some(t));
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
    let mut was_configured = false;

    loop {
        let (interval, enabled, startup_enabled, startup_grace_secs, grace_secs, watch, configured) = {
            let c = match cfg.lock() {
                Ok(c) => c.clone(),
                Err(_) => {
                    pause_watch.observation_failed();
                    publish_startup_status(&shared, &pause_watch, Instant::now());
                    set_status(&shared, "暂时无法读取策略，等待下次检测");
                    std::thread::sleep(Duration::from_secs(3));
                    continue;
                }
            };
            let watch = checked_watch(&c);
            let configured = c
                .account
                .configured(shared.lock().map(|s| s.token.is_some()).unwrap_or(false));
            (
                c.strategy.check_interval_secs.max(1),
                c.strategy.enabled,
                c.strategy.pause_on_startup,
                c.strategy.startup_grace_secs,
                c.strategy.grace_secs,
                watch,
                configured,
            )
        };
        if configured && !was_configured {
            pause_watch = AutoPauseWatch::default();
        }
        was_configured = configured;
        pause_watch.configure(enabled && configured, startup_enabled, watch.is_some());
        pause_watch.startup_grace_secs = startup_grace_secs;
        let _ = consume_startup_request(&shared, &mut pause_watch, Instant::now());

        // 处理 UI 手动指令
        let cmd = shared.lock().ok().and_then(|mut s| s.manual_cmd.take());
        if let Some(cmd) = cmd {
            match cmd {
                ManualCmd::Pause => match call_with_retry(&shared, &cfg, api::pause, "暂停") {
                    Ok(msg) => {
                        crate::ui::dbglog("[pause] source=local provider=leigod reason=manual result=api_accepted");
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
        let observation = match game_observation(&shared) {
            Ok(observation) => {
                if monitor_failed {
                    log(&shared, "进程检测已恢复");
                    monitor_failed = false;
                }
                observation
            }
            Err(e) => {
                publish_process_snapshot(&shared, None);
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
        publish_process_snapshot(&shared, Some(&observation.processes));
        let matched = observation.running.clone();
        if !configured || !enabled {
            pause_watch.observed_generation = Some(observation.generation);
        }
        if let Ok(mut s) = shared.lock() {
            s.running_games = matched.clone();
        }

        if !configured {
            set_status(&shared, "雷神未登录，等待配置账户");
            std::thread::sleep(Duration::from_secs(interval));
            continue;
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
        let decision = pause_watch.observe_game(now, &observation, grace_secs);
        publish_startup_status(&shared, &pause_watch, now);
        match decision {
            PauseDecision::LaunchWaiting(seconds) => {
                set_status(
                    &shared,
                    &format!("游戏正在启动／准备中，保护剩余 {seconds} 秒"),
                );
            }
            PauseDecision::Running => {
                // 即使在 API 冷却期，也必须观察游戏重新启动并取消旧倒计时。
                set_status(
                    &shared,
                    &format!("游戏运行中（{}），自动暂停待命中", matched.join("、")),
                );
            }
            PauseDecision::GraceStarted => {
                crate::ui::dbglog(&format!(
                    "[pause] provider=leigod reason={} grace={grace_secs} action=countdown",
                    pause_watch.exit_reason
                ));
                log(&shared, &format!("游戏已退出，进入 {grace_secs} 秒宽限期"));
                set_exit_grace_status(&shared, &pause_watch.exit);
            }
            PauseDecision::Waiting(_) => {
                set_exit_grace_status(&shared, &pause_watch.exit);
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
                        crate::ui::dbglog("[pause] source=local provider=leigod action=automatic result=api_accepted");
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
                            PauseDecision::LaunchWaiting(seconds) => set_status(
                                &shared,
                                &format!("游戏正在启动／准备中，保护剩余 {seconds} 秒"),
                            ),
                            PauseDecision::StartupWaiting {
                                remaining_secs,
                                preparing_game,
                            } => {
                                set_startup_waiting_status(&shared, remaining_secs, preparing_game);
                            }
                            PauseDecision::GraceStarted | PauseDecision::Waiting(_) => {
                                set_exit_grace_status(&shared, &pause_watch.exit);
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

/// Independent account state; shares the tested game/exit/startup policy only.
pub fn run_etalien(shared: Arc<Mutex<Shared>>, cfg: Arc<Mutex<Config>>) {
    if let Ok(c) = cfg.lock() {
        if let Ok(token) = dpapi::unprotect(&c.etalien.token_enc) {
            if !token.is_empty() {
                if let Ok(mut state) = shared.lock() {
                    state.set_token(Some(token));
                    state.account_status = "已恢复本地令牌，请刷新确认登录与计时状态".into();
                }
            }
        }
    }
    let mut pause_watch = AutoPauseWatch::default();
    let mut previous_session = None;
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let Ok(config) = cfg.lock().map(|c| c.clone()) else {
            continue;
        };
        let account = config.etalien.clone();
        let session = (
            account.enabled,
            account.token_enc.clone(),
            account.paused_state,
        );
        if previous_session.as_ref() != Some(&session) {
            pause_watch = AutoPauseWatch::default();
            previous_session = Some(session);
        }
        if !account.ready() {
            pause_watch.configure(false, false, false);
            publish_startup_status(&shared, &pause_watch, Instant::now());
            set_status(&shared, "外星仔守护未开启：请登录、校准并启用");
            if let Ok(mut s) = shared.lock() {
                s.manual_cmd = None;
            }
            continue;
        }
        let watch = checked_watch(&config);
        pause_watch.configure(
            config.strategy.enabled,
            config.strategy.pause_on_startup,
            watch.is_some(),
        );
        pause_watch.startup_grace_secs = config.strategy.startup_grace_secs;
        let _ = consume_startup_request(&shared, &mut pause_watch, Instant::now());
        let manual = shared
            .lock()
            .ok()
            .and_then(|mut s| s.manual_cmd.take())
            .is_some();
        let decision = match game_observation(&shared) {
            Ok(observation) => {
                publish_process_snapshot(&shared, Some(&observation.processes));
                let games = observation.running.clone();
                if let Ok(mut s) = shared.lock() {
                    s.running_games = games.clone();
                }
                pause_watch.observe_game(Instant::now(), &observation, config.strategy.grace_secs)
            }
            Err(_) => {
                pause_watch.observation_failed();
                publish_process_snapshot(&shared, None);
                set_status(&shared, "进程检测失败，暂缓自动暂停");
                PauseDecision::Idle
            }
        };
        publish_startup_status(&shared, &pause_watch, Instant::now());
        if manual || matches!(decision, PauseDecision::Pause | PauseDecision::StartupPause) {
            let token = shared.lock().ok().and_then(|s| s.token.clone());
            let result = token
                .as_deref()
                .ok_or_else(|| "请重新登录外星仔账号".to_string())
                .and_then(|token| {
                    crate::etalien_api::pause(
                        token,
                        &account.device_id,
                        account.paused_state.unwrap(),
                        Duration::from_secs(18),
                        || {
                            let latest = cfg.lock().map_err(|_| "无法读取配置")?.etalien.clone();
                            if !latest.ready()
                                || latest.token_enc != account.token_enc
                                || latest.paused_state != account.paused_state
                                || latest.device_id != account.device_id
                            {
                                return Err("账号或守护配置已变化，取消旧请求".into());
                            }
                            if shared.lock().map_err(|_| "无法读取账号")?.token.as_deref()
                                != Some(token)
                            {
                                return Err("登录状态已变化，取消旧请求".into());
                            }
                            if !manual {
                                auto_pause_guard(
                                    &cfg,
                                    &shared,
                                    &mut pause_watch,
                                    decision == PauseDecision::StartupPause,
                                )
                                .map_err(|_| "暂停前复核未通过，已暂缓本次自动暂停".to_string())?;
                            }
                            Ok(())
                        },
                    )
                });
            // Hold both locks in config -> state order while publishing. A late
            // response must not overwrite a logout or a newly selected account.
            let Ok(current) = cfg.lock() else {
                continue;
            };
            if !current.etalien.ready()
                || current.etalien.token_enc != account.token_enc
                || current.etalien.device_id != account.device_id
                || current.etalien.paused_state != account.paused_state
            {
                continue;
            }
            let Ok(mut state) = shared.lock() else {
                continue;
            };
            if state.token != token {
                continue;
            }
            match result {
                Ok(info) => {
                    crate::ui::dbglog(&format!(
                        "[pause] source=local provider=etalien reason={} result=confirmed",
                        if manual {
                            "manual"
                        } else if decision == PauseDecision::StartupPause {
                            "startup_idle"
                        } else {
                            pause_watch.exit_reason
                        }
                    ));
                    pause_watch.pause_succeeded();
                    state.set_status("外星仔官方状态已确认暂停");
                    state.log("暂停状态查询已确认");
                    state.account_status = crate::ui_etalien::describe(&info, account.paused_state);
                    state.manual_pause_result = Some(true);
                }
                Err(error) => {
                    crate::ui::dbglog(
                        "[pause] source=local provider=etalien result=unconfirmed_or_cancelled",
                    );
                    pause_watch.pause_failed(Instant::now());
                    state.set_status(&error);
                    state.log(&error);
                    state.manual_pause_result = Some(false);
                }
            }
        } else {
            match decision {
                PauseDecision::LaunchWaiting(seconds) => set_status(
                    &shared,
                    &format!("游戏正在启动／准备中，保护剩余 {seconds} 秒"),
                ),
                PauseDecision::Running => set_status(&shared, "游戏运行中，外星仔自动暂停待命"),
                PauseDecision::GraceStarted | PauseDecision::Waiting(_) => {
                    set_exit_grace_status(&shared, &pause_watch.exit)
                }
                PauseDecision::StartupWaiting {
                    remaining_secs,
                    preparing_game,
                } => set_startup_waiting_status(&shared, remaining_secs, preparing_game),
                PauseDecision::RetryWaiting => {} // Keep the last actionable error visible.
                PauseDecision::Idle if !config.strategy.enabled => {
                    set_status(&shared, "自动暂停总开关已关闭")
                }
                PauseDecision::Idle if watch.is_none() => {
                    set_status(&shared, "名单为空或无效，暂缓自动暂停")
                }
                _ => {}
            }
        }
        publish_startup_status(&shared, &pause_watch, Instant::now());
        std::thread::sleep(Duration::from_secs(
            config.strategy.check_interval_secs.max(1).saturating_sub(1),
        ));
    }
}

fn set_status(shared: &Arc<Mutex<Shared>>, status: &str) {
    if let Ok(mut s) = shared.lock() {
        s.set_status(status);
    }
}

fn set_exit_grace_status(shared: &Arc<Mutex<Shared>>, watch: &PauseWatch) {
    if let Ok(mut s) = shared.lock() {
        if let Some(since) = watch.empty_since {
            s.set_exit_grace(since, watch.grace_secs);
        } else {
            s.set_status("游戏已退出，等待下次检测");
        }
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
                    s.set_account_info(&t, v);
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
    let binding = cfg
        .lock()
        .map(|c| (c.account.username.clone(), c.account.cred_enc.clone()))
        .map_err(|_| CheckedCallError::Blocked(AutoPauseBlock::ConfigUnavailable))?;
    let mut last_err = String::new();
    for attempt in 1..=MAX_RETRY {
        match request_after_token(
            || ensure_token(shared, cfg),
            &mut before_request,
            |token| {
                let same_account = cfg.lock().is_ok_and(|c| {
                    (c.account.username.as_str(), c.account.cred_enc.as_str())
                        == (binding.0.as_str(), binding.1.as_str())
                });
                let same_token = shared
                    .lock()
                    .is_ok_and(|s| s.token.as_deref() == Some(token));
                if !same_account || !same_token {
                    return Err(api::ApiError("登录会话已变化，旧请求已取消".into()));
                }
                f(token)
            },
        )? {
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
                        s.set_token(None);
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
        request_after_token, set_exit_grace_status, set_status, AutoPauseBlock, AutoPauseWatch,
        CheckedCallError, PauseDecision, PauseWatch,
    };
    use crate::config::{Config, GameEntry};
    use crate::shared::{ManualCmd, Shared};
    use std::cell::Cell;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    fn lifecycle_fixture() -> (
        crate::game_lifecycle::Tracker,
        Vec<(String, String)>,
        AutoPauseWatch,
    ) {
        let mut policy = AutoPauseWatch::default();
        policy.configure(true, true, true);
        (
            crate::game_lifecycle::Tracker::default(),
            vec![("PUBG".into(), "TslGame.exe".into())],
            policy,
        )
    }

    fn process(pid: u32, exe: &str) -> crate::game_lifecycle::Process {
        crate::game_lifecycle::Process {
            id: crate::game_lifecycle::ProcessId {
                pid,
                created: Some(pid as u64),
                exe: exe.into(),
            },
            age: Some(Duration::ZERO),
        }
    }

    #[test]
    fn startup_deadline_during_pubg_launch_never_reaches_pause() {
        let t = Instant::now();
        let (mut tracker, games, mut policy) = lifecycle_fixture();
        let root = process(1, "ExecPubg.exe");
        let empty = tracker.observe(t, &[], &games, 600);
        assert_eq!(policy.observe_game(t, &empty, 90), startup_wait(180, false));
        for seconds in [172, 181, 190] {
            let now = t + Duration::from_secs(seconds);
            let obs = tracker.observe(now, &[root.clone()], &games, 600);
            assert!(matches!(
                policy.observe_game(now, &obs, 90),
                PauseDecision::LaunchWaiting(_)
            ));
        }
        let now = t + Duration::from_secs(211);
        let obs = tracker.observe(now, &[root, process(2, "TslGame.exe")], &games, 600);
        assert_eq!(policy.observe_game(now, &obs, 90), PauseDecision::Running);
    }

    #[test]
    fn restart_cancels_old_exit_but_leftover_helper_does_not_cancel_new_exit() {
        let t = Instant::now();
        let (mut tracker, games, mut policy) = lifecycle_fixture();
        let main = process(1, "TslGame.exe");
        let root = process(2, "ExecPubg.exe");
        let obs = tracker.observe(t, &[main], &games, 600);
        assert_eq!(policy.observe_game(t, &obs, 90), PauseDecision::Running);
        let obs = tracker.observe(t, &[], &games, 600);
        assert_eq!(
            policy.observe_game(t, &obs, 90),
            PauseDecision::GraceStarted
        );
        let now = t + Duration::from_secs(79);
        let obs = tracker.observe(now, &[root.clone()], &games, 600);
        assert!(matches!(
            policy.observe_game(now, &obs, 90),
            PauseDecision::LaunchWaiting(_)
        ));
        let now = t + Duration::from_secs(91);
        let obs = tracker.observe(now, &[root.clone()], &games, 600);
        assert!(matches!(
            policy.observe_game(now, &obs, 90),
            PauseDecision::LaunchWaiting(_)
        ));
        let now = t + Duration::from_secs(114);
        let obs = tracker.observe(now, &[root.clone(), process(3, "TslGame.exe")], &games, 600);
        assert_eq!(policy.observe_game(now, &obs, 90), PauseDecision::Running);
        let obs = tracker.observe(now, &[root.clone()], &games, 600);
        assert_eq!(
            policy.observe_game(now, &obs, 90),
            PauseDecision::GraceStarted
        );
        let later = now + Duration::from_secs(90);
        let obs = tracker.observe(later, &[root], &games, 600);
        assert_eq!(policy.observe_game(later, &obs, 90), PauseDecision::Pause);
    }

    #[test]
    fn exhausted_launch_starts_full_exit_grace_even_if_launcher_is_stuck() {
        let t = Instant::now();
        let (mut tracker, games, mut policy) = lifecycle_fixture();
        let processes = [process(1, "ExecPubg.exe")];
        let obs = tracker.observe(t, &processes, &games, 600);
        assert_eq!(
            policy.observe_game(t, &obs, 90),
            PauseDecision::LaunchWaiting(600)
        );
        let now = t + Duration::from_secs(600);
        let obs = tracker.observe(now, &processes, &games, 600);
        assert_eq!(
            policy.observe_game(now, &obs, 90),
            PauseDecision::GraceStarted
        );
        assert_eq!(policy.exit_reason, "launch_protection_elapsed");
        let now = t + Duration::from_secs(690);
        let obs = tracker.observe(now, &processes, &games, 600);
        assert_eq!(policy.observe_game(now, &obs, 90), PauseDecision::Pause);
    }

    #[test]
    fn a_complete_game_cycle_during_login_invalidates_the_old_candidate() {
        let t = Instant::now();
        let (mut tracker, games, mut policy) = lifecycle_fixture();
        let obs = tracker.observe(t, &[process(1, "TslGame.exe")], &games, 600);
        policy.observe_game(t, &obs, 90);
        let obs = tracker.observe(t, &[], &games, 600);
        policy.observe_game(t, &obs, 90);
        let now = t + Duration::from_secs(90);
        assert_eq!(policy.observe_game(now, &obs, 90), PauseDecision::Pause);
        tracker.observe(now, &[process(2, "TslGame.exe")], &games, 600);
        let latest = tracker.observe(now, &[], &games, 600);
        assert_eq!(
            policy.observe_game(now, &latest, 90),
            PauseDecision::GraceStarted
        );
    }

    #[test]
    fn explicit_preparation_survives_a_generation_change_at_final_check() {
        let t = Instant::now();
        let (mut tracker, games, mut policy) = lifecycle_fixture();
        let obs = tracker.observe(t, &[process(1, "TslGame.exe")], &games, 600);
        policy.observe_game(t, &obs, 0);
        let empty = tracker.observe(t, &[], &games, 600);
        policy.observe_game(t, &empty, 0);
        tracker.observe(t, &[process(2, "TslGame.exe")], &games, 600);
        let latest = tracker.observe(t, &[], &games, 600);
        policy.defer_startup(t);
        assert_eq!(
            policy.observe_game(t, &latest, 0),
            PauseDecision::LaunchWaiting(600)
        );
    }

    #[test]
    fn unknown_observation_restarts_confirmation_without_consuming_startup() {
        let t = Instant::now();
        let (mut tracker, games, mut policy) = lifecycle_fixture();
        let empty = tracker.observe(t, &[], &games, 600);
        assert_eq!(policy.observe_game(t, &empty, 90), startup_wait(180, false));
        let now = t + Duration::from_secs(180);
        let unknown = crate::game_lifecycle::Observation::unknown(now);
        assert_eq!(policy.observe_game(now, &unknown, 90), PauseDecision::Idle);
        let empty = tracker.observe(now, &[], &games, 600);
        assert_eq!(
            policy.observe_game(now, &empty, 90),
            startup_wait(180, false)
        );
    }

    #[test]
    fn changing_watch_list_drops_the_previous_games_exit_candidate() {
        let t = Instant::now();
        let (mut tracker, games, mut policy) = lifecycle_fixture();
        let running = tracker.observe(t, &[process(1, "TslGame.exe")], &games, 600);
        policy.observe_game(t, &running, 90);
        let empty = tracker.observe(t, &[], &games, 600);
        assert_eq!(
            policy.observe_game(t, &empty, 90),
            PauseDecision::GraceStarted
        );
        let now = t + Duration::from_secs(90);
        let replacement = vec![("Other".into(), "other.exe".into())];
        let observation = tracker.observe(now, &[], &replacement, 600);
        assert_eq!(
            policy.observe_game(now, &observation, 90),
            PauseDecision::Idle
        );
    }

    #[test]
    fn launch_seen_only_in_final_guard_blocks_both_provider_pause_paths() {
        let t = Instant::now();
        for provider in ["leigod", "etalien"] {
            let cfg = configured_fixture();
            cfg.lock().unwrap().games[0].exe = "TslGame.exe".into();
            let shared = Arc::new(Mutex::new(Shared::default()));
            shared.lock().unwrap().provider = provider;
            let (mut tracker, games, mut policy) = lifecycle_fixture();
            policy.observe(t, false, 90);
            let now = t + Duration::from_secs(180);
            let result = super::auto_pause_guard_with_observation(
                &cfg,
                &shared,
                &mut policy,
                true,
                now,
                || Ok(tracker.observe(now, &[process(1, "ExecPubg.exe")], &games, 600)),
            );
            assert!(matches!(
                result,
                Err(AutoPauseBlock::Protected(PauseDecision::LaunchWaiting(_)))
            ));
        }
    }

    #[test]
    fn publishing_exit_countdown_keeps_the_original_clock_and_current_policy() {
        let start = Instant::now();
        let shared = Arc::new(Mutex::new(Shared::default()));
        let mut watch = PauseWatch::default();
        watch.observe(start, true, 90);
        assert_eq!(watch.observe(start, false, 90), PauseDecision::GraceStarted);
        set_exit_grace_status(&shared, &watch);
        assert_eq!(
            shared
                .lock()
                .unwrap()
                .status_at(start + Duration::from_secs(2)),
            "游戏已退出，88 秒后自动暂停"
        );

        // A later process scan must not restart or round the countdown's clock.
        watch.observe(start + Duration::from_millis(3500), false, 90);
        set_exit_grace_status(&shared, &watch);
        assert_eq!(
            shared
                .lock()
                .unwrap()
                .status_at(start + Duration::from_secs(4)),
            "游戏已退出，86 秒后自动暂停"
        );

        // A policy change detected by the final pause guard uses its fresh value.
        watch.observe(start + Duration::from_secs(4), false, 120);
        set_exit_grace_status(&shared, &watch);
        assert_eq!(
            shared
                .lock()
                .unwrap()
                .status_at(start + Duration::from_secs(5)),
            "游戏已退出，115 秒后自动暂停"
        );
        watch.observe(start + Duration::from_secs(6), true, 120);
        set_status(&shared, "游戏运行中");
        assert!(shared.lock().unwrap().exit_grace_countdown.is_none());
    }

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
    fn startup_recovery_finishes_once_but_explicit_preparation_can_protect_a_restart() {
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
            PauseDecision::LaunchWaiting(300)
        );
        assert_eq!(
            watch.observe(now + Duration::from_secs(900), false, 90),
            PauseDecision::GraceStarted
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
            assert_eq!(watch.observe(now, false, 0), PauseDecision::Idle);
            // Re-enabling alone never arms a pause; an explicit preparation
            // request now intentionally protects a new startup/restart.
            watch.defer_startup(now);
            assert_eq!(
                watch.observe(now, false, 0),
                PauseDecision::LaunchWaiting(600)
            );
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
            assert_eq!(shared.lock().unwrap().process_snapshot, Some(vec![]));
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
            shared.lock().unwrap().process_snapshot = Some(vec!["fixture.exe".into()]);
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
            if change == 3 {
                assert!(
                    shared.lock().unwrap().process_snapshot.is_none(),
                    "a failed scan must invalidate the UI's previous process status"
                );
            }
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
            PauseDecision::GraceStarted
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
