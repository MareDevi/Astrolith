# 编辑器性能基准入口

- 日期：2026-11-06。
- 对应方案：[编辑器性能优化实施方案](editor-performance-implementation-plan.md) M0。
- 状态：harness 已实现；gpui-pre / gpui-fast 的对照结果见 `artifacts/e2e/editor-m0-*` 下的 manifest。

## 1. 组成

| 位置 | 作用 |
| --- | --- |
| `scripts/editor-perf.mjs` | 生成固定 fixture、隔离启动、执行脚本动作、收集日志、写 manifest 与 `perf.json` |
| `crates/astrolith-gpui/src/editor/perf.rs` | 默认关闭的计数与分阶段计时，target 为 `astrolith::perf` |
| `crates/astrolith-gpui/src/shell/perf.rs` | 仅开发构建：读取 `ASTROLITH_PERF_SCENARIO`，驱动模式、滚动、输入和退出 |
| `gpui-fast` feature | `astrolith-gpui` / `astrolith-desktop` 的可选后端切换，不改变默认构建 |
| `--spike=text-layout` | 开发专用 M2 对照窗口：应用自有视口布局与 Kit `TextView` 同一 source 的 A/B，不替换产品视图 |

计数器只在 `ASTROLITH_EDITOR_PERF=1` 时输出，且只记录数字：块数、准备好的节点数、缓存复用数、交给整形的字节数、几何叶子数、以及 render/layout/prepaint/paint 各阶段毫秒。不记录正文、选择或预编辑内容。

## 2. Fixtures

```sh
node scripts/editor-perf.mjs fixtures
```

默认写入 `target/editor-perf/fixtures/`（不提交）。`manifest.json` 记录每个 fixture 的字节数、SHA-256、生成参数与形态：

- `history-multiblock-1112000`：1,112,000 字节多块 Markdown。
- `history-single-paragraph-102420`：102,420 字节 CJK/emoji/强调/行内代码单段落。
- `plain-long-line-1mib`：1 MiB 无 Markdown 单行。
- `complex-script`：RTL、组合附加符、ZWJ emoji、旗帜、tab、长 URL。
- `complex-markdown`：表格、脚注、引用定义、HTML/MDX、图片、fence。
- `boundary-8mib`：8 MiB 文档边界。

生成器按 UTF-8 码点边界截断后用 ASCII 空格补齐，因此字节数与哈希可复现。

## 3. 运行

```sh
node scripts/editor-perf.mjs run \
  --fixture history-single-paragraph-102420 \
  --backend pre \
  --scenario cycle \
  --cycles 2 \
  --fullscreen-pairs 2
```

- `--backend pre|fast`：分别构建到独立的 `CARGO_TARGET_DIR`（`target/editor-perf/pre|fast`），避免构建产物互相覆盖。
- `--scenario idle|cycle|scroll-input`：分别覆盖稳定窗口、六向模式切换、滚动与输入。
- `--fullscreen-pairs N`：在驱动步骤之间用 Hyprland 全屏/平铺切换，只有 dispatcher 成功才计入 `manifest.metrics.fullscreen_pairs`。
- `--fixture <id>`、`--cycles`、`--settle`、`--wait-document`、`--timeout` 控制负载与等待。

每次运行都会创建独立的 `HOME`、`XDG_*`、偏好目录、恢复目录、runtime 目录和一个带 Astro discovery stub 的临时项目；fixture 通过启动参数作为条目打开，不经过文件浏览器。运行产物遵守 [E2E artifacts 合同](e2e-artifacts.md)：`manifest.json`、`command.txt`、`scenario.json`、`stdout.log`、`stderr.log`、`build.log`、`perf.json`。

A/B：

```sh
node scripts/editor-perf.mjs ab --fixture history-single-paragraph-102420 --scenario cycle
```

`ab` 依次运行 `pre` 与 `fast`。两个后端的构建产物、manifest 和日志分开保存；不要把图中的上游包当成运行时后端，manifest 的 `source` 记录运行时 `gpui_kit::GPUI_FAST`。

## 4. 指标口径

- `writer_frame`：WriterSurface 的 render、request-layout、prepaint、paint CPU 阶段；不含 compositor presentation。
- `mode_handler`：`Shell::set_mode` 的 handler 时间。
- `source_transaction`：会话 Change handler，从输入适配到 source/history/count 更新。
- `parse_queue`：快照请求到后台解析任务真正开始之间的等待。
- `driver_step`：脚本动作的派发到返回，用于与应用外时钟对齐。

Preview 与 Build 仍由 Kit TextView/Editor 内部完成布局；本基线通过 CPU 采样和外部时间观察，不伪造应用内分阶段计时。

## 5. M2 文本布局对照入口

```sh
node scripts/editor-perf.mjs fixtures
ASTROLITH_EDITOR_PERF=1 \
ASTROLITH_TEXT_LAYOUT_FIXTURE=target/editor-perf/fixtures/history-single-paragraph-102420.md \
ASTROLITH_TEXT_LAYOUT_STEPS='[{"wait_ms":900,"log":true},{"scroll":0.5,"wait_ms":500,"log":true},{"quit":true}]' \
ASTROLITH_TEXT_LAYOUT_EXIT_MS=8000 \
target/debug/astrolith-desktop --spike=text-layout
```

- `ASTROLITH_TEXT_LAYOUT_MODE=private|kit`：启动时选择自有布局或 Kit `TextView`；窗口内 `Tab` 切换。
- `ASTROLITH_TEXT_LAYOUT_STEPS`：`wait_ms`、`scroll`（可滚动范围的比例）、`caret`（`start|end`）、
  `mode`、`size`（窗口逻辑像素）、`insert`（在光标处插入一个字）、`log`、`quit`。
- 输出 `text_layout_frame`（wrap/shape 行数与字节数、phase 时间、pending）、`text_layout_pending`
  （冷定位边界与待处理字节）、`text_layout_edit`（复用块/行）与 `text_layout_follow`（光标跟随）。
- 入口只编译 development 构建；release 遇到 `--spike=` 仍直接拒绝。结果与限制见
  [M2 结果](editor-performance-m2-results.md)。

## 6. 限制

- 脚本输入走真实 `WriterSurface` 的 `EntityInputHandler`，但不等于物理键盘或 fcitx5 候选窗口验收。
- 单次 GPUI shaping 调用不可被时间预算中断；本入口只测量，不改变算法。
- 首次运行需为每个后端完整编译；`build_ms` 单独记录，不与运行时指标混合。
