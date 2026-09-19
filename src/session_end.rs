//! A permanent handler on the actual GUI window, shared with the fallback window.
//! No account, file, or network access here: the caller supplies the bounded job.
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{WM_ENDSESSION, WM_NCDESTROY, WM_QUERYENDSESSION};

const SUBCLASS_ID: usize = 0x4c4753;
pub const SHUTDOWN_BUDGET: Duration = Duration::from_secs(4);

#[derive(Default)]
struct Attempt {
    deadline: Option<Instant>,
    finished: bool,
    timeout_reported: bool,
}

pub struct SessionEnd {
    job: Box<dyn Fn(Instant) + Send + Sync>,
    trace: Box<dyn Fn(&str) + Send + Sync>,
    attempt: Mutex<Attempt>,
    complete: Condvar,
    budget: Duration,
}

impl SessionEnd {
    pub fn new(
        job: impl Fn(Instant) + Send + Sync + 'static,
        trace: impl Fn(&str) + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            job: Box::new(job),
            trace: Box::new(trace),
            attempt: Mutex::new(Attempt::default()),
            complete: Condvar::new(),
            budget: SHUTDOWN_BUDGET,
        })
    }

    /// Every receiving window waits for the SAME job and deadline. Returning
    /// from one window while another is still doing I/O risks process teardown.
    pub fn handle(self: &Arc<Self>, message: u32, wparam: WPARAM) -> Option<LRESULT> {
        match message {
            WM_QUERYENDSESSION => {
                (self.trace)("收到关机/注销询问，允许结束会话");
                Some(LRESULT(1))
            }
            WM_ENDSESSION if wparam.0 == 0 => {
                // No pause is sent at QUERY time: canceling shutdown must not
                // interrupt a game. A later confirmed shutdown can still run.
                (self.trace)("关机/注销已取消，未触发关机暂停");
                Some(LRESULT(0))
            }
            WM_ENDSESSION => {
                self.finish();
                Some(LRESULT(0))
            }
            _ => None,
        }
    }

    fn finish(self: &Arc<Self>) {
        let mut attempt = self.attempt.lock().unwrap_or_else(|e| e.into_inner());
        let deadline = if let Some(deadline) = attempt.deadline {
            deadline
        } else {
            let deadline = Instant::now() + self.budget;
            attempt.deadline = Some(deadline);
            let this = Arc::clone(self);
            // Neither a busy configuration lock nor a stuck network/log writer
            // may keep a Windows message handler alive beyond the total budget.
            if std::thread::Builder::new()
                .name("shutdown-pause".into())
                .spawn(move || {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        (this.trace)("收到会话结束确认，开始一次关机暂停处理");
                        (this.job)(deadline);
                    }));
                    if result.is_err() {
                        (this.trace)("关机暂停处理异常，暂停未确认");
                    }
                    let mut state = this.attempt.lock().unwrap_or_else(|e| e.into_inner());
                    state.finished = true;
                    this.complete.notify_all();
                })
                .is_err()
            {
                attempt.finished = true;
                (self.trace)("无法启动关机暂停线程，暂停未确认");
            }
            deadline
        };
        while !attempt.finished {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                if !attempt.timeout_reported {
                    attempt.timeout_reported = true;
                    (self.trace)("关机暂停等待超时，暂停未确认；继续系统关机");
                }
                break;
            }
            attempt = self
                .complete
                .wait_timeout(attempt, remaining)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
}

/// Call on the owning UI thread. This handler stays installed after the user
/// opens a silently started window; it is independent of visibility handling.
pub fn install(hwnd: HWND, handler: Arc<SessionEnd>) -> Result<(), String> {
    let data = Box::into_raw(Box::new(handler));
    if unsafe { SetWindowSubclass(hwnd, Some(session_proc), SUBCLASS_ID, data as usize) }.as_bool()
    {
        Ok(())
    } else {
        unsafe { drop(Box::from_raw(data)) };
        Err("无法安装主窗口关机监听".into())
    }
}

unsafe extern "system" fn session_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    if message == WM_NCDESTROY {
        let _ = RemoveWindowSubclass(hwnd, Some(session_proc), SUBCLASS_ID);
        drop(Box::from_raw(data as *mut Arc<SessionEnd>));
    } else {
        let handler = &*(data as *const Arc<SessionEnd>);
        if let Some(result) = handler.handle(message, wparam) {
            return result;
        }
    }
    DefSubclassProc(hwnd, message, wparam, lparam)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use windows::core::w;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, IsWindowVisible, SendMessageW, WS_EX_TOOLWINDOW, WS_POPUP,
    };

    #[test]
    fn actual_hidden_main_window_handles_cancel_then_confirm_and_deduplicates() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&calls);
        let handler = SessionEnd::new(
            move |_| {
                counted.fetch_add(1, Ordering::SeqCst);
            },
            |_| {},
        );
        unsafe {
            // An off-screen, never shown test window. No production HWND or
            // account is used; SendMessage targets this window, NOT a broadcast.
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                w!("STATIC"),
                w!("shutdown test"),
                WS_POPUP,
                -32000,
                -32000,
                1,
                1,
                None,
                None,
                None,
                None,
            )
            .unwrap();
            install(hwnd, Arc::clone(&handler)).unwrap();
            assert_eq!(
                SendMessageW(hwnd, WM_QUERYENDSESSION, WPARAM(0), LPARAM(0)).0,
                1
            );
            SendMessageW(hwnd, WM_ENDSESSION, WPARAM(0), LPARAM(0));
            assert_eq!(calls.load(Ordering::SeqCst), 0);
            SendMessageW(hwnd, WM_QUERYENDSESSION, WPARAM(0), LPARAM(0));
            SendMessageW(hwnd, WM_ENDSESSION, WPARAM(1), LPARAM(0));
            // Fallback window duplicate must neither send nor wait anew.
            handler.handle(WM_ENDSESSION, WPARAM(1));
            SendMessageW(hwnd, WM_ENDSESSION, WPARAM(1), LPARAM(0));
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            assert!(!IsWindowVisible(hwnd).as_bool());
            DestroyWindow(hwnd).unwrap();
        }
    }

    #[test]
    fn concurrent_receivers_wait_for_one_pause_before_returning() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&calls);
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        let handler = SessionEnd::new(
            move |_| {
                counted.fetch_add(1, Ordering::SeqCst);
                entered_tx.send(()).unwrap();
                release_rx.lock().unwrap().recv().unwrap();
            },
            |_| {},
        );
        let one = Arc::clone(&handler);
        let first = std::thread::spawn(move || {
            one.handle(WM_ENDSESSION, WPARAM(1));
        });
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let two = Arc::clone(&handler);
        let (returned_tx, returned_rx) = std::sync::mpsc::channel();
        let second = std::thread::spawn(move || {
            two.handle(WM_ENDSESSION, WPARAM(1));
            returned_tx.send(()).unwrap();
        });
        assert!(!first.is_finished());
        assert!(matches!(
            returned_rx.recv_timeout(Duration::from_millis(50)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));
        release_tx.send(()).unwrap();
        first.join().unwrap();
        second.join().unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn panicking_job_releases_the_waiters_without_a_second_attempt() {
        let handler = SessionEnd::new(|_| panic!("test failure"), |_| {});
        handler.handle(WM_ENDSESSION, WPARAM(1));
        assert!(handler.attempt.lock().unwrap().finished);
        handler.handle(WM_ENDSESSION, WPARAM(1));
    }

    #[test]
    fn stuck_job_cannot_hold_shutdown_or_reset_budget_on_duplicate() {
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        let mut handler = SessionEnd::new(
            move |_| {
                let _ = release_rx.lock().unwrap().recv();
            },
            |_| {},
        );
        Arc::get_mut(&mut handler).unwrap().budget = Duration::from_millis(50);
        let start = Instant::now();
        handler.handle(WM_ENDSESSION, WPARAM(1));
        assert!(start.elapsed() < Duration::from_secs(1));
        assert!(!handler.attempt.lock().unwrap().finished);
        let deadline = handler.attempt.lock().unwrap().deadline;
        handler.handle(WM_ENDSESSION, WPARAM(1));
        assert_eq!(handler.attempt.lock().unwrap().deadline, deadline);
        release_tx.send(()).unwrap();
    }
}
