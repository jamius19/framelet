/*
 * Author: Jamius Siam
 * Since: 31/10/2026
 */

use std::{fs, io};

use serde::{Deserialize, Serialize};

use crate::paths;

/// A rectangle in physical virtual-desktop pixels (per-monitor DPI aware v2 coordinates).
/// `x`/`y` may be negative for monitors left of / above the primary monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Region {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Region {
    pub fn right(&self) -> i32 {
        self.x + self.width as i32
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.height as i32
    }

    /// True when `self` lies completely inside `outer`.
    pub fn is_inside(&self, outer: &Region) -> bool {
        self.x >= outer.x && self.y >= outer.y && self.right() <= outer.right() && self.bottom() <= outer.bottom()
    }

    /// The region of the last recording, if one was saved and is still readable.
    pub fn load_last() -> Option<Region> {
        let text = fs::read_to_string(paths::last_region_file()).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Remembers this region so the next selection starts from it.
    pub fn save_last(&self) -> io::Result<()> {
        let path = paths::last_region_file();
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(path, serde_json::to_string(self).map_err(io::Error::other)?)
    }
}
