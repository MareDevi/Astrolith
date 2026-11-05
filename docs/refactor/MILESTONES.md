# v0.2.0 分阶段计划

本轮仅 M0 文档与范围校正。真实 WebView、交互终端、Astro 开发文件编辑为必交付，旧 UI 外观淘汰，mock 视觉为目标。每阶段必须可独立编译、回归、回滚；通过 `mise run check` 后提交一个或多个原子 commit。先写领域/服务/持久化行为测试，再改实现；不为配置、getter 或静态布局堆测试。

## 每阶段的复用评估门

复用现有模型、状态、功能或代码前，必须评估其对 v0.2.0 的合理性、效率、复杂度和扩展边界。功能保留矩阵不是代码冻结清单。每阶段说明哪些直接复用、哪些优化/重构、哪些重写，以及原因、风险和验证结果。

项目仍处于 alpha，可以重写不适配的模块、调整状态所有权与跨模块边界，无需保留旧内部 API 或多余兼容层。不要因怕改动而保留低效算法、重复状态、过大实体或错误抽象。性能改动记录代表性工作负载的前后测量；领域/服务/持久化改动以关键行为测试约束，删除被替代代码和只绑定旧实现细节的测试。

后文列出的复用路径与模块是实施起点；若评估表明替换更合适，应更新该阶段设计后执行，不将“复用”视为必须守住的结构。仍按可编译、可验证、可回滚的逻辑变更分步提交，保持功能与用户数据安全。

## 进度

| 阶段 | 状态 | 证据摘要 |
| --- | --- | --- |
| M0 | 已完成 | 审计文档与检查基线见 [ACCEPTANCE.md](ACCEPTANCE.md) §5 |
| M1 | 已完成（视觉分区域迁移继续） | 中性 light/dark 主题、OKLCH 边界、Token/尺度快照、旧 palette 配置备份迁移；记录见 ACCEPTANCE §6 |
| M2a | 已完成（用户原生验收） | WorkspaceMode、每模式布局、模式切换保留 session/编辑历史、旧布局备份迁移；记录见 ACCEPTANCE §7、§11 |
| M2c | 已完成（用户原生验收） | 项目绑定 PTY、ANSI 屏幕/回看、输入/复制、resize、退出与项目切换清理；证据见 ACCEPTANCE §8、§11 |
| M4a | 核心完成，真机/表单路径待验收 | 单一完整源事务栈覆盖正文与隐藏 FM，Ctrl+Z/Redo 走同一顺序；记录见 ACCEPTANCE §9 |
| M2b | 已完成（用户原生验收；Astro 高亮未启用） | Build complete-source、真实标签条、项目 `.astro` 扫描/编辑/保存、无 frontmatter 路径；记录见 ACCEPTANCE §10、§11 |
| M4b | 已完成（用户原生验收） | FrontmatterForm 迁入 controlled Popover，默认关闭、Ctrl+2/按钮打开、outside/Esc/close 关闭与焦点恢复；右 Dock 与 inspector layout 字段移除；记录见 ACCEPTANCE §13 |
| M3 | 已完成（用户原生验收） | 可持久化文档字体/字号/行高/measure 与 Focus/Typewriter；mock 16/32、768 默认；排版变更不触碰文档状态；记录见 ACCEPTANCE §12 |
| M5a | 完成（代码与自动化；原生浏览器启动未复测） | External 默认、browser argv、Open on Enter 单次、系统回退、同一 active route；记录见 ACCEPTANCE §14 |
| M5b | 完成（XWayland/X11 真实 surface；Wayland external） | 可选 `embedded-webview` + gpui-webview，保留实例、分屏 resize、输入、热更新、模式切换与失败/平台原因回退；记录见 ACCEPTANCE §14 |
| M6 | 完成（代码、自动化与 1000×700 原生大字体验证）；原生重启/缩放验收待用户确认 | 设置分为 Appearance/Workspace/Preview，保留 Project/Collections/Advanced；默认模式与按模式布局重置进入 Apply 草稿；启动/项目布局 clamp、临时浮层不恢复；可访问名称与 Tab 顺序有回归；记录见 ACCEPTANCE §15 |
| M7 | 完成（Linux Wayland 矩阵、回归与视觉收尾；平台/缩放边界显式保留） | check 867 passed、perf workload、fixture oracle；dark/light × 1000×700/1440×900/2200×1300、28 px、Inspector/Settings/Preview、双项目模式隔离；导航两行条目重叠修复与回归；记录见 ACCEPTANCE §16 |
| M7-UI | 完成（用户反馈后的界面放大与 Preferences 重设计） | 默认界面 18 px，旧 14/16 一次性迁移并备份；Preferences 去掉重复标题、dialog 放大、分区卡片与固定动作栏；记录见 ACCEPTANCE §17 |
| M9 | 完成（协议层自动化与 headless smoke；真实服务器原生交互待验收） | Build 语法高亮覆盖 Astro 常用技术栈；`astrolith-lsp` 客户端支持诊断/补全/悬停/定义/code action 与项目内服务器发现；记录见 ACCEPTANCE §20 |
| M8 | 完成（自动化与原生截图） | Writing 保留写作页；Build 单一 session 切换为 IDE 源码呈现（行号、缩进参考线、折叠、语法高亮、等宽铺满）；Preview 渲染 Markdown/MDX 正文；Inspector 仅 Writing；记录见 ACCEPTANCE §19 |
| M10 | 完成（headless 回归与原生截图） | Writing 使用 WYSIWYG 渲染正文，Markdown 标记隐藏；EditorState 仍为输入/历史模型；点击、拖选、导航、Focus/Typewriter 与完整源映射；记录见 ACCEPTANCE §21 |

M1 的退出只覆盖主题与共享控件本身；mock 的模式切换器、Build 标签条等区域属于 M2 及后续里程碑，未在 M1 提前实现。

## 依赖顺序

`M0 → M1 → M2 → M3`；`M2a → M4a → M2b/M4b`，`M2a → M2c`；`M2 → M5a → M5b（平台实现门）`；设置模型/迁移随 M2/M3/M5 交付，M6 完成设置 UI 与集成；M7 汇总真实平台证据。

M4a 的完整源事务若成为 Build 源码切换前提，应提前到 M2b，不能先交付会丢历史的 Source view。M5b 必须在至少一个明确支持的真实平台交付 surface；Wayland 保持 external fallback，但全平台 unavailable 不能作为 v0.2.0 完成状态。

## M0 — 审计与设计文档

交付 VISION、ARCHITECTURE（含功能保留矩阵）、DESIGN_SYSTEM、MILESTONES、ACCEPTANCE，以及用户文档所需的兼容技术索引。

逐项核对 pinned dependency、session/projection/history、schema/form、watcher/save/recovery、preview lifecycle、settings 和命令；区分原型演示与已有功能。只改文档，不改 Rust、依赖、版本或原型。以 check 结果记录真实基线，不声称视觉/平台验收通过。

## M1 — Neutral Tokens 与共享控件

范围：`astrolith-gpui/src/theme/{tokens,palettes,mod,typography}.rs`，邻近 `ui.rs` 和局部组件消费点。

先提供 Neutral light/dark、语义状态、spacing/type/radius/elevation snapshot，再迁移 toolbar、选择、菜单、表单的共同风格。新旧配置统一迁移到 mock Neutral 外观，淘汰旧 palette 与旧 UI 入口，保留功能数据和配置备份。核对 OKLCH 转换与透明合成；复用 Kit Theme 同步路径。

退出条件：light/dark/System 实际切换，选中态与 focus 中性且明确，正文/overlay/control 层级一致，主题测试与 check 通过。对照原型颜色表，记录仍未迁移区域；需要原生截图，单有主题测试不能关视觉任务。

风险：CSS/Kit 同名语义不同、主题投影不同步、UI zoom 破坏密度。回滚只涉及 theme/消费点，不碰文档状态。

## M2a — Workspace Mode 与布局管理

范围：core `preferences.rs`、GPUI `layout/`、`shell.rs`、`commands.rs`、settings store/resolver 测试。

新增 WorkspaceMode 和各模式布局策略；复用 ProjectWorkspace 与 retained session。新用户 Writing 默认全工具隐藏；Build 使用保存偏好；External Preview 使用紧凑状态/操作和可编辑正文。Inspector open 为临时状态，进入其他模式关闭并恢复焦点。首次升级按 mock 建立新模式布局，不恢复旧 UI 构图；后续新布局的恢复顺序见 ARCHITECTURE。

新增命令必须与当前 registry 比较，禁止覆盖 Ctrl+B/I、Ctrl+Enter 和编辑器 key contexts。具体新模式快捷键通过命令表统一定义，并派生菜单/帮助。

退出条件：三模式循环切换后 session/editor entity、未保存源码、光标、选区与 undo 仍保持；切换过程中没有文件重读或 server restart。测试真实行为，不测试枚举定义。每种布局在最小窗口下可用。

## M2b — Build 的真实源码能力

范围：`editor/session.rs`、`layout/panels.rs`、`editor/header.rs`、`project_discovery.rs` 与 file/workspace 边界。

先完成 M4a 事务门，再给同一文档提供 complete-source 表示与真实标签条，保留 Writer 的 hidden-region 映射。必须新增 `.astro` 项目源码编辑，并明确区分内容条目和开发文件身份、项目内路径安全、保存/恢复和 schema 不适用状态；不能把任意 readonly 文件入口直接改成 unrestricted write。

调查 pinned `tree-sitter-astro` 的实际能力、语言配置与 feature 成本；有真实高亮证据再启用。Diagnostics 保留已有配置/schema/server 错误；完整语言诊断/LSP 是独立扩展，不使用硬编码 “Problems 0”。

Terminal 虽非 v0.1.0 已有能力，仍是本次必交付；独立由 M2c 实现。dev-server 输出与 IDE launch 不能替代交互终端。

退出条件：真实 `.astro` 和内容文件的打开、编辑、保存、历史与恢复闭环；文件标签对应 sessions；普通内容能力全部保留。依赖变动单独 commit 并提交 lockfile。风险为投影切换与新增文件类型写入边界。

## M2c — 原生交互终端（必交付）

新增项目绑定的 terminal service/state 与 GPUI 原生终端视图，接入 Build 底部 Dock，并按 mock 的 tabs、工具栏、间距与字体实现。实施时选择实际 PTY/终端解析库，先核对当前文档、源码、平台能力与依赖成本，依赖变动单独提交。

必须支持真实 shell 输入输出、ANSI/颜色和光标呈现、滚动回看、选择/复制、键盘控制字符、PTY resize、启动失败、正常退出/退出码与会话重新启动。输出使用有界缓冲和批量更新，不能阻塞 GPUI。终端焦点与编辑器快捷键隔离。

隐藏 Dock 或切模式不销毁 PTY；关闭会话、切项目与退出必须清理归属子进程，不能遗留 shell。项目切换时的终端归属和清理规则明确显示并测试，不把终端进程当 Astro DevServer。

退出条件：在真实原生窗口内运行交互命令，输入/输出、ANSI、Ctrl+C、resize、滚动、退出/重启与资源清理均有证据；不能以静态日志、网页终端或外部应用启动代替。纯状态/进程行为测试加真实平台验收，UI 对照 mock。

## M3 — 沉浸写作

范围：`writing_modes.rs`、`writer_chrome.rs`、`layout/panels.rs`、`theme/typography.rs`、core/settings/preferences。

复用当前句子 Focus、typewriter geometry、prose decorations 和 selection toolbar。提供可持久化字体、字号、行高、measure、Focus/Typewriter 设置。明确保留 sentence focus，若追加 paragraph focus 先定义段落与代码/MDX边界并测试。

退出条件：Writing 默认中心页；长文输入、跳行、选区、滚动、Typewriter、IME composition 不丢失；字号/行高变化不改变文本和历史；错误/保存状态仍可发现。对照原型的留白与层级，以 mock 的 sans 字体、字号、行高与轮廓为目标；仅必要的平台/CJK fallback 差异需记录原因，旧 serif 风格不保留。

风险：字体测量缓存、focus decoration、选区工具条焦点和 auto-hide。真机证据包含中文输入及组合取消，不以 headless 代替。

## M4a — 完整源事务与 Undo/Redo

范围：DocumentSession 及必要的 document transaction seam。此阶段是数据正确性工作，不能混入浮层美化。

先添加重要失败回归：正文改动后只修改隐藏 FM，再 Undo/Redo；可见 source/Writer 坐标切换；未完成保存、外部变更、schema refresh、Unicode/CRLF。当前 DocumentBuffer 只保存 revision/baseline，不拥有 undo 栈；不能把其 apply_text 测试当历史集成证明。

基于 pinned EditorState history 可用 seam 选择最小桥接。完整源事务维护一个顺序，body/FM/源编辑都能撤销；重建 projection 不能产生额外用户事务或清除历史。保留 MDX 插入“撤销 tag 留 import”的现有明示语义，若需改变应单列行为变更。

退出条件：FM-only 更新是真正 undoable，交错编辑不丢正文/metadata；save/recovery 对照完整源 revision；所有拒绝路径保留原字节。不能以两套独立 history 勾选通过。

## M4b — Floating Frontmatter Inspector

范围：`shell.rs`、`frontmatter/{mod,fields}.rs`、layout/panels 关联及交互测试。

保留 Form entity、字段 row entities 和 FrontmatterEdit subscription，移除永久 right dock 的承载职责。用 pinned controlled Popover 生命周期；窗口 clamp、最大高度与单一表单滚动 owner。required/default/description/validation、image/reference、nested/raw/refusal 都保留。

退出条件：默认关闭、按钮与快捷键打开、outside/Esc 关闭、关闭后焦点正确；正文 bounds 不变；内层 DatePicker/Select 不误关父层；schema 改动不覆盖 dirty edit；最小窗口全部控件可达。做原生键盘/IME/滚动/hit-test 验收。

## M5a — External Preview 设置与生命周期

范围：core preferences、`preferences/`、GPUI preview、preview crate `external.rs`/`surface.rs`/`capabilities.rs`。

新增 presentation backend、preferred browser 与 Open on Enter。首版默认 external、系统默认 browser、Open on Enter 关闭；启用后只在显式进入 Preview 时，在 server ready 且路由可解析后打开一次，切换/失败取消旧请求，不因 render 重复启动。未运行 server 经现有 start 流程，重用同项目服务。

Preferred Browser 用 executable+argv 或经验证的平台 browser identity，不使用 shell 拼接。空值使用系统默认 opener；失败提示并提供系统默认浏览器回退操作。区分“启动 browser command 成功”和“页面已连接”，无法探测时只展示真实 server 状态。

退出条件：copy/open 使用同一 active route，模式循环不重启 server；端口冲突/进程退出/超时/未知路由反馈真实；无空白预览 pane；偏好重启恢复。

## M5b — 真实 Embedded WebView（必交付）

当前无真实 WebView，不直接启用空 feature。先论证窗口句柄、线程/事件循环、surface layering、resize、输入焦点、生命周期与额外依赖成本。必须在明确支持的平台接入真实 WebView 并保持实例，交付实际运行证据；受限平台或运行失败提供原因和 external fallback。

退出条件：至少一个明确支持的真实平台能随 Astro 页面更新而渲染；分屏 resize/焦点可用；模式切换不无谓重建；Wayland+DISPLAY 和 headless 均 unavailable。缺平台/依赖时记录阻碍并继续解决，不能据此将此必交付项标记完成或推到未定期扩展；不画截图替代。macOS/Windows 的开发任务不等于本次 Linux alpha 宣布支持。

## M6 — 设置、恢复与可访问性整合

整合 Appearance/Workspace/Preview 设置，不删除现有 Project/Collections/Advanced 内容。延用 Apply/Cancel 草稿、作用域 Reset、unknown envelope 与 future-version 保护。恢复 per-mode 布局时 clamp，临时浮层不恢复。迁移坏配置、旧配置、移动项目和异步写序列。

退出条件：重启恢复 default/last mode、layout、body typography、Focus/Typewriter、preview 设置；false/0/inherit 正确；键盘顺序和 accessible names 可核验；1000×700 与更大字号不裁切关键动作。

## M7 — 回归与视觉收尾

运行 check、相关 perf 任务和 fixture oracle；围绕变更风险补测试，不重复测试库本身。依 [ACCEPTANCE.md](ACCEPTANCE.md) 保存真实窗口截图/录屏/步骤结果、系统与显示后端、应用 commit 和主题/缩放。先做 Linux Wayland，再验证实际承诺的平台。

对照 HTML 原型记录差异：主题/表面/圆角/正文中心/工具显隐/浮层，而不是只看静态截图相似度。回归 covers create/duplicate/rename/trash、过滤/查找、builder/assets、save/conflict/recovery、settings、Help/命令、preview 与桌面集成。

退出条件：所有 mandatory acceptance 有可追溯证据；平台阻碍与未交付功能显式列出。真实 Embedded、M2b Astro 开发文件编辑、M2c 交互终端任一缺失，v0.2.0 不得整体标记完成；完整 LSP 的额外范围单独记录。每次提交只包含该逻辑变更，保持用户原型和其他未提交文件不被意外暂存。

## M8 — 按模式分离编辑区

范围：`editor/session.rs`、`layout/panels.rs`、`editor/rendered.rs`、`shell.rs`、`commands.rs`、core `PanelLayout` 默认值。

Writing 保留现有写作页。Build 与 Writing 共用同一个 `DocumentSession`/`EditorState`，但由 `set_source_view` 打开代码呈现：行号、缩进参考线、折叠、智能缩进/自动括号、关闭软换行、按扩展名选择语法高亮（Markdown/MDX、Astro、YAML）；panel 在 Build 铺满 pane、显示滚动条并隐藏写作 rail。Preview 中心改用 `TextViewState` 渲染正文投影（frontmatter 与 MDX prelude 已隐藏），只读、可选择、按设置排版；Astro preview 改为独立底部面板，默认关闭，模式本身不再占用中心区。Inspector 仅 Writing 可用：命令可用性、Panels 菜单与模式切换共同保证。

退出条件：三模式往返不重建 session，Build 显示完整源，Preview 显示渲染正文且键盘输入不写回文档；`mise run check` 通过；原生窗口截图见 ACCEPTANCE §19。
