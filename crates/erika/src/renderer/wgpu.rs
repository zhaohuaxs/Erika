use std::ffi::c_void;
use wgpu::util::DeviceExt;

use crate::core::{
    ColorPrimaries, PlatformSurface, PlayerError, PlayerVideoFrame, RenderFrameContext,
    RendererBackend, RendererRuntimeStats, Result, TransferFunction, WgpuSurfaceHandle,
    WgpuSurfaceKind,
};
use crate::danmaku::{DanmakuGlyphAtlas, DanmakuRenderPlan};
use crate::ffmpeg::{PlanarFrame, PlanarPixelFormat};
use crate::overlay::OverlayFrame;
use crate::renderer::pipeline::{
    ColorRange, SourceColorState, TargetColorState, ToneMapOperator, VideoRenderPipeline,
};


#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WgpuRendererStats {
    pub surface_width: u32,
    pub surface_height: u32,
    pub rendered_frames: u64,
    pub offscreen_frames: u64,
    pub danmaku_passes: u64,
    pub danmaku_items: u64,
    pub attached: bool,
}

/// A clear color in the renderer's working space, components in `[0, 1]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WgpuClearColor {
    pub red: f64,
    pub green: f64,
    pub blue: f64,
    pub alpha: f64,
}

impl WgpuClearColor {
    pub fn new(red: f64, green: f64, blue: f64, alpha: f64) -> Self {
        Self {
            red,
            green,
            blue,
            alpha,
        }
    }

    /// An animated test pattern, matching the Metal renderer's `ClearColor::animated`
    /// so the two backends can be compared frame-for-frame.
    pub fn animated(time_seconds: f64) -> Self {
        Self {
            red: time_seconds.sin() * 0.5 + 0.5,
            green: (time_seconds * 0.73).sin() * 0.5 + 0.5,
            blue: (time_seconds * 1.37).cos() * 0.5 + 0.5,
            alpha: 1.0,
        }
    }

    fn to_wgpu(self) -> wgpu::Color {
        wgpu::Color {
            r: self.red,
            g: self.green,
            b: self.blue,
            a: self.alpha,
        }
    }
}

/// Tightly packed RGBA8 pixels read back from an offscreen render target.
///
/// Used as the headless verification oracle for the wgpu backend: render a pass,
/// copy the target to host memory, and assert the pixels are what we expect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WgpuOffscreenReadback {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl WgpuOffscreenReadback {
    /// Returns the RGBA bytes of the pixel at `(x, y)`.
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let offset = (y as usize * self.width as usize + x as usize) * 4;
        [
            self.rgba[offset],
            self.rgba[offset + 1],
            self.rgba[offset + 2],
            self.rgba[offset + 3],
        ]
    }
}

/// Fragment-shader uniforms for the video pipeline. The field order and byte layout
/// mirror the Metal `VideoUniforms` in `renderer/metal/apple.rs` exactly, so both
/// backends consume the same data and produce the same pixels.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct VideoUniforms {
    pub is_p010: u32,
    pub full_range: u32,
    pub source_transfer: u32,
    pub target_transfer: u32,
    pub tone_map: u32,
    pub edr_output: u32,
    pub reserved0: u32,
    pub reserved1: u32,
    pub rect: [f32; 4],
    pub viewport: [f32; 4],
    pub nits: [f32; 4],
    pub luma_coefficients: [f32; 4],
    pub gamut_matrix_rows: [[f32; 4]; 3],
}

impl VideoUniforms {
    /// Build the uniform block from a resolved render pipeline, matching how the
    /// Metal renderer fills its `VideoUniforms` in `render_video_frame`.
    pub fn from_pipeline(pipeline: &VideoRenderPipeline, is_p010: bool, edr_output: bool) -> Self {
        let luma = pipeline.luma_coefficients();
        Self {
            is_p010: u32::from(is_p010),
            full_range: u32::from(matches!(pipeline.source.range, ColorRange::Full)),
            source_transfer: transfer_code(pipeline.source.transfer),
            target_transfer: transfer_code(pipeline.target.transfer),
            tone_map: tone_map_code(pipeline.tone_map.operator),
            edr_output: u32::from(edr_output),
            reserved0: 0,
            reserved1: 0,
            rect: [0.0, 0.0, 0.0, 0.0],
            viewport: [0.0, 0.0, 0.0, 0.0],
            nits: [
                pipeline.source.nominal_peak_nits,
                pipeline.target.peak_nits,
                pipeline.source.reference_white_nits,
                pipeline.target.reference_white_nits,
            ],
            luma_coefficients: [luma.kr, luma.kg, luma.kb, 0.0],
            gamut_matrix_rows: pipeline.gamut_matrix().row4s(),
        }
    }
}

// Mirror of the `transfer_code` / `tone_map_code` mappings in macos.rs. Kept in sync
// with the Metal backend; the WGSL shader branches on these same integer codes.
fn transfer_code(transfer: TransferFunction) -> u32 {
    match transfer {
        TransferFunction::Srgb => 1,
        TransferFunction::Bt1886 => 2,
        TransferFunction::Pq => 3,
        TransferFunction::Hlg => 4,
        TransferFunction::Unknown => 1,
    }
}

#[derive(Debug, Clone, Copy)]
struct VideoPresentationLayout {
    source_width: f32,
    source_height: f32,
    target_rect: [f32; 4],
    drawable_width: f32,
    drawable_height: f32,
}

impl VideoPresentationLayout {
    fn aspect_fit(
        source_width: u32,
        source_height: u32,
        drawable_width: u32,
        drawable_height: u32,
    ) -> Self {
        let source_width = source_width.max(1) as f32;
        let source_height = source_height.max(1) as f32;
        let drawable_width = drawable_width.max(1) as f32;
        let drawable_height = drawable_height.max(1) as f32;
        let scale = (drawable_width / source_width).min(drawable_height / source_height);
        let width = source_width * scale;
        let height = source_height * scale;
        let x = (drawable_width - width) * 0.5;
        let y = (drawable_height - height) * 0.5;
        Self {
            source_width,
            source_height,
            target_rect: [x, y, width, height],
            drawable_width,
            drawable_height,
        }
    }

    fn video_viewport(self) -> [f32; 4] {
        [self.drawable_width, self.drawable_height, 0.0, 0.0]
    }

    fn overlay_viewport(self) -> [f32; 2] {
        [self.drawable_width, self.drawable_height]
    }

    fn map_source_rect(self, x: f32, y: f32, width: f32, height: f32) -> [f32; 4] {
        let scale_x = self.target_rect[2] / self.source_width;
        let scale_y = self.target_rect[3] / self.source_height;
        [
            self.target_rect[0] + x * scale_x,
            self.target_rect[1] + y * scale_y,
            width * scale_x,
            height * scale_y,
        ]
    }
}

fn tone_map_code(operator: ToneMapOperator) -> u32 {
    match operator {
        ToneMapOperator::Clip => 0,
        ToneMapOperator::Reinhard => 1,
        ToneMapOperator::Mobius => 2,
    }
}

fn overlay_has_planes(frame: &OverlayFrame) -> bool {
    !frame.subtitle_planes.is_empty() || !frame.subtitle_alpha_planes.is_empty()
}

/// Overlay quad uniforms, byte-compatible with the Metal `OverlayUniforms`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct OverlayUniforms {
    pub rect: [f32; 4],
    pub tex_rect: [f32; 4],
    pub viewport: [f32; 2],
    pub overlay_mode: u32,
    pub reserved0: u32,
    pub color: [f32; 4],
}

impl OverlayUniforms {
    /// A straight-RGBA subtitle plane placed at pixel `rect` within the viewport.
    fn rgba_plane(
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        layout: VideoPresentationLayout,
    ) -> Self {
        Self {
            rect: layout.map_source_rect(x as f32, y as f32, width as f32, height as f32),
            tex_rect: [0.0, 0.0, 1.0, 1.0],
            viewport: layout.overlay_viewport(),
            overlay_mode: 0,
            reserved0: 0,
            color: [1.0, 1.0, 1.0, 1.0],
        }
    }

    /// A libass alpha coverage bitmap sampled from a horizontal R8 atlas at `atlas_x`,
    /// tinted by `color_rgba` (mode 1). Mirrors the Metal `from_alpha_atlas_bitmap`.
    #[allow(clippy::too_many_arguments)]

    #[allow(dead_code)]
    fn alpha_atlas_rect(
        color: [f32; 4],
        rect: [f32; 4],
        tex_rect: [f32; 4],
        layout: VideoPresentationLayout,
    ) -> Self {
        Self {
            rect,
            tex_rect,
            viewport: layout.overlay_viewport(),
            overlay_mode: 1,
            reserved0: 0,
            color,
        }
    }
}

/// Lazily-built GPU objects for the NV12/P010 video pipeline, tied to the color
/// target format the pipeline was compiled for.
struct VideoPipeline {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    format: wgpu::TextureFormat,
}

/// Lazily-built GPU objects for the overlay (subtitle/danmaku) compositing pass,
/// tied to the color target format it was compiled for.
struct OverlayPipeline {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    format: wgpu::TextureFormat,
}

/// Per-plane GPU resources for one overlay draw. The texture and uniform buffer are
/// retained so the bind group stays valid for the duration of the render pass.
struct OverlayDraw {
    bind_group: wgpu::BindGroup,
    _texture: wgpu::Texture,
    _uniform: wgpu::Buffer,
    instance_count: u32,
    use_batch_pipeline: bool,
}

struct WgpuDanmakuAtlasCache {
    version: u64,
    width: u32,
    height: u32,
    stride: usize,
    fill_texture: wgpu::Texture,
    outline_texture: wgpu::Texture,
}

impl WgpuDanmakuAtlasCache {
    fn can_reuse_for(&self, atlas: &DanmakuGlyphAtlas) -> bool {
        self.version == atlas.version
            && self.width == atlas.width
            && self.height == atlas.height
            && self.stride == atlas.stride
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct DanmakuBatchInstance {
    rect: [f32; 4],
    tex_rect: [f32; 4],
    color: [f32; 4],
}

impl DanmakuBatchInstance {
    fn new(rect: [f32; 4], tex_rect: [f32; 4], color: [f32; 4]) -> Self {
        Self {
            rect,
            tex_rect,
            color,
        }
    }
}

struct DanmakuBatchPipeline {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    format: wgpu::TextureFormat,
}

/// The currently uploaded video frame: GPU plane textures plus the color uniforms
/// to render it. Retained so the presenter can re-present it across vsync ticks.
struct UploadedVideoFrame {
    luma: wgpu::TextureView,
    chroma: wgpu::TextureView,
    width: u32,
    height: u32,
    uniforms: VideoUniforms,
}

struct AttachedSurface {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    handle: WgpuSurfaceHandle,
}

pub struct WgpuRenderer {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: Option<AttachedSurface>,
    video_pipeline: Option<VideoPipeline>,
    overlay_pipeline: Option<OverlayPipeline>,
    danmaku_batch_pipeline: Option<DanmakuBatchPipeline>,
    current_video: Option<UploadedVideoFrame>,
    danmaku_atlas_cache: Option<WgpuDanmakuAtlasCache>,
    danmaku_instance_buffer: Option<wgpu::Buffer>,
    danmaku_instance_buffer_len: usize,
    supports_16bit_norm: bool,
    stats: WgpuRendererStats,
}

/// Offscreen readback targets use a linear `Rgba8Unorm` format so a clear value of
/// `c` reads back as `round(c * 255)` with no transfer-function surprises.
const OFFSCREEN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

impl WgpuRenderer {
    pub fn new() -> Result<Self> {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
        }))
        .map_err(|error| PlayerError::Renderer(format!("wgpu adapter request failed: {error}")))?;

        // 16-bit normalized textures (R16Unorm/Rg16Unorm) are needed for P010/10-bit
        // upload. They are not in the WebGPU baseline, so request the feature only when
        // the adapter advertises it (true on Metal/Vulkan/DX12 native backends).
        let supports_16bit_norm = adapter
            .features()
            .contains(wgpu::Features::TEXTURE_FORMAT_16BIT_NORM);
        let supports_vulkan_ext_mem_win32 = adapter
            .features()
            .contains(wgpu::Features::VULKAN_EXTERNAL_MEMORY_WIN32);
        let supports_nv12 = adapter
            .features()
            .contains(wgpu::Features::TEXTURE_FORMAT_NV12);
        let mut required_features = wgpu::Features::empty();
        if supports_16bit_norm {
            required_features |= wgpu::Features::TEXTURE_FORMAT_16BIT_NORM;
        }
        if supports_vulkan_ext_mem_win32 {
            required_features |= wgpu::Features::VULKAN_EXTERNAL_MEMORY_WIN32;
        }
        if supports_nv12 {
            required_features |= wgpu::Features::TEXTURE_FORMAT_NV12;
        }

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("erika-wgpu-device"),
            required_features,
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::default(),
            experimental_features: wgpu::ExperimentalFeatures::default(),
            trace: wgpu::Trace::Off,
        }))
        .map_err(|error| PlayerError::Renderer(format!("wgpu device request failed: {error}")))?;

        Ok(Self {
            instance,
            adapter,
            device,
            queue,
            surface: None,
            video_pipeline: None,
            overlay_pipeline: None,
            current_video: None,
            danmaku_atlas_cache: None,
            danmaku_batch_pipeline: None,
            danmaku_instance_buffer: None,
            danmaku_instance_buffer_len: 0,
            supports_16bit_norm,
            stats: WgpuRendererStats::default(),
        })
    }

    pub fn surface(&self) -> Option<WgpuSurfaceHandle> {
        self.surface.as_ref().map(|attached| attached.handle)
    }

    /// Whether the adapter supports 16-bit normalized textures (needed for P010).
    pub fn supports_16bit_norm(&self) -> bool {
        self.supports_16bit_norm
    }

    pub fn stats(&self) -> WgpuRendererStats {
        self.stats
    }

    pub fn adapter_info(&self) -> wgpu::AdapterInfo {
        self.adapter.get_info()
    }

    /// Render a single clear pass into an offscreen `width`x`height` target and read
    /// the result back to host memory. This is the backend's headless test path: it
    /// needs no window or platform surface, so it runs under plain `cargo test`.
    pub fn clear_offscreen(
        &mut self,
        width: u32,
        height: u32,
        color: WgpuClearColor,
    ) -> Result<WgpuOffscreenReadback> {
        if width == 0 || height == 0 {
            return Err(PlayerError::Renderer(
                "offscreen target must have non-zero dimensions".to_string(),
            ));
        }

        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("erika-wgpu-offscreen"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OFFSCREEN_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("erika-wgpu-offscreen-encoder"),
            });
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("erika-wgpu-offscreen-clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(color.to_wgpu()),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }

        self.queue.submit(Some(encoder.finish()));

        let rgba = self.read_back_rgba8(&texture, width, height)?;
        self.stats.offscreen_frames += 1;
        Ok(WgpuOffscreenReadback {
            width,
            height,
            rgba,
        })
    }

    /// Copy an RGBA8 texture into host memory, stripping the row padding that
    /// `copy_texture_to_buffer` requires (rows aligned to COPY_BYTES_PER_ROW_ALIGNMENT).
    fn read_back_rgba8(&self, texture: &wgpu::Texture, width: u32, height: u32) -> Result<Vec<u8>> {
        let unpadded_bytes_per_row = width * 4;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_bytes_per_row = unpadded_bytes_per_row.div_ceil(align) * align;
        let buffer_size = (padded_bytes_per_row * height) as wgpu::BufferAddress;
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("erika-wgpu-readback"),
            size: buffer_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("erika-wgpu-readback-encoder"),
            });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(Some(encoder.finish()));

        let slice = readback.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|error| PlayerError::Renderer(format!("wgpu device poll failed: {error}")))?;
        receiver
            .recv()
            .map_err(|_| PlayerError::Renderer("wgpu readback channel dropped".to_string()))?
            .map_err(|error| PlayerError::Renderer(format!("wgpu buffer map failed: {error}")))?;

        let mapped = slice.get_mapped_range();
        let mut rgba = Vec::with_capacity((unpadded_bytes_per_row * height) as usize);
        for row in 0..height {
            let start = (row * padded_bytes_per_row) as usize;
            let end = start + unpadded_bytes_per_row as usize;
            rgba.extend_from_slice(&mapped[start..end]);
        }
        drop(mapped);
        readback.unmap();
        Ok(rgba)
    }

    /// Render a software-decoded NV12 frame through the WGSL video pipeline into an
    /// offscreen RGBA8 target and read it back. Mirrors the Metal `render_video_frame`
    /// path so results can be compared against the native backend.
    ///
    /// `luma` is `width * height` bytes (Y plane). `chroma` is the interleaved
    /// Cb/Cr plane at half resolution: `(width / 2) * (height / 2) * 2` bytes.
    pub fn render_nv12_offscreen(
        &mut self,
        width: u32,
        height: u32,
        luma: &[u8],
        chroma: &[u8],
        uniforms: VideoUniforms,
    ) -> Result<WgpuOffscreenReadback> {
        self.upload_nv12(width, height, luma, chroma, uniforms)?;
        self.render_current_offscreen(None)?
            .ok_or_else(|| PlayerError::Renderer("no current frame after upload".to_string()))
    }

    /// Upload tightly packed NV12 planes as the current video frame. `luma` is
    /// `width * height` bytes; `chroma` is the interleaved Cb/Cr plane at half
    /// resolution (`(width / 2) * (height / 2) * 2` bytes).
    pub fn upload_nv12(
        &mut self,
        width: u32,
        height: u32,
        luma: &[u8],
        chroma: &[u8],
        uniforms: VideoUniforms,
    ) -> Result<()> {
        self.upload_planar(
            PlanarFrame {
                format: PlanarPixelFormat::Nv12,
                width,
                height,
                luma: luma.to_vec(),
                chroma: chroma.to_vec(),
            },
            uniforms,
        )
    }

    /// Upload a repacked planar frame (8-bit NV12 or 10-bit P010) as the current
    /// video frame. P010 requires the `TEXTURE_FORMAT_16BIT_NORM` adapter feature.
    pub fn upload_planar(&mut self, frame: PlanarFrame, uniforms: VideoUniforms) -> Result<()> {
        self.upload_planar_with_context(frame, uniforms)
    }

    fn upload_planar_with_context(
        &mut self,
        frame: PlanarFrame,
        uniforms: VideoUniforms,
    ) -> Result<()> {
        let width = frame.width;
        let height = frame.height;
        if width == 0 || height == 0 || !width.is_multiple_of(2) || !height.is_multiple_of(2) {
            return Err(PlayerError::Renderer(
                "planar frame dimensions must be non-zero and even".to_string(),
            ));
        }
        let (luma_format, chroma_format, bytes_per_sample) = match frame.format {
            PlanarPixelFormat::Nv12 => (
                wgpu::TextureFormat::R8Unorm,
                wgpu::TextureFormat::Rg8Unorm,
                1u32,
            ),
            PlanarPixelFormat::P010 => {
                if !self.supports_16bit_norm {
                    return Err(PlayerError::Renderer(
                        "wgpu adapter lacks TEXTURE_FORMAT_16BIT_NORM required for P010/10-bit"
                            .to_string(),
                    ));
                }
                (
                    wgpu::TextureFormat::R16Unorm,
                    wgpu::TextureFormat::Rg16Unorm,
                    2u32,
                )
            }
        };
        let chroma_width = width / 2;
        let chroma_height = height / 2;
        let expected_luma = (width * height * bytes_per_sample) as usize;
        let expected_chroma = (chroma_width * chroma_height * 2 * bytes_per_sample) as usize;
        if frame.luma.len() != expected_luma {
            return Err(PlayerError::Renderer(format!(
                "{:?} luma plane is {} bytes, expected {expected_luma}",
                frame.format,
                frame.luma.len()
            )));
        }
        if frame.chroma.len() != expected_chroma {
            return Err(PlayerError::Renderer(format!(
                "{:?} chroma plane is {} bytes, expected {expected_chroma}",
                frame.format,
                frame.chroma.len()
            )));
        }

        let luma_texture = self.create_plane_texture(
            "erika-wgpu-luma",
            width,
            height,
            luma_format,
            &frame.luma,
            width * bytes_per_sample,
        );
        let chroma_texture = self.create_plane_texture(
            "erika-wgpu-chroma",
            chroma_width,
            chroma_height,
            chroma_format,
            &frame.chroma,
            chroma_width * 2 * bytes_per_sample,
        );
        let luma_view = luma_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let chroma_view = chroma_texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.current_video = Some(UploadedVideoFrame {
            luma: luma_view,
            chroma: chroma_view,
            width,
            height,
            uniforms,
        });
        Ok(())
    }

    /// Render the current video frame (optionally compositing `overlay`) into an
    /// offscreen RGBA8 target and read it back. Returns `None` if no frame has been
    /// uploaded.
    pub fn render_current_offscreen(
        &mut self,
        overlay: Option<&OverlayFrame>,
    ) -> Result<Option<WgpuOffscreenReadback>> {
        if self.current_video.is_none() {
            return Ok(None);
        }
        self.ensure_video_pipeline(OFFSCREEN_FORMAT);
        if overlay.is_some_and(overlay_has_planes) {
            self.ensure_overlay_pipeline(OFFSCREEN_FORMAT);
        }
        let (width, height) = {
            let video = self.current_video.as_ref().expect("current video frame");
            (video.width, video.height)
        };
        let target = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("erika-wgpu-video-target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OFFSCREEN_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let _ = self.draw_current_video(&target_view, overlay, None)?;
        let rgba = self.read_back_rgba8(&target, width, height)?;
        self.stats.rendered_frames += 1;
        Ok(Some(WgpuOffscreenReadback {
            width,
            height,
            rgba,
        }))
    }

    /// Encode and submit a render pass drawing the current video frame into
    /// `target_view`. The caller must have uploaded a frame and the video pipeline
    /// must be initialized.
    fn draw_current_video(
        &mut self,
        target_view: &wgpu::TextureView,
        overlay: Option<&OverlayFrame>,
        danmaku: Option<&DanmakuRenderPlan>,
    ) -> Result<usize> {
        let danmaku_plan = danmaku.filter(|plan| !plan.is_empty());
        let video = self
            .current_video
            .as_ref()
            .ok_or_else(|| PlayerError::Renderer("no current video frame".to_string()))?;
        let pipeline = self
            .video_pipeline
            .as_ref()
            .ok_or_else(|| PlayerError::Renderer("video pipeline not initialized".to_string()))?;

        let luma_view = &video.luma;
        let chroma_view = &video.chroma;

        let surface_width = self.stats.surface_width.max(1);
        let surface_height = self.stats.surface_height.max(1);
        let layout = VideoPresentationLayout::aspect_fit(
            video.width,
            video.height,
            surface_width,
            surface_height,
        );

        let overlay_draws = match overlay {
            Some(frame) if overlay_has_planes(frame) => {

                self.prepare_overlay_draws(frame, layout)?
            }
            _ => Vec::new(),
        };

        let mut uniforms = video.uniforms;
        uniforms.rect = layout.target_rect;
        uniforms.viewport = layout.video_viewport();

        let uniform_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("erika-wgpu-video-uniforms"),
                contents: bytemuck::bytes_of(&uniforms),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("erika-wgpu-video-bind-group"),
            layout: &pipeline.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&luma_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&chroma_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&pipeline.sampler),
                },
            ],
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("erika-wgpu-video-encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("erika-wgpu-video-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&pipeline.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..6, 0..1);
        }

        let has_overlay = !overlay_draws.is_empty();
        let has_danmaku = danmaku_plan.is_some();

        if has_overlay || has_danmaku {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("erika-wgpu-overlay-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            if has_overlay {
                let overlay_pipeline = self.overlay_pipeline.as_ref().ok_or_else(|| {
                    PlayerError::Renderer("overlay pipeline not initialized".to_string())
                })?;
                let batch_pipeline = self.danmaku_batch_pipeline.as_ref();
                for draw in &overlay_draws {
                    if draw.use_batch_pipeline {
                        if let Some(bp) = &batch_pipeline {
                            pass.set_pipeline(&bp.pipeline);
                            pass.set_bind_group(0, &draw.bind_group, &[]);
                            pass.draw(0..6, 0..draw.instance_count);
                        }
                    } else {
                        pass.set_pipeline(&overlay_pipeline.pipeline);
                        pass.set_bind_group(0, &draw.bind_group, &[]);
                        pass.draw(0..6, 0..1);
                    }
                }
            }

            if let Some(plan) = danmaku_plan {
                let _ = self.draw_danmaku_batch(&mut pass, plan)?;
            }
        }

        self.queue.submit(Some(encoder.finish()));
        Ok(0)
    }

    fn draw_danmaku_batch(
        &mut self,
        pass: &mut wgpu::RenderPass<'_>,
        plan: &DanmakuRenderPlan,
    ) -> Result<usize> {
        let Some(atlas) = plan.atlas.as_ref() else {
            return Ok(0);
        };
        if !atlas.is_valid() {
            return Ok(0);
        }

        self.ensure_danmaku_batch_pipeline(self.surface.as_ref().unwrap().config.format);

        let viewport_w = plan.viewport.width;
        let viewport_h = plan.viewport.height;

        let (fill_texture, outline_texture) = self.prepare_danmaku_atlas_textures(atlas);

        let mut shadow_instances = Vec::new();
        let mut outline_instances = Vec::new();
        let mut fill_instances = Vec::new();

        for item in &plan.items {
            if item.shadow_rgba[3] > 0.0 {
                let mut rect = item.rect;
                rect[0] += item.shadow_offset[0];
                rect[1] += item.shadow_offset[1];
                shadow_instances.push(DanmakuBatchInstance::new(
                    rect,
                    item.tex_rect,
                    item.shadow_rgba,
                ));
            }
            if item.outline_rgba[3] > 0.0 {
                outline_instances.push(DanmakuBatchInstance::new(
                    item.rect,
                    item.tex_rect,
                    item.outline_rgba,
                ));
            }
            fill_instances.push(DanmakuBatchInstance::new(
                item.rect,
                item.tex_rect,
                item.color_rgba,
            ));
        }

        let outline_count = shadow_instances.len() + outline_instances.len();
        let fill_count = fill_instances.len();
        let total_count = outline_count + fill_count;

        if total_count == 0 {
            return Ok(0);
        }

        let mut outline_data = Vec::with_capacity(outline_count);
        outline_data.extend_from_slice(&shadow_instances);
        outline_data.extend_from_slice(&outline_instances);

        let fill_data: Vec<DanmakuBatchInstance> = fill_instances;

        let outline_bytes = bytemuck::cast_slice(&outline_data);
        let fill_bytes = bytemuck::cast_slice(&fill_data);

        self.ensure_danmaku_instance_buffer(outline_bytes.len().max(fill_bytes.len()));

        let instance_buffer = self.danmaku_instance_buffer.as_ref().unwrap();

        let batch_pipeline = self.danmaku_batch_pipeline.as_ref().ok_or_else(|| {
            PlayerError::Renderer("danmaku batch pipeline not initialized".to_string())
        })?;

        #[repr(C)]
        #[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
        struct DanmakuBatchUniforms {
            viewport: [f32; 2],
        }

        let batch_uniforms = DanmakuBatchUniforms {
            viewport: [viewport_w.max(1) as f32, viewport_h.max(1) as f32],
        };
        let uniform_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("erika-wgpu-danmaku-batch-uniforms"),
                contents: bytemuck::bytes_of(&batch_uniforms),
                usage: wgpu::BufferUsages::UNIFORM,
            });

        pass.set_pipeline(&batch_pipeline.pipeline);

        if outline_count > 0 {
            self.queue.write_buffer(instance_buffer, 0, outline_bytes);
            let outline_view = outline_texture.create_view(&wgpu::TextureViewDescriptor::default());
            let outline_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("erika-wgpu-danmaku-outline-bgl"),
                layout: &batch_pipeline.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: instance_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&outline_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(&batch_pipeline.sampler),
                    },
                ],
            });
            pass.set_bind_group(0, &outline_bind_group, &[]);
            pass.draw(0..6, 0..outline_count as u32);
        }

        if fill_count > 0 {
            self.queue.write_buffer(instance_buffer, 0, fill_bytes);
            let fill_view = fill_texture.create_view(&wgpu::TextureViewDescriptor::default());
            let fill_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("erika-wgpu-danmaku-fill-bgl"),
                layout: &batch_pipeline.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: instance_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&fill_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(&batch_pipeline.sampler),
                    },
                ],
            });
            pass.set_bind_group(0, &fill_bind_group, &[]);
            pass.draw(0..6, 0..fill_count as u32);
        }

        Ok(plan.items.len())
    }

    /// Build per-quad GPU resources for the overlay: straight-RGBA subtitle planes
    /// (mode 0) plus libass alpha coverage bitmaps packed into one R8 atlas (mode 1).
    fn prepare_overlay_draws(
        &self,
        frame: &OverlayFrame,
        layout: VideoPresentationLayout,
    ) -> Result<Vec<OverlayDraw>> {
        if self.overlay_pipeline.is_none() {
            return Err(PlayerError::Renderer(
                "overlay pipeline not initialized".to_string(),
            ));
        }
        let mut draws = Vec::new();

        for plane in &frame.subtitle_planes {
            if plane.width == 0 || plane.height == 0 {
                continue;
            }
            let expected = plane.width as usize * plane.height as usize * 4;
            if plane.rgba.len() != expected {
                return Err(PlayerError::Renderer(format!(
                    "overlay subtitle plane has {} bytes, expected {expected} for {}x{} RGBA",
                    plane.rgba.len(),
                    plane.width,
                    plane.height
                )));
            }
            let texture = self.create_plane_texture(
                "erika-wgpu-overlay-plane",
                plane.width,
                plane.height,
                wgpu::TextureFormat::Rgba8Unorm,
                &plane.rgba,
                plane.width * 4,
            );
            let uniforms = OverlayUniforms::rgba_plane(
                plane.x,
                plane.y,
                plane.width,
                plane.height,
                layout,
            );
            draws.push(self.make_overlay_draw(&texture, uniforms));
        }

        self.append_alpha_atlas_draws(frame, layout, &mut draws)?;
        Ok(draws)
    }


    fn prepare_danmaku_atlas_textures(
        &mut self,
        atlas: &DanmakuGlyphAtlas,
    ) -> (wgpu::Texture, wgpu::Texture) {
        if let Some(cache) = &self.danmaku_atlas_cache {
            if cache.can_reuse_for(atlas) {
                return (cache.fill_texture.clone(), cache.outline_texture.clone());
            }
        }
        let fill_texture = self.create_plane_texture(
            "erika-wgpu-danmaku-fill-atlas",
            atlas.width,
            atlas.height,
            wgpu::TextureFormat::R8Unorm,
            &atlas.fill_alpha,
            atlas.stride as u32,
        );
        let outline_texture = self.create_plane_texture(
            "erika-wgpu-danmaku-outline-atlas",
            atlas.width,
            atlas.height,
            wgpu::TextureFormat::R8Unorm,
            &atlas.outline_alpha,
            atlas.stride as u32,
        );
        self.danmaku_atlas_cache = Some(WgpuDanmakuAtlasCache {
            version: atlas.version,
            width: atlas.width,
            height: atlas.height,
            stride: atlas.stride,
            fill_texture: fill_texture.clone(),
            outline_texture: outline_texture.clone(),
        });
        (fill_texture, outline_texture)
    }


    /// Pack libass alpha coverage bitmaps horizontally into one R8 atlas and add a
    /// mode-1 (coverage tinted by the bitmap's color) draw per placement. Mirrors the
    /// Metal `prepare_overlay_alpha_atlas` packing.
    fn append_alpha_atlas_draws(
        &self,
        frame: &OverlayFrame,
        layout: VideoPresentationLayout,
        draws: &mut Vec<OverlayDraw>,
    ) -> Result<()> {
        let bitmaps = &frame.subtitle_alpha_planes;
        let max_width = self.device.limits().max_texture_dimension_2d as usize;

        let mut rows: Vec<Vec<usize>> = Vec::new();
        let mut current_row_width = 0usize;
        let mut current_row: Vec<usize> = Vec::new();

        for (index, bitmap) in bitmaps.iter().enumerate() {
            if bitmap.placement.width == 0 || bitmap.placement.height == 0 || !bitmap.is_valid() {
                continue;
            }
            let bw = bitmap.placement.width as usize;
            if !current_row.is_empty() && current_row_width + bw > max_width {
                rows.push(std::mem::take(&mut current_row));
                current_row_width = 0;
            }
            current_row.push(index);
            current_row_width += bw;
        }
        if !current_row.is_empty() {
            rows.push(current_row);
        }

        for row_indices in &rows {
            let row_width: usize = row_indices
                .iter()
                .map(|i| bitmaps[*i].placement.width as usize)
                .sum();
            let row_height = row_indices
                .iter()
                .map(|i| bitmaps[*i].placement.height as usize)
                .max()
                .unwrap_or(0);
            if row_width == 0 || row_height == 0 {
                continue;
            }

            let mut pixels = vec![0u8; row_width * row_height];
            let mut instances: Vec<DanmakuBatchInstance> = Vec::with_capacity(row_indices.len());

            let mut cursor_x = 0usize;
            for &index in row_indices {
                let bitmap = &bitmaps[index];
                let bw = bitmap.placement.width as usize;
                let bh = bitmap.placement.height as usize;
                for row in 0..bh {
                    let src = row * bitmap.stride;
                    let dst = row * row_width + cursor_x;
                    pixels[dst..dst + bw].copy_from_slice(&bitmap.alpha[src..src + bw]);
                }

                let color = crate::subtitle::AssColor::from_libass_rgba(bitmap.color_rgba);
                let rect = layout.map_source_rect(
                    bitmap.placement.x as f32,
                    bitmap.placement.y as f32,
                    bitmap.placement.width as f32,
                    bitmap.placement.height as f32,
                );
                let aw = row_width.max(1) as f32;
                let ah = row_height.max(1) as f32;
                instances.push(DanmakuBatchInstance::new(
                    rect,
                    [
                        cursor_x as f32 / aw,
                        0.0,
                        bitmap.placement.width as f32 / aw,
                        bitmap.placement.height as f32 / ah,
                    ],
                    [
                        f32::from(color.red) / 255.0,
                        f32::from(color.green) / 255.0,
                        f32::from(color.blue) / 255.0,
                        f32::from(color.alpha) / 255.0,
                    ],
                ));
                cursor_x += bw;
            }

            let texture = self.create_plane_texture(
                "erika-wgpu-overlay-atlas",
                row_width as u32,
                row_height as u32,
                wgpu::TextureFormat::R8Unorm,
                &pixels,
                row_width as u32,
            );

            let _pipeline = self
                .overlay_pipeline
                .as_ref()
                .expect("overlay pipeline initialized");
            let batch_pipeline = self
                .danmaku_batch_pipeline
                .as_ref()
                .ok_or_else(|| PlayerError::Renderer("danmaku batch pipeline not initialized".to_string()))?;

            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

            #[repr(C)]
            #[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
            struct BatchUniforms {
                viewport: [f32; 2],
            }
            let batch_uniforms = BatchUniforms {
                viewport: layout.overlay_viewport(),
            };
            let uniform_buffer = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("erika-wgpu-overlay-batch-uniforms"),
                    contents: bytemuck::bytes_of(&batch_uniforms),
                    usage: wgpu::BufferUsages::UNIFORM,
                });

            let instance_bytes = bytemuck::cast_slice(&instances);
            let instance_buffer = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("erika-wgpu-overlay-batch-instances"),
                    contents: instance_bytes,
                    usage: wgpu::BufferUsages::STORAGE,
                });

            let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("erika-wgpu-overlay-batch-bgl"),
                layout: &batch_pipeline.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: instance_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(&batch_pipeline.sampler),
                    },
                ],
            });

            draws.push(OverlayDraw {
                bind_group,
                _texture: texture,
                _uniform: uniform_buffer,
                instance_count: instances.len() as u32,
                use_batch_pipeline: true,
            });
        }
        Ok(())
    }

    fn make_overlay_draw(&self, texture: &wgpu::Texture, uniforms: OverlayUniforms) -> OverlayDraw {
        let pipeline = self
            .overlay_pipeline
            .as_ref()
            .expect("overlay pipeline initialized");
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let uniform = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("erika-wgpu-overlay-uniforms"),
                contents: bytemuck::bytes_of(&uniforms),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("erika-wgpu-overlay-bind-group"),
            layout: &pipeline.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&pipeline.sampler),
                },
            ],
        });
        OverlayDraw {
            bind_group,
            _texture: texture.clone(),
            _uniform: uniform,
            instance_count: 1,
            use_batch_pipeline: false,
        }
    }

    fn create_plane_texture(
        &self,
        label: &str,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
        data: &[u8],
        bytes_per_row: u32,
    ) -> wgpu::Texture {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        texture
    }

    fn ensure_video_pipeline(&mut self, format: wgpu::TextureFormat) {
        // The render pipeline's color target format must match the render pass
        // attachment, so rebuild if the target format changed (offscreen Rgba8Unorm
        // vs the surface's format).
        if self
            .video_pipeline
            .as_ref()
            .is_some_and(|video| video.format == format)
        {
            return;
        }
        let shader = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("erika-wgpu-video-shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("wgpu_video.wgsl").into()),
            });
        let texture_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let bind_group_layout =
            self.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("erika-wgpu-video-bgl"),
                    entries: &[
                        wgpu::BindGroupLayoutEntry {
                            binding: 0,
                            visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Uniform,
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        texture_entry(1),
                        texture_entry(2),
                        wgpu::BindGroupLayoutEntry {
                            binding: 3,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                            count: None,
                        },
                    ],
                });
        let layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("erika-wgpu-video-layout"),
                bind_group_layouts: &[Some(&bind_group_layout)],
                immediate_size: 0,
            });
        let pipeline = self
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("erika-wgpu-video-pipeline"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("erika_video_vertex"),
                    buffers: &[],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("erika_video_fragment"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                multiview_mask: None,
                cache: None,
            });
        let sampler = self.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("erika-wgpu-video-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        self.video_pipeline = Some(VideoPipeline {
            pipeline,
            bind_group_layout,
            sampler,
            format,
        });
    }

    fn ensure_overlay_pipeline(&mut self, format: wgpu::TextureFormat) {
        if self
            .overlay_pipeline
            .as_ref()
            .is_some_and(|overlay| overlay.format == format)
        {
            return;
        }
        let shader = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("erika-wgpu-overlay-shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("wgpu_overlay.wgsl").into()),
            });
        let bind_group_layout =
            self.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("erika-wgpu-overlay-bgl"),
                    entries: &[
                        wgpu::BindGroupLayoutEntry {
                            binding: 0,
                            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Uniform,
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 1,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Texture {
                                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                                view_dimension: wgpu::TextureViewDimension::D2,
                                multisampled: false,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 2,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                            count: None,
                        },
                    ],
                });
        let layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("erika-wgpu-overlay-layout"),
                bind_group_layouts: &[Some(&bind_group_layout)],
                immediate_size: 0,
            });
        let pipeline = self
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("erika-wgpu-overlay-pipeline"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("erika_overlay_vertex"),
                    buffers: &[],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleStrip,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("erika_overlay_fragment"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        // Straight-alpha blending, matching the Metal overlay pipeline.
                        blend: Some(wgpu::BlendState {
                            color: wgpu::BlendComponent {
                                src_factor: wgpu::BlendFactor::SrcAlpha,
                                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                                operation: wgpu::BlendOperation::Add,
                            },
                            alpha: wgpu::BlendComponent {
                                src_factor: wgpu::BlendFactor::One,
                                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                                operation: wgpu::BlendOperation::Add,
                            },
                        }),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                multiview_mask: None,
                cache: None,
            });
        let sampler = self.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("erika-wgpu-overlay-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        self.overlay_pipeline = Some(OverlayPipeline {
            pipeline,
            bind_group_layout,
            sampler,
            format,
        });
        self.ensure_danmaku_batch_pipeline(format);
    }

    fn ensure_danmaku_batch_pipeline(&mut self, format: wgpu::TextureFormat) {
        if self
            .danmaku_batch_pipeline
            .as_ref()
            .is_some_and(|p| p.format == format)
        {
            return;
        }
        let shader = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("erika-wgpu-danmaku-batch-shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("wgpu_danmaku_batch.wgsl").into()),
            });
        let bind_group_layout =
            self.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("erika-wgpu-danmaku-batch-bgl"),
                    entries: &[
                        wgpu::BindGroupLayoutEntry {
                            binding: 0,
                            visibility: wgpu::ShaderStages::VERTEX,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Uniform,
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 1,
                            visibility: wgpu::ShaderStages::VERTEX,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Storage { read_only: true },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 2,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Texture {
                                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                                view_dimension: wgpu::TextureViewDimension::D2,
                                multisampled: false,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 3,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                            count: None,
                        },
                    ],
                });
        let layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("erika-wgpu-danmaku-batch-layout"),
                bind_group_layouts: &[Some(&bind_group_layout)],
                immediate_size: 0,
            });
        let pipeline = self
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("erika-wgpu-danmaku-batch-pipeline"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("erika_danmaku_batch_vertex"),
                    buffers: &[],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("erika_danmaku_batch_fragment"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState {
                            color: wgpu::BlendComponent {
                                src_factor: wgpu::BlendFactor::SrcAlpha,
                                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                                operation: wgpu::BlendOperation::Add,
                            },
                            alpha: wgpu::BlendComponent {
                                src_factor: wgpu::BlendFactor::One,
                                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                                operation: wgpu::BlendOperation::Add,
                            },
                        }),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                multiview_mask: None,
                cache: None,
            });
        let sampler = self.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("erika-wgpu-danmaku-batch-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        self.danmaku_batch_pipeline = Some(DanmakuBatchPipeline {
            pipeline,
            bind_group_layout,
            sampler,
            format,
        });
    }

    fn ensure_danmaku_instance_buffer(&mut self, required_len: usize) {
        if required_len == 0 {
            return;
        }
        if self.danmaku_instance_buffer_len >= required_len && self.danmaku_instance_buffer.is_some()
        {
            return;
        }
        let len = required_len.next_power_of_two().max(4096);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("erika-wgpu-danmaku-instance-buffer"),
            size: len as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.danmaku_instance_buffer = Some(buffer);
        self.danmaku_instance_buffer_len = len;
    }

    fn render_surface_clear(&mut self, color: WgpuClearColor) -> Result<()> {
        let Some(attached) = self.surface.as_ref() else {
            return Err(PlayerError::Renderer(
                "no wgpu surface attached".to_string(),
            ));
        };
        let frame = match attached.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture)
            | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => texture,
            wgpu::CurrentSurfaceTexture::Lost => {
                self.configure_surface(attached.config.width, attached.config.height);
                return Ok(());
            }
            other => {
                return Err(PlayerError::Renderer(format!(
                    "wgpu surface acquire failed: {other:?}"
                )));
            }
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("erika-wgpu-surface-encoder"),
            });
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("erika-wgpu-surface-clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(color.to_wgpu()),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }
        self.queue.submit(Some(encoder.finish()));
        frame.present();
        self.stats.rendered_frames += 1;
        Ok(())
    }

    fn configure_surface(&mut self, width: u32, height: u32) {
        let Some(attached) = self.surface.as_mut() else {
            return;
        };
        attached.config.width = width.max(1);
        attached.config.height = height.max(1);
        attached.surface.configure(&self.device, &attached.config);
        self.stats.surface_width = attached.config.width;
        self.stats.surface_height = attached.config.height;
    }

    #[cfg(target_os = "windows")]
    fn try_upload_d3d11va_frame(
        &mut self,
        frame: &PlayerVideoFrame,
    ) -> Result<Option<()>> {
        let texture_ptr = match frame.frame.d3d11va_texture_ptr() {
            Some(ptr) => ptr,
            None => return Ok(None),
        };

        let shared_info = match unsafe { crate::windows::get_d3d11_texture_shared_handle(texture_ptr) } {
            Ok(info) => info,
            Err(_) => return Ok(None),
        };

        let width = frame.frame.width();
        let height = frame.frame.height();

        let hal_device_guard = unsafe { self.device.as_hal::<wgpu::hal::dx12::Api>() };
        let hal_device = match hal_device_guard.as_deref() {
            Some(d) => d,
            None => return Ok(None),
        };

        let d3d12_device = hal_device.raw_device();

        let d3d12_resource: windows::Win32::Graphics::Direct3D12::ID3D12Resource = unsafe {
            let mut result = None;
            if d3d12_device
                .OpenSharedHandle(shared_info.handle, &mut result)
                .is_err()
            {
                return Ok(None);
            }
            match result {
                Some(r) => r,
                None => return Ok(None),
            }
        };

        let hal_texture = unsafe {
            wgpu::hal::dx12::Device::texture_from_raw(
                d3d12_resource,
                wgpu::TextureFormat::NV12,
                wgpu::TextureDimension::D2,
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                1,
                1,
            )
        };

        let wgpu_texture = unsafe {
            self.device.create_texture_from_hal::<wgpu::hal::dx12::Api>(
                hal_texture,
                &wgpu::TextureDescriptor {
                    label: Some("erika-wgpu-d3d11va-nv12"),
                    size: wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::NV12,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
            )
        };

        let luma_view = wgpu_texture.create_view(&wgpu::TextureViewDescriptor {
            aspect: wgpu::TextureAspect::Plane0,
            ..wgpu::TextureViewDescriptor::default()
        });
        let chroma_view = wgpu_texture.create_view(&wgpu::TextureViewDescriptor {
            aspect: wgpu::TextureAspect::Plane1,
            ..wgpu::TextureViewDescriptor::default()
        });

        let source = SourceColorState::new(
            frame.frame.color_primaries(),
            frame.frame.transfer_function(),
        )
        .range(frame.frame.color_range())
        .matrix(frame.frame.matrix_coefficients())
        .hdr_metadata(frame.frame.hdr_metadata());
        let pipeline =
            VideoRenderPipeline::new(source, TargetColorState::sdr(ColorPrimaries::Bt709));
        let uniforms = VideoUniforms::from_pipeline(&pipeline, false, false);

        self.current_video = Some(UploadedVideoFrame {
            luma: luma_view,
            chroma: chroma_view,
            width,
            height,
            uniforms,
        });

        Ok(Some(()))
    }
}

fn scaled_surface_size(width: u32, height: u32, scale: f64) -> (u32, u32) {
    let scale = if scale.is_finite() {
        scale.max(1.0)
    } else {
        1.0
    };
    let scaled = |value: u32| ((value.max(1) as f64) * scale).round().min(u32::MAX as f64) as u32;
    (scaled(width), scaled(height))
}

impl RendererBackend for WgpuRenderer {
    fn attach_surface(&mut self, surface: PlatformSurface) -> Result<()> {
        let PlatformSurface::Wgpu(handle) = surface else {
            return Err(PlayerError::Renderer(
                "non-wgpu surface cannot be attached to WgpuRenderer".to_string(),
            ));
        };

        // SAFETY: `create_surface_unsafe` requires the raw handle to point at a live
        // CAMetalLayer that outlives the returned surface. The embedder owns the layer
        // for the lifetime of the attachment, mirroring the Metal renderer contract.
        let target = match handle.kind {
            WgpuSurfaceKind::MacOsCaMetalLayer => {
                #[cfg(any(target_os = "macos", target_os = "ios"))]
                {
                    wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(handle.raw_window as *mut c_void)
                }
                #[cfg(not(any(target_os = "macos", target_os = "ios")))]
                {
                    return Err(PlayerError::Renderer(
                        "CoreAnimationLayer surface is only available on Apple platforms"
                            .to_string(),
                    ));
                }
            }
            WgpuSurfaceKind::WindowsHwnd => {
                use std::num::NonZeroIsize;

                let hwnd = handle.raw_window as *mut c_void;
                if hwnd.is_null() {
                    return Err(PlayerError::Renderer(
                        "invalid hwnd: null pointer".to_string(),
                    ));
                }
                let hwnd_nonzero = match NonZeroIsize::new(hwnd as isize) {
                    Some(nz) => nz,
                    None => {
                        return Err(PlayerError::Renderer(
                            "invalid hwnd: zero value".to_string(),
                        ));
                    }
                };
                let raw_window = raw_window_handle::Win32WindowHandle::new(hwnd_nonzero);
                let raw_display = raw_window_handle::WindowsDisplayHandle::new();
                wgpu::SurfaceTargetUnsafe::RawHandle {
                    raw_display_handle: Some(raw_display.into()),
                    raw_window_handle: raw_window.into(),
                }
            }
            other => {
                return Err(PlayerError::Renderer(format!(
                    "wgpu surface kind {other:?} is not wired yet"
                )));
            }
        };
        let surface = unsafe { self.instance.create_surface_unsafe(target) }.map_err(|error| {
            PlayerError::Renderer(format!("wgpu surface creation failed: {error}"))
        })?;

        let mut caps = surface.get_capabilities(&self.adapter);
        if caps.formats.is_empty() {
            let compatible_adapter = pollster::block_on(
                self.instance.request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    force_fallback_adapter: false,
                    compatible_surface: Some(&surface),
                }),
            )
            .map_err(|error| {
                PlayerError::Renderer(format!(
                    "wgpu adapter has no compatible formats and fallback adapter request failed: {error}"
                ))
            })?;
            self.adapter = compatible_adapter;
            let supports_16bit_norm = self
                .adapter
                .features()
                .contains(wgpu::Features::TEXTURE_FORMAT_16BIT_NORM);
            let supports_vulkan_ext_mem_win32 = self
                .adapter
                .features()
                .contains(wgpu::Features::VULKAN_EXTERNAL_MEMORY_WIN32);
            let mut required_features = wgpu::Features::empty();
            if supports_16bit_norm {
                required_features |= wgpu::Features::TEXTURE_FORMAT_16BIT_NORM;
            }
            if supports_vulkan_ext_mem_win32 {
                required_features |= wgpu::Features::VULKAN_EXTERNAL_MEMORY_WIN32;
            }
            let (device, queue) =
                pollster::block_on(self.adapter.request_device(&wgpu::DeviceDescriptor {
                    label: Some("erika-wgpu-device"),
                    required_features,
                    required_limits: wgpu::Limits::default(),
                    memory_hints: wgpu::MemoryHints::default(),
                    experimental_features: wgpu::ExperimentalFeatures::default(),
                    trace: wgpu::Trace::Off,
                }))
                .map_err(|error| {
                    PlayerError::Renderer(format!(
                        "wgpu device request for compatible adapter failed: {error}"
                    ))
                })?;
            self.device = device;
            self.queue = queue;
            self.supports_16bit_norm = supports_16bit_norm;
            caps = surface.get_capabilities(&self.adapter);
            if caps.formats.is_empty() {
                return Err(PlayerError::Renderer(
                    "wgpu surface reports no supported formats even with compatible adapter"
                        .to_string(),
                ));
            }
        }
        // Prefer a non-sRGB format: the video shader already emits display-encoded
        // values for the SDR target, so an sRGB surface would double-encode gamma.
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|format| !format.is_srgb())
            .unwrap_or_else(|| caps.formats[0]);
        let (surface_width, surface_height) =
            scaled_surface_size(handle.width, handle.height, handle.scale);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: surface_width,
            height: surface_height,
            present_mode: caps.present_modes[0],
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
        };
        surface.configure(&self.device, &config);

        self.stats.surface_width = config.width;
        self.stats.surface_height = config.height;
        self.stats.attached = true;
        self.surface = Some(AttachedSurface {
            surface,
            config,
            handle,
        });
        Ok(())
    }

    fn detach_surface(&mut self) -> Result<()> {
        self.surface = None;
        self.stats.attached = false;
        Ok(())
    }

    fn resize_surface(&mut self, width: u32, height: u32, scale: f64) -> Result<()> {
        if self.surface.is_none() {
            return Err(PlayerError::Renderer(
                "no wgpu surface attached".to_string(),
            ));
        }
        let (surface_width, surface_height) = scaled_surface_size(width, height, scale);
        self.configure_surface(surface_width, surface_height);
        if let Some(attached) = self.surface.as_mut() {
            attached.handle.width = width;
            attached.handle.height = height;
            attached.handle.scale = scale;
        }
        Ok(())
    }

    fn render_test_frame(&mut self, time_seconds: f64) -> Result<()> {
        let color = WgpuClearColor::animated(time_seconds);
        if self.surface.is_some() {
            self.render_surface_clear(color)
        } else {
            // No surface: exercise the GPU path headlessly and count it as a frame.
            self.clear_offscreen(16, 16, color)?;
            self.stats.rendered_frames += 1;
            Ok(())
        }
    }

    fn upload_player_frame(&mut self, frame: &PlayerVideoFrame) -> Result<()> {
        #[cfg(target_os = "windows")]
        if self.try_upload_d3d11va_frame(frame)?.is_some() {
            return Ok(());
        }

        let sw_frame;
        let frame_ref = if frame.frame.has_hw_frames_context() {
            sw_frame = frame
                .frame
                .transfer_to_system_memory()
                .map_err(|e| PlayerError::Renderer(format!("hwframe transfer failed: {e}")))?;
            &sw_frame
        } else {
            &frame.frame
        };

        let planar = frame_ref.to_planar_frame().ok_or_else(|| {
            PlayerError::Renderer(
                "wgpu: frame is not software 4:2:0 8-bit/10-bit (unsupported format)"
                    .to_string(),
            )
        })?;
        let is_p010 = matches!(planar.format, PlanarPixelFormat::P010);
        let source = SourceColorState::new(
            frame_ref.color_primaries(),
            frame_ref.transfer_function(),
        )
        .range(frame_ref.color_range())
        .matrix(frame_ref.matrix_coefficients())
        .hdr_metadata(frame_ref.hdr_metadata());
        let pipeline =
            VideoRenderPipeline::new(source, TargetColorState::sdr(ColorPrimaries::Bt709));
        let uniforms = VideoUniforms::from_pipeline(&pipeline, is_p010, false);
        self.upload_planar_with_context(planar, uniforms)
    }


    fn render_current_frame(&mut self, context: RenderFrameContext<'_>) -> Result<bool> {
        if self.current_video.is_none() {
            return Ok(false);
        }
        let Some(format) = self.surface.as_ref().map(|attached| attached.config.format) else {
            // No surface to present to (e.g. ticked before attach); the presenter
            // falls back to a test frame.
            return Ok(false);
        };
        self.ensure_video_pipeline(format);
        let danmaku = context.danmaku.filter(|plan| {
            plan.generation == context.generation
                && plan.media_time == context.media_time
                && (context.output_width == 0 || plan.viewport.width == context.output_width)
                && (context.output_height == 0 || plan.viewport.height == context.output_height)
        });
        if context.overlay.is_some_and(overlay_has_planes)
            || danmaku.is_some_and(|plan| !plan.is_empty())
        {
            self.ensure_overlay_pipeline(format);
        }
        let attached = self.surface.as_ref().expect("surface present");
        let frame = match attached.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture)
            | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => texture,
            wgpu::CurrentSurfaceTexture::Lost => {
                self.configure_surface(attached.config.width, attached.config.height);
                return Ok(false);
            }
            other => {
                return Err(PlayerError::Renderer(format!(
                    "wgpu surface acquire failed: {other:?}"
                )));
            }
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let danmaku_draws = self.draw_current_video(&view, context.overlay, danmaku)?;
        frame.present();
        self.stats.rendered_frames += 1;
        if danmaku_draws > 0 {
            self.stats.danmaku_passes += 1;
            self.stats.danmaku_items += danmaku_draws as u64;
        }
        Ok(true)
    }

    fn runtime_stats(&self) -> RendererRuntimeStats {
        let stats = self.stats();
        RendererRuntimeStats {
            surface_width: stats.surface_width,
            surface_height: stats.surface_height,
            rendered_frames: stats.rendered_frames,
            offscreen_frames: stats.offscreen_frames,
            prepared_overlay_frames: 0,
            prepared_overlay_subtitle_planes: 0,
            danmaku_passes: stats.danmaku_passes,
            danmaku_draw_items: stats.danmaku_items,
            overlay_alpha_atlas_uploads: 0,
            overlay_alpha_atlas_reuses: 0,
            last_danmaku_atlas_duration: Default::default(),
            last_danmaku_vertex_build_duration: Default::default(),
            last_danmaku_vertex_copy_duration: Default::default(),
            last_danmaku_encode_duration: Default::default(),
            last_danmaku_vertex_bytes: 0,
            last_danmaku_vertex_count: 0,
            attached: stats.attached,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::MetalSurfaceHandle;
    use crate::danmaku::{
        DanmakuFrameStats, DanmakuGlyphAtlas, DanmakuGlyphInstance, DanmakuRenderPlan,
        DanmakuViewport,
    };
    use std::time::Duration;

    fn to_u8(component: f64) -> u8 {
        (component * 255.0).round() as u8
    }

    #[test]
    fn wgpu_renderer_clears_offscreen_target_to_expected_color() {
        let mut renderer = WgpuRenderer::new().unwrap();
        let color = WgpuClearColor::new(0.25, 0.5, 0.75, 1.0);

        let readback = renderer.clear_offscreen(4, 3, color).unwrap();

        assert_eq!(readback.width, 4);
        assert_eq!(readback.height, 3);
        assert_eq!(readback.rgba.len(), 4 * 3 * 4);
        let expected = [
            to_u8(color.red),
            to_u8(color.green),
            to_u8(color.blue),
            to_u8(color.alpha),
        ];
        for y in 0..readback.height {
            for x in 0..readback.width {
                let pixel = readback.pixel(x, y);
                // Allow a tolerance of 1 LSB for rounding differences across drivers.
                for channel in 0..4 {
                    let delta = (pixel[channel] as i16 - expected[channel] as i16).unsigned_abs();
                    assert!(
                        delta <= 1,
                        "pixel ({x},{y}) channel {channel} = {} expected ~{}",
                        pixel[channel],
                        expected[channel]
                    );
                }
            }
        }
        assert_eq!(renderer.stats().offscreen_frames, 1);
    }

    #[test]
    fn wgpu_renderer_render_test_frame_without_surface_uses_offscreen_path() {
        let mut renderer = WgpuRenderer::new().unwrap();

        renderer.render_test_frame(0.0).unwrap();

        let stats = renderer.stats();
        assert_eq!(stats.rendered_frames, 1);
        assert_eq!(stats.offscreen_frames, 1);
        assert!(!stats.attached);
    }

    #[test]
    fn wgpu_renderer_prepares_danmaku_glyph_atlas_draws_and_reuses_cache() {
        let mut renderer = WgpuRenderer::new().unwrap();
        renderer.ensure_overlay_pipeline(OFFSCREEN_FORMAT);
        let atlas = DanmakuGlyphAtlas {
            width: 4,
            height: 4,
            stride: 4,
            fill_alpha: vec![255; 16],
            outline_alpha: vec![64; 16],
            version: 42,
        };
        let plan = DanmakuRenderPlan {
            media_time: Duration::from_millis(10),
            generation: 7,
            viewport: DanmakuViewport::new(32, 18),
            atlas: Some(std::sync::Arc::new(atlas.clone())),
            items: vec![DanmakuGlyphInstance {
                item_id: 1,
                rect: [1.0, 2.0, 4.0, 4.0],
                tex_rect: [0.0, 0.0, 1.0, 1.0],
                color_rgba: [1.0, 1.0, 1.0, 1.0],
                outline_rgba: [0.0, 0.0, 0.0, 0.75],
                shadow_rgba: [0.0, 0.0, 0.0, 0.0],
                shadow_offset: [1.0, 1.0],
            }],
            frame_stats: DanmakuFrameStats::default(),
        };

        let draws = renderer.prepare_danmaku_draws(&plan).unwrap();
        assert_eq!(draws.len(), 2);
        assert!(
            renderer
                .danmaku_atlas_cache
                .as_ref()
                .is_some_and(|cache| cache.can_reuse_for(&atlas))
        );

        let cached_draws = renderer.prepare_danmaku_draws(&plan).unwrap();
        assert_eq!(cached_draws.len(), 2);
        assert!(
            renderer
                .danmaku_atlas_cache
                .as_ref()
                .is_some_and(|cache| cache.can_reuse_for(&atlas))
        );
    }

    #[test]
    fn wgpu_renderer_rejects_metal_surface() {
        let mut renderer = WgpuRenderer::new().unwrap();
        let result = renderer.attach_surface(PlatformSurface::Metal(MetalSurfaceHandle::new(
            42, 640, 360, 2.0,
        )));

        assert!(matches!(result, Err(PlayerError::Renderer(_))));
    }

    // --- Video pipeline parity oracle ---------------------------------------
    //
    // `reference_pixel` is a CPU port of the WGSL `erika_video_fragment` (which is
    // itself a port of the Metal `VIDEO_SHADER_SOURCE`). Asserting the GPU output
    // matches this reference proves the wgpu backend computes the same color math
    // as the native Metal renderer for the same uniforms.

    fn ref_pq_eotf(encoded: f32) -> f32 {
        let m1 = 0.1593017578125;
        let m2 = 78.84375;
        let c1 = 0.8359375;
        let c2 = 18.8515625;
        let c3 = 18.6875;
        let p = encoded.max(0.0).powf(1.0 / m2);
        let num = (p - c1).max(0.0);
        let den = (c2 - c3 * p).max(0.000001);
        (num / den).powf(1.0 / m1)
    }

    fn ref_transfer_to_source_linear(rgb: [f32; 3], u: &VideoUniforms) -> [f32; 3] {
        let rgb = rgb.map(|c| c.max(0.0));
        match u.source_transfer {
            3 => {
                let peak = u.nits[2].max(1.0);
                rgb.map(|c| ref_pq_eotf(c) * (10000.0 / peak))
            }
            1 => rgb.map(|c| c.powf(2.2)),
            2 => rgb.map(|c| c.powf(2.4)),
            _ => rgb,
        }
    }

    fn ref_gamut(rgb: [f32; 3], u: &VideoUniforms) -> [f32; 3] {
        let m = u.gamut_matrix_rows;
        [
            m[0][0] * rgb[0] + m[0][1] * rgb[1] + m[0][2] * rgb[2],
            m[1][0] * rgb[0] + m[1][1] * rgb[1] + m[1][2] * rgb[2],
            m[2][0] * rgb[0] + m[2][1] * rgb[1] + m[2][2] * rgb[2],
        ]
    }

    fn ref_tone_map(nits: [f32; 3], u: &VideoUniforms) -> [f32; 3] {
        let source_peak = u.nits[0].max(1.0);
        let target_peak = u.nits[1].max(1.0);
        let white = (source_peak / target_peak).max(1.0);
        let x = nits.map(|n| n.max(0.0) / target_peak);
        match u.tone_map {
            1 => {
                let white2 = white * white;
                x.map(|xi| target_peak * (xi * (1.0 + xi / white2) / (1.0 + xi)).clamp(0.0, 1.0))
            }
            2 => {
                let knee = 0.75;
                let denom = (white - knee).max(0.0001);
                x.map(|xi| {
                    let t = ((xi - knee) / denom).clamp(0.0, 1.0);
                    let shoulder = knee + (1.0 - knee) * (1.0 - (1.0 - t).powf(2.0));
                    let s = if xi >= knee { shoulder } else { xi };
                    target_peak * s
                })
            }
            _ => x.map(|xi| target_peak * xi.clamp(0.0, 1.0)),
        }
    }

    fn ref_output(rgb: [f32; 3], u: &VideoUniforms) -> [f32; 3] {
        if u.edr_output != 0 {
            return rgb.map(|c| c.max(0.0));
        }
        match u.target_transfer {
            1 => rgb.map(|c| c.max(0.0).powf(1.0 / 2.2)),
            2 => rgb.map(|c| c.max(0.0).powf(1.0 / 2.4)),
            _ => rgb,
        }
    }

    fn ref_final(rgb: [f32; 3], u: &VideoUniforms) -> [f32; 3] {
        if u.edr_output != 0 {
            let headroom = (u.nits[1].max(1.0) / u.nits[3].max(1.0)).max(1.0);
            rgb.map(|c| c.clamp(0.0, headroom))
        } else {
            rgb.map(|c| c.clamp(0.0, 1.0))
        }
    }

    fn reference_pixel(y: f32, cb: f32, cr: f32, u: &VideoUniforms) -> [f32; 3] {
        let (yy, cbcr) = if u.full_range != 0 {
            (y, [cb - 0.5, cr - 0.5])
        } else if u.is_p010 != 0 {
            (
                (y - 64.0 / 1023.0) * (1023.0 / 876.0),
                [
                    (cb - 512.0 / 1023.0) * (1023.0 / 896.0),
                    (cr - 512.0 / 1023.0) * (1023.0 / 896.0),
                ],
            )
        } else {
            (
                (y - 16.0 / 255.0) * (255.0 / 219.0),
                [
                    (cb - 128.0 / 255.0) * (255.0 / 224.0),
                    (cr - 128.0 / 255.0) * (255.0 / 224.0),
                ],
            )
        };
        let kr = u.luma_coefficients[0];
        let kg = u.luma_coefficients[1].max(0.000001);
        let kb = u.luma_coefficients[2];
        let r = yy + 2.0 * (1.0 - kr) * cbcr[1];
        let b = yy + 2.0 * (1.0 - kb) * cbcr[0];
        let g = (yy - kr * r - kb * b) / kg;
        let mut rgb = [r, g, b];
        rgb = ref_transfer_to_source_linear(rgb, u);
        rgb = ref_gamut(rgb, u);
        let srw = u.nits[2].max(1.0);
        rgb = rgb.map(|c| c.max(0.0) * srw);
        rgb = ref_tone_map(rgb, u);
        let trw = u.nits[3].max(1.0);
        rgb = rgb.map(|c| c.max(0.0) / trw);
        rgb = ref_output(rgb, u);
        ref_final(rgb, u)
    }

    fn build_solid_nv12(width: u32, height: u32, y: u8, cb: u8, cr: u8) -> (Vec<u8>, Vec<u8>) {
        let luma = vec![y; (width * height) as usize];
        let chroma_pixels = (width / 2) as usize * (height / 2) as usize;
        let mut chroma = Vec::with_capacity(chroma_pixels * 2);
        for _ in 0..chroma_pixels {
            chroma.push(cb);
            chroma.push(cr);
        }
        (luma, chroma)
    }

    #[test]
    fn wgpu_video_nv12_matches_cpu_reference() {
        let mut renderer = WgpuRenderer::new().unwrap();

        let sdr = VideoUniforms::from_pipeline(&VideoRenderPipeline::sdr_default(), false, false);
        assert_eq!(sdr.source_transfer, 1);
        assert_eq!(sdr.nits[2], 100.0);

        // A full-range BT.709 identity configuration: linear in/out, clip tone map,
        // matched nits, identity gamut. Output should be the plain clamped YCbCr->RGB.
        let mut identity = sdr;
        identity.full_range = 1;
        identity.source_transfer = 0;
        identity.target_transfer = 0;
        identity.tone_map = 0;
        identity.nits = [100.0, 100.0, 100.0, 100.0];
        identity.luma_coefficients = [0.2126, 0.7152, 0.0722, 0.0];
        identity.gamut_matrix_rows = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
        ];

        let samples = [
            (16u8, 128u8, 128u8),
            (128, 128, 128),
            (200, 90, 160),
            (80, 200, 64),
            (235, 128, 128),
        ];

        for uniforms in [sdr, identity] {
            for (y, cb, cr) in samples {
                let (luma, chroma) = build_solid_nv12(4, 4, y, cb, cr);
                let out = renderer
                    .render_nv12_offscreen(4, 4, &luma, &chroma, uniforms)
                    .unwrap();

                let expect = reference_pixel(
                    f32::from(y) / 255.0,
                    f32::from(cb) / 255.0,
                    f32::from(cr) / 255.0,
                    &uniforms,
                );
                let expected = [
                    to_u8(f64::from(expect[0])),
                    to_u8(f64::from(expect[1])),
                    to_u8(f64::from(expect[2])),
                    255,
                ];

                for py in 0..out.height {
                    for px in 0..out.width {
                        let pixel = out.pixel(px, py);
                        for channel in 0..4 {
                            let delta =
                                (pixel[channel] as i16 - expected[channel] as i16).unsigned_abs();
                            assert!(
                                delta <= 2,
                                "ycbcr ({y},{cb},{cr}) full_range={} pixel ch{channel} = {} expected ~{}",
                                uniforms.full_range,
                                pixel[channel],
                                expected[channel]
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn wgpu_renderer_is_usable_as_dyn_backend_and_reports_no_current_frame() {
        let mut renderer = WgpuRenderer::new().unwrap();
        // The presenter holds the backend as `Box<dyn RendererBackend>`; confirm the
        // wgpu renderer is object-safe through the trait and reports no current frame
        // so the presenter falls back to a test frame.
        let backend: &mut dyn RendererBackend = &mut renderer;
        assert!(
            !backend
                .render_current_frame(RenderFrameContext::new(Duration::ZERO, 1))
                .unwrap()
        );
    }

    #[test]
    fn wgpu_uploads_and_renders_p010_frame() {
        let mut renderer = WgpuRenderer::new().unwrap();
        if !renderer.supports_16bit_norm() {
            // Backend without TEXTURE_FORMAT_16BIT_NORM cannot do P010; skip.
            return;
        }

        // 4x4 P010 frame: bright luma, neutral chroma. Samples are 10-bit values
        // MSB-aligned in 16-bit LE (code << 6), matching `Frame::to_planar_frame`.
        let luma_sample: u16 = 700 << 6;
        let chroma_sample: u16 = 512 << 6;
        let luma: Vec<u8> = std::iter::repeat(luma_sample)
            .take(4 * 4)
            .flat_map(u16::to_le_bytes)
            .collect();
        let chroma: Vec<u8> = std::iter::repeat(chroma_sample)
            .take(2 * 2 * 2)
            .flat_map(u16::to_le_bytes)
            .collect();

        let uniforms =
            VideoUniforms::from_pipeline(&VideoRenderPipeline::sdr_default(), true, false);
        renderer
            .upload_planar(
                PlanarFrame {
                    format: PlanarPixelFormat::P010,
                    width: 4,
                    height: 4,
                    luma,
                    chroma,
                },
                uniforms,
            )
            .unwrap();

        let readback = renderer
            .render_current_offscreen(None)
            .unwrap()
            .expect("p010 frame rendered");
        assert_eq!(readback.width, 4);
        assert_eq!(readback.height, 4);
        // A bright luma frame must not render fully black.
        assert!(readback.rgba.iter().any(|&byte| byte > 0));
    }

    #[test]
    fn wgpu_video_rejects_wrong_plane_sizes() {
        let mut renderer = WgpuRenderer::new().unwrap();
        let uniforms =
            VideoUniforms::from_pipeline(&VideoRenderPipeline::sdr_default(), false, false);

        // Luma too short for a 4x4 frame.
        let result = renderer.render_nv12_offscreen(4, 4, &[0u8; 8], &[0u8; 8], uniforms);
        assert!(matches!(result, Err(PlayerError::Renderer(_))));

        // Odd dimensions are rejected.
        let result = renderer.render_nv12_offscreen(3, 4, &[0u8; 12], &[0u8; 4], uniforms);
        assert!(matches!(result, Err(PlayerError::Renderer(_))));
    }

    #[test]
    fn wgpu_overlay_uniforms_map_source_rect_into_presentation_layout() {
        let layout = super::VideoPresentationLayout::aspect_fit(1920, 1080, 1000, 1000);
        let uniforms = super::OverlayUniforms::rgba_plane(960, 540, 192, 108, layout);

        assert_eq!(uniforms.viewport, [1000.0, 1000.0]);
        for (actual, expected) in uniforms.rect.into_iter().zip([500.0, 500.0, 100.0, 56.25]) {
            assert!((actual - expected).abs() < 0.001, "{actual} != {expected}");
        }
    }
}
