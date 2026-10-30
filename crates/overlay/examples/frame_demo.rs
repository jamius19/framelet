/*
 * Author: Jamius Siam
 * Since: 31/10/2026
 */

//! Manual demo: shows the recording frame for a few seconds.
//! Usage: `frame_demo [x y w h] [--dim] [--secs N] [--repeat N] [--capturable]` (default region 300 200 800 400).
//! `--repeat N` shows and drops the frame N extra times first, to check that the object counts stay flat.
//! `--capturable` skips `WDA_EXCLUDEFROMCAPTURE` so a screenshot can see the frame.
//! Prints GDI / USER object counts before, while shown and after drop (leak check).

use framelet_core::Region;
use framelet_overlay::RecordingFrame;
use std::time::{Duration, Instant};
use windows::Win32::System::Threading::{GR_GDIOBJECTS, GR_USEROBJECTS, GetCurrentProcess, GetGuiResources};
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MSG, MsgWaitForMultipleObjects, PM_REMOVE, PeekMessageW, QS_ALLINPUT, TranslateMessage,
};

fn gui_objects() -> String {
    let (gdi, user) = unsafe {
        let process = GetCurrentProcess();
        (GetGuiResources(process, GR_GDIOBJECTS), GetGuiResources(process, GR_USEROBJECTS))
    };
    format!("gdi={gdi} user={user}")
}

/// Pumps messages for `secs` seconds.
fn pump_for(secs: u64) {
    let end = Instant::now() + Duration::from_secs(secs);
    let mut msg = MSG::default();
    while let Some(left) = end.checked_duration_since(Instant::now()) {
        unsafe {
            MsgWaitForMultipleObjects(None, false, left.as_millis() as u32, QS_ALLINPUT);
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

fn main() {
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }.expect("DPI awareness");

    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().any(|a| a == name);
    let numbers: Vec<i32> = args.iter().filter_map(|a| a.parse().ok()).collect();
    let secs = args.iter().position(|a| a == "--secs").and_then(|i| args.get(i + 1)?.parse().ok()).unwrap_or(5);
    let region = match numbers[..] {
        [x, y, w, h, ..] => Region { x, y, width: w as u32, height: h as u32 },
        _ => Region { x: 300, y: 200, width: 800, height: 400 },
    };
    let (dim, capturable) = (flag("--dim"), flag("--capturable"));

    let repeat: u32 = args.iter().position(|a| a == "--repeat").and_then(|i| args.get(i + 1)?.parse().ok()).unwrap_or(0);

    println!("before: {}", gui_objects());
    let show = if capturable { RecordingFrame::show_capturable } else { RecordingFrame::show };
    for i in 1..=repeat {
        drop(show(region, dim));
        println!("repeat {i}: {}", gui_objects());
    }
    let frame = show(region, dim);
    println!("shown {region:?} dim={dim} capturable={capturable} -> {} ({})", frame.is_some(), gui_objects());
    pump_for(secs);
    drop(frame);
    pump_for(1);
    println!("after drop: {}", gui_objects());
}
