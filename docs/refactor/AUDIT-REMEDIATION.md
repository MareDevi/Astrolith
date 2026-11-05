# 审计整改结果（2026-11-05）

基线：`dev` @ `326eeec`。审计文档见 `docs/audits/`。本文件只记录**实际完成**的变更、实测数据与明确未完成项，不重复审计论证。

## 1. 数据安全（Phase 2a）

| 项 | 结果 | 关键提交 |
|---|---|---|
| DOC-01 自身保存误报“磁盘已更改” | 保存/重基后同步 `disk_fingerprint` 并清除外部更改标记 | `ee9d3f4` |
| DOC-03 恢复记录采用当前磁盘为基线 | 记录可选基线 + 恢复原修订号；磁盘比记录新时进入冲突，首次自动保存不会覆盖 | `dc8dacf` |
| DOC-04 全局单槽 `pending_after_save` | 改为按规范化路径的映射，不同文档的排队操作不再互相丢弃 | `66d27be` |
| DOC-05 超过 8 MiB 仍可保存 | 统一 `MAX_DOCUMENT_BYTES`，写入前返回 `DocumentError::TooLarge` | `70ef922` |
| DOC-02 冲突无出口 | 新增“从磁盘重载 / 覆盖磁盘 / 另存副本”三个命令与可用性状态；恢复记录只在明确决定后清除 | `d37a3a7` |

## 2. 配置清理（Phase 1）

- 删除 `settings/migrate.rs` 与七个一次性迁移、六个不可达 `!writable` 分支、`interface_scale` 墓碑和七个迁移测试。
- 新增唯一 `settings/upgrade.rs`：全局版本 3、项目版本 2；只在 `raw["version"] < CURRENT` 时运行；一次升级一个备份；只把仍等于旧默认值的字段重定基线；未知字段与未来版本只读语义保留。
- 删除 `telemetry_enabled`、项目文件 `display_name`/`last_opened_at`；默认值单源；父目录 fsync；超大配置按损坏隔离；`Reset All` 清理全部备份/隔离文件。
- 净删除约 800 行生产代码（`d8609c1`、`02c63ba`、`606b3ae`）。

## 3. 性能（Phase 2b/3）

### 实测（release，headless，同一机器）

| 指标 | 审计基线 | 整改后 |
|---|---:|---:|
| 写作者编辑 p50 @ 1 KiB | 0.851 ms | **0.599 ms** |
| 写作者编辑 p50 @ 100 KiB | 2.433 ms | **2.036 ms** |
| 写作者编辑 p50 @ 1 MiB | 16.045 ms | **13.247 ms** |
| 写作者编辑 p50 @ 100 KiB 单行 | 31.061 ms | **25.627 ms** |
| Shell 打开首帧 @ 1 MiB | 56.580 ms | **42.996 ms** |
| 1.3 MB WYSIWYG 窗口缩放 90 帧 | 33 分钟未完成（>22 s/帧下界） | **avg 3.455 ms / p95 3.627 ms / worst 4.446 ms，总计 16.73 s** |
| 投影 diff p50 @ 1 MiB | 0.0725 ms | 0.0685 ms |

### 已落地

- **写作者视口窗口化**（`a684a22`）：按测量高度 + 确定性估算做 O(blocks) 前缀，仅渲染可见块 ± 过扫描，顶部/底部占位保持滚动范围；光标块恒渲染；块数 ≤64 或几何未知时回退全量。
- **每次编辑管线**（`270443e`）：无分配 `token_summary`；单次映射 + `DocumentBuffer::splice_text`，保留整文档回退与 `refused` 锁存；仅在真实文本变化时重算计数/指纹。
- **Shell 渲染瘦身**（`f0aa8d4`）：菜单改由变更处理器置脏；阅读文档按 `(path, kind, revision)` 同步；`has_open_tabs()` 取代每帧标签列表克隆。
- **低风险项**：`ddb0d41`（阅读体不再每帧克隆）、`b649706`（字体/ProseStyle 缓存）、`f17d5ca`（列表计数与搜索/集合摘要缓存）、`11ed963`（监视批次按 mtime+len 复用指纹）、`366c3c8`（预览就绪增量扫描 + 真实 TCP 探测）、`81130af`（schema 轮询改为监视器驱动）、`cb357e0`（监视器代际守卫）、`af806ba`（准入预算计入真实保留字节）、`6d36779`（索引每项一次 canonicalize/stat/解析）、`790f503`（回收 xdg-open 子进程）、`4e8560d`（release LTO/codegen/strip）。
- 终端：进程组信号与非阻塞回收（`2dc59af`）、主题等宽字体实测单元格（`e0b7968`）、可见时 16 ms / 隐藏时 250 ms（`6ca10a0`）、单元格合并绘制（`7c92800`）、DECCKM/括号粘贴（`2f59a76`）、锁外屏幕快照（`ae47fc5`）、视图语义与焦点交接（`94db3c2`）。

说明：1 MiB 编辑仍未达到审计提出的 ≤4 ms 目标。剩余成本来自固定编辑器自身的输入/渲染与焦点/散文装饰重建（约 5.7 ms），复制模式已消除；进一步下降需要增量 Markdown 分析，超出本次范围。`PERF-06`（隐藏编辑器）与 `PERF-08`（文件树增量行）经评估保留，原因分别为焦点/IME 依赖与选择重映射风险。

## 4. Build Mode

- **诊断按 URI/版本过滤**（`b103eeb`）：其他文件的 publish 不再画到当前文件；旧版本不覆盖新版本。
- **优雅关闭**（`80929d9`）：`didClose` 后发送 `shutdown` 并保留 2 s 宽限，不再直接 `SIGKILL`。
- **能力门控**（`c501978`）：只安装服务器声明支持的 completion/hover/definition/codeAction。
- **失败可见 + 一次重试**（`5f1fb6e`）：会话可见状态与通知；每次附加最多重试一次。
- **请求合并**（`6553b38`/`d1e20ff`）：输入补全 70 ms 去抖，显式触发不合并。
- **客户端协议**（`64b6780`/`c1fdae8`）：10 s 超时、`$/cancelRequest`、pending 回收、URI 规范化。
- **每项目每语言一个服务器**（`907ed1e`）：新增 `project_discovery/lsp.rs`；标签切换只 `didOpen`/`didClose`；项目切换/退出按 2 s 宽限关闭一次。测试证明两个 `.ts` 标签只启动 1 个进程、三次标签激活共 3 次 `didOpen`/2 次 `didClose`。

## 5. 架构重构

- `project_discovery.rs` 从 14,952 行降到 8,876 行（其中测试 4,278），按纯移动拆出 `recovery`、`load`、`save`、`watch`、`entry_operations` 五个模块（`f0c06ed`…`8e268c8`），行为与测试集合不变。
- 布局权威从三份收敛为“全局 `settings.layouts` + 项目稀疏覆盖”两份，删除 `Shell.global_layouts` 与手工 `persist_global` 守卫（`940881b`），新增作用域隔离回归测试。
- 状态栏首次渲染 `file_status`（`8473aaa`），打开/扫描/构建器错误不再静默。

## 6. 文档、CI、依赖

- 修正审计列出的 11 处文档矛盾（`92ae383`、`24e4a18`）。
- 条件编译 POSIX-only 测试夹具，使 Windows 目标可编译（`aad8dcb`、`92a0ddc`）。
- 移除未使用的直接依赖 `lsp: serde`、`terminal: tracing`（`9bed44f`）。
- M00 spike 仅在开发构建编译（`3b36c86`）；release 签名只签客户端消费的 AppImage（`e5a8f0e`）；发布构建注入提交派生 `ASTROLITH_BUILD_ID`（`a52fa63`）。
- `test:editor-perf` 改为开发配置运行（spike 已按 FEAT-10 仅开发构建）。

## 7. 明确未完成

| 项 | 原因 |
|---|---|
| ARCH-02 中央文档面板单一写入者 | 需要把“标题 + 编辑器实体 + wiring”从 `ProjectWorkspace` 反转为事件，覆盖所有打开/重载/恢复/测试路径；P2 且回归面大，未在本次落地。 |
| BM-33 终端↔预览桥 | 需要产品决策（终端内启动 dev server，或预览采用终端端口/URL）；涉及两套进程监督模型，未在本次落地。 |
| BM-34/35/36、WS-03/05/06/10 | 预览离屏停止、单次文件系统遍历、按集合诊断、共享忽略规则、自定义路由、`astro sync` 进程组等 P2 项未完成。 |
| FEAT-01 原生菜单 | 按审计允许的方案，文档明确 Linux 无原生菜单栏，工具栏下拉/命令面板/快捷键为规范路径；未实现窗口内菜单栏。 |
| FEAT-02 `embedded-webview` 构建任务 | 未新增 gated 构建/CI 条目；文档已限定为开发者可选（X11）。 |
| PERF-06 / PERF-08 | 见 §3 说明。 |
| BM-10/12/13/14、BM-29 | 跨文件定义列坐标、`workspace/configuration`、帧拷贝、增量 `didChange`、滚动回看选择等 P2/P3 项未完成。 |

## 8. 验证

```
mise run check        # doctor/fmt/clippy/build/test/fixtures，通过
cargo test -p astrolith-gpui --locked --lib   # 442 passed, 4 ignored（最后一次全量）
mise run test:projection-perf:release
mise run test:shell-perf:release
mise run test:resize-perf:release
mise run test:editor-perf                    # 开发配置，数值不与 release 基线直接比较
```

项目与测试清单、审计基线日志保留在 `artifacts/e2e/audit-baseline-326eeec/`。
