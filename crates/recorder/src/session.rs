/*
 * Author: Jamius Siam
 * Since: 31/10/2026
 */

//! One recording: FFmpeg capture process, worker thread, post-processing and events.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Write};
use std::os::windows::io::AsRawHandle;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, bail};
use framelet_core::caps::EncoderCaps;
use framelet_core::settings::{Gif, OutputFormat};
use framelet_core::{Region, Settings, paths};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::System::Threading::{CreateEventW, INFINITE, ResetEvent, SetEvent, WaitForMultipleObjects};

use crate::plan::{self, CaptureSpec, Plan};
use crate::{FinishedRecording, RecorderEvent, ffmpeg, post};

/// How long ffmpeg gets to finalize the file after `q` before it is killed.
const STOP_GRACE: Duration = Duration::from_secs(10);
/// An ffmpeg exit with an error within this time after start triggers the next fallback plan.
const EARLY_FAILURE: Duration = Duration::from_secs(2);
/// A capture file smaller than this holds no usable video.
const MIN_CAPTURE_BYTES: u64 = 1024;

const REQ_NONE: u8 = 0;
const REQ_STOP: u8 = 1;
const REQ_CANCEL: u8 = 2;

/// Stop/cancel requests from the owner to the worker. Values only ever increase.
struct Control {
    request: AtomicU8,
    event: Event,
}

impl Control {
    fn request(&self, level: u8) {
        self.request.fetch_max(level, Ordering::SeqCst);
        // SAFETY: the event handle is valid until `Control` is dropped.
        if let Err(e) = unsafe { SetEvent(self.event.0) } {
            log::warn!("SetEvent failed: {e}");
        }
    }

    fn level(&self) -> u8 {
        self.request.load(Ordering::SeqCst)
    }
}

/// Manual-reset event used to wake the worker out of its process wait.
struct Event(HANDLE);

// SAFETY: event handles may be used from any thread.
unsafe impl Send for Event {}
unsafe impl Sync for Event {}

impl Drop for Event {
    fn drop(&mut self) {
        // SAFETY: the handle is owned by this struct.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// One recording. Dropping an active session cancels it.
pub struct RecordingSession {
    control: Arc<Control>,
}

impl RecordingSession {
    pub fn start(
        region: Region,
        settings: &Settings,
        caps: &EncoderCaps,
        on_event: impl Fn(RecorderEvent) + Send + 'static,
    ) -> anyhow::Result<RecordingSession> {
        if region.width < 2 || region.height < 2 {
            bail!("The selected region is too small to record");
        }
        let ffmpeg = caps
            .ffmpeg_path
            .clone()
            .filter(|p| p.is_file())
            .or_else(|| ffmpeg::locate_ffmpeg(settings))
            .context("FFmpeg was not found")?;

        let format = settings.recording.format;
        let gif = format == OutputFormat::Gif;
        let spec = CaptureSpec {
            region,
            fps: if gif { settings.gif.fps } else { settings.video.fps },
            cursor: settings.recording.show_cursor,
            quality: settings.video.quality,
            preset: settings.video.preset,
            intermediate: gif,
            max_secs: Some(settings.gif.max_duration_secs).filter(|&s| gif && s > 0),
        };
        let chain = plan::plan_chain(&region, settings, caps);

        let dir = paths::temp_dir();
        fs::create_dir_all(&dir).context("cannot create the temp directory")?;
        let millis = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        let stem = format!("rec-{}-{millis}", std::process::id());

        let worker = Worker {
            control: Arc::new(Control { request: AtomicU8::new(REQ_NONE), event: new_event()? }),
            ffmpeg,
            chain: chain.plans,
            warning: chain.warning,
            spec,
            gif: settings.gif.clone(),
            format,
            mkv: dir.join(format!("{stem}.mkv")),
            log: dir.join(format!("{stem}.log")),
            final_path: dir.join(format!("{stem}.{}", format.extension())),
            on_event: Box::new(on_event),
        };
        let first = worker.spawn_capture(0).map_err(|e| {
            worker.remove_files();
            anyhow::anyhow!("Cannot start FFmpeg ({}): {e}", worker.ffmpeg.display())
        })?;

        let control = worker.control.clone();
        // If the thread cannot be spawned, ffmpeg is still killed by the Job Object when the app exits.
        thread::Builder::new()
            .name("recorder".into())
            .spawn(move || worker.run(first))
            .context("cannot start the recording thread")?;
        Ok(RecordingSession { control })
    }

    /// Gracefully stops capture and finalizes the output. Non-blocking, idempotent.
    pub fn stop(&self) {
        self.control.request(REQ_STOP);
    }

    /// Stops capture and discards everything. Non-blocking, idempotent.
    pub fn cancel(&self) {
        self.control.request(REQ_CANCEL);
    }
}

impl Drop for RecordingSession {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn new_event() -> anyhow::Result<Event> {
    // SAFETY: plain Win32 call; the handle is owned by the returned `Event`.
    let handle = unsafe { CreateEventW(None, true, false, None) }.context("cannot create an event")?;
    Ok(Event(handle))
}

enum Capture {
    /// Capture finished with the plan at this index.
    Done(usize),
    Canceled,
    Failed(String),
}

enum Post {
    Done,
    Canceled,
    Failed(String),
}

struct Worker {
    control: Arc<Control>,
    ffmpeg: PathBuf,
    chain: Vec<Plan>,
    warning: Option<String>,
    spec: CaptureSpec,
    gif: Gif,
    format: OutputFormat,
    mkv: PathBuf,
    log: PathBuf,
    final_path: PathBuf,
    on_event: Box<dyn Fn(RecorderEvent) + Send>,
}

impl Worker {
    fn emit(&self, event: RecorderEvent) {
        (self.on_event)(event);
    }

    fn spawn_capture(&self, attempt: usize) -> io::Result<Child> {
        let plan = &self.chain[attempt];
        let args = plan::capture_args(plan, &self.spec, &self.mkv);
        log::info!("recording with {}: ffmpeg {}", plan.describe(), display_args(&args));
        let mut cmd = ffmpeg::command(&self.ffmpeg);
        cmd.args(&args).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(File::create(&self.log)?);
        ffmpeg::spawn(&mut cmd)
    }

    /// A panic still ends in a terminal event, otherwise the owner would wait in "processing" forever.
    fn run(self, mut child: Child) {
        if panic::catch_unwind(AssertUnwindSafe(|| self.process(&mut child))).is_err() {
            let _ = child.kill();
            let _ = child.wait();
            let partial = self.salvage(false);
            self.failed("Internal error in the recorder".into(), partial);
        }
    }

    fn process(&self, child: &mut Child) {
        match self.capture(child) {
            Capture::Canceled => self.canceled(),
            Capture::Failed(message) => {
                let partial = self.salvage(true);
                self.failed(message, partial);
            }
            Capture::Done(attempt) => self.finish(attempt),
        }
    }

    /// Runs the capture, retrying with the next plan on an early failure.
    fn capture(&self, child: &mut Child) -> Capture {
        let mut attempt = 0;
        loop {
            let started = Instant::now();
            let status = match self.wait_child(child, true) {
                Ok(status) => status,
                Err(e) => {
                    let _ = child.kill();
                    return Capture::Failed(format!("Lost track of FFmpeg: {e}"));
                }
            };
            let level = self.control.level();
            if level == REQ_CANCEL {
                return Capture::Canceled;
            }
            let has_data = file_len(&self.mkv) >= MIN_CAPTURE_BYTES;
            if has_data && (status.success() || level == REQ_STOP) {
                if !status.success() {
                    log::warn!("ffmpeg ended with {status} after stop; keeping the capture");
                }
                return Capture::Done(attempt);
            }

            let reason = ffmpeg::failure_reason(&self.log);
            log::warn!("ffmpeg ({}) failed with {status}: {reason}", self.chain[attempt].describe());
            if level == REQ_NONE && started.elapsed() < EARLY_FAILURE && attempt + 1 < self.chain.len() {
                attempt += 1;
                match self.spawn_capture(attempt) {
                    Ok(next) => {
                        *child = next;
                        continue;
                    }
                    Err(e) => return Capture::Failed(format!("Cannot start FFmpeg: {e}")),
                }
            }
            return Capture::Failed(reason);
        }
    }

    /// Waits for the process. While `graceful`, a stop request sends `q` and allows [`STOP_GRACE`]
    /// before killing; a cancel request kills immediately.
    fn wait_child(&self, child: &mut Child, graceful: bool) -> io::Result<ExitStatus> {
        let process = HANDLE(child.as_raw_handle());
        let mut deadline: Option<Instant> = None;
        loop {
            let ms = deadline.map_or(INFINITE, |d| d.saturating_duration_since(Instant::now()).as_millis() as u32);
            // SAFETY: both handles are valid for the duration of the call.
            let waited = unsafe { WaitForMultipleObjects(&[process, self.control.event.0], false, ms) };
            if waited == WAIT_OBJECT_0 {
                break;
            } else if waited.0 == WAIT_OBJECT_0.0 + 1 {
                // SAFETY: valid event handle.
                let _ = unsafe { ResetEvent(self.control.event.0) };
                match self.control.level() {
                    REQ_CANCEL => {
                        let _ = child.kill();
                        break;
                    }
                    REQ_STOP if graceful && deadline.is_none() => {
                        send_quit(child);
                        deadline = Some(Instant::now() + STOP_GRACE);
                    }
                    _ => {}
                }
            } else if waited == WAIT_TIMEOUT {
                log::warn!("ffmpeg did not stop within {STOP_GRACE:?}; killing it");
                let _ = child.kill();
                break;
            } else {
                return Err(io::Error::last_os_error());
            }
        }
        child.wait()
    }

    /// Converts the capture into the final file.
    fn finish(&self, attempt: usize) {
        self.emit(RecorderEvent::Processing);
        let result = match self.format {
            OutputFormat::Mp4 => self.run_post(post::remux_args(&self.mkv, &self.final_path), &self.final_path, true),
            OutputFormat::Gif => {
                let (width, _) = self.chain[attempt].frame_size(&self.spec.region);
                self.run_post(post::gif_args(&self.mkv, &self.final_path, width, &self.gif), &self.final_path, true)
            }
        };
        match result {
            Post::Canceled => self.canceled(),
            Post::Done if self.control.level() == REQ_CANCEL => self.canceled(),
            Post::Done => {
                let _ = fs::remove_file(&self.mkv);
                let _ = fs::remove_file(&self.log);
                let warning = self.warning.clone().or_else(|| {
                    (attempt > 0).then(|| format!("Capture fell back to {}", self.chain[attempt].describe()))
                });
                self.emit(RecorderEvent::Finished(FinishedRecording {
                    path: self.final_path.clone(),
                    format: self.format,
                    warning,
                }));
            }
            Post::Failed(message) => {
                let partial = self.salvage(self.format == OutputFormat::Gif);
                self.failed(message, partial);
            }
        }
    }

    /// Runs a post-processing ffmpeg with stderr in the log file (or discarded).
    fn run_post(&self, args: Vec<OsString>, output: &Path, keep_log: bool) -> Post {
        let stderr = if keep_log {
            File::create(&self.log).map(Stdio::from).unwrap_or_else(|_| Stdio::null())
        } else {
            Stdio::null()
        };
        log::info!("post-processing: ffmpeg {}", display_args(&args));
        let mut cmd = ffmpeg::command(&self.ffmpeg);
        cmd.args(&args).stdout(Stdio::null()).stderr(stderr);
        let mut child = match ffmpeg::spawn(&mut cmd) {
            Ok(child) => child,
            Err(e) => return Post::Failed(format!("Cannot start FFmpeg: {e}")),
        };
        match self.wait_child(&mut child, false) {
            Ok(_) if self.control.level() == REQ_CANCEL => Post::Canceled,
            Ok(status) if status.success() && file_len(output) > 0 => Post::Done,
            Ok(status) => {
                let reason = ffmpeg::failure_reason(&self.log);
                log::warn!("post-processing failed with {status}: {reason}");
                Post::Failed(reason)
            }
            Err(e) => {
                let _ = child.kill();
                Post::Failed(format!("Lost track of FFmpeg: {e}"))
            }
        }
    }

    /// Returns a playable file for a failed recording: the capture remuxed to MP4 when `remux`,
    /// otherwise the Matroska capture itself. `None` if nothing usable was captured.
    fn salvage(&self, remux: bool) -> Option<PathBuf> {
        if file_len(&self.mkv) < MIN_CAPTURE_BYTES {
            let _ = fs::remove_file(&self.mkv);
            return None;
        }
        if remux {
            let mp4 = self.mkv.with_extension("mp4");
            if let Post::Done = self.run_post(post::remux_args(&self.mkv, &mp4), &mp4, false) {
                let _ = fs::remove_file(&self.mkv);
                return Some(mp4);
            }
            let _ = fs::remove_file(&mp4);
        }
        Some(self.mkv.clone())
    }

    fn failed(&self, message: String, partial: Option<PathBuf>) {
        log::error!("recording failed: {message} (partial: {partial:?}, log: {})", self.log.display());
        if let Ok(text) = fs::read_to_string(&self.log) {
            log::error!("ffmpeg log:\n{}", text.trim_end());
        }
        if partial.as_ref() != Some(&self.final_path) {
            let _ = fs::remove_file(&self.final_path);
        }
        self.emit(RecorderEvent::Failed { message, partial });
    }

    fn canceled(&self) {
        self.remove_files();
        self.emit(RecorderEvent::Canceled);
    }

    fn remove_files(&self) {
        for path in [&self.mkv, &self.log, &self.final_path, &self.mkv.with_extension("mp4")] {
            let _ = fs::remove_file(path);
        }
    }
}

fn send_quit(child: &mut Child) {
    if let Some(mut stdin) = child.stdin.take()
        && let Err(e) = stdin.write_all(b"q\n").and_then(|_| stdin.flush())
    {
        log::debug!("cannot send q to ffmpeg: {e}");
    }
}

fn file_len(path: &Path) -> u64 {
    fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

fn display_args(args: &[OsString]) -> String {
    args.iter().map(|a| a.to_string_lossy()).collect::<Vec<_>>().join(" ")
}
