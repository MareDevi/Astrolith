# 编辑器性能独立验收

- 日期：2026-11-06。
- 被验收提交：`7fd94dc4c94c45c496492b73d5d6379f27bc7d45`。
- 结论：**阶段优化有效，整体暂不通过。** 新发现一个布局缓存正确性缺陷；默认 Build 和巨大单块输入仍未达标；原生交互证据尚未关闭。
- 范围：复核 M0–M7 文档、生产布局/解析代码、测试及 manifest 口径；独立运行完整检查、两个真实 Wayland 进程场景和一个临时回归测试。未修改产品代码。

## 1. 新发现

### P1：样式改变后复用旧字形与折行

位置：`crates/astrolith-gpui/src/editor/text_layout/mod.rs:589`（`carry_prefix`），调用于 `TextLayout::update`。

复现：先排版一个跨多行的普通段落，再给整个段落包上 `**`。渲染文字相同，字体权重已经改变，
但 `carry_prefix` 只比较文字共同前缀；调用方只检查块类型和深度。因此旧行的 `shaped` 和折行位置继续保留。
可见行的整形缓存不会因新的字体权重失效，Writing/Preview 可继续显示旧样式和旧宽度，直到其他原因重建布局。

独立临时测试 `acceptance_style_change_must_invalidate_carried_shapes` 已运行：

1. 使用现有测试字体和 `WindowTextSystem`，将普通句子重复 20 次，以 160 px 宽准备布局。
2. 确认首行已缓存字形。
3. 更新为 `**{source.trim_end()}**`，确认新 run 的字体权重与原先不同。
4. 断言首行不再持有旧整形缓存；**失败**。

命令：`mise exec -- cargo test -p astrolith-gpui --lib --locked acceptance_style_change_must_invalidate_carried_shapes`。
日志：`artifacts/e2e/editor-acceptance-cycle-20261106/regression.log`。临时测试运行后已恢复原文件，未留下源码改动。

关闭条件：前缀复用必须同时验证对应区间的排版和绘制属性；字体度量变化时重建受影响折行，颜色/装饰变化时更新绘制。
加入永久回归用例，同时保留普通末尾输入的前缀复用收益。

### P2：harness 的 passed 不能证明驱动完成或性能达标

位置：`scripts/editor-perf.mjs:767`。

状态只检查 `!timedOut && documentReady`，不检查子进程退出码、最后 quit 步骤、预期动作完成数或性能门槛。
因此文档打开后，进程提前失败仍可能写 `passed`。独立 cycle 运行的 Build p95 139.99 ms、输入运行的安装 p95
93.45 ms 也均标为 `passed`。记录基线可以允许超标，但必须把“场景完成”和“性能通过”分别判断。

关闭条件：记录并检查退出码及驱动完成；性能验收另列 SLO 判定。此次独立运行额外核对了实际步骤日志，
未把现有 `passed` 当成整体性能通过。

### P2：Hyprland 全屏驱动缺少目标窗口与状态核对

位置：`scripts/editor-perf.mjs:719–722`。

全屏命令没有指定本次子进程的窗口，仅凭两次 dispatcher 退出码累计成功次数。
焦点变化时可能操作其他窗口；成功退出也不能证明目标窗口尺寸或全屏状态发生变化。
因此现有 fullscreen pair 计数不足以验收用户提出的全屏/平铺问题。

关闭条件：按本次进程 PID 定位窗口，使用 address 明确指定目标，记录每次切换前后的实际状态和尺寸，
再关联应用帧和正确可见内容。本次没有运行这个未限定目标的选项。

## 2. 独立验证结果

`mise run check`：退出码 0。Rust 测试 1,043 通过、8 忽略；Astro fixture compatibility PASS。
临时回归测试的失败单独记录，不混入基线通过数。

环境：当前 Linux / Hyprland / Wayland，gpui-pre，dev profile，无 profiler；不代表 compositor presentation 延迟。
两个场景使用独立 HOME/偏好目录，通过项目 harness 的 `cargo build --locked` 构建真实原生进程，未使用 SourceSurface 开关。
不是逐字执行 `mise run dev` 的交互验收。

| 独立场景 | 结果 | 判断 |
| --- | --- | --- |
| 102,420 B 单段落，cycle 两轮 | Writing 26 帧，p95 4.85 ms，max 10.77 ms | 视口 CPU 帧预算通过 |
| 同场景 Preview | 5 次 prepare，max 4.33 ms | 视口准备成本低；样本少，不代表平台全面验收 |
| 同场景模式 handler | 12 次，p95 0.96 ms | handler 成本低，不等于首个正确显示帧 |
| 同场景默认 Build | 12 帧，p95/max 139.99 ms；prepaint max 138.86 ms | 仍超预算，复现 stock Editor 瓶颈 |
| 1 MiB 单行，m7-input 四次 | 事务 p95 3.32 ms；解析 max 59.54 ms；编辑安装 p95 93.45 ms | 输入事务轻；解析与布局 reconcile 仍使收敛超过 50 ms |
| 同输入场景 reconcile | p95/max 31.25 ms | 主线程接受文档时仍有明显工作 |

复现：

```sh
node scripts/editor-perf.mjs run --fixture history-single-paragraph-102420 --backend pre --scenario cycle --cycles 2 --settle 2000 --timeout 90 --test-id editor-acceptance-cycle-20261106
node scripts/editor-perf.mjs run --fixture plain-long-line-1mib --backend pre --scenario m7-input --cycles 4 --settle 2000 --timeout 90 --test-id editor-acceptance-input-20261106
```

证据：对应 `artifacts/e2e/editor-acceptance-*-20261106/` 下的 manifest、场景、日志与 perf.json。
现有生成器 limitations 中“Preview uses Kit TextView”描述已经过时；此次以实际代码和 `text_layout_frame mode="preview"`
核对，Preview 已使用应用布局。

## 3. 已明确记录的未完成项

这些不是本次新发现，不应混写为未披露缺陷：

- Build 默认仍为 stock Editor。SourceSurface 性能验证有效，但折叠、gutter/overview、括号匹配、缩进指引等功能门槛尚未关闭。
- completion/diagnostic/hover/code action/definition/search 宿主已有实现，不能继续笼统称为“全部未接入”。真实 LSP 往返尚未验收。
- M7 对多块文档的增量解析有效；单一巨大块仍整块解析，输入安装超过目标。安装时间还不是首个正确可见帧。
- 真实鼠标焦点、可见光标、物理键盘、fcitx5 组合输入、候选框和取消、剪贴板仍需平台验收。
- Hyprland 尺寸切换、分数缩放、复杂文字/bidi/tab、release 对照与长期内存稳定性未获得本次通过结论。
- Preview 对脚注、HTML/MDX、行内图片、内部链接和表格的语义限制已记录；性能通过不等于这些能力完整。

总方案首页对 M6 搜索/LSP 宿主的状态已落后于后续结果文档，应以详细结果为准并同步摘要。

## 4. 验收决定

可接受“Writing/Preview 的主要整篇排版热点已消除”和“保留轻量输入桥有测量依据”这两个阶段结论。
不能接受“原始卡顿、窗口尺寸切换和光标/输入问题已经全部修复”的整体结论。

下一轮首先修复样式缓存失效，再补可靠的验收驱动和真实焦点/输入/resize 证据；Build 保持明确未完成状态。
巨大单块超过 50 ms 的边界应单列，不用平均值或 manifest 的 passed 掩盖。
