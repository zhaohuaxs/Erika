use std::ffi::c_void;

use windows::Win32::Graphics::Direct3D11::{
    ID3D11Resource, ID3D11Texture2D, D3D11_TEXTURE2D_DESC,
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

    let dxgi_resource: IDXGIResource = resource
        .cast()
        .map_err(|e| format!("failed to cast ID3D11Resource to IDXGIResource: {e}"))?;

    let shared_handle = unsafe { dxgi_resource.GetSharedHandle() }
        .map_err(|e| format!("failed to get shared handle: {e}"))?;

    Ok(D3d11SharedHandle {
        handle: shared_handle,
        width: desc.Width,
        height: desc.Height,
        format: desc.Format.0,
    })
}
