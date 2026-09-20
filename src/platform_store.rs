//! Separate DPAPI-protected platform session. Never touches accelerator credentials.
use crate::platform_api::{Session, ORIGIN_URL};
use std::path::PathBuf;

pub trait Store {
    fn load(&self) -> Result<Option<Session>, String>;
    fn save(&self, session: &Session) -> Result<(), String>;
    fn clear(&self) -> Result<(), String>;
}
pub struct DiskStore(pub PathBuf);
impl DiskStore {
    pub fn current_user() -> Self {
        Self(crate::config::Config::path().with_file_name("platform-session.dat"))
    }
    fn temp(&self) -> PathBuf {
        self.0.with_extension("tmp")
    }
}
impl Store for DiskStore {
    fn load(&self) -> Result<Option<Session>, String> {
        let file = match std::fs::File::open(&self.0) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err("无法读取已保存的平台登录状态，请重新登录。".into()),
        };
        use std::io::Read;
        let mut cipher = String::new();
        file.take(32769)
            .read_to_string(&mut cipher)
            .map_err(|_| "保存的登录状态无法读取，请重新登录。")?;
        if cipher.len() > 32768 {
            return Err("保存的登录状态无效，请重新登录。".into());
        }
        let plain = crate::dpapi::unprotect(&cipher)
            .map_err(|_| "无法解密登录状态，请使用当前 Windows 用户重新登录。")?;
        let session: Session =
            serde_json::from_str(&plain).map_err(|_| "保存的登录状态无效，请重新登录。")?;
        if !session.valid_for(ORIGIN_URL, chrono::Utc::now().timestamp()) {
            return Err("平台登录已过期，请重新登录。".into());
        }
        Ok(Some(session))
    }
    fn save(&self, session: &Session) -> Result<(), String> {
        let plain = serde_json::to_string(session).map_err(|_| "无法保存登录状态。")?;
        let cipher = crate::dpapi::protect(&plain)
            .map_err(|_| "无法加密登录状态，本次登录仅在本次运行中保留。")?;
        let result = (|| -> std::io::Result<()> {
            use std::io::Write;
            if let Some(parent) = self.0.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut file = std::fs::File::create(self.temp())?;
            file.write_all(cipher.as_bytes())?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(self.temp(), &self.0)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(self.temp());
            return Err("无法保存登录状态，本次登录仅在本次运行中保留。".into());
        }
        Ok(())
    }
    fn clear(&self) -> Result<(), String> {
        // Attempt both files even if one is locked, so a stale temp is not retained.
        let mut failed = false;
        for path in [&self.0, &self.temp()] {
            if let Err(e) = std::fs::remove_file(path) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    failed = true;
                }
            }
        }
        if failed {
            Err("本机登录文件未能清除，请重试退出；不要将该文件分享给他人。".into())
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dpapi_store_roundtrips_replaces_and_clears_only_its_own_files() {
        let root = std::env::temp_dir().join(format!(
            "guard-platform-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let store = DiskStore(root.join("platform-session.dat"));
        let mut session = Session {
            origin: ORIGIN_URL.into(),
            token: "a".repeat(64),
            expires_at: chrono::Utc::now().timestamp() + 3600,
            user: crate::platform_api::User {
                id: "test-user".into(),
                username: "test-user".into(),
                display_name: "测试".into(),
                role: "user".into(),
                csrf: "b".repeat(64),
            },
        };
        assert!(store.load().unwrap().is_none());
        store.save(&session).unwrap();
        let cipher = std::fs::read_to_string(&store.0).unwrap();
        assert!(!cipher.contains(&session.token));
        assert!(!cipher.contains("test-user"));
        assert_eq!(store.load().unwrap().unwrap().token, session.token);
        session.token = "c".repeat(64);
        store.save(&session).unwrap();
        assert_eq!(store.load().unwrap().unwrap().token, session.token);
        let accelerator = root.join("config.toml");
        std::fs::write(&accelerator, "untouched-account-fixture").unwrap();
        std::fs::write(store.temp(), "incomplete encrypted write fixture").unwrap();
        store.clear().unwrap();
        assert!(store.load().unwrap().is_none());
        assert!(!store.temp().exists());
        assert_eq!(
            std::fs::read_to_string(&accelerator).unwrap(),
            "untouched-account-fixture"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
