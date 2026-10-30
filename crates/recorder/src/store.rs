/*
 * Author: Jamius Siam
 * Since: 31/10/2026
 */

//! Moving finished recordings to their destination and recovering orphans after a crash.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use framelet_core::caps::EncoderCaps;
use framelet_core::{Settings, naming, paths};
use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, ERROR_FILE_EXISTS};
use windows::Win32::Storage::FileSystem::{MOVEFILE_COPY_ALLOWED, MoveFileExW};
use windows::core::{HRESULT, HSTRING};

use crate::{ffmpeg, post};

pub fn save_output(temp: &Path, dest: &Path) -> io::Result<PathBuf> {
    let dir = dest.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    fs::create_dir_all(dir)?;
    let stem = dest.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Recording".into());
    let ext = dest.extension().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();

    let mut target = dest.to_path_buf();
    if target.exists() {
        target = naming::unique_path(dir, &stem, &ext);
    }
    // Another process may take the name between the check and the move; the move itself never overwrites.
    for _ in 0..32 {
        match move_file(temp, &target) {
            Ok(()) => return Ok(target),
            Err(e) if is_exists(&e) => target = naming::unique_path(dir, &stem, &ext),
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, "no free file name found"))
}

fn move_file(from: &Path, to: &Path) -> io::Result<()> {
    // SAFETY: both strings outlive the call.
    unsafe { MoveFileExW(&HSTRING::from(from.as_os_str()), &HSTRING::from(to.as_os_str()), MOVEFILE_COPY_ALLOWED) }
        .map_err(io::Error::from)
}

/// `From<windows::core::Error> for io::Error` stores the HRESULT (0x8007xxxx) as the raw OS error.
fn is_exists(e: &io::Error) -> bool {
    let exists = [ERROR_FILE_EXISTS, ERROR_ALREADY_EXISTS].map(|code| HRESULT::from_win32(code.0).0);
    e.raw_os_error().is_some_and(|code| exists.contains(&code))
}

/// Every leftover in the temp dir is from a crashed run (the app is single instance). Recordings are
/// remuxed to `<name> (recovered).mp4`; finished files that were never saved are kept as they are.
pub fn recover_orphans(settings: &Settings, caps: &EncoderCaps) -> Vec<PathBuf> {
    let dir = paths::temp_dir();
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let own_prefix = format!("rec-{}-", std::process::id());
    let ffmpeg = caps.ffmpeg_path.clone().filter(|p| p.is_file()).or_else(|| ffmpeg::locate_ffmpeg(settings));

    let mut files: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect();
    files.retain(|p| !file_name(p).starts_with(&own_prefix));
    files.sort();

    let ext_of = |p: &Path| p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let mut saved = Vec::new();
    for path in files.iter().filter(|p| p.exists()) {
        match ext_of(path).as_str() {
            "mkv" => {
                if let Some(out) = recover_capture(path, settings, ffmpeg.as_deref()) {
                    saved.push(out);
                }
            }
            // A finished file whose capture was already deleted was never saved by the app.
            "mp4" | "gif" if !path.with_extension("mkv").exists() => {
                if let Some(out) = save_recovered(path, settings, &ext_of(path)) {
                    saved.push(out);
                }
            }
            // Stale logs, probe output, partial conversions of a still present capture.
            "log" | "txt" | "mp4" | "gif" => {
                let _ = fs::remove_file(path);
            }
            _ => {}
        }
    }
    saved
}

fn recover_capture(mkv: &Path, settings: &Settings, ffmpeg: Option<&Path>) -> Option<PathBuf> {
    if fs::metadata(mkv).map(|m| m.len()).unwrap_or(0) == 0 {
        let _ = fs::remove_file(mkv);
        return None;
    }
    if let Some(ffmpeg) = ffmpeg {
        let mp4 = mkv.with_extension("mp4");
        let mut cmd = ffmpeg::command(ffmpeg);
        cmd.args(post::remux_args(mkv, &mp4)).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        let remuxed =
            ffmpeg::spawn(&mut cmd).and_then(|mut child| child.wait()).map(|status| status.success()).unwrap_or(false);
        if remuxed && let Some(out) = save_recovered(&mp4, settings, "mp4") {
            let _ = fs::remove_file(mkv);
            return Some(out);
        }
        let _ = fs::remove_file(&mp4);
        log::warn!("cannot remux orphan {}; moving the raw capture", mkv.display());
    }
    save_recovered(mkv, settings, "mkv")
}

fn save_recovered(path: &Path, settings: &Settings, ext: &str) -> Option<PathBuf> {
    let name = format!("{} (recovered).{ext}", naming::expand_pattern(&settings.general.file_name_pattern));
    match save_output(path, &settings.general.output_dir.join(name)) {
        Ok(out) => {
            log::info!("recovered {} to {}", path.display(), out.display());
            Some(out)
        }
        Err(e) => {
            log::warn!("cannot save recovered file {}: {e}", path.display());
            None
        }
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}
