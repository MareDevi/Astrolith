# 日志与本地证据入口

`apps/desktop/src/main.rs` 初始化 `astrolith-observability` 并持有 LogGuard。服务使用 tracing；保存失败证据由 `astrolith-gpui/src/editor/save_failure.rs` 记录。UI 仍需显示可恢复错误，日志不能替代用户反馈。

本地目录、配置环境变量与隐私边界以 [Privacy and local data](../user/privacy.md) 为准。恢复数据、日志、脱敏 save-failure artifact 是不同数据；Reset All 不应删除这些证据。重构保留策略见 [ARCHITECTURE](../refactor/ARCHITECTURE.md)，回归见 [ACCEPTANCE](../refactor/ACCEPTANCE.md)。
