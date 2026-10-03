use std::ffi::c_void;
use std::sync::Mutex;

use crate::presenter::WindowsDisplayHdrCapabilities;

struct CachedHdrQuery {
    hmonitor_ptr: usize,
    caps: WindowsDisplayHdrCapabilities,
}

static HDR_QUERY_CACHE: Mutex<Option<CachedHdrQuery>> = Mutex::new(None);

pub fn query_display_hdr_capabilities(
    hwnd: *mut c_void,
) -> Result<WindowsDisplayHdrCapabilities, String> {
    if hwnd.is_null() {
        return Err("hwnd is null".to_string());
    }

    let hmon = unsafe {
        windows::Win32::Graphics::Gdi::MonitorFromWindow(
            windows::Win32::Foundation::HWND(hwnd as *mut _),
            windows::Win32::Graphics::Gdi::MONITOR_DEFAULTTONEAREST,
        )
    };

    if hmon.is_invalid() {
        return Ok(WindowsDisplayHdrCapabilities::sdr_fallback());
    }

    let hmon_ptr = hmon.0 as usize;

    if let Ok(cache) = HDR_QUERY_CACHE.lock() {
        if let Some(cached) = cache.as_ref() {
            if cached.hmonitor_ptr == hmon_ptr {
                return Ok(cached.caps);
            }
        }
    }

    let output6 = find_dxgi_output6_for_hmonitor(hmon);

    let Some(output6) = output6 else {
        return Ok(WindowsDisplayHdrCapabilities::sdr_fallback());
    };

    let desc = match unsafe { output6.GetDesc1() } {
        Ok(d) => d,
        Err(e) => {
            eprintln!("erika hdr: IDXGIOutput6::GetDesc1 failed: {e:?}");
            return Ok(WindowsDisplayHdrCapabilities::sdr_fallback());
        }
    };

    let hdr_color_space =
        windows::Win32::Graphics::Dxgi::Common::DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020;
    let supports_hdr = desc.ColorSpace.0 >= hdr_color_space.0;
    let bits_per_color = desc.BitsPerColor;
    let max_luminance_nits = if supports_hdr {
        let max_lum = desc.MaxLuminance;
        if max_lum > 0.0 && max_lum.is_finite() {
            max_lum
        } else {
            1000.0
        }
    } else {
        100.0
    };

    let caps = WindowsDisplayHdrCapabilities {
        supports_hdr,
        max_luminance_nits,
        bits_per_color,
    };

    if let Ok(mut cache) = HDR_QUERY_CACHE.lock() {
        *cache = Some(CachedHdrQuery { hmonitor_ptr: hmon_ptr, caps });
    }

    Ok(caps)
}

pub fn refresh_display_hdr_capabilities(
    hwnd: *mut c_void,
) -> Result<WindowsDisplayHdrCapabilities, String> {
    if let Ok(mut cache) = HDR_QUERY_CACHE.lock() {
        *cache = None;
    }
    query_display_hdr_capabilities(hwnd)
}

fn find_dxgi_output6_for_hmonitor(
    target: windows::Win32::Graphics::Gdi::HMONITOR,
) -> Option<windows::Win32::Graphics::Dxgi::IDXGIOutput6> {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1, IDXGIOutput6};
    use windows::core::Interface;

    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.ok()?;

    let mut adapter_index = 0u32;
    loop {
        let adapter = match unsafe { factory.EnumAdapters1(adapter_index) } {
            Ok(a) => a,
            Err(_) => break,
        };

        let mut output_index = 0u32;
        loop {
            let output = match unsafe { adapter.EnumOutputs(output_index) } {
                Ok(o) => o,
                Err(_) => break,
            };

            if let Ok(desc) = unsafe { output.GetDesc() } {
                if desc.Monitor == target {
                    return output.cast::<IDXGIOutput6>().ok();
                }
            }

            output_index += 1;
        }

        adapter_index += 1;
    }

    None
}
