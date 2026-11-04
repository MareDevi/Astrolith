# 编辑器能力审计入口

实际编辑所有权为 `astrolith-gpui/src/editor/session.rs::DocumentSession`：完整源 DocumentBuffer、WriterProjection 与 retained EditorState。正文 undo/redo 来自 pinned 编辑器；隐藏 Frontmatter-only 补丁尚缺统一 history 集成证据。

Kit 固定为 0.8.1，GPUI engine 为 gpui-pre 0.3.10。当前 Writer 关闭行号、folding、自动括号与 smart indent。框架提供代码编辑相关接口不代表应用已有 Astro 语言服务。任意 Open file 仍 readonly。

当前功能及准确限制见 [User guide](../user/guide.md)，迁移边界见 [ARCHITECTURE](../refactor/ARCHITECTURE.md)，完整源事务与 Build 扩展见 [MILESTONES](../refactor/MILESTONES.md)。
