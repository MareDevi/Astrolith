# 编辑器性能 M3：Writing 接入视口文本布局

- 日期：2026-11-06。
- 对应方案：[编辑器性能优化实施方案](editor-performance-implementation-plan.md) M3。
- 状态：**Writing 产品路径已切换**到应用自有布局核心；Preview 与 Build 仍走现有实现。
- 环境：Linux 7.3.1-1-cachyos x64，Hyprland/Wayland，scale 1，gpui-pre 后端，dev profile，无 profiler。
- 证据：`artifacts/e2e/editor-m3-pre-*`。

## 1. 交付

| 交付 | 位置 |
| --- | --- |
| Writing 控制器复用 M2 布局核心 | `crates/astrolith-gpui/src/editor/writer_surface.rs` |
| 可见行绘制、选择、caret、装饰与 Focus 变暗 | `editor/text_layout/mod.rs` 的 `TextLayout::paint` |
| 命中、选择、垂直移动、IME bounds 共用同一行快照 | `writer_surface.rs`、`writer_surface/input.rs` |
| 局部编辑增量复用 | `TextLayout::update`，安装新解析修订时调用 |
| 计数 | `astrolith::perf` 的 `writer_frame` 与 `text_layout_frame` |

`WriterSurface` 仍是控制器：`EditorState` 继续负责光标、选择、composition、剪贴板与 undo；布局只读不可变的
`WysiwygDocument`，负责断行、可见行、字形、命中与几何。Kit 的 `Paragraph`/`BlockNode`、
`editor_host::collect_leaf_geometry` 与 `RichFlow` 已从 Writing 渲染路径移除。

## 2. 行为边界

- 点击、Shift 扩选、拖选、Home/End、上下左右移动和选择工具条读取同一份布局几何；上下移动按可见行，
  Markdown 隐藏语法仍由左右移动跳过。
- 链接与图片命中仍落到 `WysiwygDocument` 的 source map；颜色、行内代码背景、标题字号/粗体、
  列表标记、引用边线、代码背景与水平线由布局绘制。
- Focus 模式按 cursor sentence 变暗其他块；Copyedit 与 snippet placeholder 以渲染字节范围绘制装饰。
- 未解析的 caret 行 fail-closed：caret 跟随会跨帧继续，直到目标行完成 wrap/shape 并可见；
  用户滚走后光标不变则不再把视口拉回。
- 宽度或字体度量变化重建布局；仅颜色变化保留断行与行索引，只重绘可见行。
- 解析尚未收敛时，选择与几何继续使用已接受的修订，并通过 `TextEdit` 映射到当前 source；
  过时几何不会解释成 EOF。

## 3. 原生结果

同一构建（`target/editor-perf/pre/debug`）、`ASTROLITH_EDITOR_PERF=1`、`scroll-input` 场景
（打开 → 滚动 −4000 px → 回顶 → 输入 `中🦀` → 退出）：

| fixture | 结果 | writer frames | 总 CPU p50 / p95 / p99 | render max | 绘制块数 | 单帧 shape 字节上限 | change p95 |
| --- | --- | ---: | --- | ---: | ---: | ---: | ---: |
| `history-single-paragraph-102420` | passed | 82 | 1.13 / 3.90 / 10.41 ms | 10.39 ms | 1 | 10,076 | 0.62 ms |
| `history-multiblock-1112000` | passed | 86 | 1.13 / 4.44 / 7.55 ms | 4.94 ms | 10 / 906 | 4,895 | 2.38 ms |
| `plain-long-line-1mib` | passed | 85 | 1.00 / 3.44 / 3.82 ms | 3.31 ms | 1 | 8,193 | 4.93 ms |

六向模式切换（`cycle`，2 轮，同一 102,420 B fixture）：passed；30 个 writer frame，
p50 0.68 ms、p95 6.52 ms、p99 11.05 ms，12 次 mode handler p95 0.62 ms。
这些数值是 dev 构建、无 profiler 的进程内阶段时间，不含 compositor presentation。

`writer_frame` 的 `render` 阶段包含 `TextLayout::prepare`（wrap/shape 预算）；canvas 的
`request_layout`/`prepaint` 只做尺寸与 bounds 记录，因此这两项显示为 0。冷布局构造仍在首个
render 内完成，最长单帧出现在首次建立布局时。

## 4. 自动化验证

`mise run check` 通过（doctor、格式、lint、构建、headless tests、fixture 兼容性）。重点用例：

- 长段落在末尾输入后跨帧解析出 caret 几何，缩窗后仍能定位；
- 大文档首帧与编辑后都只 wrap 视口，不退回全文布局；
- 跳转到中部 block 后，caret 可见、hit-test 与 source 范围一致；
- 无变化光标时滚动不被拉回；typewriter 居中不移动选择；
- 原生 composition 的文本、marked range、一次 undo 与几何来源仍通过现有 headless 用例；
- 光标闪烁不重建布局、不重新 wrap；
- Focus 模式变暗集合随可见 caret 句更新。

## 5. 未完成与限制

- 真实 fcitx5 候选窗口、物理键盘、拖选与剪贴板原生验收（editor-04/05）仍为 `not_run`；
  headless composition 不能替代平台证据。
- bidi 视觉顺序、组合字符与 ZWJ emoji 只确认可生成几何；未做自动视觉断言。
- tab 仍按字素原样 shaping，未实现 tab stop 展开（沿用 M2 限制）。
- 宽度变化重建布局，滚动偏移按像素保留而不是按文档锚点恢复；精确锚点属于后续滚动锚点工作。
- 颜色变化重绘可见行；断行与行索引保留，但 shaped 行会重新生成。
- 冷布局构造（`TextLayout::new` 的 run 解析）仍在首帧内，未分帧。
- Preview 与 Build 尚未接入共享布局；M4/M6 未开始。

## 6. 复现

```sh
node scripts/editor-perf.mjs run \
  --fixture history-single-paragraph-102420 \
  --backend pre --scenario scroll-input \
  --test-id editor-m3-pre-history-single-paragraph-102420

node scripts/editor-perf.mjs run \
  --fixture history-single-paragraph-102420 \
  --backend pre --scenario cycle --cycles 2 \
  --test-id editor-m3-pre-cycle-history-single-paragraph-102420
```
