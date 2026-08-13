# JShell 项目审计报告

- 审计日期：2026-08-13
- 审计对象：工作树中未提交的系统托盘 + 后台运行、单实例锁、信号优雅退出三组改动
- 审计范围：`src/app/tray.rs`、`src/app/single_instance.rs`、`src/app/signals.rs`（全部新增）以及 `src/app/mod.rs`、`src/app/startup.rs`、`src/app/dialogs.rs`、`src/main.rs`、`src/session/config.rs`、`locales/*.yml`、`Cargo.toml/Cargo.lock` 的关联修改，及这些改动与既有关闭/退出流程、配置写入协调器、同步合并逻辑的交互
- 范围说明：按惯例只复审本次改动及其直接依赖（tray-icon/muda、interprocess、signal-hook、gtk），未重新审计未改动的全部业务模块
- 历史报告：上一轮 `0.1.0-beta.2` 发布候选审计记录保留在 [docs/RELEASE_AUDIT_0.1.0-beta.2.md](docs/RELEASE_AUDIT_0.1.0-beta.2.md)

## 1. 审计结论

本次审计共发现 5 处问题，全部已修复并复验；当前无未处理的 P0/P1/P2 问题。核心结论：

- **无内存安全问题**：事件泵在 `remove_window()` 之后继续访问实体是安全的——该 fork 的 `Window::remove_window` 只置 `removed` 标记，窗口与实体在本次 update 闭包返回后才销毁（`app.rs` `trail()`）；泵循环下一次迭代 `update_in` 报错即退出，无 use-after-drop。
- **无死锁路径**：全局 `REQUEST_TX` 互斥锁仅覆盖一次非阻塞 `mpsc::send`；Linux 托盘线程 Drop 时置标志后 join，最多等待一个泵间隔（16ms），且与锁无嵌套。
- **修复了 1 个功能性缺陷（P1）**：首次信号后信号处理器未恢复默认处置，若优雅退出卡死（如等待保存提示），后续 SIGTERM/SIGINT 将无法强制终止进程。
- **修复了 1 个多用户正确性问题（P2）**：Linux 抽象命名空间套接字与 macOS `/tmp` 路径对全系统可见，多用户同机时第二个用户永远无法启动（连接会命中他人实例并退出）；现按 UID 隔离。
- 依赖审计 0 漏洞；新增依赖仅引入 8 条 GTK3 绑定"不再维护"告警（Linux 专用，Windows/macOS 构建不引入），属警告级且为托盘生态事实标准栈。

## 2. 发现与修复

### 2.1 P1：信号处理器未关闭，二次信号无法强制退出

- `signals.rs` 原实现 `for signal in signals.forever() { ...; break; }` 除触发 clippy `never_loop`（deny 级，编译失败）外，`Signals` 迭代器析构不会自动注销处理器。
- 修复：取首个信号 → 转发退出请求 → `handle.close()` 恢复默认处置。此后第一个信号优雅退出，第二个信号恢复默认行为可强制杀死，符合"优雅优先、可逃生"语义。

### 2.2 P2：单实例套接字跨用户冲突

- Linux 抽象套接字无权限隔离，macOS 落盘于共享 `/tmp`：用户 A 的实例会让用户 B 的每次启动误判为"第二实例"并直接退出。
- 修复：Unix 平台套接字名追加 `getuid()`（`libc` 提升为正式依赖，版本已在锁文件中）；`remove_stale_socket` 使用同一命名函数。
- Windows 命名管道保留固定名：跨用户连接失败时会走 3 次重试 → 降级无锁运行（仅失去互斥，不会无法启动）。

### 2.3 P2：单实例监听线程创建失败会 panic

- 原实现 `spawn(...).expect(...)`。修复为失败时记日志并释放监听器（套接字随 Drop 释放），本实例降级无锁运行，不影响启动。

### 2.4 P2：事件泵信号分支防重入

- 同一 tick 内托盘"退出"与信号退出可能先后触发两次 `request_application_close`（幂等但冗余）。信号分支增加 `!closing_application` 守卫。

### 2.5 P3：clippy 告警清零

- `tray.rs`：`send_request` 嵌套 if 合并为 let-chain；`MenuItem::with_id` 去除多余 `to_string()`。
- `signals.rs` 的 `never_loop` 随 2.1 一并修复。

## 3. 模块审查结论

| 模块 | 结论 | 要点 |
|---|---|---|
| `tray.rs` | 通过 | 全局事件处理器经 `Once` 只装一次，经可替换的 `REQUEST_TX` 转发（muda 的处理器 OnceLock 只能设一次）；锁毒化容忍（`into_inner`/静默丢弃）；Windows/macOS 在 gpui 主线程创建（macOS NSStatusItem 硬性要求）；Linux 专用线程持有图标并自泵 GTK 事件，Drop 置位 + join 后在 gtk 线程内析构 AppIndicator；关闭转托盘时窗口实体不销毁，会话与文档完整保留 |
| `single_instance.rs` | 通过 | 绑定成功即首实例；绑定失败→连接→写 `activate`→退出（实测第二实例 11ms 内退出）；启动竞态 3 次重试；stale 套接字（macOS）清理；全部失败降级无锁运行 |
| `signals.rs` | 通过 | SIGINT/SIGTERM → 通道 → 事件泵（≤16ms 延迟）→ `closing_application = true`（绕过转托盘拦截，并允许分离窗口关闭）→ 恢复窗口 → 走含未保存提示的完整退出流程 → `save_layout_state` 保存布局（实测退出时配置 mtime 同步更新） |
| `mod.rs` 事件泵 | 通过 | `spawn_in` + `update_in` 使泵闭包同时持有实体与窗口；托盘请求、实例激活、信号退出按序处理；窗口移除后同闭包内继续访问实体安全（延迟销毁，见结论 1）；泵随实体释放自动终止 |
| `startup.rs` 关闭拦截 | 通过 | 托盘存在且非关停状态才最小化；`closing_application` 语义与 `document/window.rs` 既有用法一致（应用关停时允许分离窗口关闭） |
| `config.rs` | 通过 | `system_tray` serde 缺省 true；同步合并本地值优先（与相邻 UI 状态字段一致），已纳入本地字段保护快照测试并设互异值验证不被远端覆盖 |
| `dialogs.rs` 设置开关 | 通过 | 开→按需创建托盘（失败回退并回写 false）；关→销毁托盘（Drop 内 join，主线程阻塞 ≤16ms）；开关在 gpui 主线程执行，满足 macOS 约束 |
| `main.rs` | 通过 | 单实例判定早于应用构建；第二实例在创建任何窗口/托盘前退出（Windows `windows_subsystem` 下无闪烁） |
| locales | 通过 | 5 个新键中英双语齐全 |

## 4. 验证结果

以下均在最终工作树执行：

| 检查 | 结果 |
|---|---|
| `cargo check --all-targets` | 通过，0 警告 |
| `cargo clippy --all-targets` | 通过，0 警告（`-D warnings` 等效） |
| `cargo fmt --all -- --check` | 通过 |
| `cargo test --bin jshell` | 368 passed，0 failed（含新增 `system_tray_defaults_to_enabled_and_serializes` 与同步本地字段保护用例） |
| `cargo audit` | 0 漏洞；9 条告警全部为警告级（8× gtk-rs GTK3 "no longer maintained"，Linux 专用依赖链；1× `proc-macro-error` unmaintained，传递依赖），`cargo audit --deny warnings` 前需在审计配置中豁免 |
| 启动冒烟（Linux，NVIDIA/Vulkan） | 无 panic、无 GTK 错误；托盘创建成功 |
| 双实例冒烟 | 第二实例连接通知后 11ms 内退出（exit 0），首实例保持运行；首实例退出后套接字立即释放，新实例可重新绑定 |
| 信号退出冒烟 | `kill -TERM`/`kill -INT` 均 3 秒内优雅退出，日志记录 `received signal N, shutting down gracefully`；退出时配置落盘（`sessions.json` mtime 更新） |
| 残留进程检查 | 测试后无残留 `target/debug/jshell` 进程 |

## 5. 已知边界与平台风险

- **Wayland 最小化依赖合成器**：`set_minimized` 是请求而非命令，GNOME 忽略、KDE 支持；被忽略时关闭窗口表现为"窗口保持可见"，属安全回退（应用仍在后台运行）。
- **Linux 托盘左键无效**：tray-icon 的 appindicator 实现不发出 Click 事件，属上游 API 限制；Linux 请用托盘菜单"显示/隐藏窗口"。Windows/macOS 左键为"总是恢复窗口"（Windows 端 `activate_window` 自动 `IsIconic→SW_RESTORE`，不受任务栏最小化影响）。
- **GTK3 绑定告警**：tray-icon→libappindicator→gtk-rs GTK3 链整体停止维护，但该链是 Linux 托盘的事实标准实现且仅 Linux 构建引入；Windows/macOS 产物不受影响。若需消除告警需替换托盘后端（如 ksni），暂不处理。
- **未做单实例的跨平台信号（Windows WM_ENDSESSION）**：Windows 无控制台信号，会话结束路径不在本次范围。
- 三个新增常驻线程（`jshell-tray`/`jshell-instance`/`jshell-signals`）均为进程生命周期 daemon 线程，随进程退出终止；`jshell-tray` 在设置中关闭托盘时按需回收。
- 第二轮托盘创建（设置关闭→开启）在 Linux 上会在新线程创建 GTK 对象；gtk 进程级只初始化一次，新线程依赖自身的默认 GMainContext 泵事件，实测可用但属 GTK 文档未承诺的用法，长期运行场景建议重启应用后使用。

## 6. 结论

本次改动（系统托盘 + 后台运行、单实例锁、信号优雅退出）通过门禁：368 项测试、clippy/fmt 零告警、依赖审计零漏洞，三项功能冒烟与两项边界修复（跨用户隔离、二次信号逃生）均复验通过。改动尚未提交，建议按功能分批提交后走既有的 CI/发布流程。
