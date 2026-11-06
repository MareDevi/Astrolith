# 编辑器性能 M7：解析增量化与收尾结果

- 日期：2026-11-06。
- 对应方案：[编辑器性能优化实施方案](editor-performance-implementation-plan.md) M7。
- 状态：**本轮实施完成**。多块文档的本地编辑改走块级增量解析；单一巨大块仍整块重解析（方案允许的边界）。
  M6 的 Build 功能门槛、release 表与原生 IME/平台矩阵仍未关闭，整体任务不因本阶段关闭。
- 环境：Linux 7.3.1-1-cachyos x64，Hyprland/Wayland，scale 1，gpui-pre 0.3.11，dev profile，无 profiler。

## 1. 触发条件与决定

先补上此前缺失的解析阶段计量，再按方案判定是否需要增量解析：

| 指标 | 交付 |
| --- | --- |
| 后台解析 CPU | `astrolith::perf` 的 `parse_run`：输入字节、复用块数、耗时 |
| 输入收敛 | `document_ready`：`initial_install`（冷打开）与 `edit_install`（输入到安装）分开，另记 `layout_reconcile` |
| 关闭文档 | `document_close`；驱动新增 `close` 动作与 `m7-close` 场景 |
| 驱动场景 | `m7-input`（末尾输入多次）与 `m7-close`（六向切换后关闭无未保存修改的文档） |

整文档 worker parse 在 1.1 MB 多块 fixture 上为 99.9 ms p50 / 126.5 ms 的请求到安装，超过“输入到正确可见反馈
p95 ≤ 50 ms”的目标；96 KB 复杂 Markdown 与 102 KB 单段落仍在目标内，1 MiB 单行超出。按方案条件分支，
对受编辑影响的块做增量解析，单一巨大块保持整块重解析。

## 2. 实现

| 位置 | 职责 |
| --- | --- |
| `astrolith-document/src/wysiwyg.rs` | `parse_incremental_cancellable`：用上一修订与替换区间重解析受影响块，复用未变块；返回 `reused_blocks` |
| `writer_surface.rs` | 解析任务在启动时按“已接受文档源码 → 本快照”计算 diff；增量不适用时回退整文档解析 |
| `perf.rs` / `shell/perf.rs` | `parse_run`（含复用块数）、`document_ready` 分类、`document_close`；`close` 驱动动作 |
| `scripts/editor-perf.mjs` | `m7-input` / `m7-close` 场景；`parse_runs`、`document_convergence`、`document_close` 汇总 |

增量规则：

- 以旧块边界为锚，向前/后各扩一块，并把管道表格的连续行当作一个重解析单元；补丁两端保留行边界，
  一个 fence、表格或合并段不会被切开。
- 重解析段可越过末端锚（表格、插入的未闭合 fence、跨块合并）；被覆盖的旧块全部丢弃，只有其后的旧块
  按字节位移复用。
- 被编辑行若在旧或新文本中是链接引用定义，则回退整文档解析：定义可影响任意块的链接解析。
- 取消检查保留在块之间；被取消的解析不返回部分树。
- 复用块只平移 source 区间；渲染文本、mark 与折行结果直接保留。

正确性由 domain 测试覆盖：固定用例（标题、强调、列表、引用、fence、表格、脚注、CRLF 边界）、
400 次确定性随机编辑与整文档解析逐块比较、引用定义回退、未闭合 fence 吸收后续块。`reused_blocks`
在 1.1 MB fixture 上为 776，在 96 KB 复杂 Markdown 上为 2,955。

## 3. 原生结果（dev、无 profiler，`scenario m7-input`）

同 fixture、同驱动动作、同 gpui-pre 后端；“整文档”为增量实现前的同一场景。

| fixture | 解析路径 | parse p50 | parse p95 | 复用块 | 输入安装 p50 | 输入安装 p95 | 冷打开安装 |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `history-multiblock-1112000` | 整文档 | 99.86 ms | 102.55 ms | — | 109.80 ms | 126.53 ms | 126.53 ms |
| `history-multiblock-1112000` | 增量 | **7.53 ms** | 104.58 ms¹ | 776 | **16.09 ms** | **17.06 ms** | 131.43 ms |
| `complex-markdown`（96 KB） | 整文档 | 13.59 ms | 14.50 ms | — | 20.34 ms | 27.20 ms | 27.20 ms |
| `complex-markdown` | 增量 | **4.52 ms** | 17.51 ms¹ | 2,955 | **12.49 ms** | **15.78 ms** | 29.61 ms |
| `history-single-paragraph-102420` | 整文档/增量² | 26.87 ms | 29.30 ms | 0 | 36.77 ms | 38.49 ms | 33.18 ms |
| `plain-long-line-1mib` | 整文档/增量² | 56.67 ms | 58.25 ms | 0 | 86.18 ms | 88.38 ms | 59.29 ms |

¹ p95 含冷打开时的一次整文档解析；编辑样本本身为 4.5–7.5 ms。
² 单一巨大块没有可复用的邻块，重解析段就是该块，结果与整文档路径相同。

- 多块文档的输入收敛从 126.53 ms 降到 17.06 ms（p95），低于 50 ms 目标；布局 reconcile p50 7.3 ms。
- 1 MiB 单行仍为 86.18/88.38 ms，超过 50 ms 目标；方案明确单一巨大块可能需要整块重解析，保留为已知边界。
- 102 KB 单段落为 36.77/38.49 ms，在目标内。
- 冷打开以解析安装计：1.1 MB 为 131 ms，1 MiB 单行为 59 ms；首帧可用视口与字体准备另计，未在本轮单独采样。

## 4. 关闭与模式切换

`m7-close`：Writing → Preview → Build → Writing 后关闭无未保存修改的文档。

| fixture | `document_close` |
| --- | ---: |
| `history-multiblock-1112000` | 1.08 ms |
| `complex-markdown` | 0.38 ms |

- 该值覆盖 `close_active` 的同步阶段，不含后续延迟析构、RSS 回落或 30 分钟长运行趋势；editor-07 仍为 `not_run`。

`cycle` 六向切换（12 次/轮，2 轮）：

| fixture | Writer 帧 p50 | p95 | p99 | 模式 handler p50 | p95 |
| --- | ---: | ---: | ---: | ---: | ---: |
| `history-single-paragraph-102420` | 0.89 ms | 6.43 ms | 15.52 ms | 0.34 ms | 0.90 ms |
| `history-multiblock-1112000` | 0.96 ms | 4.03 ms | 5.21 ms | 0.74 ms | 2.76 ms |
| `complex-markdown` | 0.53 ms | 5.66 ms | 10.26 ms | 0.20 ms | 0.47 ms |

Build 仍为 stock Editor；`history-single-paragraph-102420` 的 Build 帧 p95 为 121.81 ms（`prepaint` 整形），
与 [M6 决策](editor-performance-m6-decision.md) 一致，M6 未关闭。

## 5. 收尾

- 删除 M2 的 `--spike=text-layout` 对照入口、`text_layout/spike.rs` 及其独占的 `previous_grapheme_start`、
  `max_scroll`：Writing/Preview 已使用正式布局，保留第二份只读原型属于被替代的测试辅助代码。
- 更新 `text_layout` 模块文档：不再声称“不替换任何产品视图”。
- 保留 `ASTROLITH_SOURCE_SURFACE`（M6 功能门槛未过，Build 默认仍是 stock Editor）与 `WriterSurface`
  的 `EditorState` 输入桥（M5 决策）。
- 未保留重复全文渲染或隐藏输入面；解析任务仍是 latest-only、可取消，并新增增量路径。

## 6. 复现

```sh
node scripts/editor-perf.mjs run \
  --fixture history-multiblock-1112000 \
  --backend pre --scenario m7-input --cycles 3 --settle 2000 \
  --test-id editor-m7-parse-input-history-multiblock-1112000

node scripts/editor-perf.mjs run \
  --fixture history-multiblock-1112000 \
  --backend pre --scenario m7-close --settle 2000 \
  --test-id editor-m7-close-history-multiblock-1112000

node scripts/editor-perf.mjs run \
  --fixture complex-markdown \
  --backend pre --scenario cycle --cycles 2 --settle 2000 \
  --test-id editor-m7-cycle-complex-markdown
```

证据：`artifacts/e2e/editor-m7-pre-input-*`（整文档基线）、`editor-m7-parse-input-*`（增量）、
`editor-m7-close-*`、`editor-m7-cycle-*`。

## 7. 未执行与限制

- release profile 未测量；本阶段只覆盖 dev。
- 真实 fcitx5 预编辑、候选窗口、物理键盘与剪贴板仍属 editor-04，保持 `not_run`。
- 关闭只测同步 handler；布局缓存释放、RSS 趋势与长运行属 editor-07，保持 `not_run`。
- 增量边界按解析单元处理；若未来块解析引入跨块状态，需同步扩展重解析范围或回退条件。
- 单一巨大块（1 MiB 单行）的输入收敛仍超过 50 ms，记录为已知限制，不用降级显示规避。
