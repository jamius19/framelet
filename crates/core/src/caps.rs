/*
 * Author: Jamius Siam
 * Since: 31/10/2026
 */

//! Encoder capability model, persisted to [`crate::paths::caps_file`].
//! Detection itself lives in `framelet-recorder`; this crate only owns the data shape so the
//! settings UI and the tray process agree on it.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::paths;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EncoderCaps {
    /// Resolved ffmpeg.exe, `None` if FFmpeg could not be found.
    pub ffmpeg_path: Option<PathBuf>,
    /// First line of `ffmpeg -version`, e.g. "ffmpeg version 7.1-full_build-www.gyan.dev".
    pub ffmpeg_version: Option<String>,
    /// Description of the NVIDIA adapter (DXGI vendor id 0x10DE), e.g. "NVIDIA GeForce RTX 3050".
    pub nvidia_gpu: Option<String>,
    /// `h264_nvenc` encoded a test clip successfully.
    pub nvenc_h264: bool,
    /// `ddagrab` (D3D11 Desktop Duplication) delivered a frame to `h264_nvenc` successfully.
    pub ddagrab: bool,
    /// `libx264` is available in this FFmpeg build.
    pub x264: bool,
    /// Opaque string identifying the environment the detection ran in (ffmpeg path, size and
    /// mtime, adapter driver version). A mismatch means the cache is stale.
    pub fingerprint: String,
}

impl EncoderCaps {
    /// Reads the cached detection result, if any.
    pub fn load_cached() -> Option<EncoderCaps> {
        let text = fs::read_to_string(paths::caps_file()).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub fn save_cache(&self) -> std::io::Result<()> {
        let path = paths::caps_file();
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        fs::write(path, json)
    }

    /// One-line human readable summary for the settings UI, e.g.
    /// "Hardware acceleration: NVIDIA GeForce RTX 3050 — NVENC available".
    pub fn summary(&self) -> String {
        if self.ffmpeg_path.is_none() {
            return "FFmpeg not found — recording is unavailable".into();
        }
        match (&self.nvidia_gpu, self.nvenc_h264) {
            (Some(gpu), true) => format!("Hardware acceleration: {gpu} — NVENC available"),
            (Some(gpu), false) => format!("{gpu} detected — NVENC unavailable, using software encoding (x264)"),
            (None, _) => "No NVIDIA GPU detected — using software encoding (x264)".into(),
        }
    }
}
