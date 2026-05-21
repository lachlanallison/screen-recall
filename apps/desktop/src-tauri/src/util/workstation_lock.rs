//! Windows: listen for session lock/unlock (Win+L) so capture can pause without filling disk.

use std::sync::Arc;
use std::sync::OnceLock;

use tracing::warn;

use crate::state::AppState;

static APP_STATE: OnceLock<Arc<AppState>> = OnceLock::new();

/// Call once at startup. No-op on non-Windows.
pub fn spawn_watcher(state: Arc<AppState>) {
    #[cfg(windows)]
    {
        if APP_STATE.set(state).is_ok() {
            std::thread::Builder::new()
                .name("screenrecall-wts-lock".into())
                .spawn(|| {
                    if let Err(e) = windows_wts_message_loop() {
                        warn!(?e, "workstation lock watcher exited");
                    }
                })
                .ok();
        }
    }
    #[cfg(not(windows))]
    {
        let _ = state;
    }
}

#[cfg(windows)]
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};

#[cfg(windows)]
fn windows_wts_message_loop() -> Result<(), String> {
    use std::sync::atomic::{AtomicBool, Ordering};

    use windows::core::w;
    use windows::Win32::Foundation::HINSTANCE;
    use windows::Win32::System::RemoteDesktop::WTSRegisterSessionNotification;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DispatchMessageW, GetMessageW, RegisterClassW, TranslateMessage,
        CS_VREDRAW, HWND_MESSAGE, MSG, WINDOW_EX_STYLE, WINDOW_STYLE, WNDCLASSW,
    };

    /// NOTIFY_FOR_THIS_SESSION
    const NOTIFY_FOR_THIS_SESSION: u32 = 0;

    static CLASS_ATOM: AtomicBool = AtomicBool::new(false);

    let hinstance = unsafe { windows::Win32::System::LibraryLoader::GetModuleHandleW(None) }
        .map_err(|e| format!("GetModuleHandleW: {e}"))?;

    let class_name = w!("ScreenRecallWtsLockNotify");

    if !CLASS_ATOM.swap(true, Ordering::SeqCst) {
        let wc = WNDCLASSW {
            style: CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: HINSTANCE::from(hinstance),
            hIcon: Default::default(),
            hCursor: Default::default(),
            hbrBackground: Default::default(),
            lpszMenuName: windows::core::PCWSTR::null(),
            lpszClassName: class_name,
        };
        let atom = unsafe { RegisterClassW(&wc) };
        if atom == 0 {
            return Err("RegisterClassW failed".into());
        }
    }

    let h_inst = HINSTANCE::from(hinstance);
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class_name,
            w!("ScreenRecall lock monitor"),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(h_inst),
            None,
        )
    }
    .map_err(|e| format!("CreateWindowExW: {e}"))?;

    unsafe {
        WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION)
            .map_err(|e| format!("WTSRegisterSessionNotification: {e}"))?;
    }

    let mut msg = MSG::default();
    loop {
        let r = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if !r.as_bool() {
            break;
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}

#[cfg(windows)]
unsafe extern "system" fn wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    _lparam: LPARAM,
) -> LRESULT {
    use windows::Win32::System::RemoteDesktop::WTSUnRegisterSessionNotification;
    use windows::Win32::UI::WindowsAndMessaging::{
        DefWindowProcW, PostQuitMessage, WM_DESTROY, WM_WTSSESSION_CHANGE,
    };

    const WTS_SESSION_LOCK: usize = 0x7;
    const WTS_SESSION_UNLOCK: usize = 0x8;

    match msg {
        WM_WTSSESSION_CHANGE => {
            if let Some(state) = APP_STATE.get() {
                match wparam.0 {
                    WTS_SESSION_LOCK => state.set_workstation_locked(true),
                    WTS_SESSION_UNLOCK => state.set_workstation_locked(false),
                    _ => {}
                }
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            let _ = WTSUnRegisterSessionNotification(hwnd);
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, _lparam) },
    }
}
