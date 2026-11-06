# 编辑器性能 M6 评估：stock Editor 长行不达标，需接入 SourceSurface

- 日期：2026-11-06。
- 对应方案：[编辑器性能优化实施方案](editor-performance-implementation-plan.md) M6。
- 状态：**评估完成，未通过**。Build 的 stock Kit `Editor` 在长单行 fixture 上远超帧预算；按方案进入 `SourceSurface` 接入。M6 的替换实施尚未完成。
- SourceSurface 后续实施见 [SourceSurface 结果](editor-performance-m6-source-surface-results.md)：帧预算已通过，功能门槛未过，产品路径仍为 stock Editor。
- 环境：Linux 7.3.1-1-cachyos x64，Hyprland/Wayland，scale 1，gpui-pre 后端，dev profile，无 profiler。
- 证据：`artifacts/e2e/editor-m6-pre-build-*`、`artifacts/e2e/editor-m6-probe-build-wrap-*`。

## 1. 测量入口

方案要求 M6 先复测 stock Editor，再决定保留或替换。此前 Build 没有专用阶段计时，本轮补上：

| 交付 | 位置 |
| --- | --- |
| Build 帧阶段计时 | `astrolith::perf` 的 `build_frame`；`editor/perf.rs` 的 `build_frame_element` 包装 Kit `Editor` 元素 |
| Build 驱动动作 | `shell/perf.rs`：横/纵向滚动、光标到首尾、末尾输入、tab 输入、折叠开关、软换行开关、等宽字号/字体变化、窗口缩放 |
| Build 场景 | `scripts/editor-perf.mjs` 的 `build`（滚动 + 末尾输入 + tab + 字体 + 缩放 + folding）与 `build-wrap`（软换行对照） |
| 汇总 | harness 新增 `build_frames`：total 与 layout/prepaint/paint 的 p50/p95/p99/max |

`build_frame` 只覆盖 Kit `Editor` 元素自身的 request-layout → prepaint → paint CPU 阶段，不含 compositor
presentation；计数关闭时包装层直接返回原元素，不增加元素与计时分支。驱动动作与输入仍调用真实窗口、真实
`EditorState` 输入适配器，不修改磁盘文件。

## 2. 原生结果（dev、无 profiler）

`build` 场景：进入 Build → 纵向滚动 2 轮 → 横向滚动 → 回顶 → 光标末尾输入 `中🦀` → 输入 tab →
等宽字号改 16 → 等宽字体改 `DejaVu Sans Mono` → 缩窗 1000×640 → 恢复 1600×1000 → folding 关/开。

| fixture | 帧数 | total p50 | p95 | p99 | max | layout max | prepaint max | paint max |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `plain-long-line-1mib`（1 MiB 单行） | 59 | 17.01 ms | **2,744.37 ms** | **2,779.48 ms** | 2,779.48 ms | 5.52 ms | 2,767.25 ms | 12.57 ms |
| `history-single-paragraph-102420`（102 KB 单行） | 149 | 1.43 ms | 2.87 ms | **116.20 ms** | 123.47 ms | 7.41 ms | 115.49 ms | 1.76 ms |
| `history-multiblock-1112000`（1.1 MB 多块） | 151 | 1.22 ms | 2.78 ms | 17.14 ms | 21.61 ms | 4.84 ms | 12.70 ms | 5.57 ms |
| `complex-markdown`（96 KB，2,956 块） | 131 | 1.08 ms | 2.71 ms | 13.44 ms | 14.90 ms | 7.29 ms | 10.22 ms | 3.13 ms |

- 多块文档满足方案第 3 节的帧 CPU 目标（p95 ≤ 16.7 ms、p99 ≤ 33 ms）。
- 1 MiB 单行每帧仍在 `prepaint` 整行整形，p95 2.74 s；横向滚动、末尾输入与字体变化都会重新整形整行。
- 102 KB 单行的稳定帧很低，但进入 Build 的首帧、末尾输入后的 caret reveal 与等宽字体变化各产生
  110–123 ms 的同步整形帧，p99 超出 33 ms，并出现 ≥100 ms 的已知同步工作段。
- 慢帧归因：`layout` 始终很小，成本集中在 `prepaint`；与 `editor-bottlenecks-0.9.md` 的 Build 结论一致。

### 软换行对照（被否定的替代方案）

同一 1 MiB fixture、同一构建，`build-wrap` 场景在进入 Build 后打开 `soft_wrap(true)`：

| 场景 | 帧数 | p50 | p95 | max | prepaint max |
| --- | ---: | ---: | ---: | ---: | ---: |
| `build-wrap` | 16 | 17.62 ms | 2,641.90 ms | 2,641.90 ms | 2,629.57 ms |

软换行没有消除首次整行测量：`TextWrapper` 在文本/宽度变化时于 UI 线程预先计算换行，仍要处理整行；
稳定帧 p50 也升至约 17 ms。它既不满足冷长行门槛，又改变 Build 的横向滚动语义，因此不采用。

## 3. 代码事实与公开 API 边界

- Kit `TextElement::layout_lines` 对每个可见 buffer line 调用 `shape_line` 整形**整行**文本；
  纵向虚拟化只按 buffer line 裁剪，软换行关闭时 1 MiB 单行就是一个可见行。公开 API 没有按水平视口
  裁剪整形输入或替换行渲染器的钩子。
- `EditorState` 的 `display_map()` 只读；折叠只有 `set_folding`、`unfold_at` 与 gutter 点击路径，
  没有公开的 toggle/折叠设置入口。`set_soft_wrap` 存在，但不能解决整行测量。
- LSP completion、hover、code action、编辑器内搜索与 diagnostics 绘制由 Kit `Input` 元素承担；
  不挂载该元素即失去这些 UI。Writing 的 `WriterSurface` 已经证明“自有布局 + `EditorState` 输入桥”
  可行，但当时没有同样的 Build 功能集合要保留。
- 因此“仅调参数或保留 stock Editor”无法满足 1 MiB 单行与 p99 预算，方案的条件分支成立：
  **需要 `SourceSurface`，以源文本 + 高亮 run 使用共用布局核心，并保留现有输入/LSP 行为。**

## 4. 决策与后续实施边界

决策：**Build 保持现有实现直到 `SourceSurface` 达到功能与性能门槛；M6 不关闭。** 不采用软换行、
不恢复隐藏 Editor、不通过截断/降级高亮规避整形。

后续实施必须先解决以下项，再替换产品路径：

1. 源文本行索引与可见纵向窗口；每行按合法字素/字体 run/双向上下文边界切出水平窗口整形，
   保留光标窗口外的水平滚动与命中；不得按屏幕左边界切任意字节。
2. 复用 M5 决策：`EditorState` 继续作为输入、composition、选区与 undo 桥；`SourceSurface` 作为可见
   `EntityInputHandler`，几何与 caret/IME bounds 由自有布局提供。
3. 高亮 run 由现有 highlighter 结果生成，diagnostics 装饰、行号、folding gutter、搜索/替换、
   completion/hover/code action 的宿主方案逐项验证；任何无法用公开 API 保留的能力必须在替换前
   单独实现，不能静默消失。
4. 以本轮的 `build`/`build-wrap` 场景和四个 fixture 作为回归基线；长单行、末尾输入、横/纵向滚动、
   字体/缩放与 folding 重新测量。

## 5. 复现

```sh
node scripts/editor-perf.mjs run \
  --fixture plain-long-line-1mib \
  --backend pre --scenario build --cycles 2 --settle 2000 \
  --test-id editor-m6-pre-build-plain-long-line-1mib

node scripts/editor-perf.mjs run \
  --fixture history-single-paragraph-102420 \
  --backend pre --scenario build --cycles 2 --settle 2000 \
  --test-id editor-m6-pre-build-history-single-paragraph-102420

node scripts/editor-perf.mjs run \
  --fixture plain-long-line-1mib \
  --backend pre --scenario build-wrap --settle 2000 \
  --test-id editor-m6-probe-build-wrap-plain-long-line-1mib
```

## 6. 未执行与限制

- release profile 未测量；本轮只覆盖 dev。
- 折叠只测 `set_folding` 开关与 gutter 重绘，真实 gutter 点击折叠没有公开脚本入口，保持 `not_run`。
- 等宽字体切换通过 perf 驱动的主题变更完成，不是用户设置路径；默认等宽字体族的名称未单独记录。
- 真实 fcitx5 预编辑、候选窗口、物理键盘与剪贴板仍属 editor-04；本轮输入只走 `EditorState` 适配器。
- 这些数值不含 compositor presentation 与 GPU 呈现，不代表端到端可见延迟。
