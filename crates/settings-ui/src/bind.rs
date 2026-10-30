/*
 * Author: Jamius Siam
 * Since: 31/10/2026
 */

//! Two-way mapping between [`Settings`] and the Slint `State` global.

use std::path::{Path, PathBuf};

use framelet_core::hotkey::Hotkey;
use framelet_core::naming;
use framelet_core::settings::{
    self as cfg, COUNTDOWN_OPTIONS, Codec, Dither, EncoderPref, GIF_FPS_OPTIONS, GifQuality, GifScale, OutputFormat,
    Preset, Settings, VIDEO_FPS_OPTIONS,
};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::{SettingsWindow, State};

const THEMES: [cfg::Theme; 3] = [cfg::Theme::System, cfg::Theme::Dark, cfg::Theme::Light];
const FORMATS: [OutputFormat; 2] = [OutputFormat::Mp4, OutputFormat::Gif];
const ENCODERS: [EncoderPref; 3] = [EncoderPref::Auto, EncoderPref::Nvenc, EncoderPref::Software];
const PRESETS: [Preset; 3] = [Preset::Fast, Preset::Balanced, Preset::Quality];
const SCALES: [GifScale; 4] = [GifScale::Original, GifScale::Percent75, GifScale::Percent50, GifScale::CustomWidth];
const GIF_QUALITIES: [GifQuality; 3] = [GifQuality::Smaller, GifQuality::Balanced, GifQuality::Best];
const DITHERS: [Dither; 4] = [Dither::Auto, Dither::Bayer, Dither::FloydSteinberg, Dither::None];

const MAX_GIF_WIDTH: u32 = 7680;
const MIN_GIF_WIDTH: u32 = 16;
const MAX_GIF_SECS: u32 = 3600;

fn index_of<T: PartialEq>(list: &[T], value: &T) -> i32 {
    list.iter().position(|v| v == value).unwrap_or(0) as i32
}

fn pick<T: Copy>(list: &[T], index: i32) -> T {
    list[(index.max(0) as usize).min(list.len() - 1)]
}

/// Parses a whole number inside `range`; `None` if it is not a number or out of range.
fn parse_in(text: &str, range: std::ops::RangeInclusive<u32>) -> Option<u32> {
    text.trim().parse::<u32>().ok().filter(|v| range.contains(v))
}

pub fn hotkey_keys(hotkey: &str) -> ModelRc<SharedString> {
    let keys = Hotkey::parse(hotkey).map(|h| h.to_display_string()).unwrap_or_else(|| hotkey.to_string());
    let parts: Vec<SharedString> = keys.split('+').map(SharedString::from).collect();
    ModelRc::new(VecModel::from(parts))
}

/// Updates the live file name preview.
pub fn update_preview(ui: &SettingsWindow, pattern: &str, format: OutputFormat) {
    let state = ui.global::<State>();
    let stem = naming::expand_pattern(pattern);
    state.set_pattern_preview(format!("{stem}.{}", format.extension()).into());
}

/// Copies every setting into the UI.
pub fn push(ui: &SettingsWindow, s: &Settings) {
    let st = ui.global::<State>();
    let g = &s.general;
    st.set_output_dir(g.output_dir.to_string_lossy().as_ref().into());
    st.set_output_dir_error(false);
    st.set_ask_save(g.ask_save_location);
    st.set_file_pattern(g.file_name_pattern.as_str().into());
    st.set_pattern_error(false);
    st.set_startup(g.launch_at_startup);
    st.set_tray(g.minimize_to_tray);
    st.set_hotkey_keys(hotkey_keys(&g.record_hotkey));
    st.set_theme_index(index_of(&THEMES, &g.theme));
    st.set_open_folder(g.open_folder_after_save);
    st.set_notifications(g.notifications);

    let r = &s.recording;
    st.set_format_index(index_of(&FORMATS, &r.format));
    st.set_show_cursor(r.show_cursor);
    st.set_countdown_index(index_of(&COUNTDOWN_OPTIONS, &r.countdown_secs));
    st.set_show_elapsed(r.show_elapsed_time);
    st.set_show_controls(r.show_controls);
    st.set_stop_hotkey(r.stop_with_hotkey);
    st.set_dim_while_recording(r.dim_while_recording);

    let v = &s.video;
    st.set_encoder_index(index_of(&ENCODERS, &v.encoder));
    st.set_fps_index(index_of(&VIDEO_FPS_OPTIONS, &v.fps));
    st.set_quality(v.quality as i32);
    st.set_preset_index(index_of(&PRESETS, &v.preset));
    let ffmpeg = v.ffmpeg_path.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    st.set_ffmpeg_path(ffmpeg.into());
    st.set_ffmpeg_error(false);

    let gif = &s.gif;
    st.set_gif_fps_index(index_of(&GIF_FPS_OPTIONS, &gif.fps));
    st.set_gif_scale_index(index_of(&SCALES, &gif.scale));
    st.set_gif_width(gif.custom_width.to_string().into());
    st.set_gif_width_error(false);
    st.set_gif_quality_index(index_of(&GIF_QUALITIES, &gif.quality));
    st.set_gif_loop(gif.loop_forever);
    st.set_gif_dither_index(index_of(&DITHERS, &gif.dither));
    st.set_gif_max(gif.max_duration_secs.to_string().into());
    st.set_gif_max_error(false);

    update_preview(ui, &g.file_name_pattern, r.format);
}

/// Reads the UI into `s`. Invalid text input is flagged in the UI and the previous valid value is kept.
/// The hotkey is not read here: it is only changed by the capture flow.
pub fn pull(ui: &SettingsWindow, s: &mut Settings) {
    let st = ui.global::<State>();

    let dir = st.get_output_dir();
    let dir_ok = Path::new(dir.as_str()).is_absolute();
    st.set_output_dir_error(!dir_ok);
    let g = &mut s.general;
    if dir_ok {
        g.output_dir = PathBuf::from(dir.as_str());
    }
    g.ask_save_location = st.get_ask_save();
    let pattern = st.get_file_pattern();
    let pattern_ok = !pattern.trim().is_empty();
    st.set_pattern_error(!pattern_ok);
    if pattern_ok {
        g.file_name_pattern = pattern.to_string();
    }
    g.launch_at_startup = st.get_startup();
    g.minimize_to_tray = st.get_tray();
    g.theme = pick(&THEMES, st.get_theme_index());
    g.open_folder_after_save = st.get_open_folder();
    g.notifications = st.get_notifications();

    let r = &mut s.recording;
    r.format = pick(&FORMATS, st.get_format_index());
    r.show_cursor = st.get_show_cursor();
    r.countdown_secs = pick(&COUNTDOWN_OPTIONS, st.get_countdown_index());
    r.show_elapsed_time = st.get_show_elapsed();
    r.show_controls = st.get_show_controls();
    r.stop_with_hotkey = st.get_stop_hotkey();
    r.dim_while_recording = st.get_dim_while_recording();

    let v = &mut s.video;
    v.encoder = pick(&ENCODERS, st.get_encoder_index());
    v.codec = Codec::H264;
    v.fps = pick(&VIDEO_FPS_OPTIONS, st.get_fps_index());
    v.quality = st.get_quality().clamp(0, 100) as u32;
    v.preset = pick(&PRESETS, st.get_preset_index());
    let ffmpeg = st.get_ffmpeg_path();
    let ffmpeg = ffmpeg.trim();
    let ffmpeg_ok = ffmpeg.is_empty() || Path::new(ffmpeg).is_file();
    st.set_ffmpeg_error(!ffmpeg_ok);
    if ffmpeg_ok {
        v.ffmpeg_path = (!ffmpeg.is_empty()).then(|| PathBuf::from(ffmpeg));
    }

    let gif = &mut s.gif;
    gif.fps = pick(&GIF_FPS_OPTIONS, st.get_gif_fps_index());
    gif.scale = pick(&SCALES, st.get_gif_scale_index());
    let width = parse_in(st.get_gif_width().as_str(), MIN_GIF_WIDTH..=MAX_GIF_WIDTH);
    st.set_gif_width_error(width.is_none());
    if let Some(w) = width {
        gif.custom_width = w;
    }
    gif.quality = pick(&GIF_QUALITIES, st.get_gif_quality_index());
    gif.loop_forever = st.get_gif_loop();
    gif.dither = pick(&DITHERS, st.get_gif_dither_index());
    let max = parse_in(st.get_gif_max().as_str(), 0..=MAX_GIF_SECS);
    st.set_gif_max_error(max.is_none());
    if let Some(m) = max {
        gif.max_duration_secs = m;
    }

    update_preview(ui, &s.general.file_name_pattern, s.recording.format);
}

/// Finishes text editing: numbers are clamped into range, other invalid text goes back to the last valid value.
pub fn commit_text(ui: &SettingsWindow, s: &mut Settings) {
    let st = ui.global::<State>();
    if st.get_output_dir_error() {
        st.set_output_dir(s.general.output_dir.to_string_lossy().as_ref().into());
        st.set_output_dir_error(false);
    }
    if st.get_pattern_error() {
        st.set_file_pattern(s.general.file_name_pattern.as_str().into());
        st.set_pattern_error(false);
    }
    if let Ok(w) = st.get_gif_width().trim().parse::<u32>() {
        s.gif.custom_width = w.clamp(MIN_GIF_WIDTH, MAX_GIF_WIDTH);
    }
    st.set_gif_width(s.gif.custom_width.to_string().into());
    st.set_gif_width_error(false);
    if let Ok(m) = st.get_gif_max().trim().parse::<u32>() {
        s.gif.max_duration_secs = m.min(MAX_GIF_SECS);
    }
    st.set_gif_max(s.gif.max_duration_secs.to_string().into());
    st.set_gif_max_error(false);
}
