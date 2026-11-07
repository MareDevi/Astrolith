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
- `--scenario idle|cycle|scroll-input|preview|preview-idle|build-idle|build|build-wrap|m7-input|m7-close`：分别覆盖稳定窗口、六向模式切换、Writer 滚动与输入、Preview 滚动、Preview 静止、Build 静止/Build 动作/软换行对照，以及 M7 的输入收敛和文档关闭。
- `--fullscreen-pairs N`：按启动进程 PID（含 CPU time 包装器子进程）定位窗口，使用 address 定向切换；只有状态与尺寸均确认变化才计入 `manifest.metrics.fullscreen_pairs`。`fullscreen_transitions` 保留前后快照和 dispatcher 结果，最后恢复原全屏状态。场景时长必须覆盖请求的全部切换。
- `--enforce-slo`：同时要求本次采样的 CPU 指标达标；不加时允许记录超标基线。`metrics.execution` 检查退出码、超时、完整步骤数、quit 和全屏次数；`metrics.performance` 单列测量范围、阈值及通过/失败，不代表 compositor 或真实 IME 验收。 若启动或退出时检测到 Hyprland 锁屏，采样只作诊断，性能判定为 `not_run`。
- `--fixture <id>`、`--cycles`、`--settle`、`--wait-document`、`--timeout` 控制负载与等待；
  `--test-id` 指定 artifact 目录与 manifest 标识，避免不同里程碑共用 M0 命名。

每次运行都会创建独立的 `HOME`、`XDG_*`、偏好目录、恢复目录、runtime 目录和一个带 Astro discovery stub 的临时项目；fixture 通过启动参数作为条目打开，不经过文件浏览器。运行产物遵守 [E2E artifacts 合同](e2e-artifacts.md)：`manifest.json`、`command.txt`、`scenario.json`、`stdout.log`、`stderr.log`、`build.log`、`perf.json`。

A/B：

```sh
node scripts/editor-perf.mjs ab --fixture history-single-paragraph-102420 --scenario cycle
```

`ab` 依次运行 `pre` 与 `fast`。两个后端的构建产物、manifest 和日志分开保存；不要把图中的上游包当成运行时后端，manifest 的 `source` 记录运行时 `gpui_kit::GPUI_FAST`。

## 4. 指标口径

- `writer_frame`：WriterSurface 的 render、request-layout、prepaint、paint CPU 阶段；不含 compositor presentation。
  M3 起 Writing 的布局 prepare（wrap/shape 预算）在 render 内执行，`layout_ms`/`prepaint_ms` 只覆盖 canvas 的尺寸与 bounds 记录。
- `mode_handler`：`Shell::set_mode` 的 handler 时间。
- `source_transaction`：会话 Change handler，从输入适配到 source/history/count 更新。
- `parse_queue`：快照请求到后台解析任务真正开始之间的等待。
- `parse_run`：一次后台解析的 CPU 时间、输入字节与复用的块数；`reused_blocks > 0` 表示走了增量路径。
- `document_ready`：快照请求到安装完成的总时间，分 `initial_install`（冷打开）与 `edit_install`（输入收敛），另记 `layout_reconcile`。
- `document_close`：关闭活动文档的 handler 时间。
- `driver_step`：脚本动作的派发到返回，用于与应用外时钟对齐。

Preview 自 M4 起使用应用自有布局，`text_layout_frame` 以 `mode="preview"` 记录 wrap/shape、行数与 prepare 时间；Build 仍由 Kit Editor 内部完成布局，本基线通过 CPU 采样和外部时间观察，不伪造应用内分阶段计时。

## 5. 限制

- 脚本输入走真实 `WriterSurface` 的 `EntityInputHandler`，但不等于物理键盘或 fcitx5 候选窗口验收。
- 单次 GPUI shaping 调用不可被时间预算中断；本入口只测量，不改变算法。
- 首次运行需为每个后端完整编译；`build_ms` 单独记录，不与运行时指标混合。
