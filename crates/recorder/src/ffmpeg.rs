/*
 * Author: Jamius Siam
 * Since: 31/10/2026
 */

//! FFmpeg discovery, process spawning (hidden window, Job Object) and waiting.

use std::env;
use std::fs::{self, File};
use std::io;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use framelet_core::{Settings, paths};
use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, SetInformationJobObject,
};
use windows::Win32::System::Threading::WaitForSingleObject;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const EXE_NAME: &str = "ffmpeg.exe";

/// Resolves ffmpeg.exe: settings override, next to the current exe, `<exe dir>\ffmpeg\bin`, then PATH.
pub fn locate_ffmpeg(settings: &Settings) -> Option<PathBuf> {
    if let Some(path) = &settings.video.ffmpeg_path {
        if path.is_file() {
            return Some(path.clone());
        }
        log::warn!("configured ffmpeg path does not exist: {}", path.display());
    }

    let exe_dir = env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf));
    let beside_exe = exe_dir.iter().flat_map(|dir| [dir.join(EXE_NAME), dir.join("ffmpeg").join("bin").join(EXE_NAME)]);
    let on_path = env::var_os("PATH")
        .into_iter()
        .flat_map(|path| env::split_paths(&path).collect::<Vec<_>>())
        .map(|dir| dir.join(EXE_NAME));
    beside_exe.chain(on_path).find(|p| p.is_file())
}

/// A command with no console window and no stdin.
pub fn command(ffmpeg: &Path) -> Command {
    let mut cmd = Command::new(ffmpeg);
    cmd.creation_flags(CREATE_NO_WINDOW).stdin(Stdio::null());
    cmd
}

/// Spawns the process and puts it into the app-wide kill-on-close Job Object.
pub fn spawn(cmd: &mut Command) -> io::Result<Child> {
    let child = cmd.spawn()?;
    match job() {
        // SAFETY: both handles are valid; the process handle stays open while `child` lives.
        Some(job) => {
            if let Err(e) = unsafe { AssignProcessToJobObject(job, HANDLE(child.as_raw_handle())) } {
                log::warn!("cannot assign ffmpeg to the job object: {e}");
            }
        }
        None => log::warn!("job object unavailable; ffmpeg may outlive the app"),
    }
    Ok(child)
}

/// Process-wide job that kills its processes when the last handle closes (i.e. when the app exits).
fn job() -> Option<HANDLE> {
    static JOB: OnceLock<Option<isize>> = OnceLock::new();
    let raw = *JOB.get_or_init(|| {
        // SAFETY: plain Win32 calls with valid arguments; the handle is intentionally never closed.
        unsafe {
            let job = CreateJobObjectW(None, None).ok()?;
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
            .ok()?;
            Some(job.0 as isize)
        }
    });
    raw.map(|h| HANDLE(h as *mut _))
}

/// Waits up to `timeout`; `Ok(None)` means the process is still running.
pub fn wait_timeout(child: &mut Child, timeout: Duration) -> io::Result<Option<ExitStatus>> {
    let ms = timeout.as_millis().min(u32::MAX as u128 - 1) as u32;
    // SAFETY: the process handle is valid while `child` lives.
    let r = unsafe { WaitForSingleObject(HANDLE(child.as_raw_handle()), ms) };
    if r == WAIT_OBJECT_0 || r == WAIT_TIMEOUT { child.try_wait() } else { Err(io::Error::last_os_error()) }
}

/// Result of a short ffmpeg probe run.
pub struct Probe {
    pub success: bool,
    /// Combined stdout and stderr.
    pub output: String,
}

/// Runs ffmpeg with `args`, killing it after `timeout`. Output goes to a temp file so no reader
/// thread is needed. `None` if the process could not be started.
pub fn probe(ffmpeg: &Path, args: &[&str], timeout: Duration) -> Option<Probe> {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let dir = paths::temp_dir();
    fs::create_dir_all(&dir).ok()?;
    let out_path = dir.join(format!("probe-{}-{}.txt", std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed)));
    let file = File::create(&out_path).ok()?;

    let mut cmd = command(ffmpeg);
    cmd.args(args).stdout(file.try_clone().ok()?).stderr(file);
    let mut child = match spawn(&mut cmd) {
        Ok(child) => child,
        Err(e) => {
            log::warn!("cannot run ffmpeg probe: {e}");
            let _ = fs::remove_file(&out_path);
            return None;
        }
    };
    let success = match wait_timeout(&mut child, timeout) {
        Ok(Some(status)) => status.success(),
        Ok(None) => {
            log::warn!("ffmpeg probe timed out: {args:?}");
            let _ = child.kill();
            let _ = child.wait();
            false
        }
        Err(e) => {
            log::warn!("ffmpeg probe wait failed: {e}");
            let _ = child.kill();
            let _ = child.wait();
            false
        }
    };
    let output = fs::read(&out_path).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    let _ = fs::remove_file(&out_path);
    Some(Probe { success, output })
}

/// Short human-readable reason taken from the end of an ffmpeg log file.
pub fn failure_reason(log_path: &Path) -> String {
    let text = fs::read(log_path).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    reason_from_log(&text)
}

/// Generic wrapper lines ffmpeg prints after the real cause.
const NOISE: [&str; 10] = [
    "conversion failed",
    "terminating thread",
    "task finished",
    "error sending frames",
    "nothing was written",
    "could not open encoder",
    "error while opening encoder",
    "error opening output",
    "error opening input",
    "error opening filters",
];

/// Known root causes, matched anywhere in the log (lowercase needle, friendly message).
const KNOWN: [(&str, &str); 3] = [
    ("frame dimension less than the minimum", "The selected region is too small for hardware encoding"),
    ("extends outside window area", "The selected region is outside the screen"),
    ("openencodesessionex failed", "The GPU encoder could not start (too many active encoding sessions?)"),
];

fn reason_from_log(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    if let Some((_, message)) = KNOWN.iter().find(|(needle, _)| lower.contains(needle)) {
        return (*message).into();
    }
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let is_noise = |l: &&str| NOISE.iter().any(|n| l.to_ascii_lowercase().contains(n));
    let Some(line) = lines.iter().rev().find(|l| !is_noise(l)).or(lines.last()) else {
        return "FFmpeg stopped unexpectedly".into();
    };
    // Drop the "[h264_nvenc @ 000001...] " context prefix.
    let line = match line.strip_prefix('[').and_then(|r| r.split_once("] ")) {
        Some((_, rest)) => rest,
        None => line,
    };
    let line: String = line.chars().take(160).collect();
    format!("FFmpeg error: {line}")
}
