# `main` vs `windows_mvp` 分支 Windows 适配差异分析

> 生成时间：2026-10-04
> 分支状态：`main` @ `83114cd` (2026-09-30) / `windows_mvp` @ `df3695e` (2026-10-03)

## 一、分支拓扑关系

| 指标 | 值 |
|------|-----|
| 共同祖先 | `403049af` (2026-06-08 "Correct C ABI function count in docs") |
| `main` 领先 | **393 个提交** (2026-06-08 → 2026-09-30) |
| `windows_mvp` 领先 | **6 个提交** (2026-06-11 → 2026-10-03) |
| 文件数 | `main` 569 / `windows_mvp` 136 |
| 差异规模 | 523 文件，+8761 / −194243 行 |

**关键结论**：两分支在 2026-06-08 同时从同一点分叉，朝**正交方向**独立演进 3+ 个月，互未合并。`windows_mvp` 并非 `main` 的简单子集，而是**早期分叉 + Windows 专项**；`main` 则是**早期分叉 + 全平台扩展**。

---

## 二、`windows_mvp` 独有的 6 个提交（Windows MVP 专项）

| 时间 | 提交 | 内容 |
|------|------|------|
| 06-11 | `4ad64b4` | Windows MVP 基础：WASAPI 音频 + D3D11VA/DXVA2 硬解 + wgpu Windows 渲染 + Flutter Windows 插件 + 弹幕描边自适应 |
| 06-12 | `b7a6796` | D3D11VA 纹理共享：shared handle 将 D3D11 纹理导入 D3D12/wgpu + 共享纹理缓存 |
| 06-13 | `46f4f25` | libass alpha 批量字幕渲染 + xtask Windows MSVC 构建 (x86_64/aarch64) |
| 06-17 | `e8df119` | overlay/danmaku 渲染缓存与增量更新（事件指纹、atlas 缓存、签名比对） |
| 06-19 | `44df35d` | vcpkg FFmpeg 检测 + libass 初始化容错 |
| 10-03 | `df3695e` | Windows HDR 显示能力检测 + 渲染管线改进 |

**架构策略**：`D3D11VA 硬解 → shared handle → wgpu(Vulkan) 渲染`。D3D11 仅承担硬解纹理产出与共享句柄，渲染统一走 wgpu 跨平台管线。Windows 特定代码集中在 `windows.rs`（纹理桥接）、`audio/wasapi.rs`、`windows_hdr.rs`。

---

## 三、`main` 独有的 Windows 相关能力（393 提交中的 Windows 部分）

| 模块 | 文件 | 说明 |
|------|------|------|
| **D3D11 原生渲染器** | `renderer/d3d11.rs` | 独立 `RendererBackend`，直接用 D3D11（swap chain/shader/blend/HDR10 元数据），绕过 wgpu |
| **D3D11 ArtCNN 超分** | `renderer/d3d11_artcnn.rs` | AI 神经超分集成到 D3D11 后端 |
| **WSL D3D12 桥接** | `renderer/linux_wsl_d3d12.cpp` | Linux 通过 WSL 复用 D3D12 GPU 平面拷贝 |
| **Flutter Windows 完整插件** | `packages/erika_flutter/windows/*` | SMTC 系统媒体控制 + Composition blend effect + Texture publication + `build_erika_runtime.cmake` + **C++ 单元测试** (player_lifecycle / texture_publication) |
| **windows_native_demo** | `examples/windows_native_demo/` | 原生 demo 示例 |
| **FFmpeg Windows 补丁** | `third_party/patches/ffmpeg-8.1.2/0002-windows-msvc-awk-backslash.patch` | MSVC 构建兼容补丁 |
| **HDR/色调映射管线** | 多处 | Dolby Vision mapping、Reinhard/Mobius tone curve、gamut LUT、P010 全精度插值、EDR/SDR 回退 |
| **软解回退** | `7b69501` | 跨平台软解 fallback |

`main` 同时包含大量非 Windows 工作：Linux VA-API/Wayland 原生、iOS DFM、Android AHB、OHOS Vulkan、Metal ArtCNN 超分、HTTP 持久流优化等。

---

## 四、核心策略对比

| 维度 | `windows_mvp` | `main` |
|------|---------------|--------|
| **渲染后端** | wgpu(Vulkan) 统一；D3D11 仅硬解纹理共享 | wgpu + **D3D11 原生**双后端 |
| **硬解→渲染** | D3D11VA → shared handle → wgpu | D3D11 原生渲染器直接消费 |
| **AI 超分** | ❌ 无 | ✅ D3D11 ArtCNN + Metal ArtCNN |
| **Flutter 插件** | 基础（C ABI + CMakeLists） | **生产级**（SMTC + 纹理发布 + 混合效果 + C++ 测试） |
| **HDR** | `windows_hdr.rs` 能力检测 | DXGI HDR10 元数据 + 完整色调映射管线 |
| **弹幕/字幕** | ✅ libass alpha 批量 + 增量缓存 + 事件指纹（**独有优化**） | 基础（无 `wgpu_danmaku_batch.wgsl`） |
| **音频** | ✅ WASAPI（`main` 无） | 需确认（可能依赖 Flutter 层） |
| **构建工具链** | xtask MSVC + vcpkg.json | CMake + FFmpeg MSVC 补丁 |
| **平台覆盖** | Windows + Apple（精简） | Windows + Linux + iOS + Android + OHOS |
| **third_party/wgpu-hal** | 已移除（约 −19 万行） | 保留 vendored 源码 |

---

## 五、各分支优势

### `windows_mvp` 优势
1. **聚焦度高**：136 文件纯 Windows+Apple，无多平台噪声，Windows 适配逻辑易于审阅
2. **渲染跨平台一致**：统一 wgpu，Windows 仅做纹理桥接，代码量小、维护面窄
3. **弹幕/字幕性能优化深入**：alpha 批量渲染、增量事件添加（`process_chunk`）、atlas 纹理/uniform buffer 缓存、签名比对避免重复上传——这是该分支**独有且有价值**的优化
4. **工具链现代**：xtask + vcpkg 对 Windows 构建友好，支持 arm64
5. **WASAPI 原生音频**：`main` 缺失的独立音频后端

### `main` 优势
1. **Windows 原生集成更深**：D3D11 原生渲染器绕过 wgpu 抽象开销，直接对接 DXGI swap chain / HDR10 / blend state
2. **Flutter 插件生产级**：SMTC（系统媒体控制）、Composition blend、Texture publication、C++ 单元测试——可发布质量
3. **AI 超分已落地**：ArtCNN 集成到 D3D11 和 Metal
4. **HDR/色彩管线成熟**：Dolby Vision + Reinhard/Mobius + gamut LUT + P010 全精度，远超 `windows_mvp` 的能力检测
5. **多平台统一基线**：393 提交的稳定性修复（HTTP/解码/teardown/软解回退），适合主开发线
6. **WSL 复用**：Linux 借 WSL D3D12 复用 Windows GPU 能力
7. **vendored wgpu-hal**：可控性强，便于针对性 patch

---

## 六、合并/整合建议

两分支分叉 3+ 月、差异 523 文件，直接 merge 冲突巨大。**不建议直接合并**，建议以 `main` 为基线定向整合 `windows_mvp` 的独有价值：

| `windows_mvp` 独有价值 | 整合可行性 | 说明 |
|------------------------|-----------|------|
| `audio/wasapi.rs` WASAPI | ✅ 高 | `main` 缺失，可 cherry-pick |
| `windows_hdr.rs` HDR 检测 | ⚠️ 中 | `main` 已有更完整 HDR 管线，需评估是否互补 |
| `wgpu_danmaku_batch.wgsl` + 弹幕增量缓存 | ✅ 高 | `main` 缺失，性能优化价值大 |
| libass alpha 批量 + 事件指纹 | ✅ 高 | `main` 缺失，字幕渲染优化 |
| xtask Windows MSVC 构建 | ⚠️ 中 | `main` 用 CMake，需决定工具链统一方向 |
| vcpkg FFmpeg 检测 | ⚠️ 低 | `main` 已有 FFmpeg 补丁方案 |