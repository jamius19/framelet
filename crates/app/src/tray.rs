/*
 * Author: Jamius Siam
 * Since: 31/10/2026
 */

//! Notification area icon (version 4 protocol), its context menu and balloons.

use std::mem::{size_of, transmute};
use std::sync::atomic::{AtomicIsize, Ordering};

use framelet_core::settings::Theme;
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetSystemMetricsForDpi};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_ERROR, NIIF_INFO, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NIM_SETVERSION, NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyIcon, DestroyMenu, HICON, IMAGE_ICON, LR_DEFAULTCOLOR, LoadImageW, MF_GRAYED,
    MF_SEPARATOR, MF_STRING, PostMessageW, SM_CXSMICON, SetForegroundWindow, TPM_BOTTOMALIGN, TPM_LEFTALIGN,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, WM_NULL,
};
use windows::core::{HSTRING, PCSTR, PCWSTR};

use crate::app::WM_TRAY;

const TRAY_ID: u32 = 1;
const ICON_IDLE: usize = 1;
const ICON_RECORDING: usize = 2;

static OWNER: AtomicIsize = AtomicIsize::new(0);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cmd {
    Start = 1,
    Stop,
    Settings,
    Folder,
    Exit,
}

const CMDS: [Cmd; 5] = [Cmd::Start, Cmd::Stop, Cmd::Settings, Cmd::Folder, Cmd::Exit];

fn owner() -> HWND {
    HWND(OWNER.load(Ordering::Relaxed) as *mut _)
}

fn data() -> NOTIFYICONDATAW {
    NOTIFYICONDATAW { cbSize: size_of::<NOTIFYICONDATAW>() as u32, hWnd: owner(), uID: TRAY_ID, ..Default::default() }
}

fn copy_wide<const N: usize>(dst: &mut [u16; N], s: &str) {
    for (d, c) in dst.iter_mut().zip(s.encode_utf16().take(N - 1)) {
        *d = c;
    }
}

/// Loads the icon at the small-icon size of the system DPI. Caller destroys it.
fn load_icon(recording: bool) -> HICON {
    let id = if recording { ICON_RECORDING } else { ICON_IDLE };
    // SAFETY: integer resource id passed as a pointer, per the MAKEINTRESOURCE convention.
    unsafe {
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let size = GetSystemMetricsForDpi(SM_CXSMICON, GetDpiForSystem());
        LoadImageW(Some(hinst.into()), PCWSTR(id as *const u16), IMAGE_ICON, size, size, LR_DEFAULTCOLOR)
            .map(|h| HICON(h.0))
            .unwrap_or_default()
    }
}

/// Adds the icon (also used again after Explorer restarts).
pub fn add(hwnd: HWND, recording: bool, tip: &str) {
    OWNER.store(hwnd.0 as isize, Ordering::Relaxed);
    let icon = load_icon(recording);
    let mut nid = data();
    nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
    nid.uCallbackMessage = WM_TRAY;
    nid.hIcon = icon;
    copy_wide(&mut nid.szTip, tip);
    // SAFETY: nid is fully initialized; the shell copies the icon.
    unsafe {
        if !Shell_NotifyIconW(NIM_ADD, &nid).as_bool() {
            log::warn!("Shell_NotifyIconW(NIM_ADD) failed");
        }
        nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        let _ = Shell_NotifyIconW(NIM_SETVERSION, &nid);
        let _ = DestroyIcon(icon);
    }
}

pub fn set_icon_and_tip(recording: bool, tip: &str) {
    let icon = load_icon(recording);
    let mut nid = data();
    nid.uFlags = NIF_ICON | NIF_TIP | NIF_SHOWTIP;
    nid.hIcon = icon;
    copy_wide(&mut nid.szTip, tip);
    // SAFETY: see `add`.
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
        let _ = DestroyIcon(icon);
    }
}

pub fn set_tip(tip: &str) {
    let mut nid = data();
    nid.uFlags = NIF_TIP | NIF_SHOWTIP;
    copy_wide(&mut nid.szTip, tip);
    // SAFETY: nid is fully initialized.
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

pub fn remove() {
    let nid = data();
    // SAFETY: nid is fully initialized.
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
    }
}

pub fn balloon(title: &str, text: &str, is_error: bool) {
    let mut nid = data();
    nid.uFlags = NIF_INFO;
    nid.dwInfoFlags = if is_error { NIIF_ERROR } else { NIIF_INFO };
    copy_wide(&mut nid.szInfoTitle, title);
    copy_wide(&mut nid.szInfo, text);
    // SAFETY: nid is fully initialized.
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

/// Shows the context menu at `pt` and returns the chosen command.
pub fn show_menu(pt: POINT, can_start: bool, recording: bool, hotkey: &str) -> Option<Cmd> {
    // SAFETY: the menu is created, used and destroyed here; the owner is our window.
    unsafe {
        let menu = CreatePopupMenu().ok()?;
        let start = HSTRING::from(format!("Start Recording\t{hotkey}"));
        let start_flags = if can_start { MF_STRING } else { MF_STRING | MF_GRAYED };
        let _ = AppendMenuW(menu, start_flags, Cmd::Start as usize, &start);
        if recording {
            let _ = AppendMenuW(menu, MF_STRING, Cmd::Stop as usize, &HSTRING::from("Stop Recording"));
        }
        let _ = AppendMenuW(menu, MF_STRING, Cmd::Settings as usize, &HSTRING::from("Settings"));
        let _ = AppendMenuW(menu, MF_STRING, Cmd::Folder as usize, &HSTRING::from("Open Recordings Folder"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, Cmd::Exit as usize, &HSTRING::from("Exit"));

        let _ = SetForegroundWindow(owner());
        let flags = TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN | TPM_LEFTALIGN;
        let id = TrackPopupMenu(menu, flags, pt.x, pt.y, None, owner(), None).0 as usize;
        let _ = PostMessageW(Some(owner()), WM_NULL, Default::default(), Default::default());
        let _ = DestroyMenu(menu);
        CMDS.into_iter().find(|c| *c as usize == id)
    }
}

/// Makes popup menus follow the configured theme (undocumented uxtheme ordinals 135 and 136).
/// Silently does nothing if the ordinals are unavailable.
pub fn apply_menu_theme(theme: Theme) {
    let mode: i32 = match theme {
        Theme::System => 1, // AllowDark
        Theme::Dark => 2,   // ForceDark
        Theme::Light => 3,  // ForceLight
    };
    // SAFETY: ordinals 135 (SetPreferredAppMode) and 136 (FlushMenuThemes) have these signatures on
    // Windows 10 1903+; the pointers are checked for presence first.
    unsafe {
        let Ok(lib) = LoadLibraryW(&HSTRING::from("uxtheme.dll")) else { return };
        let set_mode = GetProcAddress(lib, PCSTR(135 as *const u8));
        let flush = GetProcAddress(lib, PCSTR(136 as *const u8));
        if let (Some(set_mode), Some(flush)) = (set_mode, flush) {
            let set_mode: extern "system" fn(i32) -> i32 = transmute(set_mode);
            let flush: extern "system" fn() = transmute(flush);
            set_mode(mode);
            flush();
        }
    }
}
