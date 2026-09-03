//! 后台守护线程：进程监控 + 状态机 + 雷神 API 调用。
use crate::config::Config;
use crate::dpapi;
use crate::leigod_api as api;
use crate::monitor;
use crate::shared::{ManualCmd, Shared};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// 失败重试次数
const MAX_RETRY: usize = 3;
/// 失败后的冷却时间（秒），避免每个 tick 都打 API
const FAIL_COOLDOWN_SECS: u64 = 60;

#[derive(Debug, PartialEq, Eq)]
enum PauseDecision {
    Idle,
    Running,
    GraceStarted,
    Waiting(u64),
    Pause,
    StartupPause,
    RetryWaiting,
}

/// Startup recovery is a once-per-launch opportunity, separate from the normal
/// observed-game/exit grace period. Disabling it never arms a later surprise pause.
struct AutoPauseWatch {
    exit: PauseWatch,
    startup_pending: bool,
    active: bool,
    next_retry: Option<Instant>,
}

impl Default for AutoPauseWatch {
    fn default() -> Self {
        Self {
            exit: PauseWatch::default(),
            startup_pending: true,
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
            self.startup_pending = false;
        } else if !startup_enabled {
            self.disable_startup();
        }
    }

    fn disable_startup(&mut self) {
        if self.startup_pending {
            self.next_retry = None;
        }
        self.startup_pending = false;
    }

    fn observation_failed(&mut self) {
        self.exit.observation_failed();
    }

    fn pause_succeeded(&mut self) {
        self.exit.reset();
        self.startup_pending = false;
        self.next_retry = None;
    }

    fn pause_failed(&mut self, now: Instant) {
        self.next_retry = Some(now + Duration::from_secs(FAIL_COOLDOWN_SECS));
    }

    fn observe(&mut self, now: Instant, has_games: bool, grace_secs: u64) -> PauseDecision {
        if !self.active {
            return PauseDecision::Idle;
        }
        if has_games {
            self.startup_pending = false;
            self.next_retry = None;
            return self.exit.observe(now, true, grace_secs);
        }
        let decision = if self.startup_pending {
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
}

#[derive(Debug)]
enum CheckedCallError {
    Request(String),
    Blocked(AutoPauseBlock),
}

/// Called after ensure_token (which may wait for network/login), before EVERY
/// actual automatic pause attempt, including an internal expired-token retry.
fn auto_pause_guard(cfg: &Arc<Mutex<Config>>, startup: bool) -> Result<(), AutoPauseBlock> {
    auto_pause_guard_with_snapshot(cfg, startup, || {
        monitor::try_running_process_names().map_err(|error| error.to_string())
    })
}

fn auto_pause_guard_with_snapshot(
    cfg: &Arc<Mutex<Config>>,
    startup: bool,
    snapshot: impl FnOnce() -> Result<Vec<String>, String>,
) -> Result<(), AutoPauseBlock> {
    let cfg = cfg
        .lock()
        .map_err(|_| AutoPauseBlock::ConfigUnavailable)?
        .clone();
    if !cfg.strategy.enabled {
        return Err(AutoPauseBlock::Disabled);
    }
    if startup && !cfg.strategy.pause_on_startup {
        return Err(AutoPauseBlock::StartupDisabled);
    }
    let watch = checked_watch(&cfg).ok_or(AutoPauseBlock::InvalidWatch)?;
    let processes = snapshot().map_err(AutoPauseBlock::ObservationFailed)?;
    let matched = monitor::match_games(&processes, &watch);
    if matched.is_empty() {
        Ok(())
    } else {
        Err(AutoPauseBlock::Running(matched))
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
        let (interval, enabled, startup_enabled, grace_secs, watch) = {
            let c = match cfg.lock() {
                Ok(c) => c.clone(),
                Err(_) => {
                    std::thread::sleep(Duration::from_secs(3));
                    continue;
                }
            };
            let watch = checked_watch(&c);
            (
                c.strategy.check_interval_secs.max(1),
                c.strategy.enabled,
                c.strategy.pause_on_startup,
                c.strategy.grace_secs,
                watch,
            )
        };
        pause_watch.configure(enabled, startup_enabled, watch.is_some());

        // 处理 UI 手动指令
        let cmd = shared.lock().ok().and_then(|mut s| s.manual_cmd.take());
        if let Some(cmd) = cmd {
            match cmd {
                ManualCmd::Pause => match call_with_retry(&shared, &cfg, api::pause, "暂停") {
                    Ok(msg) => {
                        pause_watch.pause_succeeded();
                        log(&shared, &format!("手动暂停成功: {msg}"));
                        if let Ok(mut s) = shared.lock() {
                            s.manual_pause_result = Some(true);
                        }
                        set_status(&shared, "已暂停计时");
                        refresh_account_info(&shared, &cfg);
                    }
                    Err(e) => {
                        if let Ok(mut s) = shared.lock() {
                            s.manual_pause_result = Some(false);
                        }
                        alert(
                            &shared,
                            &format!("暂停失败：{e}。时长仍在消耗，请打开雷神客户端手动暂停！"),
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
        match pause_watch.observe(now, !matched.is_empty(), grace_secs) {
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
            decision @ (PauseDecision::Pause | PauseDecision::StartupPause) => {
                let startup = decision == PauseDecision::StartupPause;
                let result = call_with_retry_checked(&shared, &cfg, api::pause, "暂停", || {
                    auto_pause_guard(&cfg, startup)
                });
                match result {
                    Ok(msg) => {
                        pause_watch.pause_succeeded();
                        let reason = if startup {
                            "启动检查确认无名单游戏运行，已补暂停计时"
                        } else {
                            "宽限期结束，已自动暂停计时"
                        };
                        log(&shared, &format!("{reason}: {msg}"));
                        crate::ui::dbglog(&format!("[worker] auto-pause ok: {msg}"));
                        set_status(&shared, "已暂停计时");
                        refresh_account_info(&shared, &cfg);
                    }
                    Err(CheckedCallError::Request(e)) => {
                        alert(&shared, &format!("自动暂停未确认：{e}。请检查网络或登录，并在雷神客户端确认暂停状态；工具稍后重试。"));
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
                    },
                }
            }
            PauseDecision::Idle => set_status(&shared, "空闲（无名单游戏运行）"),
        }

        std::thread::sleep(Duration::from_secs(interval));
    }
}

fn set_status(shared: &Arc<Mutex<Shared>>, status: &str) {
    if let Ok(mut s) = shared.lock() {
        s.status = status.to_string();
    }
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
        auto_pause_guard_with_snapshot, checked_watch, request_after_token, AutoPauseBlock,
        AutoPauseWatch, CheckedCallError, PauseDecision, PauseWatch,
    };
    use crate::config::{Config, GameEntry};
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
    fn startup_recovery_pauses_immediately_once_and_manual_success_settles_it() {
        let now = Instant::now();
        let mut watch = AutoPauseWatch::default();
        watch.configure(true, true, true);
        assert_eq!(watch.observe(now, false, 90), PauseDecision::StartupPause);
        // The same success path is used by startup, exit, and manual pauses.
        watch.pause_succeeded();
        assert_eq!(watch.observe(now, false, 90), PauseDecision::Idle);
        watch.configure(true, true, true);
        assert_eq!(
            watch.observe(now + Duration::from_secs(600), false, 90),
            PauseDecision::Idle
        );
    }

    #[test]
    fn startup_snapshot_failure_retains_recovery_until_a_complete_observation() {
        let now = Instant::now();
        let mut watch = AutoPauseWatch::default();
        watch.configure(true, true, true);
        watch.observation_failed();
        watch.observation_failed();
        assert!(watch.startup_pending);
        assert_eq!(
            watch.observe(now + Duration::from_secs(20), false, 90),
            PauseDecision::StartupPause
        );
    }

    #[test]
    fn failed_startup_pause_keeps_pending_and_retries_after_the_bounded_cooldown() {
        let now = Instant::now();
        let mut watch = AutoPauseWatch::default();
        watch.configure(true, true, true);
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
            PauseDecision::StartupPause
        );
        watch.pause_succeeded();
        assert_eq!(
            watch.observe(now + Duration::from_secs(120), false, 90),
            PauseDecision::Idle
        );
    }

    #[test]
    fn game_start_during_login_or_retry_cancels_startup_and_requires_normal_exit_grace() {
        let now = Instant::now();
        let mut watch = AutoPauseWatch::default();
        watch.configure(true, true, true);
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
        }
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

    #[test]
    fn valid_empty_snapshot_executes_one_pause_and_normal_exit_ignores_startup_opt_out() {
        for startup in [true, false] {
            let cfg = configured_fixture();
            // Disabling startup recovery must leave normal game-exit pauses enabled.
            cfg.lock().unwrap().strategy.pause_on_startup = startup;
            let calls = Cell::new(0);
            let snapshots = Cell::new(0);
            let result = request_after_token(
                || Some("offline-fixture-token".to_string()),
                || {
                    auto_pause_guard_with_snapshot(&cfg, startup, || {
                        snapshots.set(snapshots.get() + 1);
                        Ok(Vec::new())
                    })
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
        let game_started = Cell::new(false);
        let calls = Cell::new(0);
        let result = request_after_token(
            || {
                // Simulate the game launching while a slow login completes.
                game_started.set(true);
                Some("offline-fixture-token".to_string())
            },
            || {
                auto_pause_guard_with_snapshot(&cfg, true, || {
                    Ok(if game_started.get() {
                        vec!["FIXTURE.EXE".into()]
                    } else {
                        Vec::new()
                    })
                })
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
                || auto_pause_guard_with_snapshot(&cfg, true, || Err("fixture scan failed".into())),
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
}
