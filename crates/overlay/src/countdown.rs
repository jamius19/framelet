/*
 * Author: Jamius Siam
 * Since: 31/10/2026
 */

//! Countdown: a small layered window centered over the region, one `WM_TIMER` tick per second.

use crate::gfx::{Dib, Font, Rect, dp, render_pill};
use crate::win::{self, EscGuard};
use framelet_core::Region;
use std::cell::{Cell, RefCell};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromPoint};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

const CLASS: PCWSTR = w!("FrameletCountdownOverlay");
const TIMER_ID: usize = 1;

struct Countdown {
    done: Cell<bool>,
    cancelled: Cell<bool>,
    remaining: Cell<u32>,
    inner: RefCell<Inner>,
}

struct Inner {
    hwnd: HWND,
    dib: Dib,
    font: Font,
    radius: i32,
    origin: POINT,
}

pub fn run(region: Region, seconds: u32) -> bool {
    let center = POINT { x: region.x + region.width as i32 / 2, y: region.y + region.height as i32 / 2 };
    let dpi = win::monitor_dpi(unsafe { MonitorFromPoint(center, MONITOR_DEFAULTTONEAREST) });
    let size = dp(112.0, dpi);
    let origin = POINT { x: center.x - size / 2, y: center.y - size / 2 };
    let rect = Rect::new(origin.x, origin.y, origin.x + size, origin.y + size);

    // Click-through, so the countdown never steals clicks meant for the app being recorded.
    let Some(hwnd) = win::create_layered_popup(CLASS, wndproc, IDC_ARROW, WS_EX_TRANSPARENT, rect) else {
        log::warn!("countdown window unavailable, skipping countdown");
        return true;
    };
    let Some(dib) = Dib::new(size, size) else {
        win::destroy_and_flush(hwnd);
        return true;
    };
    let mut inner = Inner { hwnd, dib, font: Font::new(56.0, 600, dpi), radius: dp(28.0, dpi), origin };
    inner.render(seconds);
    let state = Countdown {
        done: Cell::new(false),
        cancelled: Cell::new(false),
        remaining: Cell::new(seconds),
        inner: RefCell::new(inner),
    };

    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, &state as *const Countdown as isize);
        let _ = ShowWindow(hwnd, SW_SHOWNA);
        SetTimer(Some(hwnd), TIMER_ID, 1000, None);
    }
    let esc = EscGuard::new(hwnd);
    let completed = win::pump(|| state.done.get());
    drop(esc);

    unsafe {
        let _ = KillTimer(Some(hwnd), TIMER_ID);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
    }
    win::destroy_and_flush(hwnd);
    completed && !state.cancelled.get()
}

impl Inner {
    fn render(&mut self, number: u32) {
        let size = self.dib.width;
        let Some(pill) = render_pill(&number.to_string(), &self.font, (0, 0), self.radius, Some((size, size))) else {
            return;
        };
        self.dib.pixels().copy_from_slice(&pill.pixels);
        self.dib.present(self.hwnd, self.origin, None);
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: GWLP_USERDATA is set in `run` to a `Countdown` that outlives the window, and cleared before drop.
    let state = unsafe { (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Countdown).as_ref() };
    let Some(state) = state else {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    };

    if EscGuard::is_esc(msg, wparam) {
        state.cancelled.set(true);
        state.done.set(true);
        return LRESULT(0);
    }
    match msg {
        WM_TIMER => {
            let remaining = state.remaining.get() - 1;
            state.remaining.set(remaining);
            if remaining == 0 {
                state.done.set(true);
            } else if let Ok(mut inner) = state.inner.try_borrow_mut() {
                inner.render(remaining);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
