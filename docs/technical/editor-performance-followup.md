# 编辑器性能验收后的修复

- 日期：2026-11-07。
- 起点：[独立验收](editor-performance-acceptance.md)（`da32639`）。原始数据和当时的未通过结论保留。
- 依赖：继续使用锁定的 GPUI Kit 发布包；没有 vendor、registry 修改或额外的产品依赖。

## 修复内容

| 修复 | 行为与证据 |
| --- | --- |
| Markdown 样式变化后旧布局复用 | 前缀复用同时验证文字和样式。加粗、删除线变化不会继承旧字形；普通末尾编辑继续复用不变行。 |
| 表格切主题仍显示旧颜色 | 递归更新单元格的绘制缓存，保留已测量折行；回归测试先复现再验证。 |
| Writing、Preview 窗口宽度滞后 | 画布在 prepaint 测到新宽度后请求下一帧。短内容无后台布局工作时也会更新，避免一直用旧宽度。 |
| 1 MiB 单块安装 | 普通文字按连续 UTF-8 区间追加，保留转义、行内标记和源码映射。布局分块比较共同前缀，解析复用已算出的替换区间。 |
| 性能脚本误判 | 检查退出码、超时、完整步骤和 quit；采样指标与执行状态单列。`--enforce-slo` 可使超标运行失败。 |
| Hyprland 全屏脚本 | 通过启动进程定位窗口，定向派发并核对窗口地址、尺寸和全屏状态。只统计验证过的切换对。 |
| Writing 的 IME 预编辑进入保存 | 预编辑仍从 live 输入模型绘制，保存和恢复快照只读已提交的源码。确认以一个源码事务吸收；取消恢复原文与选区。失焦、离开 Writing 或撤销时取消未确认内容。 |

## 实测与边界

Linux / Hyprland / Wayland，gpui-pre，dev 构建，无 profiler。数据来自 `artifacts/e2e/` 下同名场景。

- `editor-fix-fullscreen-20261106`：Writing 两轮，窗口 `1263×1384` 到 `2560×1440` 再回平铺；帧 CPU p95 10.24 ms。
- `editor-fix-preview-fullscreen-20261106`：修复前的 Preview 实际出现旧列宽；最后一次回平铺后仍以 1136 px 绘制。截图显示表格右侧被裁掉。
- `editor-fix-preview-width-20261106`：修复后两轮都从 745 px 到 1136 px，再回 745 px；prepare p95 1.25 ms。只计布局准备，不含整帧呈现。
- `editor-fix-input-20261106`：1 MiB 单行，四次输入；解析 p95 8.61 ms，安装 p95 39.66 ms，低于验收基线的 59.54 / 93.45 ms。
- `editor-fix-input-final-20261106`：相同 fixture，20 次输入；安装 p95 21.57 ms，布局 reconcile p95 11.15 ms。事务 p95 4.42 ms 超过 4 ms 门槛，严格模式保留失败。

- `editor-fix-input-gated-20261107`：复用 diff 后 20 次输入，事务 p95 3.99 ms、安装 p95 15.24 ms。该轮处于锁屏状态，仅保留 CPU 诊断数据；历史脚本的 `passed` 不作为原生验收。
- `editor-fix-locked-gate-20261107`：加入锁屏检测后，场景执行正常，性能状态为 `not_run`；`--enforce-slo` 退出码 1。
- `editor-followup-mode-smoke-20261107`：最终代码在原生进程中完成 12 次实际模式切换、26 个驱动步骤并以 0 退出。锁屏使性能状态为 `not_run`；此轮仅作为启动和模式切换验证。

原生测试窗口曾确认 Writing 显示光标和 fcitx5/Rime 候选框，也观察到预编辑仍显示时 `Ctrl+S` 写入了临时文字。修复前的回归测试复现 `sync_buffer` 吸收预编辑，以及取消后选中文字未恢复。修复后的 GPUI 测试覆盖保存快照、提交、unmark、取消和一次源码撤销。修复后的原生键盘复测因 Hyprland 锁屏无法取得测试窗口焦点，尚未验收。

`mise run check`：1,050 Rust 测试通过，8 忽略；5 项性能脚本测试和 Astro fixture 通过。GPUI 无窗口测试不能代替真实候选框和系统输入测试。

默认 Build 仍用 stock Editor，长单行整形仍超预算。实验 SourceSurface 的折叠及功能对等尚未完成，因此保持现有默认路径。平台输入、分数缩放、release、长期内存和输入到呈现延迟仍按原计划独立验收。
