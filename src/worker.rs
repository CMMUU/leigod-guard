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

    // 精简模式（v0.5）：只做"游戏全退后自动暂停"，不自动恢复计时。
    let mut pause_watch = PauseWatch::default();
    let mut next_retry: Option<Instant> = None;
    let mut monitor_failed = false;

    loop {
        let (interval, enabled, grace_secs, watch) = {
            let c = match cfg.lock() {
                Ok(c) => c.clone(),
                Err(_) => {
                    std::thread::sleep(Duration::from_secs(3));
                    continue;
                }
            };
            let watch: Vec<(String, String)> = c
                .games
                .iter()
                .map(|g| (g.name.clone(), g.exe.clone()))
                .collect();
            (
                c.strategy.check_interval_secs.max(1),
                c.strategy.enabled,
                c.strategy.grace_secs,
                watch,
            )
        };

        // 处理 UI 手动指令
        let cmd = shared.lock().ok().and_then(|mut s| s.manual_cmd.take());
        if let Some(cmd) = cmd {
            match cmd {
                ManualCmd::Pause => match call_with_retry(&shared, &cfg, api::pause, "暂停") {
                    Ok(msg) => {
                        pause_watch.reset();
                        next_retry = None;
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

        if !enabled {
            // 停用后清掉旧的退出/失败记录，重新启用需要重新观察到游戏运行。
            pause_watch.reset();
            next_retry = None;
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
        let matched = monitor::match_games(&processes, &watch);
        if let Ok(mut s) = shared.lock() {
            s.running_games = matched.clone();
        }

        if !enabled {
            set_status(&shared, "自动暂停已停用");
            std::thread::sleep(Duration::from_secs(interval));
            continue;
        }

        let now = Instant::now();
        if next_retry.is_some_and(|t| now >= t) {
            next_retry = None;
        }

        match pause_watch.observe(now, !matched.is_empty(), grace_secs) {
            PauseDecision::Running => {
                // 即使在 API 冷却期，也必须观察游戏重新启动并取消旧倒计时。
                next_retry = None;
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
            PauseDecision::Pause if next_retry.is_some() => {
                set_status(&shared, "暂停失败，等待重试");
            }
            PauseDecision::Pause => match call_with_retry(&shared, &cfg, api::pause, "暂停") {
                Ok(msg) => {
                    pause_watch.reset();
                    next_retry = None;
                    log(&shared, &format!("宽限期结束，已自动暂停计时: {msg}"));
                    crate::ui::dbglog(&format!("[worker] auto-pause ok: {msg}"));
                    set_status(&shared, "已暂停计时");
                    refresh_account_info(&shared, &cfg);
                }
                Err(e) => {
                    alert(&shared, &format!("自动暂停失败：{e}。时长仍在消耗，请立即手动暂停（客户端/公众号/官网）！"));
                    next_retry = Some(Instant::now() + Duration::from_secs(FAIL_COOLDOWN_SECS));
                    set_status(&shared, "暂停失败，等待重试");
                }
            },
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
    let mut last_err = String::new();
    for attempt in 1..=MAX_RETRY {
        let token =
            match ensure_token(shared, cfg) {
                Some(t) => t,
                None => return Err(
                    "token 缺失且无法自动重登：请在“账户”页重新登录（验证码方式）或重新粘贴 token"
                        .to_string(),
                ),
            };
        match f(&token) {
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
    Err(last_err)
}

#[cfg(test)]
mod tests {
    use super::{PauseDecision, PauseWatch};
    use std::time::{Duration, Instant};

    #[test]
    fn startup_idle_never_pauses_without_observing_a_game() {
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
}
