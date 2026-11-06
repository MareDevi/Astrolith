# 编辑器性能 M4：Preview 接入共享阅读布局

- 日期：2026-11-06。
- 对应方案：[编辑器性能优化实施方案](editor-performance-implementation-plan.md) M4。
- 状态：**Preview 产品路径已切换**到应用自有视口布局；`PreviewPages`、8 KiB 分页、`ListState` 高度修复和 Kit `TextView` 子视图已从 Preview 移除。Writing 与 Preview 共用解析结果和布局核心；Build 仍走现有实现。
- 环境：Linux 7.3.1-1-cachyos x64，Hyprland/Wayland，scale 1，gpui-pre 后端，dev profile，无 profiler。
- 证据：`artifacts/e2e/editor-m4-pre-*`；外观检查用同一 harness 的 `preview-idle` 场景在真实窗口截屏（截图本身不提交）。

## 1. 交付

| 交付 | 位置 |
| --- | --- |
| Preview 只读视图：滚动、选择、键盘导航、复制、链接激活、滚动锚点 | `crates/astrolith-gpui/src/editor/rendered.rs` |
| 共享解析结果 | `WriterSurface::shared_document`；shell 在 Preview 同步时传入同一 `Arc<WysiwygDocument>` |
| 表格网格、脚注、原始 HTML/MDX、独立图片的布局与绘制 | `editor/text_layout/mod.rs` |
| 图片块资源加载与 `window.paint_image` 绘制 | `rendered.rs` 的 `PreviewImagePainter`、`ImageCacheItem` |
| 语义模型扩展 | `astrolith-document/src/wysiwyg.rs`：`TableRow`、`Footnote`、`Raw`、`Image`、引用式链接/图片、引用定义与脚注定义 |
| 计数 | `astrolith::perf` 的 `text_layout_frame` 新增 `mode="preview"` |
| harness | `scripts/editor-perf.mjs` 的 `preview`、`preview-idle` 场景；`Shell::apply_perf_step` 在 Preview 模式驱动阅读滚动 |

`RenderedDocument` 只持有不可变 `Arc<WysiwygDocument>`、自己的 `TextLayout`、选择/焦点/滚动状态和图片缓存；不再有
`TextViewState`、页面订阅或 `ListState`。同一接受修订下，Writer 已解析的文档直接复用，Preview 不再重复解析；
仅在 Writer 尚未产出该文本（例如直接在测试中挂载 Preview）时，等待约 1.2 s 后回退到自己的后台解析。

## 2. 语义差异矩阵

| Markdown 结构 | Writing | Preview | 说明 |
| --- | --- | --- | --- |
| 标题、强调、删除线、行内代码、链接文字 | 排版呈现 | 排版呈现 | 共用 `resolve_runs`；链接保留颜色 |
| 列表、引用、围栏代码、分隔线 | 排版呈现 | 排版呈现 | 列表标记与引用边线由布局绘制 |
| GFM 竖线表格 | 网格呈现 | 网格呈现 | 表头加粗+底色；列对齐；单元格独立换行 |
| 引用式链接/图片 `[x][r]`、`![a][i]` | 目标解析 | 目标解析 | 定义行不显示；未解析时保留原文字面 |
| 脚注定义 `[^1]: body` | `[1] body` 块 | 同左 | 脚注引用 `[^1]` 保留为可见字面文本，不重编号/不加返回链接 |
| 原始 HTML/MDX 流 | 等宽、弱化文本块 | 同左 | 原样保留，不执行、不嵌入组件 |
| 独立成段图片 | alt 文本 | 实际图片 | 相对路径、Astro `public/` 与 `http(s)` 由 GPUI asset cache 异步加载；未完成时显示 alt 占位 |
| 行内图片 | alt 文本 | alt 文本 | 与 Writing 一致；不在段落内嵌图片对象 |
| Select All + Copy | 正文 source | 正文 source | 保持既有策略 |
| 鼠标/键盘选择 + Copy | 编辑选择 | 只读选择 | Preview 复制显示文本；表格单元格以 `\t` 分隔 |
| 链接点击 | 由编辑面处理 | `http`/`https`/`mailto` 交给系统打开 | 内部相对链接不激活 |

## 3. 原生结果

同一 harness、`ASTROLITH_EDITOR_PERF=1`、`--backend pre`、dev 构建，`preview` 场景（进入 Preview →
滚动正 1200/2400 px → 回顶，重复两轮）与 `preview-idle`；数值为进程内 `text_layout_frame` 阶段时间。

| fixture | 场景 | Preview 帧 | prepare max | 单帧 wrap 上限 | 单帧 shape 上限 | 测得总高度 |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| `history-single-paragraph-102420` | preview | 8 | 4.30 ms | 9,663 B | 9,356 B | 13,868 px |
| `complex-markdown`（2,956 块） | preview | 6 | 1.87 ms | 447 B | 394 B | 274,310 px |
| `plain-long-line-1mib`（单块） | preview-idle | 3 | 4.60 ms | 7,382 B | 8,064 B | 262,156 px |

- 102 KB 单段落在 8 帧内完成可见区准备；稳定帧 `wrapped_rows=0`、`shaped_rows=0`，只有复用行。
- 96 KB、2,956 块的 `complex-markdown` 每帧只处理视口附近几十行；表格、脚注、原始块与图片没有触发整篇布局。
- 1 MiB 单块每帧仍受 4 ms 预算限制，wrap 与 shape 都是固定窗口，不随全文增长。
- 六向模式切换（`cycle`，2 轮）：12 次 mode handler p50 0.31 ms、p95 0.88 ms；Preview 侧 prepare max 4.01 ms。
- 原生外观检查确认：表头底色、列分隔线、中文单元格、脚注块、原始 MDX 块、图片占位与围栏代码均正确绘制，
  文本随窗口宽度重新断行；Preview 中不再出现 `combine_highlights` 的累计前缀重合并路径，因为 Kit `TextView`
  不再挂载。

这些是 dev 构建、无 profiler 的进程内阶段时间，不含 compositor presentation；p95/p99 见各 artifact 的
`perf.json`。Build 的专用阶段计时仍不存在，M6 再验收。

## 4. 自动化验证

`mise run check` 通过（doctor、格式、lint、构建、headless tests、fixture 兼容性）。新增/更新的重点用例：

- `astrolith-document`：表格行/列对齐/表头加粗与 source 映射；引用式链接和图片解析且定义隐藏；脚注定义与
  原始 HTML/MDX 内容保留；独立图片成为 `Image` 块。
- `text_layout`：表格单元格独立换行、列原点上的 caret/hit-test、跨单元格选择矩形；图片已知尺寸与回退尺寸，
  以及 `set_image_metrics` 的局部高度更新。
- `RenderedDocument`：同一帧不重复安装正文；Select All 复制 source；跨块选择复制显示文本；长文档首帧只
  wrap 视口并在 resize 后重建布局。
- `shell`：Preview 进入/退出、模式切换与未变化帧的安装计数保持既有行为。

## 5. 未完成与限制

- 表格列为等宽，不支持 colspan、列宽按内容收缩或超宽表格的横向滚动；列对齐已生效。
- 脚注引用仍是可见的 `[^1]` 文本，没有上标、编号或回链；脚注定义以块形式展示。
- 原始 HTML/MDX 只显示源码，不解释内联标签、不执行组件。
- 行内图片保留 alt 文本；独立成段图片才绘制图片。
- 图片高度按设备像素比例估算，未针对显示器 scale 做精确校准；加载完成后只更新图片块高度。
- 表格/图片块的缓存生命周期沿用文档布局；关闭文档时随 `RenderedDocument` 一起释放，未单独统计表格几何账本。
- 真实 fcitx5、拖选与链接原生验收仍属于 editor-04/05 的后续平台证据；本里程碑的 Preview 是只读面，
  不注册输入处理器。
- Build 尚未接入共享布局；M5–M7 未开始。

## 6. 复现

```sh
node scripts/editor-perf.mjs run \
  --fixture history-single-paragraph-102420 \
  --backend pre --scenario preview --cycles 2 \
  --test-id editor-m4-pre-preview-history-single-paragraph-102420

node scripts/editor-perf.mjs run \
  --fixture complex-markdown \
  --backend pre --scenario preview --cycles 2 \
  --test-id editor-m4-pre-preview-complex-markdown
```
