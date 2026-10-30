/*
 * Author: Jamius Siam
 * Since: 31/10/2026
 */

//! The tray window, application state and the recording state machine
//! (Idle -> Selecting -> Countdown -> Recording -> Processing -> Idle).
//!
//! Selection, countdown, dialogs and menus run nested message loops on this thread, so the state lives
//! in a `RefCell` that is only ever borrowed briefly (never across such a call).

use std::cell::RefCell;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;
use std::thread;
use std::time::Instant;

use anyhow::{Result, anyhow};
use framelet_core::caps::EncoderCaps;
use framelet_core::hotkey::Hotkey;
use framelet_core::{Region, Settings, ipc};
use framelet_overlay::RecordingFrame;
use framelet_recorder::{FinishedRecording, RecorderEvent, RecordingSession};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{NIN_BALLOONUSERCLICK, NIN_SELECT};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, KillTimer, PostMessageW, PostQuitMessage, RegisterClassExW,
    RegisterWindowMessageW, SetTimer, WM_APP, WM_CLOSE, WM_CONTEXTMENU, WM_DESTROY, WM_HOTKEY, WM_TIMER, WNDCLASSEXW,
    WS_EX_TOOLWINDOW, WS_POPUP,
};
use windows::core::{HSTRING, w};

use crate::controller::{Controller, format_elapsed};
use crate::save::{self, ExitChoice};
use crate::tray::{self, Cmd};
use crate::{hotkey, notify};

/// Tray icon callback (NOTIFYICON_VERSION_4).
pub const WM_TRAY: u32 = WM_APP + 1;
/// `Box<RecorderEvent>` from the recording worker.
const WM_RECORDER: u32 = WM_APP + 2;
/// `Box<WorkerResult>` from a capability-detection worker.
const WM_WORKER: u32 = WM_APP + 3;
/// Start the selection flow.
const WM_BEGIN: u32 = WM_APP + 4;
/// Controller buttons.
pub const WM_CTRL_STOP: u32 = WM_APP + 5;
pub const WM_CTRL_CANCEL: u32 = WM_APP + 6;

const TIMER_ELAPSED: usize = 1;
/// NIN_SELECT | NINF_KEY: icon activated from the keyboard.
const NIN_KEYSELECT: u32 = NIN_SELECT + 1;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    Idle,
    Selecting,
    Countdown,
    Recording,
    Processing,
}

struct WorkerResult {
    caps: EncoderCaps,
    recovered: Vec<PathBuf>,
}

/// Registered window messages (`ipc`) and `TaskbarCreated`.
struct Msgs {
    taskbar_created: u32,
    settings_changed: u32,
    suspend: u32,
    resume: u32,
    activate: u32,
}

static MSGS: OnceLock<Msgs> = OnceLock::new();

struct App {
    hwnd: HWND,
    settings: Settings,
    caps: Option<EncoderCaps>,
    state: State,
    session: Option<RecordingSession>,
    started: Option<Instant>,
    controller: Option<Controller>,
    /// Red border (and optional dim) around the region; lives exactly as long as the controller would.
    frame: Option<RecordingFrame>,
    /// The hotkey string that is currently registered, if any.
    hotkey: Option<String>,
    /// The hotkey string of the last registration attempt; failures are only announced when it changes.
    hotkey_tried: Option<String>,
    exit_after: bool,
    #[cfg(debug_assertions)]
    demo: bool,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Runs `f` on the app state. `None` if the state does not exist or is borrowed (re-entrancy).
fn with<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|cell| {
        let mut guard = cell.try_borrow_mut().ok()?;
        guard.as_mut().map(f)
    })
}

fn msg_id(name: &str) -> u32 {
    // SAFETY: plain Win32 call with a valid string.
    unsafe { RegisterWindowMessageW(&HSTRING::from(name)) }
}

pub fn wait_for_existing_instance_and_activate() {
    let activate = msg_id(ipc::MSG_ACTIVATE);
    for _ in 0..20 {
        // SAFETY: plain Win32 calls.
        unsafe {
            use windows::Win32::UI::WindowsAndMessaging::FindWindowW;
            if let Ok(hwnd) = FindWindowW(&HSTRING::from(ipc::TRAY_WINDOW_CLASS), None) {
                let _ = PostMessageW(Some(hwnd), activate, WPARAM(0), LPARAM(0));
                return;
            }
        }
        thread::sleep(std::time::Duration::from_millis(100));
    }
    log::warn!("another instance holds the mutex but has no window");
}

/// Creates the hidden tray window and the application state. `demo` is debug-only.
pub fn init(demo: Option<bool>) -> Result<HWND> {
    let _ = MSGS.set(Msgs {
        taskbar_created: msg_id("TaskbarCreated"),
        settings_changed: msg_id(ipc::MSG_SETTINGS_CHANGED),
        suspend: msg_id(ipc::MSG_SUSPEND_HOTKEYS),
        resume: msg_id(ipc::MSG_RESUME_HOTKEYS),
        activate: msg_id(ipc::MSG_ACTIVATE),
    });
    let settings = Settings::load();
    tray::apply_menu_theme(settings.general.theme);

    // SAFETY: standard window class registration and creation on this thread.
    let hwnd = unsafe {
        let hinst = GetModuleHandleW(None)?;
        let class = HSTRING::from(ipc::TRAY_WINDOW_CLASS);
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            lpszClassName: windows::core::PCWSTR(class.as_ptr()),
            ..Default::default()
        };
        if RegisterClassExW(&wc) == 0 {
            return Err(anyhow!("cannot register the tray window class"));
        }
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            &class,
            w!("Framelet"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(hinst.into()),
            None,
        )?
    };

    let app = App {
        hwnd,
        settings: settings.clone(),
        caps: None,
        state: State::Idle,
        session: None,
        started: None,
        controller: None,
        frame: None,
        hotkey: None,
        hotkey_tried: None,
        exit_after: false,
        #[cfg(debug_assertions)]
        demo: demo.is_some(),
    };
    APP.with_borrow_mut(|a| *a = Some(app));

    tray::add(hwnd, false, "Framelet");
    #[cfg(debug_assertions)]
    if let Some(capturable) = demo {
        demo_controller(capturable);
        return Ok(hwnd);
    }
    let _ = demo;
    let spec = settings.general.record_hotkey.clone();
    with(|a| a.sync_hotkey(&spec));
    spawn_worker(hwnd, settings, true);
    Ok(hwnd)
}

/// Drops the state (after the message loop ended).
pub fn shutdown() {
    APP.with_borrow_mut(|a| *a = None);
}

#[cfg(debug_assertions)]
fn demo_controller(capturable: bool) {
    let region = Region { x: 300, y: 200, width: 800, height: 400 };
    with(|a| {
        a.state = State::Recording;
        a.started = Some(Instant::now());
        a.controller = Controller::create(a.hwnd, region, true, capturable);
        // SAFETY: plain Win32 call.
        unsafe { SetTimer(Some(a.hwnd), TIMER_ELAPSED, 1000, None) };
    });
}

impl App {
    fn sync_hotkey(&mut self, spec: &str) {
        if self.hotkey.as_deref() == Some(spec) {
            return;
        }
        if self.hotkey.take().is_some() {
            hotkey::unregister(self.hwnd);
        }
        let changed = self.hotkey_tried.as_deref() != Some(spec);
        self.hotkey_tried = Some(spec.to_string());
        if hotkey::register(self.hwnd, spec, changed) {
            self.hotkey = Some(spec.to_string());
        }
    }

    /// Capture is over (stopped, canceled or auto-stopped): hide the controller, wait for the result.
    fn enter_processing(&mut self) {
        if self.state == State::Processing {
            return;
        }
        self.state = State::Processing;
        self.controller = None;
        self.frame = None;
        // SAFETY: plain Win32 call.
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_ELAPSED);
        }
        tray::set_icon_and_tip(false, "Framelet — Processing…");
    }

    fn back_to_idle(&mut self) {
        self.state = State::Idle;
        self.session = None;
        self.started = None;
        tray::set_icon_and_tip(false, "Framelet");
        if self.exit_after {
            // SAFETY: our own window.
            unsafe {
                let _ = DestroyWindow(self.hwnd);
            }
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_TRAY => on_tray(wparam, lparam),
        WM_HOTKEY => on_hotkey(hwnd),
        WM_TIMER => on_timer(),
        WM_BEGIN => begin_recording(hwnd),
        WM_RECORDER => on_recorder(hwnd, lparam),
        WM_WORKER => on_worker(lparam),
        WM_CTRL_STOP => stop_recording(),
        WM_CTRL_CANCEL => cancel_recording(),
        WM_CLOSE => request_exit(),
        WM_DESTROY => {
            on_destroy(hwnd);
            // SAFETY: plain Win32 call.
            unsafe { PostQuitMessage(0) };
        }
        _ => {
            if let Some(m) = MSGS.get() {
                if msg == m.taskbar_created {
                    let (rec, tip) = with(|a| (a.state == State::Recording, tooltip(a))).unwrap_or_default();
                    tray::add(hwnd, rec, &tip);
                    return LRESULT(0);
                } else if msg == m.settings_changed {
                    on_settings_changed(hwnd);
                    return LRESULT(0);
                } else if msg == m.suspend {
                    with(|a| {
                        if a.hotkey.take().is_some() {
                            hotkey::unregister(a.hwnd);
                        }
                    });
                    return LRESULT(0);
                } else if msg == m.resume {
                    let spec = Settings::load().general.record_hotkey;
                    with(|a| a.sync_hotkey(&spec));
                    return LRESULT(0);
                } else if msg == m.activate {
                    open_settings();
                    return LRESULT(0);
                }
            }
            // SAFETY: default processing with the arguments we were given.
            return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
        }
    }
    LRESULT(0)
}

fn tooltip(a: &App) -> String {
    match (a.state, a.started) {
        (State::Recording, Some(t)) => format!("Framelet — Recording {}", format_elapsed(t.elapsed().as_secs())),
        (State::Processing, _) => "Framelet — Processing…".into(),
        _ => "Framelet".into(),
    }
}

fn on_destroy(hwnd: HWND) {
    log::info!("shutting down");
    tray::remove();
    hotkey::unregister(hwnd);
    with(|a| {
        a.controller = None;
        a.frame = None;
        a.session = None;
    });
    // SAFETY: our own window.
    unsafe {
        let _ = KillTimer(Some(hwnd), TIMER_ELAPSED);
    }
}

fn on_tray(wparam: WPARAM, lparam: LPARAM) {
    let event = lparam.0 as u32 & 0xFFFF;
    let Some(state) = with(|a| a.state) else { return };
    match event {
        NIN_SELECT | NIN_KEYSELECT => match state {
            State::Idle => post_to_self(WM_BEGIN),
            State::Recording => stop_recording(),
            _ => {}
        },
        WM_CONTEXTMENU => {
            if matches!(state, State::Selecting | State::Countdown) {
                return;
            }
            let pt = POINT { x: wparam.0 as i16 as i32, y: (wparam.0 >> 16) as i16 as i32 };
            let hotkey = with(|a| Hotkey::parse(&a.settings.general.record_hotkey).map(|h| h.to_display_string()))
                .flatten()
                .unwrap_or_default();
            // The menu is a nested loop; state may change while it is open, so re-check on dispatch.
            match tray::show_menu(pt, state == State::Idle, state == State::Recording, &hotkey) {
                Some(Cmd::Start) => post_to_self(WM_BEGIN),
                Some(Cmd::Stop) => stop_recording(),
                Some(Cmd::Settings) => open_settings(),
                Some(Cmd::Folder) => open_recordings_folder(),
                Some(Cmd::Exit) => request_exit(),
                None => {}
            }
        }
        NIN_BALLOONUSERCLICK => {
            if let Some(path) = notify::take_click_target() {
                save::reveal(&path);
            }
        }
        _ => {}
    }
}

fn post_to_self(msg: u32) {
    if let Some(hwnd) = with(|a| a.hwnd) {
        // SAFETY: our own window.
        unsafe {
            let _ = PostMessageW(Some(hwnd), msg, WPARAM(0), LPARAM(0));
        }
    }
}

fn on_hotkey(_hwnd: HWND) {
    let Some((state, stop_with_hotkey)) = with(|a| (a.state, a.settings.recording.stop_with_hotkey)) else {
        return;
    };
    match state {
        State::Idle => begin_recording(_hwnd),
        State::Recording if stop_with_hotkey => stop_recording(),
        _ => {}
    }
}

fn on_timer() {
    with(|a| {
        if a.state != State::Recording {
            return;
        }
        let secs = a.started.map(|t| t.elapsed().as_secs()).unwrap_or(0);
        if let Some(c) = &a.controller {
            c.set_elapsed(secs);
        }
        tray::set_tip(&tooltip(a));
    });
}

fn set_state(state: State) {
    with(|a| a.state = state);
}

/// Selection -> countdown -> start. Runs nested loops, so no borrow is held across them.
fn begin_recording(hwnd: HWND) {
    let Some(settings) = with(|a| {
        (a.state == State::Idle).then(|| {
            a.state = State::Selecting;
            a.settings.clone()
        })
    })
    .flatten() else {
        return;
    };

    let Some(region) = selected_region() else {
        return set_state(State::Idle);
    };
    set_state(State::Countdown);
    if !framelet_overlay::countdown(region, settings.recording.countdown_secs) {
        return set_state(State::Idle);
    }

    // Re-detect when FFmpeg was missing or has gone away, so installing it needs no restart.
    let cached = with(|a| a.caps.clone()).flatten().filter(|c| c.ffmpeg_path.as_deref().is_some_and(|p| p.is_file()));
    let caps = match cached {
        Some(caps) => caps,
        None => {
            let caps = framelet_recorder::load_or_detect_caps(&settings, false);
            with(|a| a.caps = Some(caps.clone()));
            caps
        }
    };
    if caps.ffmpeg_path.is_none() {
        notify::error("FFmpeg was not found. Put ffmpeg.exe next to framelet.exe or set its path in Settings.");
        return set_state(State::Idle);
    }

    let raw = hwnd.0 as isize;
    match RecordingSession::start(region, &settings, &caps, move |event| post_event(raw, event)) {
        Ok(session) => {
            if let Err(e) = region.save_last() {
                log::warn!("could not save the last region: {e}");
            }
            start_recording_ui(session, region, &settings);
        }
        Err(e) => {
            notify::error(&format!("Could not start recording: {e}"));
            set_state(State::Idle);
        }
    }
}

/// Debug builds accept `FRAMELET_TEST_REGION=x,y,w,h` to skip the interactive overlay (automated runs).
fn selected_region() -> Option<Region> {
    #[cfg(debug_assertions)]
    if let Ok(v) = std::env::var("FRAMELET_TEST_REGION") {
        let n: Vec<i32> = v.split(',').filter_map(|p| p.trim().parse().ok()).collect();
        if let [x, y, w, h] = n[..] {
            return Some(Region { x, y, width: w as u32, height: h as u32 });
        }
    }
    framelet_overlay::select_region(Region::load_last())
}

fn post_event(hwnd: isize, event: RecorderEvent) {
    let ptr = Box::into_raw(Box::new(event));
    // SAFETY: on failure the box is reclaimed; on success the window procedure owns it.
    unsafe {
        if PostMessageW(Some(HWND(hwnd as *mut _)), WM_RECORDER, WPARAM(0), LPARAM(ptr as isize)).is_err() {
            drop(Box::from_raw(ptr));
        }
    }
}

fn start_recording_ui(session: RecordingSession, region: Region, settings: &Settings) {
    with(|a| {
        a.session = Some(session);
        a.state = State::Recording;
        a.started = Some(Instant::now());
        // Before the controller, so the controller is created above the frame windows.
        a.frame = RecordingFrame::show(region, settings.recording.dim_while_recording);
        if settings.recording.show_controls {
            a.controller = Controller::create(a.hwnd, region, settings.recording.show_elapsed_time, false);
        }
        // SAFETY: our own window.
        unsafe { SetTimer(Some(a.hwnd), TIMER_ELAPSED, 1000, None) };
        tray::set_icon_and_tip(true, "Framelet — Recording 0:00");
    });
}

fn stop_recording() {
    with(|a| {
        if a.state != State::Recording {
            return;
        }
        #[cfg(debug_assertions)]
        if a.demo {
            // SAFETY: our own window.
            return unsafe {
                let _ = DestroyWindow(a.hwnd);
            };
        }
        if let Some(s) = &a.session {
            s.stop();
        }
        a.enter_processing();
    });
}

fn cancel_recording() {
    with(|a| {
        if a.state != State::Recording {
            return;
        }
        #[cfg(debug_assertions)]
        if a.demo {
            // SAFETY: our own window.
            return unsafe {
                let _ = DestroyWindow(a.hwnd);
            };
        }
        if let Some(s) = &a.session {
            s.cancel();
        }
        a.enter_processing();
    });
}

fn on_recorder(hwnd: HWND, lparam: LPARAM) {
    // SAFETY: the pointer comes from `post_event`; each message is delivered once.
    let event = unsafe { *Box::from_raw(lparam.0 as *mut RecorderEvent) };
    log::info!("recorder event: {event:?}");
    match event {
        RecorderEvent::Processing => {
            with(|a| a.enter_processing());
        }
        RecorderEvent::Canceled => {
            with(|a| a.back_to_idle());
        }
        RecorderEvent::Finished(rec) => {
            let Some(settings) = with(|a| {
                a.enter_processing();
                a.settings.clone()
            }) else {
                return;
            };
            on_finished(hwnd, &settings, &rec);
            with(|a| a.back_to_idle());
        }
        RecorderEvent::Failed { message, partial } => {
            let Some(settings) = with(|a| {
                a.enter_processing();
                a.settings.clone()
            }) else {
                return;
            };
            on_failed(&settings, &message, partial);
            with(|a| a.back_to_idle());
        }
    }
}

fn file_name(p: &std::path::Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn on_finished(hwnd: HWND, settings: &Settings, rec: &FinishedRecording) {
    match save::save_finished(hwnd, settings, rec) {
        Ok(saved) => {
            let dir = saved.path.parent().map(|p| p.display().to_string()).unwrap_or_default();
            log::info!("saved {}", saved.path.display());
            if saved.dialog_canceled {
                notify::info(&format!("Save canceled — recording kept in {dir}"), Some(saved.path.clone()));
            } else if settings.general.notifications {
                let mut text = file_name(&saved.path);
                if let Some(w) = &rec.warning {
                    text = format!("{text}\n{w}");
                }
                notify::info(&format!("Recording saved\n{text}"), Some(saved.path.clone()));
            }
            if settings.general.open_folder_after_save {
                save::reveal(&saved.path);
            }
        }
        Err(e) => notify::error(&format!("Could not save the recording: {e:#}. It remains at {}", rec.path.display())),
    }
}

fn on_failed(settings: &Settings, message: &str, partial: Option<PathBuf>) {
    let Some(partial) = partial else {
        return notify::error(&format!("Recording failed: {message}"));
    };
    match save::save_partial(settings, &partial) {
        Ok(path) => {
            log::error!("recording failed: {message}");
            notify::error_with_file(&format!("Recording failed: {message}\nPartial recording saved."), path);
        }
        Err(e) => notify::error(&format!("Recording failed: {message}. Saving the partial file failed: {e:#}")),
    }
}

fn on_worker(lparam: LPARAM) {
    // SAFETY: the pointer comes from `spawn_worker`; each message is delivered once.
    let result = unsafe { *Box::from_raw(lparam.0 as *mut WorkerResult) };
    log::info!("capabilities: {}", result.caps.summary());
    with(|a| a.caps = Some(result.caps));
    if !result.recovered.is_empty() {
        let n = result.recovered.len();
        notify::info(&format!("Recovered {n} recording(s)"), result.recovered.first().cloned());
    }
}

/// Short-lived worker: capability detection and, at startup, orphan recovery.
fn spawn_worker(hwnd: HWND, settings: Settings, recover: bool) {
    let raw = hwnd.0 as isize;
    let spawned = thread::Builder::new().name("startup".into()).spawn(move || {
        let caps = framelet_recorder::load_or_detect_caps(&settings, false);
        let recovered = if recover { framelet_recorder::recover_orphans(&settings, &caps) } else { Vec::new() };
        let ptr = Box::into_raw(Box::new(WorkerResult { caps, recovered }));
        // SAFETY: on failure the box is reclaimed; on success the window procedure owns it.
        unsafe {
            if PostMessageW(Some(HWND(raw as *mut _)), WM_WORKER, WPARAM(0), LPARAM(ptr as isize)).is_err() {
                drop(Box::from_raw(ptr));
            }
        }
    });
    if let Err(e) = spawned {
        log::error!("cannot start worker thread: {e}");
    }
}

fn on_settings_changed(hwnd: HWND) {
    let new = Settings::load();
    tray::apply_menu_theme(new.general.theme);
    with(|a| {
        a.sync_hotkey(&new.general.record_hotkey);
        if a.settings.video != new.video {
            spawn_worker(hwnd, new.clone(), false);
        }
        a.settings = new;
    });
}

fn open_settings() {
    let exe = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join(ipc::SETTINGS_EXE)));
    match exe {
        Some(exe) if exe.exists() => {
            if let Err(e) = Command::new(&exe).spawn() {
                notify::error(&format!("Could not open Settings: {e}"));
            }
        }
        _ => notify::error(&format!("{} was not found next to framelet.exe", ipc::SETTINGS_EXE)),
    }
}

fn open_recordings_folder() {
    if let Some(dir) = with(|a| a.settings.general.output_dir.clone()) {
        save::open_folder(&dir);
    }
}

fn request_exit() {
    let Some((state, hwnd)) = with(|a| (a.state, a.hwnd)) else { return };
    match state {
        State::Recording => match save::ask_exit(hwnd) {
            ExitChoice::Cancel => {}
            choice => {
                with(|a| {
                    a.exit_after = true;
                    if a.state != State::Recording {
                        // The recording ended while the dialog was open.
                        if a.state == State::Idle {
                            // SAFETY: our own window.
                            unsafe {
                                let _ = DestroyWindow(a.hwnd);
                            }
                        }
                        return;
                    }
                    if let Some(s) = &a.session {
                        match choice {
                            ExitChoice::Discard => s.cancel(),
                            _ => s.stop(),
                        }
                    }
                    a.enter_processing();
                });
            }
        },
        State::Idle => {
            // SAFETY: our own window.
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
        }
        State::Processing => {
            if with(|a| std::mem::replace(&mut a.exit_after, true)).unwrap_or(false) {
                log::warn!("second exit request while processing; quitting now, the temp capture is recovered later");
                // Dropping the session would cancel it and delete the capture; keep it for recovery.
                with(|a| a.session.take().map(std::mem::forget));
                // SAFETY: our own window.
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
            } else {
                notify::info("Finishing the recording. Framelet will exit afterwards.", None);
            }
        }
        State::Selecting | State::Countdown => {}
    }
}
