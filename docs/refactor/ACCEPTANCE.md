# v0.2.0 验收标准与 M0 证据

所有功能项默认 **待实现/待验证**。源码存在、headless 通过、原型截图和真机交互是不同证据，不互相替代。本文不宣称 v0.2.0 已交付。真实 WebView、交互终端和 Astro 开发文件编辑为 mandatory；旧 UI 不在外观保留范围，视觉以 mock 为验收目标。

## 1. 证据规则

每条记录包含：commit、操作系统、窗口后端、显示缩放、UI 字号、主题、fixture、操作步骤、预期/实际、自动日志或原生截图/录屏路径。平台未测标“未测”，不能写“通过”。不永久保存带用户私有正文或完整主机路径的验收附件。

领域逻辑使用纯测试；entity/subscription 使用 GPUI 测试；焦点/布局/键盘使用真实事件交互；IME、Wayland 绘制和浏览器启动需真实平台。只有需要长期防回归的重要行为才提交测试，人工验收/性能 smoke 不全部转成固定测试。

Fixtures 首选 `fixtures/parity-project`（Markdown/MDX、CRLF、Unicode、schema、components、external changes）和 `fixtures/astro5`、`astro6`、`astro7`。破坏性文件操作在临时副本中运行；不修改冻结 fixture 的 expected 数据来让测试通过。

## 2. 核心行为验收

| ID | 步骤 | 通过标准 | 证明方式 / 负责阶段 |
| --- | --- | --- | --- |
| A01 | 使用空配置启动，打开真实内容条目 | Writing；导航、Inspector、底部工具默认隐藏；正文中心且有最大宽度 | 原生 bounds/截图，M2/M3 |
| A02 | 编辑未保存正文；Writing→Build→Preview→Writing；重复快捷键与按钮 | 文档身份和 retained session 不变，文本、selection、caret、history 不丢 | GPUI entity/操作测试 + 原生，M2 |
| A03 | 切换模式后 Undo/Redo，切换另一已打开文件再返回 | 正确恢复编辑顺序；不重读磁盘覆盖 buffer；各文件 history 独立 | focused regression，M2/M4a |
| A04 | dark/light/System；系统外观变化；旧 palette 配置升级 | 新 Neutral 主题统一、无蓝色通用强调；旧外观迁移到 mock Neutral，不提供旧 palette UI；控件与浮层同时更新 | theme tests + 两主题原生截图，M1/M6 |
| A05 | 正文编辑中打开/关闭 Frontmatter；测量 editor bounds | 浮层覆盖内容而不改变正文中心/宽度；初始关闭；按钮状态一致 | GPUI geometry + 原生，M4b |
| A06 | 修改实际 schema，刷新表单，切换集合/文档 | 字段由真实 schema 驱动，旧结果/旧事件不写入新文档；未知字段保留 | schema/GPUI tests，M4 |
| A07 | 用 text/description、number/integer、bool、date、enum、tags、nested、image、reference、raw/literal/refusal | required/default/constraints/description 与原语义一致；defaults 不因打开旧文件落盘 | `frontmatter`/schema suites + 原生字段路径，M4 |
| A08 | dirty body→隐藏 FM 标量修改→Undo→Undo→Redo→Redo | 完整源按事务顺序恢复；表单与正文一致；无新增或丢失 hidden bytes | 新完整源 history 回归，M4a；当前不满足此集成证明 |
| A09 | 修改 FM 后保存 CRLF/引号/注释/未知字段/MDX imports 文档 | 仅必要字段字节变化；不全量重排；拒绝不安全 patch 时保留原文并说明 | byte comparison + save tests，M4 |
| A10 | Inspector 内开 Select/DatePicker，按 Esc；再按 Esc；点击外部 | 先关子层再关父层；恢复有效 focus；不穿透触发危险操作 | 真实键盘/鼠标 + native，M4b |
| A11 | 最小窗口、长 schema、长 label、大字号；Inspector 滚到底 | close 可达、表单单独滚动、不越界、不滚正文；focus ring 不被裁切 | 最小窗/zoom 原生，M4/M6 |
| A12 | 启用 Focus/Typewriter；中文输入、组合取消、跨段选区、跳行、撤销 | 光标/选区/IME 正常；focus 依真实 caret；typewriter 不跳动/循环刷新 | 纯几何/装饰 tests + 真机，M3 |
| A13 | 调整正文 font/size/line-height/measure，保存并重启 | 四项生效且持久化；缺字体有 fallback；不改变文档和 history | settings tests + 原生，M3/M6 |
| A14 | Build 中导航、打开多个真实文件、编辑保存 `.astro` 与内容条目 | 标签对应 sessions；源码能力真实；原有 writer/schema/save 仍可用 | 新文件类型闭环 + regression，M2b |
| A15 | Preview external，启动 server 后 copy/open active entry | 使用真实 route/URL；编辑区仍可用，无空白网页 pane；launch 失败可恢复 | fake surface tests + 真实浏览器，M5a |
| A16 | 修改 preferred browser、Open on Enter；反复进入 Preview、失败重试 | argv 无 shell；按设置一次打开；旧请求取消；设置重启恢复 | lifecycle/argv tests + 原生，M5a/M6 |
| A17 | server ready 后跨模式切换；切项目；自然退出/端口冲突/超时 | 同项目不重启；新项目正确移交；失败保留 stderr 摘要和可编辑文档 | preview integration tests + real Astro，M5 |
| A18 | Wayland 同时设置 DISPLAY；选择 Embedded；无编译 surface 的 X11/macOS/Windows | 明确 unavailable 原因；external 操作可用；不切 XWayland | capability tests + 原生 Wayland，M5 |
| A19 | 在真正支持的平台选择 Embedded，更新文档并 resize/隐藏再显示 | 至少一个明确支持的真实平台运行 WebView；分屏、输入、热更新和生命周期正确；全平台 unavailable 或截图不算完成 | 平台 surface smoke/录屏，M5b；未交付前不得勾选 |
| A20 | 旧 global/project/collection 配置、future version、损坏数据、移动项目 | 功能配置/false/0/未知顶层字段保留；旧外观字段显式迁移而不恢复旧 UI；future version 不覆盖；layout clamp | core/store/migration tests，M2/M6 |
| A21 | 两个项目分别改 mode/layout 后重启，取消设置草稿与 Reset All | 项目状态隔离；恢复顺序正确；浮层仍关闭；Reset 不删除 recovery/logs | resolver/store/GPUI tests，M6 |
| A22 | 模式切换器与所有新动作仅键盘操作；大字号、长路径、reduced motion | 顺序合理、焦点可见、控件具名、无颜色独占含义；无强制动画 | semantic tree/键盘 + 对比度记录，M6/M7 |
| A23 | 审查 Cargo/入口/渲染树与构建产物 | Rust+GPUI 主 UI；无网页主壳；新增依赖有必要性和 lockfile | diff/source/build，M1…M7 |
| A24 | Build 打开终端，执行交互命令、ANSI 输出、Ctrl+C、resize、滚动回看、选择复制 | 真实 PTY/shell 输入输出；焦点快捷键隔离；无静态日志或网页终端替代 | PTY/service tests + 原生录屏，M2c |
| A25 | 终端运行时隐藏 Dock/切模式，再关闭会话、切项目和退出 | 隐藏不丢会话；按归属清理子进程；错误/退出码可见，可重新启动 | lifecycle tests + 原生进程验证，M2c |
| A26 | 新配置与旧配置升级后，同尺寸同主题逐区域对照 mock | toolbar、模式切换、导航、正文 sans 排版、圆角、表面、Inspector 与设置尽量一致；无旧 UI 兼容皮肤 | 对照截图与 bounds 记录，M1/M7 |
| A27 | 审查各阶段复用/重构/重写决策及性能改动 | 已评估对 v0.2.0 的适配性、效率和复杂度；有具体理由与行为验证，性能结论有代表性测量；被替代旧实现已清理 | 设计记录、diff、回归/性能结果，M1…M7 |

A14、A19、A24、A25 为功能完成门；A26 为视觉完成门。平台局部 fallback 不豁免整个 Embedded 功能交付。

## 3. 功能保留回归

以下不是新增需求。以 [ARCHITECTURE.md](ARCHITECTURE.md) 矩阵中的有效能力与正确性语义为基线，任何失效都阻止相应阶段完成。模型、状态结构、内部 API、源码路径和算法允许重写；验收不要求保留旧实现。行为测试随新边界调整，但不能削弱产品语义以掩盖回归。

- 项目 reopen/discovery、嵌套目录、过滤/排序、Content Finder open/link；watcher/config/schema 更新不丢 active session。
- create/duplicate/rename/move/trash/reveal/path copy；并发新编辑必须阻止过期破坏操作。
- find/replace/go-to-line、格式命令、原快捷键、selection toolbar、Copyedit、placeholder Tab/Esc、MDX import 保留语义。
- assets 安全复制、路径模式、图片字段、Alt-hover；不降低超时/大小限制、不接受项目外路径逃逸。
- autosave、blur/save、switch/quit guard、in-flight newer revision、conflict、recovery 与脱敏 save-failure artifact。
- Preferences 三层继承、Apply/Cancel/Reset、主题与字体 fallback；Help、command palette、IDE argv、fullscreen、single instance、protocol。
- dev-server start/stop/restart/retry、active route、browser launch 错误；readonly Open file 的既有边界不被误改。

现有关键回归证据位置：

| 测试位置 | 重要现有案例 |
| --- | --- |
| `astrolith-gpui/src/project_discovery.rs` | `discovered_project_opens_edits_and_retains_sessions`、`frontmatter_form_field_edit_preserves_concurrent_body_edit`、`projected_writer_keeps_hidden_bytes_across_typing_and_undo`、`projected_body_and_form_edits_survive_a_stale_save_completion` |
| 同上 | `writer_formatting_undoes_in_one_step_and_keeps_hidden_bytes`、`focus_mode_dims_other_sentences_and_the_prose_layer_skips_code`、`undo_removes_the_inserted_tag_and_leaves_the_import`、save/conflict/recovery/rename/trash cases |
| `astrolith-gpui/src/shell.rs` | `switching_projects_switches_the_saved_layouts`、`opening_a_project_attaches_the_preview_without_an_edit`、palette/keyboard/preferences cases |
| `astrolith-gpui/src/frontmatter/{tests,field_tests}.rs` | typed controls、lossless patch、raw/refusal、image/reference/metadata |
| `astrolith-preview/tests/preview_gate.rs` 与 `src/*tests.rs` | server lifecycle、capability、route、process ownership |
| `astrolith-workspace/tests/` | index/operations/watcher/fixture oracle |
| `astrolith-document/src/`、core preferences tests | parsing/projection/serialization/save/recovery、继承与保存状态 |

现有正文 undo 测试不能证明隐藏 FM 的历史可撤销。`DocumentBuffer::apply_text` 能恢复旧文本也不是“用户 Ctrl+Z 已集成”的证明。

## 4. 视觉与平台矩阵

原生验收至少覆盖 dark/light × 1000×700 / 1440×900 / 宽窗口 × 默认/放大 UI。显示缩放取平台可用的 100%、150%、200%，记录实际组合，不能用截图尺寸推测显示缩放。

逐项比对原型：中性表面层级、正文中心/measure、模式选中、toolbar 密度、Inspector 半径/inset/滚动、菜单控件一致性。预期共享边缘应在渲染像素上对齐；不通过散落 px correction 修复。记录允许差异：平台 title bar、必要的字体/CJK fallback、原生焦点行为、无 mock 外框/静态文案/假终端；旧 palette、旧 serif 默认与旧三栏 chrome 不属于允许差异。

| 平台 | 必测项 | 当前审计结论 |
| --- | --- | --- |
| Linux native Wayland | IME、焦点、scale/resize、popover/scroll/hit test、外部 browser、真实 Astro | 产品主路径；本轮未做原生验收 |
| Linux X11 | 原生布局/输入；Embedded 仅有实际 adapter 才测 | capability 理论支持不构成实际 WebView 支持 |
| macOS / Windows | 编译与真实窗口/IME/菜单/WebView 生命周期 | 当前未交付支持；无机器证据不得宣布完成 |
| headless | pure/GPUI/fixture tests | 不能证明字体绘制、显示服务器或真实 browser 成功 |

## 5. M0 执行记录

- 需求范围：只读审计后新增文档；未修改 Rust、配置/依赖、版本、原型。
- 基线检查：执行 `mise run check`，doctor、format、lint、build 成功；GPUI lib tests 为 338 passed、1 failed、4 ignored。
- 失败测试：`help::tests::user_and_derived_documentation_links_resolve`，`help.rs:437` 报 README 链接缺少 `docs/technical/development.md`。在任何本轮文档写入之前已复现。
- 原因：基线提交删除技术文档，但 README/用户文档与 Help 链接检查仍引用它们。M0 提供五个内容明确的兼容技术入口，链接到本次审计和实际源码，不删测试、不改变用户指南语义、不恢复旧计划的完成结论。
- 最终检查：补充文档后 `mise run check` 退出码 0；doctor、fmt、clippy、build 通过；Rust 测试合计 821 passed、0 failed、8 ignored；fixture 脚本单元测试 6 passed，真实 Astro fixture compatibility 为 PASS。报告生成于临时目录，不作为仓库文件提交。
- workspace 测试中 ignored 项含 opt-in 性能/真实项目/preview gate；后续 fixture runner 另行执行了真实安装项目的 headless 编辑工作流，性能与真实 browser preview gate 不因此视为已通过。原型为源码审阅，未启动 web mock；未进行 native smoke、截图或平台验收。

2026-11-04 范围校正：已将真实 WebView、交互终端、Astro 开发文件编辑和 mock 外观还原写入必交付标准，删除旧 UI 外观保留要求。文档校正后再次执行 `mise run check` 通过；未开始功能实现。

M0 文档完成不等于 A01…A27 已通过。后续每阶段按实际结果更新证据，不预先勾选。

## 6. M1 执行记录

- 范围：`theme/{tokens,palettes,mod,typography,metrics}.rs`、`ui.rs`、导航面板表面、设置页外观入口、偏好迁移及受字体度量影响的链接点击回归。
- 结果：
  - `cargo test -p astrolith-gpui --locked`：343 passed、0 failed、4 ignored。
  - `cargo test -p astrolith-core --locked` 与设置迁移定向测试通过；新增备份迁移测试验证 `global.json.pre-neutral.json`、旧键移除与未知字段保留。
  - `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --locked -- -D warnings`、`cargo build --package astrolith-desktop --locked` 通过。
  - 真实 Linux Wayland 原生窗口：以隔离的 `ASTROLITH_SETTINGS_PATH` 分别启动 light/dark，窗口截图确认中性画布、sidebar 表面、title bar、按钮与空状态；未使用 mock 外框或网页壳。
- 视觉对照：light 画布纯白、sidebar 近白、title bar 近白；dark 画布 `#0a0a0a`、sidebar `#171717`、title bar `#131313`。通用强调色不再为蓝。focus ring 为中性灰；light 值由 mock 的 0.708 调整为 0.65 以满足 3:1。
- 回归：中性无衬线字体度量使长 URL 软换行，暴露 pinned `range_to_bounds` 跨行矩形不可用；链接/图片命中改为逐字符回退，现有 URL 交替点击测试继续通过。
- 未完成/未验证：
  - A04 的 System 跟随与两主题截图已在本机验证，但 A26 的全区域 mock 对照未完成；模式 tabs、Build 标签条属于 M2。
  - Kit light 输入填充无法复现 mock `input/50`；按钮/输入 pill 轮廓未逐控件还原。
  - 未在 macOS/Windows 或 150%/200% 显示缩放下验收。



## 7. M2a 执行记录

- 范围：core `preferences.rs`、GPUI `settings/{store,migrate}.rs`、`shell.rs`、`commands.rs`、`project_discovery.rs` 的视图状态、模式切换与项目布局持久化。
- 新增 `WorkspaceMode`（Writing/Build/Preview）、`ModeLayouts` 与 `ModeLayoutOverrides`。Writing 默认隐藏导航、Inspector 与底部工具；Build 默认导航开启、工具关闭；Preview 默认保留可编辑正文并显示紧凑预览状态条。每个模式独立保存几何与可见性。
- 模式切换复用同一 `ProjectWorkspace`、`DocumentSession` 与 EditorState；测试验证 Build→Preview→Writing 后 editor/session 身份、文本、选区与 undo 历史不变。Inspector 视为临时状态：进入其他模式关闭并不写回布局。
- 三模式命令 `Writing workspace`、`Build workspace`、`Preview workspace`、`Cycle workspace mode` 加入统一 registry，快捷键为 Ctrl+Shift+1/2/3 与 Ctrl+Shift+M，未覆盖 Ctrl+B/I、Ctrl+Enter 或编辑器 context；菜单、palette 与快捷键文档由同一 registry 派生。
- 旧 `layout` 字段仅用于识别升级：全局文件备份为 `global.json.pre-mode.json` 并写入新每模式默认；项目文件备份为 `<id>.pre-mode.json` 并删除旧构图。未知顶层字段继续保留。
- 项目 `last_mode` 优先于 global `default_mode` 恢复；布局解析按模式叠加项目覆盖。模式命令与可见性状态并入共享 `CommandAvailability`。
- 验证：`mise run check` 退出码 0；Rust workspace 测试通过（GPUI 346 passed、4 ignored），fixture 单元 6 passed，Astro fixture compatibility PASS。
- 未完成：A01/A26 的原生窗口、真实模式下 screenshot/bounds 对照与最小窗口视觉检查尚未执行；Inspector 的浮动 Popover 属于 M4b。M2b/M2c（Build 源码、终端）未在本记录中声明完成。

## 8. M2c 执行记录

- 新增 `astrolith-terminal` 服务 crate：`portable-pty 0.9.0` 持有 PTY 子进程，`vt100 0.18.1` 解析 ANSI、颜色、光标、宽字符与有界 scrollback（2000 行）。reader 线程分批解析并递增 revision；GPUI 以 16 ms 轮询 revision，不在渲染路径读文件或等待进程。
- 新增 GPUI `TerminalPanel`：Build 模式底部 Dock 显示；项目路径绑定 shell，输入/箭头/功能键/Ctrl 组合、选择复制、粘贴、滚轮回看、PTY resize、Clear/Restart、退出码/失败提示。终端焦点用 keystroke interceptor 消费按键，避免触发编辑器/应用快捷键。
- 生命周期：隐藏 Dock 与切换模式不销毁 session；切换项目先终止旧子进程再启动新 shell；窗口/实体销毁时 `TerminalSession::drop` 终止并回收 reader。Terminal 与 Astro dev server 完全分离。
- 新增 `Toggle terminal` 命令（Ctrl+`）与 View 菜单项；快捷键文档由命令 registry 校验。
- 验证：`astrolith-terminal` 4 个服务测试通过（ANSI 红色与退出码 7、交互输入/resize、Ctrl-D 退出码 3、kill 长进程、revision 增长）；GPUI 集成测试验证真实 shell 输出进入屏幕、Dock/模式隐藏后 session 与 pid 不变、切项目后旧 pid 终止且新 root 启动。`cargo test --workspace --locked` 全部通过（GPUI 347 passed、4 ignored）。
- 未完成：A24/A25 的真实原生窗口录屏、IME/焦点隔离、ANSI 视觉与 150%/200% 缩放验收尚未执行；headless 与 service 测试不能替代真机证据。

## 9. M4a 执行记录

- `DocumentSession` 新增单一完整源事务栈，记录 `before/after` 的连续替换区间、可见 selection 与编辑意图；正文、格式化、链接包裹、组件标签和隐藏 frontmatter patch 都进入同一顺序。导入 patch 仍保持“撤销标签保留 import”的既有语义，不由事务回滚。
- 编辑器内部 undo 栈不再作为用户历史：`Ctrl+Z`/`Ctrl+Shift+Z`/`Ctrl+Y` 在编辑器 focus 下由会话 interceptor 截获；projection 重建改用不写历史的 `set_value`。每次 undo/redo 重建投影后恢复该事务的可见 selection，并继续把完整源作为唯一权威。
- 连续键入按 900 ms 与相邻插入合并，避免逐字符 undo 破坏现有粒度；事务数上限 500。非相邻或结构编辑保持原子。
- 新增 `HistoryChanged` 事件：`ProjectWorkspace` 转发后，Shell 强制 frontmatter form 从恢复后的完整源重新加载，避免隐藏字段 undo 后表单与正文分叉。
- 回归：新增正文编辑→隐藏 FM 修改→Undo→Undo→Redo→Redo 的交错测试，以及 CRLF + 中文/emoji 的字节保真 undo/redo 测试；现有 projection、formatting、MDX import、save/conflict/recovery 测试全部继续通过（GPUI 349 passed、4 ignored）。
- 未完成/未覆盖：原生窗口中的真实键盘、IME composition、跨模式 undo 与表单焦点恢复尚未执行；恢复文件在外部重载路径上的历史失效主要依赖现有 session 重开，未新增磁盘恢复集成用例。
