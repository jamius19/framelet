/*
 * Author: Jamius Siam
 * Since: 31/10/2026
 */

//! Software rendering helpers: DIB sections, rounded "pill" labels with GDI text.

use framelet_core::ExportScale;
use std::mem::size_of;
use std::ptr::{null, null_mut};
use windows::Win32::Foundation::{COLORREF, HWND, POINT, RECT, SIZE};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::{ULW_ALPHA, UPDATELAYEREDWINDOWINFO, UpdateLayeredWindowIndirect};
use windows::core::w;

/// Integer rectangle, right/bottom exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self { left, top, right, bottom }
    }

    pub fn width(&self) -> i32 {
        self.right - self.left
    }

    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }

    pub fn is_empty(&self) -> bool {
        self.right <= self.left || self.bottom <= self.top
    }

    pub fn union(self, o: Rect) -> Rect {
        Rect::new(self.left.min(o.left), self.top.min(o.top), self.right.max(o.right), self.bottom.max(o.bottom))
    }

    pub fn intersect(self, o: Rect) -> Rect {
        Rect::new(self.left.max(o.left), self.top.max(o.top), self.right.min(o.right), self.bottom.min(o.bottom))
    }

    pub fn contains(&self, p: POINT) -> bool {
        p.x >= self.left && p.x < self.right && p.y >= self.top && p.y < self.bottom
    }

    pub fn offset(self, dx: i32, dy: i32) -> Rect {
        Rect::new(self.left + dx, self.top + dy, self.right + dx, self.bottom + dy)
    }

    pub fn inflate(self, d: i32) -> Rect {
        Rect::new(self.left - d, self.top - d, self.right + d, self.bottom + d)
    }

    fn to_win(self) -> RECT {
        RECT { left: self.left, top: self.top, right: self.right, bottom: self.bottom }
    }
}

/// Scales device-independent pixels to physical pixels.
pub fn dp(value: f32, dpi: u32) -> i32 {
    (value * dpi as f32 / 96.0).round() as i32
}

/// Top-down 32-bpp DIB section with its own memory DC. Pixels are premultiplied `0xAARRGGBB`.
pub struct Dib {
    dc: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u32,
    pub width: i32,
    pub height: i32,
}

impl Dib {
    pub fn new(width: i32, height: i32) -> Option<Self> {
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        unsafe {
            let dc = CreateCompatibleDC(None);
            let mut bits = null_mut();
            let Ok(bitmap) = CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) else {
                let _ = DeleteDC(dc);
                return None;
            };
            let old = SelectObject(dc, bitmap.into());
            Some(Self { dc, bitmap, old, bits: bits.cast(), width, height })
        }
    }

    pub fn pixels(&mut self) -> &mut [u32] {
        // SAFETY: `bits` points to a `width * height` u32 buffer owned by the bitmap, valid until drop.
        unsafe { std::slice::from_raw_parts_mut(self.bits, (self.width * self.height) as usize) }
    }

    /// Pushes the DIB to a layered window placed at `pos`; `dirty` limits the update to a sub-rectangle.
    /// Position and size are passed on every call: later updates that omit them were not displayed.
    pub fn present(&self, hwnd: HWND, pos: POINT, dirty: Option<Rect>) {
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let src = POINT::default();
        let size = SIZE { cx: self.width, cy: self.height };
        let dirty = dirty.map(Rect::to_win);
        let info = UPDATELAYEREDWINDOWINFO {
            cbSize: size_of::<UPDATELAYEREDWINDOWINFO>() as u32,
            hdcDst: HDC::default(),
            pptDst: &pos,
            psize: &size,
            hdcSrc: self.dc,
            pptSrc: &src,
            crKey: COLORREF(0),
            pblend: &blend,
            dwFlags: ULW_ALPHA,
            prcDirty: dirty.as_ref().map_or(null(), |r| r),
        };
        if !unsafe { UpdateLayeredWindowIndirect(hwnd, &info) }.as_bool() {
            log::warn!("UpdateLayeredWindow failed");
        }
    }
}

impl Drop for Dib {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

/// GDI font, deleted on drop.
pub struct Font(HFONT);

impl Font {
    pub fn new(size_dp: f32, weight: i32, dpi: u32) -> Self {
        Self(unsafe {
            CreateFontW(
                -dp(size_dp, dpi),
                0,
                0,
                0,
                weight,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                ANTIALIASED_QUALITY,
                (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
                w!("Segoe UI Variable Text"),
            )
        })
    }
}

impl Drop for Font {
    fn drop(&mut self) {
        let _ = unsafe { DeleteObject(self.0.into()) };
    }
}

/// Premultiplied-alpha label bitmap.
pub struct Pill {
    pub width: i32,
    pub height: i32,
    pub pixels: Vec<u32>,
}

const PILL_BG: f32 = 28.0 / 255.0;
const PILL_ALPHA: f32 = 0.92;
const PILL_STROKE: f32 = 0.12;

/// Signed distance from a point to a rounded rectangle of size `w`x`h` (negative inside).
fn rounded_rect_sdf(x: f32, y: f32, w: f32, h: f32, r: f32) -> f32 {
    let qx = (x - w / 2.0).abs() - (w / 2.0 - r);
    let qy = (y - h / 2.0).abs() - (h / 2.0 - r);
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

/// Extent of `text` in `font`, in pixels.
fn text_size(text: &str, font: &Font) -> SIZE {
    let wide: Vec<u16> = text.encode_utf16().collect();
    let mut ext = SIZE::default();
    unsafe {
        let dc = CreateCompatibleDC(None);
        let old = SelectObject(dc, font.0.into());
        let _ = GetTextExtentPoint32W(dc, &wide, &mut ext);
        SelectObject(dc, old);
        let _ = DeleteDC(dc);
    }
    ext
}

/// Premultiplied pill pixel at signed distance `d` from the outline, with white `text` coverage on top.
fn pill_pixel(d: f32, text: f32) -> u32 {
    let shape = (0.5 - d).clamp(0.0, 1.0);
    // Neutral gray background, so one premultiplied channel is enough; white layers on top.
    let (mut v, mut a) = (PILL_BG * PILL_ALPHA, PILL_ALPHA);
    for layer in [PILL_STROKE * (d + 1.5).clamp(0.0, 1.0), text] {
        v = layer + v * (1.0 - layer);
        a = layer + a * (1.0 - layer);
    }
    let a8 = (a * shape * 255.0).round() as u32;
    let v8 = ((v * shape * 255.0).round() as u32).min(a8);
    a8 << 24 | v8 << 16 | v8 << 8 | v8
}

/// Renders `text` centered in a dark translucent rounded rectangle. The size is `fixed`, or the
/// text extent plus `pad` on each side. Text is drawn with GDI as white-on-black grayscale
/// coverage and composited analytically, so it stays clean on the translucent background.
pub fn render_pill(text: &str, font: &Font, pad: (i32, i32), radius: i32, fixed: Option<(i32, i32)>) -> Option<Pill> {
    let wide: Vec<u16> = text.encode_utf16().collect();
    let ext = text_size(text, font);
    let (w, h) = fixed.unwrap_or((ext.cx + 2 * pad.0, ext.cy + 2 * pad.1));

    let mut dib = Dib::new(w, h)?;
    unsafe {
        SelectObject(dib.dc, font.0.into());
        SetBkMode(dib.dc, TRANSPARENT);
        SetTextColor(dib.dc, COLORREF(0x00FF_FFFF));
        let _ = TextOutW(dib.dc, (w - ext.cx) / 2, (h - ext.cy) / 2, &wide);
        let _ = GdiFlush();
    }

    let r = radius.min(w.min(h) / 2) as f32;
    let coverage = dib.pixels();
    let mut pixels = Vec::with_capacity(coverage.len());
    for y in 0..h {
        for x in 0..w {
            let d = rounded_rect_sdf(x as f32 + 0.5, y as f32 + 0.5, w as f32, h as f32, r);
            let text = ((coverage[(y * w + x) as usize] >> 8) & 0xFF) as f32 / 255.0;
            pixels.push(pill_pixel(d, text));
        }
    }
    Some(Pill { width: w, height: h, pixels })
}

/// Alpha-composites premultiplied `s` over premultiplied `d`.
fn over(s: u32, d: u32) -> u32 {
    let inv = 255 - (s >> 24);
    let ch = |shift: u32| {
        let c = ((s >> shift) & 0xFF) + (((d >> shift) & 0xFF) * inv + 127) / 255;
        c.min(255) << shift
    };
    ch(24) | ch(16) | ch(8) | ch(0)
}

/// Composites `pill` at (`x`, `y`) onto `buf` (row stride `stride`), limited to `clip`.
pub fn blend_pill(buf: &mut [u32], stride: i32, clip: Rect, x: i32, y: i32, pill: &Pill) {
    let area = Rect::new(x, y, x + pill.width, y + pill.height).intersect(clip);
    for row in area.top..area.bottom {
        for col in area.left..area.right {
            let s = pill.pixels[((row - y) * pill.width + (col - x)) as usize];
            let d = &mut buf[(row * stride + col) as usize];
            *d = over(s, *d);
        }
    }
}

/// Composites a premultiplied `color` over the part of `rect` inside `clip`.
pub fn fill_rect(buf: &mut [u32], stride: i32, clip: Rect, rect: Rect, color: u32) {
    let area = rect.intersect(clip);
    if area.is_empty() {
        return;
    }
    for row in area.top..area.bottom {
        for px in &mut buf[(row * stride + area.left) as usize..(row * stride + area.right) as usize] {
            *px = over(color, *px);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Scale(ExportScale),
    Record,
    Cancel,
}

/// Toolbar pill; button rectangles are relative to the pill's top-left corner.
pub struct Toolbar {
    pub pill: Pill,
    /// In the order of [`ExportScale::ALL`]; adjacent, so there is no dead zone between segments.
    pub scales: [Rect; 3],
    pub record: Rect,
    pub cancel: Rect,
}

fn scale_label(scale: ExportScale) -> &'static str {
    match scale {
        ExportScale::Half => "0.5×",
        ExportScale::ThreeQuarters => "0.75×",
        ExportScale::Full => "1×",
    }
}

/// Premultiplied color from straight RGB and alpha.
fn premul(rgb: u32, alpha: f32) -> u32 {
    let ch = |shift: u32| ((((rgb >> shift) & 0xFF) as f32 * alpha).round() as u32) << shift;
    ((alpha * 255.0).round() as u32) << 24 | ch(16) | ch(8) | ch(0)
}

/// Distance from (`x`, `y`) to the segment `a`-`b`.
fn segment_distance(x: f32, y: f32, a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let t = (((x - a.0) * dx + (y - a.1) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    (x - a.0 - t * dx).hypot(y - a.1 - t * dy)
}

/// Renders the compact toolbar: `dims` | export scale segments | red-dot "Record" button | "✕" button. The
/// selected scale gets a filled segment, the hovered button a soft highlight. Text goes through GDI like
/// [`render_pill`]; dot and cross are drawn analytically.
pub fn render_toolbar(
    dims: &str,
    font: &Font,
    selected: ExportScale,
    hover: Option<Button>,
    dpi: u32,
) -> Option<Toolbar> {
    const RECORD: &str = "Record";
    let s = |v: f32| dp(v, dpi);
    let k = dpi as f32 / 96.0;
    let (dims_ext, record_ext) = (text_size(dims, font), text_size(RECORD, font));
    let labels = ExportScale::ALL.map(scale_label);
    let label_exts = labels.map(|l| text_size(l, font));

    let h = s(34.0);
    let (btn_h, dot) = (s(26.0), s(8.0));
    let btn_y = (h - btn_h) / 2;
    let divider_x = s(14.0) + dims_ext.cx + s(12.0);
    // Segmented control: a track holding segments sized to their labels, inset a little from its edge.
    let inset = s(2.0);
    let seg_ws = label_exts.map(|e| (e.cx + 2 * s(5.0)).max(s(22.0)));
    let track_x = divider_x + s(1.0).max(1) + s(6.0);
    let track = Rect::new(track_x, btn_y, track_x + seg_ws.iter().sum::<i32>() + 2 * inset, btn_y + btn_h);
    let scales = [0, 1, 2].map(|i| {
        let left = track.left + inset + seg_ws[..i].iter().sum::<i32>();
        Rect::new(left, track.top + inset, left + seg_ws[i], track.bottom - inset)
    });
    let divider2_x = track.right + s(6.0);
    let rec_x = divider2_x + s(1.0).max(1) + s(6.0);
    let rec_text_x = rec_x + s(8.0) + dot + s(5.0);
    let rec_w = rec_text_x - rec_x + record_ext.cx + s(9.0);
    let can_x = rec_x + rec_w + s(2.0);
    let can_w = s(24.0);
    let w = can_x + can_w + s(5.0);
    let record = Rect::new(rec_x, btn_y, rec_x + rec_w, btn_y + btn_h);
    let cancel = Rect::new(can_x, btn_y, can_x + can_w, btn_y + btn_h);

    let mut dib = Dib::new(w, h)?;
    unsafe {
        SelectObject(dib.dc, font.0.into());
        SetBkMode(dib.dc, TRANSPARENT);
        SetTextColor(dib.dc, COLORREF(0x00FF_FFFF));
        let draw = |text: &str, x: i32, ext: SIZE| {
            let wide: Vec<u16> = text.encode_utf16().collect();
            let _ = TextOutW(dib.dc, x, (h - ext.cy) / 2, &wide);
        };
        draw(dims, s(14.0), dims_ext);
        draw(RECORD, rec_text_x, record_ext);
        for ((text, ext), seg) in labels.into_iter().zip(label_exts).zip(scales) {
            draw(text, seg.left + (seg.right - seg.left - ext.cx) / 2, ext);
        }
        let _ = GdiFlush();
    }

    let radius = s(10.0).min(h / 2) as f32;
    let btn_radius = s(8.0) as f32;
    let seg_radius = (btn_radius - inset as f32).max(1.0);
    let (div_top, div_bottom) = ((h - s(16.0)) / 2, (h + s(16.0)) / 2);
    let dividers = [divider_x, divider2_x].map(|x| Rect::new(x, div_top, x + s(1.0).max(1), div_bottom));
    let dot_c = ((rec_x + s(8.0)) as f32 + dot as f32 / 2.0, h as f32 / 2.0);
    let cross_c = ((can_x + can_w / 2) as f32, h as f32 / 2.0);
    let (arm, thickness) = (4.0 * k, 1.5 * k);
    let highlight = premul(0xFFFFFF, 0.16);
    let track_fill = premul(0xFFFFFF, 0.07);
    let hovered = match hover {
        Some(Button::Record) => Some(record),
        Some(Button::Cancel) => Some(cancel),
        Some(Button::Scale(_)) | None => None,
    };
    // Unselected, unhovered segment labels are dimmed.
    let dimmed: Vec<Rect> = ExportScale::ALL
        .iter()
        .zip(scales)
        .filter(|&(&sc, _)| sc != selected && hover != Some(Button::Scale(sc)))
        .map(|(_, r)| r)
        .collect();
    // Anti-aliased coverage of a rounded rectangle at (`fx`, `fy`).
    let rect_cov = |fx: f32, fy: f32, r: Rect, radius: f32| {
        let d = rounded_rect_sdf(fx - r.left as f32, fy - r.top as f32, r.width() as f32, r.height() as f32, radius);
        (0.5 - d).clamp(0.0, 1.0)
    };

    let coverage = dib.pixels();
    let mut pixels = Vec::with_capacity(coverage.len());
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let mut text = ((coverage[(y * w + x) as usize] >> 8) & 0xFF) as f32 / 255.0;
            if dimmed.iter().any(|r| r.contains(POINT { x, y })) {
                text *= 0.6;
            }
            let mut px = pill_pixel(rounded_rect_sdf(fx, fy, w as f32, h as f32, radius), text);

            px = over(scale(track_fill, rect_cov(fx, fy, track, btn_radius)), px);
            for (&sc, &seg) in ExportScale::ALL.iter().zip(&scales) {
                let hovered_seg = hover == Some(Button::Scale(sc));
                let alpha = match (sc == selected, hovered_seg) {
                    (true, true) => 0.34,
                    (true, false) => 0.28,
                    (false, true) => 0.16,
                    (false, false) => continue,
                };
                px = over(scale(premul(0xFFFFFF, alpha), rect_cov(fx, fy, seg, seg_radius)), px);
            }
            if let Some(b) = hovered {
                px = over(scale(highlight, rect_cov(fx, fy, b, btn_radius)), px);
            }
            if dividers.iter().any(|d| d.contains(POINT { x, y })) {
                px = over(premul(0xFFFFFF, 0.18), px);
            }
            let dot_cov = (0.5 - ((fx - dot_c.0).hypot(fy - dot_c.1) - dot as f32 / 2.0)).clamp(0.0, 1.0);
            if dot_cov > 0.0 {
                px = over(premul(0xFF453A, dot_cov), px);
            }
            let (c, a) = (cross_c, arm);
            let cross_d = segment_distance(fx, fy, (c.0 - a, c.1 - a), (c.0 + a, c.1 + a))
                .min(segment_distance(fx, fy, (c.0 - a, c.1 + a), (c.0 + a, c.1 - a)));
            let cross_cov = (0.5 + thickness / 2.0 - cross_d).clamp(0.0, 1.0);
            if cross_cov > 0.0 {
                px = over(premul(0xFFFFFF, cross_cov * 0.9), px);
            }
            pixels.push(px);
        }
    }
    Some(Toolbar { pill: Pill { width: w, height: h, pixels }, scales, record, cancel })
}

/// Scales a premultiplied color by `f` (0..=1).
fn scale(c: u32, f: f32) -> u32 {
    let ch = |shift: u32| (((c >> shift) & 0xFF) as f32 * f).round() as u32;
    ch(24) << 24 | ch(16) << 16 | ch(8) << 8 | ch(0)
}
