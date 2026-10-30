/*
 * Author: Jamius Siam
 * Since: 31/10/2026
 */

//! Identifiers shared between `framelet.exe` (tray) and `framelet-settings.exe`.
//!
//! The tray process owns a hidden top-level window of class [`TRAY_WINDOW_CLASS`]
//! (top-level, not message-only, so `FindWindowW` and `TaskbarCreated` reach it). Other processes
//! find it with `FindWindowW(TRAY_WINDOW_CLASS, None)` and `PostMessageW` one of the messages
//! below, obtained with `RegisterWindowMessageW(<name>)`. All messages use wParam = lParam = 0.

/// Window class of the tray process's hidden window.
pub const TRAY_WINDOW_CLASS: &str = "Framelet.TrayWindow";

/// Settings file changed on disk: tray reloads settings, re-registers hotkey, refreshes state.
pub const MSG_SETTINGS_CHANGED: &str = "Framelet.SettingsChanged";

/// Settings window is capturing a new hotkey: tray unregisters its global hotkey(s).
pub const MSG_SUSPEND_HOTKEYS: &str = "Framelet.SuspendHotkeys";

/// Hotkey capture ended: tray registers its hotkey(s) again from the current settings.
pub const MSG_RESUME_HOTKEYS: &str = "Framelet.ResumeHotkeys";

/// A second `framelet.exe` was launched: the running instance opens the settings window.
pub const MSG_ACTIVATE: &str = "Framelet.Activate";

/// Named mutex (session-local) guaranteeing one tray process.
pub const TRAY_MUTEX: &str = "Local\\Framelet.Tray";

/// Named mutex (session-local) guaranteeing one settings window process.
pub const SETTINGS_MUTEX: &str = "Local\\Framelet.Settings";

/// Title of the settings window (used to find and focus an existing instance).
pub const SETTINGS_WINDOW_TITLE: &str = "Framelet Settings";

/// File names of the two executables; they live in the same directory.
pub const TRAY_EXE: &str = "framelet.exe";
pub const SETTINGS_EXE: &str = "framelet-settings.exe";
