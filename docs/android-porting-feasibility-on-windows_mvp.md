# 以 `windows_mvp` 为基线汲取 `main` 优化并扩展 Android 平台可行性分析

> 生成时间：2026-10-04
> 基线分支：`windows_mvp` @ `df3695e` (136 文件，6 独有提交)
> 汲取来源：`main` @ `83114cd` (569 文件，393 独有提交)
> 共同祖先：`403049af` (2026-06-08)

---

## 一、总体评估

**结论：技术可行，推荐分阶段执行，预估为中大型工程。**

两个子任务**正交**，可并行规划但需协调基础设施移植顺序：

| 子任务 | 目标 | 与 Android 的关系 |
|--------|------|-------------------|
| **A. 汲取 main 的 Windows 优化** | 增强 windows_mvp 的 Windows 深度 | 独立，不影响 Android |
| **B. 扩展 Android 平台** | 从 main 移植 Android 全栈 | 独立，不依赖 A |

核心依据：`windows_mvp` 与 `main` 的 Android 代码共享同一 wgpu::hal 互操作范式，且 `windows_mvp` 的弹幕/字幕优化是**平台无关**的，Android 可直接受益。

---

## 二、Android 移植技术拆解

### 2.1 需移植的源码清单

| 层 | 文件 | 规模 | 依赖 |
|----|------|------|------|
| **NDK FFI** | `crates/erika/src/android.rs` | ~400 行 | `trace` ✅、NDK 符号 |
| **硬解** | `crates/erika/src/android/mediacodec.rs` | ~800 行 | NDK AImageReader/AHardwareBuffer/AMediaCodec |
| **渲染器** | `crates/erika/src/renderer/android_vulkan.rs` | **~1900 行** | `ash`、`wgpu::hal::vulkan`、`renderer::pipeline`、`android::mediacodec`、`trace` |
| **着色器** | `renderer/android_ahb.frag` + `.vert` | 小 | AHardwareBuffer 采样 |
| **Flutter 插件** | `packages/erika_flutter/android/` | **30+ Kotlin 文件** | gradle + NDK + Flutter embedding |
| **构建** | `build.gradle` + `erika-native.gradle` + `settings.gradle` | 中 | Android NDK + CMake |

### 2.2 依赖变更

```toml
# crates/erika/Cargo.toml 需新增
[target.'cfg(target_os = "android")'.dependencies]
ash = "0.38.0"
```

```rust
// crates/erika/src/lib.rs 需新增
#[cfg(target_os = "android")]
pub mod android;
```

### 2.3 android_vulkan.rs 内部依赖分析（关键）

经 grep 分析，`android_vulkan.rs` 的 `crate::` 依赖仅：

| 依赖 | windows_mvp 是否具备 | 处置 |
|------|---------------------|------|
| `crate::renderer::pipeline::{ColorRange, MatrixCoefficients}` | ✅ 有 `pipeline.rs` | 需确认 API 签名一致 |
| `crate::trace::diagnostic` | ✅ 有 `trace` 模块 | 直接可用 |
| `crate::android::mediacodec::describe_hardware_buffer` | ❌ 需移植 | 同批移植 |

**结论：内部依赖链浅，障碍比预期小。** 主要风险在 `pipeline.rs` 的 API 是否在两分支间已分叉。

### 2.4 wgpu::hal 互操作兼容性（核心可行性点）

| 分支 | hal 使用 | 目标 Backend |
|------|----------|-------------|
| `windows_mvp` | `wgpu::hal::dx12::Api` + `wgpu::hal::vulkan::Device::texture_from_d3d11_shared_handle` | DX12/Vulkan (Windows) |
| `main` (Android) | `wgpu::hal::vulkan::Api` + `Instance::init_with_callback` + `create_adapter_from_hal` | Vulkan (Android) |

两分支**共用 `wgpu::hal::vulkan` 路径**，仅平台 surface 和纹理导入源不同（Windows: D3D11 shared handle / Android: AHardwareBuffer）。`windows_mvp` 的 wgpu 29.0.3 在 Windows 已能暴露 hal，Android target 下同版本同样支持 `hal::vulkan`。

**判定：hal 互操作层兼容，无需切换 wgpu 版本。**

---

## 三、`windows_mvp` 作为 Android 基线的优势

1. **wgpu 统一渲染范式**：windows_mvp 的策略是「wgpu 高层 API + hal 纹理共享」，Android 的 `android_vulkan.rs` 正是同一范式（hal 互操作导入外部纹理）。架构一致，无需引入 main 的 D3D11 原生渲染器双后端复杂度。

2. **弹幕/字幕优化自动生效**：`wgpu_danmaku_batch.wgsl`（WGSL 着色器）和 libass 增量缓存（Rust 逻辑）**完全平台无关**。Android 移植后直接获得：
   - alpha 批量字幕渲染
   - 事件指纹增量更新（`process_chunk`）
   - overlay atlas 纹理/uniform buffer 缓存
   - danmaku 签名比对避免重复上传

   这是 `main` 分支 Android **不具备**的优化。

3. **无历史包袱**：windows_mvp 仅 136 文件，不包含 main 的 Linux/iOS/OHOS 代码，Android 移植不会与无关平台代码冲突。

4. **构建工具链可扩展**：xtask 已支持 MSVC，可新增 Android NDK 交叉编译目标（`aarch64-linux-android` / `x86_64-linux-android`）。

---

## 四、障碍与风险矩阵

| # | 障碍 | 严重度 | 缓解方案 |
|---|------|--------|----------|
| 1 | **`pipeline.rs` API 分叉** | ⚠️ 中 | 对比两分支 `pipeline.rs` 差异，若 main 的 Android 代码依赖 main 独有的 pipeline API，需一并移植或适配 |
| 2 | **main 独有基础设施模块** | ⚠️ 中 | `debug_hud`、`export`、`luma_stats`、`subtitle_charset` 可能被 Android 代码间接依赖，需逐个确认并移植 |
| 3 | **Flutter 插件结构差异** | 🔴 高 | windows_mvp 的 Flutter 插件仅基础 C ABI，main 的 Android 插件是 30+ Kotlin 文件的完整实现（lifecycle/surface/media session/audio focus）。需整体移植 Dart 层 + Kotlin 层 + gradle |
| 4 | **构建系统双轨** | ⚠️ 中 | windows_mvp 用 xtask + vcpkg，Android 需 gradle + NDK + CMake。需决定：xtask 调用 gradle？还是 Android 独立用 gradle？ |
| 5 | **wgpu hal feature 显式启用** | ⚠️ 低 | windows_mvp 的 Cargo.toml 未显式 `features = ["hal"]`，但 Windows 构建已用 hal。需在 Android target 验证 hal 可见性，必要时显式启用 |
| 6 | **393 提交的基础设施漂移** | 🔴 高 | main 的 Android 代码基于 06-08 分叉后的 main 演进，可能引用了 main 后续提交新增的 API/类型。需以 Android 代码为起点，递归排查缺失依赖 |
| 7 | **FFmpeg 交叉编译** | ⚠️ 中 | windows_mvp 用 vcpkg FFmpeg，Android 需 NDK 交叉编译 FFmpeg（或用 main 的构建脚本）。erika_ffmpeg_sys 的 Android 构建需验证 |
| 8 | **测试与验证** | ⚠️ 中 | windows_mvp 无 Android 测试环境，移植后需 Android 设备/模拟器验证 Vulkan + MediaCodec + AHardwareBuffer 全链路 |

---

## 五、分阶段路线图

### 阶段 0：基础设施对齐（前置）
- 对比 `pipeline.rs`、`core/` 公共 API 在两分支的差异
- 移植 `debug_hud`、`luma_stats`、`subtitle_charset` 等 Android 间接依赖的平台无关模块
- 确认 wgpu 29.0.3 在 Android target 的 hal 可见性

### 阶段 1：Rust 原生层移植
1. `Cargo.toml` 添加 `ash` android target 依赖
2. `lib.rs` 添加 `#[cfg(target_os = "android")] pub mod android;`
3. 移植 `android.rs` + `android/mediacodec.rs`（NDK FFI + MediaCodec 硬解）
4. 移植 `renderer/android_vulkan.rs` + `android_ahb.frag/vert`（Vulkan + AHardwareBuffer 渲染）
5. 适配 `pipeline.rs` API 差异
6. Android NDK 交叉编译验证 `cargo build --target aarch64-linux-android`

### 阶段 2：FFmpeg 交叉编译
- 配置 `erika_ffmpeg_sys` 的 Android NDK 构建
- 或复用 main 的 FFmpeg Android 构建脚本

### 阶段 3：Flutter Android 插件移植
1. 移植 `packages/erika_flutter/android/` 全部 30+ Kotlin 文件
2. 移植 `build.gradle` + `erika-native.gradle` + `settings.gradle`
3. 确认 Dart 层 platform channel 与 windows_mvp 的 C ABI 一致（可能需适配）
4. 移植 Android 单元测试

### 阶段 4：集成验证
- Android 设备 Vulkan + MediaCodec 硬解全链路
- 弹幕/字幕渲染验证（确认 windows_mvp 优化在 Android 生效）
- HDR（SCRGB_LINEAR dataspace）验证
- Flutter Android 插件端到端播放

### 阶段 5（可选）：汲取 main 的 Windows 优化
- 与 Android 移植并行，不互相阻塞
- D3D11 原生渲染器 / ArtCNN / Flutter Windows 完整插件 / HDR 管线

---

## 六、关键决策点

| # | 决策 | 选项 | 推荐 |
|---|------|------|------|
| 1 | 渲染后端策略 | (a) 维持 wgpu 统一 / (b) 引入 main 的 D3D11 原生双后端 | (a) 维持 wgpu 统一，与 Android 架构一致 |
| 2 | 构建工具链 | (a) xtask 统一管 Android / (b) Android 独立 gradle | (b) Android 独立 gradle，xtask 仅管 Rust 交叉编译 |
| 3 | Flutter 插件 Dart 层 | (a) 适配 windows_mvp 的 C ABI / (b) 直接用 main 的 | 需对比两分支 Dart 层差异后决定 |
| 4 | 是否同时移植 Linux/iOS/OHOS | (a) 仅 Android / (b) 全平台 | (a) 仅 Android，控制范围 |
| 5 | pipeline.rs API 对齐方向 | (a) windows_mvp 适配 main / (b) main 代码适配 windows_mvp | 取决于哪边 API 更成熟，需逐函数对比 |