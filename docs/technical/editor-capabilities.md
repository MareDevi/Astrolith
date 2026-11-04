# 编辑器能力审计入口

实际编辑所有权为 `astrolith-gpui/src/editor/session.rs::DocumentSession`：完整源 DocumentBuffer、WriterProjection 与 retained EditorState。正文 undo/redo 来自 pinned 编辑器；隐藏 Frontmatter-only 补丁尚缺统一 history 集成证据。

Kit 固定为 0.8.1，GPUI engine 为 gpui-pre 0.3.10。Writing 关闭行号、folding、自动括号与 smart indent，保留正文装饰与 Focus/Typewriter；Build 在同一个 EditorState 上打开行号、缩进参考线、folding、自动括号/智能缩进和语法高亮（Markdown/MDX、Astro、YAML；未知扩展名回退 text），并关闭软换行。框架提供代码编辑相关接口不代表应用已有 Astro 语言服务或 LSP。任意 Open file 仍 readonly。

Preview 模式不编辑文档：它用 pinned rich-text `TextViewState` 渲染 writer projection，frontmatter 与 MDX prelude 不出现；项目内相对图片与 Astro `public/` 路径由 GPUI asset cache 异步读取，app 不提供远程图片 HTTP client。Astro preview 是独立底部面板，不是 Preview 模式的内容。

当前功能及准确限制见 [User guide](../user/guide.md)，迁移边界见 [ARCHITECTURE](../refactor/ARCHITECTURE.md)，完整源事务与 Build 扩展见 [MILESTONES](../refactor/MILESTONES.md)。
