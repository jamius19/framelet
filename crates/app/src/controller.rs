/*
 * Author: Jamius Siam
 * Since: 31/10/2026
 */

//! Floating recording controller: a small draggable pill with a recording dot, elapsed time and
//! Stop / Cancel buttons. It never takes focus and is excluded from screen capture.

use std::cell::Cell;
use std::ffi::c_void;
use std::mem::size_of;
use std::ptr::null_mut;
use std::slice;

use framelet_core::Region;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BeginPaint, BitBlt, CLEARTYPE_QUALITY, CreateCompatibleDC, CreateDIBSection,
    CreateFontW, DIB_RGB_COLORS, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DeleteDC, DeleteObject, DrawTextW,
    EndPaint, FF_DONTCARE, FW_SEMIBOLD, GetMonitorInfoW, HGDIOBJ, InvalidateRect, MONITOR_DEFAULTTONEAREST,
    MONITORINFO, MonitorFromRect, OUT_DEFAULT_PRECIS, PAINTSTRUCT, SRCCOPY, ScreenToClient, SelectObject, SetBkMode,
    SetTextColor, TRANSPARENT, UpdateWindow,
};
use windows::Win32::UI::Controls::{
    ICC_WIN95_CLASSES, INITCOMMONCONTROLSEX, InitCommonControlsEx, TTF_SUBCLASS, TTM_ADDTOOLW, TTM_NEWTOOLRECT,
    TTS_ALWAYSTIP, TTS_NOPREFIX, TTTOOLINFOW, WM_MOUSELEAVE,
};
use windows::Win32::UI::HiDpi::{GetDpiForWindow, MDT_EFFECTIVE_DPI, GetDpiForMonitor};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_USERDATA, GetClientRect, GetWindowLongPtrW,
    HTCAPTION, HTCLIENT, IDC_ARROW, LoadCursorW, MA_NOACTIVATE, PostMessageW, RegisterClassExW, SW_SHOWNOACTIVATE,
    SWP_NOACTIVATE, SWP_NOZORDER, SendMessageW, SetWindowDisplayAffinity, SetWindowLongPtrW, SetWindowPos, ShowWindow,
    WDA_EXCLUDEFROMCAPTURE, WM_DPICHANGED, WM_ERASEBKGND, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE,
    WM_MOUSEMOVE, WM_NCCREATE, WM_NCDESTROY, WM_NCHITTEST, WM_NCLBUTTONDBLCLK, WM_PAINT, WNDCLASSEXW,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{PCWSTR, PWSTR, w};

use crate::app::{WM_CTRL_CANCEL, WM_CTRL_STOP};

const CLASS: PCWSTR = w!("Framelet.Controller");

// Layout in DIP (96 dpi).
const WIDTH: i32 = 206;
const HEIGHT: i32 = 40;
const GAP: i32 = 12;
const BTN_W: i32 = 32;
const BTN_H: i32 = 28;
const BTN_Y: i32 = 6;
const STOP_X: i32 = 124;
const CANCEL_X: i32 = 166;

const BG: u32 = 0x20_20_20;
const BORDER: u32 = 0x3A_3A_3A;
const RED: u32 = 0xFF_45_3A;
const WHITE: u32 = 0xFF_FF_FF;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Btn {
    Stop,
    Cancel,
}

struct State {
    owner: HWND,
    show_elapsed: bool,
    capturable: bool,
    dpi: Cell<u32>,
    elapsed: Cell<u64>,
    hover: Cell<Option<Btn>>,
    pressed: Cell<Option<Btn>>,
    tracking: Cell<bool>,
    tooltip: Cell<HWND>,
}

pub struct Controller {
    hwnd: HWND,
}

impl Drop for Controller {
    fn drop(&mut self) {
        // SAFETY: the window belongs to this thread.
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

/// `0:12`, or `1:02:03` past an hour.
pub fn format_elapsed(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

impl Controller {
    /// Creates and shows the controller next to (never over, if possible) `region`.
    /// `capturable` is only for the debug demo, to be able to screenshot it.
    pub fn create(owner: HWND, region: Region, show_elapsed: bool, capturable: bool) -> Option<Controller> {
        let state = Box::new(State {
            owner,
            show_elapsed,
            capturable,
            dpi: Cell::new(96),
            elapsed: Cell::new(0),
            hover: Cell::new(None),
            pressed: Cell::new(None),
            tracking: Cell::new(false),
            tooltip: Cell::new(HWND::default()),
        });
        let state = Box::into_raw(state);
        // SAFETY: `state` is owned by the window from WM_NCCREATE on and freed in WM_NCDESTROY.
        unsafe {
            match create_window(region, state) {
                Ok(hwnd) => Some(Controller { hwnd }),
                Err(e) => {
                    log::error!("cannot create controller window: {e}");
                    drop(Box::from_raw(state));
                    None
                }
            }
        }
    }

    pub fn set_elapsed(&self, secs: u64) {
        // SAFETY: the pointer was stored by WM_NCCREATE and stays valid while the window exists.
        unsafe {
            if let Some(st) = state_of(self.hwnd) {
                st.elapsed.set(secs);
                let _ = InvalidateRect(Some(self.hwnd), None, false);
                let _ = UpdateWindow(self.hwnd);
            }
        }
    }
}

unsafe fn create_window(region: Region, state: *mut State) -> windows::core::Result<HWND> {
    // SAFETY: plain Win32 calls; `state` outlives the window.
    unsafe {
        let hinst = windows::Win32::System::LibraryLoader::GetModuleHandleW(None)?;
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassExW(&wc); // fails harmlessly when already registered

        let rect = RECT { left: region.x, top: region.y, right: region.right(), bottom: region.bottom() };
        let monitor = MonitorFromRect(&rect, MONITOR_DEFAULTTONEAREST);
        let (mut dpi_x, mut dpi_y) = (96, 96);
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
        let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(monitor, &mut mi);
        let (w, h, gap) = (scale(WIDTH, dpi_x), scale(HEIGHT, dpi_x), scale(GAP, dpi_x));
        let (x, y) = place(region, w, h, gap, mi.rcWork);

        let ex = WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
        let hwnd = CreateWindowExW(
            ex,
            CLASS,
            w!("Framelet Controller"),
            WS_POPUP,
            x,
            y,
            w,
            h,
            None,
            None,
            Some(hinst.into()),
            Some(state as *const c_void),
        )?;
        let st = &*state;
        st.dpi.set(GetDpiForWindow(hwnd));
        if !st.capturable {
            let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
        }
        let round = DWMWCP_ROUND.0;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &round as *const _ as *const c_void,
            size_of::<i32>() as u32,
        );
        let border = COLORREF(rgb_to_colorref(BORDER));
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            &border as *const _ as *const c_void,
            size_of::<u32>() as u32,
        );
        create_tooltips(hwnd, st);
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        Ok(hwnd)
    }
}

fn scale(dip: i32, dpi: u32) -> i32 {
    (dip as f32 * dpi as f32 / 96.0).round() as i32
}

fn rgb_to_colorref(rgb: u32) -> u32 {
    ((rgb & 0xFF) << 16) | (rgb & 0xFF00) | (rgb >> 16 & 0xFF)
}

/// Below the region centered, else above, else beside it, else inside at the bottom right.
/// The controller is excluded from capture, so the last resort is harmless.
fn place(region: Region, w: i32, h: i32, gap: i32, work: RECT) -> (i32, i32) {
    let clamp = |v: i32, lo: i32, hi: i32| v.min(hi).max(lo);
    let cx = clamp(region.x + (region.width as i32 - w) / 2, work.left, work.right - w);
    let cy = clamp(region.y + (region.height as i32 - h) / 2, work.top, work.bottom - h);
    if region.bottom() + gap + h <= work.bottom {
        (cx, region.bottom() + gap)
    } else if region.y - gap - h >= work.top {
        (cx, region.y - gap - h)
    } else if region.right() + gap + w <= work.right {
        (region.right() + gap, cy)
    } else if region.x - gap - w >= work.left {
        (region.x - gap - w, cy)
    } else {
        let x = clamp(region.right() - gap - w, work.left, work.right - w);
        let y = clamp(region.bottom() - gap - h, work.top, work.bottom - h);
        (x, y)
    }
}

fn button_rect(btn: Btn, dpi: u32) -> RECT {
    let x = if btn == Btn::Stop { STOP_X } else { CANCEL_X };
    RECT {
        left: scale(x, dpi),
        top: scale(BTN_Y, dpi),
        right: scale(x + BTN_W, dpi),
        bottom: scale(BTN_Y + BTN_H, dpi),
    }
}

fn button_at(x: i32, y: i32, dpi: u32) -> Option<Btn> {
    [Btn::Stop, Btn::Cancel].into_iter().find(|b| {
        let r = button_rect(*b, dpi);
        x >= r.left && x < r.right && y >= r.top && y < r.bottom
    })
}

unsafe fn create_tooltips(hwnd: HWND, st: &State) {
    // SAFETY: standard tooltip control setup; text pointers are static.
    unsafe {
        let icc = INITCOMMONCONTROLSEX { dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32, dwICC: ICC_WIN95_CLASSES };
        let _ = InitCommonControlsEx(&icc);
        let Ok(tip) = CreateWindowExW(
            WS_EX_TOPMOST,
            w!("tooltips_class32"),
            PCWSTR::null(),
            WS_POPUP | windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(TTS_ALWAYSTIP | TTS_NOPREFIX),
            0,
            0,
            0,
            0,
            Some(hwnd),
            None,
            None,
            None,
        ) else {
            return;
        };
        let _ = SetWindowDisplayAffinity(tip, WDA_EXCLUDEFROMCAPTURE);
        st.tooltip.set(tip);
        for (id, btn, text) in [(1, Btn::Stop, w!("Stop and save")), (2, Btn::Cancel, w!("Discard recording"))] {
            let ti = TTTOOLINFOW {
                cbSize: size_of::<TTTOOLINFOW>() as u32,
                uFlags: TTF_SUBCLASS,
                hwnd,
                uId: id,
                rect: button_rect(btn, st.dpi.get()),
                lpszText: PWSTR(text.as_ptr() as *mut u16),
                ..Default::default()
            };
            SendMessageW(tip, TTM_ADDTOOLW, None, Some(LPARAM(&ti as *const _ as isize)));
        }
    }
}

unsafe fn update_tooltip_rects(hwnd: HWND, st: &State) {
    // SAFETY: same as `create_tooltips`.
    unsafe {
        for (id, btn) in [(1, Btn::Stop), (2, Btn::Cancel)] {
            let ti = TTTOOLINFOW {
                cbSize: size_of::<TTTOOLINFOW>() as u32,
                hwnd,
                uId: id,
                rect: button_rect(btn, st.dpi.get()),
                ..Default::default()
            };
            SendMessageW(st.tooltip.get(), TTM_NEWTOOLRECT, None, Some(LPARAM(&ti as *const _ as isize)));
        }
    }
}

unsafe fn state_of<'a>(hwnd: HWND) -> Option<&'a State> {
    // SAFETY: GWLP_USERDATA holds the `State` pointer between WM_NCCREATE and WM_NCDESTROY.
    unsafe { (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const State).as_ref() }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: called by the system with valid arguments on the owning thread.
    unsafe {
        if msg == WM_NCCREATE {
            let cs = lparam.0 as *const CREATESTRUCTW;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, (*cs).lpCreateParams as isize);
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        let Some(st) = state_of(hwnd) else { return DefWindowProcW(hwnd, msg, wparam, lparam) };
        let (mx, my) = (lparam.0 as i16 as i32, (lparam.0 >> 16) as i16 as i32);
        match msg {
            WM_ERASEBKGND => LRESULT(1),
            WM_PAINT => {
                paint(hwnd, st);
                LRESULT(0)
            }
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_NCHITTEST => {
                let mut pt = POINT { x: mx, y: my };
                let _ = ScreenToClient(hwnd, &mut pt);
                LRESULT(if button_at(pt.x, pt.y, st.dpi.get()).is_some() { HTCLIENT } else { HTCAPTION } as isize)
            }
            WM_NCLBUTTONDBLCLK => LRESULT(0),
            WM_MOUSEMOVE => {
                if !st.tracking.get() {
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    let _ = TrackMouseEvent(&mut tme);
                    st.tracking.set(true);
                }
                set_hover(hwnd, st, button_at(mx, my, st.dpi.get()));
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                st.tracking.set(false);
                set_hover(hwnd, st, None);
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                st.pressed.set(button_at(mx, my, st.dpi.get()));
                if st.pressed.get().is_some() {
                    SetCapture(hwnd);
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let pressed = st.pressed.take();
                let _ = ReleaseCapture();
                let _ = InvalidateRect(Some(hwnd), None, false);
                if pressed.is_some() && pressed == button_at(mx, my, st.dpi.get()) {
                    let m = if pressed == Some(Btn::Stop) { WM_CTRL_STOP } else { WM_CTRL_CANCEL };
                    let _ = PostMessageW(Some(st.owner), m, WPARAM(0), LPARAM(0));
                }
                LRESULT(0)
            }
            WM_DPICHANGED => {
                st.dpi.set((wparam.0 & 0xFFFF) as u32);
                let r = &*(lparam.0 as *const RECT);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                update_tooltip_rects(hwnd, st);
                let _ = InvalidateRect(Some(hwnd), None, false);
                LRESULT(0)
            }
            WM_NCDESTROY => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                drop(Box::from_raw(st as *const State as *mut State));
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

fn set_hover(hwnd: HWND, st: &State, hover: Option<Btn>) {
    if st.hover.get() != hover {
        st.hover.set(hover);
        // SAFETY: valid window handle.
        unsafe {
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }
}

/// Software canvas over a top-down 32-bit 0xAARRGGBB buffer; shapes are anti-aliased with
/// signed distance fields, text is drawn afterwards with GDI.
struct Canvas<'a> {
    px: &'a mut [u32],
    w: i32,
    h: i32,
}

impl Canvas<'_> {
    fn blend(&mut self, x: i32, y: i32, rgb: u32, alpha: f32) {
        if alpha <= 0.0 || x < 0 || y < 0 || x >= self.w || y >= self.h {
            return;
        }
        let dst = &mut self.px[(y * self.w + x) as usize];
        let mix = |s: u32, d: u32| (d as f32 + (s as f32 - d as f32) * alpha).round() as u32;
        let r = mix(rgb >> 16 & 0xFF, *dst >> 16 & 0xFF);
        let g = mix(rgb >> 8 & 0xFF, *dst >> 8 & 0xFF);
        let b = mix(rgb & 0xFF, *dst & 0xFF);
        *dst = 0xFF00_0000 | r << 16 | g << 8 | b;
    }

    /// Fills the bounding box of a shape, with coverage from `dist` (signed distance, px).
    fn shape(&mut self, bounds: (f32, f32, f32, f32), rgb: u32, alpha: f32, dist: impl Fn(f32, f32) -> f32) {
        let (x0, y0, x1, y1) = bounds;
        for y in (y0.floor() as i32 - 1)..(y1.ceil() as i32 + 1) {
            for x in (x0.floor() as i32 - 1)..(x1.ceil() as i32 + 1) {
                let cov = (0.5 - dist(x as f32 + 0.5, y as f32 + 0.5)).clamp(0.0, 1.0);
                self.blend(x, y, rgb, cov * alpha);
            }
        }
    }

    fn round_rect(&mut self, r: RECT, radius: f32, rgb: u32, alpha: f32) {
        let (cx, cy) = ((r.left + r.right) as f32 / 2.0, (r.top + r.bottom) as f32 / 2.0);
        let (hx, hy) = ((r.right - r.left) as f32 / 2.0 - radius, (r.bottom - r.top) as f32 / 2.0 - radius);
        let bounds = (r.left as f32, r.top as f32, r.right as f32, r.bottom as f32);
        self.shape(bounds, rgb, alpha, |x, y| {
            let (qx, qy) = ((x - cx).abs() - hx, (y - cy).abs() - hy);
            qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - radius
        });
    }

    fn circle(&mut self, cx: f32, cy: f32, r: f32, rgb: u32) {
        self.shape((cx - r, cy - r, cx + r, cy + r), rgb, 1.0, |x, y| (x - cx).hypot(y - cy) - r);
    }

    fn line(&mut self, a: (f32, f32), b: (f32, f32), width: f32, rgb: u32) {
        let bounds = (a.0.min(b.0) - width, a.1.min(b.1) - width, a.0.max(b.0) + width, a.1.max(b.1) + width);
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        self.shape(bounds, rgb, 1.0, |x, y| {
            let t = (((x - a.0) * dx + (y - a.1) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
            (x - a.0 - t * dx).hypot(y - a.1 - t * dy) - width / 2.0
        });
    }
}

fn paint(hwnd: HWND, st: &State) {
    // SAFETY: standard double-buffered WM_PAINT; every GDI object created here is released here.
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);
        let mut rc = RECT::default();
        let _ = GetClientRect(hwnd, &mut rc);
        let (w, h) = (rc.right, rc.bottom);
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = null_mut();
        if let Ok(dib) = CreateDIBSection(Some(hdc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0)
            && !bits.is_null()
        {
            let mem = CreateCompatibleDC(Some(hdc));
            let old = SelectObject(mem, dib.into());
            let px = slice::from_raw_parts_mut(bits as *mut u32, (w * h) as usize);
            px.fill(0xFF00_0000 | BG);
            draw_shapes(&mut Canvas { px, w, h }, st);
            draw_text(mem, st);
            let _ = BitBlt(hdc, 0, 0, w, h, Some(mem), 0, 0, SRCCOPY);
            SelectObject(mem, old);
            let _ = DeleteDC(mem);
            let _ = DeleteObject(HGDIOBJ(dib.0));
        }
        let _ = EndPaint(hwnd, &ps);
    }
}

fn draw_shapes(c: &mut Canvas, st: &State) {
    let dpi = st.dpi.get();
    let s = dpi as f32 / 96.0;
    c.circle(18.0 * s, HEIGHT as f32 / 2.0 * s, 5.0 * s, RED);
    for btn in [Btn::Stop, Btn::Cancel] {
        let r = button_rect(btn, dpi);
        if st.hover.get() == Some(btn) {
            let down = st.pressed.get() == Some(btn);
            c.round_rect(r, 5.0 * s, WHITE, if down { 0.16 } else { 0.10 });
        }
        let (cx, cy) = ((r.left + r.right) as f32 / 2.0, (r.top + r.bottom) as f32 / 2.0);
        match btn {
            Btn::Stop => {
                let half = 5.0 * s;
                let sq = RECT {
                    left: (cx - half).round() as i32,
                    top: (cy - half).round() as i32,
                    right: (cx + half).round() as i32,
                    bottom: (cy + half).round() as i32,
                };
                c.round_rect(sq, 1.5 * s, WHITE, 1.0);
            }
            Btn::Cancel => {
                let d = 4.5 * s;
                c.line((cx - d, cy - d), (cx + d, cy + d), 1.5 * s, WHITE);
                c.line((cx - d, cy + d), (cx + d, cy - d), 1.5 * s, WHITE);
            }
        }
    }
}

fn draw_text(mem: windows::Win32::Graphics::Gdi::HDC, st: &State) {
    let dpi = st.dpi.get();
    // SAFETY: font is created, selected, and deleted within this function.
    unsafe {
        let font = CreateFontW(
            -scale(14, dpi),
            0,
            0,
            0,
            FW_SEMIBOLD.0 as i32,
            0,
            0,
            0,
            Default::default(),
            OUT_DEFAULT_PRECIS,
            Default::default(),
            CLEARTYPE_QUALITY,
            FF_DONTCARE.0 as u32,
            w!("Segoe UI Variable Text"),
        );
        let old = SelectObject(mem, font.into());
        SetBkMode(mem, TRANSPARENT);
        SetTextColor(mem, COLORREF(rgb_to_colorref(WHITE)));
        let mut text: Vec<u16> = if st.show_elapsed {
            format_elapsed(st.elapsed.get()).encode_utf16().collect()
        } else {
            "Recording".encode_utf16().collect()
        };
        let mut r = RECT { left: scale(32, dpi), top: 0, right: scale(STOP_X - 4, dpi), bottom: scale(HEIGHT, dpi) };
        DrawTextW(mem, &mut text, &mut r, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
        SelectObject(mem, old);
        let _ = DeleteObject(HGDIOBJ(font.0));
    }
}
