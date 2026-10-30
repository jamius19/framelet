/*
 * Author: Jamius Siam
 * Since: 31/10/2026
 */

//! Recording frame: a red border just outside the recorded region and, optionally, a dim over the rest
//! of the virtual desktop. Built from plain solid layered windows (no per-pixel bitmaps), so it costs
//! next to no memory. Non-modal: the windows live on the calling thread until the frame is dropped.

use crate::gfx::{Rect, dp};
use crate::win;
use framelet_core::Region;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BLACK_BRUSH, CombineRgn, CreateRectRgn, CreateSolidBrush, DeleteObject, FillRect, GetStockObject, HBRUSH, HDC,
    HGDIOBJ, MONITOR_DEFAULTTONEAREST, MonitorFromRect, RGN_DIFF, SetWindowRgn,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

const CLASS: PCWSTR = w!("FrameletRecordingFrame");
/// Border thickness in DIP, scaled by the DPI of the region's monitor.
const BORDER_DIP: f32 = 2.0;
/// Same red as the controller's recording dot, as a GDI `0x00BBGGRR` color.
const RED: COLORREF = COLORREF(0x003A_45FF);
/// Black at 45%, like the selection overlay's dim.
const DIM_ALPHA: u8 = 0x73;

/// Frame shown while recording. Dropping it destroys the windows, their regions and the brush.
///
/// Non-modal: the windows are plain click-through, never-activated, topmost layered windows on the
/// calling (UI) thread. No timers, no threads, no message loop of its own; the caller's loop services them.
pub struct RecordingFrame {
    windows: Vec<HWND>,
    /// Owned by the frame (the windows only borrow it), deleted after they are destroyed.
    red: HBRUSH,
}

impl RecordingFrame {
    /// Shows a red border (~2 DIP, just outside `region` so it never enters the captured pixels) and, if
    /// `dim`, a 45% black dim over the rest of the virtual desktop. Excluded from screen capture.
    /// Create it before the floating controller so the controller stays above it.
    /// Returns `None` if a window could not be created (nothing is left behind).
    pub fn show(region: Region, dim: bool) -> Option<Self> {
        Self::create(region, dim, false)
    }

    /// Like `show`, but without `WDA_EXCLUDEFROMCAPTURE`, so screenshots can see the frame (demos only).
    #[doc(hidden)]
    pub fn show_capturable(region: Region, dim: bool) -> Option<Self> {
        Self::create(region, dim, true)
    }

    fn create(region: Region, dim: bool, capturable: bool) -> Option<Self> {
        let rect = Rect::new(region.x, region.y, region.right(), region.bottom());
        let win_rect = RECT { left: rect.left, top: rect.top, right: rect.right, bottom: rect.bottom };
        // SAFETY: plain Win32 call.
        let dpi = win::monitor_dpi(unsafe { MonitorFromRect(&win_rect, MONITOR_DEFAULTTONEAREST) });
        let thickness = dp(BORDER_DIP, dpi).max(1);

        // On any failure `?` drops `frame`, which destroys whatever was already created.
        let mut frame = RecordingFrame { windows: Vec::new(), red: HBRUSH::default() };
        if dim {
            // SAFETY: stock objects are shared and must not be deleted.
            let black = HBRUSH(unsafe { GetStockObject(BLACK_BRUSH) }.0);
            for strip in dim_strips(rect, virtual_desktop()).into_iter().filter(|s| !s.is_empty()) {
                frame.add(strip, black, DIM_ALPHA, capturable)?;
            }
        }
        // Created after the dim windows, so the border sits above them.
        // SAFETY: plain GDI call; the brush is deleted in `drop`.
        frame.red = unsafe { CreateSolidBrush(RED) };
        let outer = rect.inflate(thickness);
        let border = frame.add(outer, frame.red, 255, capturable)?;
        set_ring_region(border, outer, thickness)?;

        for hwnd in &frame.windows {
            // SAFETY: windows created above on this thread.
            let _ = unsafe { ShowWindow(*hwnd, SW_SHOWNOACTIVATE) };
        }
        Some(frame)
    }

    /// Creates a click-through, never-activated layered window at `rect` that paints itself with `brush`.
    fn add(&mut self, rect: Rect, brush: HBRUSH, alpha: u8, capturable: bool) -> Option<HWND> {
        let ex = WS_EX_TRANSPARENT | WS_EX_NOACTIVATE;
        let hwnd = win::create_layered_popup(CLASS, wndproc, IDC_ARROW, ex, rect)?;
        self.windows.push(hwnd);
        // SAFETY: `hwnd` is ours; the brush outlives it (stock object, or deleted after the windows).
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, brush.0 as isize);
            SetLayeredWindowAttributes(hwnd, COLORREF(0), alpha, LWA_ALPHA)
                .inspect_err(|e| log::error!("SetLayeredWindowAttributes failed: {e}"))
                .ok()?;
            if !capturable {
                let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
            }
        }
        Some(hwnd)
    }
}

impl Drop for RecordingFrame {
    fn drop(&mut self) {
        // SAFETY: the windows (and their regions) belong to this thread; the brush is deleted once unused.
        unsafe {
            for hwnd in self.windows.drain(..) {
                let _ = DestroyWindow(hwnd);
            }
            if !self.red.0.is_null() {
                let _ = DeleteObject(HGDIOBJ(self.red.0));
            }
        }
    }
}

fn virtual_desktop() -> Rect {
    // SAFETY: plain Win32 calls.
    let (x, y, w, h) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };
    Rect::new(x, y, x + w, y + h)
}

/// Top, bottom, left and right strips: `desktop` minus `region` (clamped to it). Some may be empty.
fn dim_strips(region: Rect, desktop: Rect) -> [Rect; 4] {
    let r = Rect::new(
        region.left.clamp(desktop.left, desktop.right),
        region.top.clamp(desktop.top, desktop.bottom),
        region.right.clamp(desktop.left, desktop.right),
        region.bottom.clamp(desktop.top, desktop.bottom),
    );
    [
        Rect::new(desktop.left, desktop.top, desktop.right, r.top),
        Rect::new(desktop.left, r.bottom, desktop.right, desktop.bottom),
        Rect::new(desktop.left, r.top, r.left, r.bottom),
        Rect::new(r.right, r.top, desktop.right, r.bottom),
    ]
}

/// Shapes `hwnd` (positioned at `outer`) into a ring `thickness` wide: the window minus its inner rectangle.
fn set_ring_region(hwnd: HWND, outer: Rect, thickness: i32) -> Option<()> {
    let (w, h) = (outer.width(), outer.height());
    // SAFETY: on success the window owns `ring` and must not delete it; on failure we do.
    unsafe {
        let ring = CreateRectRgn(0, 0, w, h);
        let inner = CreateRectRgn(thickness, thickness, w - thickness, h - thickness);
        CombineRgn(Some(ring), Some(ring), Some(inner), RGN_DIFF);
        let _ = DeleteObject(HGDIOBJ(inner.0));
        if SetWindowRgn(hwnd, Some(ring), false) == 0 {
            log::error!("SetWindowRgn failed");
            let _ = DeleteObject(HGDIOBJ(ring.0));
            return None;
        }
    }
    Some(())
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => {
            // SAFETY: GWLP_USERDATA is the brush set in `add`; it outlives the window.
            unsafe {
                let brush = HBRUSH(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut _);
                if brush.0.is_null() {
                    return DefWindowProcW(hwnd, msg, wparam, lparam);
                }
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                FillRect(HDC(wparam.0 as *mut _), &rc, brush);
            }
            LRESULT(1)
        }
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        // SAFETY: default processing with the arguments we were given.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
