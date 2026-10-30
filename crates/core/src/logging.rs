/*
 * Author: Jamius Siam
 * Since: 31/10/2026
 */

//! Minimal file logger for the `log` facade: `%LOCALAPPDATA%\Framelet\logs\<name>.log`.
//! The file is truncated on start when it exceeds 1 MiB. Debug builds also print to stderr.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use log::{LevelFilter, Log, Metadata, Record};

use crate::paths;

const MAX_LOG_BYTES: u64 = 1024 * 1024;

struct FileLogger {
    file: Mutex<File>,
}

impl Log for FileLogger {
    fn enabled(&self, _: &Metadata) -> bool {
        true
    }

    fn log(&self, record: &Record) {
        let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let line = format!("[{secs}] {:<5} {}: {}\n", record.level(), record.target(), record.args());
        if cfg!(debug_assertions) {
            eprint!("{line}");
        }
        if let Ok(mut f) = self.file.lock() {
            let _ = f.write_all(line.as_bytes());
        }
    }

    fn flush(&self) {
        if let Ok(mut f) = self.file.lock() {
            let _ = f.flush();
        }
    }
}

/// Installs the logger. Failure to open the log file is not fatal: logging is simply disabled.
pub fn init(name: &str) {
    let dir = paths::logs_dir();
    let path = dir.join(format!("{name}.log"));
    let _ = fs::create_dir_all(&dir);
    let too_big = fs::metadata(&path).map(|m| m.len() > MAX_LOG_BYTES).unwrap_or(false);
    let file = OpenOptions::new().create(true).append(!too_big).write(true).truncate(too_big).open(&path);
    if let Ok(file) = file {
        let level = if cfg!(debug_assertions) { LevelFilter::Debug } else { LevelFilter::Info };
        if log::set_boxed_logger(Box::new(FileLogger { file: Mutex::new(file) })).is_ok() {
            log::set_max_level(level);
        }
    }
}
