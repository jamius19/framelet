/*
 * Author: Jamius Siam
 * Since: 31/10/2026
 */

//! Manual demo: select a region, then run a 3 second countdown over it.
//! `--retry`: a canceled selection reopens the overlay (used to check for resource leaks).
//! `--initial x,y,w,h`: open with that region selected (every reopen too). `--no-countdown`: stop after selecting.

use framelet_core::Region;
use framelet_overlay::{countdown, select_region};
use log::{LevelFilter, Log, Metadata, Record};
use windows::Win32::System::Threading::{GR_GDIOBJECTS, GR_USEROBJECTS, GetCurrentProcess, GetGuiResources};
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};

struct StderrLogger;

impl Log for StderrLogger {
    fn enabled(&self, _: &Metadata) -> bool {
        true
    }

    fn log(&self, record: &Record) {
        eprintln!("[{}] {}", record.level(), record.args());
    }

    fn flush(&self) {}
}

fn gui_objects() -> String {
    let (gdi, user) = unsafe {
        let process = GetCurrentProcess();
        (GetGuiResources(process, GR_GDIOBJECTS), GetGuiResources(process, GR_USEROBJECTS))
    };
    format!("gdi={gdi} user={user}")
}

fn parse_region(text: &str) -> Option<Region> {
    let n: Vec<i32> = text.split(',').filter_map(|p| p.trim().parse().ok()).collect();
    match n[..] {
        [x, y, w, h] => Some(Region { x, y, width: w as u32, height: h as u32 }),
        _ => None,
    }
}

fn main() {
    let _ = log::set_logger(&StderrLogger);
    log::set_max_level(LevelFilter::Debug);
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }.expect("DPI awareness");

    let args: Vec<String> = std::env::args().collect();
    let retry = args.iter().any(|a| a == "--retry");
    let initial = args.iter().position(|a| a == "--initial").and_then(|i| parse_region(args.get(i + 1)?));
    println!("before: {}", gui_objects());
    let mut region = select_region(initial);
    println!("select_region -> {region:?} ({})", gui_objects());
    while retry && region.is_none() {
        region = select_region(initial);
        println!("select_region -> {region:?} ({})", gui_objects());
    }
    if let Some(region) = region.filter(|_| !args.iter().any(|a| a == "--no-countdown")) {
        println!("countdown -> {} ({})", countdown(region, 3), gui_objects());
    }
}
