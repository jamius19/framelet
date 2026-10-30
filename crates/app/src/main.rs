/*
 * Author: Jamius Siam
 * Since: 31/10/2026
 */

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod controller;
mod hotkey;
mod notify;
mod save;
mod tray;

use framelet_core::{ipc, logging};
use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, GetMessageW, MSG, TranslateMessage};
use windows::core::HSTRING;

fn main() {
    logging::init("framelet");
    std::panic::set_hook(Box::new(|info| log::error!("panic: {info}")));

    // Debug-only: `--demo-controller` shows the controller alone; `--demo-controller=visible` also lets it
    // be captured (screenshots).
    #[cfg(debug_assertions)]
    let demo = std::env::args().find_map(|a| match a.as_str() {
        "--demo-controller" => Some(false),
        "--demo-controller=visible" => Some(true),
        _ => None,
    });
    #[cfg(not(debug_assertions))]
    let demo = None;

    // SAFETY: the mutex handle is intentionally kept until the process exits.
    let _single_instance = unsafe {
        let mutex = CreateMutexW(None, true, &HSTRING::from(ipc::TRAY_MUTEX));
        if demo.is_none() && (mutex.is_err() || GetLastError() == ERROR_ALREADY_EXISTS) {
            log::info!("already running, activating the existing instance");
            app::wait_for_existing_instance_and_activate();
            return;
        }
        mutex
    };

    log::info!("framelet {} starting", env!("CARGO_PKG_VERSION"));
    if let Err(e) = app::init(demo) {
        log::error!("startup failed: {e:#}");
        return;
    }

    let mut msg = MSG::default();
    // SAFETY: standard message loop on the thread that created the windows.
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    app::shutdown();
    log::info!("exited");
}
