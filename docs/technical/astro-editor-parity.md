# 既有功能与保留审计入口

旧 parity ledger 不在当前基线。此入口不重建已删除的逐行结论，也不宣称旧验收已通过。

2026-11-04 对实际源码建立的功能保留矩阵见 [ARCHITECTURE](../refactor/ARCHITECTURE.md)，覆盖项目发现、导航、编辑、Frontmatter、builder、assets、保存/恢复、设置、预览与桌面能力。现有测试位置、已知缺口和后续证据要求见 [ACCEPTANCE](../refactor/ACCEPTANCE.md)。

产品仍为 Linux alpha；真实 Wayland 交互、浏览器/协议启动、包安装与签名发布不由 headless 测试证明。`.astro` 开发文件编辑、交互 Terminal 与真实 Embedded surface 当前尚未交付，已列为 v0.2.0 必交付；完整 LSP 为单独扩展。不能从原型推断当前已有这些能力，也不能因缺失而省略必交付项。旧 UI 外观将按 mock 替换。后续范围与依赖见 [MILESTONES](../refactor/MILESTONES.md)。
