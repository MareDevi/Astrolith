# 编辑器性能 M0/M1 实施结果

- 日期：2026-11-06。
- 对应方案：[编辑器性能优化实施方案](editor-performance-implementation-plan.md)。
- 范围：M0（可重复基线与测量入口）与 M1（生命周期与刷新隔离）。M2 起未开始。
- 环境：Linux 7.3.1-1-cachyos x64，Hyprland 0.57.1/Wayland，scale 1，dev profile，无 profiler。
- 口径：`writer_frame` 只覆盖 WriterSurface 的 request-layout → prepaint → paint CPU 阶段，不含 compositor presentation；所有数值来自同一 fixture、同一构建配置、同一动作序列。

## 1. 交付

| 交付 | 位置 |
| --- | --- |
| 确定性 fixture 生成器与 manifest | `scripts/editor-perf.mjs fixtures`、`target/editor-perf/fixtures/manifest.json` |
| 隔离启动、Hyprland 动作、日志与 manifest | `scripts/editor-perf.mjs run`，产物在 `artifacts/e2e/editor-m0-*` |
| 默认关闭的计数与分阶段计时 | `crates/astrolith-gpui/src/editor/perf.rs`，target `astrolith::perf` |
| 开发构建脚本动作驱动 | `crates/astrolith-gpui/src/shell/perf.rs` |
| 可选 GPUI Fast 后端 | `gpui-fast` feature（两个 crate），默认不启用 |
| 生命周期修正 | 准备节点按已接受修订保留、caret 独立层与定时器、宽度 epoch 失效、隐藏模式停止 blink |

## 2. gpui-pre / gpui-fast A/B

fixture `history-single-paragraph-102420`（102,420 B CJK/emoji 单段落），`cycle` 六向切换 ×2，Hyprland 全屏/平铺 2 对：

| 后端 | 帧数 | 总 CPU p50 | p95 | p99 | max | prepaint p95 | paint p95 | mode handler p95 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| gpui-pre | 40 | 21.6 ms | 52.1 ms | 62.8 ms | 62.8 ms | 23.2 ms | 22.0 ms | 0.86 ms |
| gpui-fast | 31 | 82.9 ms | 277.2 ms | 370.5 ms | 370.5 ms | 176.3 ms | 93.8 ms | 0.81 ms |

fixture `history-multiblock-1112000`（1,112,000 B 多块文章），相同动作：

| 后端 | 帧数 | 总 CPU p50 | p95 | p99 | max | prepaint p95 | paint p95 | mode handler p95 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| gpui-pre | 42 | 1.7 ms | 20.1 ms | 34.2 ms | 34.2 ms | 0.93 ms | 1.71 ms | 2.71 ms |
| gpui-fast | 41 | 3.5 ms | 8.1 ms | 24.7 ms | 24.7 ms | 3.02 ms | 4.38 ms | 4.12 ms |

结论：

- **GPUI Fast 不采用为默认后端。** 它在长单段落上显著更慢（p95 277 ms 对 52 ms）；多块文档 p95 更低但 p50、prepaint/paint p95 和模式 handler 更差，收益不稳定。
- 后端由运行时日志 `GPUI_FAST=true/false` 确认，manifest 的 `source` 记录了该行；两个后端使用独立 `CARGO_TARGET_DIR`。
- 本 A/B 保持默认 dev profile。gpui-fast 没有单独获得字体栈 opt-level 3；这是刻意的，因为默认后端尚未证明需要该 profile 变更。

## 3. M1 效果证据

- 每个 fixture 每次运行只构建 1–3 个准备节点，其余帧全部复用（`nodes_built_total` 1–3，`nodes_reused_total` 28–84）。同一已接受修订内不再每帧重建 Paragraph/样式 run，也不再生成新的 inline state。
- `writer_repaint` 样本为 0：当前 GPUI 版本在 caret 定时器通知后仍会重跑视图渲染，因此“只重绘 overlay”在本后端不是可观察的结果。M1 的可验证收益是样式准备与节点重建被消除；减少每帧文本测量需要 M2 的自有布局。
- 单段落 `writer_frame` p95 52 ms，已显著低于历史诊断中 610 ms 的布局样本，但两者 fixture 生成、采样方式与进程状态不同，不能当作受控前后对比。
- caret blink、隐藏模式（Build/Preview）与失焦都会停止 overlay 定时器；恢复 Writing 后由下一次渲染重建可见 caret。

## 4. 字体栈 dev opt-level 3 A/B

在 Build 模式打开 `history-single-paragraph-102420`，停留 6 s，`/usr/bin/time` 记录整个进程的总 CPU；同一 target、fixture、字体、窗口与动作，n=3：

| profile | user CPU | sys CPU | elapsed | max RSS |
| --- | ---: | ---: | ---: | ---: |
| 当前 dev profile | 3.03–3.08 s | 0.53–0.58 s | 10.5–10.6 s | 424–432 MB |
| 字体栈 `opt-level = 3` | 1.96–2.02 s | 0.55–0.83 s | 10.5–11.4 s | 426–430 MB |

字体栈优化把 Build 长单行的进程用户态 CPU 降低约 35%，系统态、墙钟等待与内存基本不变。收益成立，因此采用 `font-types`、`fontdb`、`harfrust`、`read-fonts`、`skrifa`、`swash`、`ttf-parser` 的 dev opt-level 3；该组只影响开发构建，release 不变。此测量不是帧时间，也不代表冷启动首屏延迟已达标。

## 5. 决策门

单段落 p95 52 ms、p99 62.8 ms，未达到 16.7 / 33 ms 目标；多块文档 p95 20.1 ms 也未达标。Fast + M1 不足以关闭性能任务，按方案进入 M2（自有文本布局可行性）——本阶段未执行。

## 6. 未执行项

- Preview/Build 的应用内分阶段计时；本次只对 Writing 计数，Build 使用进程 CPU 对照，Preview 仅有外部观察。
- 释放板、冷进程首屏、8 MiB 边界、真实 fcitx5 候选窗口与物理键盘。
- 所有原生场景（editor-01..09）中的 IME、选择、剪贴板、跨平台项均保持 `not_run`。

## 7. 复现

```sh
node scripts/editor-perf.mjs fixtures
node scripts/editor-perf.mjs ab --fixture history-single-paragraph-102420 \
  --scenario cycle --cycles 2 --settle 2500 --fullscreen-pairs 2 --timeout 240
```

每个后端的结果保存在独立 `artifacts/e2e/editor-m0-<backend>-<fixture>-<timestamp>/`，包含 `manifest.json`、`perf.json`、`command.txt`、`scenario.json`、`stdout.log`、`stderr.log` 和 `build.log`。
