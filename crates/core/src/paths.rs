/*
 * Author: Jamius Siam
 * Since: 31/10/2026
 */

//! Well-known locations. Settings roam (`%APPDATA%`); caches, logs and temp files are local
//! (`%LOCALAPPDATA%`). Directories are not created here; callers create them when writing.

use std::env;
use std::path::PathBuf;

use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{FOLDERID_Videos, KF_FLAG_DEFAULT, SHGetKnownFolderPath};

const APP_DIR: &str = "Framelet";

fn env_dir(var: &str) -> PathBuf {
    env::var_os(var).map(PathBuf::from).unwrap_or_else(env::temp_dir)
}

/// `%APPDATA%\Framelet`
pub fn config_dir() -> PathBuf {
    env_dir("APPDATA").join(APP_DIR)
}

/// `%LOCALAPPDATA%\Framelet`
pub fn data_dir() -> PathBuf {
    env_dir("LOCALAPPDATA").join(APP_DIR)
}

pub fn settings_file() -> PathBuf {
    config_dir().join("settings.json")
}

/// Cached encoder capability detection result.
pub fn caps_file() -> PathBuf {
    data_dir().join("caps.json")
}

/// Region of the last recording, restored when the selection overlay opens.
pub fn last_region_file() -> PathBuf {
    data_dir().join("last_region.json")
}

pub fn logs_dir() -> PathBuf {
    data_dir().join("logs")
}

/// In-progress recordings. Anything left here at startup is an orphan from a crash.
pub fn temp_dir() -> PathBuf {
    data_dir().join("temp")
}

/// `<Videos known folder>\Framelet`, falling back to `%USERPROFILE%\Videos\Framelet`.
pub fn default_output_dir() -> PathBuf {
    // SAFETY: SHGetKnownFolderPath returns a CoTaskMem-allocated string that we free.
    let videos = unsafe {
        SHGetKnownFolderPath(&FOLDERID_Videos, KF_FLAG_DEFAULT, None).ok().and_then(|p| {
            let s = p.to_string().ok();
            CoTaskMemFree(Some(p.0 as *const _));
            s
        })
    };
    videos
        .map(PathBuf::from)
        .unwrap_or_else(|| env_dir("USERPROFILE").join("Videos"))
        .join(APP_DIR)
}
