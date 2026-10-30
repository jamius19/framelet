/*
 * Author: Jamius Siam
 * Since: 31/10/2026
 */

//! Output file naming.
//!
//! Pattern tokens: `{YYYY}` year, `{MM}` month, `{DD}` day, `{hh}` hour (24h), `{mm}` minute,
//! `{ss}` second. Everything else is copied verbatim and then sanitized for Windows.

use std::path::{Path, PathBuf};

use windows::Win32::System::SystemInformation::GetLocalTime;

const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1",
    "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Expands the pattern with the current local time and returns a safe file stem (no extension).
pub fn expand_pattern(pattern: &str) -> String {
    // SAFETY: GetLocalTime has no preconditions.
    let t = unsafe { GetLocalTime() };
    let expanded = pattern
        .replace("{YYYY}", &format!("{:04}", t.wYear))
        .replace("{MM}", &format!("{:02}", t.wMonth))
        .replace("{DD}", &format!("{:02}", t.wDay))
        .replace("{hh}", &format!("{:02}", t.wHour))
        .replace("{mm}", &format!("{:02}", t.wMinute))
        .replace("{ss}", &format!("{:02}", t.wSecond));
    sanitize_stem(&expanded)
}

/// Replaces characters that are invalid in Windows file names, trims trailing dots/spaces
/// and avoids reserved device names. Never returns an empty string.
pub fn sanitize_stem(stem: &str) -> String {
    let mut s: String = stem
        .chars()
        .map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { '_' } else { c })
        .collect();
    s = s.chars().take(200).collect::<String>().trim().trim_end_matches(['.', ' ']).to_string();
    if s.is_empty() {
        s = "Recording".into();
    }
    // Windows treats `NUL.txt` like `NUL`, so the part before the first dot decides.
    let base = s.split('.').next().unwrap_or_default().trim_end();
    if RESERVED.iter().any(|r| r.eq_ignore_ascii_case(base)) {
        s.insert(base.len(), '_');
    }
    s
}

/// Returns `dir\stem.ext`, or `dir\stem (2).ext`, `(3)`, ... for the first name that does not
/// exist yet. Callers must still create the file without overwriting (e.g. `MoveFileExW`
/// without `MOVEFILE_REPLACE_EXISTING`) to be race-free.
pub fn unique_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}).{ext}")))
        .find(|p| !p.exists())
        .expect("unbounded range always yields a free name")
}
