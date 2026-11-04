# 并发与响应性审计入口

`astrolith-gpui/src/runtime.rs` 当前建立 2 个 Tokio workers、最多 4 个 blocking threads，以 semaphore 限制同时 2 个 blocking jobs。`runtime::spawn` 将服务 future 桥接至 GPUI Task；释放 owner task 时取消服务任务。已经开始的 blocking 操作不能被立即打断，permit 保持到工作结束。

项目发现/监听、保存、schema 刷新和 Copyedit 使用身份、revision 或 generation 拒绝过期结果。模式切换应保留这些 owner，不从 render 创建服务、重复 I/O 或重建 session。

结构与风险见 [ARCHITECTURE](../refactor/ARCHITECTURE.md)，性能工作负载入口在 `mise.toml`。headless CPU workload 不能证明原生输入、窗口缩放或帧延迟验收，证据要求见 [ACCEPTANCE](../refactor/ACCEPTANCE.md)。
