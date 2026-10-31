/*
 * Author: Jamius Siam
 * Since: 31/10/2026
 */

//! FFmpeg capture pipeline: discovery, capability detection, capture planning, process lifecycle,
//! remux / scale / WebM / GIF post-processing, save and crash recovery. See ARCHITECTURE.md.

mod detect;
mod display;
mod ffmpeg;
mod plan;
mod post;
mod session;
mod store;

use std::io;
use std::path::{Path, PathBuf};

use framelet_core::Settings;
use framelet_core::caps::EncoderCaps;
use framelet_core::settings::OutputFormat;

pub use session::RecordingSession;

/// Events delivered from the recording worker thread to the `on_event` callback.
#[derive(Debug)]
pub enum RecorderEvent {
    /// Capture ended (stop, max duration reached); post-processing (remux / scaled re-encode / WebM / GIF) is
    /// running.
    Processing,
    /// The final file is ready inside the temp dir. The receiver moves it with [`save_output`].
    Finished(FinishedRecording),
    /// Recording canceled; temp files removed.
    Canceled,
    /// Capture or processing failed. `partial` is a salvaged playable file in the temp dir, if any.
    /// Its own extension (`.mp4` or `.mkv`) is the container it holds.
    Failed { message: String, partial: Option<PathBuf> },
}

#[derive(Debug)]
pub struct FinishedRecording {
    pub path: PathBuf,
    pub format: OutputFormat,
    /// Non-fatal note to surface to the user, e.g. "NVENC unavailable — used software encoding".
    pub warning: Option<String>,
}

/// Resolves ffmpeg.exe: settings override, next to the current exe, `<exe dir>\ffmpeg\bin`, then PATH.
pub fn locate_ffmpeg(settings: &Settings) -> Option<PathBuf> {
    ffmpeg::locate_ffmpeg(settings)
}

/// Returns cached capabilities if the fingerprint still matches, otherwise runs detection
/// (spawns short FFmpeg probes, ~1 s) and updates the cache. Blocking; call off the UI thread.
pub fn load_or_detect_caps(settings: &Settings, force: bool) -> EncoderCaps {
    detect::load_or_detect_caps(settings, force)
}

/// Moves `temp` to `dest` without overwriting (picks "name (2).ext" etc. if taken), across
/// volumes if needed. Returns the final path.
pub fn save_output(temp: &Path, dest: &Path) -> io::Result<PathBuf> {
    store::save_output(temp, dest)
}

/// Finalizes recordings orphaned in the temp dir by a crash into the output dir
/// (as "<name> (recovered).mp4"). Blocking; call off the UI thread. Returns saved files.
pub fn recover_orphans(settings: &Settings, caps: &EncoderCaps) -> Vec<PathBuf> {
    store::recover_orphans(settings, caps)
}
