/*
 * Author: Jamius Siam
 * Since: 31/10/2026
 */

//! Manual verification tool.
//!
//! `record_demo X Y W H SECS mp4|webm|gif [--settings JSON] [--ffmpeg PATH] [--cancel] [--name NAME]`
//! `             [--scale 0.5|0.75|1] [--force-caps] [--no-dda] [--no-nvenc] [--cancel-processing]`
//! `record_demo recover [--settings JSON]` runs crash recovery.
//! Records the region, stops (or cancels), prints events and saves the result into `OUT_DIR`.

use std::env;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use framelet_core::settings::OutputFormat;
use framelet_core::{ExportScale, Region, Settings};
use framelet_recorder::{RecorderEvent, RecordingSession, load_or_detect_caps, recover_orphans, save_output};
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};

const OUT_DIR: &str = concat!(
    r"C:\Users\jamius19\AppData\Local\Temp\claude\C--Users-jamius19-projects-framelet",
    r"\fde2937d-38e2-4928-a780-4a624cdf4b09\scratchpad\recorder"
);

fn main() -> anyhow::Result<()> {
    // SAFETY: called once at startup, before any window exists.
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)? };
    framelet_core::logging::init("recorder-demo");

    let args: Vec<String> = env::args().skip(1).collect();
    let flag = |name: &str| args.iter().position(|a| a == name);
    let value = |name: &str| flag(name).and_then(|i| args.get(i + 1)).cloned();
    if args.first().map(String::as_str) == Some("recover") {
        let settings: Settings = match value("--settings") {
            Some(json) => serde_json::from_str(&json)?,
            None => Settings::default(),
        };
        let caps = load_or_detect_caps(&settings, false);
        println!("recovered: {:#?}", recover_orphans(&settings, &caps));
        return Ok(());
    }
    anyhow::ensure!(
        args.len() >= 6,
        "usage: record_demo X Y W H SECS mp4|webm|gif [flags] | recover [--settings JSON]"
    );
    let n = |i: usize| args[i].parse::<i32>();
    let region = Region { x: n(0)?, y: n(1)?, width: n(2)? as u32, height: n(3)? as u32 };
    let secs: f64 = args[4].parse()?;

    let mut settings: Settings = match value("--settings") {
        Some(json) => serde_json::from_str(&json)?,
        None => Settings::default(),
    };
    settings.recording.format = match args[5].as_str() {
        "gif" => OutputFormat::Gif,
        "webm" => OutputFormat::Webm,
        _ => OutputFormat::Mp4,
    };

    let scale = match value("--scale").as_deref() {
        None | Some("1") => ExportScale::Full,
        Some("0.75") => ExportScale::ThreeQuarters,
        Some("0.5") => ExportScale::Half,
        Some(other) => anyhow::bail!("--scale must be 0.5, 0.75 or 1, got {other}"),
    };

    let t = Instant::now();
    let mut caps = load_or_detect_caps(&settings, flag("--force-caps").is_some());
    println!("caps ({:?}): {caps:#?}\nsummary: {}", t.elapsed(), caps.summary());
    caps.ddagrab &= flag("--no-dda").is_none();
    caps.nvenc_h264 &= flag("--no-nvenc").is_none();
    if let Some(path) = value("--ffmpeg") {
        caps.ffmpeg_path = Some(PathBuf::from(path));
    }

    let (tx, rx) = mpsc::channel();
    let started = Instant::now();
    let session = RecordingSession::start(region, scale, &settings, &caps, move |event| {
        let _ = tx.send((started.elapsed(), event));
    });
    let session = match session {
        Ok(s) => s,
        Err(e) => {
            println!("start failed: {e:#}");
            return Ok(());
        }
    };

    std::thread::sleep(Duration::from_secs_f64(secs));
    let t = Instant::now();
    if flag("--cancel").is_some() {
        session.cancel();
        session.cancel();
    } else {
        session.stop();
        session.stop();
    }
    println!("stop/cancel call returned in {:?}", t.elapsed());

    let name = value("--name").unwrap_or_else(|| "demo".into());
    while let Ok((at, event)) = rx.recv() {
        println!("[{at:?}] {event:?}");
        match event {
            RecorderEvent::Processing => {
                if flag("--cancel-processing").is_some() {
                    // Let the post-processing ffmpeg get going, so the kill hits a running encode.
                    std::thread::sleep(Duration::from_secs(2));
                    session.cancel();
                }
            }
            RecorderEvent::Finished(done) => {
                let dest = PathBuf::from(OUT_DIR).join(format!("{name}.{}", done.format.extension()));
                println!("saved: {}", save_output(&done.path, &dest)?.display());
                break;
            }
            RecorderEvent::Canceled => break,
            RecorderEvent::Failed { partial, .. } => {
                if let Some(p) = partial {
                    let ext = p.extension().unwrap_or_default().to_string_lossy().into_owned();
                    let dest = PathBuf::from(OUT_DIR).join(format!("{name}-partial.{ext}"));
                    println!("saved partial: {}", save_output(&p, &dest)?.display());
                }
                break;
            }
        }
    }
    Ok(())
}
