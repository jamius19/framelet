/*
 * Author: Jamius Siam
 * Since: 31/10/2026
 */

//! Saving finished recordings (automatic or via the Save As dialog), the exit prompt and
//! Explorer helpers. COM is initialized only for the duration of each call.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use framelet_core::naming;
use framelet_core::settings::{OutputFormat, Settings};
use framelet_recorder::{FinishedRecording, save_output};
use windows::Win32::Foundation::{ERROR_CANCELLED, HWND};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize,
};
use windows::Win32::UI::Controls::{
    TASKDIALOG_BUTTON, TASKDIALOGCONFIG, TDCBF_CANCEL_BUTTON, TDF_ALLOW_DIALOG_CANCELLATION, TDF_USE_COMMAND_LINKS,
    TaskDialogIndirect,
};
use windows::Win32::UI::Shell::Common::{COMDLG_FILTERSPEC, ITEMIDLIST};
use windows::Win32::UI::Shell::{
    FOS_FORCEFILESYSTEM, FOS_OVERWRITEPROMPT, FOS_PATHMUSTEXIST, FileSaveDialog, IFileSaveDialog, IShellItem,
    SHCreateItemFromParsingName, SHOpenFolderAndSelectItems, SHParseDisplayName, SIGDN_FILESYSPATH, ShellExecuteW,
};
use windows::Win32::UI::WindowsAndMessaging::{SW_SHOWNORMAL, SetForegroundWindow};
use windows::core::{HRESULT, HSTRING, PCWSTR, w};

/// Where a finished recording ended up.
pub struct Saved {
    pub path: PathBuf,
    /// The Save As dialog was dismissed, so the recording went to the output folder instead.
    pub dialog_canceled: bool,
}

/// Saves `rec` per settings. Never loses the recording: a canceled dialog falls back to the output folder.
pub fn save_finished(hwnd: HWND, settings: &Settings, rec: &FinishedRecording) -> Result<Saved> {
    let dir = &settings.general.output_dir;
    fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let stem = naming::expand_pattern(&settings.general.file_name_pattern);
    let ext = rec.format.extension();
    let default_dest = dir.join(format!("{stem}.{ext}"));
    let (dest, dialog_canceled) = if settings.general.ask_save_location {
        match ask_path(hwnd, dir, &stem, rec.format) {
            Ok(Some(p)) => (p, false),
            Ok(None) => (default_dest, true),
            Err(e) => {
                log::warn!("save dialog failed: {e}");
                (default_dest, false)
            }
        }
    } else {
        (default_dest, false)
    };
    let path = save_output(&rec.path, &dest).with_context(|| format!("cannot save to {}", dest.display()))?;
    Ok(Saved { path, dialog_canceled })
}

/// Saves a salvaged partial recording as "<name> (partial).<ext>" in the output folder.
pub fn save_partial(settings: &Settings, partial: &Path) -> Result<PathBuf> {
    let dir = &settings.general.output_dir;
    fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let stem = naming::expand_pattern(&settings.general.file_name_pattern);
    let ext = partial.extension().and_then(|e| e.to_str()).unwrap_or("mkv");
    let dest = dir.join(format!("{stem} (partial).{ext}"));
    save_output(partial, &dest).with_context(|| format!("cannot save to {}", dest.display()))
}

struct ComGuard;

impl Drop for ComGuard {
    fn drop(&mut self) {
        // SAFETY: balanced with the successful CoInitializeEx in `ask_path`/`reveal`.
        unsafe { CoUninitialize() }
    }
}

fn ask_path(hwnd: HWND, dir: &Path, stem: &str, format: OutputFormat) -> windows::core::Result<Option<PathBuf>> {
    // SAFETY: COM calls on this thread; objects are released before CoUninitialize (guard declared first).
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        let _com = ComGuard;
        let dialog: IFileSaveDialog = CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER)?;
        let (label, spec) = match format {
            OutputFormat::Mp4 => (w!("MP4 video"), w!("*.mp4")),
            OutputFormat::Webm => (w!("WebM video"), w!("*.webm")),
            OutputFormat::Gif => (w!("GIF image"), w!("*.gif")),
        };
        dialog.SetFileTypes(&[COMDLG_FILTERSPEC { pszName: label, pszSpec: spec }])?;
        dialog.SetDefaultExtension(&HSTRING::from(format.extension()))?;
        dialog.SetFileName(&HSTRING::from(stem))?;
        let dir = dir.to_string_lossy().replace('/', "\\");
        let folder: IShellItem = SHCreateItemFromParsingName(&HSTRING::from(dir), None)?;
        dialog.SetFolder(&folder)?;
        // No overwrite prompt: `save_output` never overwrites, it picks "name (2)" instead.
        dialog.SetOptions((dialog.GetOptions()? & !FOS_OVERWRITEPROMPT) | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST)?;
        let _ = SetForegroundWindow(hwnd);
        if let Err(e) = dialog.Show(Some(hwnd)) {
            return if e.code() == HRESULT::from_win32(ERROR_CANCELLED.0) { Ok(None) } else { Err(e) };
        }
        let name = dialog.GetResult()?.GetDisplayName(SIGDN_FILESYSPATH)?;
        let path = PathBuf::from(name.to_string()?);
        CoTaskMemFree(Some(name.0 as *const _));
        Ok(Some(path))
    }
}

pub enum ExitChoice {
    StopAndSave,
    Discard,
    Cancel,
}

pub fn ask_exit(hwnd: HWND) -> ExitChoice {
    const STOP: i32 = 100;
    const DISCARD: i32 = 101;
    let buttons = [
        TASKDIALOG_BUTTON { nButtonID: STOP, pszButtonText: w!("Stop and save") },
        TASKDIALOG_BUTTON { nButtonID: DISCARD, pszButtonText: w!("Discard recording") },
    ];
    let config = TASKDIALOGCONFIG {
        cbSize: size_of::<TASKDIALOGCONFIG>() as u32,
        hwndParent: hwnd,
        dwFlags: TDF_ALLOW_DIALOG_CANCELLATION | TDF_USE_COMMAND_LINKS,
        dwCommonButtons: TDCBF_CANCEL_BUTTON,
        pszWindowTitle: w!("Framelet"),
        pszMainInstruction: w!("A recording is in progress"),
        pszContent: w!("Stop and save it before exiting, or discard it?"),
        cButtons: buttons.len() as u32,
        pButtons: buttons.as_ptr(),
        nDefaultButton: STOP,
        ..Default::default()
    };
    let mut pressed = 0;
    // SAFETY: `config` and `buttons` outlive the call.
    let shown = unsafe {
        let _ = SetForegroundWindow(hwnd);
        TaskDialogIndirect(&config, Some(&mut pressed), None, None)
    };
    match (shown, pressed) {
        (Ok(()), STOP) => ExitChoice::StopAndSave,
        (Ok(()), DISCARD) => ExitChoice::Discard,
        _ => ExitChoice::Cancel,
    }
}

pub fn open_folder(dir: &Path) {
    let _ = fs::create_dir_all(dir);
    // SAFETY: plain shell call with owned strings.
    unsafe {
        ShellExecuteW(None, w!("open"), &HSTRING::from(dir.as_os_str()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

/// Opens Explorer on the file's folder with the file selected.
pub fn reveal(file: &Path) {
    // SAFETY: COM calls on this thread; the PIDL is freed and COM released before returning.
    let revealed = unsafe {
        if CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_err() {
            false
        } else {
            let _com = ComGuard;
            let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
            let parsed = SHParseDisplayName(&HSTRING::from(file.as_os_str()), None, &mut pidl, 0, None).is_ok();
            let ok = parsed && SHOpenFolderAndSelectItems(pidl, None, 0).is_ok();
            if !pidl.is_null() {
                CoTaskMemFree(Some(pidl as *const _));
            }
            ok
        }
    };
    if !revealed {
        log::warn!("cannot select {} in Explorer", file.display());
        if let Some(dir) = file.parent() {
            open_folder(dir);
        }
    }
}
