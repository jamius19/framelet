/*
 * Author: Jamius Siam
 * Since: 31/10/2026
 */

//! Persistent application settings (`%APPDATA%\Framelet\settings.json`).
//!
//! Every struct uses `#[serde(default)]` so missing or unknown fields never break loading;
//! `validate()` clamps out-of-range values after load.

use std::fs;
use std::io;
use std::path::PathBuf;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::{hotkey::Hotkey, paths};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Settings {
    pub general: General,
    pub recording: Recording,
    pub video: Video,
    pub gif: Gif,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    /// Directory where recordings are saved automatically.
    pub output_dir: PathBuf,
    /// Show a Save As dialog after each recording instead of saving automatically.
    pub ask_save_location: bool,
    /// File name pattern without extension. Tokens: see [`crate::naming`].
    pub file_name_pattern: String,
    pub launch_at_startup: bool,
    /// Minimizing the settings window closes it and leaves Framelet in the tray.
    pub minimize_to_tray: bool,
    /// Global hotkey that starts (and, if enabled, stops) a recording. Format: "Alt+Shift+R".
    pub record_hotkey: String,
    pub theme: Theme,
    pub open_folder_after_save: bool,
    pub notifications: bool,
}

impl Default for General {
    fn default() -> Self {
        Self {
            output_dir: paths::default_output_dir(),
            ask_save_location: false,
            file_name_pattern: "Framelet {YYYY}-{MM}-{DD} {hh}.{mm}.{ss}".into(),
            launch_at_startup: false,
            minimize_to_tray: true,
            record_hotkey: "Alt+Shift+R".into(),
            theme: Theme::Dark,
            open_folder_after_save: false,
            notifications: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    System,
    Dark,
    Light,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Recording {
    pub format: OutputFormat,
    pub show_cursor: bool,
    /// 0 (off), 3 or 5 seconds.
    pub countdown_secs: u32,
    pub show_elapsed_time: bool,
    pub show_controls: bool,
    /// Pressing the record hotkey while recording stops the recording.
    pub stop_with_hotkey: bool,
    /// Dim the screen outside the recorded region while recording (the red border is always shown).
    pub dim_while_recording: bool,
}

impl Default for Recording {
    fn default() -> Self {
        Self {
            format: OutputFormat::Mp4,
            show_cursor: true,
            countdown_secs: 0,
            show_elapsed_time: true,
            show_controls: true,
            stop_with_hotkey: true,
            dim_while_recording: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputFormat {
    Mp4,
    Webm,
    Gif,
}

impl OutputFormat {
    pub fn extension(self) -> &'static str {
        match self {
            OutputFormat::Mp4 => "mp4",
            OutputFormat::Webm => "webm",
            OutputFormat::Gif => "gif",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Video {
    pub encoder: EncoderPref,
    pub codec: Codec,
    /// 15, 24, 30 or 60.
    pub fps: u32,
    /// 0 = smallest file, 100 = highest quality.
    pub quality: u32,
    pub preset: Preset,
    /// Explicit ffmpeg.exe path. `None` = auto-detect (next to exe, then PATH).
    pub ffmpeg_path: Option<PathBuf>,
}

impl Default for Video {
    fn default() -> Self {
        Self {
            encoder: EncoderPref::Auto,
            codec: Codec::H264,
            fps: 30,
            quality: 75,
            preset: Preset::Balanced,
            ffmpeg_path: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EncoderPref {
    /// Prefer NVIDIA NVENC, fall back to x264.
    Auto,
    Nvenc,
    Software,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Codec {
    H264,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Preset {
    Fast,
    Balanced,
    Quality,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Gif {
    /// 10, 15, 20, 24 or 30.
    pub fps: u32,
    pub scale: GifScale,
    /// Used when `scale == GifScale::CustomWidth`. Height keeps the aspect ratio.
    pub custom_width: u32,
    pub quality: GifQuality,
    pub loop_forever: bool,
    pub dither: Dither,
    /// 0 = unlimited. Recording stops automatically after this many seconds.
    pub max_duration_secs: u32,
}

impl Default for Gif {
    fn default() -> Self {
        Self {
            fps: 15,
            scale: GifScale::Original,
            custom_width: 640,
            quality: GifQuality::Balanced,
            loop_forever: true,
            dither: Dither::Auto,
            max_duration_secs: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GifScale {
    Original,
    Percent75,
    Percent50,
    CustomWidth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GifQuality {
    Smaller,
    Balanced,
    Best,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dither {
    Auto,
    Bayer,
    FloydSteinberg,
    None,
}

pub const VIDEO_FPS_OPTIONS: [u32; 4] = [15, 24, 30, 60];
pub const GIF_FPS_OPTIONS: [u32; 5] = [10, 15, 20, 24, 30];
pub const COUNTDOWN_OPTIONS: [u32; 3] = [0, 3, 5];

impl Settings {
    /// Loads settings from disk. A missing file yields defaults; a file that is not JSON is
    /// backed up to `settings.json.bak` and defaults are used. A section that does not parse
    /// (e.g. an unknown enum value) falls back to its defaults without touching the others.
    pub fn load() -> Settings {
        let path = paths::settings_file();
        let mut settings = match fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(json) => Settings {
                    general: section(&json, "general"),
                    recording: section(&json, "recording"),
                    video: section(&json, "video"),
                    gif: section(&json, "gif"),
                },
                Err(e) => {
                    log::warn!("settings file is invalid ({e}); backing up and using defaults");
                    let _ = fs::rename(&path, path.with_extension("json.bak"));
                    Settings::default()
                }
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => Settings::default(),
            Err(e) => {
                log::warn!("cannot read settings file: {e}");
                Settings::default()
            }
        };
        settings.validate();
        settings
    }

    /// Validates and writes settings atomically (temp file + rename).
    pub fn save(&mut self) -> io::Result<()> {
        self.validate();
        let path = paths::settings_file();
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, json)?;
        fs::rename(&tmp, &path)
    }

    /// Clamps every value into its valid range, replacing invalid values with defaults.
    pub fn validate(&mut self) {
        let g = &mut self.general;
        if g.output_dir.as_os_str().is_empty() || g.output_dir.is_relative() {
            g.output_dir = paths::default_output_dir();
        }
        if g.file_name_pattern.trim().is_empty() {
            g.file_name_pattern = General::default().file_name_pattern;
        }
        if Hotkey::parse(&g.record_hotkey).is_none() {
            g.record_hotkey = General::default().record_hotkey;
        }

        let r = &mut self.recording;
        if !COUNTDOWN_OPTIONS.contains(&r.countdown_secs) {
            r.countdown_secs = 0;
        }

        let v = &mut self.video;
        if !VIDEO_FPS_OPTIONS.contains(&v.fps) {
            v.fps = Video::default().fps;
        }
        v.quality = v.quality.min(100);
        if v.ffmpeg_path.as_ref().is_some_and(|p| p.as_os_str().is_empty()) {
            v.ffmpeg_path = None;
        }

        let gif = &mut self.gif;
        if !GIF_FPS_OPTIONS.contains(&gif.fps) {
            gif.fps = Gif::default().fps;
        }
        gif.custom_width = gif.custom_width.clamp(16, 7680);
        gif.max_duration_secs = gif.max_duration_secs.min(3600);
    }
}

fn section<T: DeserializeOwned + Default>(json: &serde_json::Value, name: &str) -> T {
    let Some(value) = json.get(name) else { return T::default() };
    serde_json::from_value(value.clone()).unwrap_or_else(|e| {
        log::warn!("settings section \"{name}\" is invalid ({e}); using its defaults");
        T::default()
    })
}
