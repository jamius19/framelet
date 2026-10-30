/*
 * Author: Jamius Siam
 * Since: 31/10/2026
 */

//! Global record hotkey registration.

use framelet_core::hotkey::Hotkey;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::{HOT_KEY_MODIFIERS, MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey};

const HOTKEY_ID: i32 = 1;

/// Registers `spec` (e.g. "Alt+Shift+R"). On failure `false` is returned and, if `notify`, the user is told.
pub fn register(hwnd: HWND, spec: &str, notify: bool) -> bool {
    let Some(hk) = Hotkey::parse(spec) else {
        if notify {
            crate::notify::error(&format!("Shortcut \"{spec}\" is not valid"));
        }
        return false;
    };
    // SAFETY: plain Win32 call on our own window.
    let modifiers = HOT_KEY_MODIFIERS(hk.modifiers) | MOD_NOREPEAT;
    let result = unsafe { RegisterHotKey(Some(hwnd), HOTKEY_ID, modifiers, hk.vk) };
    match result {
        Ok(()) => true,
        Err(e) => {
            log::warn!("RegisterHotKey({spec}) failed: {e}");
            if notify {
                let shortcut = hk.to_display_string();
                let text = format!("Shortcut {shortcut} is in use by another app. Pick another one in Settings.");
                crate::notify::error(&text);
            }
            false
        }
    }
}

pub fn unregister(hwnd: HWND) {
    // SAFETY: plain Win32 call on our own window.
    unsafe {
        let _ = UnregisterHotKey(Some(hwnd), HOTKEY_ID);
    }
}
