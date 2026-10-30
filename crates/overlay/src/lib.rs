/*
 * Author: Jamius Siam
 * Since: 31/10/2026
 */

//! Region selection overlay and pre-recording countdown. Both run a nested modal message loop on
//! the calling (UI) thread, like `DialogBox`. [`RecordingFrame`] is non-modal. See ARCHITECTURE.md.

mod countdown;
mod frame;
mod gfx;
mod select;
mod win;

pub use frame::RecordingFrame;

use framelet_core::Region;

/// Shows the fullscreen selection overlay over the whole virtual desktop and runs a nested
/// modal message loop on the calling (UI) thread until the user finishes.
/// The selection stays editable (draw / move / resize) until confirmed (Enter, double click inside, Record
/// button); the returned region is in physical pixels. `None` if canceled (Esc / right click / ✕ button).
/// `initial`, clamped to the virtual desktop, opens already selected; if it does not (sufficiently)
/// intersect the desktop it is ignored.
/// All overlay windows and GDI resources are destroyed before returning.
pub fn select_region(initial: Option<Region>) -> Option<Region> {
    select::run(initial)
}

/// Shows a countdown (`seconds`, `seconds-1`, … 1) centered over `region`, nested modal loop.
/// Returns `false` if the user pressed Esc. `seconds == 0` returns `true` immediately.
pub fn countdown(region: Region, seconds: u32) -> bool {
    if seconds == 0 {
        return true;
    }
    countdown::run(region, seconds)
}
