/*
 * Author: Jamius Siam
 * Since: 31/10/2026
 */

//! Verification helper: runs `select_demo` and injects input, but only after the overlay window exists.
//! Usage: `drive <scenario> <out_dir> [x y]` (scenarios: full, esc, leak, tiny, rclick, esc-countdown, perf, edit,
//! dblclick, record, cancel-btn, esc-sel, rclick-sel, tiny-sel, initial, initial-off, initial-clamp, perf-move,
//! scale-probe, scale, scale-default).
//! `x y` is the screen position of the toolbar button for `record` / `cancel-btn`. `scale` takes three positions
//! (`x y` pairs): the 0.75× segment (hover), the 0.5× segment (click) and the Record button (click).

use std::fs::File;
use std::path::Path;
use std::process::{Child, Command};
use std::thread::sleep;
use std::time::{Duration, Instant};
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, SetCursorPos};
use windows::core::{PCWSTR, w};

const SELECT: PCWSTR = w!("FrameletSelectOverlay");
const COUNTDOWN: PCWSTR = w!("FrameletCountdownOverlay");

fn ms(n: u64) {
    sleep(Duration::from_millis(n));
}

fn exists(class: PCWSTR) -> bool {
    unsafe { FindWindowW(class, PCWSTR::null()) }.is_ok_and(|h| !h.is_invalid())
}

fn wait_for(class: PCWSTR) {
    let start = Instant::now();
    while !exists(class) {
        assert!(start.elapsed() < Duration::from_secs(8), "window never appeared");
        ms(10);
    }
}

fn send(input: INPUT) {
    unsafe { SendInput(&[input], size_of::<INPUT>() as i32) };
}

fn mouse(flags: MOUSE_EVENT_FLAGS) {
    let mi = MOUSEINPUT { dwFlags: flags, ..Default::default() };
    send(INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi } });
    ms(30);
}

fn key(vk: VIRTUAL_KEY) {
    for flags in [KEYBD_EVENT_FLAGS(0), KEYEVENTF_KEYUP] {
        let ki = KEYBDINPUT { wVk: vk, dwFlags: flags, ..Default::default() };
        send(INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki } });
    }
    ms(100);
}

fn move_to(x: i32, y: i32) {
    let _ = unsafe { SetCursorPos(x, y) };
    ms(10);
}

/// Moves in `steps` increments so the overlay sees a real drag.
fn drag_to(from: (i32, i32), to: (i32, i32), steps: i32) {
    for i in 1..=steps {
        move_to(from.0 + (to.0 - from.0) * i / steps, from.1 + (to.1 - from.1) * i / steps);
    }
}

fn shot(out: &Path, name: &str) {
    let file = out.join(format!("{name}.png"));
    let status = Command::new("ffmpeg")
        .args(["-y", "-v", "error", "-f", "gdigrab", "-i", "desktop", "-frames:v", "1"])
        .arg(&file)
        .status()
        .expect("ffmpeg");
    println!("shot {} ({status})", file.display());
}

fn spawn(out: &Path, args: &[&str]) -> Child {
    let exe = std::env::current_exe().unwrap().with_file_name("select_demo.exe");
    let log = File::create(out.join("demo.log")).unwrap();
    Command::new(exe)
        .args(args)
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .spawn()
        .expect("spawn select_demo")
}

fn finish(mut child: Child, out: &Path) {
    let start = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if start.elapsed() > Duration::from_secs(8) {
            let _ = child.kill();
            println!("TIMEOUT: demo did not exit");
            break;
        }
        ms(20);
    }
    println!("--- demo output ---\n{}", std::fs::read_to_string(out.join("demo.log")).unwrap());
}

fn select_drag(from: (i32, i32), to: (i32, i32)) {
    move_to(from.0, from.1);
    mouse(MOUSEEVENTF_LEFTDOWN);
    drag_to(from, to, 20);
    mouse(MOUSEEVENTF_LEFTUP);
}

fn main() {
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }.expect("DPI awareness");
    let args: Vec<String> = std::env::args().collect();
    let out = Path::new(&args[2]);
    let button = args.get(3).zip(args.get(4)).map(|(x, y)| (x.parse().unwrap(), y.parse().unwrap()));
    let point = |i: usize| -> (i32, i32) { (args[i].parse().unwrap(), args[i + 1].parse().unwrap()) };
    let demo_args: &[&str] = match args[1].as_str() {
        "leak" => &["--retry", "--no-countdown"],
        "full" | "esc-countdown" => &[],
        "initial" => &["--initial", "500,400,800,500", "--no-countdown"],
        "initial-off" => &["--initial", "9000,9000,500,500", "--no-countdown"],
        "initial-clamp" => &["--initial", "3500,1900,800,600", "--no-countdown"],
        _ => &["--no-countdown"],
    };
    let child = spawn(out, demo_args);
    wait_for(SELECT);
    ms(300);

    match args[1].as_str() {
        "full" => {
            move_to(900, 500);
            shot(out, "hint");
            move_to(400, 300);
            mouse(MOUSEEVENTF_LEFTDOWN);
            drag_to((400, 300), (1300, 801), 20);
            shot(out, "drag");
            // Record the countdown as a frame sequence (single shots are too slow to catch each tick).
            let mut rec = Command::new("ffmpeg")
                .args(["-y", "-v", "error", "-f", "gdigrab", "-framerate", "2", "-i", "desktop", "-t", "4"])
                .arg(out.join("countdown_%02d.png"))
                .spawn()
                .expect("ffmpeg");
            mouse(MOUSEEVENTF_LEFTUP);
            shot(out, "released");
            key(VK_RETURN);
            let _ = rec.wait();
            shot(out, "after");
        }
        "esc" => key(VK_ESCAPE),
        "leak" => {
            // Esc twice (overlay reopens each time), then a real selection, an edit and Enter.
            for _ in 0..2 {
                key(VK_ESCAPE);
                ms(500);
                assert!(exists(SELECT), "overlay did not reopen");
            }
            select_drag((300, 300), (700, 600));
            select_drag((500, 450), (600, 500));
            key(VK_RETURN);
        }
        "tiny" => {
            select_drag((500, 500), (504, 504));
            ms(200);
            println!("overlay still up after tiny selection: {}", exists(SELECT));
            shot(out, "tiny");
            mouse(MOUSEEVENTF_RIGHTDOWN);
            mouse(MOUSEEVENTF_RIGHTUP);
        }
        "rclick" => {
            move_to(300, 300);
            mouse(MOUSEEVENTF_LEFTDOWN);
            drag_to((300, 300), (700, 600), 10);
            mouse(MOUSEEVENTF_RIGHTDOWN);
            mouse(MOUSEEVENTF_RIGHTUP);
            mouse(MOUSEEVENTF_LEFTUP);
        }
        "esc-countdown" => {
            select_drag((300, 300), (700, 600));
            key(VK_RETURN);
            wait_for(COUNTDOWN);
            ms(500);
            key(VK_ESCAPE);
        }
        "edit" => {
            // Draw (300,300)-(900,700); the selection must stay up after release.
            select_drag((300, 300), (900, 700));
            println!("overlay up after release: {}", exists(SELECT));
            shot(out, "1_drawn");
            // Move by (+200,+100) -> left/top 500/400.
            select_drag((600, 500), (800, 600));
            shot(out, "2_moved");
            // Bottom-right corner (right/bottom 1101/801) by (+200,+100).
            select_drag((1100, 800), (1300, 900));
            shot(out, "3_corner");
            // Top edge handle (x mid of 500..1301 = 900, top 400) up by 100.
            select_drag((900, 400), (900, 300));
            shot(out, "4_edge");
            // Left edge handle dragged past the right edge: normalised.
            select_drag((500, 600), (1500, 600));
            shot(out, "5_flipped");
            // Draw outside replaces.
            select_drag((2000, 200), (2400, 500));
            shot(out, "6_replaced");
            key(VK_RETURN);
        }
        "dblclick" => {
            select_drag((300, 300), (900, 700));
            ms(100);
            move_to(600, 500);
            mouse(MOUSEEVENTF_LEFTDOWN);
            mouse(MOUSEEVENTF_LEFTUP);
            mouse(MOUSEEVENTF_LEFTDOWN);
            mouse(MOUSEEVENTF_LEFTUP);
        }
        "record" | "cancel-btn" => {
            select_drag((300, 300), (900, 700));
            shot(out, "toolbar");
            move_to(900, 400);
            let (x, y) = button.expect("pass the button position as `x y`");
            move_to(x, y);
            shot(out, "hover");
            mouse(MOUSEEVENTF_LEFTDOWN);
            mouse(MOUSEEVENTF_LEFTUP);
        }
        "scale-probe" => {
            select_drag((300, 300), (900, 700));
            move_to(1500, 900);
            shot(out, "probe");
            key(VK_ESCAPE);
        }
        "scale-default" => {
            // Enter confirms with the untouched toolbar: the scale must be 1x.
            select_drag((300, 300), (900, 700));
            key(VK_RETURN);
        }
        "scale" => {
            select_drag((300, 300), (900, 700));
            move_to(1500, 900);
            shot(out, "1_default");
            let (hover, half, record) = (point(3), point(5), point(7));
            move_to(hover.0, hover.1);
            shot(out, "2_hover_75_at_1x");
            move_to(half.0, half.1);
            shot(out, "3_hover_half");
            mouse(MOUSEEVENTF_LEFTDOWN);
            mouse(MOUSEEVENTF_LEFTUP);
            println!("overlay still up after clicking 0.5x: {}", exists(SELECT));
            shot(out, "4_half_selected");
            move_to(record.0, record.1);
            shot(out, "5_half_selected_record_hover");
            mouse(MOUSEEVENTF_LEFTDOWN);
            mouse(MOUSEEVENTF_LEFTUP);
        }
        "esc-sel" => {
            select_drag((300, 300), (900, 700));
            key(VK_ESCAPE);
        }
        "rclick-sel" => {
            select_drag((300, 300), (900, 700));
            mouse(MOUSEEVENTF_RIGHTDOWN);
            mouse(MOUSEEVENTF_RIGHTUP);
        }
        "tiny-sel" => {
            select_drag((300, 300), (900, 700));
            select_drag((1500, 500), (1503, 504));
            shot(out, "kept");
            key(VK_RETURN);
        }
        "initial" => {
            shot(out, "initial");
            key(VK_RETURN);
        }
        "initial-off" => {
            shot(out, "initial_off");
            key(VK_ESCAPE);
        }
        "initial-clamp" => {
            shot(out, "initial_clamp");
            key(VK_RETURN);
        }
        "perf-move" => {
            // Big selection, then move it around the screen with the mouse held.
            select_drag((100, 100), (2000, 1200));
            move_to(1000, 600);
            mouse(MOUSEEVENTF_LEFTDOWN);
            for _ in 0..3 {
                drag_to((1000, 600), (1800, 1000), 200);
                drag_to((1800, 1000), (1000, 600), 200);
            }
            mouse(MOUSEEVENTF_LEFTUP);
            key(VK_ESCAPE);
        }
        "perf" => {
            // Sweep the whole screen repeatedly; the demo reports render timings on stderr.
            move_to(100, 100);
            mouse(MOUSEEVENTF_LEFTDOWN);
            for _ in 0..3 {
                drag_to((100, 100), (2400, 1300), 300);
                drag_to((2400, 1300), (100, 100), 300);
            }
            key(VK_ESCAPE);
            mouse(MOUSEEVENTF_LEFTUP);
        }
        other => panic!("unknown scenario {other}"),
    }
    finish(child, out);
}
