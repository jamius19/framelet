/*
 * Author: Jamius Siam
 * Since: 31/10/2026
 */

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod bind;
mod win;

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::panic::{self, AssertUnwindSafe};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use framelet_core::caps::EncoderCaps;
use framelet_core::hotkey::{Hotkey, MOD_ALT, MOD_CONTROL, MOD_WIN};
use framelet_core::ipc;
use framelet_core::settings::{self as cfg, Settings};
use slint::{ComponentHandle, Timer, TimerMode};

slint::include_modules!();

const SAVE_DEBOUNCE: Duration = Duration::from_millis(300);
const VIDEO_PAGE: i32 = 2;
/// Mica is used when the OS supports it (`--mica off` disables it in debug builds).
static MICA: AtomicBool = AtomicBool::new(true);

struct Ctx {
    ui: slint::Weak<SettingsWindow>,
    settings: RefCell<Settings>,
    /// Last state written to (or loaded from) disk.
    saved: RefCell<Settings>,
    save_timer: Timer,
    caps_requested: Cell<bool>,
    /// The FFmpeg path the last detection ran with.
    detected_ffmpeg: RefCell<Option<PathBuf>>,
}

impl Ctx {
    fn ui(&self) -> SettingsWindow {
        self.ui.upgrade().expect("settings window outlives its callbacks")
    }

    fn on_changed(self: &Rc<Self>) {
        let ui = self.ui();
        bind::pull(&ui, &mut self.settings.borrow_mut());
        self.apply_runtime(&ui);
        self.schedule_save();
    }

    /// Applies settings that take effect immediately inside this window.
    fn apply_runtime(&self, ui: &SettingsWindow) {
        let g = self.settings.borrow().general.clone();
        win::set_minimize_to_tray(g.minimize_to_tray);
        apply_theme(ui, g.theme);
    }

    fn on_commit_text(self: &Rc<Self>) {
        let ui = self.ui();
        bind::commit_text(&ui, &mut self.settings.borrow_mut());
        self.schedule_save();
        // `pull` already took over the typed path, so compare with what the last detection used.
        if self.settings.borrow().video.ffmpeg_path != *self.detected_ffmpeg.borrow() && self.caps_requested.get() {
            self.detect_caps(false);
        }
    }

    fn schedule_save(self: &Rc<Self>) {
        let ctx = Rc::downgrade(self);
        self.save_timer.start(TimerMode::SingleShot, SAVE_DEBOUNCE, move || {
            if let Some(ctx) = ctx.upgrade() {
                ctx.save_now();
            }
        });
    }

    /// Writes the settings if they changed, then tells the tray process.
    fn save_now(&self) {
        self.save_timer.stop();
        let mut settings = self.settings.borrow_mut();
        if *settings == *self.saved.borrow() {
            return;
        }
        let result = settings.save();
        let message = match &result {
            Ok(()) => String::new(),
            Err(e) => {
                log::error!("saving settings failed: {e}");
                format!("Couldn't save your settings: {e}")
            }
        };
        if result.is_ok() {
            *self.saved.borrow_mut() = settings.clone();
            win::post_to_tray(ipc::MSG_SETTINGS_CHANGED);
        }
        if let Some(ui) = self.ui.upgrade() {
            ui.global::<State>().set_save_error(message.into());
        }
    }

    fn on_startup_toggled(&self, enabled: bool) {
        let ui = self.ui();
        if let Err(e) = win::set_startup(enabled) {
            log::error!("updating the startup entry failed: {e}");
            let state = ui.global::<State>();
            state.set_startup(win::startup_enabled());
            state.set_save_error("Couldn't change the startup setting.".into());
            return;
        }
        self.settings.borrow_mut().general.launch_at_startup = enabled;
    }

    fn on_page_changed(self: &Rc<Self>, page: i32) {
        // Leaving a page ends any text editing on it.
        self.on_commit_text();
        if page == VIDEO_PAGE && !self.caps_requested.get() {
            self.detect_caps(false);
        }
    }

    /// Runs hardware detection off the UI thread. A panicking detector falls back to the cache.
    fn detect_caps(&self, force: bool) {
        self.caps_requested.set(true);
        let ui = self.ui();
        ui.global::<State>().set_caps_detecting(true);
        let settings = self.settings.borrow().clone();
        *self.detected_ffmpeg.borrow_mut() = settings.video.ffmpeg_path.clone();
        let weak = ui.as_weak();
        std::thread::spawn(move || {
            let detect = || framelet_recorder::load_or_detect_caps(&settings, force);
            let caps = panic::catch_unwind(AssertUnwindSafe(detect)).ok().or_else(EncoderCaps::load_cached);
            let _ = slint::invoke_from_event_loop(move || {
                let Some(ui) = weak.upgrade() else { return };
                let state = ui.global::<State>();
                state.set_caps_detecting(false);
                state.set_caps_known(caps.is_some());
                let fake_no_nvenc = cfg!(debug_assertions) && std::env::args().any(|a| a == "--no-nvenc");
                state.set_caps_nvenc(!fake_no_nvenc && caps.as_ref().is_some_and(|c| c.nvenc_h264));
                let summary = caps.map(|c| c.summary()).unwrap_or_else(|| "Hardware detection isn't available.".into());
                state.set_caps_summary(summary.into());
            });
        });
    }

    fn browse_output(self: &Rc<Self>) {
        let ui = self.ui();
        let Some(hwnd) = win::hwnd_of(ui.window()) else { return };
        let current = self.settings.borrow().general.output_dir.clone();
        if let Some(dir) = win::pick_folder(hwnd, &current) {
            ui.global::<State>().set_output_dir(dir.to_string_lossy().as_ref().into());
            self.on_changed();
        }
    }

    fn browse_ffmpeg(self: &Rc<Self>) {
        let ui = self.ui();
        let Some(hwnd) = win::hwnd_of(ui.window()) else { return };
        let current = self.settings.borrow().video.ffmpeg_path.clone();
        if let Some(file) = win::pick_ffmpeg(hwnd, current.as_deref()) {
            ui.global::<State>().set_ffmpeg_path(file.to_string_lossy().as_ref().into());
            self.on_changed();
            self.on_commit_text();
        }
    }

    fn start_capture(&self) {
        let ui = self.ui();
        let state = ui.global::<State>();
        state.set_hotkey_error("".into());
        state.set_capturing(true);
        win::set_capture(true);
        win::post_to_tray(ipc::MSG_SUSPEND_HOTKEYS);
    }

    fn end_capture(&self) {
        if win::is_capturing() {
            win::set_capture(false);
            win::post_to_tray(ipc::MSG_RESUME_HOTKEYS);
        }
        if let Some(ui) = self.ui.upgrade() {
            ui.global::<State>().set_capturing(false);
        }
    }

    /// Called for every non-modifier key press while capturing.
    fn on_captured_key(&self, vk: u32, modifiers: u32) {
        let ui = self.ui();
        let state = ui.global::<State>();
        if vk == win::VK_ESC && modifiers == 0 {
            state.set_hotkey_error("".into());
            return self.end_capture();
        }
        let error = match Hotkey::from_parts(modifiers, vk) {
            None if modifiers & (MOD_CONTROL | MOD_ALT | MOD_WIN) == 0 => "Include at least one of Ctrl, Alt or Win.",
            None => "This key can't be used in a shortcut.",
            Some(h) if !win::hotkey_available(h) => "This shortcut is used by another app",
            Some(h) => {
                let text = h.to_display_string();
                state.set_hotkey_keys(bind::hotkey_keys(&text));
                state.set_hotkey_error("".into());
                self.settings.borrow_mut().general.record_hotkey = text;
                // Saved before the tray is told to resume, so it registers the new combination.
                self.save_now();
                return self.end_capture();
            }
        };
        state.set_hotkey_error(error.into());
    }
}

fn apply_theme(ui: &SettingsWindow, theme: cfg::Theme) {
    let dark = match theme {
        cfg::Theme::System => win::system_prefers_dark(),
        cfg::Theme::Dark => true,
        cfg::Theme::Light => false,
    };
    ui.global::<Theme>().set_dark(dark);
    if let Some(hwnd) = win::hwnd_of(ui.window()) {
        let (caption, text) = if dark { (0x202020, 0xFFFFFF) } else { (0xF3F3F3, 0x1A1A1A) };
        let mica = ui.global::<Theme>().get_mica();
        win::style_window(hwnd, dark, (!mica).then_some(caption), text);
    }
}

/// The native window only exists once the event loop runs, so styling and the subclass are
/// installed from a short retry loop.
fn setup_native_window(ui: slint::Weak<SettingsWindow>, ctx: Weak<Ctx>, attempts: u32) {
    Timer::single_shot(Duration::from_millis(10), move || {
        let (Some(window), Some(ctx)) = (ui.upgrade(), ctx.upgrade()) else { return };
        match win::hwnd_of(window.window()) {
            Some(hwnd) => {
                win::install_subclass(hwnd);
                win::set_window_icon(hwnd);
                if window.global::<Theme>().get_mica() {
                    win::enable_mica(hwnd);
                }
                let theme = ctx.settings.borrow().general.theme;
                apply_theme(&window, theme);
            }
            None if attempts > 0 => setup_native_window(ui, Rc::downgrade(&ctx), attempts - 1),
            None => log::warn!("native window handle unavailable"),
        }
    });
}

fn connect(ui: &SettingsWindow, ctx: &Rc<Ctx>) {
    let state = ui.global::<State>();
    let c = ctx.clone();
    state.on_changed(move || c.on_changed());
    let c = ctx.clone();
    state.on_commit_text(move || c.on_commit_text());
    let c = ctx.clone();
    state.on_startup_toggled(move |on| c.on_startup_toggled(on));
    let c = ctx.clone();
    state.on_browse_output(move || c.browse_output());
    let c = ctx.clone();
    state.on_browse_ffmpeg(move || c.browse_ffmpeg());
    let c = ctx.clone();
    state.on_detect_again(move || c.detect_caps(true));
    let c = ctx.clone();
    state.on_start_capture(move || c.start_capture());
    let c = ctx.clone();
    state.on_cancel_capture(move || c.end_capture());
    let c = ctx.clone();
    state.on_page_changed(move |page| c.on_page_changed(page));

    let c = Rc::downgrade(ctx);
    win::set_key_handler(move |vk, mods| {
        if let Some(c) = c.upgrade() {
            c.on_captured_key(vk, mods);
        }
    });
}

/// Debug builds accept `--page N`, `--theme dark|light`, `--encoder nvenc`, `--no-nvenc` and `--mica off`
/// (in memory only) for screenshots.
fn debug_overrides(settings: &mut Settings) -> i32 {
    let mut page = 0;
    if cfg!(debug_assertions) {
        let args: Vec<String> = std::env::args().collect();
        for pair in args.windows(2) {
            match (pair[0].as_str(), pair[1].as_str()) {
                ("--page", n) => page = n.parse().unwrap_or(0),
                ("--mica", "off") => MICA.store(false, Ordering::Relaxed),
                ("--theme", "dark") => settings.general.theme = cfg::Theme::Dark,
                ("--theme", "light") => settings.general.theme = cfg::Theme::Light,
                ("--encoder", "nvenc") => settings.video.encoder = cfg::EncoderPref::Nvenc,
                _ => {}
            }
        }
    }
    page
}

fn main() -> Result<(), slint::PlatformError> {
    framelet_core::logging::init("settings");
    if !win::acquire_single_instance() {
        return Ok(());
    }

    let mut settings = Settings::load();
    settings.general.launch_at_startup = win::startup_enabled();
    let page = debug_overrides(&mut settings);

    let ui = SettingsWindow::new()?;
    ui.set_window_title(ipc::SETTINGS_WINDOW_TITLE.into());
    let state = ui.global::<State>();
    state.set_version(env!("CARGO_PKG_VERSION").into());
    state.set_page(page);
    ui.global::<Theme>().set_mica(MICA.load(Ordering::Relaxed) && win::mica_supported());
    bind::push(&ui, &settings);
    win::set_minimize_to_tray(settings.general.minimize_to_tray);
    apply_theme(&ui, settings.general.theme);

    let ctx = Rc::new(Ctx {
        ui: ui.as_weak(),
        saved: RefCell::new(settings.clone()),
        settings: RefCell::new(settings),
        save_timer: Timer::default(),
        caps_requested: Cell::new(false),
        detected_ffmpeg: RefCell::new(None),
    });
    connect(&ui, &ctx);
    setup_native_window(ui.as_weak(), Rc::downgrade(&ctx), 100);
    if page == VIDEO_PAGE {
        ctx.on_page_changed(page);
    }

    ui.run()?;

    // Closing the window flushes a pending save and gives the hotkeys back to the tray.
    ctx.save_now();
    ctx.end_capture();
    Ok(())
}
