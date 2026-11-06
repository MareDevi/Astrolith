# 编辑器性能 M6：SourceSurface 实施结果

- 日期：2026-11-06。
- 对应方案：[编辑器性能优化实施方案](editor-performance-implementation-plan.md) M6。
- 状态：**SourceSurface 已实现并通过帧预算；completion、diagnostics、hover 与 code action 浮层已由公开状态宿主接管，Build 产品路径仍保留 stock Editor。** 功能门槛（folding/搜索/overview ruler 等）未全部通过，M6 不关闭。
- 环境：Linux 7.3.1-1-cachyos x64，Hyprland/Wayland，scale 1，gpui-pre 0.3.11，dev profile，无 profiler。
- 入口：debug 构建设置 `ASTROLITH_SOURCE_SURFACE=1` 时 Build 使用 SourceSurface；默认仍是 stock Kit `Editor`。perf harness 使用 `--source-surface` 记录该开关。
- 证据：`artifacts/e2e/editor-m6-surface-build-*`。

## 1. 实现

| 位置 | 职责 |
| --- | --- |
| `editor/text_layout/source.rs` | 源文本行索引、纵向可见窗口、按字素边界切分的水平测量块、带上下文的有界整形窗口、caret/hit-test/selection 几何 |
| `editor/source_surface.rs` | 可见 `EntityInputHandler`；`EditorState` 继续承担输入、composition、选区与 undo；自有绘制、行号 gutter、点击/拖选、方向选择、命令绑定与 caret 跟随 |
| `editor/input_boundaries.rs` | Writing 与 Source 共用的字素/单词边界，删除重复实现 |
| `editor/session.rs` | 按内部开关创建 surface；Build 滚动仍走同一 driver 入口 |
| `layout/panels.rs` | Build 内容选择 SourceSurface 或原 stock Editor |
| `scripts/editor-perf.mjs` | `--source-surface` 开关；`build_frame.total_ms` 计入 `prepare_ms`，并单列准备阶段 |

关键实现约束：

- 每行按 4 KiB 字素对齐块测量；只整形覆盖视口与一倍宽度的窗口，窗口左边界带 24 字节上下文并按字素对齐。
- 1 MiB 单行每帧只整形可见窗口；`shaping_bytes` 实测最大 6.9 KiB（1 MiB 行）与 7.9 KiB（102 KB 行）。
- 未解析窗口的 caret/hit-test/IME bounds 返回 `None`，不回退到估算位置。
- 行终止符不参与整形；空行有零宽窗口。
- 跨行编辑只丢弃受影响行的测量；未变行保留。
- 水平/纵向滚动共享同一 `ScrollHandle`；canvas 尺寸来自测量/估算宽度与行数，绘制只覆盖可见行。
- caret 行即使不在视口内也按 caret 偏移逐步测量并在预算内整形，用于 Ctrl+End/末尾输入；预算耗尽时下一帧继续。

## 2. 原生结果（dev、无 profiler，`scenario build`）

同一 fixture、同一驱动动作、同一 gpui-pre 后端；stock 数据来自 [M6 决策](editor-performance-m6-decision.md)。

| fixture | 实现 | 帧数 | total p50 | p95 | p99 | max | prepare max |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `plain-long-line-1mib`（1 MiB 单行） | stock Editor | 59 | 17.01 ms | 2,744.37 ms | 2,779.48 ms | 2,779.48 ms | — |
| `plain-long-line-1mib` | SourceSurface | 957 | 2.22 ms | **3.19 ms** | **4.80 ms** | 28.65 ms | 19.15 ms |
| `history-single-paragraph-102420`（102 KB 单行） | stock Editor | 149 | 1.43 ms | 2.87 ms | 116.20 ms | 123.47 ms | — |
| `history-single-paragraph-102420` | SourceSurface | 1,024 | 0.25 ms | **0.59 ms** | **0.72 ms** | 12.80 ms | 6.88 ms |
| `history-multiblock-1112000`（1.1 MB 多块） | stock Editor | 151 | 1.22 ms | 2.78 ms | 17.14 ms | 21.61 ms | — |
| `history-multiblock-1112000` | SourceSurface | — | 1.55 ms | **4.21 ms** | **7.23 ms** | 20.37 ms | — |
| `complex-markdown`（96 KB） | stock Editor | 131 | 1.08 ms | 2.71 ms | 13.44 ms | 14.90 ms | — |
| `complex-markdown` | SourceSurface | — | 0.85 ms | **1.91 ms** | **2.23 ms** | 10.31 ms | — |

- 四个 fixture 的帧 CPU p95 均低于 16.7 ms，p99 低于 33 ms；长单行的 100 ms 级同步整形段消失。
- 剩余最大值出现在首批整形/字体测量（19–29 ms），不属于稳定的每帧工作；p95 已覆盖。
- 102 KB 与 1 MiB 单行的 `shaping_bytes` 最大值分别为 7,916 与 6,912 字节，说明工作随可见窗口而不是全文增长。

## 3. 功能边界（未通过替换门槛）

SourceSurface 满足几何与性能门槛。completion、hover、code action 与 diagnostic 浮层已由 SourceSurface 通过公开状态绘制（见 §3.2–§3.4）；以下 Build 行为仍由 Kit `Input` 元素承担，surface 尚未提供对应 UI/交互：

- diagnostics overview ruler、折叠 gutter 与 folding 后的行映射；
- 编辑器内搜索/替换、bracket matching、indent guides、auto-close/smart-indent 的可见反馈；
- 真实 fcitx5 预编辑/候选窗口、物理键盘与剪贴板原生验收仍属 editor-04。

`EditorState` 仍收到文本事务并保持 undo/selection/composition，未接入的浮层与装饰不会出现。因此：

- 默认继续使用 stock Editor；`ASTROLITH_SOURCE_SURFACE=1` 仅用于 A/B 与后续接入。
- M6 仍为未完成；下一步必须先为上述能力选定可用的公开宿主方案或自行实现，再切换产品路径。
- 不得用“复杂块回退 stock Editor”掩盖未完成项。

### 3.1 公开宿主方案盘点（2026-11-06）

Kit 的浮层控件（`input/popovers/*`）是 `pub(crate)`，无法直接复用；但 `EditorState` 将浮层**状态**与**提交入口**公开，宿主可以镜像同一份状态自行绘制：

| 能力 | 公开 API | 宿主可行性 |
| --- | --- | --- |
| Completion 菜单 | `completion_menu_state()`（`open`/`items`/`query`/`trigger_start_offset`/`revision()`）、`insert_completion()`、`dismiss_completion_overlay()`、`show_completions()` | 已实施，见 §3.2 |
| 输入触发的 completion 请求 | `EditorState::replace_text_in_range` 内部会走 `on_text_typed` 与 `Lsp::update` | 无需宿主接入，输入路径已具备 |
| Hover popover | `hover_popover()`、`present_hover()`、`clear_hover_state()`、`Lsp::hover_provider` | 已实施：宿主自行 debounce 并调用 provider，见 §3.4 |
| Code action 菜单 | `code_action_menu_state()`、`present_code_actions()`、`perform_code_action()` | 已实施：宿主调用 `Lsp::code_action_providers` 后提交状态，见 §3.4 |
| Diagnostic popover | `diagnostic_popover()`、`present_diagnostic()`、`diagnostics()`（`DiagnosticSet` 可遍历） | 可读可写，宿主自行做命中与绘制 |
| Go to definition | `Lsp::definition_provider` | 导航实现（`go_to_definition`）是 `pub(crate)`，宿主需自行跳转 |
| Folding | `display_map()`（只读）、`set_folding`/`unfold_at` | 折叠后的显示映射可读；SourceSurface 尚未按 `DisplayMap` 渲染 |

结论：completion 可以完全由公开 API 承载；hover/code action/diagnostic/definition 的**呈现**可以自行实现，但它们的**触发时序**目前在 Kit 元素内部，宿主必须自己重建（provider 调用、去抖、范围解析）。这些能力在替换产品路径前必须逐项落地。

### 3.2 已实施：completion 菜单宿主

`SourceSurface` 现在镜像 `EditorState` 的公开 completion 状态：

- 位置取自有布局的 caret（内容坐标 → 滚动后的窗口坐标），弹层在下方放不下时上移，并在视口内钳制；
- 每帧最多绘制 12 行，选中行跟随 `completion_menu_state().revision()` 重置；键盘 `MoveUp`/`MoveDown` 在菜单打开时只改选中行，`Enter` 提交，`Escape` 关闭；
- 点击行提交，点击编辑区关闭菜单；`ctrl-space`（`ShowCompletions`）绑定到 `show_completions()`；
- 提交仍走 `insert_completion()`：`trigger_start_offset..cursor` 被替换，菜单由 `dismiss_completion_overlay()` 关闭，undo 与 source 事务语义不变；
- 未整形（无 caret 几何）时不显示菜单，继续 fail-closed。

验证：`completion_menu_mirrors_the_retained_overlay`（headless）覆盖状态镜像、键盘移动与提交区间；原生 `editor-m6-surface-build-completion-host` 重跑 build 场景确认渲染路径未回归。

### 3.3 已实施：diagnostics 下划线与 hover popover

`SourceSurface` 现在把 `EditorState` 的公开诊断状态投影到自有布局：

- 每个整形窗口从 `EditorState::diagnostics()` 读取与窗口相交的 `DiagnosticEntry`，与语法 run 用 `combine_highlights` 合并；诊断范围剪到整形窗口内，按严重度绘制波浪下划线（error/warning/info/hint 取 `highlight_theme.style.status` 的同名颜色）。
- 指针移动（非拖选）时按自有命中几何取字节偏移，用 `DiagnosticSet::for_offset` 查条目；命中变化时调用 `present_diagnostic()`，离开下划线调用 `clear_diagnostic_popover()`。状态与内容只在 `EditorState` 保留一份，宿主不另存诊断副本。
- 浮层从诊断起点或实际命中点的自有 caret 几何锚定，在视口内钳制，下方放不下时上移；显示消息与可选 code，边框/背景/前景取对应严重度的状态色。无已整形几何时不显示浮层，继续 fail-closed。

验证：`diagnostic_runs_underline_exactly_the_reported_range`（headless）断言 run 只覆盖诊断范围且保留下划线 `wavy`；`diagnostic_hover_mirrors_and_clears_the_retained_popover`（headless）覆盖命中锚定、状态镜像、popover 渲染与移开清除。

仍未接入：overview ruler 与 gutter 诊断标记；诊断未参与 M6 性能基线（`build` 场景未注入 LSP 诊断）。

### 3.4 已实施：hover popover 与 code action 菜单

Kit 把 hover/code action 的**触发时序**写在元素内部，SourceSurface 用公开 provider 与状态重建同一流程：

- Hover：指针停在已整形文本上后等待 150 ms，再调用 `Lsp::hover_provider`；`symbol_range` 先取 `RopeExt::word_range`，再用响应里的 `range` 校正，结果写入 `present_hover()`。每次指针移动替换待处理任务，经过的位置不会发请求；同一 symbol 内移动直接复用现有浮层；Alt、离开整形区或文本变化调用 `clear_hover_state()`。
- 浮层内容由 `HoverContents` 展平成 Markdown 交给 Kit `TextView`，锚定 symbol 起点的自有 caret 几何，视口内钳制并在下方放不下时上移；未整形时不显示，继续 fail-closed。
- Code action：`ToggleCodeActions` 打开时，宿主调用 `Lsp::code_action_providers` 的 `code_actions()`（当前 selection range），把成功的响应包装为 `CodeActionItem` 后写入 `present_code_actions()`；菜单最多绘制 12 行，`MoveUp`/`MoveDown` 在菜单打开时改选中行，`Enter` 提交，`Escape` 与点击编辑区关闭，点击行提交。
- 提交仍走 `perform_code_action(_, _, push_to_history = true)`，随后 `dismiss_code_action_overlay()`；undo 与 source 事务语义不变。文本变化会同时关闭菜单。

验证：`hover_popover_mirrors_and_clears_the_retained_state`、`hover_contents_joins_marked_strings`、`code_action_menu_mirrors_the_retained_overlay`（headless）覆盖状态镜像、内容展平、键盘移动、提交与关闭；原生 `editor-m6-surface-build-overlay-hosts`（`complex-markdown`，132 帧，total p95 2.67 ms）确认新增浮层路径未影响渲染。真实 LSP hover/code action 往返仍属 editor-09，本轮未触发。

## 4. 复现

```sh
node scripts/editor-perf.mjs run \
  --fixture plain-long-line-1mib \
  --backend pre --scenario build --cycles 2 --settle 2000 \
  --source-surface --test-id editor-m6-surface-build-plain-long-line-1mib

node scripts/editor-perf.mjs run \
  --fixture history-single-paragraph-102420 \
  --backend pre --scenario build --cycles 2 --settle 2000 \
  --source-surface --test-id editor-m6-surface-build-history-single-paragraph-102420
```

## 5. 未执行与限制

- release profile、gpui-fast 组合未测。
- 折叠开关动作仍调用 `EditorState::set_folding`；SourceSurface 不读取折叠后的 display map，视觉上未生效。
- 输入坐标验证停在 `EditorState` 适配器与 headless composition；真实 fcitx5 未执行。
- hover 与 code action 的宿主只在 headless 测试中验证状态与绘制；真实语言服务器的往返、浮层候选位置与原生输入未在本轮场景触发（属 editor-09）。
- 长 RTL/双向文本的窗口化命中只做字素/整形上下文保护，未做原生 bidi 视觉滚动验收。
- 亮度/主题仅改色的失效路径依赖每帧样式比较；未用计数器单独统计。
