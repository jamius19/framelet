/*
 * Author: Jamius Siam
 * Since: 31/10/2026
 */

//! Notifications are tray balloons (Windows 11 shows them as toasts). A balloon may carry a file
//! that is revealed in Explorer when the user clicks it.

use std::cell::RefCell;
use std::path::PathBuf;

use crate::tray;

thread_local! {
    static CLICK_TARGET: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

pub fn info(text: &str, target: Option<PathBuf>) {
    show(false, "Framelet", text, target);
}

/// Errors are always shown, whatever the notification setting is.
pub fn error(text: &str) {
    log::error!("{text}");
    show(true, "Framelet", text, None);
}

pub fn error_with_file(text: &str, file: PathBuf) {
    log::error!("{text}");
    show(true, "Framelet", text, Some(file));
}

fn show(is_error: bool, title: &str, text: &str, target: Option<PathBuf>) {
    CLICK_TARGET.with_borrow_mut(|t| *t = target);
    tray::balloon(title, text, is_error);
}

/// File to reveal for the balloon the user just clicked.
pub fn take_click_target() -> Option<PathBuf> {
    CLICK_TARGET.with_borrow_mut(Option::take)
}
