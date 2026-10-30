/*
 * Author: Jamius Siam
 * Since: 31/10/2026
 */

//! Post-processing command lines: MKV to MP4 remux and MKV to GIF conversion.

use std::ffi::OsString;
use std::path::Path;

use framelet_core::settings::{Dither, Gif, GifQuality, GifScale};

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

/// Two-pass palette GIF from the capture. `src_width` is the width of the captured frames.
pub fn gif_args(input: &Path, output: &Path, src_width: u32, gif: &Gif) -> Vec<OsString> {
    let mut chain = vec![format!("fps={}", gif.fps)];
    match gif.scale {
        GifScale::Original => {}
        GifScale::Percent75 => chain.push("scale=iw*3/4:-1:flags=lanczos".into()),
        GifScale::Percent50 => chain.push("scale=iw/2:-1:flags=lanczos".into()),
        GifScale::CustomWidth if gif.custom_width < src_width => {
            chain.push(format!("scale={}:-1:flags=lanczos", gif.custom_width));
        }
        GifScale::CustomWidth => {}
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
