/*
 * Author: Jamius Siam
 * Since: 31/10/2026
 */

//! Encoder capability detection with a fingerprinted on-disk cache.

use std::fs;
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use framelet_core::Settings;
use framelet_core::caps::EncoderCaps;

use crate::display::{self, Adapter};
use crate::ffmpeg;

const FAST_TIMEOUT: Duration = Duration::from_secs(10);
const ENCODE_TIMEOUT: Duration = Duration::from_secs(15);
/// Bump when the probes change so older caches are discarded.
const DETECTION_VERSION: u32 = 1;

pub fn load_or_detect_caps(settings: &Settings, force: bool) -> EncoderCaps {
    let ffmpeg = ffmpeg::locate_ffmpeg(settings);
    let gpu = display::nvidia_adapter();
    let fingerprint = fingerprint(ffmpeg.as_deref(), gpu.as_ref());

    if !force && let Some(cached) = EncoderCaps::load_cached().filter(|c| c.fingerprint == fingerprint) {
        log::debug!("using cached encoder capabilities");
        return cached;
    }

    let mut caps = EncoderCaps {
        ffmpeg_path: ffmpeg.clone(),
        nvidia_gpu: gpu.as_ref().map(|a| a.description.clone()),
        fingerprint,
        ..EncoderCaps::default()
    };
    match &ffmpeg {
        Some(path) => probe_ffmpeg(path, gpu.as_ref().map_or(0, |a| a.index), &mut caps),
        None => log::warn!("ffmpeg not found; recording is unavailable"),
    }
    log::info!("encoder capabilities: {caps:?}");
    if let Err(e) = caps.save_cache() {
        log::warn!("cannot save the capability cache: {e}");
    }
    caps
}

fn fingerprint(ffmpeg: Option<&Path>, gpu: Option<&Adapter>) -> String {
    let file = ffmpeg
        .and_then(|path| {
            let meta = fs::metadata(path).ok()?;
            let mtime = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_secs();
            Some(format!("{}|{}|{mtime}", path.display(), meta.len()))
        })
        .unwrap_or_else(|| "no-ffmpeg".into());
    let gpu = gpu
        .map(|a| format!("{}|{}", a.description, a.driver.as_deref().unwrap_or("?")))
        .unwrap_or_else(|| "no-nvidia".into());
    format!("v{DETECTION_VERSION}|{file}|{gpu}")
}

fn probe_ffmpeg(path: &Path, adapter: u32, caps: &mut EncoderCaps) {
    caps.ffmpeg_version = ffmpeg::probe(path, &["-hide_banner", "-version"], FAST_TIMEOUT)
        .and_then(|p| p.output.lines().next().map(str::to_owned));

    let listed =
        ffmpeg::probe(path, &["-hide_banner", "-encoders"], FAST_TIMEOUT).map(|p| p.output).unwrap_or_default();
    let has_encoder = |name: &str| listed.lines().any(|l| l.split_whitespace().nth(1) == Some(name));
    caps.x264 = has_encoder("libx264");

    if has_encoder("h264_nvenc") {
        let args = "-hide_banner -loglevel error -f lavfi -i color=black:s=256x256:r=30 \
                    -frames:v 5 -c:v h264_nvenc -f null -";
        caps.nvenc_h264 = run_probe(path, args);
        if !caps.nvenc_h264 {
            log::info!("h264_nvenc is listed but failed to encode");
        }
    }

    // ddagrab must deliver a frame to the encoder it will actually feed. Without NVENC, check the
    // CPU path (download to system memory) instead.
    let (graph, encoder) = if caps.nvenc_h264 {
        ("ddagrab=0:framerate=30", "h264_nvenc")
    } else {
        ("ddagrab=0:framerate=30,hwdownload,format=bgra", "rawvideo")
    };
    let args = format!(
        "-hide_banner -loglevel error -init_hw_device d3d11va=dd:{adapter} -filter_hw_device dd \
         -filter_complex {graph} -frames:v 1 -c:v {encoder} -f null -"
    );
    caps.ddagrab = run_probe(path, &args);
}

/// Runs ffmpeg with whitespace-separated `args`; true if it exited successfully in time.
fn run_probe(path: &Path, args: &str) -> bool {
    let args: Vec<&str> = args.split_whitespace().collect();
    ffmpeg::probe(path, &args, ENCODE_TIMEOUT).is_some_and(|p| p.success)
}
