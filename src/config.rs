use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
        }
    }
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
    use super::Config;

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
                Ok(mut cfg) => {
                    // serde(default) 仅处理缺失字段，旧文件缺失 strategy 时补默认
                    cfg.games.retain(|g| !g.exe.trim().is_empty());
                    cfg
                }
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
