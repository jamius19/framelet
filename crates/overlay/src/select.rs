/*
 * Author: Jamius Siam
 * Since: 31/10/2026
 */

//! Fullscreen region selection overlay: one layered window over the virtual desktop.
//! The selection stays editable (move / resize / redraw) until confirmed with Enter, a double click or the
//! toolbar's Record button, or canceled with Esc, right click or the toolbar's ✕ button.
//! All geometry here is in client coordinates (= buffer coordinates); `origin` maps to screen.

use crate::gfx::{Button, Dib, Font, Pill, Rect, blend_pill, dp, fill_rect, render_pill, render_toolbar};
use crate::win::{self, EscGuard};
use framelet_core::{ExportScale, Region};
use std::cell::{Cell, RefCell};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{HMONITOR, MONITOR_DEFAULTTONEAREST, MonitorFromPoint};
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

const CLASS: PCWSTR = w!("FrameletSelectOverlay");
const MIN_SIZE: i32 = 8;
/// Premultiplied ARGB: black at 45%, near-clear selection interior (alpha 0 would let clicks through).
const DIM: u32 = 0x7300_0000;
const CLEAR: u32 = 0x0100_0000;
const WHITE: u32 = 0xFFFF_FFFF;
/// Black at 60%: backs the white dashes (fills their gaps and the outer outline) so they read on white content.
const SHADOW: u32 = 0x9900_0000;
const HANDLE_OUTLINE: u32 = 0xB300_0000;
const HINT: &str = "Drag to select an area · Esc to cancel";

struct Select {
    done: Cell<bool>,
    result: Cell<Option<(Region, ExportScale)>>,
    inner: RefCell<Inner>,
}

/// DPI-dependent sizes in physical pixels.
#[derive(Clone, Copy)]
struct Metrics {
    /// White dash thickness, and the whole border ring (white + 1 px dark outline) around the selection.
    line: i32,
    ring: i32,
    dash: i32,
    period: i32,
    /// Handle square size, corner grab radius, grab band outside / inside an edge, toolbar distance.
    handle: i32,
    corner: i32,
    grab_out: i32,
    grab_in: i32,
    gap: i32,
}

impl Metrics {
    fn new(dpi: u32) -> Self {
        let line = dp(1.0, dpi).max(1);
        let dash = dp(5.0, dpi).max(2);
        Self {
            line,
            ring: line + 1,
            dash,
            period: dash + dp(4.0, dpi).max(2),
            handle: dp(8.0, dpi).max(5),
            corner: dp(8.0, dpi),
            grab_out: dp(6.0, dpi),
            grab_in: dp(4.0, dpi),
            gap: dp(8.0, dpi),
        }
    }
}

/// What lies under a point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hit {
    Outside,
    Inside,
    /// Resize handle: which edge moves on each axis (-1 left/top, 1 right/bottom, 0 none).
    Handle(i8, i8),
    Toolbar,
    Button(Button),
}

enum Drag {
    /// Drawing a new rectangle; `prev` is restored if the result is too small.
    New { anchor: POINT, prev: Option<Rect> },
    Move { start: POINT, orig: Rect },
    /// `(dx, dy)` is the grabbed handle, `eff` the one moving now (differs after dragging past the opposite edge).
    Resize { start: POINT, orig: Rect, dx: i8, dy: i8, eff: (i8, i8) },
    Press(Button),
}

enum Action {
    SetScale(ExportScale),
    Confirm,
    Cancel,
}

/// Where the toolbar was painted, in client coordinates (button rectangles span the full pill height).
struct Placed {
    bounds: Rect,
    scales: [Rect; 3],
    record: Rect,
    cancel: Rect,
}

/// Bounds of everything painted last frame.
#[derive(Default)]
struct Drawn {
    sel: Option<Rect>,
    toolbar: Option<Rect>,
    hint: Option<Rect>,
}

struct Inner {
    hwnd: HWND,
    dib: Dib,
    origin: POINT,
    /// Monitor under the cursor, its bounds and DPI; drives label scaling and hint placement.
    monitor: HMONITOR,
    monitor_rect: Rect,
    dpi: u32,
    metrics: Metrics,
    label_font: Font,
    hint: Option<Pill>,
    sel: Option<Rect>,
    drag: Option<Drag>,
    hover: Option<Button>,
    /// Export size chosen on the toolbar; starts at 1x on every open.
    scale: ExportScale,
    /// Set by a double click inside the selection; confirms when that click is released.
    confirm_on_up: bool,
    toolbar: Option<Placed>,
    drawn: Drawn,
}

pub fn run(initial: Option<Region>) -> Option<(Region, ExportScale)> {
    let (x, y, w, h) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };
    let hwnd = win::create_layered_popup(CLASS, wndproc, IDC_CROSS, Default::default(), Rect::new(x, y, x + w, y + h))?;
    let Some(dib) = Dib::new(w, h) else {
        win::destroy_and_flush(hwnd);
        return None;
    };

    let mut cursor = POINT::default();
    let _ = unsafe { GetCursorPos(&mut cursor) };
    let monitor = unsafe { MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST) };
    let origin = POINT { x, y };
    let mut inner = Inner {
        hwnd,
        dib,
        origin,
        monitor,
        monitor_rect: Rect::new(0, 0, 0, 0),
        dpi: 0,
        metrics: Metrics::new(96),
        label_font: Font::new(13.0, 600, 96),
        hint: None,
        sel: initial.and_then(|r| initial_rect(r, origin, w, h)),
        drag: None,
        hover: None,
        scale: ExportScale::Full,
        confirm_on_up: false,
        toolbar: None,
        drawn: Drawn { sel: Some(Rect::new(0, 0, w, h)), ..Default::default() },
    };
    inner.set_monitor(monitor);
    inner.render(false);
    let state = Select { done: Cell::new(false), result: Cell::new(None), inner: RefCell::new(inner) };

    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, &state as *const Select as isize);
        let _ = ShowWindow(hwnd, SW_SHOW);
    }
    let esc = EscGuard::with_enter(hwnd);
    let completed = win::pump(|| state.done.get());
    drop(esc);

    unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) };
    win::destroy_and_flush(hwnd);
    if completed { state.result.take() } else { None }
}

/// `region` in client coordinates clamped to the desktop (`w`x`h`), or `None` if too little of it is left.
fn initial_rect(region: Region, origin: POINT, w: i32, h: i32) -> Option<Rect> {
    let clamp = |start: i32, len: u32, origin: i32, max: i32| {
        let lo = (start as i64 - origin as i64).clamp(0, max as i64);
        let hi = (start as i64 + len as i64 - origin as i64).clamp(0, max as i64);
        (lo as i32, hi as i32)
    };
    let (left, right) = clamp(region.x, region.width, origin.x, w);
    let (top, bottom) = clamp(region.y, region.height, origin.y, h);
    (right - left >= MIN_SIZE && bottom - top >= MIN_SIZE).then(|| Rect::new(left, top, right, bottom))
}

impl Inner {
    /// Switches to the monitor under the cursor, rebuilding DPI-dependent resources if needed.
    fn set_monitor(&mut self, monitor: HMONITOR) {
        self.monitor = monitor;
        self.monitor_rect = self.client_monitor_rect(monitor);
        let dpi = win::monitor_dpi(monitor);
        if dpi != self.dpi {
            self.dpi = dpi;
            self.metrics = Metrics::new(dpi);
            self.label_font = Font::new(13.0, 600, dpi);
            let hint_font = Font::new(15.0, 400, dpi);
            self.hint = render_pill(HINT, &hint_font, (dp(20.0, dpi), dp(12.0, dpi)), dp(12.0, dpi), None);
        }
    }

    fn client_monitor_rect(&self, monitor: HMONITOR) -> Rect {
        win::monitor_rect(monitor).offset(-self.origin.x, -self.origin.y)
    }

    /// Bounds of the monitor that holds the center of `s`; the toolbar is kept on it.
    fn selection_monitor_rect(&self, s: Rect) -> Rect {
        let center = POINT { x: (s.left + s.right) / 2 + self.origin.x, y: (s.top + s.bottom) / 2 + self.origin.y };
        self.client_monitor_rect(unsafe { MonitorFromPoint(center, MONITOR_DEFAULTTONEAREST) })
    }

    /// Repaints what changed. `toolbar_only`: just the toolbar's hover state changed.
    fn render(&mut self, toolbar_only: bool) {
        let (width, height) = (self.dib.width, self.dib.height);
        let full = Rect::new(0, 0, width, height);
        let (dpi, m) = (self.dpi, self.metrics);

        let toolbar = self.sel.and_then(|s| {
            let mut text = format!("{} × {}", s.width(), s.height());
            if self.scale != ExportScale::Full {
                let (w, h) = self.scale.output_size(s.width() as u32, s.height() as u32);
                text = format!("{text} → {w} × {h}");
            }
            let bar = render_toolbar(&text, &self.label_font, self.scale, self.hover, dpi)?;
            let (x, y) = label_pos(s, &bar.pill, self.selection_monitor_rect(s), m.gap);
            Some((x, y, bar))
        });
        let hint = self.sel.is_none().then_some(()).and(self.hint.as_ref()).map(|p| {
            let mon = self.monitor_rect;
            ((mon.left + mon.right - p.width) / 2, (mon.top + mon.bottom - p.height) / 2, p)
        });

        self.toolbar = toolbar.as_ref().map(|(x, y, bar)| {
            let bounds = Rect::new(*x, *y, x + bar.pill.width, y + bar.pill.height);
            let span = |r: Rect| Rect::new(x + r.left, bounds.top, x + r.right, bounds.bottom);
            Placed { bounds, scales: bar.scales.map(span), record: span(bar.record), cancel: span(bar.cancel) }
        });
        let drawn = Drawn {
            sel: self.sel.map(|s| s.inflate(m.ring + m.handle / 2 + 2)),
            toolbar: self.toolbar.as_ref().map(|t| t.bounds),
            hint: hint.map(|(x, y, p)| Rect::new(x, y, x + p.width, y + p.height)),
        };

        let old = std::mem::replace(&mut self.drawn, drawn);
        let (old, new) = (&old, &self.drawn);
        let dirty = if toolbar_only {
            [old.toolbar, new.toolbar].into_iter().flatten().reduce(Rect::union)
        } else {
            [old.sel, old.toolbar, old.hint, new.sel, new.toolbar, new.hint].into_iter().flatten().reduce(Rect::union)
        };
        let Some(dirty) = dirty.map(|r| r.intersect(full)).filter(|r| !r.is_empty()) else { return };

        let buf = self.dib.pixels();
        paint_background(buf, width, dirty, self.sel, &m);
        if let Some(s) = self.sel {
            paint_handles(buf, width, dirty, s, &m);
        }
        if let Some((x, y, bar)) = &toolbar {
            blend_pill(buf, width, dirty, *x, *y, &bar.pill);
        }
        if let Some((x, y, pill)) = hint {
            blend_pill(buf, width, dirty, x, y, pill);
        }

        self.dib.present(self.hwnd, self.origin, Some(dirty));
    }

    /// Region of the current selection, if it is large enough.
    fn region(&self) -> Option<Region> {
        let s = self.sel.filter(|s| s.width() >= MIN_SIZE && s.height() >= MIN_SIZE)?;
        Some(Region {
            x: self.origin.x + s.left,
            y: self.origin.y + s.top,
            width: s.width() as u32,
            height: s.height() as u32,
        })
    }

    fn hit(&self, pt: POINT) -> Hit {
        if let Some(t) = &self.toolbar {
            if let Some(i) = t.scales.iter().position(|r| r.contains(pt)) {
                return Hit::Button(Button::Scale(ExportScale::ALL[i]));
            }
            if t.record.contains(pt) {
                return Hit::Button(Button::Record);
            }
            if t.cancel.contains(pt) {
                return Hit::Button(Button::Cancel);
            }
            if t.bounds.contains(pt) {
                return Hit::Toolbar;
            }
        }
        let Some(s) = self.sel else { return Hit::Outside };
        let m = &self.metrics;

        // Corners get a roomier grab area than the edge bands (shrunk on tiny selections to keep a middle).
        let grab = m.corner.min(s.width() / 3).min(s.height() / 3);
        for (dx, dy) in [(-1, -1), (1, -1), (-1, 1), (1, 1)] {
            let cx = if dx < 0 { s.left } else { s.right };
            let cy = if dy < 0 { s.top } else { s.bottom };
            if (pt.x - cx).abs() <= grab && (pt.y - cy).abs() <= grab {
                return Hit::Handle(dx, dy);
            }
        }
        // Edge bands: -1 / 1 near the low / high edge, 0 in the middle, `None` out of reach.
        let zone = |v: i32, lo: i32, hi: i32| {
            let inner = m.grab_in.min((hi - lo) / 3);
            if v < lo - m.grab_out || v >= hi + m.grab_out {
                None
            } else if v < lo + inner {
                Some(-1)
            } else if v >= hi - inner {
                Some(1)
            } else {
                Some(0)
            }
        };
        match (zone(pt.x, s.left, s.right), zone(pt.y, s.top, s.bottom)) {
            (Some(0), Some(0)) => Hit::Inside,
            (Some(dx), Some(dy)) => Hit::Handle(dx, dy),
            _ => Hit::Outside,
        }
    }

    fn hover_at(&self, pt: POINT) -> Option<Button> {
        match self.hit(pt) {
            Hit::Button(b) => Some(b),
            _ => None,
        }
    }

    /// Cursor for the current drag, or for whatever is under the mouse.
    fn cursor(&self) -> PCWSTR {
        match &self.drag {
            Some(Drag::New { .. }) => IDC_CROSS,
            Some(Drag::Move { .. }) => IDC_SIZEALL,
            Some(Drag::Resize { eff, .. }) => handle_cursor(eff.0, eff.1),
            Some(Drag::Press(_)) => IDC_HAND,
            None => {
                let mut pt = POINT::default();
                let _ = unsafe { GetCursorPos(&mut pt) };
                match self.hit(POINT { x: pt.x - self.origin.x, y: pt.y - self.origin.y }) {
                    Hit::Outside => IDC_CROSS,
                    Hit::Inside => IDC_SIZEALL,
                    Hit::Handle(dx, dy) => handle_cursor(dx, dy),
                    Hit::Toolbar => IDC_ARROW,
                    Hit::Button(_) => IDC_HAND,
                }
            }
        }
    }

    fn on_mouse_move(&mut self, pt: POINT) {
        let mut changed = false;
        let screen = POINT { x: pt.x + self.origin.x, y: pt.y + self.origin.y };
        let monitor = unsafe { MonitorFromPoint(screen, MONITOR_DEFAULTTONEAREST) };
        if monitor != self.monitor {
            self.set_monitor(monitor);
            changed = true;
        }

        let (w, h) = (self.dib.width, self.dib.height);
        let sel = match &mut self.drag {
            Some(Drag::New { anchor, .. }) => Some(drag_rect(w, h, *anchor, pt)),
            Some(Drag::Move { start, orig }) => Some(move_rect(w, h, *orig, pt.x - start.x, pt.y - start.y)),
            Some(Drag::Resize { start, orig, dx, dy, eff }) => {
                let (rect, ex, ey) = resize_rect(w, h, *orig, (*dx, *dy), pt.x - start.x, pt.y - start.y);
                *eff = (ex, ey);
                Some(rect)
            }
            Some(Drag::Press(_)) | None => None,
        };
        if sel.is_some() {
            changed |= sel != self.sel;
            self.sel = sel;
        }

        let mut hover_changed = false;
        if matches!(self.drag, None | Some(Drag::Press(_))) {
            let hover = self.hover_at(pt);
            hover_changed = hover != self.hover;
            self.hover = hover;
        }
        if changed || hover_changed {
            self.render(!changed);
        }
    }

    fn on_button_down(&mut self, pt: POINT, double: bool) {
        self.confirm_on_up = false;
        let drag = match (self.hit(pt), self.sel) {
            (Hit::Toolbar, _) => return,
            (Hit::Button(b), _) => Drag::Press(b),
            (Hit::Handle(dx, dy), Some(orig)) => Drag::Resize { start: pt, orig, dx, dy, eff: (dx, dy) },
            (Hit::Inside, Some(orig)) => {
                self.confirm_on_up = double;
                Drag::Move { start: pt, orig }
            }
            _ => Drag::New { anchor: pt, prev: self.sel },
        };
        unsafe { SetCapture(self.hwnd) };
        self.drag = Some(drag);
    }

    /// Switches the export scale; the toolbar's width follows its size label, so the hover is re-evaluated.
    fn set_scale(&mut self, scale: ExportScale, pt: POINT) {
        if scale == self.scale {
            return;
        }
        self.scale = scale;
        self.render(true);
        let hover = self.hover_at(pt);
        if hover != self.hover {
            self.hover = hover;
            self.render(true);
        }
    }

    /// Finishes a drag; returns what a click on a toolbar button or a double click asks for.
    fn on_button_up(&mut self, pt: POINT) -> Option<Action> {
        let drag = self.drag.take()?;
        let _ = unsafe { ReleaseCapture() };
        match drag {
            Drag::Press(b) => (self.hover_at(pt) == Some(b)).then_some(match b {
                Button::Scale(scale) => Action::SetScale(scale),
                Button::Record => Action::Confirm,
                Button::Cancel => Action::Cancel,
            }),
            Drag::New { prev, .. } => {
                // Too small: keep what was there before (or nothing).
                if self.region().is_none() && self.sel != prev {
                    self.sel = prev;
                    self.render(false);
                }
                None
            }
            Drag::Move { .. } | Drag::Resize { .. } => self.confirm_on_up.then_some(Action::Confirm),
        }
    }
}

/// Rectangle spanned by two points, both pixels included, clamped to the `w`x`h` desktop.
fn drag_rect(w: i32, h: i32, a: POINT, b: POINT) -> Rect {
    let (ax, bx) = (a.x.clamp(0, w - 1), b.x.clamp(0, w - 1));
    let (ay, by) = (a.y.clamp(0, h - 1), b.y.clamp(0, h - 1));
    Rect::new(ax.min(bx), ay.min(by), ax.max(bx) + 1, ay.max(by) + 1)
}

/// `orig` shifted by (`dx`, `dy`), kept inside the `w`x`h` desktop.
fn move_rect(w: i32, h: i32, orig: Rect, dx: i32, dy: i32) -> Rect {
    let left = (orig.left + dx).clamp(0, (w - orig.width()).max(0));
    let top = (orig.top + dy).clamp(0, (h - orig.height()).max(0));
    Rect::new(left, top, left + orig.width(), top + orig.height())
}

/// `orig` with the grabbed edges shifted by (`dx`, `dy`). Returns the new rectangle and which edge now moves
/// on each axis (flips when dragged past the opposite edge).
fn resize_rect(w: i32, h: i32, orig: Rect, handle: (i8, i8), dx: i32, dy: i32) -> (Rect, i8, i8) {
    let (left, right, ex) = resize_axis(orig.left, orig.right, handle.0, dx, w);
    let (top, bottom, ey) = resize_axis(orig.top, orig.bottom, handle.1, dy, h);
    (Rect::new(left, top, right, bottom), ex, ey)
}

/// One axis of [`resize_rect`]: moves the `dir` edge of `lo..hi` by `delta` within `0..max`, normalizing and
/// enforcing `MIN_SIZE`.
fn resize_axis(lo: i32, hi: i32, dir: i8, delta: i32, max: i32) -> (i32, i32, i8) {
    if dir == 0 {
        return (lo, hi, 0);
    }
    let (moving, fixed) = if dir < 0 { (lo + delta, hi) } else { (hi + delta, lo) };
    let moving = moving.clamp(0, max);
    let eff = if moving >= fixed { 1 } else { -1 };
    let (a, b) = if eff > 0 { (fixed, moving.max(fixed + MIN_SIZE)) } else { (moving.min(fixed - MIN_SIZE), fixed) };
    let (a, b) = if b > max {
        (max - MIN_SIZE, max)
    } else if a < 0 {
        (0, MIN_SIZE)
    } else {
        (a, b)
    };
    (a, b, eff)
}

fn handle_cursor(dx: i8, dy: i8) -> PCWSTR {
    match (dx, dy) {
        (0, _) => IDC_SIZENS,
        (_, 0) => IDC_SIZEWE,
        (a, b) if a == b => IDC_SIZENWSE,
        _ => IDC_SIZENESW,
    }
}

/// Toolbar just below-right of the selection; flips to the inside edge when that would leave the
/// monitor, and is finally clamped to it.
fn label_pos(sel: Rect, pill: &Pill, mon: Rect, gap: i32) -> (i32, i32) {
    let mut x = sel.right + gap;
    if x + pill.width > mon.right {
        x = sel.right - gap - pill.width;
    }
    let mut y = sel.bottom + gap;
    if y + pill.height > mon.bottom {
        y = sel.bottom - gap - pill.height;
    }
    (
        x.clamp(mon.left, (mon.right - pill.width).max(mon.left)),
        y.clamp(mon.top, (mon.bottom - pill.height).max(mon.top)),
    )
}

/// Color of the border ring at (`x`, `y`) just outside `s`: white dashes along the edges (solid at the
/// corners) over a dark shadow, which also forms the outermost pixel. Dashes are anchored to the
/// selection's top-left corner, so they travel with it.
fn ring_pixel(x: i32, y: i32, s: Rect, m: &Metrics) -> u32 {
    let dx = (s.left - x).max(x - (s.right - 1)).max(0);
    let dy = (s.top - y).max(y - (s.bottom - 1)).max(0);
    if dx.max(dy) > m.line {
        return SHADOW;
    }
    let along = match (dx > 0, dy > 0) {
        (true, true) => return WHITE,
        (false, true) => x - s.left,
        _ => y - s.top,
    };
    if along.rem_euclid(m.period) < m.dash { WHITE } else { SHADOW }
}

/// Paints dim / border ring / clear interior into `dirty`, one row of spans at a time.
fn paint_background(buf: &mut [u32], stride: i32, dirty: Rect, sel: Option<Rect>, m: &Metrics) {
    for y in dirty.top..dirty.bottom {
        let row = &mut buf[(y * stride) as usize..((y + 1) * stride) as usize];
        let fill = |row: &mut [u32], x0: i32, x1: i32, color: u32| {
            let (a, b) = (x0.max(dirty.left), x1.min(dirty.right));
            if a < b {
                row[a as usize..b as usize].fill(color);
            }
        };
        match sel.map(|s| (s, s.inflate(m.ring))) {
            Some((s, o)) if y >= o.top && y < o.bottom => {
                fill(row, 0, o.left, DIM);
                fill(row, o.right, stride, DIM);
                let ring = |row: &mut [u32], x0: i32, x1: i32| {
                    for x in x0.max(dirty.left)..x1.min(dirty.right) {
                        row[x as usize] = ring_pixel(x, y, s, m);
                    }
                };
                if y < s.top || y >= s.bottom {
                    ring(row, o.left, o.right);
                } else {
                    ring(row, o.left, s.left);
                    fill(row, s.left, s.right, CLEAR);
                    ring(row, s.right, o.right);
                }
            }
            _ => fill(row, 0, stride, DIM),
        }
    }
}

/// Eight white handle squares with a dark outline, centered on the border ring; edge handles are left out
/// when the edge is too short for them.
fn paint_handles(buf: &mut [u32], stride: i32, clip: Rect, s: Rect, m: &Metrics) {
    let xs = [s.left - m.ring / 2, (s.left + s.right) / 2, s.right + m.ring / 2];
    let ys = [s.top - m.ring / 2, (s.top + s.bottom) / 2, s.bottom + m.ring / 2];
    for (row, &cy) in ys.iter().enumerate() {
        for (col, &cx) in xs.iter().enumerate() {
            let skip = (row == 1 && col == 1)
                || (col == 1 && s.width() < 3 * m.handle)
                || (row == 1 && s.height() < 3 * m.handle);
            if skip {
                continue;
            }
            let (left, top) = (cx - m.handle / 2, cy - m.handle / 2);
            let square = Rect::new(left, top, left + m.handle, top + m.handle);
            fill_rect(buf, stride, clip, square.inflate(1), HANDLE_OUTLINE);
            fill_rect(buf, stride, clip, square, WHITE);
        }
    }
}

fn client_point(lparam: LPARAM) -> POINT {
    POINT { x: lparam.0 as i16 as i32, y: (lparam.0 >> 16) as i16 as i32 }
}

/// Finishes with the current selection and export scale, if the selection is valid.
fn confirm(state: &Select) {
    let result = state.inner.try_borrow().ok().and_then(|s| Some((s.region()?, s.scale)));
    if result.is_some() {
        state.result.set(result);
        state.done.set(true);
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: GWLP_USERDATA is set in `run` to a `Select` that outlives the window, and cleared before drop.
    let state = unsafe { (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Select).as_ref() };
    let Some(state) = state else {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    };
    // `try_borrow_mut`: Win32 calls made while borrowed (SetCapture, ReleaseCapture...) may re-enter.
    let inner = || state.inner.try_borrow_mut().ok();

    if EscGuard::is_esc(msg, wparam) {
        state.done.set(true);
        return LRESULT(0);
    }
    if EscGuard::is_enter(msg, wparam) {
        confirm(state);
        return LRESULT(0);
    }
    match msg {
        WM_MOUSEMOVE => {
            if let Some(mut s) = inner() {
                s.on_mouse_move(client_point(lparam));
            }
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            if let Some(mut s) = inner() {
                s.on_button_down(client_point(lparam), msg == WM_LBUTTONDBLCLK);
            }
        }
        WM_LBUTTONUP => {
            let pt = client_point(lparam);
            match inner().and_then(|mut s| s.on_button_up(pt)) {
                Some(Action::SetScale(scale)) => {
                    if let Some(mut s) = inner() {
                        s.set_scale(scale, pt);
                    }
                }
                Some(Action::Confirm) => confirm(state),
                Some(Action::Cancel) => state.done.set(true),
                None => {}
            }
        }
        // Cancel on button-up so the matching "up" is not delivered to the window underneath.
        WM_RBUTTONUP | WM_DISPLAYCHANGE => state.done.set(true),
        WM_SETCURSOR => {
            let cursor = state.inner.try_borrow().map_or(IDC_CROSS, |s| s.cursor());
            if let Ok(cursor) = unsafe { LoadCursorW(None, cursor) } {
                unsafe { SetCursor(Some(cursor)) };
            }
            return LRESULT(1);
        }
        _ => return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
    LRESULT(0)
}
