# 编辑器性能 M2：自有文本布局可行性结果

- 日期：2026-11-06。
- 对应方案：[编辑器性能优化实施方案](editor-performance-implementation-plan.md) M2。
- 状态：**M2 可行性门槛通过**；产品默认视图未替换，Writing/Preview/Build 仍走现有实现。
- 环境：Linux 7.3.1-1-cachyos x64，Hyprland/Wayland，scale 1，gpui-pre 后端，dev profile，无 profiler。
- 证据：`artifacts/e2e/editor-m2-layout-core/`、`artifacts/e2e/editor-m2-private-vs-kit/`。

## 1. 交付

| 交付 | 位置 |
| --- | --- |
| 视口文本布局核心 | `crates/astrolith-gpui/src/editor/text_layout/` |
| 复用的行结果：包装检查点、Fenwick 高度索引、命中/光标/选择/IME 几何 | 同上 `mod.rs` |
| 增量 reconcile：稳定块与安全编辑前缀复用 | `TextLayout::update` |
| 开发专用原生对照入口 | `--spike=text-layout`，`text_layout/spike.rs` |
| 计数器 | `astrolith::perf` 的 `text_layout_frame`、`text_layout_pending`、`text_layout_edit`、`text_layout_follow` |
| 自动化测试 | `text_layout/tests.rs`（7 项，headless） |
| 依赖 | `unicode-linebreak = 0.1.5`（图里已有，仅提升为直接依赖） |

核心直接使用 GPUI 的 `WindowTextSystem::shape_line`、`TextRun`、`ShapedLine`，不经过 Kit Inline 的
retained-layout 全局表，也不构造 Kit `Paragraph`/`InlineNode`。

## 2. 设计与实现边界

- 包装按渲染字节进行，断行机会来自 UAX #14；行边界必须是字素边界或强制换行。
- 包装状态可恢复：未完成的 visual row 保存 `start/scan/width/last_break`，长块跨帧继续，不重扫整段。
- 每个块保存已发现的 breaks、measured rows、已 wrap 前缀与估算尾部；块高度只更新自身，高度索引用 Fenwick 树。
- 只有与视口（含半屏 overscan）相交的行才 shape；视口外的 shaped line 在保留两屏后丢弃。
- 行 shaping 带同文本行内的前置上下文（最多 24 字节），再 `split_at` 掉上下文，避免在字素/连接上下文中间起形。
- 目标区域优先：跟随光标时，即使目标行在视口外也会 shape 该行，随后用同一几何滚动；未解析的范围返回 `None`。
- 局部编辑：文本、标记与块类型完全相同的块整块复用（含已 shape 的行）；被编辑块只保留公共前缀前 8 字节之外的行。

## 3. cosmic-text 开发优化 A/B

M0 的 dev 字体栈优化遗漏了 `cosmic-text`（实际调用 shaping 的包）。同一 fixture
`history-single-paragraph-102420`、同一动作序列、同一 target，固定步骤
（等待 1.2 s → Ctrl+End → 等待 1.2 s → 滚动 0.5 → 800 ms 退出）：

| profile | user CPU | sys CPU | max RSS | 单帧最多 shape 行数 |
| --- | ---: | ---: | ---: | ---: |
| 无 `cosmic-text` 覆盖 | 0.87–0.89 s | 0.29–0.30 s | 250,640–252,628 KB | 2 |
| `cosmic-text = { opt-level = 3 }` | 0.39–0.42 s | 0.23–0.52 s | 251,424–252,608 KB | 45 |

结论：开发构建中 shaping 吞吐是 M2 的主要瓶颈；`cosmic-text` 的 opt-level 3 让同一进程用户态 CPU 约减半，
单帧 shape 行数从 2 提升到 45。代价是一次性开发编译时间，release 不受影响。该覆盖单独提交。

## 4. 原生场景结果

fixture `history-single-paragraph-102420`（102,420 B，单段落，CJK/emoji/强调/行内代码），
`ASTROLITH_EDITOR_PERF=1`，日志为数值计数。

| 场景 | 观察 |
| --- | --- |
| 首屏 | 首个 prepare 11.2–15.6 ms（布局构造与首次字体预热，未纳入预算）；随后 4 帧完成 89 行 wrap 与 65 行 shape；第 6 帧 `pending=false`，首屏准确约在首帧后 40–55 ms |
| 中部滚动（0.5/0.6） | 4 帧内补齐 43–55 行，wrap ≤ 12.4 KB/帧，shape ≤ 53 行/帧；之后每帧 0 字节 wrap |
| Ctrl+End | 剩余 ~20 KB 在 3 帧（wrap 预算 2 ms/帧）内补完；底部视口再 3 帧 shape；随后 caret follow 把 scroll 移到 2852（max 2932），IME bounds `(1369.0, 4196.4, 1 × 23.2)` |
| 局部编辑（caret 末尾插入） | `reused_blocks=0 reused_rows=131 rebuilt_blocks=1`；下一帧只 wrap 1 行/265 B、shape 265 B |
| 缩窗 2458→640 px | 新宽度 epoch 重建布局，仅 visible 区 wrap 35 行；高度估算 4236→16172；2 帧内 `pending=false` |
| 模式对照 | 同步骤下 private 进程 user 0.39 s / RSS 252,756 KB；Kit TextView user 16.39 s / RSS 348,104 KB |
| 1 MiB 无标记单行 | 首屏/中部只 wrap 既定窗口；中部后 measured rows 1291，总高度估算 79,660；进程 user 0.51 s、RSS 256 MB |
| 8 MiB 边界（6,862 块） | 每帧 wrap ≤ 4.9 KB、shape ≤ 12 行；measured rows 仅可见块 148；进程 user 1.58 s、RSS 408 MB |

`text_layout_step` 中曾出现 `ime_x=null`：光标位于尚未 wrap 到的偏移时几何按 fail-closed 返回空，
下一帧补齐后恢复数值；这是预期的冷定位边界处理，并由 `text_layout_pending` 记录待处理字节数。

## 5. 自动化测试

`cargo test -p astrolith-gpui --locked text_layout`（headless）：

- 行边界只落在字素/断行机会/强制换行或不可断词的应急行；
- 1 MiB 单段首屏本轮 wrap/shape 字节数远小于文档，深跳只继续推进且单帧不遍历全文；
- caret、hit-test、selection 与 IME 折叠范围使用同一行几何；
- 未解析范围返回 `None`；
- Fenwick 高度索引局部更新与按位置查找；
- 未受影响的块在编辑后保持身份，长块追加只复用安全前缀。

## 6. 未完成与限制

- spike 是开发对照窗口，不是产品视图；没有注册原生 `EntityInputHandler`，未执行 fcitx5 候选、
  物理键盘、拖选与剪贴板验收（editor-04/05 仍为 `not_run`）。
- bidi/组合字符/ZWJ emoji 使用 `complex-script` fixture 只确认可生成几何，视觉顺序未自动断言。
- tab 仍按字素原样 shape，未实现 tab stop 展开；不满足最终验收，但不阻塞 M2 的“可见区工作量”门槛。
- 首个 prepare 仍包含布局构造与字体预热；若冷打开要求 100 ms 可见，需要把 `TextLayout::new`
  的 run 解析移出关键路径（或分帧），属于 M3 接入时的预算问题。
- `TextLayout::update` 为验证复用正确性会保留每个块渲染文本的副本；8 MiB 文档的 RSS 含此副本与
  wgpu/驱动占用，尚未单独归账。
- 编辑复用是“整块相同”或“安全前缀”；在块首附近编辑会重排该块余下部分，但不会波及后续未变块。

## 7. 复现

```sh
node scripts/editor-perf.mjs fixtures
RUST_LOG='warn,astrolith::perf=info,astrolith_gpui=info,astrolith_desktop=info' \
ASTROLITH_EDITOR_PERF=1 \
ASTROLITH_TEXT_LAYOUT_FIXTURE=target/editor-perf/fixtures/history-single-paragraph-102420.md \
ASTROLITH_TEXT_LAYOUT_STEPS='[{"wait_ms":900,"log":true},{"scroll":0.5,"wait_ms":500,"log":true},
  {"caret":"end","wait_ms":900,"log":true},{"insert":true},{"wait_ms":600,"log":true,"quit":true}]' \
ASTROLITH_TEXT_LAYOUT_EXIT_MS=12000 \
target/debug/astrolith-desktop --spike=text-layout
```

`ASTROLITH_TEXT_LAYOUT_MODE=kit` 打开同一 source 的 Kit `TextView` 对照；`Tab` 在窗口内切换。
