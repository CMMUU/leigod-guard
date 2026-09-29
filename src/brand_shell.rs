//! Refresh this application's existing shortcuts without restarting Explorer.
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use windows::core::{Interface, PCWSTR};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED, STGM_READWRITE,
};
use windows::Win32::UI::Shell::{
    IShellLinkW, SHChangeNotify, ShellLink, SHCNE_UPDATEITEM, SHCNF_FLUSHNOWAIT, SHCNF_PATHW,
};

const ICON: &[u8] = include_bytes!("../assets/app-icon.ico");

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

fn path_from_wide(buffer: &[u16]) -> PathBuf {
    let end = buffer
        .iter()
        .position(|&value| value == 0)
        .unwrap_or(buffer.len());
    std::ffi::OsString::from_wide(&buffer[..end]).into()
}

fn same_file(left: &Path, right: &Path) -> bool {
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left
            .as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(&right.as_os_str().to_string_lossy()),
        _ => false,
    }
}

fn write_icon(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    fs::create_dir_all(root)?;
    // Changing the artwork changes its path, so the shell cannot reuse an old
    // icon cached for the executable's unchanged path. Keep old pinned resources.
    let name = format!("app-icon-{:x}.ico", Sha256::digest(ICON));
    let path = root.join(name);
    if path.exists() {
        if fs::read(&path)? != ICON {
            return Err("brand cache contents differ".into());
        }
    } else {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        file.write_all(ICON)?;
    }
    Ok(path)
}

struct ComApartment;
impl ComApartment {
    fn new() -> windows::core::Result<Self> {
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        }
        Ok(Self)
    }
}
impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}

fn refresh_link(link_path: &Path, executable: &Path, icon: &Path) -> windows::core::Result<bool> {
    // Never resolve a shortcut: resolving can invoke shell UI or contact a share.
    // Only a directly stored target that is this executable is eligible.
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        let file: IPersistFile = link.cast()?;
        let link_name = wide(link_path);
        file.Load(PCWSTR(link_name.as_ptr()), STGM_READWRITE)?;
        let mut target = [0u16; 32768];
        link.GetPath(&mut target, std::ptr::null_mut(), 4)?; // SLGP_RAWPATH
        let target = path_from_wide(&target);
        if !target.is_absolute()
            || target.to_string_lossy().starts_with(r"\\")
            || !same_file(&target, executable)
        {
            return Ok(false);
        }
        let mut old_icon = [0u16; 32768];
        let mut index = 0;
        link.GetIconLocation(&mut old_icon, &mut index)?;
        if index == 0 && path_from_wide(&old_icon) == icon {
            return Ok(false);
        }
        let icon_name = wide(icon);
        link.SetIconLocation(PCWSTR(icon_name.as_ptr()), 0)?;
        file.Save(PCWSTR(link_name.as_ptr()), true)?;
        SHChangeNotify(
            SHCNE_UPDATEITEM,
            SHCNF_PATHW | SHCNF_FLUSHNOWAIT,
            Some(link_name.as_ptr().cast()),
            None,
        );
        Ok(true)
    }
}

pub fn refresh_owned_shortcuts() -> Result<(), Box<dyn std::error::Error>> {
    let _com = ComApartment::new()?;
    let executable = std::env::current_exe()?;
    let local = dirs::data_local_dir().ok_or("local app data unavailable")?;
    let icon = write_icon(&local.join("LeigodGuard").join("branding"))?;
    let mut folders = Vec::new();
    if let Some(desktop) = dirs::desktop_dir() {
        folders.push(desktop);
    }
    if let Some(roaming) = dirs::data_dir() {
        folders.push(roaming.join(r"Microsoft\Windows\Start Menu\Programs\LeigodGuard"));
        folders.push(roaming.join(r"Microsoft\Internet Explorer\Quick Launch\User Pinned\TaskBar"));
    }
    for folder in folders {
        let Ok(entries) = fs::read_dir(folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !entry.file_type().is_ok_and(|kind| kind.is_file())
                || !path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("lnk"))
            {
                continue;
            }
            // One unavailable/broken shortcut must not prevent the others.
            let _ = refresh_link(&path, &executable, &icon);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refreshes_only_matching_shortcut_and_preserves_launch_options() {
        // A separate COM apartment and temporary links; no real user shortcuts.
        std::thread::spawn(|| {
            let _com = ComApartment::new().unwrap();
            let root =
                std::env::temp_dir().join(format!("guard-brand-test-{}", std::process::id()));
            fs::create_dir(&root).unwrap();
            let exe = root.join("leigod-guard.exe");
            fs::write(&exe, b"inert fixture").unwrap();
            let icon = write_icon(&root).unwrap();
            assert_eq!(write_icon(&root).unwrap(), icon);
            assert_eq!(fs::read(&icon).unwrap(), ICON);
            let shortcut = root.join("legacy.lnk");
            unsafe {
                let link: IShellLinkW =
                    CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).unwrap();
                link.SetPath(PCWSTR(wide(&exe).as_ptr())).unwrap();
                link.SetArguments(windows::core::w!("--minimized")).unwrap();
                link.SetWorkingDirectory(PCWSTR(wide(&root).as_ptr()))
                    .unwrap();
                let file: IPersistFile = link.cast().unwrap();
                file.Save(PCWSTR(wide(&shortcut).as_ptr()), true).unwrap();
            }
            let before = fs::read(&shortcut).unwrap();
            let other = root.join("other.exe");
            fs::write(&other, b"another inert fixture").unwrap();
            assert!(!refresh_link(&shortcut, &other, &icon).unwrap());
            assert_eq!(before, fs::read(&shortcut).unwrap());
            assert!(refresh_link(&shortcut, &exe, &icon).unwrap());
            assert!(!refresh_link(&shortcut, &exe, &icon).unwrap());
            unsafe {
                let link: IShellLinkW =
                    CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).unwrap();
                let file: IPersistFile = link.cast().unwrap();
                file.Load(PCWSTR(wide(&shortcut).as_ptr()), STGM_READWRITE)
                    .unwrap();
                let mut args = [0u16; 100];
                link.GetArguments(&mut args).unwrap();
                assert_eq!(path_from_wide(&args), Path::new("--minimized"));
                let mut cwd = [0u16; 32768];
                link.GetWorkingDirectory(&mut cwd).unwrap();
                assert_eq!(path_from_wide(&cwd), root);
            }
            fs::remove_dir_all(root).unwrap();
        })
        .join()
        .unwrap();
    }
}
