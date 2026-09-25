//! Windows 进程监控：Toolhelp32 快照，纯外部观察，不注入不读写内存。
use crate::game_lifecycle::{Observation, Phase, Process, ProcessId, Tracker};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use windows::Win32::Foundation::FILETIME;
use windows::Win32::Foundation::{CloseHandle, ERROR_NO_MORE_FILES};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};

struct MonitorState {
    tracker: Tracker,
    last: Observation,
    valid_for: Duration,
    manual_until: Option<Instant>,
    enabled: bool,
}

pub struct GameMonitor(Mutex<MonitorState>);

impl GameMonitor {
    #[cfg(test)]
    pub fn fixture(observation: Observation) -> Arc<Self> {
        Arc::new(Self(Mutex::new(MonitorState {
            tracker: Tracker::default(),
            last: observation,
            valid_for: Duration::from_secs(7),
            manual_until: None,
            enabled: true,
        })))
    }
    pub fn start(config: Arc<Mutex<crate::config::Config>>) -> Arc<Self> {
        let monitor = Arc::new(Self(Mutex::new(MonitorState {
            tracker: Tracker::default(),
            last: Observation::unknown(Instant::now()),
            valid_for: Duration::from_secs(7),
            manual_until: None,
            enabled: false,
        })));
        let bg = monitor.clone();
        if std::thread::Builder::new()
            .name("game-observer".into())
            .spawn(move || loop {
                let (cfg, interval) = match config.lock() {
                    Ok(c) => (Some(c.clone()), c.strategy.check_interval_secs.max(1)),
                    Err(_) => (None, 3),
                };
                if let Some(cfg) = cfg {
                    let _ = bg.observe_now(&cfg);
                }
                std::thread::sleep(Duration::from_secs(interval));
            })
            .is_err()
        {
            crate::ui::dbglog(
                "[game] observer thread unavailable; automatic actions require fresh verification",
            );
        }
        monitor
    }

    pub fn latest(&self) -> Observation {
        let now = Instant::now();
        self.0
            .lock()
            .ok()
            .and_then(|s| {
                (now.saturating_duration_since(s.last.at) <= s.valid_for).then(|| s.last.clone())
            })
            .unwrap_or_else(|| Observation::unknown(now))
    }

    pub fn defer(&self, clicked: Instant) -> bool {
        if let Ok(mut s) = self.0.lock() {
            if !s.enabled || s.last.phase == Phase::Running {
                return false;
            }
            s.manual_until = Some(clicked + Duration::from_secs(600));
            crate::ui::dbglog("[game] explicit preparation requested; 600 seconds");
            true
        } else {
            false
        }
    }

    pub fn manual_remaining(&self) -> u64 {
        let now = Instant::now();
        self.0
            .lock()
            .ok()
            .and_then(|s| s.manual_until)
            .map_or(0, |until| {
                until.saturating_duration_since(now).as_secs().min(600)
            })
    }

    pub fn observe_now(&self, cfg: &crate::config::Config) -> Result<Observation, String> {
        // Serialize only process observation. Never hold this mutex over HTTP.
        let mut s = self.0.lock().map_err(|_| "游戏观察状态不可用")?;
        s.enabled = cfg.strategy.enabled;
        let watch: Vec<_> = cfg
            .games
            .iter()
            .map(|g| (g.name.clone(), g.exe.trim().to_string()))
            .collect();
        let valid = !watch.is_empty()
            && watch
                .iter()
                .all(|(_, e)| crate::config::valid_game_executable(e));
        let scan = if valid {
            process_snapshot(&watch).map_err(|_| "进程快照不可用")
        } else {
            Err("名单为空或无效")
        };
        let now = Instant::now();
        let mut out = match scan {
            Ok(processes) => {
                s.tracker
                    .observe(now, &processes, &watch, cfg.strategy.launch_grace_secs)
            }
            Err(e) => {
                if s.last.phase != Phase::Unknown {
                    crate::ui::dbglog("[game] observation=unknown; automatic pause held");
                }
                s.last = Observation::unknown(now);
                return Err(e.into());
            }
        };
        if out.phase == Phase::Running || !cfg.strategy.enabled {
            s.manual_until = None;
        }
        if let Some(until) = s.manual_until.filter(|u| *u > now) {
            out.phase = Phase::Launching;
            out.activity_seen = true;
            out.launch_until = Some(out.launch_until.map_or(until, |u| u.max(until)));
        }
        if out.phase != s.last.phase
            || out.generation != s.last.generation
            || out.evidence != s.last.evidence
        {
            crate::ui::dbglog(&format!(
                "[game] phase={:?} generation={} remaining={} evidence={:?}",
                out.phase,
                out.generation,
                out.remaining(now),
                out.evidence
            ));
        }
        s.valid_for = Duration::from_secs(
            cfg.strategy
                .check_interval_secs
                .max(1)
                .saturating_mul(2)
                .saturating_add(1),
        );
        s.last = out.clone();
        Ok(out)
    }
}

fn process_snapshot(watch: &[(String, String)]) -> windows::core::Result<Vec<Process>> {
    unsafe {
        let handle = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)?;
        let result = (|| {
            let mut list = Vec::new();
            let mut entry = PROCESSENTRY32W {
                dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
                ..Default::default()
            };
            Process32FirstW(handle, &mut entry)?;
            loop {
                let end = entry
                    .szExeFile
                    .iter()
                    .position(|c| *c == 0)
                    .unwrap_or(entry.szExeFile.len());
                let exe = String::from_utf16_lossy(&entry.szExeFile[..end]);
                let relevant = watch.iter().any(|(_, e)| {
                    e.eq_ignore_ascii_case(&exe)
                        || crate::game_lifecycle::launchers(e)
                            .iter()
                            .any(|l| l.eq_ignore_ascii_case(&exe))
                });
                let created = if relevant {
                    creation_time(entry.th32ProcessID)
                } else {
                    None
                };
                let now_ticks = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .ok()
                    .map(|d| (d.as_nanos() / 100) as u64 + 116444736000000000);
                let age = created
                    .zip(now_ticks)
                    .and_then(|(birth, now)| now.checked_sub(birth))
                    .map(|ticks| Duration::from_nanos(ticks.saturating_mul(100)));
                list.push(Process {
                    id: ProcessId {
                        pid: entry.th32ProcessID,
                        created,
                        exe,
                    },
                    age,
                });
                match Process32NextW(handle, &mut entry) {
                    Ok(()) => {}
                    Err(e) if e.code() == ERROR_NO_MORE_FILES.to_hresult() => break,
                    Err(e) => return Err(e),
                }
            }
            Ok(list)
        })();
        let _ = CloseHandle(handle);
        result
    }
}

unsafe fn creation_time(pid: u32) -> Option<u64> {
    let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
    let mut created = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    let result = GetProcessTimes(handle, &mut created, &mut exit, &mut kernel, &mut user);
    let _ = CloseHandle(handle);
    result
        .ok()
        .map(|_| (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
}

/// 供进程选择界面展示；自动暂停必须使用能区分失败的 try_running_process_names。
pub fn running_process_names() -> Vec<String> {
    try_running_process_names().unwrap_or_default()
}

/// 返回完整快照中的可执行文件名；枚举失败时不能把空列表当作游戏已退出。
pub fn try_running_process_names() -> windows::core::Result<Vec<String>> {
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)?;
        // 将枚举封装在闭包中，确保任何错误返回前都会关闭快照句柄。
        let result = (|| {
            let mut names = Vec::new();
            let mut entry = PROCESSENTRY32W {
                dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
                ..Default::default()
            };
            Process32FirstW(snapshot, &mut entry)?;
            loop {
                let end = entry
                    .szExeFile
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(entry.szExeFile.len());
                let name = String::from_utf16_lossy(&entry.szExeFile[..end]);
                if !name.is_empty() {
                    names.push(name);
                }
                match Process32NextW(snapshot, &mut entry) {
                    Ok(()) => {}
                    Err(e) if e.code() == ERROR_NO_MORE_FILES.to_hresult() => break,
                    // 丢弃不完整列表，避免遗漏还在运行的游戏。
                    Err(e) => return Err(e),
                }
            }
            Ok(names)
        })();
        let _ = CloseHandle(snapshot);
        result
    }
}

/// 在进程列表中查找名单内的游戏，返回匹配到的游戏名（小写 exe 匹配）
pub fn match_games(processes: &[String], watch_exes: &[(String, String)]) -> Vec<String> {
    let lower: Vec<String> = processes.iter().map(|p| p.to_lowercase()).collect();
    let mut hits = Vec::new();
    for (name, exe) in watch_exes {
        let exe_l = exe.to_lowercase();
        if lower.iter().any(|p| p == &exe_l) {
            hits.push(name.clone());
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::match_games;

    #[test]
    fn stale_observation_becomes_unknown_without_hiding_its_failure() {
        let mut observation = crate::game_lifecycle::Observation::unknown(
            std::time::Instant::now() - std::time::Duration::from_secs(30),
        );
        observation.phase = crate::game_lifecycle::Phase::Absent;
        assert_eq!(
            super::GameMonitor::fixture(observation)
                .latest()
                .game_running(),
            None
        );
    }

    #[test]
    fn matches_whole_executable_names_case_insensitively() {
        let processes = vec!["TSLGAME.EXE".into(), "game.exe.backup".into()];
        let watch = vec![
            ("PUBG".into(), "TslGame.exe".into()),
            ("Other".into(), "game.exe".into()),
        ];
        assert_eq!(match_games(&processes, &watch), vec!["PUBG"]);
    }
}
