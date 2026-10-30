/*
 * Author: Jamius Siam
 * Since: 31/10/2026
 */

//! Shared Win32 plumbing: window creation, modal loop, DPI/monitor queries, Esc / Enter handling.

use crate::gfx::Rect;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, HMONITOR, MONITORINFO};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey, VK_ESCAPE, VK_RETURN};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::PCWSTR;

pub type WndProc = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT;

/// Registers `class` (if needed) and creates a hidden layered topmost tool popup at `rect`.
pub fn create_layered_popup(
    class: PCWSTR,
    wndproc: WndProc,
    cursor: PCWSTR,
    extra_ex_style: WINDOW_EX_STYLE,
    rect: Rect,
) -> Option<HWND> {
    unsafe {
        let instance = GetModuleHandleW(None).ok()?.into();
        let wc = WNDCLASSW {
            style: CS_DBLCLKS,
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            hCursor: LoadCursorW(None, cursor).unwrap_or_default(),
            lpszClassName: class,
            ..Default::default()
        };
        // Fails harmlessly with ERROR_CLASS_ALREADY_EXISTS on later calls.
        RegisterClassW(&wc);
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | extra_ex_style,
            class,
            PCWSTR::null(),
            WS_POPUP,
            rect.left,
            rect.top,
            rect.width(),
            rect.height(),
            None,
            None,
            Some(instance),
            None,
        )
        .inspect_err(|e| log::error!("CreateWindowExW failed: {e}"))
        .ok()
    }
}

/// Nested modal loop: pumps messages until `done()`. On `WM_QUIT` the quit is re-posted for the
/// outer loop and `false` is returned.
pub fn pump(done: impl Fn() -> bool) -> bool {
    let mut msg = MSG::default();
    while !done() {
        unsafe {
            match GetMessageW(&mut msg, None, 0, 0).0 {
                0 => {
                    PostQuitMessage(msg.wParam.0 as i32);
                    return false;
                }
                -1 => return false,
                _ => {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }
    }
    true
}

/// Destroys the window, then waits for DWM so nothing of it remains on screen when we return.
pub fn destroy_and_flush(hwnd: HWND) {
    unsafe {
        let _ = DestroyWindow(hwnd);
        let _ = DwmFlush();
    }
}

pub fn monitor_dpi(monitor: HMONITOR) -> u32 {
    let (mut x, mut y) = (0, 0);
    match unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut x, &mut y) } {
        Ok(()) if x > 0 => x,
        _ => 96,
    }
}

/// Monitor bounds in physical virtual-desktop pixels.
pub fn monitor_rect(monitor: HMONITOR) -> Rect {
    let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
    let r = if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() { info.rcMonitor } else { Default::default() };
    Rect::new(r.left, r.top, r.right, r.bottom)
}

const ESC_HOTKEY_ID: i32 = 0x4652;
const ENTER_HOTKEY_ID: i32 = 0x4653;

/// Makes Esc (and optionally Enter) reach `hwnd` even if Windows refuses to give it keyboard focus
/// (foreground lock when started from a tray click or hotkey): requests foreground and holds temporary
/// hotkeys, delivered as `WM_HOTKEY`. If a hotkey is taken by someone else, `WM_KEYDOWN` is the fallback.
pub struct EscGuard {
    hwnd: HWND,
    enter: bool,
}

impl EscGuard {
    pub fn new(hwnd: HWND) -> Self {
        Self::register(hwnd, false)
    }

    /// Like [`EscGuard::new`], additionally holding Enter.
    pub fn with_enter(hwnd: HWND) -> Self {
        Self::register(hwnd, true)
    }

    fn register(hwnd: HWND, enter: bool) -> Self {
        unsafe {
            let _ = SetForegroundWindow(hwnd);
            let hotkey = RegisterHotKey(Some(hwnd), ESC_HOTKEY_ID, MOD_NOREPEAT, VK_ESCAPE.0 as u32);
            let enter_hotkey =
                enter.then(|| RegisterHotKey(Some(hwnd), ENTER_HOTKEY_ID, MOD_NOREPEAT, VK_RETURN.0 as u32));
            log::debug!(
                "foreground obtained: {}, Esc hotkey registered: {}, Enter hotkey registered: {:?}",
                GetForegroundWindow() == hwnd,
                hotkey.is_ok(),
                enter_hotkey.map(|r| r.is_ok())
            );
        }
        Self { hwnd, enter }
    }

    /// True for messages meaning "Esc was pressed".
    pub fn is_esc(msg: u32, wparam: WPARAM) -> bool {
        match msg {
            WM_HOTKEY => wparam.0 as i32 == ESC_HOTKEY_ID,
            WM_KEYDOWN => wparam.0 == VK_ESCAPE.0 as usize,
            _ => false,
        }
    }

    /// True for messages meaning "Enter was pressed" (only delivered as a hotkey with [`EscGuard::with_enter`]).
    pub fn is_enter(msg: u32, wparam: WPARAM) -> bool {
        match msg {
            WM_HOTKEY => wparam.0 as i32 == ENTER_HOTKEY_ID,
            WM_KEYDOWN => wparam.0 == VK_RETURN.0 as usize,
            _ => false,
        }
    }
}

impl Drop for EscGuard {
    fn drop(&mut self) {
        let _ = unsafe { UnregisterHotKey(Some(self.hwnd), ESC_HOTKEY_ID) };
        if self.enter {
            let _ = unsafe { UnregisterHotKey(Some(self.hwnd), ENTER_HOTKEY_ID) };
        }
    }
}
