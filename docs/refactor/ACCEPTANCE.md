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
- 原生验收：用户于 2026-11-04 确认 A24/A25 通过（见 §11）；headless 与 service 测试仍作为自动回归证据保留。

## 9. M4a 执行记录

- `DocumentSession` 新增单一完整源事务栈，记录 `before/after` 的连续替换区间、可见 selection 与编辑意图；正文、格式化、链接包裹、组件标签和隐藏 frontmatter patch 都进入同一顺序。导入 patch 仍保持“撤销标签保留 import”的既有语义，不由事务回滚。
- 编辑器内部 undo 栈不再作为用户历史：`Ctrl+Z`/`Ctrl+Shift+Z`/`Ctrl+Y` 在编辑器 focus 下由会话 interceptor 截获；projection 重建改用不写历史的 `set_value`。每次 undo/redo 重建投影后恢复该事务的可见 selection，并继续把完整源作为唯一权威。
- 连续键入按 900 ms 与相邻插入合并，避免逐字符 undo 破坏现有粒度；事务数上限 500。非相邻或结构编辑保持原子。
- 新增 `HistoryChanged` 事件：`ProjectWorkspace` 转发后，Shell 强制 frontmatter form 从恢复后的完整源重新加载，避免隐藏字段 undo 后表单与正文分叉。
- 回归：新增正文编辑→隐藏 FM 修改→Undo→Undo→Redo→Redo 的交错测试，以及 CRLF + 中文/emoji 的字节保真 undo/redo 测试；现有 projection、formatting、MDX import、save/conflict/recovery 测试全部继续通过（GPUI 349 passed、4 ignored）。
- 未完成/未覆盖：原生窗口中的真实键盘、IME composition、跨模式 undo 与表单焦点恢复尚未执行；恢复文件在外部重载路径上的历史失效主要依赖现有 session 重开，未新增磁盘恢复集成用例。

## 10. M2b 执行记录

- Build 模式把当前 session 的 EditorState 切到完整源表示：同一个 `DocumentBuffer`、同一 editor、同一完整源事务栈。切回 Writing/Preview 时按 projection 隐藏 frontmatter/imports，保序映射 caret 与 selection；格式化命令在完整源视图下禁用，避免把源码坐标当作可见坐标。
- Build 顶部新增真实标签条，数据来自 `ProjectWorkspace::open_tabs`；点击激活 retained session，关闭按钮走现有脏文档确认与保存路径。
- 新增项目 `.astro` 源码能力：`ProjectWorkspace::open_project_source` 校验项目内路径、拒绝 `node_modules`/`.git`、复用 8 MiB/50,000 行上限、DocumentSession、保存、冲突、恢复与完整源历史；`.astro` 作为 Plain 文档，不显示 frontmatter/schema 控件。
- 新增 `Open project source…` 命令与有界 `.astro` 扫描（深度、目录、结果数上限，跳过依赖/构建目录，不跟随符号链接），供 Build 文件导航使用。Open file 对项目内 `.astro` 进入可编辑 session，项目外文件仍保持原有 readonly 入口。
- MDX 导入 patch 的既有语义保持不变：撤销组件标签仍保留 import。ProjectWorkspace 在 source view 切换和激活时应用当前模式；进入 Build 不改文件、不重读磁盘、不重启服务。
- 回归：新增 `.astro` 扫描排序/跳过依赖、`.astro` 打开/编辑/保存/无 frontmatter、Build 完整源与标签条、source-view 编辑与 Undo/映射回 Writer 测试；`cargo test --workspace --locked` 通过（GPUI 353 passed、4 ignored），clippy `-D warnings` 通过。
- 未启用 `tree-sitter-astro` 高亮（没有真实高亮证据，不让空 feature 冒充），未新增 Problems 面板或 LSP；`.astro` 导航为命令/对话框入口，侧栏项目树未扩展。
- 用户于 2026-11-04 确认 A14 通过（见 §11）；原生源码浏览、焦点、标签条与 `.astro` 保存不再列为待验收。

## 11. 用户原生验收确认（2026-11-04）

用户确认以下项目在当前原生窗口中验收通过：

| ID | 范围 | 状态 |
| --- | --- | --- |
| A01 | Writing 默认布局、工具隐藏、正文居中 | 用户验收通过 |
| A14 | Build 导航、真实标签、`.astro` 与内容条目的编辑保存 | 用户验收通过 |
| A24 | 交互终端、ANSI、Ctrl+C、resize、滚动回看、选择复制 | 用户验收通过 |
| A25 | 隐藏 Dock/切模式保留会话，切项目/退出清理进程 | 用户验收通过 |
| A26 | 新布局与 mock 的分区域视觉对照 | 用户验收通过 |
| A12 | Focus/Typewriter、中文输入、组合取消、跨段选区、跳行与滚动 | 用户验收通过（M3） |
| A05/A10 | Inspector 浮层不改变正文 bounds；按钮/快捷键打开；outside/Esc/关闭按钮关闭；嵌套 Select/DatePicker 逐层关闭 | 用户验收通过（M4b） |

本次回复未附带截图路径、显示缩放和主题组合明细；后续如归档需要，应补齐这些附件元数据。A19/M5b（真实 Embedded WebView）不在本次确认范围。

## 12. M3 执行记录

- 范围：core `preferences.rs`（写作字段与解析）、GPUI `writing_modes.rs`、`layout/panels.rs`、`theme/{typography,mod}.rs`、`settings/store.rs`（迁移）、`preferences/{mod,general,tests}.rs`、`project_discovery.rs` 的写作模式入口、`shell.rs`（应用/持久化）。
- 正文排版：删除按 pane 宽度缩放的字号/行高算法，新增 `ProseStyle`。字号、行高倍数、measure 与字体都来自持久设置；超出范围的手改值在渲染边界 clamp。`set_prose_style` 只改变 Editor 的绘制属性。
- 默认对齐 mock：sans 正文（Inter，缺失回退平台 UI 字体）、16 px、2.0 行高（32 px）、768 px measure、96 px 顶部留白、32 px 内边距。旧默认 18/720 与 serif 路径不再作为默认。
- 持久化字段：`prose_font_family`、`editor_font_size`、`editor_line_height`、`editor_width`、`focus_mode`、`typewriter_mode`。Focus/Typewriter 命令、启动恢复、Preferences Apply 共用同一 `WritingModes`；sentence focus 保留，未新增 paragraph focus。
- 升级迁移：缺少 `editor_line_height` 的 `global.json` 先备份为 `global.json.pre-writing.json`，把未改动的旧默认 18/720 更新为 16/768，并写入全部写作字段；用户显式设置的其他字号/宽度保留。未知顶层字段继续保留。
- 设置 UI：General 的 Writing 区新增 Document font、Document font size、Line height、Page width、Focus mode、Typewriter mode；字体必须已安装，行高限制 1.2–3.0。
- 回归测试：
  - `writing_typography_and_modes_resolve_from_the_global_scope`：解析与 serde 往返。
  - `pre_writing_defaults_upgrade_once_and_preserve_explicit_values`：一次性迁移、备份、显式字号保留。
  - `line_height_stays_inside_the_readable_band`：范围校验。
  - `prose_style_keeps_configured_values_and_clamps_corrupt_ones`：渲染边界 clamp。
  - `typography_changes_leave_the_document_state_untouched`：切换 22 px / 2.6 / 900 后文本、选区与 caret 不变。
  - `writing_mode_toggles_persist_and_restore`：Focus/Typewriter 命令写入 `global.json`，重新加载为 true，Apply 后 live 状态同步。
  - 既有 sentence focus、typewriter 几何、投影与 undo 测试全部继续通过。
- 自动检查：`mise run check` 退出码 0；doctor、fmt、clippy `-D warnings`、build 通过；Rust workspace 844 passed、0 failed、8 ignored；fixture 单元 6 passed，Astro fixture compatibility PASS。
- 原生证据：Linux Wayland（Hyprland），dark/System 主题，窗口 2538×1384；fixture `fixtures/parity-project/src/content/blog/markdown-showcase.md`。默认设置截图确认正文 sans、居中 768 measure、保存状态与计数可见；随后把 `editor_font_size=22`、`editor_line_height=2.4`、`editor_width=900`、`focus_mode=true` 写入 `global.json` 并重启，截图中字号、行距、页宽与 sentence focus 均按持久值生效，文档计数仍为 120 words / 827 characters。截图临时保存在 `/tmp/astrolith-m3-writing.png` 与 `/tmp/astrolith-m3-typography.png`，未提交用户内容。
- 原生验收：用户于 2026-11-04 确认 A12 通过（中文输入、组合取消、跨段选区、跳行、选区、滚动与 Typewriter 行为正常）。本次回复未附截图与显示缩放组合明细。
- 未完成/未验证：
  - 150%/200% 显示缩放、最小窗口下的设置页新字段布局未截图验收。
  - paragraph focus 不在本次范围。

## 13. M4b 执行记录

- 范围：`shell.rs` 的 transient inspector 状态与命令、`editor/header.rs` 的触发按钮和浮层宿主、`frontmatter/mod.rs` 的 body-only 渲染、core `PanelLayout` 的右栏字段清理、`commands.rs` 的 frontmatter availability。
- 永久 right Dock 已移除；`inspector_visible`/`inspector_width` 不再属于 `PanelLayout`。旧配置中的这些键由 serde 忽略，未知顶层字段继续保留。Inspector 不写入任何模式布局。
- `FrontmatterForm` 保留 entity、字段 row entities、reference catalog、图片/引用控件和 `FrontmatterEdit` subscription；移除了表单自带的 “Details” header，浮层提供 “Frontmatter / Entry details · collection” 与关闭按钮。表单 body 是唯一滚动区域，header/close 不随滚动。
- 使用 pinned controlled `Popover`：默认关闭，`Ctrl+2` 或文档 rail 的 Details 按钮打开；outside、Esc、关闭按钮关闭；Esc 后焦点回到正文编辑器；模式切换关闭浮层。无 frontmatter 的文档禁用按钮与命令。
- 布局：TopRight 锚定触发按钮，6 px 偏移，宽度 `min(416, viewport−32)`，高度 `viewport−144`（下限 240）；pinned Positioner 负责窗口 margin、翻转与 clamp。浮层覆盖正文但不改变 `document-panel` bounds。
- 回归测试：
  - `frontmatter_inspector_is_an_overlay_with_unchanged_document_bounds`：默认关闭、按钮打开、真实 form 可见、正文 bounds 不变、outside 关闭、关闭按钮关闭。
  - `inspector_escape_restores_editor_focus_and_mode_switch_closes_it`：`Ctrl+2` 快捷键打开、Esc 关闭并恢复编辑器焦点、模式切换关闭。
  - 现有 frontmatter 字段/校验/patch、模式布局、project layout、resize 与 shell 测试全部继续通过。
- 自动检查：`mise run check` 退出码 0；doctor、fmt、clippy `-D warnings`、build 通过；Rust workspace 846 passed、0 failed、8 ignored；fixture 单元 6 passed，Astro fixture compatibility PASS。
- 原生证据：Linux Wayland（Hyprland），dark/System，窗口 1263×1384，fixture `fixtures/parity-project/src/content/blog/markdown-showcase.md`。截图确认默认关闭、`Ctrl+2` 打开真实 schema 字段（title/description/slug/date/updated/draft/featured/order/rating/tags/category/status/author/related）、右侧 16 px inset、正文与保存状态仍可见、Esc 关闭。截图临时保存于 `/tmp/astrolith-m4b-closed.png`、`/tmp/astrolith-m4b-open.png`、`/tmp/astrolith-m4b-escaped.png`。
- 原生验收：用户于 2026-11-04 确认 M4b 通过，包含内层 Select/DatePicker 先关子层、再关父层的 Esc 顺序（A10）。本次回复未附截图、显示缩放与主题组合明细。
- 未完成/未验证：
  - 最小窗口、150%/200% 缩放、原生 IME 焦点与长表单滚动未截图验收。
  - Schema 刷新时已提交字段编辑依赖完整源重建；未新增专门的 schema-refresh 表单集成测试。

## 14. M5a/M5b 执行记录

- 范围：core `preferences.rs`、GPUI `preferences/{mod,preview,tests}.rs`、`preview/{mod,embedded,tests}.rs`、`shell.rs`、preview crate `{capabilities,external,lib}.rs`、`astrolith-gpui`/`astrolith-desktop` feature 与 lockfile。
- M5a 偏好：`GlobalPreferencesV2` 新增 `preview`。默认 `External`、空 browser argv（系统 opener）、`open_on_enter=false`；`BrowserCommand` 保存 executable 与逐项 arguments，`{url}` 在启动前替换，缺失时追加为最后一项。设置页新增 Preview pane，支持 presentation、browser executable/arguments、Open on Enter，并对占位符、控制字符、无 executable 的 arguments 做内联校验。旧 `global.json` 缺字段时由 serde default 补齐，未知字段与显式 `false` 保留。
- M5a 生命周期：仅在用户显式进入 Preview 时 arm 一次 Open on Enter；attach 早于项目加载时保持 arm，离开 Preview、启动失败或手动打开后取消/消费。server ready 且路由可解析后启动一次；同一次访问内的 render/通知不会重复启动。copy 与 open 共用 `url_for_entry` 的同一 URL。
- 首选浏览器失败时保留已解析 URL，显示错误并提供 “Open with system browser” 回退按钮；系统 opener 路径不变。消息只声称 browser command 已启动，不声称页面已连接。
- M5b 依赖：`astrolith-gpui` 新增可选 feature `embedded-webview`，编译 pinned `gpui-webview 0.8.1` 与 `lb-wry 0.53.3`；`astrolith-desktop` 转发同名 feature。默认构建不引入 WebKit 依赖，`mise run check` 的默认矩阵不编译该 surface。
- M5b 实现：`EmbeddedPreview` 持有 WebView entity，首次 ready 时 `WebView::build`，之后只 `load_url`/`show`/`hide`，不因模式切换重建。Preview 模式在 center Dock 使用 `h_split`（参考 42/58）承载正文与 embedded panel，splitter 可拖动；离开 Preview 隐藏并移除 pane，返回时显示同一实例。URL 跟随 active entry 路由，未知路由回退到 server base URL。
- 可用性：`HostEnvironment` 按实际窗口后端分类——`WAYLAND_DISPLAY` 存在即 Wayland external；只有 `DISPLAY` 时为 X11/XWayland，可启用 embedded。`EmbeddedUnavailable` 明确区分未编译、Wayland、headless 与不支持平台；设置页与 Preview 状态显示原因。应用不修改 `WAYLAND_DISPLAY`/`GDK_BACKEND`，XWayland 验证通过显式环境启动。
- 自动检查：
  - `mise run check` 退出码 0；doctor、fmt、clippy `-D warnings`、build 通过；Rust workspace 862 passed、0 failed、8 ignored；fixture 单元 6 passed，Astro fixture compatibility PASS。
  - `cargo test -p astrolith-gpui --features embedded-webview --locked`：366 passed、0 failed、4 ignored。
  - `cargo clippy -p astrolith-gpui --features embedded-webview --all-targets --locked -- -D warnings`：无问题。
- 新增回归：preview preferred argv/占位符/回退、Open on Enter 单次与 attach 前进入、离开取消、embedded URL 跟随 entry、缺 feature 原因、browser argv 设置校验、capability 的 Wayland/XWayland/headless 分支。
- 原生证据（Linux Hyprland，XWayland；feat `embedded-webview`）：
  - 启动：`env -u WAYLAND_DISPLAY DISPLAY=:1 ASTROLITH_SETTINGS_PATH=… target/debug/astrolith-desktop /tmp/astrolith-m5-project`。日志记录 X11 client、`Astro dev server ready` 与 `embedded preview WebView created url=…/blog/markdown-showcase`；页面真实渲染于右侧分屏（`/tmp/astrolith-m5-xwayland2.png`）。
  - Astro 热更新：运行中修改内容文件，页面自动显示新增 “Live embedded update” 段落（`/tmp/astrolith-m5-xwayland-hot2.png`）。
  - resize：以 Xlib 将窗口从 1000×700 改为 1263 宽再缩回，WebView 与分屏同步重排并继续渲染（`/tmp/astrolith-m5-xwayland-resized.png`）。
  - 焦点/输入：在 GPUI 正文输入字符改变文档；在 WebView 内滚轮滚动页面到 “Hot update marker” 与表格区域（`/tmp/astrolith-m5-webview-wheel.png`），证明 native surface 接收指针输入。
  - 模式切换：Writing→Preview 返回后仍是同一 WebView 实例，进程日志中 `embedded preview WebView created` 始终只有 1 次；返回页面保留原内容（`/tmp/astrolith-m5-rel-writing.png`）。
  - Wayland fallback：正常 Wayland 启动（保留 `WAYLAND_DISPLAY`）时不创建 WebView，Preview 保持 external 操作（`/tmp/astrolith-m5-wayland-fallback.png`）。
  - M5a 原生：设置 `presentation=external`、`open_on_enter=true`、browser executable 为记录 argv 的脚本、arguments `["--new-window", "{url}"]`。启动后 server ready 一次，脚本收到恰好一项 `--new-window` 与 entry URL，应用日志记录只打开一次（`/tmp/astrolith-m5-openenter.log`、`/tmp/astrolith-m5-browser-opened.txt`）。
- 已知限制/未验证：
  - 本机 NVIDIA + XWayland 下 WebKitGTK 默认 DMA-BUF renderer 显示黑屏并记录 `Failed to create GBM buffer of size 1x1`；以 `WEBKIT_DISABLE_DMABUF_RENDERER=1` 启动后正常渲染。应用未自动设置该变量（workspace 禁止 `unsafe set_var`），因此该环境需要显式设置；这是本机/驱动的运行前提，不是已消除的产品缺陷。
  - 外部浏览器原生启动使用记录脚本验证 argv；未实际启动系统浏览器。系统 opener 命令仍由既有单元测试覆盖。
  - macOS/Windows 未编译运行验收；150%/200% 显示缩放、Wayland 下的 embedded 不存在（按设计 external）。
  - 分屏宽度未持久化：用户拖动 splitter 只在当前 Dock 生命周期内有效；模式离开后返回按参考比例重建。M6 若要求恢复该宽度需新增字段。

## 15. M6 执行记录

- 范围：GPUI `preferences/{mod,appearance,workspace,preview}.rs`、`shell.rs`（设置上下文/应用/对话框尺寸/无障碍名称）、`editor/header.rs`、`project_discovery.rs`、`preferences/tests.rs` 与本轮新增 shell 回归测试。核心 `preferences.rs` 与 store 的既有迁移、未知字段、future-version 和 revision 语义未改写。
- 设置整合：`General` 拆为 `Appearance`（主题、界面字体/字号、正文排版、标题颜色）与 `Workspace`（默认模式、按模式布局重置、项目布局覆盖重置、Focus/Typewriter、自动保存、新文件类型、外部编辑器）。`Preview`、`Project Settings`、`Collections`、`Advanced` 内容保留；Apply/Cancel 仍是一次全量草稿，Esc/Cancel 不落盘。
- 默认模式：`Workspace` 页写入 `global.default_mode`。启动无项目时使用该值；打开项目后仍以项目 `last_mode` 优先（既有 M2a 测试与代码路径未变）。
- 布局重置：草稿只记录显式重置集合，不携带打开的对话框快照。Apply 时把全局对应模式恢复为 `ModeLayouts::default()`，项目重置清空 `layout_overrides`；普通 Apply 不触碰 live dock 或保留的底部面板。重置后按解析结果 clamp 并立即应用当前模式。
- 恢复与 clamp：启动按视口宽度 clamp 所有模式布局（回归把 10000 px 存值 clamp 到 390/400），项目范围安装时同样 clamp。Inspector 仍为临时状态，启动和布局重置都不恢复。正文排版、Focus/Typewriter、preview `open_on_enter=false`、空浏览器 argv 与 External presentation 由重启回归覆盖；`false`/`0`/inherit 的解析语义继续由 core `explicit_false_and_zero_survive_inheritance` 与 resolve 测试约束。
- 对话框与大字：宽度改为 `viewport−64`（560–880），高度 `viewport−64`（420–680）；导航宽度随界面字号从 184 增长到最多 280。Apply/Cancel 所在动作行允许说明文字收缩、按钮不收缩；字段标签可换行而不是撑宽对话框。1000×700、`ui_font_size=28` 的 GPUI 布局回归断言 Apply/Cancel 完整位于视口内，`Appearance/Workspace/Preview/Advanced` 均可见且有名称。
- 可访问性：设置页导航使用稳定 ID（`preferences-pane-*`），显式 `tab_index` 使 Tab 顺序为导航(0) → 字段(0 的树序) → Cancel/Apply(1)。顶栏图标按钮（command palette、panels、preferences）、Build 标签关闭、文档操作/Inspector 触发器与关闭、库返回/更多操作补充 `accessibility_label`；测试直接读取快照 label。原有可见文字按钮继续以 label 作为名称。
- 新增回归：
  - `startup_restores_the_default_mode_layout_typography_and_writing_modes`：真实 `global.json` 装载后恢复 Build 默认模式、深色主题、正文排版、Focus/Typewriter、preview 设置，布局超限 clamp，Inspector 关闭。
  - `workspace_settings_apply_the_default_mode_and_reset_one_layout_scope`：Workspace 草稿写 default mode、重置 Writing 布局，Apply 后 live dock、解析布局与磁盘 `global.json` 一致。
  - `project_layout_reset_clears_overrides_while_global_defaults_stay`：项目覆盖重置后继承全局布局，项目文件 `layout_overrides` 清空，全局布局不变。
  - `preferences_sheet_names_controls_and_fits_the_minimum_window`：1000×700 + 28 px 界面字号下 Apply/Cancel 不越界，窗格名称可读，图标按钮有名称，首个 Tab 到导航。
  - `project_and_collection_panes_follow_the_open_project`：更新后的 Appearance/Workspace/Preview 顺序与项目可见性。
- 自动检查：`mise run check` 退出码 0；doctor、fmt、clippy `-D warnings`、build 通过；workspace Rust 测试 866 passed、0 failed、8 ignored；fixture 单元 6 passed，Astro fixture compatibility PASS。
- 原生证据（Linux Hyprland，XWayland/X11 窗口，dark，真实窗口 1000×700，`ui_font_size=28`，`default_mode=build`）：`/tmp/astrolith-m6-x11-prefs.png` 显示 Preferences 对话框、Appearance/Workspace/Preview/Advanced 与 Cancel/Apply 均在窗口内，未裁切关键动作；`/tmp/astrolith-m6-x11-workspace.png` 为同一 1000×700 主窗口，模式切换器与导航文字换行但无裁切。另以 Wayland 后端启动过一次，窗口创建日志正常，但本次会话的工作区截图未捕获该 surface，因此不作为视觉证据，也不据此声称 Wayland 视觉通过。
- 未完成/未验证：
  - A20/A21 的原生重启（两项目 mode/layout 隔离、Reset All、移动项目）目前只有 store/shell 自动化证据，未做用户原生确认。
  - A11/A22 的原生最小窗口长 schema、150%/200% 显示缩放、对比度与 reduced motion 审计未执行；本轮只覆盖 1000×700 + 28 px 界面字号的 GPUI 布局与 XWayland 截图。
  - macOS/Windows 未构建运行验收；Wayland 原生截图未取得。
  - M6 完成不改变 M7 的截图/平台验证待办；A19 真实 Embedded 等既有平台结论不受本轮影响。

## 16. M7 执行记录

- 范围：全量自动检查、性能 workload、fixture oracle、`docs/refactor/` 要求的原生视觉矩阵与回归复核。代码改动只有一处：`project_discovery.rs` 的导航行高与对应回归测试（commit `ea5ec8c`）；其余为验收文档。
- 自动化：`mise run check` 退出码 0；doctor、fmt、clippy `-D warnings`、build 通过；workspace Rust 测试 **867 passed、0 failed、8 ignored**；fixture 单元 6 passed，Astro fixture compatibility PASS；`astrolith-workspace/tests/fixture_oracle.rs` 3 个 fixture oracle 用例通过。
- 回归范围（均在上述 check 中执行）：create/duplicate/rename/trash 与并发编辑保护；draft filter、排序、Content Finder 查找/插入；MDX builder 的 palette/表单/占位符/import 计划；assets 复制与图片路径安全；save/conflict/recovery/autosave 与脱敏失败记录；设置继承/Apply/Cancel/Reset/迁移；Help、命令注册、快捷键文档；preview server 生命周期、路由与 capability；desktop_actions、single-instance、URL scheme 与启动参数。
- 性能 workload（当前开发/发布配置基线，不是与旧版本的优化对比；M7 行高改动不在这些 workload 的测量路径内）：
  - `PROJECTION_DIFF` 1 KB / 100 KB / 1 MB：p50 0.0011 / 0.0266 / 0.1403 ms；单行 100 KB 0.0127 ms。
  - `PROJECTED_WRITER` 1 KB / 100 KB / 1 MB：p50 1.087 / 12.082 / 107.448 ms；单行 100 KB 45.788 ms。
  - `SHELL_RESIZE`：90 帧，avg 2.496 ms，p95 2.873 ms，worst 3.070 ms。
  - `SHELL_OPEN`：1 KB / 100 KB / 1 MB / 单行 100 KB 的 frame 为 1.037 / 11.559 / 115.362 / 9.804 ms。
  - release `EDITOR_WORKLOAD`：100 次 headless input 的 p50 为 2.930 / 4.323 / 2.670 / 9.934 ms；1 MB `set_value` 39.098 ms。
- 新回归与视觉缺陷修复：
  - 原生审查发现 Build 导航中“标题 + 文件名”两行条目与下一条目重叠。原因：pinned `List` 以首行（目录行，单行）测量统一行高，随后所有条目复用该高度。
  - 新增 `library_entry_rows_leave_room_for_the_filename`：集合含子目录时，断言两行条目的实际 bounds 不与下一条目重叠；修复前失败，修复后通过。
  - 修复：目录行与条目行都设置 `min_h(rems(3.25))`，随界面字号缩放；1000×700 与 28 px 界面字号下均无重叠。
- 原生证据（Linux Hyprland 0.57.1，Wayland，HDMI-A-1 2560×1440 @ scale 1.0 / 100%，`grim` 截图；应用 commit `ea5ec8c`；fixture `fixtures/parity-project` 复制到 `/tmp/m7-project` 并装入真实 Astro 7.4.4 依赖；第二项目 `/tmp/m7-project-b` 为 quiet-site 副本）：
  - Writing：dark 1000×700 / 1440×900 / 2200×1300，light 1000×700 / 1440×900 / 2200×1300。正文 sans、16/32、768 measure 居中；保存状态与计数在文档 rail；宽窗测量不变。
  - Build：dark 1000×700 / 1440×900，light 1000×700 / 1440×900。真实完整源、真实标签条、集合导航、目录行与两行条目间距正常；1000×700 下文件标签、集合面包屑、保存状态与 Inspector 触发器均可见。
  - Preview：dark 1000×700 显示真实 stopped 状态与禁用动作；1440×900 启动真实 Astro dev server 后显示 `http://127.0.0.1:41625/`、ready 状态和可用 Open/Copy 动作；模式往返后同一文档与 server 保留。
  - Inspector：dark/light 1000×700 浮层覆盖正文且正文 bounds 不变；28 px 界面字号下字段可读、关闭按钮可达。
  - Settings：dark/light 1000×700；28 px 界面字号下 Appearance/Workspace/Preview/Advanced 与 Cancel/Apply 可见，内容区可滚动。
  - 大字号：`ui_font_size=28` 的 Writing / Build / Inspector / Settings 已截图；模式切换器、文档 rail、Apply/Cancel 未被裁切。导航筛选占位文字与文档 rail 在 1000 宽下更紧凑，但不影响关键动作。
  - 重启隔离：项目 A 切到 Build、项目 B 切到 Preview 后退出；重启 A 恢复 Build，重启 B 恢复 Preview；`global.default_mode` 仍为 `writing`。项目注册表中 `last_mode` 分别为 `build` / `preview`。
  - 原型参考：light Writing 的浏览器截图 `/tmp/m7-mock-writing-light.png`（1000×700）；其余原型区域按源码对照。
  - 截图位于 `/tmp/m7-shots/`（`writing-dark-*`、`writing-light-*`、`build-dark-*`、`build-light-*`、`preview-*`、`inspector-*`、`settings-*`、`mode-a-after-b-restart`、`mode-b-after-restart`）。未提交二进制附件；文件只含 fixture 内容。
- 与 HTML 原型的分区域差异：
  - Writing 原生是可编辑 Markdown 完整源视图并弱化标记；原型是渲染后的演示 HTML，还包含 mock 的固定副标题/导语。原生不复制这类演示文案。
  - 原生文档身份、集合、字数与保存状态位于顶部文档 rail；原型放在底部状态栏。Writing 不保留底部状态栏，1000 宽时字数让位给保存状态。
  - 原生顶栏为搜索、面板、设置图标与平台窗口装饰；原型含 macOS 三点、Focus 段落按钮和主题切换按钮。原生 Focus 通过命令/键盘，不使用原型按钮。
  - Build 原生显示真实完整源、真实标签与按需终端；原型是静态 `index.astro` 加硬编码 Terminal/“Problems 0”。原生的 Problems 计数不会被伪造。
  - External Preview 原生是带真实 server 状态的紧凑底栏并保留可编辑正文；原型是居中演示卡片和 alert，属于已在 VISION/M5a 明确纠正的行为。
  - 中性表面层级、模式 pill、圆角、768 正文 measure 与 mock 基本一致；导航行高在修复后统一且随字号缩放。
- 验收映射：A04/A11/A22 的 dark/light、1000×700、28 px 与键盘驱动证据已补；A15/A16/A17 的真实 server ready 状态已复测；A20/A21 增加原生双项目模式隔离证据；A26 由本轮矩阵与 §11 用户确认共同支撑。A01/A05/A10/A12/A14/A24/A25/A26 的用户确认见 §11；A19 仍引用 §14 的 XWayland/X11 evidence，本轮未重跑 Embedded。
- 未完成/阻碍：
  - macOS/Windows 未构建、未运行、未做 IME/WebView 生命周期验收；不得宣称平台支持。
  - Wayland 下 Embedded 按设计 unavailable；A19 的真实 surface 证据仅为 XWayland/X11，本机 NVIDIA + XWayland 仍需 `WEBKIT_DISABLE_DMABUF_RENDERER=1`。本轮未重跑 Embedded。
  - 显示缩放只覆盖 scale 1.0（100%）；150%/200% 显示缩放未测，28 px 界面字号仅覆盖 UI zoom 轴。
  - A11 的长 schema 滚动、A20/A21 的原生 Reset All 与移动项目、A22 的 reduced motion/对比度审计仍未完成。
  - 完整 LSP、Astro 语法高亮、Problems 面板仍是独立扩展，未以占位 UI 冒充。
  - 因此本记录不宣布 v0.2.0 整体完成；平台矩阵与上述未完成项必须显式保留。

## 17. UI 放大与 Preferences 重设计记录

- 范围：core `preferences.rs` 默认值、GPUI settings store 迁移、`theme/metrics.rs`、`preferences/{mod,appearance,preview}.rs` 与全部 pane 的控件尺寸、`shell.rs` 的 dialog 尺寸。
- 问题与修复：
  - Preferences 出现两遍 “Preferences”：dialog 标题与导航内标题重复。导航内标题已删除；dialog 标题保留一次，内容区改为 pane 标题（如 Appearance）+ 一句说明。
  - 界面整体偏小：默认 `ui_font_size` 从 14 提升到 18，`type_scale.ui` 同步。正文仍使用独立的 16/32，不受界面字号控制。
  - 顶部导航偏矮：TitleBar 高度改为 `rems(3.5)`（18 px 基准约 63 px），模式切换按钮取消 `small` 并增加内边距。
  - 模式切换外框与选中 pill 半径不一致：外层与每个模式按钮统一使用 `rounded_full`，选中态与容器同心。
  - 页面宽度改为百分比：`page_width_percent` 默认 64%，可调 30–100%，按编辑器 pane 宽度渲染并 clamp 到 320–1200 px；旧 `editor_width` px 值一次性转换为百分比（768→64、900→75），备份为 `global.json.pre-page-width.json`，旧键删除。
  - 旧配置 14/16 px 被视为旧视觉比例，一次性迁移到 18；其他显式字号保留。迁移前备份为 `global.json.pre-interface-scale.json`，写入 `interface_scale: 1` 防止重复迁移。用户仍可在 Appearance 面板调整 8–28。
- Preferences 重设计：
  - dialog 尺寸 720–1040 × 560–1040，随窗口增长；左侧导航加宽并随界面字号增长。
  - 表单改为圆角分组卡片（`group_box` 表面 + 细边界 + 内边距）；section 标题与字段标签使用 `text_base`，提示使用 `text_sm`；文本输入与下拉按钮取消 `small` 尺寸。
  - Apply/Cancel 固定到 header 右侧，说明文案在 pane 标题下；`preferences-body` 仍为唯一滚动区。User guide / Keyboard shortcuts 固定在 pane 列表下方，不再被导航高度挤出。
  - Appearance 的 section 由重复的 “Appearance” 改为 “Theme”，主题字段标签改为 “Colour mode”；Preview 的 section 改为 “Browser”，消除 pane 标题与 section 标题同名。
- 验证：
  - 新增 `retired_interface_scale_upgrades_once_and_preserves_other_sizes`：16→18 迁移、备份、`interface_scale` 标记、重复加载不重迁移、20 px 显式值保留。
  - `mise run check` 退出码 0；workspace Rust 测试 **868 passed、0 failed、8 ignored**；fixture oracle 与 fixture 单元测试通过。
  - 原生截图（Hyprland 0.57.1 / Wayland / 100%，dark 与 light，18 px）：`/tmp/m7-prefs-redesign2-1000x700.png`、`/tmp/m7-prefs-1440-appearance.png`、`/tmp/m7-prefs-dark-redesign.png`、`/tmp/m7-app-1440.png`、`/tmp/m7-build-18px.png`、`/tmp/m7-ui-toolbar-percent.png`、`/tmp/m7-writing-percent-1000b.png`、`/tmp/m7-prefs-page-percent.png`。确认单一标题、卡片、导航链接、Apply/Cancel、增高后的顶部导航，以及 64% 页宽随 1000/1440 窗口变化。
- 未完成/未验证：
  - 只对 Appearance 做了逐像素截图；Workspace、Preview、Project、Collections、Advanced 由 GPUI 测试覆盖，未逐页原生截图。
  - 未重新执行 M7 的完整性能 workload；本轮改动不涉及列表/文档热路径，性能结论仍引用 §16。

## 18. Build 文件浏览器执行记录

- 范围：`astrolith-workspace/src/files.rs`、`astrolith-gpui/src/file_browser.rs`、`project_discovery.rs`、`shell.rs`。
- 行为：Writing/Preview 保持原 Library；Build 的左侧栏改为文件浏览器。真实项目树按目录优先、大小写不敏感名称排序；跳过 `node_modules`、`target`、`dist`、`build`、`.git`、`.astro` 等依赖/输出/生成目录，不跟随符号链接，深度 16、节点 5000、检查项 20000 有界。读取失败的子目录被跳过，根目录失败显示错误状态。
- 交互：点击或方向键选择后 Enter 均会确认行；目录展开/折叠，文件打开。内容条目与项目内 `.astro` 走原有 retained session（标签、dirty、history、保存），其他项目文件经 `ReadonlyFileRequested` 交给 Shell 走既定 8 MiB/50,000 行 readonly 路径。扫描在服务阻塞通道执行，渲染不读取文件系统。
- 刷新：项目打开时建立根；进入 Build、索引发布与手动刷新都会重扫，展开状态与当前选中行按 domain ID 保留；打开中的文件按路径高亮。
- 回归：
  - workspace `files` 单元测试：目录优先与嵌套相对路径、依赖/输出/隐藏目录规则、符号链接跳过、根目录不可读报错（3 个用例）。
  - GPUI `build_swaps_the_library_for_a_project_file_explorer`：Writing 显示 Library 且无 explorer；Build 显示 explorer 且无 Library；根行包含 `src`/`index.astro`/`package.json` 且不含 `node_modules`；点击目录只展开该层；点击 `.astro` 建立真实 session；选择后 Enter 打开第二个 `.astro`；内容条目走索引 session；配置文件的 readonly 请求携带规范化路径；切回 Writing 恢复 Library。
- 验证：`mise run check` 退出码 0；doctor、fmt、clippy `-D warnings`、build 通过；workspace Rust 测试 **872 passed、0 failed、8 ignored**；fixture 单元 6 passed，Astro fixture compatibility PASS。
- 原生证据：Hyprland 0.57.1 / Wayland / 100%，`fixtures/parity-project`，主题 Light；截图 `/tmp/astrolith--build-explorer.png`（Build 顶部真实标签条、Explorer 目录优先文件列表、中央完整源码）与 `/tmp/astrolith--build-explorer-final2.png`（最终列表实现、无打开文档时中央文案指向 explorer）。同一原生会话日志记录 `.astro` 与 Markdown 进入 session、`.tsx`/`.jsx`/`.vue`/`.svelte`/`.json`/`.mjs`/`.gitignore`/`bun.lock` 进入 readonly 路径；该会话使用初版 explorer 渲染器，最终 `List` 实现的点击、目录展开、Enter 打开与 readonly 路由由 headless 回归覆盖。
- 限制：原生窗口在并行编译/测试期间被系统以 `SIGKILL`（exit 137）终止，发生在上述交互与截图之后；未重跑完整三模式键盘矩阵、macOS/Windows 或 150%/200% 显示缩放。Writing 的 Library 与 Build 的 explorer 仍共享同一左侧 Dock 宽度与布局记录。

## 19. 按模式分离编辑区执行记录

- 范围：`editor/session.rs`（源视图代码呈现）、`layout/panels.rs`（code/prose 两种 panel 呈现）、`editor/rendered.rs`（原生 Markdown/MDX 阅读面）、`shell.rs`（按模式选择中心面板、Inspector 限制、Astro preview 底部化）、`commands.rs`（Inspector 可用性）、core `PanelLayout` 默认值。
- 行为：
  - Writing 仍是居中写作页：正文投影、Focus/Typewriter、选区工具条、Alt 图片预览与文档 rail 不变；唯一 Inspector 入口保留在此模式。
  - Build 复用同一 `DocumentSession` 与 `EditorState`，`set_source_view` 同时切换行号、缩进参考线、折叠、自动括号/智能缩进、软换行关闭与语法高亮语言（`.md`/`.mdx` → markdown、`.astro` → astro、`.yaml`/`.yml` → yaml，其余 text）。panel 在 Build 铺满 pane、恢复滚动条并隐藏写作 rail。
  - Build 的 buffer tab 条只列真实 retained session：有 buffer 时才出现，无 buffer 时整条不渲染，不再保留空行；多个 tab 在可用宽度内排布，超出后水平滚动（`overflow_x_scroll` + `restrict_scroll_to_axis`），沿用 `tab_active`/`tab_foreground`/`tab_bar`/`border` 设计 token、32 px 高圆角 tab、dirty 点与关闭按钮。
  - Preview 中心显示当前 session 的 writer projection（frontmatter 与 MDX prelude 不出现）经原生 `TextViewState` 渲染的正文；只读、可选择、按文档字体/字号/行高/页宽排版，并解析项目内相对图片与 Astro `public/` 绝对路径。进入 Preview 后焦点归阅读面，键入不会写回 session。
  - Astro preview 从 Preview 模式的中心分屏改为独立底部面板（默认关闭，Panels 菜单在非 Build 模式显示 “Astro preview”）；打开时才 arm Open-on-Enter 并在 Embedded 可用时分屏。Preview 默认布局不再显示该 dock。
  - Inspector 仅 Writing：`ToggleInspector` 可用性要求 `workspace_mode == writing`，Panels 菜单只在 Writing 列出 Details，模式切换关闭并拒绝重新打开。
- 回归：
  - 新增 `build_buffer_tabs_appear_only_while_buffers_are_open`：空 Build 无 `build-tabs`；打开条目后 tab 条与 `build-tab-0` 出现；关闭最后一个 buffer 后整条消失。
  - 新增 GPUI `modes_show_writing_build_and_rendered_preview_surfaces`：Writing 有 rail/Inspector 触发器；Preview 只有 `rendered-preview-panel`、无文档 rail 与 Astro dock、预览源等于 writer projection、键入不修改 buffer、Inspector 命令无效；Build 无文档 rail、编辑器值为完整源、Inspector 命令无效；回到 Writing 恢复 rail。
  - `source_view_edits_the_same_buffer_and_maps_back_to_the_writer` 增加语法语言断言：Build 为 `markdown`，写作面为 `text`。
  - 新增 `images_resolve_from_the_document_or_astro_public`：文档相对、`public/` 绝对与项目外 traversal/URL 的解析规则。
  - 更新 `mode_layouts_persist_per_mode_and_a_project_restores_its_last_mode` 与 core 默认布局断言：Preview 默认 `preview_visible = false`。
- 验证：`mise run check` 退出码 0；doctor、fmt、clippy `-D warnings`、build 通过；workspace Rust 测试 **875 passed、0 failed、8 ignored**；fixture 单元 6 passed，Astro fixture compatibility PASS。
- 原生证据：Hyprland 0.57.1 / Wayland / 100%，dark，fixture `fixtures/parity-project`，`markdown-showcase.md`。截图：
  - `/tmp/astrolith-smoke/writing2.png`：写作页与文档 rail 保留。
  - `/tmp/astrolith-smoke/build.png`：完整源、行号/折叠 gutter、Markdown 语法着色、等宽铺满 pane、Explorer 与标签条。
  - `/tmp/astrolith-smoke/build-notabs.png`：无 buffer 的 Build 不再出现空白 tab 行。
  - `/tmp/astrolith-smoke/build-tabs.png`：单 buffer 时应用 token 风格的 buffer tab 与关闭按钮。
  - `/tmp/astrolith-smoke/preview3.png`：标题/段落/链接/代码块/表格/引用渲染，项目图片 `/parity-banner.svg` 从 `public/` 载入；无文档 rail 与 Astro dock。
- 限制：
  - 远程图片仍依赖 GPUI 自带 HTTP client；当前 Linux 构建没有 HttpClient，只有项目内本地图片可显示。
  - `TextView` 的表格列宽由库决定；窄页宽下会出现单元格末字符换行（截图中 “Frontmatter”），未在应用层覆盖。
  - Build 的 Astro 语法高亮依赖 `tree-sitter-astro`；该 grammar 未注册 `rust`/`ts` 代码块语言，Preview 中这类 fenced code 回退为纯文本。未新增 AST/语言服务。
  - macOS/Windows、150%/200% 显示缩放与 IME 未在本轮重验。

## 20. Build 模式语法高亮与语言服务扩展执行记录

- 范围：`astrolith-lsp`（新 crate：framing、语言/服务器发现、异步 JSON-RPC 客户端、UTF-16/字符坐标转换）、`astrolith-gpui/src/editor/lsp.rs`（provider 适配与诊断投影）、`editor/session.rs`（项目根、客户端生命周期、文档同步、跨文件定位事件）、`project_discovery.rs`（可编辑源扩展、定义目标打开）、`commands.rs`（`Ctrl+Space`、`F12`）。
- 语法高亮：gpui-kit 显式启用 markdown/markdown-inline、astro、css、html、javascript、svelte、toml、tsx、typescript、yaml grammar；Astro 的 injection 依赖同时启用。`editor_language` 覆盖 `.astro`、TS/MTS/CTS、TSX/JSX、JS/MJS/CJS、JSON/JSONC、CSS、HTML、Svelte、YAML、TOML、Markdown/MDX；未知扩展仍回退 `text`。
- 可编辑文件：Build explorer 与 Open Project Source 走同一 retained session 的文本源扩展为 Astro 常用技术栈；集合条目仍优先按索引 session 打开；资源、项目外路径与依赖目录不进入写路径。
- 语言服务：
  - 只在 Build 完整源、已设项目根且语言有服务器定义时启动；活动文档才保留客户端，切换标签/模式/关闭文档停止进程。
  - 发现顺序：项目及祖先 `node_modules/.bin` → `PATH`；不自动安装。服务器：`astro-ls`、`typescript-language-server`、`vscode-json-language-server`、`vscode-css-language-server`、`vscode-html-language-server`、`svelteserver`、`vue-language-server`。
  - 协议：initialize/initialized、FULL 文本 didOpen/didChange/didClose（150 ms 合并）、`publishDiagnostics` → editor `DiagnosticSet`、completion/hover/definition/codeAction 请求。UTF-16 位置在响应边界转换为 editor 字符坐标；诊断在发布前转换。
  - 交互：补全自动触发与 `Ctrl+Space`；悬停显示；`F12`/右键跳转定义，跨文件经 session 事件让 workspace 打开目标标签并定位；`Ctrl+.` code action 只应用当前文档 plain edits。
- 回归：
  - `astrolith-lsp` 单元测试：Content-Length 编解码与跨包读取/超限/缺头、UTF-16 ↔ 字符 ↔ 字节坐标（含非 BMP）、响应嵌套位置转换、语言与扩展映射、项目内/上级/`PATH` 发现与缺失/不可执行/无服务器语言、真实 duplex 握手、server 请求应答、错误响应、stdout 诊断事件。
  - `astrolith-gpui`：`build_highlights_the_astro_stack`、`build_edits_text_sources_but_not_assets`，以及原 Build/源视图回归。
  - 一次性真机式 smoke（未提交）：临时项目内 `node_modules/.bin/astro-ls` 假服务器 + headless GPUI 打开 `.astro` → Build → 服务器收到 didOpen → 编辑器 `DiagnosticSet` 得到 1 条诊断；`Client::spawn_process` 也以真实子进程验证 initialize/诊断/shutdown/exit。
- 限制：未实现 rename、workspace symbol、signature help、diagnostics 面板、inlay hints、semantic tokens、snippet 补全与跨文件/`documentChanges` code action；服务器未安装时仅记录日志并保留高亮，不下载/提示安装。Markdown 无内置服务器。原生 Wayland 上的真实 `astro-ls`/TS server 交互、IME、150%/200% 缩放未在本轮重验。
- 验证：`mise run check` 退出码 0；doctor、fmt、clippy `-D warnings`、build 通过；workspace Rust 测试合计 734 passed、0 failed、7 ignored（含 `astrolith-lsp` 24 个）；fixture 单元 6 passed，Astro fixture compatibility PASS。`Cargo.lock` 随新 crate 与 grammar 依赖提交。

## 21. Writing WYSIWYG 执行记录

- 范围：`astrolith-document/src/wysiwyg.rs`（纯渲染块解析、行内标记与源映射）、`astrolith-gpui/src/editor/writer_surface.rs`（GPUI Kit `editor_host` 渲染、光标/选区绘制、点击/拖选、上下左右导航、Focus/Typewriter 滚动）、`editor/session.rs`（writer entity、点击选区、导航拦截、copyedit/占位符转发）、`layout/panels.rs`（Writing 渲染 writer surface）、`editor/selection_toolbar.rs`（用渲染选区定位）。
- 行为：
  - Writing 不再绘制 Markdown 标记：ATX 标题字号由 `BlockNode::Heading` 提供，`**`/`*`/`~~`/行内代码/链接目标/列表符号/引用符号/围栏均隐藏；粗体、斜体、删除线、行内代码背景、链接文字、预算代码块、列表和引用按排版呈现。
  - retained `EditorState` 仍是唯一输入模型；光标、选区、IME、复制粘贴、格式化命令、撤销/重做与保存沿用原 session 事务。`WysiwygDocument` 只从 `WriterProjection.visible` 解析，渲染面不读取磁盘，也不越过可见字节边界。
  - 点击/拖选通过 `LeafGeometry` 映射回 source 字节；左/右导航跳过隐藏标记，上/下导航按渲染行框移动；浮动格式工具条使用渲染选区矩形定位。
  - Focus 按渲染块与当前句子范围降低非活动块透明度；Typewriter 用渲染光标位置滚动居中，并在普通模式下保证光标可见。
- 回归：
  - `astrolith-document`：8 个 WYSIWYG 用例覆盖标题/强调/代码/链接/图片/列表/引用/围栏、隐藏标记的 source 映射、尾随空行、畸形语法保留与 UTF-8 边界。
  - `astrolith-gpui`：新增 `writing_surface_click_moves_the_caret_and_keeps_typing_working`，点击渲染页后光标进入文档且键入回到 buffer；原 384 个 GPUI 用例全部通过。
- 验证：`mise run check` 退出码 0；doctor、fmt、clippy `-D warnings`、build 通过；workspace Rust 测试 **914 passed、0 failed、8 ignored**；fixture 单元 6 passed，Astro fixture compatibility PASS。
- 原生证据：Hyprland / Wayland，`fixtures/parity-project`，`work-in-progress.md`，截图 `/tmp/wysiwyg-native.png`。确认标题按标题字号渲染、正文不显示 `#`/反引号、行内代码有背景、页面保持居中与留白。
- 限制：setext 标题、表格、缩进代码块、HTML/MDX 组件与引用链定义仍按普通文本/源码显示；图片显示 alt 文本而非内嵌图片；原生 IME 组合、150%/200% 缩放与 macOS/Windows 未在本轮重验。
