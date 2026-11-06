# 编辑器性能 M5 触发评估：保留 Writing 输入桥

- 日期：2026-11-06。
- 对应方案：[编辑器性能优化实施方案](editor-performance-implementation-plan.md) M5。
- 状态：**已评估，未触发迁移**。Writing 继续使用 pinned `EditorState` 作为输入模型；不移除其文本、选区、composition 与内部 undo 状态。
- 环境：Linux 7.3.1-1-cachyos x64，Hyprland/Wayland，scale 1，gpui-pre 后端，dev profile，无 profiler。
- 证据：`artifacts/e2e/editor-m5-gate-pre-writing-102420`、`artifacts/e2e/editor-m5-gate-pre-writing-1mib`。

## 1. 触发条件

方案把 M5 定为条件执行：渲染路径已稳定（M3/M4 已完成）之后，只有“重复 EditorState 状态、后台高亮/DisplayMap 工作或长期维护复杂度有可测成本”才值得重写输入内核；如果当前桥已达标，先记录保留决定，不为移除类型重写输入。

本轮按该顺序核对：先确认 Writing 的 `EditorState` 实际还在做什么，再测量这条桥的输入成本，最后对照方案第 3 节的目标判断是否触发。

## 2. 代码事实

- `WriterSurface` 是 `EntityInputHandler`，把 `text_for_range`、选区、marked range、粘贴、替换与 UTF-16 换算转发给 `DocumentSession::editor`；输入、composition 和本地 undo 仍由 pinned `EditorState` 承担。
- Writing 的 `EditorState` 以 `.language("text")`、`line_number(false)`、`indent_guides(false)`、`folding(false)`、`auto_close(false)`、`smart_indent(false)` 构造，应用没有调用 `set_highlighter` 或安装高亮工厂；该状态也从不作为 Kit `Editor`/`Input` 控件挂载或绘制。
- 因此 Writing 没有 tree-sitter 解析、没有语法高亮重算，也没有渲染期的软换行、字形整形或 `paragraph`/`BlockNode` 构造。编辑时仍会更新 `DisplayMap` 的行级簿记和内部 undo，但这些工作包含在下面的输入成本里。
- `DocumentSession::retained_bytes` 已把 editor 文本计入加载准入；`EditorState` 保留的是 writer projection 的一份额外 Rope。它按 `MAX_DOCUMENT_BYTES = 8 MiB` 有界，且不随帧或光标闪烁增长。
- 权威历史仍是 `SourceHistory`；`EditorState` 的 undo 只服务其内部桥，不作为用户可见的完整源历史。

## 3. 原生测量

同一 dev 构建、`ASTROLITH_EDITOR_PERF=1`、`scroll-input` 场景（进入 Writing → 滚动 → 回顶 → 输入 `中🦀`，10 轮）。`driver_step input` 覆盖 `WriterSurface::replace_text_in_range` 内 pinned 状态的实际更新（Rope、DisplayMap 簿记、内部 undo、事件投递）；`change_handler` 是随后应用侧把编辑映射进 `DocumentBuffer`/`SourceHistory`/投影的 `source_transaction`。

| fixture | 输入次数 | Kit 输入更新 p50 / max | 应用 source 事务 p95 / max | writer 帧 p95 / max |
| --- | ---: | --- | --- | --- |
| `history-single-paragraph-102420` | 10 | 0.31 / 0.40 ms | 1.84 / 1.84 ms | 3.87 / 10.10 ms |
| `plain-long-line-1mib`（单块） | 10 | 0.29 / 0.72 ms | 3.83 / 3.83 ms | 3.64 / 4.48 ms |

- 1 MiB 单块下，整条输入桥（含 DisplayMap 簿记与内部 undo）最大 0.72 ms，低于应用自身 source 事务的 3.83 ms p95；最慢工作在应用侧的投影/快照映射，不在 pinned 输入状态。
- 102 KB 与 1 MiB 的 source 事务 p95 分别为 1.84 ms 与 3.83 ms，均在方案第 3 节的 4 ms 目标内；writer 帧 p95 也低于 16.7 ms。
- 光标闪烁、滚动与稳定帧没有重新解析、合并且不增加 `EditorState` 工作；`writer_repaints` 为 0，帧计数只来自实际内容或 caret 变化。
- 未观测到后台高亮解析或 DisplayMap 整篇整形：该状态不绘制，wrap 宽度为未布局状态，编辑路径只做行级簿记。

## 4. 决策

**保留现有 Writing 输入桥，不实施 M5 的输入内核重写。**

依据：

1. 迁移触发条件针对的是 pinned 状态的重复工作与后台高亮/DisplayMap 成本。当前它们的可测成本是 1 MiB 单块最大 0.72 ms，不构成热点，也不接近目标边界。
2. 应用侧 source 事务的主要成本来自现有投影映射与快照比较，移除 `EditorState` 不会消除这部分；把两者一起重写超出了“为有可测成本的输入内核迁移”的范围。
3. 重复全文状态按 8 MiB 上限有界并已计入加载准入，不随帧增长；没有证据表明它造成帧时间或内存压力。
4. 保留 bridge 不改变现有行为：一次 composition 一次 undo、剪贴板、方向选择与模式切换仍走已验证的 pinned 实现。

后续重新打开的明确条件：

- 任一 fixture 的 Kit 输入更新（`driver_step input`）达到或超过应用 source 事务成本，或其 p95 接近 4 ms；
- 8 MiB 边界、长时间运行或 `retained_bytes` 表明重复文本造成实际内存压力；
- 需要统一键盘、粘贴、格式化、snippet、frontmatter、reload 的事务入口，且该需求本身可测，不再只是“移除一个类型”。

真实 fcitx5 预编辑、候选窗口、物理键盘与剪贴板的原生验收（editor-04）仍属于未执行的平台证据；保留输入桥不等于已完成该验收。

## 5. 复现

```sh
node scripts/editor-perf.mjs run \
  --fixture history-single-paragraph-102420 \
  --backend pre --scenario scroll-input --cycles 10 \
  --test-id editor-m5-gate-pre-writing-102420

node scripts/editor-perf.mjs run \
  --fixture plain-long-line-1mib \
  --backend pre --scenario scroll-input --cycles 10 \
  --test-id editor-m5-gate-pre-writing-1mib
```
