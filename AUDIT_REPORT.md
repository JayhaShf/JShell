# JShell v1.0.0 全项目审计报告

- 审计日期：2026-08-19
- 审计对象：当前完整工作树（应用代码、测试、依赖锁文件、配置迁移、同步、SSH/SFTP、文档编辑、IPC、打包脚本与 GitHub Actions）
- 本地环境：Linux x86_64，Rust 1.97.1；Windows 与 macOS 由原生 CI 矩阵负责
- 历史报告：[`docs/RELEASE_AUDIT_0.1.0-beta.2.md`](docs/RELEASE_AUDIT_0.1.0-beta.2.md)

## 1. 结论

本轮不是托盘专项复核，而是对 v1.0.0 当前候选工作树的全范围审计。审计覆盖功能正确性、异步生命周期、并发与状态代际、文件原子性、凭据与加密、网络边界、输入限制、依赖安全、跨平台构建和发布产物。

结论如下：

- 未发现未处理的 P0 代码缺陷；本轮确认的可直接修复 P1/P2 问题均已修复并加入回归测试或发布门禁。
- Debug 与 release 两套测试各 442 项全部通过，Clippy 在 `-D warnings` 下通过。
- Linux 安全存储可用和不可用两种真实启动冒烟均通过；双实例、SIGTERM、SIGINT、配置持久化/禁止持久化和残留进程均已验证。
- 依赖审计无未豁免漏洞。锁文件仍有 9 条 `unmaintained` 和 1 条 `unsound` 告警，均逐项记录在 `.cargo/audit.toml`；其中 `RUSTSEC-2024-0429` 是不能表述为“无内存安全风险”的 Linux GTK3/glib 残余风险。
- 提交 `2e36846` 对应的 GitHub Actions run `32201408633` 全部通过：质量门、Rust 1.95 MSRV、Linux x86_64、Windows x86_64、macOS aarch64 和 macOS x86_64 交叉构建均成功；其中 Linux 原生测试 442/442、Windows 原生测试 441/441、macOS aarch64 原生测试 441/441。

## 2. 审计范围与方法

### 2.1 代码与业务路径

- 启动、日志、托盘、单实例、信号退出和配置写入协调
- SSH、SFTP、代理、主机密钥、串口和本地终端
- SFTP 浏览、上传、下载、删除、归档和远程命令
- 远程文档读取、冲突检测、原子保存和窗口状态
- WebDAV/S3/R2 同步、凭据存储、加密载荷和冲突覆盖
- 配置导入导出、旧格式迁移、大小限制和文件权限
- 终端/SFTP 事件 generation 过滤、重连和中断清理
- Cargo 依赖、许可证、Linux 打包、四平台发布产物

### 2.2 检查方法

- 逐文件审阅当前变更及直接调用链
- 搜索 `unsafe`、进程执行、文件删除、权限修改、panic/unwrap 和凭据形态字符串
- 检查异步操作是否有截止时间、取消、输出/输入上限和终态事件
- 检查临时文件提交、失败回滚、配置/凭据一致性和陈旧事件隔离
- 执行 Debug/release 逻辑测试、静态检查、依赖审计、Actions lint 和 Linux GUI 冒烟

## 3. 已修复问题

| 优先级 | 范围 | 问题 | 修复与验证 |
|---|---|---|---|
| P1 | SFTP 生命周期 | 初始 TCP/SSH 认证、channel、subsystem 和 SFTP handshake 可无限等待；关闭最多等 supervisor 2 秒后强制 abort；文档/传输辅助 channel 也缺少 setup 上限 | 主连接和辅助 subsystem 均增加统一 30 秒 setup deadline；主 setup 期间 `Close` 立即取消，其他命令保序；成功完成全部 setup 后才发布 ready。新增取消、超时、backoff 和保序测试 |
| P1 | SFTP 断线 | 仅检查 SSH handle 是否关闭，主 SFTP subsystem 单独失效时可能一直显示已连接 | 定时执行有界的 `canonicalize(".")` 主 subsystem 探测，失败进入既有 generation/backoff 重连 |
| P1 | 远程命令 | 300 秒超时只覆盖输出等待，不覆盖 channel open 和 `exec` | 超时覆盖完整 open/exec/read/exit-status 操作；输出继续限制为 256 KiB，无 exit status 时 fail closed |
| P2 | SSH 生命周期 | setup 收到关闭后返回成功但不发送终态 `Closed` | 统一终态事件发送，初始化取消返回 `ssh session closed`，错误路径和正常退出各只发送一次；新增事件测试 |
| P1 | SSH 辅助任务 | 系统指标/历史命令 probe 可无限等待或积累无界输出，陈旧结果可能写入新 generation | 增加 30 秒 deadline、64 KiB/既有历史大小上限、JoinSet 回收和 generation 校验 |
| P1 | 事件代际 | 重连后旧 SSH/SFTP/系统采样事件和中断删除标记可能影响新连接 | 后端事件补齐 generation，UI 仅接受当前代际；generation 前进时释放中断状态并补逻辑测试 |
| P1 | 主机密钥 | 多算法记录、同主机旧 key、无关损坏行和并发首次写入存在误拒绝或覆盖风险 | 精确 key 优先；目标相关损坏 fail closed、无关损坏隔离；known_hosts 写入加锁、原子化和权限控制；扩充 exact/hashed/wildcard/revoked/并发测试 |
| P1 | 配置安全 | 配置文件无严格读取上限，原子替换和 v1→v2 迁移失败语义不足；密钥不可用时存在错误恢复风险 | 读取前执行大小限制；同目录临时文件、flush/sync、原子替换、0600；安全迁移失败保留源文件并 fail closed；v2 缺 key 不降级 |
| P1 | 同步传输 | 同步 endpoint/redirect 边界、条件覆盖和大响应处理不够严格 | 生产 endpoint 强制 HTTPS；测试 HTTP 仅允许 IP loopback；重定向保持同源且禁止降级；下载流式限制 8 MiB；WebDAV/S3 条件写入与冲突语义补齐 |
| P1 | 同步凭据 | 表单变化、失败保存和多凭据部分写入可能留下错误预览或半提交 keyring 状态 | 操作绑定不可变 target snapshot；失败时回滚旧凭据/配置；错误信息脱敏；补失败注入和 target 变更测试 |
| P2 | 导入导出 | 导出可留下部分文件，导入在解析后才限制大小，保存失败会污染缓存 | 导出同目录原子替换并设私有权限；导入先限长再解析；失败恢复原缓存和磁盘状态 |
| P1 | 远程文档 | 仅依赖 size/mtime 的冲突检测会漏掉同大小同时间内容变化；读取后增长可能越过可编辑上限 | 增加内容 hash/二次大小验证、结果未知状态和原子远端替换；补同元数据变化与增长测试 |
| P2 | 文档 UI | 关闭非活动文档可能错误移动当前焦点 | 按关闭位置计算保留焦点并加入布局回归测试 |
| P1 | SFTP 文件操作 | 本地下载替换、远端上传提交、失败清理和中断删除结果存在不完整状态风险 | 临时路径 + 提交/备份/回滚；已有目标不被失败提交破坏；中断删除统一回收；路径和输出限制测试 |
| P1 | 单实例 IPC | 握手无 ACK/截止时间，监听连接可耗尽线程；同机多用户身份边界不足 | 双向 ACK、500 ms 非阻塞 I/O deadline、并发连接上限、Unix peer UID 校验和 UID 隔离 socket；新增停滞/并发/ACK 测试 |
| P2 | 启动兼容 | Unix 本地终端只依赖 `$SHELL`；macOS 登录 shell 环境读取可卡住；日志目录权限不明确 | `$SHELL`→账户 shell→平台默认回退；macOS 环境读取 3 秒超时；Unix 日志目录 0700 |
| P2 | 依赖与打包 | 重复旧版 `notify` 依赖、许可证未完整进入产物、Linux runtime 依赖声明不一致 | 移除直接旧版 notify；四平台产物包含 GPL 和字体许可证；Linux 包显式依赖 appindicator runtime |
| P2 | 多系统 CI | 平台矩阵只构建不测试，macOS runner 即将弃用，macOS SIGTERM 超时会强杀后假通过 | 原生 Windows/Linux/macOS aarch64 跑测试，x86_64 macOS 交叉编译测试；迁移 `macos-15`；SIGTERM 超时/非零退出均失败；`lipo` 校验架构 |
| P2 | Linux 冒烟 | keyring runtime 未隔离，全局 `pgrep -x jshell` 会误报/干扰其他实例；隔离 runtime 与 Wayland socket 冲突 | 隔离 `HOME`/`XDG_RUNTIME_DIR`/DBus/keyring，使用 X11/Xvfb；只检查本次启动 PID；portal 清理有界重试；keyring/fallback 两种模式实测通过 |

## 4. 功能与逻辑测试结果

| 检查 | 最终结果 |
|---|---|
| `cargo fmt --check` | 通过 |
| `cargo test --locked` | 442 passed，0 failed |
| `cargo test --locked --release` | 442 passed，0 failed |
| `cargo check --locked --all-targets` | 通过 |
| `cargo clippy --locked --all-targets --all-features -- -D warnings` | 通过，0 warning |
| `cargo build --locked --release` | 通过 |
| `cargo audit --file Cargo.lock --db /home/jay/.cargo/advisory-db --no-fetch --deny warnings` | 通过；扫描 1063 个锁定依赖，无未豁免告警 |
| `actionlint v1.7.12` | `ci.yml`、`release.yml` 通过 |
| `bash -n scripts/*.sh` | 通过 |
| `git diff --check` | 通过 |
| 工作区凭据形态扫描 | 未发现匹配的凭据字符串 |

新增/重点逻辑覆盖包括：

- SSH/SFTP setup 取消、统一超时、命令保序、重试分类和终态事件
- 配置大小边界、加密迁移、原子替换、权限和失败回滚
- host key 多算法、hashed/wildcard/revoked、损坏输入和并发写入
- 同步 HTTPS/loopback、同源 redirect、ETag/If-Match/If-None-Match 和流式大小上限
- 文档内容 hash 冲突、读取后增长、未知写入结果和焦点保持
- 单实例 ACK、停滞连接 deadline、peer UID 和并发连接上限
- SFTP 临时文件提交、已有目标保护、输出上限和危险归档条目
- locale 键一致性、终端 URL/状态/IP/端口解析和 generation 过滤

## 5. Linux 功能冒烟

使用 `target/release/jshell` 和隔离目录执行两轮真实 GUI 冒烟，每轮 11 项检查：

| 场景 | 结果 |
|---|---|
| 隔离 Secret Service/keyring 可用 | 启动存活、无 panic、双实例、SIGTERM、SIGINT、退出码 0、配置持久化、安全存储可用、无本次测试残留进程，全部通过 |
| DBus/keyring 故意不可用 | 启动存活、无 panic、双实例、SIGTERM、SIGINT、退出码 0、配置不落盘、记录 secure-storage failure、无本次测试残留进程，全部通过 |

冒烟使用隔离 `HOME`，不会读取或覆盖真实用户配置。keyring 场景还隔离 DBus、keyring control 和 `XDG_RUNTIME_DIR`。

## 6. 多系统兼容性

| 平台 | 自动化策略 | 本轮本地结论 |
|---|---|---|
| Linux x86_64 | Ubuntu 22.04 原生 test/build；Xvfb 下 keyring/fallback 冒烟；appindicator runtime；release 包许可证检查 | 当前主机已实际通过两种 GUI 冒烟和完整 Rust 门禁 |
| Windows x86_64 | 原生 `cargo test` + release build；发布工作流另行执行解包、许可证和 5 秒启动存活检查 | CI run `32201408633` 原生测试 441/441、release build 通过 |
| macOS aarch64 | `macos-15` 原生 test/build；发布工作流另行执行 bundle/plist/codesign/license/lipo、启动及 SIGTERM 检查 | CI run `32201408633` 原生测试 441/441、release build 通过 |
| macOS x86_64 | `macos-15` 上交叉 `cargo test --no-run` + build；发布工作流另行执行 bundle/codesign/license/lipo | CI run `32201408633` 交叉测试编译和 release build 通过；未执行 x86_64 原生测试，Intel 实机运行仍是发布前边界 |

声明的 MSRV 为 Rust 1.95.0。当前 pinned GPUI revision 同时使用 `slice::as_array` 和 `std::hint::cold_path`；前者在 Rust 1.93.0 稳定，后者在 Rust 1.95.0 稳定，因此项目最低版本为 Rust 1.95.0。CI 通过独立 Ubuntu 22.04 MSRV `cargo check --all-targets` job 固定验证此最低版本。当前主机没有 rustup，因此未在本地重复安装 1.95.0。

## 7. 依赖安全与残余风险

`.cargo/audit.toml` 当前精确豁免：

- 9 条 `unmaintained`：GTK3 绑定链 7 条、`paste` 1 条、`proc-macro-error` 1 条。
- 1 条 `unsound`：`RUSTSEC-2024-0429`，glib 0.18 `VariantStrIter` 相关未定义行为，修复版本为 glib 0.20。

项目源码未直接调用该迭代 API，但 Linux 托盘的 GTK3 传递依赖仍包含受影响 glib，因此风险不能写成零。Windows/macOS 不引入这条 Linux GTK3 托盘链。彻底消除需要迁移 Linux 托盘后端或等待依赖链升级；新增任何未列入 ignore 清单的 advisory 仍会使 CI 失败。

仍需产品或外部环境决策的边界：

- SSH 首次连接采用自动 TOFU，尚无指纹确认 UI；首次连接遭中间人攻击的风险需由产品决策处理。
- 旧 v1 配置迁移仍保留硬件 ID 获取失败时的兼容兜底，仅用于读取旧数据；贸然删除会使旧用户数据不可迁移。
- 未提供真实非生产 SSH/SFTP、WebDAV/S3/R2 账户，本轮不能验证公网服务端差异、权限模型和断网时序。
- Windows 关机的 `WM_ENDSESSION`、macOS 公证和 Windows 代码签名尚未实现；不影响本地逻辑正确性，但影响系统关机保存和分发体验。
- Linux Wayland 最小化和 appindicator 左键行为仍受桌面环境/上游限制。

## 8. 发布判定

当前 Linux 本地验证和四平台 CI 门禁均通过。发布前仍必须满足：

1. 已完成：推送提交 `2e36846` 并确认 GitHub Actions run `32201408633` 的 quality、MSRV、Windows、Linux、macOS aarch64/x86_64 全部通过。
2. 在非生产 SSH/SFTP 主机执行连接、权限、上传/下载、删除、静默断线重连和远程编辑冲突测试。
3. 有测试凭据时执行真实 WebDAV/S3/R2 条件覆盖与冲突测试。
4. 明确接受第 7 节 TOFU、glib unsound、签名/公证和平台 UI 限制后再发布。
