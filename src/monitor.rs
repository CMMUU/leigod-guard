//! Windows 进程监控：Toolhelp32 快照，纯外部观察，不注入不读写内存。
use windows::Win32::Foundation::{CloseHandle, ERROR_NO_MORE_FILES};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};

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
    fn matches_whole_executable_names_case_insensitively() {
        let processes = vec!["TSLGAME.EXE".into(), "game.exe.backup".into()];
        let watch = vec![
            ("PUBG".into(), "TslGame.exe".into()),
            ("Other".into(), "game.exe".into()),
        ];
        assert_eq!(match_games(&processes, &watch), vec!["PUBG"]);
    }
}
