/*
 * Author: Jamius Siam
 * Since: 31/10/2026
 */

//! Post-processing command lines: MKV to MP4 remux, scaled MP4 re-encode, MKV to WebM (VP9) and MKV to GIF.

use std::ffi::OsString;
use std::path::Path;
use std::thread;

use framelet_core::ExportScale;
use framelet_core::settings::{Dither, Gif, GifQuality, GifScale};

use crate::plan::{CaptureSpec, Codec, TAG_FRAMES};

const BASE: [&str; 6] = ["-hide_banner", "-nostats", "-loglevel", "warning", "-y", "-nostdin"];

fn base(input: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = BASE.iter().map(OsString::from).collect();
    args.push("-i".into());
    args.push(input.as_os_str().to_owned());
    args
}

/// Stream copy into MP4 with the moov atom up front.
pub fn remux_args(input: &Path, output: &Path) -> Vec<OsString> {
    let mut args = base(input);
    args.extend(["-c", "copy", "-movflags", "+faststart"].map(OsString::from));
    args.push(output.as_os_str().to_owned());
    args
}

/// Re-encode into MP4 at `size` (Lanczos) for a capture made with `captured`, using the encoder family
/// `codec` and the user's quality settings (`spec` must not be an intermediate). Scaling keeps the colour
/// matrix of the data, so the tag follows the capture encoder, not `codec`.
pub fn scaled_mp4_args(
    input: &Path,
    output: &Path,
    captured: Codec,
    codec: Codec,
    size: (u32, u32),
    spec: &CaptureSpec,
) -> Vec<OsString> {
    let mut args = base(input);
    let filter = format!("scale={}:{}:flags=lanczos,{TAG_FRAMES}", size.0, size.1);
    let gop = (spec.fps * 2).to_string();
    let mut push = |items: &[&str]| args.extend(items.iter().map(OsString::from));
    push(&["-vf", &filter]);
    match codec {
        Codec::Nvenc => {
            push(&["-c:v", "h264_nvenc", "-preset", spec.nvenc_preset(), "-tune", "hq", "-rc", "vbr"]);
            push(&["-cq", &spec.nvenc_cq().to_string(), "-b:v", "0", "-profile:v", "high"]);
        }
        Codec::X264 => {
            push(&["-c:v", "libx264", "-preset", "medium", "-crf", &spec.x264_crf().to_string()]);
        }
    }
    let matrix = if captured == Codec::Nvenc { "bt470bg" } else { "bt709" };
    push(&["-pix_fmt", "yuv420p", "-g", &gop, "-colorspace", matrix, "-color_range", "tv"]);
    push(&["-movflags", "+faststart"]);
    args.push(output.as_os_str().to_owned());
    args
}

/// Re-encode into WebM (VP9, CPU) from a capture made with `captured`; `frame` is the size of the captured
/// frames. The export scale resizes with Lanczos (like the scaled MP4); at 1x the size is kept, odd or not.
/// `spec` must not be an intermediate. The colour matrix of the data is kept, so the tag follows the capture
/// encoder.
pub fn webm_args(
    input: &Path,
    output: &Path,
    captured: Codec,
    frame: (u32, u32),
    scale: ExportScale,
    spec: &CaptureSpec,
) -> Vec<OsString> {
    let mut args = base(input);
    let (width, height) = scale.output_size(frame.0, frame.1);
    let filter = if scale == ExportScale::Full {
        TAG_FRAMES.to_string()
    } else {
        format!("scale={width}:{height}:flags=lanczos,{TAG_FRAMES}")
    };
    // VP9 tiles are at least 256 px wide; row-mt and tiles are what let the encoder use several cores.
    let tiles = (width / 256).max(1).ilog2().min(4);
    let threads = thread::available_parallelism().map_or(4, |n| n.get().min(8));
    let matrix = if captured == Codec::Nvenc { "bt470bg" } else { "bt709" };
    let mut push = |items: &[&str]| args.extend(items.iter().map(OsString::from));
    push(&["-vf", &filter, "-c:v", "libvpx-vp9", "-b:v", "0", "-crf", &spec.vp9_crf().to_string()]);
    push(&["-deadline", "good", "-cpu-used", spec.vp9_cpu_used(), "-row-mt", "1"]);
    push(&["-tile-columns", &tiles.to_string(), "-threads", &threads.to_string()]);
    push(&["-pix_fmt", "yuv420p", "-g", &(spec.fps * 2).to_string()]);
    push(&["-colorspace", matrix, "-color_range", "tv", "-an"]);
    args.push(output.as_os_str().to_owned());
    args
}

/// Two-pass palette GIF from the capture. `src_width` is the width of the captured frames; `scale` (the
/// per-recording export scale) multiplies the configured GIF scaling, and the result never exceeds `src_width`.
pub fn gif_args(input: &Path, output: &Path, src_width: u32, gif: &Gif, scale: ExportScale) -> Vec<OsString> {
    let mut chain = vec![format!("fps={}", gif.fps)];
    let base_width = match gif.scale {
        GifScale::Original => src_width,
        GifScale::Percent75 => src_width * 3 / 4,
        GifScale::Percent50 => src_width / 2,
        GifScale::CustomWidth => gif.custom_width.min(src_width),
    };
    let width = ((base_width as f64 * scale.factor()).round() as u32).max(2);
    if width < src_width {
        chain.push(format!("scale={width}:-1:flags=lanczos"));
    }
    let colors = match gif.quality {
        GifQuality::Smaller => 64,
        GifQuality::Balanced => 128,
        GifQuality::Best => 256,
    };
    let dither = match gif.dither {
        Dither::Auto => "sierra2_4a",
        Dither::Bayer => "bayer:bayer_scale=3",
        Dither::FloydSteinberg => "floyd_steinberg",
        Dither::None => "none",
    };
    let graph = format!(
        "{},split[a][b];[a]palettegen=max_colors={colors}:stats_mode=full[p];\
         [b][p]paletteuse=dither={dither}:diff_mode=rectangle",
        chain.join(",")
    );

    let mut args = base(input);
    args.extend(["-filter_complex".into(), graph.into()]);
    args.extend(["-loop", if gif.loop_forever { "0" } else { "-1" }].map(OsString::from));
    args.push(output.as_os_str().to_owned());
    args
}
