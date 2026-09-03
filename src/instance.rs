//! One application instance per Windows session; also used by the installer.
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE};
use windows::Win32::System::Threading::CreateMutexW;

pub const MUTEX_NAME: &str = r"Local\LeigodGuard";

pub struct InstanceGuard(HANDLE);

impl InstanceGuard {
    pub fn acquire() -> windows::core::Result<Option<Self>> {
        Self::acquire_named(MUTEX_NAME)
    }

    fn acquire_named(name: &str) -> windows::core::Result<Option<Self>> {
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        // Holding the handle, rather than mutex ownership, keeps the named object
        // alive. The installer checks for its existence before replacing files.
        let handle = unsafe { CreateMutexW(None, false, PCWSTR(wide.as_ptr()))? };
        let already_running = unsafe { GetLastError() == ERROR_ALREADY_EXISTS };
        if already_running {
            unsafe {
                let _ = CloseHandle(handle);
            }
            Ok(None)
        } else {
            Ok(Some(Self(handle)))
        }
    }
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::InstanceGuard;

    #[test]
    fn second_instance_is_rejected_until_the_first_exits() {
        let name = format!(r"Local\LeigodGuard-Test-{}", std::process::id());
        let first = InstanceGuard::acquire_named(&name).unwrap().unwrap();
        assert!(InstanceGuard::acquire_named(&name).unwrap().is_none());
        drop(first);
        assert!(InstanceGuard::acquire_named(&name).unwrap().is_some());
    }
}
