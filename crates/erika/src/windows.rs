use std::ffi::c_void;
use std::sync::Mutex;

use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11Resource, ID3D11Texture2D, D3D11_RESOURCE_MISC_SHARED,
    D3D11_RESOURCE_MISC_SHARED_NTHANDLE, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
};
use windows::Win32::Graphics::Dxgi::{
    DXGI_SHARED_RESOURCE_READ, IDXGIResource, IDXGIResource1,
};
use windows::Win32::Foundation::{HANDLE, LUID};
use windows::core::Interface;

pub struct D3d11SharedHandle {
    pub handle: HANDLE,
    pub width: u32,
    pub height: u32,
    pub format: i32,
    pub adapter_luid: LUID,
}

struct SharedTextureCache {
    texture: Option<ID3D11Texture2D>,
    width: u32,
    height: u32,
    format: i32,
    nthandle: bool,
}

static SHARED_TEXTURE_CACHE: Mutex<Option<SharedTextureCache>> = Mutex::new(None);

pub unsafe fn get_d3d11_texture_shared_handle(
    d3d11_texture: *mut c_void,
) -> Result<D3d11SharedHandle, String> {
    if d3d11_texture.is_null() {
        return Err("d3d11 texture pointer is null".to_string());
    }

    let texture = unsafe { ID3D11Texture2D::from_raw(d3d11_texture as *mut _) };

    let mut desc = D3D11_TEXTURE2D_DESC::default();
    unsafe { texture.GetDesc(&mut desc) };

    let resource: ID3D11Resource = texture
        .cast()
        .map_err(|e| format!("failed to cast ID3D11Texture2D to ID3D11Resource: {e}"))?;

    let dxgi_resource: IDXGIResource = match resource.cast() {
        Ok(r) => r,
        Err(_) => {
            return unsafe { create_shared_copy_and_get_handle(&texture, &desc) };
        }
    };

    let adapter_luid = unsafe { get_d3d11_adapter_luid(&texture) };
    eprintln!(
        "d3d11va shared handle: D3D11 adapter LUID = {}:{}",
        adapter_luid.LowPart, adapter_luid.HighPart
    );

    let dxgi_resource1: Option<IDXGIResource1> = dxgi_resource.cast().ok();

    if let Some(r1) = dxgi_resource1 {
        match unsafe {
            r1.CreateSharedHandle(
                None,
                DXGI_SHARED_RESOURCE_READ.0 as u32,
                windows::core::PCWSTR::null(),
            )
        } {
            Ok(handle) => {
                eprintln!("d3d11va shared handle: direct path (NT handle)");
                return Ok(D3d11SharedHandle {
                    handle,
                    width: desc.Width,
                    height: desc.Height,
                    format: desc.Format.0,
                    adapter_luid,
                });
            }
            Err(e) => {
                eprintln!("d3d11va shared handle: direct IDXGIResource1::CreateSharedHandle failed: {e:?}");
            }
        }
    }

    match unsafe { dxgi_resource.GetSharedHandle() } {
        Ok(handle) => {
            eprintln!("d3d11va shared handle: direct path (kernel handle)");
            Ok(D3d11SharedHandle {
                handle,
                width: desc.Width,
                height: desc.Height,
                format: desc.Format.0,
                adapter_luid,
            })
        }
        Err(_) => unsafe { create_shared_copy_and_get_handle(&texture, &desc) },
    }
}

unsafe fn get_d3d11_adapter_luid(texture: &ID3D11Texture2D) -> LUID {
    let device = match unsafe { texture.GetDevice() } {
        Ok(d) => d,
        Err(_) => return LUID::default(),
    };
    let dxgi_device: Result<windows::Win32::Graphics::Dxgi::IDXGIDevice, _> = device.cast();
    if let Ok(dxgi_dev) = dxgi_device {
        let adapter = unsafe { dxgi_dev.GetAdapter() };
        if let Ok(adapter) = adapter {
            let desc = unsafe { adapter.GetDesc() };
            if let Ok(desc) = desc {
                return desc.AdapterLuid;
            }
        }
    }
    LUID::default()
}

unsafe fn create_shared_copy_and_get_handle(
    src_texture: &ID3D11Texture2D,
    src_desc: &D3D11_TEXTURE2D_DESC,
) -> Result<D3d11SharedHandle, String> {
    let device: ID3D11Device = unsafe { src_texture.GetDevice() }
        .map_err(|e| format!("failed to get ID3D11Device from texture: {e}"))?;

    let adapter_luid = unsafe { get_d3d11_adapter_luid(src_texture) };

    let ctx = unsafe { device.GetImmediateContext() }
        .map_err(|e| format!("failed to get immediate context: {e}"))?;

    let mut cache = SHARED_TEXTURE_CACHE
        .lock()
        .map_err(|e| format!("cache lock failed: {e}"))?;

    let needs_new = match cache.as_ref() {
        Some(c) => {
            c.width != src_desc.Width
                || c.height != src_desc.Height
                || c.format != src_desc.Format.0
                || c.texture.is_none()
                || !c.nthandle
        }
        None => true,
    };

    if needs_new {
        eprintln!("d3d11va shared handle: staging cache miss, creating new texture (SHARED | SHARED_NTHANDLE)");
        let shared_desc = D3D11_TEXTURE2D_DESC {
            Width: src_desc.Width,
            Height: src_desc.Height,
            MipLevels: 1,
            ArraySize: 1,
            Format: src_desc.Format,
            SampleDesc: src_desc.SampleDesc,
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: 0,
            CPUAccessFlags: 0,
            MiscFlags: (D3D11_RESOURCE_MISC_SHARED | D3D11_RESOURCE_MISC_SHARED_NTHANDLE).0 as u32,
        };

        let shared_texture: ID3D11Texture2D = unsafe {
            let mut result = None;
            device
                .CreateTexture2D(&shared_desc, None, Some(&mut result))
                .map_err(|e| format!("CreateTexture2D for shared copy failed: {e}"))?;
            result.ok_or_else(|| "CreateTexture2D returned null".to_string())?
        };

        *cache = Some(SharedTextureCache {
            texture: Some(shared_texture),
            width: src_desc.Width,
            height: src_desc.Height,
            format: src_desc.Format.0,
            nthandle: true,
        });
    }

    let shared_texture = cache
        .as_ref()
        .and_then(|c| c.texture.as_ref())
        .ok_or_else(|| "no shared texture in cache".to_string())?;

    if !needs_new {
        eprintln!("d3d11va shared handle: staging cache hit");
    }
    eprintln!("d3d11va shared handle: CopySubresourceRegion staging path");

    unsafe {
        ctx.CopySubresourceRegion(
            shared_texture,
            0,
            0,
            0,
            0,
            src_texture,
            0,
            None,
        );
    }

    let resource: ID3D11Resource = shared_texture
        .cast()
        .map_err(|e| format!("cast shared texture to ID3D11Resource: {e}"))?;

    let dxgi_resource1: IDXGIResource1 = match resource.cast() {
        Ok(r1) => r1,
        Err(e) => {
            eprintln!("d3d11va shared handle: cast to IDXGIResource1 failed: {e}");
            let dxgi_resource: IDXGIResource = resource
                .cast()
                .map_err(|e2| format!("cast to IDXGIResource also failed: {e2}"))?;
            let handle = unsafe { dxgi_resource.GetSharedHandle() }
                .map_err(|e2| format!("GetSharedHandle failed: {e2}"))?;
            return Ok(D3d11SharedHandle {
                handle,
                width: src_desc.Width,
                height: src_desc.Height,
                format: src_desc.Format.0,
                adapter_luid,
            });
        }
    };

    match unsafe {
        dxgi_resource1.CreateSharedHandle(
            None,
            DXGI_SHARED_RESOURCE_READ.0 as u32,
            windows::core::PCWSTR::null(),
        )
    } {
        Ok(handle) => Ok(D3d11SharedHandle {
            handle,
            width: src_desc.Width,
            height: src_desc.Height,
            format: src_desc.Format.0,
            adapter_luid,
        }),
        Err(e) => {
            eprintln!("d3d11va shared handle: CreateSharedHandle failed: {e:?}, falling back to GetSharedHandle");
            let dxgi_resource: IDXGIResource = resource
                .cast()
                .map_err(|e2| format!("cast to IDXGIResource failed: {e2}"))?;
            let handle = unsafe { dxgi_resource.GetSharedHandle() }
                .map_err(|e2| format!("GetSharedHandle also failed: {e2}"))?;
            Ok(D3d11SharedHandle {
                handle,
                width: src_desc.Width,
                height: src_desc.Height,
                format: src_desc.Format.0,
                adapter_luid,
            })
        }
    }
}
