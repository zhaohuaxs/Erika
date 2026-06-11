struct DanmakuBatchUniforms {
    viewport: vec2<f32>,
};

struct DanmakuBatchInstance {
    rect: vec4<f32>,
    tex_rect: vec4<f32>,
    color: vec4<f32>,
};

@group(0) @binding(0) var<uniform> uniforms: DanmakuBatchUniforms;
@group(0) @binding(1) var<storage, read> instances: array<DanmakuBatchInstance>;
@group(0) @binding(2) var atlas_texture: texture_2d<f32>;
@group(0) @binding(3) var atlas_sampler: sampler;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) tex_coord: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn erika_danmaku_batch_vertex(
    @builtin(vertex_index) vertex_id: u32,
    @builtin(instance_index) instance_id: u32,
) -> VertexOut {
    var unit_positions = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 1.0),
    );
    var tex_coords = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 1.0),
    );

    let inst = instances[instance_id];
    let pixel = inst.rect.xy + unit_positions[vertex_id] * inst.rect.zw;
    let ndc = vec2<f32>(
        pixel.x / max(uniforms.viewport.x, 1.0) * 2.0 - 1.0,
        1.0 - pixel.y / max(uniforms.viewport.y, 1.0) * 2.0,
    );

    var out: VertexOut;
    out.position = vec4<f32>(ndc, 0.0, 1.0);
    out.tex_coord = inst.tex_rect.xy + tex_coords[vertex_id] * inst.tex_rect.zw;
    out.color = inst.color;
    return out;
}

@fragment
fn erika_danmaku_batch_fragment(in: VertexOut) -> @location(0) vec4<f32> {
    let mask = textureSample(atlas_texture, atlas_sampler, in.tex_coord).r;
    return vec4<f32>(in.color.rgb, in.color.a * mask);
}