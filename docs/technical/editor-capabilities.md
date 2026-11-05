# 编辑器能力审计入口

实际编辑所有权为 `astrolith-gpui/src/editor/session.rs::DocumentSession`：完整源 DocumentBuffer、WriterProjection 与 retained EditorState。正文 undo/redo 来自 pinned 编辑器；隐藏 Frontmatter-only 补丁尚缺统一 history 集成证据。

Kit 固定为 0.8.1，GPUI engine 为 gpui-pre 0.3.10。Writing 使用 WYSIWYG 渲染面：Markdown 标题、强调、行内代码、链接文字、列表、引用、围栏代码与分隔线按排版呈现，标题井号、强调星号、列表符号、链接目标、代码围栏不绘制；段落换行、标题字号、代码背景和 Focus/Typewriter 均来自渲染层。Build 在同一个 EditorState 上打开行号、缩进参考线、folding、自动括号/智能缩进和语法高亮（Markdown/MDX、Astro、TypeScript/TSX、JavaScript/JSX、JSON/JSONC、CSS、HTML、Svelte、YAML、TOML；未知扩展名回退 text），并关闭软换行。

WYSIWYG 的数据所有权没有分叉：retained `EditorState` 仍是输入模型，负责光标、选区、IME、复制粘贴和编辑器内部行为；`astrolith-document::wysiwyg::WysiwygDocument` 从 writer projection 解析渲染块并保留每个可见字节到源字节的映射。`astrolith-gpui/src/editor/writer_surface.rs` 用 pinned `editor_host` seam 渲染 `BlockNode`、收集每个 leaf 的 `LeafGeometry`，据此绘制光标、处理点击/拖选和上下左右导航；输入、撤销和保存仍走原有 session 事务。鼠标点击与键入、模式往返和隐藏 frontmatter/imports 的完整源映射有 headless 回归。

WYSIWYG 的限制：setext 标题、表格、缩进代码块、HTML/MDX 组件和引用链接定义仍按普通文本或原始源码显示，不伪造富文本结构；图片显示 alt 文本而不是内嵌图片；行内标记只在可解析成对分隔符时隐藏，畸形语法保持可见。渲染面不读取磁盘，也不绕过 `WriterProjection` 的可见字节边界。

Build 的工程文件导航把 Astro 常用文本源打开为同一个 retained `DocumentSession`：`.astro`、`.ts/.mts/.cts/.tsx/.jsx/.js/.mjs/.cjs`、`.json/.jsonc`、`.css/.scss/.less`、`.html`、`.svelte`、`.vue`、`.md/.mdx`、`.yaml/.yml` 与 `.toml` 可编辑；二进制/资源文件与项目外文件仍走 readonly 快照。内容集合条目仍优先使用其 collection session，frontmatter 表单与 schema 只作用于这些条目。

语言服务由 `astrolith-lsp` 提供：仅当 Build 显示完整源、存在项目根且对应语言有已安装服务器时，才为当前活动文档启动一个 stdio LSP 客户端。服务器按 `node_modules/.bin`（含上级 workspace 提升）优先、`PATH` 兜底查找仓库已安装的可执行文件，不自动安装：`astro-ls`、`typescript-language-server`、`vscode-json-language-server`、`vscode-css-language-server`、`vscode-html-language-server`、`svelteserver`、`vue-language-server`。支持诊断（来自 `publishDiagnostics` 的 gutter/下划线）、补全（自动触发与 `Ctrl+Space`）、悬停、跳转定义（`F12` 或右键菜单，跨文件经 workspace 打开目标标签）与 code action（`Ctrl+.`，只应用当前文档的 plain edits）。服务器以完整文本 `didChange` 同步，输入后 150 ms 合并；切换模式、切换到其他标签或关闭文档会停止该客户端的进程。Markdown 没有内置服务器，仍只有语法高亮。

不是完整 IDE：没有 rename、workspace symbol、signature help、diagnostics panel、inlay hints、semantic tokens、snippet 补全或 `documentChanges`/跨文件 code action；服务器未安装时只记录日志并保留普通高亮，不下载依赖。LSP 的协议层与进程生命周期有自动化测试；原生服务器交互仍需真机证据。任意 Open file 入口仍 readonly。

Preview 模式不编辑文档：它用 pinned rich-text `TextViewState` 渲染 writer projection，frontmatter 与 MDX prelude 不出现；项目内相对图片与 Astro `public/` 路径由 GPUI asset cache 异步读取，app 不提供远程图片 HTTP client。Astro preview 是独立底部面板，不是 Preview 模式的内容。

当前功能及准确限制见 [User guide](../user/guide.md)，迁移边界见 [ARCHITECTURE](../refactor/ARCHITECTURE.md)，完整源事务与 Build 扩展见 [MILESTONES](../refactor/MILESTONES.md)。
