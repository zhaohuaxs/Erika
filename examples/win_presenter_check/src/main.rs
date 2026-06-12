use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, HINSTANCE, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::PCWSTR;

use erika::core::{MediaRequest, PlatformSurface, WgpuSurfaceHandle, WgpuSurfaceKind};
use erika::danmaku::DanmakuTimeline;
use erika::presenter::{PresenterConfig, PresenterRuntime};

const WINDOW_CLASS_NAME: &str = "ErikaWinPresenterCheck";
const DEFAULT_WIDTH: i32 = 960;
const DEFAULT_HEIGHT: i32 = 540;

fn fmt_duration(d: Duration) -> String {
    let total_secs = d.as_secs_f64();
    let hours = (total_secs / 3600.0).floor() as u64;
    let mins = ((total_secs % 3600.0) / 60.0).floor() as u64;
    let secs = total_secs % 60.0;
    if hours > 0 {
        format!("{hours}:{mins:02}:{secs:06.3}")
    } else {
        format!("{mins:02}:{secs:06.3}")
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let uri = args.next().unwrap_or_else(|| {
        eprintln!("usage: win_presenter_check <video-path> [danmaku-path]");
        eprintln!("  Plays a video file in a Win32 window using Erika + wgpu + WASAPI");
        eprintln!("  Optionally loads a danmaku file (xml/json/jsonl)");
        std::process::exit(1);
    });
    let danmaku_path = args.next();

    let hwnd = create_window(DEFAULT_WIDTH, DEFAULT_HEIGHT);
    if hwnd.is_invalid() {
        eprintln!("failed to create window");
        std::process::exit(1);
    }

    unsafe { let _ = ShowWindow(hwnd, SW_SHOW); };

    let config = PresenterConfig::default();
    let mut presenter = match PresenterRuntime::new(config) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("failed to create presenter: {e}");
            std::process::exit(1);
        }
    };

    let dpi = unsafe { GetDpiForWindow(hwnd) };
    let scale = dpi as f64 / 96.0;
    let surface = PlatformSurface::Wgpu(WgpuSurfaceHandle::new(
        WgpuSurfaceKind::WindowsHwnd,
        hwnd.0 as u64,
        0,
        DEFAULT_WIDTH as u32,
        DEFAULT_HEIGHT as u32,
        scale,
    ));

    if let Err(e) = presenter.attach_surface(surface) {
        eprintln!("failed to attach surface: {e}");
        std::process::exit(1);
    }

    if let Err(e) = presenter.open(MediaRequest::new(&uri)) {
        eprintln!("failed to open {uri}: {e}");
        std::process::exit(1);
    }

    let has_danmaku = danmaku_path.is_some();
    if let Some(ref path) = danmaku_path {
        match DanmakuTimeline::from_file(path) {
            Ok(timeline) => {
                let item_count = timeline.items().len();
                presenter.set_danmaku_timeline(timeline);
                eprintln!("loaded danmaku: {path} ({item_count} items)");
            }
            Err(e) => {
                eprintln!("failed to load danmaku {path}: {e}");
            }
        }
    }

    if let Err(e) = presenter.play() {
        eprintln!("failed to play: {e}");
        std::process::exit(1);
    }

    eprintln!("playing: {uri}");
    let start = Instant::now();
    let mut msg = MSG::default();
    let mut running = true;
    let mut frame_count: u64 = 0;
    let mut fps_timer = Instant::now();
    let mut fps_frame_count: u64 = 0;
    let mut display_fps: f64 = 0.0;

    while running {
        while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
            if msg.message == WM_DESTROY {
                running = false;
                break;
            }
            unsafe { let _ = TranslateMessage(&msg); };
            unsafe { DispatchMessageW(&msg) };
        }
        if !running {
            break;
        }

        let time_seconds = start.elapsed().as_secs_f64();
        if let Err(e) = presenter.render_tick(time_seconds) {
            eprintln!("render_tick failed: {e}");
            break;
        }

        frame_count += 1;
        fps_frame_count += 1;

        let fps_elapsed = fps_timer.elapsed().as_secs_f64();
        if fps_elapsed >= 1.0 {
            display_fps = fps_frame_count as f64 / fps_elapsed;
            fps_frame_count = 0;
            fps_timer = Instant::now();
        }

        if frame_count % 15 == 0 {
            let snapshot = presenter.runtime_snapshot();
            let media_time = snapshot.media_time;
            let stats = snapshot.stats;
            let danmaku_time = snapshot.current_danmaku_items;
            let danmaku_frames = stats.danmaku_frames;
            let danmaku_items_total = stats.danmaku_items;
            let rendered = stats.rendered_video_frames;

            let decode_label = match snapshot.video_decode_backend {
                Some(erika::ffmpeg::DecoderBackend::D3d11va) => "D3D11VA",
                Some(erika::ffmpeg::DecoderBackend::Dxva2) => "DXVA2",
                Some(erika::ffmpeg::DecoderBackend::Software) => "SW",
                #[cfg(any(target_os = "macos", target_os = "ios"))]
                Some(erika::ffmpeg::DecoderBackend::VideoToolbox) => "VT",
                None => "-",
            };

            let mut title = format!(
                "Erika | wgpu+WASAPI | {decode_label} | {:.0} fps | video: {} | rendered: {rendered}",
                display_fps,
                fmt_duration(media_time),
            );

            if has_danmaku {
                let danmaku_media_time = snapshot.media_time;
                let drift_us = media_time.as_micros() as i64 - danmaku_media_time.as_micros() as i64;
                title.push_str(&format!(
                    " | dm: {} | dm_frames: {danmaku_frames} | dm_items: {danmaku_items_total} | dm_visible: {danmaku_time} | drift: {drift_us}us",
                    fmt_duration(danmaku_media_time),
                ));
            }

            let title_w = windows::core::HSTRING::from(&title as &str);
            unsafe { let _ = SetWindowTextW(hwnd, PCWSTR(title_w.as_ptr())); };
        }

        std::thread::sleep(std::time::Duration::from_millis(1));
    }

    let _ = presenter.stop();
    eprintln!("done");
}

fn create_window(width: i32, height: i32) -> HWND {
    unsafe {
        let class_name = windows::core::HSTRING::from(WINDOW_CLASS_NAME);
        let h_instance: HINSTANCE = windows::Win32::System::LibraryLoader::GetModuleHandleW(None)
            .expect("GetModuleHandleW")
            .into();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: h_instance,
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        RegisterClassW(&wc);

        let window_name = windows::core::HSTRING::from("Erika Windows Presenter Check");

        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            &class_name,
            &window_name,
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            width,
            height,
            None,
            None,
            Some(h_instance),
            None,
        )
        .expect("CreateWindowExW")
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
