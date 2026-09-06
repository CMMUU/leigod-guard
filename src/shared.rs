//! UI 线程与后台工作线程之间的共享状态。
use std::collections::VecDeque;
use std::time::Instant;

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

pub struct Shared {
    /// 当前状态描述：空闲 / 加速中 / 宽限期倒计时 等
    pub status: String,
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
            running_games: Vec::new(),
            process_snapshot: None,
            logs: VecDeque::with_capacity(500),
            alert: None,
            account_status: "未登录".into(),
            account_info: None,
            account_info_updated_at: None,
            account_info_received_at: None,
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
