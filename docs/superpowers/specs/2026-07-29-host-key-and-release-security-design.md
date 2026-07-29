# 主机密钥校验与发布打包设计

日期：2026-07-29

## 目标

解决审计中优先级最高的两个问题：

1. SSH 终端和 SFTP 连接当前会接受任意服务器主机密钥。
2. Cargo 构建的二进制文件名为 `jshell`，但 Release 工作流打包的是 `ashell`。

## 范围

本次修改将为所有 SSH 传输增加严格的 OpenSSH `known_hosts` 校验，并让发布打包流程使用现有 Cargo 二进制名称。本次不增加首次连接信任对话框，不修改凭据存储方式，不重命名产品，也不处理审计中的其他低优先级问题。

## 主机密钥校验

### 安全策略

仅当服务器提供的公钥与 `<用户目录>/.ssh/known_hosts` 中对应主机和端口的记录匹配时，才允许建立连接。

- 密钥匹配时允许连接。
- 主机未知时拒绝连接。
- 主机密钥发生变化时拒绝连接，并返回可诊断错误。
- `known_hosts` 文件无法读取或内容格式错误时拒绝连接，并返回可诊断错误。
- 端口 22 使用标准主机名格式；非默认端口通过 `russh_keys` 使用 OpenSSH 的 `[host]:port` 格式。

此策略有意采用严格校验，而不是首次使用时自动信任（TOFU）。用户可以在独立核对指纹后，通过 OpenSSH 或 `ssh-keyscan` 填充 `known_hosts` 文件。

### 架构

新增共享模块 `session::host_keys`，统一负责主机密钥校验。该模块将：

- 在 Windows、macOS 和 Linux 上统一解析 `<用户目录>/.ssh/known_hosts`；
- 使用 `russh_keys::check_known_hosts_path` 处理文件解析、哈希主机名匹配、密钥比较和非默认端口；
- 对未知主机、密钥变化和解析失败返回明确错误，不静默接受。

两个 SSH Handler 都携带目标主机和端口：

- `backend::ssh::ClientHandler`
- `sftp::SftpClientHandler`

二者的 `check_server_key` 实现调用同一个共享校验函数，确保终端 SSH 与 SFTP 不会产生不同的安全行为。

### 错误流程

`russh` 会在身份认证之前调用 Handler。主机密钥校验失败会立即中止密钥交换，因此密码、私钥签名和 SFTP 请求都不会发送到不受信任的端点。错误继续通过现有连接错误链传递到当前状态或错误界面。

## 发布打包

内部可执行文件名继续使用 `jshell`，与 `Cargo.toml`、`build.rs`、Linux 桌面文件、Debian 元数据和 macOS 打包脚本保持一致。

Release 工作流将：

- Windows 平台复制 `jshell.exe`；
- Linux 和 macOS 平台复制 `jshell`；
- 将 `CFBundleExecutable` 设置为 `jshell`；
- 保留压缩包名称和外层 `ashell.app` 名称，以兼容现有发布 URL 和 Homebrew Cask 元数据。

## 测试

主机密钥测试使用临时 `known_hosts` 文件和真实的 `ssh_key::PublicKey`，覆盖以下行为：

- 主机和密钥匹配时允许连接；
- 未知主机被拒绝；
- 密钥发生变化时返回错误；
- 非默认端口使用正确的 OpenSSH 主机格式。

发布配置通过 Cargo 元数据检查，并确认工作流中的二进制引用与 `jshell` 目标一致。完整验证命令为：

```text
cargo fmt --check
cargo test --quiet
cargo check --all-targets
cargo build --release
```

在 Windows 上执行 Release 构建前，需要停止当前正在运行的 `target/release/jshell.exe`，否则 Cargo 无法覆盖该文件。

## 非目标

- 不自动修改用户的 `known_hosts` 文件。
- 不增加跳过校验的开关或不安全兼容模式。
- 不增加主机指纹确认界面。
- 不对 JShell 或 Ashell 进行产品重命名。
- 不修改配置加密或配置文件原子写入逻辑。
