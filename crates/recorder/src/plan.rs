/*
 * Author: Jamius Siam
 * Since: 31/10/2026
 */

//! Capture planning: which grabber and encoder to use for a region, the fallback chain, and the
//! FFmpeg command line for each plan.
//!
//! Colour: NVENC converts BGRA to YUV on the GPU with the BT.601 matrix (verified by decoding a known
//! colour chart), so NVENC output is tagged `bt470bg`. libx264 goes through an explicit BT.709
//! conversion and is tagged `bt709`. Both are limited range, sRGB transfer, BT.709 primaries
//! (stamped on the frames, see [`TAG_FRAMES`]).

use std::ffi::OsString;
use std::path::Path;

use framelet_core::caps::EncoderCaps;
use framelet_core::settings::{EncoderPref, Preset};
use framelet_core::{Region, Settings};

use crate::display::{self, OutputMatch};

/// Smallest frame this GPU's H.264 NVENC accepts (found by testing on an RTX 3050; other GPUs may
/// differ, the automatic fallback covers those).
const NVENC_MIN_WIDTH: u32 = 145;
const NVENC_MIN_HEIGHT: u32 = 49;

/// FFmpeg 7.1 ignores `-color_primaries` / `-color_trc` for frames that carry no tags (gdigrab, and
/// anything after a software conversion), so they are stamped on the frames instead.
pub const TAG_FRAMES: &str = "setparams=color_primaries=bt709:color_trc=iec61966-2-1";

#[derive(Debug, Clone, Copy)]
pub enum Grab {
    /// Desktop Duplication on one output; `x`/`y` are relative to that output.
    Dda { adapter: u32, output: u32, x: i32, y: i32 },
    /// GDI, virtual-desktop coordinates.
    Gdi,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    Nvenc,
    X264,
}

#[derive(Debug, Clone, Copy)]
pub struct Plan {
    pub grab: Grab,
    pub codec: Codec,
}

impl Plan {
    pub fn describe(&self) -> String {
        let grab = match self.grab {
            Grab::Dda { .. } => "ddagrab",
            Grab::Gdi => "gdigrab",
        };
        let codec = match self.codec {
            Codec::Nvenc => "NVENC",
            Codec::X264 => "x264",
        };
        format!("{grab} + {codec}")
    }

    /// Size of the encoded frames. libx264 needs even dimensions, so odd sizes lose one pixel row/column.
    pub fn frame_size(&self, region: &Region) -> (u32, u32) {
        match self.codec {
            Codec::Nvenc => (region.width, region.height),
            Codec::X264 => (region.width & !1, region.height & !1),
        }
    }
}

/// Everything about a capture that does not depend on the plan.
#[derive(Debug, Clone)]
pub struct CaptureSpec {
    pub region: Region,
    pub fps: u32,
    pub cursor: bool,
    /// 0..=100
    pub quality: u32,
    pub preset: Preset,
    /// GIF, WebM and scaled MP4 intermediates use near-lossless quality and ignore `quality`/`preset`.
    pub intermediate: bool,
    pub max_secs: Option<u32>,
}

impl CaptureSpec {
    pub fn nvenc_cq(&self) -> u32 {
        if self.intermediate { 15 } else { 35 - (self.quality * 18 + 50) / 100 }
    }

    pub fn x264_crf(&self) -> u32 {
        if self.intermediate { 14 } else { 30 - (self.quality * 14 + 50) / 100 }
    }

    /// libvpx-vp9 CRF (0..=63) for the WebM post-processing encode: 44 at quality 0, 24 at the default 75,
    /// 18 at 100. Measured on sharp text against the near-lossless intermediate, VMAF is at least that of the
    /// NVENC MP4 at the same quality setting, in a much smaller file.
    pub fn vp9_crf(&self) -> u32 {
        44 - (self.quality * 26 + 50) / 100
    }

    /// libvpx-vp9 `-cpu-used` (lower is slower and compresses better). FFmpeg 7.1's libvpx is both slower and
    /// larger at 5 and 6 than at 4 (measured), so Fast stops at 4.
    pub fn vp9_cpu_used(&self) -> &'static str {
        match self.preset {
            Preset::Fast => "4",
            Preset::Balanced => "3",
            Preset::Quality => "1",
        }
    }

    pub fn nvenc_preset(&self) -> &'static str {
        match (self.intermediate, self.preset) {
            (true, _) | (_, Preset::Balanced) => "p4",
            (_, Preset::Fast) => "p2",
            (_, Preset::Quality) => "p6",
        }
    }

    fn x264_preset(&self) -> &'static str {
        match (self.intermediate, self.preset) {
            (true, _) => "veryfast",
            (_, Preset::Fast) => "ultrafast",
            (_, Preset::Balanced) => "superfast",
            (_, Preset::Quality) => "veryfast",
        }
    }
}

/// Encoder for scaling a capture made with `captured` down to `size`: the same family, unless NVENC cannot
/// take a frame that small.
pub fn scaled_codec(captured: Codec, size: (u32, u32)) -> Codec {
    if captured == Codec::Nvenc && size.0 >= NVENC_MIN_WIDTH && size.1 >= NVENC_MIN_HEIGHT {
        Codec::Nvenc
    } else {
        Codec::X264
    }
}

/// Ordered plans to try (first is preferred) plus a note for the user when the preferred encoder
/// could not be honoured.
pub struct PlanChain {
    pub plans: Vec<Plan>,
    pub warning: Option<String>,
}

pub fn plan_chain(region: &Region, settings: &Settings, caps: &EncoderCaps) -> PlanChain {
    let big_enough = region.width >= NVENC_MIN_WIDTH && region.height >= NVENC_MIN_HEIGHT;
    let mut warning = None;
    let nvenc = match settings.video.encoder {
        EncoderPref::Software => false,
        EncoderPref::Auto => caps.nvenc_h264 && big_enough,
        EncoderPref::Nvenc if !caps.nvenc_h264 => {
            warning = Some("NVENC is unavailable \u{2014} used software encoding".to_string());
            false
        }
        EncoderPref::Nvenc if !big_enough => {
            warning = Some("Region is too small for NVENC \u{2014} used software encoding".to_string());
            false
        }
        EncoderPref::Nvenc => true,
    };

    let dda = if caps.ddagrab { display::find_output(region) } else { None };
    let dda_grab =
        dda.as_ref().map(|m: &OutputMatch| Grab::Dda { adapter: m.adapter, output: m.output, x: m.x, y: m.y });

    let mut plans = Vec::new();
    if nvenc {
        // NVENC cannot take D3D11 frames from another vendor's adapter.
        if let (Some(grab), Some(m)) = (dda_grab, &dda)
            && m.nvidia
        {
            plans.push(Plan { grab, codec: Codec::Nvenc });
        }
        plans.push(Plan { grab: Grab::Gdi, codec: Codec::Nvenc });
    } else if let Some(grab) = dda_grab {
        plans.push(Plan { grab, codec: Codec::X264 });
    }
    plans.push(Plan { grab: Grab::Gdi, codec: Codec::X264 });
    PlanChain { plans, warning }
}

/// FFmpeg arguments that capture `spec.region` with `plan` into the Matroska file `out`.
pub fn capture_args(plan: &Plan, spec: &CaptureSpec, out: &Path) -> Vec<OsString> {
    let region = &spec.region;
    let (fw, fh) = plan.frame_size(region);
    let cursor = u8::from(spec.cursor);
    let fps = spec.fps;
    let mut args: Vec<OsString> = Vec::new();
    let mut push = |items: &[&str]| args.extend(items.iter().map(OsString::from));

    push(&["-hide_banner", "-nostats", "-loglevel", "warning", "-y"]);

    // x264 needs yuv420p with an explicit BT.709 conversion (and even dimensions).
    let mut convert = Vec::new();
    if plan.codec == Codec::X264 {
        if (fw, fh) != (region.width, region.height) {
            convert.push(format!("crop={fw}:{fh}:0:0"));
        }
        convert.push("scale=out_color_matrix=bt709:out_range=tv".to_string());
        convert.push(TAG_FRAMES.to_string());
        convert.push("format=yuv420p".to_string());
    }

    match plan.grab {
        Grab::Dda { adapter, output, x, y } => {
            push(&["-init_hw_device", &format!("d3d11va=dd:{adapter}"), "-filter_hw_device", "dd"]);
            let (w, h) = (region.width, region.height);
            let mut graph = format!(
                "ddagrab=output_idx={output}:framerate={fps}:draw_mouse={cursor}\
                 :offset_x={x}:offset_y={y}:video_size={w}x{h}"
            );
            if plan.codec == Codec::X264 {
                graph.push_str(",hwdownload,format=bgra,");
                graph.push_str(&convert.join(","));
            }
            push(&["-filter_complex", &graph]);
        }
        Grab::Gdi => {
            push(&["-f", "gdigrab", "-draw_mouse", &cursor.to_string(), "-framerate", &fps.to_string()]);
            push(&["-offset_x", &region.x.to_string(), "-offset_y", &region.y.to_string()]);
            push(&["-video_size", &format!("{}x{}", region.width, region.height), "-i", "desktop"]);
            let filters = if convert.is_empty() { TAG_FRAMES.to_string() } else { convert.join(",") };
            push(&["-vf", &filters]);
        }
    }

    if let Some(secs) = spec.max_secs {
        push(&["-t", &secs.to_string()]);
    }

    match plan.codec {
        Codec::Nvenc => {
            push(&["-c:v", "h264_nvenc", "-preset", spec.nvenc_preset(), "-tune", "hq", "-rc", "vbr"]);
            push(&["-cq", &spec.nvenc_cq().to_string(), "-b:v", "0", "-profile:v", "high"]);
            push(&["-g", &(fps * 2).to_string(), "-colorspace", "bt470bg"]);
        }
        Codec::X264 => {
            push(&["-c:v", "libx264", "-preset", spec.x264_preset(), "-crf", &spec.x264_crf().to_string()]);
            push(&["-pix_fmt", "yuv420p", "-g", &(fps * 2).to_string(), "-colorspace", "bt709"]);
        }
    }
    // Write every packet through and use short clusters, so a killed or crashed capture loses at most
    // about a second (otherwise ffmpeg keeps the first seconds in its I/O buffer).
    push(&["-color_range", "tv", "-flush_packets", "1", "-cluster_time_limit", "1000"]);
    args.push(out.as_os_str().to_owned());
    args
}
