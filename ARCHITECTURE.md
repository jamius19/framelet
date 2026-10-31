# Framelet — Architecture

Lightweight Windows 11 region screen recorder. Rust + windows-rs + external FFmpeg (ddagrab + h264_nvenc).

## Process model

Two executables, deployed side by side with `ffmpeg.exe`:

| Exe | Crate | Role | UI tech |
|---|---|---|---|
| `framelet.exe` | `crates/app` | Always-running tray process: tray icon/menu, global hotkey, selection overlay, countdown, floating controller, recording orchestration, notifications | Raw Win32 (windows-rs), GDI |
| `framelet-settings.exe` | `crates/settings-ui` | Settings window, launched on demand by the tray, exits when closed | Slint 1.18 |

**Why a separate settings process:** winit (Slint's backend) can create its event loop only once per
process, and a GUI toolkit's renderer, font caches and allocator arenas are never fully returned to the
OS after a window closes. Running settings in its own short-lived process means the tray process never
loads Slint at all and idles at a few MB; closing settings frees 100% of its memory.

Communication: settings are persisted to `%APPDATA%\Framelet\settings.json`; the settings process then
posts registered window messages to the tray's hidden window (`framelet_core::ipc`). No sockets, no polling.

## Crates

```
crates/core       framelet-core      settings model + validation + atomic save, paths, file naming,
                                     hotkey parse/format, IPC identifiers, EncoderCaps model, file logger
crates/recorder   framelet-recorder  ffmpeg discovery, capability detection, capture planning (DXGI output
                                     mapping), ffmpeg process lifecycle, remux/scale/WebM/GIF post-processing,
                                     save/move, crash recovery
crates/overlay    framelet-overlay   selection overlay + countdown (layered Win32 windows, nested modal loops),
                                     recording frame (non-modal: red border + optional dim, solid click-through windows)
crates/app        framelet           tray process (bin)
crates/settings-ui framelet-settings settings window (bin)
```

Dependency direction: `app → {recorder, overlay, core}`, `settings-ui → {recorder, core}`,
`recorder → core`, `overlay → core`. Public APIs of `recorder` and `overlay` are defined in their
`lib.rs` and must not change without updating this document.

## Threading (tray process)

- **Main thread**: Win32 message loop. Hidden top-level window (`ipc::TRAY_WINDOW_CLASS`) owns the tray
  icon, hotkey, timers, and receives worker events via `PostMessageW(WM_APP + n, 0, Box<RecorderEvent> as LPARAM)`.
  Overlay selection and countdown run as nested modal loops on this thread (like `DialogBox`).
- **Recording worker** (one per recording, exits when the recording is finalized): waits on the ffmpeg
  process, runs post-processing, emits `RecorderEvent`s.
- **Startup worker** (short-lived): capability cache refresh + orphan recovery.
- No polling. The controller's 1-second timer exists only while recording.

## Recording pipeline

1. Region from overlay: physical virtual-desktop pixels (process is Per-Monitor-DPI-Aware v2).
2. Plan:
   - Region inside one monitor → `ddagrab` (D3D11 Desktop Duplication) on that DXGI output, frames stay
     on the GPU and go straight into `h264_nvenc` (NVENC accepts D3D11 BGRA frames and converts to YUV on GPU).
   - Region spans monitors, or ddagrab unavailable → `gdigrab` + `h264_nvenc`.
   - No NVENC (or Encoder = Software) → `libx264`.
   - Odd dimensions: NVENC encodes exact odd sizes (H.264 SPS cropping, verified with FFmpeg 7.1).
     libx264 + yuv420p needs even sizes → crop 1 px (never scale/stretch).
3. Capture writes a **Matroska temp file** in `%LOCALAPPDATA%\Framelet\temp` (playable even if the process
   is killed). FFmpeg stderr goes to a log file next to it (no pipe-draining threads). FFmpeg is placed in
   a Job Object with KILL_ON_JOB_CLOSE so it never outlives the app.
4. Stop = write `q` to ffmpeg stdin (graceful), kill after timeout. Cancel = kill + delete.
5. Post-process, per the export scale picked on the selection toolbar (0.5× / 0.75× / 1×, default 1×, never
   persisted; output sizes are rounded to even numbers):
   - MP4 at 1× → stream-copy remux with `+faststart`.
   - MP4 below 1× → the capture is a high-quality intermediate (like GIF), then re-encoded with
     `scale=W:H:flags=lanczos` (same encoder family as the capture: NVENC, or libx264 `medium` when software
     or when the scaled size is below the NVENC minimum), user's quality mapping, `+faststart`. Live capture
     stays full-resolution on the GPU.
   - WebM → the capture is a high-quality intermediate (like GIF), then encoded on the CPU with `libvpx-vp9`
     (`-crf` from the quality setting: 44 at 0, 24 at 75, 18 at 100; `-deadline good`, `-cpu-used` 4 / 3 / 1 for
     Fast / Balanced / High Quality, `-row-mt 1`, tile columns from the width, no audio). The export scale applies
     as `scale=W:H:flags=lanczos`; at 1× the size is kept, odd or not. The colour matrix of the capture
     encoder is preserved in the tags (`bt470bg` after NVENC, `bt709` after x264), like the scaled MP4.
   - GIF → `palettegen`/`paletteuse` two-stage filter from the temp video (high-quality intermediate); the
     export scale multiplies the configured GIF scale (never above the region size).
6. Save: auto to output dir with pattern name (never overwrites), or Save As dialog. Dialog canceled →
   saved to the output dir anyway and the user is told (no silent loss).
7. Crash recovery: on startup, leftover temp recordings are remuxed into the output dir as "(recovered)".

## Settings model

See `crates/core/src/settings.rs` — the single source of truth (JSON, `#[serde(default)]`, `validate()`).

## Conventions

- Every source file starts with the author header (`Author: Jamius Siam`, `Since: DD/MM/YYYY`).
- Lines ≤ 120 chars.
- `anyhow` for application-level errors, `log` for diagnostics (`framelet_core::logging`).
- `unsafe` blocks get a one-line `// SAFETY:` comment only where the invariant is non-obvious.
