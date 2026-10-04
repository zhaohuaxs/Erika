# `pipeline.rs` 两分支 API 差异深度对比

> 生成时间：2026-10-04
> 决策已确认：引入 main 的 D3D11 原生双后端
> `main` pipeline.rs: **3316 行** / `windows_mvp` pipeline.rs: **867 行**（差 +2449 行）

---

## 一、API 差异全景

### 1.1 main 独有的公共 API（windows_mvp 完全缺失）

| 类别 | API | 说明 |
|------|-----|------|
| **常量** | `VIDEO_INPUT_MODE_MASK`, `VIDEO_INPUT_PACKED_ALPHA_RIGHT`, `VIDEO_INPUT_P010_BYTES` | 视频输入模式标志 |
| **Dolby Vision** | `DOVI_MAX_PIECES`, `DOVI_MAX_MMR_ORDER`, `DOVI_MMR_COEFFS` | DV RPU 常量 |
| | `DoviComponentCurve` struct | 每分量重塑曲线 |
| | `DoviFramePq` struct | 每帧 L1 PQ 亮度元数据 |
| | `DoviSourceMetadata` struct | RPU 载荷 |
| | `DoviUniforms` struct + `disabled()`/`of()`/`of_for_representation()` | Shader uniform 块 (~3KB) |
| | `pq_code_to_nits()`, `pq_code_from_nits()`, `nits_from_pq_code()` | PQ EOTF/OETF |
| | `dovi_lms_to_rgb_matrix()` | DV LMS→RGB 复合矩阵 |
| **IPT 色调映射** | `ipt_rgb2lms_matrix()`, `ipt_lms2rgb_matrix()` | libplacebo IPT 空间转换 |
| **Gamut LUT** | `primaries_code()`, `code_to_primaries()`, `gamut_primaries_code()` | 色域 LUT 键编码 |
| **AI 超分** | `LumaUpscalerMode` enum (Off/ArtCnnC4F16/ArtCnnC4F16Ds/ArtCnnC4F32) + `is_enabled()` | ArtCNN 模式 |
| **RenderGraph** | `RenderPassKind` enum, `RenderPass` struct, `RenderGraph` struct | 渲染 pass 图调度 |

### 1.2 枚举变体差异

| 枚举 | `main` | `windows_mvp` |
|------|--------|---------------|
| `ToneMapOperator` | Clip, Reinhard, Mobius, **Bt2390, Spline, Bt2446a, St209410** (7) | Clip, Reinhard, Mobius (3) |
| `ToneMapOperator::default()` | **Bt2390** | Mobius |

### 1.3 struct 字段差异

| struct | `main` 独有字段 | `windows_mvp` 独有字段 |
|--------|----------------|----------------------|
| `SourceColorState` | `dovi: Option<DoviSourceMetadata>`, `measured_scene_avg_nits: Option<f32>` | 无 |
| `ToneMapConfig` | `curve_param: f32`, `contrast_ratio: f32` | `knee_start: f32`, `desaturate: f32` |
| `ToneMapConfig::default()` | `{Bt2390, 0.0, 0.0}` | `{Mobius, 0.75, 0.0}` |

> ⚠️ `ToneMapConfig` **字段完全不兼容**，非增量差异而是替换关系。

### 1.4 方法差异

| 类型 | `main` 独有方法 | `windows_mvp` 独有方法 |
|------|----------------|----------------------|
| `SourceColorState` | `dovi()`, `measured_scene_avg_nits()` | 无 |
| `TargetColorState` | `sdr_tone_map_target()`, `extended_linear()`, `hdr10()` | `windows_hdr()` |
| `HdrMetadata` | `max_frame_average_light_level_nits()` | 无 |
| `ContentLightMetadata` | `max_frame_average_light_level_nits()` | 无 |
| `ToneMapConfig` | `effective_curve_param()` | 无 |
| `RgbMatrix` | `mul`/`mul_vec`/`inverse` 为 `pub(crate)` | 同方法但**私有** |

### 1.5 `HdrMetadata::nominal_peak_nits()` 实现分歧

- **main**：mastering peak 与 CLL 取 `min`（当 CLL ≥ 10），逻辑复杂
- **windows_mvp**：CLL 优先，fallback 到 mastering，逻辑简单

---

## 二、消费者依赖矩阵

### 2.1 `android_vulkan.rs` → pipeline 依赖

```
use crate::renderer::pipeline::{ColorRange, MatrixCoefficients};
```

| 依赖项 | windows_mvp 具备 | main 独有 API |
|--------|-----------------|---------------|
| `ColorRange` | ✅ | 无 |
| `MatrixCoefficients` | ✅ | 无 |

**结论：android_vulkan.rs 对 main 独有 pipeline API 零依赖。Android 移植无需改动 pipeline.rs。**

### 2.2 `windows_mvp` 的 `wgpu.rs` → pipeline 依赖

```
use crate::renderer::pipeline::{
    ColorRange, SourceColorState, TargetColorState, ToneMapOperator, VideoRenderPipeline, ...
};
```

| 依赖项 | 使用方式 | main 兼容性 |
|--------|----------|-------------|
| `ToneMapOperator::{Clip, Reinhard, Mobius}` | `tone_map_code()` match 3 变体 | ⚠️ main 添加 4 变体，match 非穷尽需补全 |
| `SourceColorState` | 基础字段 | ⚠️ main 添加 `dovi`/`measured_scene_avg_nits` 字段，构造需适配 |
| `TargetColorState` | 基础方法 | ✅ main 是超集 |
| `VideoRenderPipeline` | 基础用法 | ⚠️ 需确认 `graph` 字段差异 |

**结论：windows_mvp 的 wgpu.rs 依赖轻，但 `ToneMapOperator` 枚举扩展会触发非穷尽 match 编译错误，需补全 4 个新变体映射。**

### 2.3 `d3d11.rs` → pipeline 依赖（重度）

```
use crate::renderer::pipeline::{
    Chromaticity, LumaUpscalerMode, PrimariesCoordinates, SourceColorState, TargetColorState,
    VideoRenderPipeline, VideoUniforms, primaries_coordinates,
};
```

| 依赖项 | main 独有 | 用途 |
|--------|----------|------|
| `LumaUpscalerMode` | ✅ | ArtCNN 超分模式（d3d11.rs 30+ 处引用） |
| `SourceColorState::dovi()` | ✅ | `.dovi(frame.frame.dovi_metadata())` 调用 |
| `VideoUniforms` | 需确认 | Shader uniform 组装 |
| `Chromaticity`, `PrimariesCoordinates`, `primaries_coordinates()` | ✅ 两分支都有 | 色域坐标 |

d3d11.rs 内嵌 HLSL 着色器实现完整的 Dolby Vision RPU reshaping + IPT tone mapping + gamut LUT，对应 pipeline.rs 的 `DoviUniforms`/`ipt_*`/`pq_code_*` API。

### 2.4 `d3d11_artcnn.rs` → pipeline 依赖

```
use crate::renderer::pipeline::LumaUpscalerMode;
use crate::renderer::artcnn::{FrameTokenCache, LayerOffsets, MIDDLE_LAYER_COUNT, model_for_mode};
```

| 依赖项 | main 独有 |
|--------|----------|
| `pipeline::LumaUpscalerMode` | ✅ |
| `renderer::artcnn` 模块 | ✅（整个模块 main 独有） |

---

## 三、合并策略

### 3.1 推荐方案：以 main pipeline.rs 为主体，丢弃 windows_mvp 独有内容

**依据**：windows_mvp 的 pipeline.rs 独有内容（`ToneMapConfig{knee_start,desaturate}`、`TargetColorState::windows_hdr()`、`RgbMatrix` 私有方法）**未被 windows_mvp 的 wgpu.rs 实际依赖**（经 grep 验证）。

| 步骤 | 操作 | 风险 |
|------|------|------|
| 1 | 用 main 的 `pipeline.rs` (3316 行) 替换 windows_mvp 的 (867 行) | 低：windows_mvp 独有内容无消费者 |
| 2 | 适配 windows_mvp 的 `wgpu.rs`：补全 `ToneMapOperator` match 的 4 个新变体 | 低：机械添加映射 |
| 3 | 适配 windows_mvp 的 `wgpu.rs`：`SourceColorState` 构造补 `dovi`/`measured_scene_avg_nits` 字段 | 低：设为 `None` 即可 |
| 4 | 确认 `VideoRenderPipeline` / `VideoUniforms` API 兼容 | 中：需逐字段对比 |

### 3.2 不推荐：双向合并

双向保留 `ToneMapConfig{knee_start,desaturate}` 和 `windows_hdr()` 会引入两套 tone map 配置语义，增加维护负担且无消费者。

### 3.3 必须一并移植的 main 独有模块

| 模块 | 被谁依赖 | 必要性 |
|------|----------|--------|
| `renderer/artcnn.rs` | d3d11_artcnn.rs | ✅ D3D11 ArtCNN |
| `renderer/presentation.rs` | d3d11.rs | ✅ PresentationLayout |
| `renderer/gamut.rs` | d3d11.rs（可能） | ✅ 色域 LUT |
| `renderer/frame.rs` | d3d11.rs（可能） | ✅ 帧抽象 |
| `renderer/output.rs` | d3d11.rs（可能） | ✅ 输出管理 |
| `core::LumaUpscalerBackendStatus` | d3d11.rs/d3d11_artcnn.rs | ✅ 超分状态 |

---

## 四、对两个子任务的影响修订

### 4.1 Android 移植（android_vulkan.rs）— 障碍降低

**原评估**：pipeline.rs API 分叉是中等障碍。
**修订评估**：**零障碍**。android_vulkan.rs 仅依赖 `ColorRange` + `MatrixCoefficients`，两分支完全一致。Android 移植**不依赖** pipeline.rs 的 main 独有 API，可与 pipeline 合并**完全解耦**。

### 4.2 D3D11 原生双后端移植 — 障碍明确

**必须的前置工作**：
1. 移植 main 的 `pipeline.rs`（+2449 行）替换 windows_mvp 版本
2. 移植 `renderer/artcnn.rs` + `presentation.rs` + `gamut.rs` + `frame.rs` + `output.rs`
3. 移植 `core::LumaUpscalerBackendStatus` 等类型
4. 适配 windows_mvp 的 `wgpu.rs` 到新 pipeline API（补全枚举 match + struct 字段）
5. 然后才能移植 `d3d11.rs` + `d3d11_artcnn.rs`

**工作量**：pipeline.rs + 5 个依赖模块 + d3d11.rs (~4500 行) + d3d11_artcnn.rs (~800 行) + wgpu.rs 适配 ≈ 大型移植。

### 4.3 修订后的执行顺序

```
阶段 0: pipeline.rs 合并（main 主体） + artcnn/presentation/gamut/frame/output 移植
阶段 1: windows_mvp wgpu.rs 适配新 pipeline API
  ├── 4a. Android 移植（与 pipeline 合并并行，零依赖）
  └── 4b. D3D11 双后端移植（依赖阶段 0+1 完成）
```

Android 和 D3D11 可**并行**推进：Android 不等 pipeline 合并，D3D11 必须等。

---

## 五、决策确认记录

| 决策 | 选择 | 影响 |
|------|------|------|
| 渲染后端策略 | **引入 main 的 D3D11 原生双后端** | 需移植 pipeline.rs +2449 行 + 5 个依赖模块 |
| pipeline.rs 合并方向 | **以 main 为主体，丢弃 windows_mvp 独有** | windows_mvp 独有内容无消费者，安全丢弃 |
| Android 与 D3D11 顺序 | **并行** | Android 零依赖 pipeline 独有 API |