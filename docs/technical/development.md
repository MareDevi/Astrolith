# 开发、验证与打包

此文件是 2026-11-04 M0 审计新增的兼容入口。旧技术文档已从基线删除；当前架构与重构计划见 [ARCHITECTURE](../refactor/ARCHITECTURE.md) 和 [MILESTONES](../refactor/MILESTONES.md)。

## 工具与检查

`mise.toml` 管理 Rust 1.99.1、Node 26.12.1、Bun 1.4.4。先 `mise install`，通过 `mise run doctor` 检查系统依赖，再执行 `mise run build` / `mise run dev`。`mise run check` 组合 doctor、fmt、lint、build、test 与 fixture verification；Cargo build/test/clippy 使用 `--locked`。

核心服务与领域逻辑采用 TDD。按变更选用 `mise run test:document`、`test:gpui`、`test:workspace`、`test:astro`。性能任务见 `mise.toml`；常规 headless 不等于真实平台验收。验收界限和本轮结果见 [ACCEPTANCE](../refactor/ACCEPTANCE.md)。

## 架构边界

入口只初始化 Kit、窗口和日志。`astrolith-gpui` 编排原生实体，`astrolith-core` 管模型与设置，`astrolith-document` 管完整源/patch/save，`astrolith-astro` 管静态发现/schema/components，`astrolith-workspace` 管文件与监听，`astrolith-preview` 管进程/路由/surface。GUI render 不执行 blocking I/O。

## 5. Packaging and update signing

打包任务为 `mise run package:metadata`、`package:validate`、`package:deb`、`package:appimage`、`package:rpm`，输出在 `dist/`。依赖与包安装步骤见 [Linux packaging](../../packaging/linux/README.md)。

`astrolith-gpui/src/update.rs` 的更新能力要求构建配置 HTTPS manifest URL 与 Ed25519 public key。仓库用户文档明确尚无完整签名发布与干净环境安装验收证据。不能将构建成功等同于发布可用；继续保留 capability-disabled 和错误反馈。
