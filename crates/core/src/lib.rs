/*
 * Author: Jamius Siam
 * Since: 31/10/2026
 */

//! Shared types and helpers used by every Framelet crate: settings model, paths,
//! file naming, hotkey parsing, IPC identifiers, encoder capability model and logging.

pub mod caps;
pub mod hotkey;
pub mod ipc;
pub mod logging;
pub mod naming;
pub mod paths;
pub mod region;
pub mod settings;

pub use region::{ExportScale, Region};
pub use settings::Settings;
