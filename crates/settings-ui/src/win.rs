/*
 * Author: Jamius Siam
 * Since: 31/10/2026
 */

//! Win32 helpers: single instance, tray messages, registry, DWM styling, file dialogs,
//! window subclass (minimize-to-tray and hotkey capture) and hotkey availability test.

use std::cell::RefCell;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use framelet_core::hotkey::{Hotkey, MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN};
use framelet_core::ipc;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::Foundation::{
    COLORREF, ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, WPARAM,
};
use windows::Win32::Graphics::Dwm::{
    DWM_WINDOW_CORNER_PREFERENCE, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE,
    DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmExtendFrameIntoClientArea,
    DwmSetWindowAttribute,
};
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
};
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, REG_SZ, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW,
    RegSetKeyValueW,
};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, HOT_KEY_MODIFIERS, MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey, VK_CONTROL, VK_ESCAPE, VK_LWIN,
    VK_MENU, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    DefSubclassProc, FileOpenDialog, FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, IFileOpenDialog, IShellItem,
    SHCreateItemFromParsingName, SIGDN_FILESYSPATH, SetWindowSubclass,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconFromResourceEx, FindWindowW, GetSystemMetrics, ICON_BIG, ICON_SMALL, IsIconic, LR_DEFAULTCOLOR,
    PostMessageW, RegisterWindowMessageW, SC_MINIMIZE, SM_CXICON, SM_CXSMICON, SW_RESTORE, SendMessageW,
    SetForegroundWindow, ShowWindow, WM_CHAR, WM_KEYDOWN, WM_KEYUP, WM_SETICON, WM_SYSCHAR, WM_SYSCOMMAND,
    WM_SYSKEYDOWN, WM_SYSKEYUP,
};
use windows::core::{HSTRING, PCWSTR, w};

const RUN_KEY: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
const PERSONALIZE_KEY: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
const RUN_VALUE: PCWSTR = w!("Framelet");
const SUBCLASS_ID: usize = 0x46_52_4D;
const TEST_HOTKEY_ID: i32 = 0x4652;

static MINIMIZE_TO_TRAY: AtomicBool = AtomicBool::new(false);
static CAPTURING: AtomicBool = AtomicBool::new(false);

type KeyHandler = Rc<dyn Fn(u32, u32)>;

thread_local! {
    static KEY_HANDLER: RefCell<Option<KeyHandler>> = const { RefCell::new(None) };
}

fn wide(s: &str) -> HSTRING {
    HSTRING::from(s)
}

/// Returns `true` if this is the only settings process. Otherwise the existing window is
/// restored and focused and `false` is returned. The mutex lives until the process exits.
pub fn acquire_single_instance() -> bool {
    // SAFETY: plain Win32 calls with valid arguments; the mutex handle is intentionally never closed.
    unsafe {
        let created = CreateMutexW(None, false, &wide(ipc::SETTINGS_MUTEX));
        if created.is_ok() && GetLastError() != ERROR_ALREADY_EXISTS {
            return true;
        }
        if let Ok(hwnd) = FindWindowW(PCWSTR::null(), &wide(ipc::SETTINGS_WINDOW_TITLE)) {
            if IsIconic(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_RESTORE);
            }
            let _ = SetForegroundWindow(hwnd);
        }
        false
    }
}

/// Posts one of the `ipc::MSG_*` messages to the tray process, if it is running.
pub fn post_to_tray(message: &str) {
    // SAFETY: plain Win32 calls with valid arguments.
    unsafe {
        let Ok(tray) = FindWindowW(&wide(ipc::TRAY_WINDOW_CLASS), PCWSTR::null()) else {
            return;
        };
        let msg = RegisterWindowMessageW(&wide(message));
        if msg != 0 {
            let _ = PostMessageW(Some(tray), msg, WPARAM(0), LPARAM(0));
        }
    }
}

pub fn hwnd_of(window: &slint::Window) -> Option<HWND> {
    match window.window_handle().window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(HWND(h.hwnd.get() as *mut c_void)),
        _ => None,
    }
}

fn read_dword(subkey: PCWSTR, value: PCWSTR) -> Option<u32> {
    let mut data = 0u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: `data` and `size` outlive the call and match the DWORD request.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            subkey,
            value,
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut u32 as *mut c_void),
            Some(&mut size),
        )
    };
    status.is_ok().then_some(data)
}

/// Whether Windows apps use the dark theme (`AppsUseLightTheme == 0`). Defaults to dark.
pub fn system_prefers_dark() -> bool {
    read_dword(PERSONALIZE_KEY, w!("AppsUseLightTheme")).is_none_or(|v| v == 0)
}

pub fn startup_enabled() -> bool {
    let mut size = 0u32;
    // SAFETY: only queries the size of the value.
    unsafe { RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE, RRF_RT_REG_SZ, None, None, Some(&mut size)).is_ok() }
}

/// Adds or removes the `Run` registry value that starts `framelet.exe` next to this executable.
pub fn set_startup(enabled: bool) -> Result<(), String> {
    // SAFETY: the data pointer and byte length describe a live UTF-16 buffer.
    let status = unsafe {
        if enabled {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let tray = exe.with_file_name(ipc::TRAY_EXE);
            let command: Vec<u16> =
                format!("\"{}\"", tray.display()).encode_utf16().chain(std::iter::once(0)).collect();
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                RUN_VALUE,
                REG_SZ.0,
                Some(command.as_ptr() as *const c_void),
                (command.len() * 2) as u32,
            )
        } else {
            RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE)
        }
    };
    // Deleting a value that does not exist is fine.
    if status.is_ok() || (!enabled && !startup_enabled()) { Ok(()) } else { Err(format!("{status:?}")) }
}

fn colorref(rgb: u32) -> COLORREF {
    COLORREF(((rgb & 0xFF) << 16) | (rgb & 0xFF00) | ((rgb >> 16) & 0xFF))
}

/// Matches the title bar to the app theme: immersive dark mode, round corners and, without Mica,
/// a caption colour equal to the window background.
pub fn style_window(hwnd: HWND, dark: bool, caption_rgb: Option<u32>, text_rgb: u32) {
    let dark_flag: i32 = dark.into();
    let caption = colorref(caption_rgb.unwrap_or(0));
    let text = colorref(text_rgb);
    let corners = DWMWCP_ROUND;
    // SAFETY: each pointer refers to a live local of the size passed alongside it.
    unsafe {
        let set = |attr, value: *const c_void, size: usize| {
            let _ = DwmSetWindowAttribute(hwnd, attr, value, size as u32);
        };
        set(DWMWA_USE_IMMERSIVE_DARK_MODE, &dark_flag as *const _ as _, size_of::<i32>());
        if caption_rgb.is_some() {
            set(DWMWA_CAPTION_COLOR, &caption as *const _ as _, size_of::<COLORREF>());
            set(DWMWA_TEXT_COLOR, &text as *const _ as _, size_of::<COLORREF>());
        }
        set(
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &corners as *const DWM_WINDOW_CORNER_PREFERENCE as _,
            size_of::<DWM_WINDOW_CORNER_PREFERENCE>(),
        );
    }
}

/// Mica backdrops need Windows 11 22H2 (build 22621) or newer.
pub fn mica_supported() -> bool {
    let mut buf = [0u16; 16];
    let mut size = size_of_val(&buf) as u32;
    // SAFETY: `buf` and `size` describe the same live buffer.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion"),
            w!("CurrentBuildNumber"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut c_void),
            Some(&mut size),
        )
    };
    let text = String::from_utf16_lossy(&buf);
    status.is_ok() && text.trim_end_matches('\0').parse::<u32>().is_ok_and(|build| build >= 22621)
}

/// Enables the Mica backdrop (Windows 11 22621+) behind the whole window.
pub fn enable_mica(hwnd: HWND) {
    let margins = MARGINS { cxLeftWidth: -1, cxRightWidth: -1, cyTopHeight: -1, cyBottomHeight: -1 };
    let mainwindow: i32 = 2; // DWMSBT_MAINWINDOW
    // SAFETY: both pointers refer to live locals of the stated size.
    unsafe {
        let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            &mainwindow as *const i32 as *const c_void,
            size_of::<i32>() as u32,
        );
    }
}

fn run_dialog(owner: HWND, configure: impl FnOnce(&IFileOpenDialog) -> windows::core::Result<()>) -> Option<PathBuf> {
    // SAFETY: standard COM usage on the UI thread; the returned string is freed with CoTaskMemFree.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        configure(&dialog).ok()?;
        dialog.Show(Some(owner)).ok()?;
        let item = dialog.GetResult().ok()?;
        let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = name.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(name.0 as *const c_void));
        path
    }
}

fn set_initial_folder(dialog: &IFileOpenDialog, folder: &Path) {
    // SAFETY: valid COM objects; failure just leaves the default folder.
    unsafe {
        if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(&wide(&folder.to_string_lossy()), None) {
            let _ = dialog.SetFolder(&item);
        }
    }
}

pub fn pick_folder(owner: HWND, initial: &Path) -> Option<PathBuf> {
    run_dialog(owner, |dialog| {
        // SAFETY: valid COM object.
        unsafe {
            dialog.SetOptions(dialog.GetOptions()? | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM)?;
            dialog.SetTitle(w!("Choose where recordings are saved"))?;
        }
        set_initial_folder(dialog, initial);
        Ok(())
    })
}

pub fn pick_ffmpeg(owner: HWND, current: Option<&Path>) -> Option<PathBuf> {
    run_dialog(owner, |dialog| {
        let filters = [
            COMDLG_FILTERSPEC { pszName: w!("FFmpeg (ffmpeg.exe)"), pszSpec: w!("ffmpeg.exe") },
            COMDLG_FILTERSPEC { pszName: w!("Programs (*.exe)"), pszSpec: w!("*.exe") },
        ];
        // SAFETY: valid COM object; the filter strings are static.
        unsafe {
            dialog.SetOptions(dialog.GetOptions()? | FOS_FORCEFILESYSTEM)?;
            dialog.SetTitle(w!("Select ffmpeg.exe"))?;
            dialog.SetFileTypes(&filters)?;
        }
        if let Some(dir) = current.and_then(Path::parent).filter(|d| d.is_dir()) {
            set_initial_folder(dialog, dir);
        }
        Ok(())
    })
}

/// Checks whether `RegisterHotKey` would succeed for `hotkey` (and releases it again).
pub fn hotkey_available(hotkey: Hotkey) -> bool {
    // SAFETY: registers and immediately releases a thread-level hotkey (no window).
    unsafe {
        let mods = HOT_KEY_MODIFIERS(hotkey.modifiers) | MOD_NOREPEAT;
        let ok = RegisterHotKey(None, TEST_HOTKEY_ID, mods, hotkey.vk).is_ok();
        if ok {
            let _ = UnregisterHotKey(None, TEST_HOTKEY_ID);
        }
        ok
    }
}

pub fn set_minimize_to_tray(enabled: bool) {
    MINIMIZE_TO_TRAY.store(enabled, Ordering::Relaxed);
}

/// While capturing, key presses are swallowed and passed to `on_key(vk, modifiers)`.
pub fn set_capture(active: bool) {
    CAPTURING.store(active, Ordering::Relaxed);
}

pub fn is_capturing() -> bool {
    CAPTURING.load(Ordering::Relaxed)
}

pub fn set_key_handler(handler: impl Fn(u32, u32) + 'static) {
    KEY_HANDLER.with(|h| *h.borrow_mut() = Some(Rc::new(handler)));
}

fn is_down(vk: u16) -> bool {
    // SAFETY: GetKeyState has no preconditions.
    unsafe { GetKeyState(vk as i32) < 0 }
}

fn is_modifier_key(vk: u32) -> bool {
    // Shift, Ctrl, Alt, left/right Win and the left/right variants.
    matches!(vk, 0x10 | 0x11 | 0x12 | 0x5B | 0x5C | 0xA0..=0xA5)
}

unsafe extern "system" fn subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    if CAPTURING.load(Ordering::Relaxed) {
        match msg {
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                let vk = wparam.0 as u32;
                if !is_modifier_key(vk) {
                    let mut mods = 0;
                    for (down, flag) in [
                        (is_down(VK_CONTROL.0), MOD_CONTROL),
                        (is_down(VK_MENU.0), MOD_ALT),
                        (is_down(VK_SHIFT.0), MOD_SHIFT),
                        (is_down(VK_LWIN.0) || is_down(VK_RWIN.0), MOD_WIN),
                    ] {
                        if down {
                            mods |= flag;
                        }
                    }
                    let handler = KEY_HANDLER.with(|h| h.borrow().clone());
                    if let Some(handler) = handler {
                        handler(vk, mods);
                    }
                }
                return LRESULT(0);
            }
            WM_KEYUP | WM_SYSKEYUP | WM_CHAR | WM_SYSCHAR => return LRESULT(0),
            _ => {}
        }
    }
    if msg == WM_SYSCOMMAND && (wparam.0 & 0xFFF0) as u32 == SC_MINIMIZE && MINIMIZE_TO_TRAY.load(Ordering::Relaxed) {
        let _ = slint::quit_event_loop();
        return LRESULT(0);
    }
    // SAFETY: forwards the unmodified message to the next handler in the subclass chain.
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

/// Picks the smallest PNG image of the embedded .ico that is at least `want` pixels wide.
fn icon_image(want: i32) -> Option<(&'static [u8], i32)> {
    const ICO: &[u8] = include_bytes!("../../../assets/framelet.ico");
    let count = u16::from_le_bytes(ICO.get(4..6)?.try_into().ok()?) as usize;
    let mut best: Option<(i32, usize, usize)> = None;
    for i in 0..count {
        let e = ICO.get(6 + i * 16..22 + i * 16)?;
        let size = if e[0] == 0 { 256 } else { e[0] as i32 };
        let len = u32::from_le_bytes(e[8..12].try_into().ok()?) as usize;
        let offset = u32::from_le_bytes(e[12..16].try_into().ok()?) as usize;
        let better = best.is_none_or(|(b, ..)| (size >= want && (b < want || size < b)) || (b < want && size > b));
        if better {
            best = Some((size, offset, len));
        }
    }
    let (size, offset, len) = best?;
    Some((ICO.get(offset..offset + len)?, size))
}

/// Sets the title bar / taskbar icon from the embedded `assets/framelet.ico`.
pub fn set_window_icon(hwnd: HWND) {
    for (kind, metric) in [(ICON_SMALL, SM_CXSMICON), (ICON_BIG, SM_CXICON)] {
        // SAFETY: the metric query has no preconditions; the icon bytes are a valid PNG image.
        let want = unsafe { GetSystemMetrics(metric) };
        let Some((data, size)) = icon_image(want) else { continue };
        // SAFETY: `data` is a complete icon image and the handle is owned by the window afterwards.
        unsafe {
            if let Ok(icon) = CreateIconFromResourceEx(data, true, 0x0003_0000, size, size, LR_DEFAULTCOLOR) {
                SendMessageW(hwnd, WM_SETICON, Some(WPARAM(kind as usize)), Some(LPARAM(icon.0 as isize)));
            }
        }
    }
}

pub fn install_subclass(hwnd: HWND) {
    // SAFETY: `subclass_proc` matches SUBCLASSPROC and stays valid for the process lifetime.
    unsafe {
        let _ = SetWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID, 0);
    }
}

pub const VK_ESC: u32 = VK_ESCAPE.0 as u32;
