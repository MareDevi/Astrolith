# Astrolith v0.2.0 产品理念

审计日期：2026-11-04。基线：`244f3ee`，workspace 版本 `0.1.0`。
本轮只完成 M0：代码与原型审计、迁移设计、验收计划；以下目标均不代表已经实现。

2026-11-04 范围校正：真实 Embedded WebView、交互终端和 Astro 开发文件编辑均为 v0.2.0 必交付。视觉以 astrolith-web-mock 为主要验收依据，淘汰现有旧 UI；复用功能、领域模型和状态基础设施不表示保留旧外观。

## 产品核心

**Astrolith 默认是一张纸，需要时才变成 IDE。**

首要任务是编辑 Astro 内容集合中的真实文档。Writing 让正文占据注意力中心；Build 按需暴露开发工具；Preview 检查同一项目的真实页面。三种模式共享文档身份、未保存文本、历史、保存状态和项目上下文。

借鉴 iA Writer 的留白与写作节奏、Zed 的键盘效率与工具布局、Base UI 的克制控件。主界面继续由 Rust 和 GPUI Kit 原生绘制。真实 WebView 必须作为网页预览后端交付，由用户选择 Embedded 或 External；不能接管应用主界面。

## 已有基础与目标差异

当前应用已经具有居中写作、正文投影、保留的多文档 session、动态 Frontmatter、组件插入、冲突感知保存、恢复与外部 Astro 预览。这些能力应延续，但现有实现只是候选基础，不是不可替换的约束。完整功能保留矩阵见 [ARCHITECTURE.md](ARCHITECTURE.md)。

目前没有 Writing / Build / Preview 工作区模式枚举。`editor/writing_modes.rs::WritingModes` 只是 Focus/Typewriter 两个开关。`layout/state.rs` 是现有 Dock 布局的别名。不能将这些概念混用。

现有项目编辑以 Markdown、MDX、YAML 集合条目为中心；任意本地文件为 readonly。原型中的 `index.astro` 编辑器、多文件标签条、Terminal 和 Problems 不等于应用已有能力。这些是本次必须补齐的能力，必须有真实文件、事务与进程支撑；不能因基线缺失而降为可选。

## 复用、重构与重写原则

项目处于 alpha 阶段。复用必须先评估已有模型、状态、功能与代码是否适合 v0.2.0：语义是否正确、架构是否合理、性能是否高效、复杂度是否必要、能否支撑三模式与新开发能力。不能因为“已经存在”就沿用，也不能为了最小 diff 把新设计塞进不合适的旧抽象。

合理且高效的实现直接复用；局部问题通过重构解决；模型、状态所有权或算法从根本上不适配时，允许并应当重写对应模块，必要时跨模块调整边界。无需为了 alpha 内部 API、类型名、旧状态结构或实现细节保留兼容层。删除被替代的旧代码，避免双轨状态和重复逻辑。

保留的是有效产品能力、用户数据与正确性约束，不是旧实现本身。优化或重写应有具体问题、目标和验证依据；涉及性能用有代表性的工作负载测量，涉及文档与状态用行为回归证明。分阶段交付可独立验证的改动，不以“允许重写”为由进行无目标的全仓重写。

## 模式体验

| 模式 | 默认注意力 | 按需能力 | 保持的状态 |
| --- | --- | --- | --- |
| Writing | 居中的正文，关闭导航、Inspector、底部工具 | 文件导航、格式命令、Focus、Typewriter、悬浮元数据 | 同一 session、光标、选区、滚动、保存与恢复 |
| Build | 文档源码与项目导航 | 真实文件标签、Astro 源码编辑、诊断和交互终端；不自动展开所有 Dock | 同一文档身份与历史，不重新读取磁盘覆盖 buffer |
| Preview | 编辑区和真实预览操作 | External 使用紧凑状态与打开浏览器；可用的 Embedded 才分屏 | 同一 dev server、路由和编辑内容 |

新旧用户统一使用 mock 对应的新 UI，默认进入 Writing。旧 palette、serif 默认排版、旧 Dock 布局和旧 chrome 不作为兼容外观保留。迁移仍需保留文档、恢复记录、项目配置和有效功能设置；旧外观配置不再覆盖新设计。Inspector 每次启动关闭。

## 原型使用边界

主要参考在 `astrolith-web-mock/src/App.tsx`、`src/index.css` 和 `src/components/`。该目录是视觉与交互参考，不是应用的运行依赖。

视觉应尽量还原原型：中性表面、居中模式切换器、导航、正文排版、控件轮廓、圆角、留白、分组浮层和设置。不以旧 UI 风格或现有组件默认样式为理由降低还原度。需要修正的原型行为：

- 原型切换模式会换演示文件名；原生应用必须保持当前文档。
- Focus 为固定段落 opacity，Preview 为静态 HTML，Terminal 为硬编码文本，浏览器按钮为 alert。原生实现必须读取真实状态。
- 原型默认 light、embedded；产品先完善 dark，并保持 Linux external 工作流。
- 深色 `--sidebar-primary` 仍为蓝色；按本次中性设计要求纠正，见 [DESIGN_SYSTEM.md](DESIGN_SYSTEM.md)。
- 原型的 Esc 同时关闭设置和 Inspector；原生应用只关闭最上层，并恢复焦点。

## 不可牺牲的边界

正文与元数据必须共享完整源码权威状态，保持未知字段、注释、引号、CRLF 和隐藏 MDX imports。禁止以一次完整 YAML 重序列化换取 UI 简单。

设置、日志、错误提示、路径安全、异步代际校验、保存失败证据和恢复入口属于产品能力。简化界面不能删除它们。

Linux 原生 Wayland 不为 WebView 切换到 XWayland。macOS/Windows 当前仅有代码路径，不能因编译成功宣称平台支持。无原生 IME、焦点、缩放、绘制证据时，验收仍待完成。

## 交付顺序

先统一 mock tokens 与原生控件，再建立共享布局模式和完整源事务；补齐 Astro 源码编辑、交互终端、写作与浮动 Inspector，再交付真实 Embedded/External 预览，最后完成持久化与平台回归。具体依赖、风险和退出条件见 [MILESTONES.md](MILESTONES.md)；验收步骤见 [ACCEPTANCE.md](ACCEPTANCE.md)。
