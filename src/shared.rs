//! UI 线程与后台工作线程之间的共享状态。
use std::collections::VecDeque;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug)]
pub enum ManualCmd {
    Pause,
    Resume,
}

/// Runtime-only startup protection state. No account or accelerator state is
/// implied; None means the worker still needs a successful process observation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StartupPauseStatus {
    pub pending: bool,
    pub remaining_secs: Option<u64>,
    pub preparing_game: bool,
}

/// Display-only countdown anchored to the worker's actual game-exit observation.
/// Repainting this value never scans processes or sends a pause request.
#[derive(Clone, Copy, Debug)]
pub struct ExitGraceCountdown {
    pub started_at: Instant,
    pub grace_secs: u64,
}

impl ExitGraceCountdown {
    fn status_at(self, now: Instant) -> String {
        let remaining = Duration::from_secs(self.grace_secs)
            .saturating_sub(now.saturating_duration_since(self.started_at));
        if remaining.is_zero() {
            "游戏已退出，宽限期已结束，等待复核".into()
        } else {
            let seconds = remaining
                .as_secs()
                .saturating_add(u64::from(remaining.subsec_nanos() != 0));
            format!("游戏已退出，{seconds} 秒后自动暂停")
        }
    }
}

pub struct Shared {
    /// 当前状态描述：空闲 / 加速中 / 宽限期倒计时 等
    pub status: String,
    pub exit_grace_countdown: Option<ExitGraceCountdown>,
    /// 当前检测到的在运行名单游戏
    pub running_games: Vec<String>,
    /// Latest successful basename-only process observation for the UI.
    /// None means uninitialized or failed; never display it as all games stopped.
    pub process_snapshot: Option<Vec<String>>,
    /// 滚动日志
    pub logs: VecDeque<String>,
    /// 需要弹窗告警的消息（暂停/恢复失败等），UI 线程取出后弹窗
    pub alert: Option<String>,
    /// 账户状态展示文本
    pub account_status: String,
    /// 最近一次 user_info 查询的原始结果（仅在内存中提取状态与时长）
    pub account_info: Option<serde_json::Value>,
    pub account_info_updated_at: Option<chrono::DateTime<chrono::Local>>,
    pub account_info_received_at: Option<Instant>,
    /// A tray open can happen while egui is asleep and misses the focus change.
    pub account_refresh_requested: bool,
    /// UI 手动指令
    pub manual_cmd: Option<ManualCmd>,
    /// 最近一次手动暂停的结果；独立于会被监控状态覆盖的展示文本。
    pub manual_pause_result: Option<bool>,
    /// Published by the worker; controls whether a startup-only deferral is useful.
    pub startup_pause_status: StartupPauseStatus,
    /// UI/tray request only: protect startup checks until at least click + 10 min.
    /// The worker consumes this even while a slow login is completing.
    pub startup_defer_requested_at: Option<Instant>,
    /// 内存中的 token（不落盘明文）
    pub token: Option<String>,
    /// 极验人机验证结果：Some("")=用户关闭窗口取消；Some(json)=验证通过的三元组
    pub captcha_result: Option<String>,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            status: "初始化…".into(),
            exit_grace_countdown: None,
            running_games: Vec::new(),
            process_snapshot: None,
            logs: VecDeque::with_capacity(500),
            alert: None,
            account_status: "未登录".into(),
            account_info: None,
            account_info_updated_at: None,
            account_info_received_at: None,
            account_refresh_requested: false,
            manual_cmd: None,
            manual_pause_result: None,
            startup_pause_status: StartupPauseStatus {
                pending: true,
                ..StartupPauseStatus::default()
            },
            startup_defer_requested_at: None,
            token: None,
            captcha_result: None,
        }
    }
}

impl Shared {
    pub fn status_at(&self, now: Instant) -> String {
        self.exit_grace_countdown
            .map_or_else(|| self.status.clone(), |countdown| countdown.status_at(now))
    }

    pub fn set_status(&mut self, status: &str) {
        self.exit_grace_countdown = None;
        self.status = status.into();
    }

    pub fn set_exit_grace(&mut self, started_at: Instant, grace_secs: u64) {
        let countdown = ExitGraceCountdown {
            started_at,
            grace_secs,
        };
        self.status = countdown.status_at(Instant::now());
        self.exit_grace_countdown = Some(countdown);
    }

    pub fn clear_account_info(&mut self) {
        self.account_info = None;
        self.account_info_updated_at = None;
        self.account_info_received_at = None;
    }

    pub fn set_token(&mut self, token: Option<String>) {
        if self.token != token {
            self.clear_account_info();
        }
        self.token = token;
    }

    pub fn set_account_info(&mut self, token: &str, value: serde_json::Value) {
        if self.token.as_deref() == Some(token) {
            self.account_info = Some(value);
            self.account_info_updated_at = Some(chrono::Local::now());
            self.account_info_received_at = Some(Instant::now());
        }
    }

    pub fn log(&mut self, msg: &str) {
        let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        self.logs.push_back(format!("[{ts}] {msg}"));
        while self.logs.len() > 500 {
            self.logs.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_countdown_ticks_each_second_without_another_worker_update() {
        let start = Instant::now();
        let mut shared = Shared::default();
        shared.set_exit_grace(start, 90);
        for (millis, seconds) in [
            (0, 90),
            (999, 90),
            (1000, 89),
            (2000, 88),
            (2999, 88),
            (3000, 87),
        ] {
            assert_eq!(
                shared.status_at(start + Duration::from_millis(millis)),
                format!("游戏已退出，{seconds} 秒后自动暂停")
            );
        }
        for seconds in [90, 91, 120] {
            assert_eq!(
                shared.status_at(start + Duration::from_secs(seconds)),
                "游戏已退出，宽限期已结束，等待复核"
            );
        }
        assert!(shared.manual_cmd.is_none());
        assert!(shared.manual_pause_result.is_none());
    }

    #[test]
    fn leaving_exit_grace_discards_the_display_countdown() {
        let start = Instant::now();
        let mut shared = Shared::default();
        for status in [
            "游戏运行中",
            "进程检测失败",
            "自动暂停已停用",
            "正在暂停计时…",
            "暂停失败，等待重试",
        ] {
            shared.set_exit_grace(start, 90);
            shared.set_status(status);
            assert!(shared.exit_grace_countdown.is_none());
            assert_eq!(shared.status_at(start + Duration::from_secs(20)), status);
        }
    }

    #[test]
    fn old_account_responses_cannot_restore_balance_after_token_changes() {
        let mut shared = Shared::default();
        shared.set_token(Some("fixture-a".into()));
        shared.set_account_info(
            "fixture-a",
            serde_json::json!({"data": {"expiry_time_samp": 3600}}),
        );
        assert!(shared.account_info_received_at.is_some());
        shared.set_token(Some("fixture-b".into()));
        assert!(shared.account_info.is_none());
        assert!(shared.account_info_updated_at.is_none());
        shared.set_account_info(
            "fixture-a",
            serde_json::json!({"data": {"expiry_time_samp": 9999}}),
        );
        assert!(shared.account_info.is_none());
        shared.set_token(None);
        shared.set_account_info(
            "fixture-b",
            serde_json::json!({"data": {"expiry_time_samp": 99}}),
        );
        assert!(shared.account_info_received_at.is_none());
    }
}
