use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const DEFAULT_STARTUP_GRACE_SECS: u64 = 180;

fn default_startup_grace_secs() -> u64 {
    DEFAULT_STARTUP_GRACE_SECS
}

/// 监控的游戏条目
#[derive(Clone, Serialize, Deserialize)]
pub struct GameEntry {
    /// 显示名称，如 PUBG
    pub name: String,
    /// 进程名，如 TslGame.exe
    pub exe: String,
    /// 关联的加速方案名（AccelPlan.name），可为空
    pub plan: String,
}

/// 预设加速方案（二期：抓取自雷神客户端的实际加速参数）
#[derive(Clone, Serialize, Deserialize)]
pub struct AccelPlan {
    pub name: String,
    pub game: String,
    pub region: String,
    pub node: String,
    pub mode: String,
    pub note: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Strategy {
    /// 自动启停总开关
    pub enabled: bool,
    /// 进程检测间隔（秒）
    pub check_interval_secs: u64,
    /// 游戏退出后暂停宽限期（秒）
    pub grace_secs: u64,
    /// 加速启动后的最短运行时间（秒），防止误启停
    pub min_run_secs: u64,
    /// 开机自启
    pub autostart: bool,
    /// 关机/注销前自动暂停计时
    #[serde(default = "default_true")]
    pub pause_on_shutdown: bool,
    /// 每次启动时，名单有效且无名单游戏运行则补暂停一次。
    #[serde(default = "default_true")]
    pub pause_on_startup: bool,
    /// 启动后连续确认没有名单游戏的宽限时间，与游戏退出宽限期独立。
    #[serde(default = "default_startup_grace_secs")]
    pub startup_grace_secs: u64,
}

fn default_true() -> bool {
    true
}

impl Default for Strategy {
    fn default() -> Self {
        Self {
            enabled: true,
            check_interval_secs: 3,
            grace_secs: 90,
            min_run_secs: 300,
            autostart: false,
            pause_on_shutdown: true,
            pause_on_startup: true,
            startup_grace_secs: DEFAULT_STARTUP_GRACE_SECS,
        }
    }
}

/// Process snapshots contain executable basenames, never full paths or patterns.
/// Use the same validation when adding custom games and deciding pause safety.
pub fn valid_game_executable(exe: &str) -> bool {
    let exe = exe.trim();
    if exe.len() <= 4
        || !exe.to_ascii_lowercase().ends_with(".exe")
        || exe
            .chars()
            .any(|ch| ch.is_control() || "/\\:<>\"|?*".contains(ch))
    {
        return false;
    }
    let stem = exe.split('.').next().unwrap_or_default().to_uppercase();
    let numbered_device = (stem.starts_with("COM") || stem.starts_with("LPT"))
        && stem.get(3..).is_some_and(|suffix| {
            suffix.chars().count() == 1
                && matches!(suffix.chars().next(), Some('1'..='9' | '¹' | '²' | '³'))
        });
    !stem.is_empty() && !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL") && !numbered_device
}

/// 账户凭据（密码哈希与 token 均经 DPAPI 加密后存储）
#[derive(Clone, Serialize, Deserialize, Default)]
pub struct Account {
    pub username: String,
    /// DPAPI 加密后的 MD5(密码)
    pub cred_enc: String,
    /// DPAPI 加密后的 account_token
    pub token_enc: String,
}

#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Updates {
    /// 仅检查公开版本信息；下载安装仍需用户点击。
    pub check_on_startup: bool,
}

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct Config {
    pub games: Vec<GameEntry>,
    pub plans: Vec<AccelPlan>,
    #[serde(default)]
    pub strategy: Strategy,
    #[serde(default)]
    pub account: Account,
    #[serde(default)]
    pub updates: Updates,
}

#[cfg(test)]
mod tests {
    use super::{valid_game_executable, Config};

    #[test]
    fn older_config_does_not_enable_update_requests() {
        let cfg: Config = toml::from_str("games = []\nplans = []\n").unwrap();
        assert!(!cfg.updates.check_on_startup);
        assert!(cfg.strategy.enabled);
    }

    #[test]
    fn update_preference_survives_serialization() {
        let mut cfg = Config::default();
        cfg.updates.check_on_startup = true;
        let text = toml::to_string(&cfg).unwrap();
        let restored: Config = toml::from_str(&text).unwrap();
        assert!(restored.updates.check_on_startup);
    }

    #[test]
    fn older_strategy_enables_startup_recovery_but_preserves_explicit_opt_out() {
        let old = "games = []\nplans = []\n[strategy]\nenabled = true\ncheck_interval_secs = 3\ngrace_secs = 90\nmin_run_secs = 300\nautostart = false\n";
        let cfg: Config = toml::from_str(old).unwrap();
        assert!(cfg.strategy.pause_on_startup);
        assert!(Config::default().strategy.pause_on_startup);
        assert_eq!(cfg.strategy.startup_grace_secs, 180);
        assert_eq!(Config::default().strategy.startup_grace_secs, 180);
        let opted_out: Config =
            toml::from_str(&format!("{old}pause_on_startup = false\n")).unwrap();
        assert!(!opted_out.strategy.pause_on_startup);
        let roundtrip: Config = toml::from_str(&toml::to_string(&opted_out).unwrap()).unwrap();
        assert!(!roundtrip.strategy.pause_on_startup);
    }

    #[test]
    fn startup_grace_migrates_from_v070_and_is_independent_of_exit_grace() {
        let old = "games = []\nplans = []\n[strategy]\nenabled = true\ncheck_interval_secs = 3\ngrace_secs = 90\nmin_run_secs = 300\nautostart = false\npause_on_startup = true\n";
        let mut cfg: Config = toml::from_str(old).unwrap();
        assert_eq!(cfg.strategy.startup_grace_secs, 180);
        cfg.strategy.startup_grace_secs = 300;
        let restored: Config = toml::from_str(&toml::to_string(&cfg).unwrap()).unwrap();
        assert_eq!(restored.strategy.startup_grace_secs, 300);
        assert_eq!(restored.strategy.grace_secs, 90);
    }

    #[test]
    fn game_executables_are_trimmed_basenames_not_paths_patterns_or_devices() {
        for valid in [
            "TslGame.exe",
            " javaw.EXE ",
            "Game-Win64-Shipping.exe",
            "我的游戏.exe",
        ] {
            assert!(valid_game_executable(valid), "rejected {valid}");
        }
        for invalid in [
            "",
            " ",
            ".exe",
            "game",
            "game.*",
            "a/game.exe",
            "C:\\game.exe",
            "game.exe:stream",
            "CON.exe",
            "COM1.exe",
            "LPT³.exe",
            "game\n.exe",
        ] {
            assert!(!valid_game_executable(invalid), "accepted {invalid}");
        }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        let base = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        base.join("leigod-guard").join("config.toml")
    }

    pub fn load() -> Self {
        let path = Self::path();
        match std::fs::read_to_string(&path) {
            Ok(text) => match toml::from_str::<Config>(&text) {
                // Preserve invalid entries so the worker can fail closed for
                // the entire watch list and the user can correct them in the UI.
                Ok(cfg) => cfg,
                Err(_) => {
                    eprintln!("配置解析失败，使用默认配置。请检查配置格式。");
                    Config {
                        strategy: Strategy::default(),
                        ..Config::default()
                    }
                }
            },
            Err(_) => Config {
                strategy: Strategy::default(),
                ..Config::default()
            },
        }
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let text = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&path, text).map_err(|e| e.to_string())
    }
}
