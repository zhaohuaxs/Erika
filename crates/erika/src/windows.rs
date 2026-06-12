use std::ffi::c_void;
use std::sync::Mutex;

use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11Resource, ID3D11Texture2D, D3D11_RESOURCE_MISC_SHARED,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
};
use windows::Win32::Graphics::Dxgi::IDXGIResource;
use windows::Win32::Foundation::HANDLE;
use windows::core::Interface;

pub struct D3d11SharedHandle {
    pub handle: HANDLE,
    pub width: u32,
    pub height: u32,
    pub format: i32,
}

struct SharedTextureCache {
    texture: Option<ID3D11Texture2D>,
    width: u32,
    height: u32,
    format: i32,
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

    match unsafe { dxgi_resource.GetSharedHandle() } {
        Ok(handle) => Ok(D3d11SharedHandle {
            handle,
            width: desc.Width,
            height: desc.Height,
            format: desc.Format.0,
        }),
        Err(_) => unsafe { create_shared_copy_and_get_handle(&texture, &desc) },
    }
}

unsafe fn create_shared_copy_and_get_handle(
    src_texture: &ID3D11Texture2D,
    src_desc: &D3D11_TEXTURE2D_DESC,
) -> Result<D3d11SharedHandle, String> {
    let device: ID3D11Device = unsafe { src_texture.GetDevice() }
        .map_err(|e| format!("failed to get ID3D11Device from texture: {e}"))?;

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
        }
        None => true,
    };

    if needs_new {
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
            MiscFlags: D3D11_RESOURCE_MISC_SHARED.0 as u32,
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
        });
    }

    let shared_texture = cache
        .as_ref()
        .and_then(|c| c.texture.as_ref())
        .ok_or_else(|| "no shared texture in cache".to_string())?;

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

    let dxgi_resource: IDXGIResource = resource
        .cast()
        .map_err(|e| format!("cast to IDXGIResource: {e}"))?;

    let handle = unsafe { dxgi_resource.GetSharedHandle() }
        .map_err(|e| format!("GetSharedHandle on shared copy: {e}"))?;

    Ok(D3d11SharedHandle {
        handle,
        width: src_desc.Width,
        height: src_desc.Height,
        format: src_desc.Format.0,
    })
}
